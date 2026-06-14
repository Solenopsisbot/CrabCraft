//! The networking client: connects, logs in, and runs the play loop, updating
//! shared state ([`Shared`]) that other threads (e.g. the renderer) can read.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use crab_net::Connection;
use crab_protocol::packet::{Packet, State};
use crab_protocol::versions::v1_20_1::handshake::{Handshake, NextState};
use crab_protocol::versions::v1_20_1::login::{
    EncryptionRequest, LoginDisconnect, LoginStart, LoginSuccess, SetCompression,
};
use crab_protocol::versions::v1_20_1::play::{
    ClientChatMessage, ClientInformation, ConfirmTeleport, KeepAlive, KeepAliveResponse,
    PlayDisconnect, SetPlayerPosition, SetPlayerPositionRotation, SynchronizePlayerPosition,
    SystemChat,
};
use crab_protocol::versions::PROTOCOL_1_20_1;
use crab_protocol::BufExt;
use crab_world::{Chunk, World};
use tokio::io::{AsyncRead, AsyncWrite};

// Clientbound Play packet IDs consumed directly (decoded inline / by crab-world).
const ID_JOIN_GAME: i32 = 0x28;
const ID_MAP_CHUNK: i32 = 0x24;
const ID_UNLOAD_CHUNK: i32 = 0x1e;
const ID_BLOCK_CHANGE: i32 = 0x0a;

/// Our current position/orientation as last told by the server.
#[derive(Default, Clone, Copy, Debug)]
pub struct PlayerState {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub spawned: bool,
}

impl PlayerState {
    fn apply(&mut self, p: &SynchronizePlayerPosition) {
        self.x = if p.flags & 0x01 != 0 {
            self.x + p.x
        } else {
            p.x
        };
        self.y = if p.flags & 0x02 != 0 {
            self.y + p.y
        } else {
            p.y
        };
        self.z = if p.flags & 0x04 != 0 {
            self.z + p.z
        } else {
            p.z
        };
        self.yaw = if p.flags & 0x08 != 0 {
            self.yaw + p.yaw
        } else {
            p.yaw
        };
        self.pitch = if p.flags & 0x10 != 0 {
            self.pitch + p.pitch
        } else {
            p.pitch
        };
    }
}

/// State shared between the network task and any reader (the renderer).
#[derive(Debug)]
pub struct Shared {
    pub world: Mutex<World>,
    pub player: Mutex<PlayerState>,
    /// Cleared to `false` when the session ends, so readers can stop.
    pub running: AtomicBool,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            world: Mutex::new(World::overworld()),
            player: Mutex::new(PlayerState::default()),
            running: AtomicBool::new(true),
        }
    }
}

impl Default for Shared {
    fn default() -> Self {
        Self::new()
    }
}

/// Connects to `addr`, logs in as `username` (offline mode), and runs the play
/// loop, updating `shared`. Runs until `deadline` elapses (if given) or the
/// server disconnects us. Always clears `shared.running` on exit.
pub async fn connect_and_play(
    addr: &str,
    username: &str,
    shared: Arc<Shared>,
    deadline: Option<Duration>,
) -> Result<()> {
    let result = run_inner(addr, username, &shared, deadline).await;
    shared.running.store(false, Ordering::SeqCst);
    result
}

async fn run_inner(
    addr: &str,
    username: &str,
    shared: &Arc<Shared>,
    deadline: Option<Duration>,
) -> Result<()> {
    let (host, port) = split_host_port(addr);
    tracing::info!(server = %addr, %username, "connecting");
    let mut conn = Connection::connect(addr)
        .await
        .with_context(|| format!("failed to connect to {addr}"))?;

    conn.send(&Handshake {
        protocol_version: PROTOCOL_1_20_1,
        server_address: host,
        server_port: port,
        next_state: NextState::Login,
    })
    .await?;
    conn.set_state(State::Login);
    conn.send(&LoginStart {
        name: username.to_string(),
        uuid: None,
    })
    .await?;

    // --- Login ---
    loop {
        let raw = conn.read_packet().await.context("reading login packet")?;
        match raw.id {
            id if id == SetCompression::ID => {
                let pkt: SetCompression = raw.decode()?;
                conn.set_compression(pkt.threshold);
            }
            id if id == EncryptionRequest::ID => {
                bail!("server is online-mode (EncryptionRequest); auth not implemented yet");
            }
            id if id == LoginDisconnect::ID => {
                let pkt: LoginDisconnect = raw.decode()?;
                bail!("login refused: {}", pkt.reason_json);
            }
            id if id == LoginSuccess::ID => {
                let pkt: LoginSuccess = raw.decode()?;
                tracing::info!(uuid = %pkt.uuid, name = %pkt.username, "logged in");
                conn.set_state(State::Play);
                break;
            }
            _ => {}
        }
    }

    play_loop(&mut conn, username, shared, deadline).await
}

