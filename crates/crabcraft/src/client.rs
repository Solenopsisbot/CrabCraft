//! The networking client: connects, logs in, and runs the play loop, updating
//! shared state ([`Shared`]) that other threads (e.g. the renderer) can read.

use std::collections::{HashMap, HashSet};
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
    PlayDisconnect, PlayerDigging, SetCreativeSlot, SetPlayerPosition, SetPlayerPositionRotation,
    SlotItem, SynchronizePlayerPosition, SystemChat, UseItemOn,
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
const ID_SPAWN_ENTITY: i32 = 0x01;
const ID_SPAWN_PLAYER: i32 = 0x03;
const ID_REL_MOVE: i32 = 0x2b;
const ID_MOVE_LOOK: i32 = 0x2c;
const ID_ENTITY_TELEPORT: i32 = 0x68;
const ID_ENTITY_DESTROY: i32 = 0x3e;

/// A tracked non-self entity (other player, mob, item, …) for rendering.
#[derive(Clone, Copy, Debug)]
pub struct Entity {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub half_width: f32,
    pub height: f32,
    pub color: [f32; 3],
}

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
    /// Edge-triggered: break the targeted block (left click).
    pub attack: bool,
    /// Edge-triggered: place a block (right click).
    pub use_item: bool,
}

/// Maps a face normal to the Minecraft direction enum (0=down..5=east).
fn face_direction(face: [i32; 3]) -> i32 {
    match face {
        [0, -1, 0] => 0,
        [0, 1, 0] => 1,
        [0, 0, -1] => 2,
        [0, 0, 1] => 3,
        [-1, 0, 0] => 4,
        [1, 0, 0] => 5,
        _ => 1,
    }
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
    /// Chunk columns whose mesh needs (re)building, drained by the renderer.
    pub dirty_chunks: Mutex<HashSet<(i32, i32)>>,
    /// Other entities (players/mobs/...) by entity id.
    pub entities: Mutex<HashMap<i32, Entity>>,
    /// Cleared to `false` when the session ends, so readers can stop.
    pub running: AtomicBool,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            world: Mutex::new(World::overworld()),
            player: Mutex::new(PlayerState::default()),
            controls: Mutex::new(Controls::default()),
            dirty_chunks: Mutex::new(HashSet::new()),
            entities: Mutex::new(HashMap::new()),
            running: AtomicBool::new(true),
        }
    }
}

fn kind_color(kind: i32) -> [f32; 3] {
    let h = (kind as u32).wrapping_mul(2_654_435_761);
    [
        0.4 + ((h >> 16) & 0xff) as f32 / 255.0 * 0.5,
        0.4 + ((h >> 8) & 0xff) as f32 / 255.0 * 0.5,
        0.4 + (h & 0xff) as f32 / 255.0 * 0.5,
    ]
}

