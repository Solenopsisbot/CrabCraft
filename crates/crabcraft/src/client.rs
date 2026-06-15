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
    ChatCommand, ClickContainer, ClientChatMessage, ClientCommand, ClientInformation,
    ConfirmTeleport, InteractEntity, Interaction, KeepAlive, KeepAliveResponse, PlayDisconnect,
    PlayerDigging, SetContainerContent, SetContainerSlot, SetCreativeSlot, SetHealth, SetHeldItem,
    SetPlayerPosition, SetPlayerPositionRotation, SlotItem, SwingArm, SynchronizePlayerPosition,
    SystemChat, UseItemOn,
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
const ID_UPDATE_HEALTH: i32 = 0x57;
const ID_RESPAWN: i32 = 0x41;
const ID_CONTAINER_CONTENT: i32 = 0x12;
const ID_CONTAINER_SLOT: i32 = 0x14;
const ID_SET_HELD_ITEM: i32 = 0x4d;
const ID_ENTITY_METADATA: i32 = 0x52;

/// Number of slots in the player inventory window (crafting + armour + main +
/// hotbar + offhand).
const PLAYER_INVENTORY_SLOTS: usize = 46;

/// An in-progress survival dig: which block, the dug face, and ticks remaining.
#[derive(Clone, Copy, Debug)]
struct DigProgress {
    block: [i32; 3],
    face: i8,
    ticks_left: u32,
}

/// A tracked non-self entity (other player, mob, item, …) for rendering.
#[derive(Clone, Copy, Debug)]
pub struct Entity {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub half_width: f32,
    pub height: f32,
    /// Entity-type registry id (122 = player); used to pick a model/colour.
    pub type_id: i32,
    /// Render scale (slimes set this from their metadata size).
    pub scale: f32,
    /// For dropped-item entities: the contained item id (for icon rendering).
    pub item: Option<i32>,
}