async fn play_loop<S>(
    conn: &mut Connection<S>,
    username: &str,
    shared: &Arc<Shared>,
    deadline: Option<Duration>,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut greeted = false;
    let mut pos_tick = tokio::time::interval(Duration::from_secs(1));

    let deadline_fut = async {
        match deadline {
            Some(d) => tokio::time::sleep(d).await,
            None => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(deadline_fut);

    loop {
        tokio::select! {
            biased;
            _ = &mut deadline_fut => {
                tracing::info!("session deadline reached");
                break;
            }
            result = conn.read_packet() => {
                let raw = match result {
                    Ok(raw) => raw,
                    Err(e) => { tracing::info!("connection closed: {e}"); break; }
                };
                match raw.id {
                    id if id == KeepAlive::ID => {
                        let k: KeepAlive = raw.decode()?;
                        conn.send(&KeepAliveResponse { id: k.id }).await?;
                    }
                    id if id == SynchronizePlayerPosition::ID => {
                        let p: SynchronizePlayerPosition = raw.decode()?;
                        let (just_spawned, pos) = {
                            let mut ps = shared.player.lock().unwrap();
                            ps.apply(&p);
                            let js = !ps.spawned;
                            ps.spawned = true;
                            (js, *ps)
                        };
                        conn.send(&ConfirmTeleport { teleport_id: p.teleport_id }).await?;
                        conn.send(&SetPlayerPositionRotation {
                            x: pos.x, y: pos.y, z: pos.z, yaw: pos.yaw, pitch: pos.pitch,
                            on_ground: true,
                        }).await?;
                        if just_spawned {
                            tracing::info!("spawned at ({:.1}, {:.1}, {:.1})", pos.x, pos.y, pos.z);
                            conn.send(&ClientInformation::sensible_defaults()).await?;
                        }
                        if !greeted {
                            greeted = true;
                            let msg = format!("{username} here via Crabcraft (pure Rust).");
                            conn.send(&ClientChatMessage::unsigned(msg)).await?;
                        }
                    }
                    id if id == SystemChat::ID => {
                        let c: SystemChat = raw.decode()?;
                        if !c.overlay {
                            tracing::info!(target: "chat", "{}", plain_text(&c.content));
                        }
                    }
                    id if id == ID_JOIN_GAME => {
                        if let Err(e) = handle_join_game(&raw, shared) {
                            tracing::warn!("Join Game parse failed: {e}");
                        }
                    }
                    id if id == ID_MAP_CHUNK => {
                        let mut body = raw.body.clone();
                        let mut world = shared.world.lock().unwrap();
                        let sc = world.section_count();
                        match Chunk::parse(&mut body, sc) {
                            Ok(chunk) => world.load_chunk(chunk),
                            Err(e) => tracing::warn!("chunk parse failed: {e}"),
                        }
                    }
                    id if id == ID_UNLOAD_CHUNK => {
                        let mut body = raw.body.clone();
                        if let (Ok(cx), Ok(cz)) = (body.read_i32(), body.read_i32()) {
                            shared.world.lock().unwrap().unload_chunk(cx, cz);
                        }
                    }
                    id if id == ID_BLOCK_CHANGE => {
                        let mut body = raw.body.clone();
                        if let (Ok((bx, by, bz)), Ok(s)) = (body.read_position(), body.read_varint()) {
                            shared.world.lock().unwrap().set_block_state(bx, by, bz, s as u32);
                        }
                    }
                    id if id == PlayDisconnect::ID => {
                        let d: PlayDisconnect = raw.decode()?;
                        tracing::warn!("disconnected: {}", plain_text(&d.reason_json));
                        break;
                    }
                    _ => {}
                }
            }
            _ = pos_tick.tick() => {
                let pos = { *shared.player.lock().unwrap() };
                if pos.spawned {
                    conn.send(&SetPlayerPosition { x: pos.x, y: pos.y, z: pos.z, on_ground: true }).await?;
                }
            }
        }
    }
    Ok(())
}

fn handle_join_game(raw: &crab_net::RawPacket, shared: &Arc<Shared>) -> Result<()> {
    let mut b = raw.body.clone();
    let _entity_id = b.read_i32()?;
    let _hardcore = b.read_bool()?;
    let _game_mode = b.read_u8()?;
    let _prev_game_mode = b.read_i8()?;
    let world_count = b.read_varint()?.max(0);
    for _ in 0..world_count {
        let _ = b.read_string(32767)?;
    }
    let codec = crab_protocol::nbt::read_nbt(&mut b)?;
    let dimension_type = b.read_string(32767)?;
    if let Some((min_y, height)) = crab_world::dimension_extent(&codec, &dimension_type) {
        *shared.world.lock().unwrap() = World::new(min_y, height);
        tracing::info!("dimension {dimension_type}: min_y={min_y} height={height}");
    }
    Ok(())
}

/// Very small chat-component flattener for readable logging.
fn plain_text(json: &str) -> String {
    if let Some(start) = json.find("\"text\":\"") {
        let rest = &json[start + 8..];
        if let Some(end) = rest.find('"') {
            let text = &rest[..end];
            if !text.is_empty() {
                return text.to_string();
            }
        }
    }
    json.to_string()
}

fn split_host_port(addr: &str) -> (String, u16) {
    match addr.rsplit_once(':') {
        Some((host, port)) => (host.to_string(), port.parse().unwrap_or(25565)),
        None => (addr.to_string(), 25565),
    }
}
