//! Crabcraft client — Play milestone.
//!
//! Connects to an **offline-mode** 1.20.1 server, logs in, then runs a real
//! play loop: answers KeepAlive (so we don't get kicked), confirms spawn,
//! reports position, tracks the world (chunks + block updates), reads
//! server/system chat, and sends a greeting. No auth, no encryption, no
//! rendering yet.
//!
//! Usage:
//! ```text
//! crabcraft [ADDR] [USERNAME] [SECONDS]
//! # defaults: 127.0.0.1:25565  Ferris  35
//! ```

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
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let mut args = std::env::args().skip(1);
    let addr = args.next().unwrap_or_else(|| "127.0.0.1:25565".to_string());
    let username = args.next().unwrap_or_else(|| "Ferris".to_string());
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(35);
    let (host, port) = split_host_port(&addr);

    tracing::info!(server = %addr, %username, protocol = PROTOCOL_1_20_1, "Crabcraft connecting");
    let mut conn = Connection::connect(&addr)
        .await
        .with_context(|| format!("failed to connect to {addr}"))?;

    // --- Handshake -> Login ---
    conn.send(&Handshake {
        protocol_version: PROTOCOL_1_20_1,
        server_address: host,
        server_port: port,
        next_state: NextState::Login,
    })
    .await?;
    conn.set_state(State::Login);

    conn.send(&LoginStart {
        name: username.clone(),
        uuid: None,
    })
    .await?;
    tracing::info!("sent LoginStart; waiting for the server to accept us...");

    // --- Login loop ---
    loop {
        let raw = conn.read_packet().await.context("reading login packet")?;
        match raw.id {
            id if id == SetCompression::ID => {
                let pkt: SetCompression = raw.decode()?;
                tracing::info!(threshold = pkt.threshold, "server enabled compression");
                conn.set_compression(pkt.threshold);
            }
            id if id == EncryptionRequest::ID => {
                bail!(
                    "server is in ONLINE mode (sent EncryptionRequest). Microsoft auth + \
                     encryption aren't implemented yet — use an offline-mode server for now."
                );
            }
            id if id == LoginDisconnect::ID => {
                let pkt: LoginDisconnect = raw.decode()?;
                bail!("server refused login: {}", pkt.reason_json);
            }
            id if id == LoginSuccess::ID => {
                let pkt: LoginSuccess = raw.decode()?;
                tracing::info!(uuid = %pkt.uuid, name = %pkt.username, "LOGIN SUCCESS — entering Play");
                conn.set_state(State::Play);
                break;
            }
            other => tracing::warn!(id = format_args!("{other:#04x}"), "ignoring login packet"),
        }
    }

    play_session(&mut conn, &username, Duration::from_secs(seconds)).await?;
    tracing::info!("session over — disconnecting cleanly");
    Ok(())
}

/// In-memory mirror of where the server thinks we are.
#[derive(Default, Debug)]
struct PlayerState {
    x: f64,
    y: f64,
    z: f64,
    yaw: f32,
    pitch: f32,
    spawned: bool,
}

impl PlayerState {
    /// Apply a server teleport, honouring per-axis relative flags.
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

// Clientbound Play packet IDs we consume directly rather than modelling as
// `Packet` structs (chunk data is decoded by crab-world; the others are tiny).
const ID_JOIN_GAME: i32 = 0x28;
const ID_MAP_CHUNK: i32 = 0x24;
const ID_UNLOAD_CHUNK: i32 = 0x1e;
const ID_BLOCK_CHANGE: i32 = 0x0a;

/// Runs the play loop until `duration` elapses or the server disconnects us.
async fn play_session<S>(conn: &mut Connection<S>, username: &str, duration: Duration) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut state = PlayerState::default();
    let mut greeted = false;
    let mut keepalive_count = 0u32;

    let mut world = World::overworld();
    let mut world_dumped = false;

    let mut pos_tick = tokio::time::interval(Duration::from_secs(1));
    let deadline = tokio::time::sleep(duration);
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            biased;

            // Hard time limit for this demo session.
            _ = &mut deadline => {
                tracing::info!(keepalives = keepalive_count, "demo time elapsed");
                break;
            }