/// Our current position/orientation as last told by the server.
#[derive(Clone, Copy, Debug)]
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
    /// Current health (0..=20) and food (0..=20).
    pub health: f32,
    pub food: i32,
    /// Selected hotbar slot (0..=8).
    pub selected_slot: u8,
    /// Game mode: 0 = survival, 1 = creative, 2 = adventure, 3 = spectator.
    pub gamemode: u8,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            spawned: false,
            on_ground: false,
            vel: [0.0; 3],
            health: 20.0,
            food: 20,
            selected_slot: 0,
            gamemode: 0,
        }
    }
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
    /// Desired hotbar slot (0..=8), set by number keys / scroll.
    pub selected_slot: u8,
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
    /// The player inventory (window 0): `PLAYER_INVENTORY_SLOTS` slots.
    pub inventory: Mutex<Vec<Option<SlotItem>>>,
    /// Sink for sound-effect names (e.g. `"dig/grass1"`); set when audio is on.
    pub sfx: Mutex<Option<std::sync::mpsc::Sender<String>>>,
    /// Recent chat lines (incoming system/player chat + our own), newest last.
    pub chat_log: Mutex<std::collections::VecDeque<String>>,
    /// Chat/command lines the UI wants sent (drained by the net thread).
    pub chat_outbox: Mutex<Vec<String>>,
    /// The item on the cursor while the inventory is open.
    pub carried: Mutex<Option<SlotItem>>,
    /// Latest container `stateId` (echoed back in inventory clicks).
    pub window_state: Mutex<i32>,
    /// Inventory clicks the UI wants performed: `(slot, button)`.
    pub click_outbox: Mutex<Vec<(i16, i8)>>,
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
            inventory: Mutex::new(vec![None; PLAYER_INVENTORY_SLOTS]),
            sfx: Mutex::new(None),
            chat_log: Mutex::new(std::collections::VecDeque::new()),
            chat_outbox: Mutex::new(Vec::new()),
            carried: Mutex::new(None),
            window_state: Mutex::new(0),
            click_outbox: Mutex::new(Vec::new()),
            running: AtomicBool::new(true),
        }
    }
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
    let mut dead = false;
    let mut block_sequence: i32 = 0;
    // Hold-to-dig state and the previous-tick attack-held flag (for click edges).
    let mut dig: Option<DigProgress> = None;
    let mut was_attacking = false;
    let mut last_selected: u8 = 0;
    // Accumulated walk distance, for spacing footstep sounds.
    let mut step_dist = 0.0_f64;
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
                            let line = plain_text(&c.content);
                            tracing::info!(target: "chat", "{line}");
                            push_chat(shared, line);
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
                    id if id == ID_ENTITY_METADATA => {
                        let _ = handle_entity_metadata(&raw, shared);
                    }
                    id if id == ID_UPDATE_HEALTH => {
                        if let Ok(pkt) = raw.decode::<SetHealth>() {
                            let prev = {
                                let mut ps = shared.player.lock().unwrap();
                                let prev = ps.health;
                                ps.health = pkt.health;
                                ps.food = pkt.food;
                                prev
                            };
                            // Took damage (and not the initial 20->20 sync): hurt sound.
                            if pkt.health < prev && pkt.health > 0.0 {
                                queue_sound(shared, crab_audio::hurt_sound().to_string());
                            }
                            if pkt.health <= 0.0 && !dead {
                                dead = true;
                                tracing::info!("died — respawning");
                                conn.send(&ClientCommand { action: 0 }).await?;
                            } else if pkt.health > 0.0 {
                                dead = false;
                            }
                        }
                    }
                    id if id == ID_RESPAWN => {
                        // Entities don't carry across a respawn/dimension change.
                        shared.entities.lock().unwrap().clear();
                    }
                    id if id == ID_CONTAINER_CONTENT => {
                        if let Ok(pkt) = raw.decode::<SetContainerContent>() {
                            handle_container_content(shared, pkt);
                        }
                    }
                    id if id == ID_CONTAINER_SLOT => {
                        if let Ok(pkt) = raw.decode::<SetContainerSlot>() {
                            handle_container_slot(shared, &pkt);
                        }
                    }
                    id if id == ID_SET_HELD_ITEM => {
                        // Clientbound "Set Held Item": a single hotbar index byte.
                        let mut body = raw.body.clone();
                        if let Ok(slot) = body.read_u8() {
                            if slot <= 8 {
                                shared.player.lock().unwrap().selected_slot = slot;
                            }
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
                // Left-click ("attack") is held: hold-to-dig / continuous
                // attack. Right-click ("use") is edge-triggered: place once.
                let (controls, attack_held, do_place) = {
                    let mut c = shared.controls.lock().unwrap();
                    let snap = *c;
                    c.use_item = false;
                    (snap, snap.attack, snap.use_item)
                };
                let attack_edge = attack_held && !was_attacking;
                was_attacking = attack_held;
                let snapshot = { *shared.player.lock().unwrap() };

                // Hotbar slot change (number keys / scroll): tell the server and
                // mirror locally so the HUD highlight follows immediately.
                if snapshot.spawned
                    && controls.selected_slot != last_selected
                    && controls.selected_slot < 9
                {
                    last_selected = controls.selected_slot;
                    conn.send(&SetHeldItem {
                        slot: i16::from(controls.selected_slot),
                    })
                    .await?;
                    shared.player.lock().unwrap().selected_slot = controls.selected_slot;
                }

                // Send queued chat / commands; show our own line locally.
                let outgoing: Vec<String> =
                    std::mem::take(&mut *shared.chat_outbox.lock().unwrap());
                for msg in outgoing {
                    if let Some(cmd) = msg.strip_prefix('/') {
                        conn.send(&ChatCommand::new(cmd.to_string())).await?;
                    } else {
                        conn.send(&ClientChatMessage::unsigned(msg.clone())).await?;
                    }
                    push_chat(shared, msg);
                }

                // Inventory clicks: swap the clicked slot with the cursor stack,
                // predict locally, and tell the server (it corrects via 0x12/0x14).
                let clicks: Vec<(i16, i8)> =
                    std::mem::take(&mut *shared.click_outbox.lock().unwrap());
                for (slot, button) in clicks {
                    let idx = slot as usize;
                    let state_id = *shared.window_state.lock().unwrap();
                    let swapped = {
                        let mut inv = shared.inventory.lock().unwrap();
                        let mut carried = shared.carried.lock().unwrap();
                        if idx >= inv.len() {
                            None
                        } else {
                            std::mem::swap(&mut inv[idx], &mut *carried);
                            Some((inv[idx], *carried))
                        }
                    };
                    if let Some((slot_item, carried)) = swapped {
                        conn.send(&ClickContainer {
                            window_id: 0,
                            state_id,
                            slot,
                            button,
                            mode: 0,
                            changed: vec![(slot, slot_item)],
                            carried,
                        })
                        .await?;
                    }
                }

                if snapshot.spawned {
                    let yaw = f64::from(controls.yaw).to_radians();
                    let pitch = f64::from(controls.pitch).to_radians();
                    let eye = [snapshot.x, snapshot.y + 1.62, snapshot.z];
                    let dir = [
                        -yaw.sin() * pitch.cos(),
                        -pitch.sin(),
                        yaw.cos() * pitch.cos(),
                    ];
                    let hit = crab_physics::raycast(&shared.world.lock().unwrap(), eye, dir, 5.0);

                    // Combat: on a fresh click, attack a nearer entity in reach.
                    let mut attacked_entity = false;
                    if attack_edge {
                        let block_dist = hit.as_ref().map(|h| {
                            let dx = f64::from(h.block[0]) + 0.5 - eye[0];
                            let dy = f64::from(h.block[1]) + 0.5 - eye[1];
                            let dz = f64::from(h.block[2]) + 0.5 - eye[2];
                            (dx * dx + dy * dy + dz * dz).sqrt()
                        });
                        if let Some((target, t)) = nearest_entity_hit(shared, eye, dir, 3.0) {
                            if block_dist.is_none_or(|bd| t <= bd) {
                                conn.send(&SwingArm { hand: 0 }).await?;
                                conn.send(&InteractEntity {
                                    target,
                                    interaction: Interaction::Attack,
                                    sneaking: false,
                                })
                                .await?;
                                attacked_entity = true;
                                queue_sound(shared, "entity/player/attack/weak1".to_string());
                                dig = cancel_dig(conn, dig, &mut block_sequence).await?;
                            }
                        }
                        // Clicking toward a block always plays its hit sound, even
                        // before it breaks (and we send a swing).
                        if !attacked_entity {
                            conn.send(&SwingArm { hand: 0 }).await?;
                            if let Some(h) = &hit {
                                play_break_sound(shared, h.block);
                            }
                        }
                    }

                    // Hold-to-dig (skipped on the tick we hit an entity).
                    if !attacked_entity {
                        if let (true, Some(hit)) = (attack_held, hit) {
                            let target = hit.block;
                            let face = face_direction(hit.face) as i8;
                            let same = dig.is_some_and(|d| d.block == target);
                            if same {
                                if let Some(d) = dig.as_mut() {
                                    d.ticks_left = d.ticks_left.saturating_sub(1);
                                    let (b, left) = (d.block, d.ticks_left);
                                    if left == 0 {
                                        block_sequence += 1;
                                        conn.send(&dig_packet(2, b, face, block_sequence)).await?;
                                        dig = None;
                                        play_break_sound(shared, b);
                                        break_block_local(shared, b);
                                    } else if left % 4 == 0 {
                                        // Repeated mining "hit" sound while digging.
                                        play_break_sound(shared, b);
                                    }
                                }
                            } else {
                                // Aiming at a new block: cancel the old dig, then
                                // start (and possibly instantly finish) the new one.
                                dig = cancel_dig(conn, dig, &mut block_sequence).await?;
                                let state = shared
                                    .world
                                    .lock()
                                    .unwrap()
                                    .block_state(target[0], target[1], target[2])
                                    .unwrap_or(0);
                                if let Some(ticks) = crab_registry::break_ticks(state) {
                                    block_sequence += 1;
                                    conn.send(&dig_packet(0, target, face, block_sequence)).await?;
                                    if snapshot.gamemode == 1 || ticks == 0 {
                                        // Creative / instant: server breaks now.
                                        if snapshot.gamemode != 1 {
                                            block_sequence += 1;
                                            conn.send(&dig_packet(2, target, face, block_sequence))
                                                .await?;
                                        }
                                        play_break_sound(shared, target);
                                        break_block_local(shared, target);
                                    } else {
                                        dig = Some(DigProgress {
                                            block: target,
                                            face,
                                            ticks_left: ticks,
                                        });
                                    }
                                }
                            }
                        } else {
                            // Released, or not aiming at a block: cancel any dig.
                            dig = cancel_dig(conn, dig, &mut block_sequence).await?;
                        }
                    }

                    // Place on a fresh right-click against the targeted face.
                    if do_place {
                        if let Some(hit) = hit {
                            let p = hit.place_position();
                            block_sequence += 1;
                            conn.send(&UseItemOn {
                                hand: 0,
                                x: hit.block[0],
                                y: hit.block[1],
                                z: hit.block[2],
                                direction: face_direction(hit.face),
                                cursor: [0.5, 0.5, 0.5],
                                inside_block: false,
                                sequence: block_sequence,
                            })
                            .await?;
                            shared.world.lock().unwrap().set_block_state(p[0], p[1], p[2], 1);
                            mark_dirty(shared, p[0] >> 4, p[2] >> 4);
                            if let Some(name) = block_name_at(shared, p) {
                                queue_sound(shared, crab_audio::break_sound(name));
                            }
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
                        // Footsteps: accumulate horizontal distance while grounded
                        // and play a step sound for the block underfoot each ~2 m.
                        if r.on_ground {
                            let (dx, dz) = (x - snapshot.x, z - snapshot.z);
                            step_dist += (dx * dx + dz * dz).sqrt();
                            if step_dist >= 2.0 {
                                step_dist = 0.0;
                                let foot = [
                                    x.floor() as i32,
                                    (y - 0.2).floor() as i32,
                                    z.floor() as i32,
                                ];
                                if let Some(name) = block_name_at(shared, foot) {
                                    queue_sound(shared, crab_audio::step_sound(name));
                                }
                            }
                        }
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
    let game_mode = b.read_u8()?;
    shared.player.lock().unwrap().gamemode = game_mode;
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
    let (half_width, height) = crab_registry::entity_def(kind as u32)
        .map(|d| (d.width / 2.0, d.height))
        .unwrap_or((0.45, 1.3));
    shared.entities.lock().unwrap().insert(
        id,
        Entity {
            x,
            y,
            z,
            half_width,
            height,
            type_id: kind,
            scale: 1.0,
            item: None,
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
            type_id: 122, // "player"
            scale: 1.0,
            item: None,
        },
    );
    Ok(())
}

/// Parses Set Entity Metadata, extracting the fields we render: slime/magma
/// cube `size` (index 16, VarInt) and the dropped-item stack (index 8, Slot).
/// Other fields are skipped by type; parsing stops at an unsupported type.
fn handle_entity_metadata(raw: &crab_net::RawPacket, shared: &Arc<Shared>) -> Result<()> {
    let mut b = raw.body.clone();
    let id = b.read_varint()?;
    let is_sizable = {
        let entities = shared.entities.lock().unwrap();
        entities.get(&id).is_some_and(|e| {
            matches!(
                crab_registry::entity_name(e.type_id as u32),
                Some("slime" | "magma_cube")
            )
        })
    };
    loop {
        let index = b.read_u8()?;
        if index == 0xff {
            break;
        }
        let mtype = b.read_varint()?;
        match mtype {
            0 => {
                b.read_i8()?;
            }
            1 => {
                let v = b.read_varint()?;
                if index == 16 && is_sizable && v > 0 {
                    set_entity_size(shared, id, v as f32);
                }
            }
            2 => {
                b.read_varlong()?;
            }
            3 => {
                b.read_f32()?;
            }
            4 | 5 => {
                b.read_string(32767)?;
            }
            6 => {
                if b.read_bool()? {
                    b.read_string(32767)?;
                }
            }
            7 => {
                let item = read_slot_item(&mut b)?;
                if index == 8 {
                    if let Some(e) = shared.entities.lock().unwrap().get_mut(&id) {
                        e.item = item;
                    }
                }
            }
            8 => {
                b.read_bool()?;
            }
            9 => {
                b.read_f32()?;
                b.read_f32()?;
                b.read_f32()?;
            }
            10 => {
                b.read_position()?;
            }
            11 => {
                if b.read_bool()? {
                    b.read_position()?;
                }
            }
            12 | 14 | 15 | 19 | 20 | 21 | 22 => {
                b.read_varint()?;
            }
            13 => {
                if b.read_bool()? {
                    b.read_uuid()?;
                }
            }
            16 => {
                let _ = crab_protocol::nbt::read_nbt(&mut b)?;
            }
            18 => {
                b.read_varint()?;
                b.read_varint()?;
                b.read_varint()?;
            }
            _ => break, // unsupported type; stop scanning this packet
        }
    }
    Ok(())
}

/// Reads a metadata Slot, returning the contained item id (or None if empty).
fn read_slot_item<B: crab_protocol::BufExt>(b: &mut B) -> Result<Option<i32>> {
    if b.read_bool()? {
        let item_id = b.read_varint()?;
        let _count = b.read_i8()?;
        let _nbt = crab_protocol::nbt::read_nbt(b)?;
        Ok(Some(item_id))
    } else {
        Ok(None)
    }
}

/// Sets a slime/magma-cube entity's render scale + bounding box from its size.
fn set_entity_size(shared: &Arc<Shared>, id: i32, size: f32) {
    if let Some(e) = shared.entities.lock().unwrap().get_mut(&id) {
        e.scale = size;
        // Vanilla slime bbox edge = 0.51 * size.
        e.half_width = 0.255 * size;
        e.height = 0.51 * size;
    }
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

/// A readable label for an item id (e.g. `"diamond"`), falling back to `item#N`.
fn item_label(id: i32) -> String {
    u32::try_from(id)
        .ok()
        .and_then(crab_registry::item_name)
        .map_or_else(|| format!("item#{id}"), str::to_string)
}

/// Renders the non-empty inventory slots for logging.
fn inventory_summary(inv: &[Option<SlotItem>]) -> String {
    let items: Vec<String> = inv
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.map(|it| format!("[{i}]={}x{}", item_label(it.item_id), it.count)))
        .collect();
    if items.is_empty() {
        "empty".to_string()
    } else {
        items.join(" ")
    }
}

/// Replaces the tracked player inventory from a full window snapshot (window 0).
fn handle_container_content(shared: &Arc<Shared>, pkt: SetContainerContent) {
    if pkt.window_id != 0 {
        return; // only the player's own inventory is tracked
    }
    *shared.window_state.lock().unwrap() = pkt.state_id;
    let mut inv = shared.inventory.lock().unwrap();
    *inv = pkt.slots;
    inv.resize(PLAYER_INVENTORY_SLOTS, None);
    tracing::info!("inventory: {}", inventory_summary(&inv));
}

/// Applies a single slot update to the tracked player inventory.
fn handle_container_slot(shared: &Arc<Shared>, pkt: &SetContainerSlot) {
    *shared.window_state.lock().unwrap() = pkt.state_id;
    if pkt.window_id != 0 {
        return;
    }
    let Ok(idx) = usize::try_from(pkt.slot) else {
        return;
    };
    let mut inv = shared.inventory.lock().unwrap();
    if idx < inv.len() {
        inv[idx] = pkt.item;
        if let Some(it) = pkt.item {
            tracing::info!(
                slot = pkt.slot,
                item = item_label(it.item_id),
                count = it.count,
                "inventory slot updated"
            );
        }
    }
}

/// Builds a `PlayerDigging` packet (0 = start, 1 = cancel, 2 = finish).
fn dig_packet(status: i32, block: [i32; 3], face: i8, sequence: i32) -> PlayerDigging {
    PlayerDigging {
        status,
        x: block[0],
        y: block[1],
        z: block[2],
        face,
        sequence,
    }
}

/// Queues a sound-effect name for the audio thread (no-op if audio is off).
fn queue_sound(shared: &Arc<Shared>, name: String) {
    if let Some(tx) = shared.sfx.lock().unwrap().as_ref() {
        let _ = tx.send(name);
    }
}

/// The block name at `block`, if any (for picking material sounds).
fn block_name_at(shared: &Arc<Shared>, block: [i32; 3]) -> Option<&'static str> {
    shared
        .world
        .lock()
        .unwrap()
        .block_state(block[0], block[1], block[2])
        .and_then(crab_registry::block_name)
}

/// Queues the block-break sound for `block` (call before it is removed).
fn play_break_sound(shared: &Arc<Shared>, block: [i32; 3]) {
    if let Some(name) = block_name_at(shared, block) {
        queue_sound(shared, crab_audio::break_sound(name));
    }
}

/// Removes a block locally (set to air) and marks its chunk dirty for re-mesh.
fn break_block_local(shared: &Arc<Shared>, block: [i32; 3]) {
    shared
        .world
        .lock()
        .unwrap()
        .set_block_state(block[0], block[1], block[2], 0);
    mark_dirty(shared, block[0] >> 4, block[2] >> 4);
}

/// Sends a "cancel dig" (status 1) for any in-progress dig and returns `None`.
async fn cancel_dig<S>(
    conn: &mut Connection<S>,
    dig: Option<DigProgress>,
    seq: &mut i32,
) -> Result<Option<DigProgress>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if let Some(d) = dig {
        *seq += 1;
        conn.send(&dig_packet(1, d.block, d.face, *seq)).await?;
    }
    Ok(None)
}

/// The id and entry distance of the nearest entity whose bounding box the ray
/// `eye`+`dir` enters within `reach` blocks (used for melee target selection).
fn nearest_entity_hit(
    shared: &Arc<Shared>,
    eye: [f64; 3],
    dir: [f64; 3],
    reach: f64,
) -> Option<(i32, f64)> {
    let entities = shared.entities.lock().unwrap();
    let mut best: Option<(i32, f64)> = None;
    for (&id, e) in entities.iter() {
        let hw = f64::from(e.half_width);
        let min = [e.x - hw, e.y, e.z - hw];
        let max = [e.x + hw, e.y + f64::from(e.height), e.z + hw];
        if let Some(t) = crab_physics::ray_aabb(eye, dir, min, max) {
            if t <= reach && best.is_none_or(|(_, bt)| t < bt) {
                best = Some((id, t));
            }
        }
    }
    best
}

/// Appends a chat line to the capped chat log.
fn push_chat(shared: &Arc<Shared>, line: String) {
    let mut log = shared.chat_log.lock().unwrap();
    log.push_back(line);
    while log.len() > 100 {
        log.pop_front();
    }
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
