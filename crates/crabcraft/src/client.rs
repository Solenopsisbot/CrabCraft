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
    EncryptionRequest, EncryptionResponse, LoginDisconnect, LoginStart, LoginSuccess,
    SetCompression,
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
    pub on_ground: bool,
    /// Velocity `[x, y, z]` for client-side physics.
    pub vel: [f64; 3],
}

/// Player input intent, written by the renderer and consumed by the net thread.
#[derive(Default, Clone, Copy, Debug)]
pub struct Controls {
    /// Forward/back, -1..=1 (W/S).
    pub forward: f32,
    /// Strafe right/left, -1..=1 (D/A).
    pub strafe: f32,
    /// Jump held (space).
    pub jump: bool,
    /// Look yaw in degrees (Minecraft convention).
    pub yaw: f32,
    /// Look pitch in degrees (positive = down).
    pub pitch: f32,
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
        self.vel = [0.0, 0.0, 0.0]; // a teleport cancels momentum
    }
}

/// State shared between the network task and any reader (the renderer).
#[derive(Debug)]
pub struct Shared {
    pub world: Mutex<World>,
    pub player: Mutex<PlayerState>,
    /// Player input intent (written by the renderer).
    pub controls: Mutex<Controls>,
    /// Cleared to `false` when the session ends, so readers can stop.
    pub running: AtomicBool,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            world: Mutex::new(World::overworld()),
            player: Mutex::new(PlayerState::default()),
            controls: Mutex::new(Controls::default()),
            running: AtomicBool::new(true),
        }
    }
}

impl Default for Shared {
    fn default() -> Self {
        Self::new()
    }
}

/// How to authenticate: offline (just a name) or online (a real account
/// session, enabling the encryption handshake).
pub enum LoginMode {
    Offline { username: String },
    Online(crab_auth::Session),
}

/// Connects to `addr`, logs in per `login`, and runs the play loop, updating
/// `shared`. Runs until `deadline` elapses (if given) or the server
/// disconnects us. Always clears `shared.running` on exit.
pub async fn connect_and_play(
    addr: &str,
    login: LoginMode,
    shared: Arc<Shared>,
    deadline: Option<Duration>,
) -> Result<()> {
    let result = run_inner(addr, &login, &shared, deadline).await;
    shared.running.store(false, Ordering::SeqCst);
    result
}

async fn run_inner(
    addr: &str,
    login: &LoginMode,
    shared: &Arc<Shared>,
    deadline: Option<Duration>,
) -> Result<()> {
    let (host, port) = split_host_port(addr);
    let (name, uuid) = match login {
        LoginMode::Offline { username } => (username.clone(), None),
        LoginMode::Online(session) => (session.username.clone(), Some(session.uuid)),
    };
    let session = match login {
        LoginMode::Online(session) => Some(session),
        LoginMode::Offline { .. } => None,
    };
    tracing::info!(server = %addr, username = %name, online = session.is_some(), "connecting");

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
        name: name.clone(),
        uuid,
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
                let req: EncryptionRequest = raw.decode()?;
                let Some(session) = session else {
                    bail!("server is online-mode but no account session; run in online mode");
                };
                // Negotiate the shared secret, prove session ownership to Mojang,
                // then send the RSA-encrypted response and switch on AES.
                let secret = crab_auth::random_shared_secret();
                let hash = crab_auth::server_hash(&req.server_id, &secret, &req.public_key);
                crab_auth::join_server(&session.access_token, session.uuid, &hash)
                    .await
                    .context("sessionserver join")?;
                let enc_secret = crab_auth::encrypt_to_server(&req.public_key, &secret)?;
                let enc_token = crab_auth::encrypt_to_server(&req.public_key, &req.verify_token)?;
                conn.send(&EncryptionResponse {
                    shared_secret: enc_secret,
                    verify_token: enc_token,
                })
                .await?;
                conn.enable_encryption(secret);
                tracing::info!("encryption enabled (online mode)");
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

    play_loop(&mut conn, &name, shared, deadline).await
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
    // ~20 Hz physics + position updates, like the vanilla client.
    let tick_dt = 0.05;
    let mut pos_tick = tokio::time::interval(Duration::from_secs_f64(tick_dt));

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
                let controls = *shared.controls.lock().unwrap();
                let snapshot = { *shared.player.lock().unwrap() };
                if snapshot.spawned {
                    // Step client physics (input-driven horizontal velocity +
                    // gravity + collision) if our chunk is loaded.
                    let stepped = {
                        let world = shared.world.lock().unwrap();
                        let (fx, fz) = (snapshot.x.floor() as i32, snapshot.z.floor() as i32);
                        world.is_loaded(fx, fz).then(|| {
                            // Horizontal move relative to look yaw.
                            let yaw = f64::from(controls.yaw).to_radians();
                            let (sin, cos) = (yaw.sin(), yaw.cos());
                            // forward = (-sin, cos); right = (-cos, -sin)
                            let mut vx =
                                -sin * f64::from(controls.forward) - cos * f64::from(controls.strafe);
                            let mut vz =
                                cos * f64::from(controls.forward) - sin * f64::from(controls.strafe);
                            let len = (vx * vx + vz * vz).sqrt();
                            const WALK: f64 = 4.317;
                            if len > 1e-6 {
                                vx = vx / len * WALK;
                                vz = vz / len * WALK;
                            } else {
                                vx = 0.0;
                                vz = 0.0;
                            }
                            let mut vel = snapshot.vel;
                            vel[0] = vx;
                            vel[2] = vz;
                            if controls.jump && snapshot.on_ground {
                                vel[1] = 8.5;
                            }
                            crab_physics::step_player(
                                &world,
                                [snapshot.x, snapshot.y, snapshot.z],
                                vel,
                                tick_dt,
                            )
                        })
                    };
                    if let Some(r) = stepped {
                        let (x, y, z) = (r.position[0], r.position[1], r.position[2]);
                        {
                            let mut ps = shared.player.lock().unwrap();
                            ps.x = x;
                            ps.y = y;
                            ps.z = z;
                            ps.vel = r.velocity;
                            ps.on_ground = r.on_ground;
                            ps.yaw = controls.yaw;
                            ps.pitch = controls.pitch;
                        }
                        conn.send(&SetPlayerPositionRotation {
                            x,
                            y,
                            z,
                            yaw: controls.yaw,
                            pitch: controls.pitch,
                            on_ground: r.on_ground,
                        })
                        .await?;
                    } else {
                        conn.send(&SetPlayerPosition {
                            x: snapshot.x,
                            y: snapshot.y,
                            z: snapshot.z,
                            on_ground: snapshot.on_ground,
                        })
                        .await?;
                    }
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