/// Marks a chunk and its 4 neighbours dirty (neighbours so border face-culling
/// updates when an adjacent chunk's blocks change).
fn mark_dirty(shared: &Arc<Shared>, cx: i32, cz: i32) {
    let mut dirty = shared.dirty_chunks.lock().unwrap();
    for c in [
        (cx, cz),
        (cx + 1, cz),
        (cx - 1, cz),
        (cx, cz + 1),
        (cx, cz - 1),
    ] {
        dirty.insert(c);
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
    let mut block_sequence: i32 = 0;
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
                let entities = shared.entities.lock().unwrap().len();
                tracing::info!(entities, "session deadline reached");
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
                            // Hold a stack of stone in hotbar slot 0 (creative) so
                            // right-click placing has something to place.
                            conn.send(&SetCreativeSlot {
                                slot: 36,
                                item: Some(SlotItem {
                                    item_id: 1,
                                    count: 64,
                                }),
                            })
                            .await?;
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
                        let parsed = {
                            let mut world = shared.world.lock().unwrap();
                            let sc = world.section_count();
                            match Chunk::parse(&mut body, sc) {
                                Ok(chunk) => {
                                    let coord = (chunk.x, chunk.z);
                                    world.load_chunk(chunk);
                                    Some(coord)
                                }
                                Err(e) => {
                                    tracing::warn!("chunk parse failed: {e}");
                                    None
                                }
                            }
                        };
                        if let Some((cx, cz)) = parsed {
                            mark_dirty(shared, cx, cz);
                        }
                    }
                    id if id == ID_UNLOAD_CHUNK => {
                        let mut body = raw.body.clone();
                        if let (Ok(cx), Ok(cz)) = (body.read_i32(), body.read_i32()) {
                            shared.world.lock().unwrap().unload_chunk(cx, cz);
                            mark_dirty(shared, cx, cz);
                        }
                    }
                    id if id == ID_BLOCK_CHANGE => {
                        let mut body = raw.body.clone();
                        if let (Ok((bx, by, bz)), Ok(s)) = (body.read_position(), body.read_varint()) {
                            tracing::debug!("server block_change ({bx},{by},{bz}) -> state {s}");
                            shared.world.lock().unwrap().set_block_state(bx, by, bz, s as u32);
                            mark_dirty(shared, bx >> 4, bz >> 4);
                        }
                    }
                    id if id == ID_SPAWN_ENTITY => {
                        let _ = handle_spawn_object(&raw, shared);
                    }
                    id if id == ID_SPAWN_PLAYER => {
                        let _ = handle_spawn_player(&raw, shared);
                    }
                    id if id == ID_REL_MOVE || id == ID_MOVE_LOOK => {
                        let _ = handle_rel_move(&raw, shared);
                    }
                    id if id == ID_ENTITY_TELEPORT => {
                        let _ = handle_entity_teleport(&raw, shared);
                    }
                    id if id == ID_ENTITY_DESTROY => {
                        let _ = handle_entity_destroy(&raw, shared);
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
                // Read controls; consume edge-triggered click flags.
                let (controls, do_break, do_place) = {
                    let mut c = shared.controls.lock().unwrap();
                    let snap = *c;
                    c.attack = false;
                    c.use_item = false;
                    (snap, snap.attack, snap.use_item)
                };
                let snapshot = { *shared.player.lock().unwrap() };

                // Break / place the targeted block.
                if snapshot.spawned && (do_break || do_place) {
                    let yaw = f64::from(controls.yaw).to_radians();
                    let pitch = f64::from(controls.pitch).to_radians();
                    let eye = [snapshot.x, snapshot.y + 1.62, snapshot.z];
                    let dir = [
                        -yaw.sin() * pitch.cos(),
                        -pitch.sin(),
                        yaw.cos() * pitch.cos(),
                    ];
                    let hit = crab_physics::raycast(&shared.world.lock().unwrap(), eye, dir, 5.0);
                    if let Some(hit) = hit {
                        let dir_enum = face_direction(hit.face);
                        if do_break {
                            block_sequence += 1;
                            conn.send(&PlayerDigging {
                                status: 0,
                                x: hit.block[0],
                                y: hit.block[1],
                                z: hit.block[2],
                                face: dir_enum as i8,
                                sequence: block_sequence,
                            })
                            .await?;
                            shared.world.lock().unwrap().set_block_state(
                                hit.block[0],
                                hit.block[1],
                                hit.block[2],
                                0,
                            );
                            mark_dirty(shared, hit.block[0] >> 4, hit.block[2] >> 4);
                        }
                        if do_place {
                            let p = hit.place_position();
                            block_sequence += 1;
                            conn.send(&UseItemOn {
                                hand: 0,
                                x: hit.block[0],
                                y: hit.block[1],
                                z: hit.block[2],
                                direction: dir_enum,
                                cursor: [0.5, 0.5, 0.5],
                                inside_block: false,
                                sequence: block_sequence,
                            })
                            .await?;
                            shared.world.lock().unwrap().set_block_state(p[0], p[1], p[2], 1);
                            mark_dirty(shared, p[0] >> 4, p[2] >> 4);
                        }
                    }
                }

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

fn handle_spawn_object(raw: &crab_net::RawPacket, shared: &Arc<Shared>) -> Result<()> {
    let mut b = raw.body.clone();
    let id = b.read_varint()?;
    let _uuid = b.read_uuid()?;
    let kind = b.read_varint()?;
    let (x, y, z) = (b.read_f64()?, b.read_f64()?, b.read_f64()?);
    shared.entities.lock().unwrap().insert(
        id,
        Entity {
            x,
            y,
            z,
            half_width: 0.45,
            height: 1.3,
            color: kind_color(kind),
        },
    );
    Ok(())
}

fn handle_spawn_player(raw: &crab_net::RawPacket, shared: &Arc<Shared>) -> Result<()> {
    let mut b = raw.body.clone();
    let id = b.read_varint()?;
    let _uuid = b.read_uuid()?;
    let (x, y, z) = (b.read_f64()?, b.read_f64()?, b.read_f64()?);
    shared.entities.lock().unwrap().insert(
        id,
        Entity {
            x,
            y,
            z,
            half_width: 0.3,
            height: 1.8,
            color: [0.9, 0.35, 0.35],
        },
    );
    Ok(())
}

/// Relative move (works for both `rel_entity_move` and `entity_move_look`;
/// trailing look bytes are ignored).
fn handle_rel_move(raw: &crab_net::RawPacket, shared: &Arc<Shared>) -> Result<()> {
    let mut b = raw.body.clone();
    let id = b.read_varint()?;
    let (dx, dy, dz) = (b.read_i16()?, b.read_i16()?, b.read_i16()?);
    if let Some(e) = shared.entities.lock().unwrap().get_mut(&id) {
        e.x += f64::from(dx) / 4096.0;
        e.y += f64::from(dy) / 4096.0;
        e.z += f64::from(dz) / 4096.0;
    }
    Ok(())
}

fn handle_entity_teleport(raw: &crab_net::RawPacket, shared: &Arc<Shared>) -> Result<()> {
    let mut b = raw.body.clone();
    let id = b.read_varint()?;
    let (x, y, z) = (b.read_f64()?, b.read_f64()?, b.read_f64()?);
    if let Some(e) = shared.entities.lock().unwrap().get_mut(&id) {
        e.x = x;
        e.y = y;
        e.z = z;
    }
    Ok(())
}

fn handle_entity_destroy(raw: &crab_net::RawPacket, shared: &Arc<Shared>) -> Result<()> {
    let mut b = raw.body.clone();
    let count = b.read_varint()?.max(0);
    let mut entities = shared.entities.lock().unwrap();
    for _ in 0..count {
        entities.remove(&b.read_varint()?);
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