            // Inbound packets.
            result = conn.read_packet() => {
                let raw = match result {
                    Ok(raw) => raw,
                    Err(e) => {
                        tracing::info!("connection closed by server: {e}");
                        break;
                    }
                };

                match raw.id {
                    id if id == KeepAlive::ID => {
                        let k: KeepAlive = raw.decode()?;
                        conn.send(&KeepAliveResponse { id: k.id }).await?;
                        keepalive_count += 1;
                        tracing::info!(id = k.id, n = keepalive_count, "<3 keepalive answered");
                    }
                    id if id == SynchronizePlayerPosition::ID => {
                        let p: SynchronizePlayerPosition = raw.decode()?;
                        state.apply(&p);
                        // Acknowledge, then confirm our position.
                        conn.send(&ConfirmTeleport { teleport_id: p.teleport_id }).await?;
                        conn.send(&SetPlayerPositionRotation {
                            x: state.x,
                            y: state.y,
                            z: state.z,
                            yaw: state.yaw,
                            pitch: state.pitch,
                            on_ground: true,
                        })
                        .await?;

                        if !state.spawned {
                            state.spawned = true;
                            tracing::info!(
                                "SPAWNED at ({:.2}, {:.2}, {:.2}) yaw={:.1}",
                                state.x, state.y, state.z, state.yaw
                            );
                            // Send client settings only now. Sending serverbound
                            // Play packets immediately after LoginSuccess races the
                            // server's inbound Login->Play protocol switch and gets
                            // misdecoded; waiting until we've received a Play packet
                            // (this position sync) guarantees the server is ready.
                            conn.send(&ClientInformation::sensible_defaults()).await?;
                            tracing::info!("sent ClientInformation");
                        }

                        if !greeted {
                            greeted = true;
                            let msg = format!(
                                "{username} here via Crabcraft — a 1.20.1 client written in pure Rust. We're so back."
                            );
                            conn.send(&ClientChatMessage::unsigned(msg)).await?;
                            tracing::info!("sent greeting to chat");
                        }
                    }
                    id if id == SystemChat::ID => {
                        let c: SystemChat = raw.decode()?;
                        if !c.overlay {
                            tracing::info!(target: "chat", "{}", plain_text(&c.content));
                        }
                    }
                    id if id == PlayDisconnect::ID => {
                        let d: PlayDisconnect = raw.decode()?;
                        tracing::warn!("server disconnected us: {}", plain_text(&d.reason_json));
                        break;
                    }
                    id if id == ID_JOIN_GAME => {
                        // Parse only the prefix we need: through the dimension
                        // codec + current dimension type, to size the world.
                        let mut b = raw.body.clone();
                        let parsed = (|| -> Result<()> {
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
                            if let Some((min_y, height)) =
                                crab_world::dimension_extent(&codec, &dimension_type)
                            {
                                world = World::new(min_y, height);
                                tracing::info!(
                                    "dimension {dimension_type}: min_y={min_y} height={height} ({} sections)",
                                    world.section_count()
                                );
                            }
                            Ok(())
                        })();
                        if let Err(e) = parsed {
                            tracing::warn!("failed to parse Join Game: {e}");
                        }
                    }
                    id if id == ID_MAP_CHUNK => {
                        let mut body = raw.body.clone();
                        match Chunk::parse(&mut body, world.section_count()) {
                            Ok(chunk) => {
                                let (cx, cz, n) = (chunk.x, chunk.z, chunk.sections.len());
                                world.load_chunk(chunk);
                                tracing::debug!(
                                    "loaded chunk ({cx},{cz}) sections={n} total={}",
                                    world.chunk_count()
                                );
                            }
                            Err(e) => tracing::warn!("chunk parse failed: {e}"),
                        }
                    }
                    id if id == ID_UNLOAD_CHUNK => {
                        let mut body = raw.body.clone();
                        if let (Ok(cx), Ok(cz)) = (body.read_i32(), body.read_i32()) {
                            world.unload_chunk(cx, cz);
                        }
                    }
                    id if id == ID_BLOCK_CHANGE => {
                        let mut body = raw.body.clone();
                        if let (Ok((bx, by, bz)), Ok(s)) = (body.read_position(), body.read_varint()) {
                            world.set_block_state(bx, by, bz, s as u32);
                            tracing::debug!("block update ({bx},{by},{bz}) -> state {s}");
                        }
                    }
                    other => tracing::trace!(id = format_args!("{other:#04x}"), bytes = raw.body.len(), "ignored play packet"),
                }
            }

            // Heartbeat: report our position once a second once spawned.
            _ = pos_tick.tick() => {
                if state.spawned {
                    conn.send(&SetPlayerPosition {
                        x: state.x,
                        y: state.y,
                        z: state.z,
                        on_ground: true,
                    })
                    .await?;

                    // Once our chunk has arrived, dump the column under our feet
                    // to prove we actually decoded the world.
                    if !world_dumped {
                        let (bx, by, bz) =
                            (state.x.floor() as i32, state.y.floor() as i32, state.z.floor() as i32);
                        if world.is_loaded(bx, bz) {
                            world_dumped = true;
                            tracing::info!(
                                "world: {} chunks loaded; block column at x={bx} z={bz}:",
                                world.chunk_count()
                            );
                            for dy in (by - 4..=by + 1).rev() {
                                match world.block_state(bx, dy, bz) {
                                    Some(s) => {
                                        let name = crab_registry::block_name(s).unwrap_or("unknown");
                                        tracing::info!("    y={dy:>4}: {name} (state {s})");
                                    }
                                    None => tracing::info!("    y={dy:>4}: <none>"),
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Very small chat-component flattener: pulls the top-level `"text"` out of a
/// JSON component for readable logging. (A proper component parser is a later
/// milestone; this is just so the demo output isn't raw JSON noise.)
fn plain_text(json: &str) -> String {
    // Cheap heuristic: find `"text":"..."`. Good enough for join/leave/system lines.
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

/// Splits `"host:port"` into parts, defaulting the port to 25565.
fn split_host_port(addr: &str) -> (String, u16) {
    match addr.rsplit_once(':') {
        Some((host, port)) => (host.to_string(), port.parse().unwrap_or(25565)),
        None => (addr.to_string(), 25565),
    }
}
