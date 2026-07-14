//! The networking client: connects, logs in, and runs the play loop, updating
//! shared state ([`Shared`]) that other threads (e.g. the renderer) can read.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use bytes::Buf;
use crab_core::{
    ClientCommand, ClientCore, ClientEvent, ClientSnapshot, CommandQueue, ConnectionPhase,
    ProtocolVersion, RecipeKey, ReplayRecorder, SessionContext,
};
use crab_net::Connection;
use crab_protocol::packet::{Packet, State};
use crab_protocol::versions::v1_20_1::handshake::{Handshake, NextState};
use crab_protocol::versions::v1_20_1::login::{
    EncryptionRequest, EncryptionResponse, LoginDisconnect, LoginStart, LoginSuccess,
    SetCompression,
};
use crab_protocol::versions::v1_20_1::play::{
    ChatCommand, ClickContainer, ClientChatMessage, ClientCommand as ClientStatusCommand,
    ClientInformation, ClientboundCloseContainer, ClientboundPlayerAbilities, CloseContainer,
    ConfirmTeleport, ContainerButtonClick, EditBook, EnchantItem, InteractEntity, Interaction,
    KeepAlive, KeepAliveResponse, OpenScreen, Ping, PlaceRecipe, PlayDisconnect, PlayerCommand,
    PlayerDigging, Pong, RenameItem, ResourcePackStatus, ServerboundPlayerAbilities,
    SetContainerContent, SetContainerData, SetContainerSlot, SetHealth, SetHeldItem,
    SetPlayerPosition, SetPlayerPositionRotation, SlotItem, SteerBoat, SteerVehicle, SwingArm,
    SynchronizePlayerPosition, SystemChat, UseItem, UseItemOn, VehicleMove,
};
use crab_protocol::versions::v1_20_2::configuration::{
    ClientboundFinishConfiguration, ConfigurationClientInformation, ConfigurationKeepAlive,
    ConfigurationKeepAliveResponse, ConfigurationPing, ConfigurationPong,
    ConfigurationResourcePackStatus, FinishConfiguration, RegistryData,
};
use crab_protocol::versions::v1_20_2::login::{LoginAcknowledged, LoginStart as LoginStart764};
use crab_protocol::versions::v1_20_2::play::ChunkBatchReceived;
use crab_protocol::versions::v1_20_3::{configuration as configuration765, play as play765};
use crab_protocol::versions::v1_20_5::{configuration as configuration766, play as play766};
use crab_protocol::versions::v1_21::play as play767;
use crab_protocol::versions::v1_21_2::{configuration as configuration768, play as play768};
use crab_protocol::versions::v1_21_4::play as play769;
use crab_protocol::versions::v1_21_5::play as play770;
use crab_protocol::BufExt;
use crab_world::{Chunk, World};
use sha1::{Digest, Sha1};
use tokio::io::{AsyncRead, AsyncWrite};

// Clientbound Play packet IDs consumed directly (decoded inline / by crab-world).
const ID_JOIN_GAME: i32 = 0x28;
const ID_MAP_CHUNK: i32 = 0x24;
const ID_UNLOAD_CHUNK: i32 = 0x1e;
const ID_BLOCK_CHANGE: i32 = 0x0a;
const ID_BLOCK_ENTITY_DATA: i32 = 0x08;
const ID_SPAWN_ENTITY: i32 = 0x01;
const ID_SPAWN_PLAYER: i32 = 0x03;
const ID_REL_MOVE: i32 = 0x2b;
const ID_MOVE_LOOK: i32 = 0x2c;
const ID_VEHICLE_MOVE: i32 = 0x2e;
const ID_ENTITY_ROTATION: i32 = 0x2d;
const ID_ENTITY_TELEPORT: i32 = 0x68;
const ID_ENTITY_HEAD_ROTATION: i32 = 0x42;
const ID_ENTITY_VELOCITY: i32 = 0x54;
const ID_ENTITY_EQUIPMENT: i32 = 0x55;
const ID_SET_PASSENGERS: i32 = 0x59;
const ID_ENTITY_DESTROY: i32 = 0x3e;
const ID_UPDATE_HEALTH: i32 = 0x57;
const ID_RESPAWN: i32 = 0x41;
const ID_CONTAINER_CONTENT: i32 = 0x12;
const ID_CONTAINER_SLOT: i32 = 0x14;
const ID_SET_HELD_ITEM: i32 = 0x4d;
const ID_ENTITY_METADATA: i32 = 0x52;
const ID_GAME_STATE: i32 = 0x1f;
const ID_EXPERIENCE: i32 = 0x56;
const ID_UPDATE_TIME: i32 = 0x5e;
const ID_ENTITY_SOUND: i32 = 0x61;
const ID_SOUND: i32 = 0x62;
const ID_CLEAR_TITLES: i32 = 0x0e;
const ID_ACTION_BAR: i32 = 0x46;
const ID_TITLE_SUBTITLE: i32 = 0x5d;
const ID_TITLE_TEXT: i32 = 0x5f;
const ID_TITLE_TIME: i32 = 0x60;
const ID_BOSS_BAR: i32 = 0x0b;
const ID_INIT_WORLD_BORDER: i32 = 0x22;
const ID_WORLD_BORDER_CENTER: i32 = 0x47;
const ID_WORLD_BORDER_LERP: i32 = 0x48;
const ID_WORLD_BORDER_SIZE: i32 = 0x49;
const ID_WORLD_BORDER_WARNING_DELAY: i32 = 0x4a;
const ID_WORLD_BORDER_WARNING_REACH: i32 = 0x4b;
const ID_SCOREBOARD_DISPLAY: i32 = 0x51;
const ID_SCOREBOARD_OBJECTIVE: i32 = 0x58;
const ID_TEAMS: i32 = 0x5a;
const ID_SCOREBOARD_SCORE: i32 = 0x5b;
const ID_PLAYER_REMOVE: i32 = 0x39;
const ID_PLAYER_INFO: i32 = 0x3a;
const ID_PLAYER_LIST_HEADER: i32 = 0x65;
const ID_PARTICLES: i32 = 0x26;
const ID_MAP_DATA: i32 = 0x29;
const ID_ENTITY_ANIMATION: i32 = 0x04;
const ID_HURT_ANIMATION: i32 = 0x21;
const ID_REMOVE_ENTITY_EFFECT: i32 = 0x3f;
const ID_ENTITY_EFFECT: i32 = 0x6c;
const ID_DECLARE_RECIPES: i32 = 0x6d;
const ID_UNLOCK_RECIPES: i32 = 0x3d;
const ID_RESOURCE_PACK: i32 = 0x40;

/// Number of slots in the player inventory window (crafting + armour + main +
/// hotbar + offhand).
const PLAYER_INVENTORY_SLOTS: usize = 46;

/// A server-owned menu and its complete slot list. For chest-style menus the
/// container slots come first, followed by 36 player main/hotbar slots.
#[derive(Clone, Debug)]
pub struct ContainerState {
    pub window_id: u8,
    pub menu_type: i32,
    pub title: String,
    pub slots: Vec<Option<SlotItem>>,
    /// Data-component/NBT metadata parallel to `slots`, retained for item UI
    /// such as interactive bundle contents.
    pub slot_metadata: Vec<Option<crab_protocol::nbt::Nbt>>,
    pub state_id: i32,
    /// Numeric menu properties. Furnace-family menus use indices 0..=3.
    pub properties: [i16; 4],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StonecutterRecipe {
    pub id: String,
    pub ingredients: Vec<i32>,
    pub result: Option<SlotItem>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CraftingRecipe {
    pub id: String,
    pub width: u8,
    pub height: u8,
    pub ingredients: Vec<Vec<i32>>,
    pub result: Option<SlotItem>,
}

#[derive(Default)]
struct DeclaredRecipes {
    crafting: Vec<CraftingRecipe>,
    stonecutting: Vec<StonecutterRecipe>,
}

impl ContainerState {
    /// Vanilla generic 9-column container row count (including shulker boxes).
    #[must_use]
    pub fn generic_rows(&self) -> Option<usize> {
        match self.menu_type {
            0..=5 => Some(self.menu_type as usize + 1),
            19 => Some(3),
            _ => None,
        }
    }

    #[must_use]
    pub fn container_slot_count(&self) -> usize {
        self.slots.len().saturating_sub(36)
    }

    #[must_use]
    pub fn furnace_texture(&self) -> Option<&'static str> {
        match self.menu_type {
            9 => Some("blast_furnace"),
            13 => Some("furnace"),
            21 => Some("smoker"),
            _ => None,
        }
    }

    #[must_use]
    pub fn simple_container_texture(&self) -> Option<&'static str> {
        match self.menu_type {
            6 => Some("dispenser"),
            7 => Some("anvil"),
            10 => Some("brewing_stand"),
            11 => Some("crafting_table"),
            12 => Some("enchanting_table"),
            14 => Some("grindstone"),
            15 => Some("hopper"),
            17 => Some("loom"),
            20 => Some("smithing"),
            22 => Some("cartography_table"),
            23 => Some("stonecutter"),
            _ => None,
        }
    }
}

/// An in-progress survival dig: which block, the dug face, and ticks remaining.
#[derive(Clone, Copy, Debug)]
struct DigProgress {
    block: [i32; 3],
    face: i8,
    ticks_left: u32,
    ticks_total: u32,
}

/// The block currently being mined and its crack stage (0..=9), published for
/// the renderer to draw the destroy-stage overlay.
#[derive(Clone, Copy, Debug)]
pub struct DigOverlay {
    pub block: [i32; 3],
    pub stage: u8,
}

/// Server-driven overworld ambience used by the renderer.
#[derive(Clone, Copy, Debug, Default)]
pub struct EnvironmentState {
    pub world_age: i64,
    pub time_of_day: i64,
    pub rain_level: f32,
    pub thunder_level: f32,
}

/// One server-authoritative status effect. `duration == -1` means infinite.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActiveEffect {
    pub amplifier: u8,
    pub duration: i32,
    pub ambient: bool,
    pub show_particles: bool,
    pub show_icon: bool,
}

#[derive(Clone, Debug)]
pub struct TitleState {
    pub title: String,
    pub subtitle: String,
    pub fade_in: u32,
    pub stay: u32,
    pub fade_out: u32,
    pub remaining: u32,
}

impl Default for TitleState {
    fn default() -> Self {
        Self {
            title: String::new(),
            subtitle: String::new(),
            fade_in: 10,
            stay: 70,
            fade_out: 20,
            remaining: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct BossBarState {
    pub title: String,
    pub health: f32,
    pub color: i32,
    pub divisions: i32,
    pub flags: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct WorldBorderState {
    pub x: f64,
    pub z: f64,
    pub diameter: f64,
    pub target_diameter: f64,
    pub lerp_remaining_ms: i32,
    pub warning_blocks: i32,
    pub warning_time: i32,
}

impl Default for WorldBorderState {
    fn default() -> Self {
        Self {
            x: 0.0,
            z: 0.0,
            diameter: 59_999_968.0,
            target_diameter: 59_999_968.0,
            lerp_remaining_ms: 0,
            warning_blocks: 5,
            warning_time: 15,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ScoreboardState {
    pub objectives: HashMap<String, String>,
    pub sidebar: Option<String>,
    pub scores: HashMap<String, HashMap<String, i32>>,
    pub teams: HashMap<String, TeamState>,
}

impl ScoreboardState {
    #[must_use]
    pub fn decorated_name(&self, name: &str) -> String {
        self.teams
            .values()
            .find(|team| team.members.contains(name))
            .map_or_else(
                || name.to_owned(),
                |team| format!("{}{}{}", team.prefix, name, team.suffix),
            )
    }
}

#[derive(Clone, Debug, Default)]
pub struct TeamState {
    pub display_name: String,
    pub prefix: String,
    pub suffix: String,
    pub formatting: i32,
    pub members: HashSet<String>,
}

#[derive(Clone, Debug, Default)]
pub struct PlayerListEntry {
    pub name: String,
    pub display_name: Option<String>,
    pub game_mode: i32,
    pub latency: i32,
    pub listed: bool,
}

#[derive(Clone, Debug, Default)]
pub struct PlayerListState {
    pub entries: HashMap<String, PlayerListEntry>,
    pub header: String,
    pub footer: String,
}

#[derive(Clone, Copy, Debug)]
pub struct Particle {
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub color: [f32; 3],
    pub age: u16,
    pub lifetime: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapMarker {
    pub kind: i32,
    pub x: i8,
    pub z: i8,
    pub direction: u8,
    pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapState {
    pub scale: i8,
    pub locked: bool,
    pub colors: Vec<u8>,
    pub markers: Vec<MapMarker>,
}

impl Default for MapState {
    fn default() -> Self {
        Self {
            scale: 0,
            locked: false,
            colors: vec![0; 128 * 128],
            markers: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourcePackRequest {
    pub uuid: Option<uuid::Uuid>,
    pub url: String,
    pub hash: String,
    pub forced: bool,
    pub prompt: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourcePackLayer {
    pub uuid: Option<uuid::Uuid>,
    pub archive: PathBuf,
}

/// Text stored by a sign or hanging-sign block entity. Both sides are retained
/// because 1.20 signs can be edited and read independently.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignState {
    pub front: [String; 4],
    pub back: [String; 4],
    pub front_glowing: bool,
    pub back_glowing: bool,
}

type PositionedSign = ((i32, i32, i32), SignState);

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
    /// Facing yaw in degrees (Minecraft convention), for model rotation.
    pub yaw: f32,
    /// Head facing yaw, independent of body yaw for players and mobs.
    pub head_yaw: f32,
    pub velocity: [f64; 3],
    /// Main hand, offhand, boots, leggings, chestplate, helmet.
    pub equipment: [Option<i32>; 6],
    pub vehicle: Option<i32>,
    pub pose: i32,
    pub invisible: bool,
    pub glowing: bool,
    pub swing_sequence: u64,
    pub hurt_sequence: u64,
    /// For dropped-item entities: the contained item id (for icon rendering).
    pub item: Option<i32>,
    /// For falling-block entities: the exact protocol block-state ID carried by
    /// Spawn Entity's object-data field.
    pub block_state: Option<u32>,
}

/// Our current position/orientation as last told by the server.
#[derive(Clone, Copy, Debug)]
pub struct PlayerState {
    /// Server entity id assigned by Join Game (needed by player-command packets).
    pub entity_id: i32,
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
    /// Experience bar fill (0..=1) and level.
    pub xp_bar: f32,
    pub xp_level: i32,
    pub sneaking: bool,
    pub sprinting: bool,
    pub allow_flying: bool,
    pub flying: bool,
    pub flying_speed: f32,
    pub walking_speed: f32,
    pub swimming: bool,
    pub gliding: bool,
    /// Root entity currently ridden by the local player.
    pub vehicle: Option<i32>,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            entity_id: 0,
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
            xp_bar: 0.0,
            xp_level: 0,
            sneaking: false,
            sprinting: false,
            allow_flying: false,
            flying: false,
            flying_speed: 0.05,
            walking_speed: 0.1,
            swimming: false,
            gliding: false,
            vehicle: None,
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
    /// Sprint held (Control).
    pub sprint: bool,
    /// Sneak held (Shift).
    pub sneak: bool,
    /// Edge-triggered double-Space flight toggle.
    pub toggle_flight: bool,
    /// Edge-triggered vanilla swap-main/offhand action (F).
    pub swap_hands: bool,
    /// Look yaw in degrees (Minecraft convention).
    pub yaw: f32,
    /// Look pitch in degrees (positive = down).
    pub pitch: f32,
    /// Edge-triggered: break the targeted block (left click).
    pub attack: bool,
    /// Held right-click: place/interact on press, release active item use on release.
    pub use_item: bool,
    /// Desired hotbar slot (0..=8), set by number keys / scroll.
    pub selected_slot: u8,
}

/// Coalescing, event-driven invalidation queue for chunk mesh workers.
#[derive(Debug, Default)]
pub struct DirtyChunks {
    pending: Mutex<HashSet<(i32, i32)>>,
    ready: Condvar,
}

impl DirtyChunks {
    pub fn mark_neighborhood(&self, cx: i32, cz: i32) {
        let mut pending = self.pending.lock().unwrap();
        pending.extend([
            (cx, cz),
            (cx + 1, cz),
            (cx - 1, cz),
            (cx, cz + 1),
            (cx, cz - 1),
        ]);
        self.ready.notify_one();
    }

    pub fn extend(&self, chunks: impl IntoIterator<Item = (i32, i32)>) {
        self.pending.lock().unwrap().extend(chunks);
        self.ready.notify_one();
    }

    pub fn take_batch_wait(&self, max: usize) -> Vec<(i32, i32)> {
        let mut pending = self.pending.lock().unwrap();
        if pending.is_empty() {
            let (next, _) = self
                .ready
                .wait_timeout(pending, Duration::from_millis(100))
                .unwrap();
            pending = next;
        }
        let batch: Vec<_> = pending.iter().take(max).copied().collect();
        for coord in &batch {
            pending.remove(coord);
        }
        batch
    }
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

/// Applies the client-side prediction for the vanilla swap-hands action.
/// The server remains authoritative and will reconcile both slots afterward.
fn swap_selected_with_offhand(inventory: &mut [Option<SlotItem>], selected_slot: u8) {
    let selected = 36 + usize::from(selected_slot.min(8));
    if inventory.len() > 45 {
        inventory.swap(selected, 45);
    }
}

fn controlled_boat_step(
    position: [f64; 3],
    yaw: f32,
    forward: f32,
    strafe: f32,
    dt: f64,
) -> ([f64; 3], f32) {
    let yaw = yaw + strafe.clamp(-1.0, 1.0) * 70.0 * dt as f32;
    let speed = if forward >= 0.0 { 4.0 } else { 2.0 } * f64::from(forward.clamp(-1.0, 1.0));
    let radians = f64::from(yaw).to_radians();
    (
        [
            position[0] - radians.sin() * speed * dt,
            position[1],
            position[2] + radians.cos() * speed * dt,
        ],
        yaw,
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FluidKind {
    Water,
    Lava,
}

fn fluid_kind_at(world: &World, feet: [f64; 3], height: f64) -> Option<FluidKind> {
    let x0 = (feet[0] - crab_physics::PLAYER_HALF_WIDTH).floor() as i32;
    let x1 = (feet[0] + crab_physics::PLAYER_HALF_WIDTH - 1e-7).floor() as i32;
    let y0 = (feet[1] + 0.1).floor() as i32;
    let y1 = (feet[1] + height * 0.8).floor() as i32;
    let z0 = (feet[2] - crab_physics::PLAYER_HALF_WIDTH).floor() as i32;
    let z1 = (feet[2] + crab_physics::PLAYER_HALF_WIDTH - 1e-7).floor() as i32;
    let mut water = false;
    for x in x0..=x1 {
        for y in y0..=y1 {
            for z in z0..=z1 {
                match world
                    .block_state(x, y, z)
                    .and_then(crab_registry::block_name)
                {
                    Some("minecraft:lava") => return Some(FluidKind::Lava),
                    Some("minecraft:water") => water = true,
                    _ => {}
                }
            }
        }
    }
    water.then_some(FluidKind::Water)
}

fn movement_speed(sneaking: bool, sprinting: bool, fluid: Option<FluidKind>) -> f64 {
    if let Some(fluid) = fluid {
        return match fluid {
            FluidKind::Water if sprinting => 3.0,
            FluidKind::Water if sneaking => 1.5,
            FluidKind::Water => 2.0,
            FluidKind::Lava => 1.0,
        };
    }
    if sneaking {
        1.295
    } else if sprinting {
        5.612
    } else {
        4.317
    }
}

fn effect_level(effects: &HashMap<i32, ActiveEffect>, id: i32) -> u32 {
    effects
        .get(&id)
        .map_or(0, |effect| u32::from(effect.amplifier) + 1)
}

fn effect_movement_multiplier(effects: &HashMap<i32, ActiveEffect>) -> f64 {
    let speed = 1.0 + 0.2 * f64::from(effect_level(effects, 1));
    let slowness = (1.0 - 0.15 * f64::from(effect_level(effects, 2))).max(0.0);
    speed * slowness
}

fn effect_mining_multiplier(effects: &HashMap<i32, ActiveEffect>) -> f64 {
    let haste = 1.0 + 0.2 * f64::from(effect_level(effects, 3));
    let fatigue = 0.3_f64.powi(effect_level(effects, 4).min(4) as i32);
    haste * fatigue
}

fn adjusted_break_ticks(ticks: u32, effects: &HashMap<i32, ActiveEffect>) -> u32 {
    if ticks == 0 {
        return 0;
    }
    (f64::from(ticks) / effect_mining_multiplier(effects))
        .ceil()
        .clamp(1.0, f64::from(u32::MAX)) as u32
}

fn flight_speed(ability_speed: f32, sprinting: bool) -> f64 {
    let base = f64::from(ability_speed.max(0.0)) * 218.4;
    if sprinting {
        base * 2.0
    } else {
        base
    }
}

fn particle_color(id: i32) -> [f32; 3] {
    let hash = (id as u32).wrapping_mul(0x9e37_79b9);
    [
        0.35 + f32::from((hash & 0xff) as u8) / 510.0,
        0.35 + f32::from(((hash >> 8) & 0xff) as u8) / 510.0,
        0.35 + f32::from(((hash >> 16) & 0xff) as u8) / 510.0,
    ]
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
    /// Immutable protocol and generated-registry selection for this session.
    pub context: SessionContext,
    /// Deterministic single-owner reducer. Only the session task mutates it;
    /// readers consume the coherently published snapshot below.
    core: Mutex<CoreRuntime>,
    snapshot: RwLock<Arc<ClientSnapshot>>,
    replay_output: Mutex<Option<PathBuf>>,
    pub world: Mutex<World>,
    /// Registry codec received during 764's Configuration state.
    pub registry_codec: Mutex<Option<crab_protocol::nbt::Nbt>>,
    pub player: Mutex<PlayerState>,
    pub environment: Mutex<EnvironmentState>,
    /// Active status effects on the local player, keyed by vanilla effect id.
    pub effects: Mutex<HashMap<i32, ActiveEffect>>,
    pub action_bar: Mutex<Option<(String, u32)>>,
    pub title: Mutex<TitleState>,
    pub boss_bars: Mutex<HashMap<String, BossBarState>>,
    pub world_border: Mutex<WorldBorderState>,
    pub scoreboard: Mutex<ScoreboardState>,
    pub player_list: Mutex<PlayerListState>,
    pub particles: Mutex<Vec<Particle>>,
    pub stonecutter_recipes: Mutex<Vec<StonecutterRecipe>>,
    pub crafting_recipes: Mutex<Vec<CraftingRecipe>>,
    pub unlocked_recipes: Mutex<HashSet<String>>,
    pub maps: Mutex<HashMap<i32, MapState>>,
    pub latest_map: Mutex<Option<i32>>,
    pub signs: Mutex<HashMap<(i32, i32, i32), SignState>>,
    pub resource_pack_request: Mutex<Option<ResourcePackRequest>>,
    pub resource_pack_prompt_open: Mutex<bool>,
    pub cached_resource_pack: Mutex<Option<PathBuf>>,
    /// Validated raw server packs in lowest-to-highest priority order.
    pub resource_pack_layers: Mutex<Vec<ResourcePackLayer>>,
    /// Base client resources used beneath server-pack overrides.
    pub base_resource_jar: Mutex<Option<PathBuf>>,
    /// True for the windowed client, which acknowledges a pack only after its
    /// CPU/GPU atlases have actually been replaced.
    pub renderer_active: AtomicBool,
    /// Whether the next renderer reload is an Add Pack that needs a terminal
    /// status. Remove Pack reloads are intentionally unacknowledged.
    pub resource_pack_reload_ack: AtomicBool,
    /// Bounded typed commands produced by presentation adapters.
    pub commands: CommandQueue,
    /// Player input intent (written by the renderer).
    pub controls: Mutex<Controls>,
    /// Chunk columns whose mesh needs (re)building, drained by the renderer.
    pub dirty_chunks: DirtyChunks,
    /// Other entities (players/mobs/...) by entity id.
    pub entities: Mutex<HashMap<i32, Entity>>,
    /// The player inventory (window 0): `PLAYER_INVENTORY_SLOTS` slots.
    pub inventory: Mutex<Vec<Option<SlotItem>>>,
    /// Raw item NBT parallel to `inventory`; used by books, maps, names and
    /// other metadata-bearing items without making lightweight SlotItem non-Copy.
    pub inventory_nbt: Mutex<Vec<Option<crab_protocol::nbt::Nbt>>>,
    /// Currently open server-owned container, if any.
    pub container: Mutex<Option<ContainerState>>,
    /// Sink for sound-effect names (e.g. `"dig/grass1"`); set when audio is on.
    pub sfx: Mutex<Option<std::sync::mpsc::Sender<String>>>,
    /// Recent chat lines (incoming system/player chat + our own), newest last.
    pub chat_log: Mutex<std::collections::VecDeque<String>>,
    /// The item on the cursor while the inventory is open.
    pub carried: Mutex<Option<SlotItem>>,
    /// Latest container `stateId` (echoed back in inventory clicks).
    pub window_state: Mutex<i32>,
    /// The block currently being mined + its crack stage, for the destroy-stage
    /// overlay (`None` when not digging).
    pub dig: Mutex<Option<DigOverlay>>,
    /// Cleared to `false` when the session ends, so readers can stop.
    pub running: AtomicBool,
}

#[derive(Debug)]
struct CoreRuntime {
    core: ClientCore,
    replay: Option<ReplayRecorder>,
}

impl Shared {
    pub fn new() -> Self {
        Self::with_context(SessionContext::new(ProtocolVersion::default()))
    }

    pub fn with_context(context: SessionContext) -> Self {
        let core = ClientCore::new(context);
        let snapshot = Arc::new(core.snapshot().clone());
        Self {
            context,
            core: Mutex::new(CoreRuntime { core, replay: None }),
            snapshot: RwLock::new(snapshot),
            replay_output: Mutex::new(None),
            world: Mutex::new(World::overworld()),
            registry_codec: Mutex::new(None),
            player: Mutex::new(PlayerState::default()),
            environment: Mutex::new(EnvironmentState::default()),
            effects: Mutex::new(HashMap::new()),
            action_bar: Mutex::new(None),
            title: Mutex::new(TitleState::default()),
            boss_bars: Mutex::new(HashMap::new()),
            world_border: Mutex::new(WorldBorderState::default()),
            scoreboard: Mutex::new(ScoreboardState::default()),
            player_list: Mutex::new(PlayerListState::default()),
            particles: Mutex::new(Vec::new()),
            stonecutter_recipes: Mutex::new(Vec::new()),
            crafting_recipes: Mutex::new(Vec::new()),
            unlocked_recipes: Mutex::new(HashSet::new()),
            maps: Mutex::new(HashMap::new()),
            latest_map: Mutex::new(None),
            signs: Mutex::new(HashMap::new()),
            resource_pack_request: Mutex::new(None),
            resource_pack_prompt_open: Mutex::new(false),
            cached_resource_pack: Mutex::new(None),
            resource_pack_layers: Mutex::new(Vec::new()),
            base_resource_jar: Mutex::new(None),
            renderer_active: AtomicBool::new(false),
            resource_pack_reload_ack: AtomicBool::new(false),
            commands: CommandQueue::new(256),
            controls: Mutex::new(Controls::default()),
            dirty_chunks: DirtyChunks::default(),
            entities: Mutex::new(HashMap::new()),
            inventory: Mutex::new(vec![None; PLAYER_INVENTORY_SLOTS]),
            inventory_nbt: Mutex::new(vec![None; PLAYER_INVENTORY_SLOTS]),
            container: Mutex::new(None),
            sfx: Mutex::new(None),
            chat_log: Mutex::new(std::collections::VecDeque::new()),
            carried: Mutex::new(None),
            window_state: Mutex::new(0),
            dig: Mutex::new(None),
            running: AtomicBool::new(true),
        }
    }

    /// Enqueues a discrete presentation command without blocking the caller.
    /// Returns `false` if the bounded queue is full or unavailable.
    pub fn queue_command(&self, command: ClientCommand) -> bool {
        match self.commands.try_push(command.clone()) {
            Ok(()) => {
                let mut runtime = self.core.lock().unwrap();
                let _ = runtime.core.apply_command(command.clone());
                if let Some(replay) = &mut runtime.replay {
                    replay.record_command(command);
                }
                *self.snapshot.write().unwrap() = Arc::new(runtime.core.snapshot().clone());
                true
            }
            Err(error) => {
                tracing::warn!(%error, "dropping client command");
                false
            }
        }
    }

    /// Applies a semantic event atomically and publishes one coherent snapshot.
    fn apply_core_event(&self, event: ClientEvent) {
        let mut runtime = self.core.lock().unwrap();
        runtime.core.apply_event(event.clone());
        if let Some(replay) = &mut runtime.replay {
            replay.record_event(event);
        }
        *self.snapshot.write().unwrap() = Arc::new(runtime.core.snapshot().clone());
    }

    /// Latest coherent renderer- and diagnostics-facing session snapshot.
    pub fn snapshot(&self) -> Arc<ClientSnapshot> {
        Arc::clone(&self.snapshot.read().unwrap())
    }

    /// Enables opt-in, redacted semantic replay capture for this session.
    pub fn enable_replay(&self, output: PathBuf) {
        self.core.lock().unwrap().replay = Some(ReplayRecorder::new(self.context));
        *self.replay_output.lock().unwrap() = Some(output);
    }

    fn flush_replay(&self) {
        let output = self.replay_output.lock().unwrap().clone();
        let Some(output) = output else {
            return;
        };
        let json = self
            .core
            .lock()
            .unwrap()
            .replay
            .as_ref()
            .and_then(|recorder| recorder.replay().to_json().ok());
        if let Some(json) = json {
            if let Err(error) = std::fs::write(&output, json) {
                tracing::warn!(%error, path = %output.display(), "failed to write semantic replay");
            } else {
                tracing::info!(path = %output.display(), "wrote redacted semantic replay");
            }
        }
    }
}

/// Marks a chunk and its 4 neighbours dirty (neighbours so border face-culling
/// updates when an adjacent chunk's blocks change).
fn mark_dirty(shared: &Arc<Shared>, cx: i32, cz: i32) {
    shared.dirty_chunks.mark_neighborhood(cx, cz);
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

/// Resolves the configured wire protocol for startup-time registry and asset
/// selection. Connection setup performs the same validation again.
pub fn configured_session_context() -> Result<SessionContext> {
    let protocol = match std::env::var("CRABCRAFT_PROTOCOL") {
        Ok(value) => value
            .parse()
            .map_err(|error| anyhow::anyhow!("invalid CRABCRAFT_PROTOCOL: {error}"))?,
        Err(std::env::VarError::NotPresent) => ProtocolVersion::default(),
        Err(error) => bail!("invalid CRABCRAFT_PROTOCOL: {error}"),
    };
    Ok(SessionContext::new(protocol))
}

#[cfg(test)]
fn canonical_clientbound_764_id(wire: i32) -> i32 {
    ProtocolVersion::V1_20_2
        .canonical_clientbound_id(wire)
        .unwrap_or(-1)
}

#[cfg(test)]
fn canonical_clientbound_765_id(wire: i32) -> i32 {
    ProtocolVersion::V1_20_3
        .canonical_clientbound_id(wire)
        .unwrap_or(-1)
}

#[cfg(test)]
fn canonical_clientbound_766_id(wire: i32) -> i32 {
    ProtocolVersion::V1_20_5
        .canonical_clientbound_id(wire)
        .unwrap_or(-1)
}

#[cfg(test)]
fn canonical_clientbound_768_id(wire: i32) -> i32 {
    ProtocolVersion::V1_21_2
        .canonical_clientbound_id(wire)
        .unwrap_or(-1)
}

#[cfg(test)]
fn canonical_clientbound_770_id(wire: i32) -> i32 {
    ProtocolVersion::V1_21_5
        .canonical_clientbound_id(wire)
        .unwrap_or(-1)
}

#[cfg(test)]
fn serverbound_764_id(state: State, canonical: i32) -> i32 {
    ProtocolVersion::V1_20_2.serverbound_id(state, canonical)
}

#[cfg(test)]
fn serverbound_765_id(state: State, canonical: i32) -> i32 {
    ProtocolVersion::V1_20_3.serverbound_id(state, canonical)
}

#[cfg(test)]
fn serverbound_766_id(state: State, canonical: i32) -> i32 {
    ProtocolVersion::V1_20_5.serverbound_id(state, canonical)
}

#[cfg(test)]
fn serverbound_768_id(state: State, canonical: i32) -> i32 {
    ProtocolVersion::V1_21_2.serverbound_id(state, canonical)
}

#[cfg(test)]
fn serverbound_769_id(state: State, canonical: i32) -> i32 {
    ProtocolVersion::V1_21_4.serverbound_id(state, canonical)
}

#[cfg(test)]
fn serverbound_770_id(state: State, canonical: i32) -> i32 {
    ProtocolVersion::V1_21_5.serverbound_id(state, canonical)
}

fn offline_uuid(name: &str) -> uuid::Uuid {
    let mut bytes = md5::compute(format!("OfflinePlayer:{name}")).0;
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes)
}

async fn configuration_loop<S>(
    conn: &mut Connection<S>,
    shared: &Arc<Shared>,
    protocol: ProtocolVersion,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if matches!(
        protocol,
        ProtocolVersion::V1_21_2 | ProtocolVersion::V1_21_4 | ProtocolVersion::V1_21_5
    ) {
        conn.send_unmapped(&configuration768::ClientInformation::sensible_defaults())
            .await?;
    } else {
        conn.send(&ConfigurationClientInformation {
            locale: "en_us".to_string(),
            view_distance: 10,
            chat_mode: 0,
            chat_colors: true,
            skin_parts: 0x7f,
            main_hand: 1,
            text_filtering: false,
            server_listing: true,
        })
        .await?;
    }
    let mut choices = tokio::time::interval(Duration::from_millis(50));
    loop {
        tokio::select! {
            raw = conn.read_packet() => {
                let raw = raw.context("reading configuration packet")?;
                if protocol.uses_split_registry() {
                    match raw.id {
                        0x02 => {
                            let mut body = raw.body.clone();
                            let reason = read_text_component(&mut body, protocol)?;
                            bail!("configuration refused: {reason}");
                        }
                        0x03 => {
                            conn.send(&FinishConfiguration).await?;
                            return Ok(());
                        }
                        0x04 => {
                            let packet: ConfigurationKeepAlive = raw.decode()?;
                            conn.send(&ConfigurationKeepAliveResponse { id: packet.id }).await?;
                        }
                        0x05 => {
                            let packet: ConfigurationPing = raw.decode()?;
                            conn.send(&ConfigurationPong { id: packet.id }).await?;
                        }
                        0x07 => {
                            let packet: configuration766::RegistryData = raw.decode()?;
                            merge_registry_data_766(shared, packet);
                        }
                        0x08 => handle_remove_resource_pack_765(&raw, shared)?,
                        0x09 => {
                            let packet: configuration765::AddResourcePack = raw.decode()?;
                            set_resource_pack_request(
                                shared,
                                Some(packet.uuid),
                                packet.url,
                                packet.hash,
                                packet.forced,
                                packet.prompt.as_ref().map(nbt_plain_text),
                            );
                        }
                        0x0e => {
                            conn.send_unmapped(&configuration766::SelectKnownPacks).await?;
                        }
                        _ => {}
                    }
                    continue;
                }
                match raw.id {
                    0x01 => {
                        let mut body = raw.body.clone();
                        let reason = read_text_component(&mut body, protocol)?;
                        bail!("configuration refused: {reason}");
                    }
                    id if id == ClientboundFinishConfiguration::ID => {
                        conn.send(&FinishConfiguration).await?;
                        return Ok(());
                    }
                    id if id == ConfigurationKeepAlive::ID => {
                        let packet: ConfigurationKeepAlive = raw.decode()?;
                        conn.send(&ConfigurationKeepAliveResponse { id: packet.id }).await?;
                    }
                    id if id == ConfigurationPing::ID => {
                        let packet: ConfigurationPing = raw.decode()?;
                        conn.send(&ConfigurationPong { id: packet.id }).await?;
                    }
                    id if id == RegistryData::ID => {
                        let packet: RegistryData = raw.decode()?;
                        *shared.registry_codec.lock().unwrap() = Some(packet.codec);
                    }
                    0x06 if protocol == ProtocolVersion::V1_20_2 => {
                        handle_resource_pack_request(&raw, shared)?;
                    }
                    0x06 if protocol == ProtocolVersion::V1_20_3 => {
                        handle_remove_resource_pack_765(&raw, shared)?;
                    }
                    0x07 if protocol == ProtocolVersion::V1_20_3 => {
                        let packet: configuration765::AddResourcePack = raw.decode()?;
                        set_resource_pack_request(
                            shared,
                            Some(packet.uuid),
                            packet.url,
                            packet.hash,
                            packet.forced,
                            packet.prompt.as_ref().map(nbt_plain_text),
                        );
                    }
                    _ => {}
                }
            }
            _ = choices.tick() => {
                let commands = shared.commands.take_matching(|command| matches!(
                    command,
                    ClientCommand::ResourcePackDecision(_) | ClientCommand::ResourcePackStatus(_)
                ))?;
                for command in commands {
                    let ClientCommand::ResourcePackDecision(accepted) = command else {
                        if let ClientCommand::ResourcePackStatus(status) = command {
                            send_configuration_pack_status(conn, shared, protocol, status).await?;
                        }
                        continue;
                    };
                    if !accepted {
                        send_configuration_pack_status(conn, shared, protocol, 1).await?;
                        continue;
                    }
                    send_configuration_pack_status(conn, shared, protocol, 3).await?;
                    let request = shared.resource_pack_request.lock().unwrap().clone();
                    let base = shared.base_resource_jar.lock().unwrap().clone();
                    match request {
                        Some(request) => match download_resource_pack(&request).await {
                            Ok(archive) => match activate_resource_pack(
                                shared,
                                request.uuid,
                                archive,
                                base.as_deref(),
                            ) {
                                Ok(path) => {
                                    *shared.cached_resource_pack.lock().unwrap() = Some(path);
                                    if !shared.renderer_active.load(Ordering::SeqCst) {
                                        send_configuration_pack_status(conn, shared, protocol, 0).await?;
                                    }
                                }
                                Err(error) => {
                                    tracing::warn!(%error, "configuration resource-pack activation failed");
                                    send_configuration_pack_status(conn, shared, protocol, 2).await?;
                                }
                            },
                            Err(error) => {
                                tracing::warn!(%error, "configuration resource-pack download failed");
                                send_configuration_pack_status(conn, shared, protocol, 2).await?;
                            }
                        },
                        None => send_configuration_pack_status(conn, shared, protocol, 2).await?,
                    }
                }
            }
        }
    }
}

async fn send_configuration_pack_status<S>(
    conn: &mut Connection<S>,
    shared: &Arc<Shared>,
    protocol: ProtocolVersion,
    status: i32,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    match protocol {
        ProtocolVersion::V1_20_2 => {
            conn.send(&ConfigurationResourcePackStatus { status })
                .await?;
        }
        ProtocolVersion::V1_20_3
        | ProtocolVersion::V1_20_5
        | ProtocolVersion::V1_21
        | ProtocolVersion::V1_21_2
        | ProtocolVersion::V1_21_4
        | ProtocolVersion::V1_21_5 => {
            let uuid = shared
                .resource_pack_request
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|request| request.uuid)
                .context("765 resource-pack status has no pack UUID")?;
            conn.send(&configuration765::ResourcePackStatus { uuid, status })
                .await?;
        }
        ProtocolVersion::V1_20_1 => bail!("configuration status used in protocol 763"),
    }
    Ok(())
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
    shared.apply_core_event(ClientEvent::ConnectionPhaseChanged(
        ConnectionPhase::Connecting,
    ));
    let result = run_inner(addr, &login, &shared, deadline).await;
    let reason = result
        .as_ref()
        .err()
        .map_or_else(|| "session ended".to_string(), ToString::to_string);
    shared.apply_core_event(ClientEvent::Disconnected { reason });
    shared.flush_replay();
    shared.running.store(false, Ordering::SeqCst);
    result
}

async fn run_inner(
    addr: &str,
    login: &LoginMode,
    shared: &Arc<Shared>,
    deadline: Option<Duration>,
) -> Result<()> {
    let protocol = shared.context.protocol;
    let (host, port) = split_host_port(addr);
    let (name, uuid) = match login {
        LoginMode::Offline { username } => (username.clone(), None),
        LoginMode::Online(session) => (session.username.clone(), Some(session.uuid)),
    };
    let session = match login {
        LoginMode::Online(session) => Some(session),
        LoginMode::Offline { .. } => None,
    };
    tracing::info!(server = %addr, username = %name, online = session.is_some(), protocol = protocol.number(), "connecting");

    let mut conn = Connection::connect(addr)
        .await
        .with_context(|| format!("failed to connect to {addr}"))?;

    conn.send(&Handshake {
        protocol_version: protocol.number(),
        server_address: host,
        server_port: port,
        next_state: NextState::Login,
    })
    .await?;
    conn.set_state(State::Login);
    shared.apply_core_event(ClientEvent::ConnectionPhaseChanged(ConnectionPhase::Login));
    match protocol {
        ProtocolVersion::V1_20_1 => {
            conn.send(&LoginStart {
                name: name.clone(),
                uuid,
            })
            .await?;
        }
        ProtocolVersion::V1_20_2
        | ProtocolVersion::V1_20_3
        | ProtocolVersion::V1_20_5
        | ProtocolVersion::V1_21
        | ProtocolVersion::V1_21_2
        | ProtocolVersion::V1_21_4
        | ProtocolVersion::V1_21_5 => {
            conn.send(&LoginStart764 {
                name: name.clone(),
                uuid: uuid.unwrap_or_else(|| offline_uuid(&name)),
            })
            .await?;
        }
    }

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
                if protocol != ProtocolVersion::V1_20_1 {
                    if let Some(mapper) = protocol.serverbound_mapper() {
                        conn.set_packet_id_mapper(mapper);
                    }
                    conn.send(&LoginAcknowledged).await?;
                    conn.set_state(State::Configuration);
                    shared.apply_core_event(ClientEvent::ConnectionPhaseChanged(
                        ConnectionPhase::Configuration,
                    ));
                    configuration_loop(&mut conn, shared, protocol).await?;
                }
                conn.set_state(State::Play);
                shared.apply_core_event(ClientEvent::ConnectionPhaseChanged(ConnectionPhase::Play));
                break;
            }
            _ => {}
        }
    }

    play_loop(&mut conn, &name, shared, deadline, protocol).await
}

async fn play_loop<S>(
    conn: &mut Connection<S>,
    username: &str,
    shared: &Arc<Shared>,
    deadline: Option<Duration>,
    protocol: ProtocolVersion,
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
    let mut was_using = false;
    let mut was_jumping = false;
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
                if protocol != ProtocolVersion::V1_20_1
                    && (raw.id == 0x0c
                        || protocol == ProtocolVersion::V1_21_5 && raw.id == 0x0b)
                {
                    if matches!(
                        protocol,
                        ProtocolVersion::V1_21_2
                            | ProtocolVersion::V1_21_4
                            | ProtocolVersion::V1_21_5
                    ) {
                        conn.send_unmapped(&play768::ChunkBatchReceived {
                            chunks_per_tick: 64.0,
                        })
                        .await?;
                    } else if protocol.uses_split_registry() {
                        conn.send_unmapped(&play766::ChunkBatchReceived {
                            chunks_per_tick: 64.0,
                        })
                        .await?;
                    } else {
                        conn.send_unmapped(&ChunkBatchReceived {
                            chunks_per_tick: 64.0,
                        })
                        .await?;
                    }
                }
                if protocol == ProtocolVersion::V1_20_3 && raw.id == 0x67 {
                    conn.send_unmapped(&play765::ConfigurationAcknowledged).await?;
                    conn.set_state(State::Configuration);
                    configuration_loop(conn, shared, protocol).await?;
                    conn.set_state(State::Play);
                    continue;
                }
                if matches!(protocol, ProtocolVersion::V1_20_5 | ProtocolVersion::V1_21)
                    && raw.id == 0x69
                {
                    conn.send_unmapped(&play766::ConfigurationAcknowledged).await?;
                    conn.set_state(State::Configuration);
                    configuration_loop(conn, shared, protocol).await?;
                    conn.set_state(State::Play);
                    continue;
                }
                if matches!(
                    protocol,
                    ProtocolVersion::V1_21_2 | ProtocolVersion::V1_21_4
                ) && raw.id == 0x70
                {
                    conn.send_unmapped(&play766::ConfigurationAcknowledged).await?;
                    conn.set_state(State::Configuration);
                    configuration_loop(conn, shared, protocol).await?;
                    conn.set_state(State::Play);
                    continue;
                }
                if protocol == ProtocolVersion::V1_21_5 && raw.id == 0x6f {
                    conn.send_unmapped(&play766::ConfigurationAcknowledged).await?;
                    conn.set_state(State::Configuration);
                    configuration_loop(conn, shared, protocol).await?;
                    conn.set_state(State::Play);
                    continue;
                }
                if protocol == ProtocolVersion::V1_20_3 && raw.id == 0x42 {
                    handle_reset_score_765(&raw, shared)?;
                    continue;
                }
                if protocol == ProtocolVersion::V1_20_3 && raw.id == 0x43 {
                    handle_remove_resource_pack_765(&raw, shared)?;
                    continue;
                }
                if matches!(protocol, ProtocolVersion::V1_20_5 | ProtocolVersion::V1_21)
                    && raw.id == 0x44
                {
                    handle_reset_score_765(&raw, shared)?;
                    continue;
                }
                if matches!(protocol, ProtocolVersion::V1_20_5 | ProtocolVersion::V1_21)
                    && raw.id == 0x45
                {
                    handle_remove_resource_pack_765(&raw, shared)?;
                    continue;
                }
                if matches!(protocol, ProtocolVersion::V1_20_5 | ProtocolVersion::V1_21)
                    && raw.id == 0x46
                {
                    handle_resource_pack_request_765(&raw, shared)?;
                    continue;
                }
                if protocol == ProtocolVersion::V1_21_5 {
                    match raw.id {
                        0x48 => {
                            handle_reset_score_765(&raw, shared)?;
                            continue;
                        }
                        0x49 => {
                            handle_remove_resource_pack_765(&raw, shared)?;
                            continue;
                        }
                        0x4a => {
                            handle_resource_pack_request_765(&raw, shared)?;
                            continue;
                        }
                        0x43 => {
                            handle_recipe_book_add_768(&raw, shared)?;
                            continue;
                        }
                        0x44 => {
                            handle_recipe_book_remove_768(&raw, shared)?;
                            continue;
                        }
                        0x45 => continue,
                        0x59 => {
                            let mut body = raw.body.clone();
                            *shared.carried.lock().unwrap() =
                                play770::read_component_slot(&mut body)?.item;
                            continue;
                        }
                        0x65 => {
                            let mut body = raw.body.clone();
                            let slot = body.read_varint()?;
                            let decoded = play770::read_component_slot(&mut body)?;
                            if let Ok(slot) = usize::try_from(slot) {
                                if let Some(destination) =
                                    shared.inventory.lock().unwrap().get_mut(slot)
                                {
                                    *destination = decoded.item;
                                }
                                if let Some(destination) =
                                    shared.inventory_nbt.lock().unwrap().get_mut(slot)
                                {
                                    *destination = decoded.metadata;
                                }
                            }
                            continue;
                        }
                        _ => {}
                    }
                }
                if matches!(
                    protocol,
                    ProtocolVersion::V1_21_2 | ProtocolVersion::V1_21_4
                ) && raw.id == 0x49
                {
                    handle_reset_score_765(&raw, shared)?;
                    continue;
                }
                if matches!(
                    protocol,
                    ProtocolVersion::V1_21_2 | ProtocolVersion::V1_21_4
                ) && raw.id == 0x4a
                {
                    handle_remove_resource_pack_765(&raw, shared)?;
                    continue;
                }
                if matches!(
                    protocol,
                    ProtocolVersion::V1_21_2 | ProtocolVersion::V1_21_4
                ) && raw.id == 0x4b
                {
                    handle_resource_pack_request_765(&raw, shared)?;
                    continue;
                }
                if matches!(
                    protocol,
                    ProtocolVersion::V1_21_2 | ProtocolVersion::V1_21_4
                ) && raw.id == 0x44
                {
                    handle_recipe_book_add_768(&raw, shared)?;
                    continue;
                }
                if matches!(
                    protocol,
                    ProtocolVersion::V1_21_2 | ProtocolVersion::V1_21_4
                ) && raw.id == 0x45
                {
                    handle_recipe_book_remove_768(&raw, shared)?;
                    continue;
                }
                if matches!(
                    protocol,
                    ProtocolVersion::V1_21_2 | ProtocolVersion::V1_21_4
                ) && raw.id == 0x46
                {
                    // Recipe-book open/filter preferences are UI-local today;
                    // consume the fixed eight booleans by ignoring this framed body.
                    continue;
                }
                if protocol == ProtocolVersion::V1_21_2 && raw.id == 0x5a {
                    let mut body = raw.body.clone();
                    let carried = if body.read_bool()? {
                        play766::read_component_slot_768(&mut body)?.item
                    } else {
                        None
                    };
                    *shared.carried.lock().unwrap() = carried;
                    continue;
                }
                if protocol == ProtocolVersion::V1_21_4 && raw.id == 0x5a {
                    let mut body = raw.body.clone();
                    *shared.carried.lock().unwrap() =
                        play766::read_component_slot_768(&mut body)?.item;
                    continue;
                }
                if protocol == ProtocolVersion::V1_21_2 && raw.id == 0x66 {
                    let mut body = raw.body.clone();
                    let slot = body.read_varint()?;
                    let item = if body.read_bool()? {
                        play766::read_component_slot_768(&mut body)?.item
                    } else {
                        None
                    };
                    if let Ok(slot) = usize::try_from(slot) {
                        if let Some(destination) =
                            shared.inventory.lock().unwrap().get_mut(slot)
                        {
                            *destination = item;
                        }
                    }
                    continue;
                }
                if protocol == ProtocolVersion::V1_21_4 && raw.id == 0x66 {
                    let mut body = raw.body.clone();
                    let slot = body.read_varint()?;
                    let decoded = play766::read_component_slot_768(&mut body)?;
                    if let Ok(slot) = usize::try_from(slot) {
                        if let Some(destination) = shared.inventory.lock().unwrap().get_mut(slot) {
                            *destination = decoded.item;
                        }
                        if let Some(destination) =
                            shared.inventory_nbt.lock().unwrap().get_mut(slot)
                        {
                            *destination = decoded.metadata;
                        }
                    }
                    continue;
                }
                let packet_id = protocol.canonical_clientbound_id(raw.id).unwrap_or(-1);
                match packet_id {
                    id if id == KeepAlive::ID => {
                        let k: KeepAlive = raw.decode()?;
                        conn.send(&KeepAliveResponse { id: k.id }).await?;
                    }
                    id if id == Ping::ID => {
                        let ping: Ping = raw.decode()?;
                        conn.send(&Pong { id: ping.id }).await?;
                    }
                    id if id == SynchronizePlayerPosition::ID => {
                        let (p, corrected_velocity) = if matches!(
                            protocol,
                            ProtocolVersion::V1_21_2
                                | ProtocolVersion::V1_21_4
                                | ProtocolVersion::V1_21_5
                        ) {
                            let update: play768::SynchronizePlayerPosition = raw.decode()?;
                            (
                                SynchronizePlayerPosition {
                                    x: update.x,
                                    y: update.y,
                                    z: update.z,
                                    yaw: update.yaw,
                                    pitch: update.pitch,
                                    flags: update.flags as u8,
                                    teleport_id: update.teleport_id,
                                },
                                Some((update.velocity, update.flags)),
                            )
                        } else {
                            (raw.decode()?, None)
                        };
                        let (just_spawned, pos) = {
                            let mut ps = shared.player.lock().unwrap();
                            ps.apply(&p);
                            if let Some((velocity, flags)) = corrected_velocity {
                                for (axis, relative_flag) in [0x20, 0x40, 0x80].into_iter().enumerate() {
                                    ps.vel[axis] = if flags & relative_flag != 0 {
                                        ps.vel[axis] + velocity[axis]
                                    } else {
                                        velocity[axis]
                                    };
                                }
                            }
                            let js = !ps.spawned;
                            ps.spawned = true;
                            (js, *ps)
                        };
                        shared.apply_core_event(ClientEvent::PositionSynchronized {
                            position: [pos.x, pos.y, pos.z],
                            yaw: pos.yaw,
                            pitch: pos.pitch,
                        });
                        conn.send(&ConfirmTeleport { teleport_id: p.teleport_id }).await?;
                        conn.send(&SetPlayerPositionRotation {
                            x: pos.x, y: pos.y, z: pos.z, yaw: pos.yaw, pitch: pos.pitch,
                            on_ground: true,
                        }).await?;
                        if just_spawned {
                            tracing::info!("spawned at ({:.1}, {:.1}, {:.1})", pos.x, pos.y, pos.z);
                            if matches!(
                                protocol,
                                ProtocolVersion::V1_21_2
                                    | ProtocolVersion::V1_21_4
                                    | ProtocolVersion::V1_21_5
                            ) {
                                conn.send_unmapped(&play768::ClientInformation(
                                    configuration768::ClientInformation::sensible_defaults(),
                                ))
                                .await?;
                                if matches!(
                                    protocol,
                                    ProtocolVersion::V1_21_4 | ProtocolVersion::V1_21_5
                                ) {
                                    conn.send_unmapped(&play769::PlayerLoaded).await?;
                                }
                            } else {
                                conn.send(&ClientInformation::sensible_defaults()).await?;
                            }
                        }
                        if !greeted {
                            greeted = true;
                            let msg = format!("{username} here via Crabcraft (pure Rust).");
                            if protocol == ProtocolVersion::V1_21_5 {
                                conn.send_unmapped(&play770::ClientChatMessage::unsigned(msg))
                                    .await?;
                            } else {
                                conn.send(&ClientChatMessage::unsigned(msg)).await?;
                            }
                        }
                    }
                    id if id == SystemChat::ID => {
                        let (line, overlay) = if protocol.uses_nbt_components() {
                            let mut body = raw.body.clone();
                            let component = crab_protocol::nbt::read_anonymous_nbt(&mut body)?;
                            (nbt_plain_text(&component), body.read_bool()?)
                        } else {
                            let component: SystemChat = raw.decode()?;
                            (plain_text(&component.content), component.overlay)
                        };
                        if overlay {
                            *shared.action_bar.lock().unwrap() = Some((line, 60));
                        } else {
                            tracing::info!(target: "chat", "{line}");
                            push_chat(shared, line);
                        }
                    }
                    id if id == ID_ACTION_BAR => {
                        let mut b = raw.body.clone();
                        if let Ok(text) = read_text_component(&mut b, protocol) {
                            *shared.action_bar.lock().unwrap() = Some((text, 60));
                        }
                    }
                    id if id == ID_TITLE_TEXT || id == ID_TITLE_SUBTITLE => {
                        let mut b = raw.body.clone();
                        if let Ok(text) = read_text_component(&mut b, protocol) {
                            let mut title = shared.title.lock().unwrap();
                            if id == ID_TITLE_TEXT {
                                title.title = text;
                            } else {
                                title.subtitle = text;
                            }
                            title.remaining = title.fade_in + title.stay + title.fade_out;
                        }
                    }
                    id if id == ID_TITLE_TIME => {
                        let mut b = raw.body.clone();
                        if let (Ok(fade_in), Ok(stay), Ok(fade_out)) =
                            (b.read_i32(), b.read_i32(), b.read_i32())
                        {
                            let mut title = shared.title.lock().unwrap();
                            title.fade_in = fade_in.max(0) as u32;
                            title.stay = stay.max(0) as u32;
                            title.fade_out = fade_out.max(0) as u32;
                        }
                    }
                    id if id == ID_CLEAR_TITLES => {
                        let mut b = raw.body.clone();
                        if let Ok(reset) = b.read_bool() {
                            let mut title = shared.title.lock().unwrap();
                            title.remaining = 0;
                            title.title.clear();
                            title.subtitle.clear();
                            if reset {
                                let defaults = TitleState::default();
                                title.fade_in = defaults.fade_in;
                                title.stay = defaults.stay;
                                title.fade_out = defaults.fade_out;
                            }
                        }
                    }
                    id if id == ID_BOSS_BAR => {
                        let mut b = raw.body.clone();
                        if let (Ok(uuid), Ok(action)) = (b.read_uuid(), b.read_varint()) {
                            let key = uuid.to_string();
                            let mut bars = shared.boss_bars.lock().unwrap();
                            match action {
                                0 => {
                                    if let (Ok(title), Ok(health), Ok(color), Ok(divisions), Ok(flags)) = (
                                        read_text_component(&mut b, protocol),
                                        b.read_f32(),
                                        b.read_varint(),
                                        b.read_varint(),
                                        b.read_u8(),
                                    ) {
                                        bars.insert(key, BossBarState {
                                            title,
                                            health: health.clamp(0.0, 1.0),
                                            color,
                                            divisions,
                                            flags,
                                        });
                                    }
                                }
                                1 => {
                                    bars.remove(&key);
                                }
                                2 => {
                                    if let (Some(bar), Ok(health)) = (bars.get_mut(&key), b.read_f32()) {
                                        bar.health = health.clamp(0.0, 1.0);
                                    }
                                }
                                3 => {
                                    if let (Some(bar), Ok(title)) =
                                        (bars.get_mut(&key), read_text_component(&mut b, protocol))
                                    {
                                        bar.title = title;
                                    }
                                }
                                4 => {
                                    if let (Some(bar), Ok(color), Ok(divisions)) =
                                        (bars.get_mut(&key), b.read_varint(), b.read_varint())
                                    {
                                        bar.color = color;
                                        bar.divisions = divisions;
                                    }
                                }
                                5 => {
                                    if let (Some(bar), Ok(flags)) = (bars.get_mut(&key), b.read_u8()) {
                                        bar.flags = flags;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    id if id == ID_INIT_WORLD_BORDER => {
                        let mut b = raw.body.clone();
                        if let (
                            Ok(x),
                            Ok(z),
                            Ok(diameter),
                            Ok(target_diameter),
                            Ok(speed),
                            Ok(_portal_boundary),
                            Ok(warning_blocks),
                            Ok(warning_time),
                        ) = (
                            b.read_f64(),
                            b.read_f64(),
                            b.read_f64(),
                            b.read_f64(),
                            b.read_varint(),
                            b.read_varint(),
                            b.read_varint(),
                            b.read_varint(),
                        ) {
                            *shared.world_border.lock().unwrap() = WorldBorderState {
                                x,
                                z,
                                diameter,
                                target_diameter,
                                lerp_remaining_ms: speed.max(0),
                                warning_blocks,
                                warning_time,
                            };
                        }
                    }
                    id if id == ID_WORLD_BORDER_CENTER => {
                        let mut b = raw.body.clone();
                        if let (Ok(x), Ok(z)) = (b.read_f64(), b.read_f64()) {
                            let mut border = shared.world_border.lock().unwrap();
                            border.x = x;
                            border.z = z;
                        }
                    }
                    id if id == ID_WORLD_BORDER_LERP => {
                        let mut b = raw.body.clone();
                        if let (Ok(diameter), Ok(target), Ok(speed)) =
                            (b.read_f64(), b.read_f64(), b.read_varint())
                        {
                            let mut border = shared.world_border.lock().unwrap();
                            border.diameter = diameter;
                            border.target_diameter = target;
                            border.lerp_remaining_ms = speed.max(0);
                        }
                    }
                    id if id == ID_WORLD_BORDER_SIZE => {
                        let mut b = raw.body.clone();
                        if let Ok(diameter) = b.read_f64() {
                            let mut border = shared.world_border.lock().unwrap();
                            border.diameter = diameter;
                            border.target_diameter = diameter;
                            border.lerp_remaining_ms = 0;
                        }
                    }
                    id if id == ID_WORLD_BORDER_WARNING_DELAY => {
                        let mut b = raw.body.clone();
                        if let Ok(warning_time) = b.read_varint() {
                            shared.world_border.lock().unwrap().warning_time = warning_time;
                        }
                    }
                    id if id == ID_WORLD_BORDER_WARNING_REACH => {
                        let mut b = raw.body.clone();
                        if let Ok(warning_blocks) = b.read_varint() {
                            shared.world_border.lock().unwrap().warning_blocks = warning_blocks;
                        }
                    }
                    id if id == ID_SCOREBOARD_DISPLAY => {
                        let mut b = raw.body.clone();
                        if let (Ok(position), Ok(name)) = (b.read_i8(), b.read_string(32767)) {
                            if position == 1 {
                                shared.scoreboard.lock().unwrap().sidebar =
                                    (!name.is_empty()).then_some(name);
                            }
                        }
                    }
                    id if id == ID_SCOREBOARD_OBJECTIVE => {
                        let mut b = raw.body.clone();
                        if let (Ok(name), Ok(action)) = (b.read_string(32767), b.read_i8()) {
                            let mut scoreboard = shared.scoreboard.lock().unwrap();
                            match action {
                                0 | 2 => {
                                    if let Ok(display) = read_text_component(&mut b, protocol) {
                                        let _render_type = b.read_varint();
                                        if protocol.uses_nbt_components() {
                                            skip_optional_number_format(&mut b)?;
                                        }
                                        scoreboard.objectives.insert(name, display);
                                    }
                                }
                                1 => {
                                    scoreboard.objectives.remove(&name);
                                    scoreboard.scores.remove(&name);
                                    if scoreboard.sidebar.as_deref() == Some(name.as_str()) {
                                        scoreboard.sidebar = None;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    id if id == ID_SCOREBOARD_SCORE => {
                        let mut b = raw.body.clone();
                        if protocol.uses_nbt_components() {
                            if let (Ok(item), Ok(objective), Ok(value)) =
                                (b.read_string(32767), b.read_string(32767), b.read_varint())
                            {
                                if b.read_bool()? {
                                    let _ = crab_protocol::nbt::read_anonymous_nbt(&mut b)?;
                                }
                                skip_optional_number_format(&mut b)?;
                                shared.scoreboard.lock().unwrap().scores
                                    .entry(objective).or_default().insert(item, value);
                            }
                        } else if let (Ok(item), Ok(action), Ok(objective)) =
                            (b.read_string(32767), b.read_varint(), b.read_string(32767)) {
                            let mut scoreboard = shared.scoreboard.lock().unwrap();
                            let scores = scoreboard.scores.entry(objective).or_default();
                            if action == 1 {
                                scores.remove(&item);
                            } else if let Ok(value) = b.read_varint() {
                                scores.insert(item, value);
                            }
                        }
                    }
                    id if id == ID_TEAMS => {
                        let _ = handle_team_packet(&raw, shared, protocol);
                    }
                    id if id == ID_PLAYER_INFO => {
                        let mut b = raw.body.clone();
                        if let (Ok(actions), Ok(count)) = (b.read_u8(), b.read_varint()) {
                            let mut list = shared.player_list.lock().unwrap();
                            for _ in 0..count.max(0) {
                                let Ok(uuid) = b.read_uuid() else { break };
                                let key = uuid.to_string();
                                let entry = list.entries.entry(key).or_default();
                                if actions & 0x01 != 0 {
                                    if let Ok(name) = b.read_string(16) {
                                        entry.name = name;
                                    }
                                    let properties = b.read_varint().unwrap_or(0).max(0);
                                    for _ in 0..properties {
                                        let _ = b.read_string(32767);
                                        let _ = b.read_string(32767);
                                        if b.read_bool().unwrap_or(false) {
                                            let _ = b.read_string(32767);
                                        }
                                    }
                                }
                                if actions & 0x02 != 0 && b.read_bool().unwrap_or(false) {
                                    let _ = b.read_uuid();
                                    let _ = b.read_i64();
                                    for _ in 0..2 {
                                        let len = b.read_varint().unwrap_or(0).max(0) as usize;
                                        let _ = b.read_bytes(len);
                                    }
                                }
                                if actions & 0x04 != 0 {
                                    entry.game_mode = b.read_varint().unwrap_or(0);
                                }
                                if actions & 0x08 != 0 {
                                    entry.listed = b.read_varint().unwrap_or(0) != 0;
                                }
                                if actions & 0x10 != 0 {
                                    entry.latency = b.read_varint().unwrap_or(0);
                                }
                                if actions & 0x20 != 0 {
                                    entry.display_name = if b.read_bool().unwrap_or(false) {
                                        read_text_component(&mut b, protocol).ok()
                                    } else {
                                        None
                                    };
                                }
                                if protocol == ProtocolVersion::V1_21_2 && actions & 0x40 != 0 {
                                    let _list_order = b.read_varint();
                                }
                                if matches!(
                                    protocol,
                                    ProtocolVersion::V1_21_4 | ProtocolVersion::V1_21_5
                                ) {
                                    if actions & 0x80 != 0 {
                                        let _list_order = b.read_varint();
                                    }
                                    if actions & 0x40 != 0 {
                                        let _show_hat = b.read_bool();
                                    }
                                }
                            }
                        }
                    }
                    id if id == ID_PLAYER_REMOVE => {
                        let mut b = raw.body.clone();
                        if let Ok(count) = b.read_varint() {
                            let mut list = shared.player_list.lock().unwrap();
                            for _ in 0..count.max(0) {
                                if let Ok(uuid) = b.read_uuid() {
                                    list.entries.remove(&uuid.to_string());
                                }
                            }
                        }
                    }
                    id if id == ID_PLAYER_LIST_HEADER => {
                        let mut b = raw.body.clone();
                        if let (Ok(header), Ok(footer)) =
                            (read_text_component(&mut b, protocol), read_text_component(&mut b, protocol))
                        {
                            let mut list = shared.player_list.lock().unwrap();
                            list.header = header;
                            list.footer = footer;
                        }
                    }
                    id if id == ID_PARTICLES => {
                        let mut b = raw.body.clone();
                        let modern = protocol.uses_data_components();
                        let legacy_particle_id = (!modern).then(|| b.read_varint());
                        let long_distance = b.read_bool();
                        let always_show = if matches!(
                            protocol,
                            ProtocolVersion::V1_21_4 | ProtocolVersion::V1_21_5
                        ) {
                            b.read_bool()
                        } else {
                            Ok(false)
                        };
                        let position = (b.read_f64(), b.read_f64(), b.read_f64());
                        let offsets = (b.read_f32(), b.read_f32(), b.read_f32());
                        let speed = b.read_f32();
                        let count = b.read_i32();
                        let particle_id = if modern {
                            b.read_varint()
                        } else {
                            legacy_particle_id.expect("legacy particle id is present")
                        };
                        if let (
                            Ok(particle_id),
                            Ok(long_distance),
                            Ok(_always_show),
                            Ok(x),
                            Ok(y),
                            Ok(z),
                            Ok(offset_x),
                            Ok(offset_y),
                            Ok(offset_z),
                            Ok(speed),
                            Ok(count),
                        ) = (
                            particle_id,
                            long_distance,
                            always_show,
                            position.0,
                            position.1,
                            position.2,
                            offsets.0,
                            offsets.1,
                            offsets.2,
                            speed,
                            count,
                        ) {
                            let player = *shared.player.lock().unwrap();
                            let dx = x - player.x;
                            let dy = y - player.y;
                            let dz = z - player.z;
                            let max_distance = if long_distance { 256.0 } else { 32.0 };
                            if dx * dx + dy * dy + dz * dz <= max_distance * max_distance {
                                let mut particles = shared.particles.lock().unwrap();
                                let spawn_count = count.clamp(1, 128);
                                for index in 0..spawn_count {
                                    let hash = |salt: i32| {
                                        let value = (index * 1_103_515_245 + salt * 12_345)
                                            .wrapping_abs()
                                            % 2_001;
                                        value as f32 / 1_000.0 - 1.0
                                    };
                                    let spread = if count == 0 { 0.0 } else { 1.0 };
                                    particles.push(Particle {
                                        position: [
                                            x as f32 + hash(1) * offset_x * spread,
                                            y as f32 + hash(2) * offset_y * spread,
                                            z as f32 + hash(3) * offset_z * spread,
                                        ],
                                        velocity: [
                                            hash(4) * speed,
                                            hash(5) * speed,
                                            hash(6) * speed,
                                        ],
                                        color: particle_color(particle_id),
                                        age: 0,
                                        lifetime: 20 + (particle_id.unsigned_abs() % 30) as u16,
                                    });
                                }
                                if particles.len() > 4_096 {
                                    let excess = particles.len() - 4_096;
                                    particles.drain(..excess);
                                }
                            }
                        }
                    }
                    id if id == ID_MAP_DATA => {
                        let _ = handle_map_data(&raw, shared, protocol);
                    }
                    id if id == ID_JOIN_GAME => {
                        if let Err(e) = handle_join_game(&raw, shared, protocol) {
                            tracing::warn!("Join Game parse failed: {e}");
                        }
                    }
                    id if id == ID_MAP_CHUNK => {
                        let mut body = raw.body.clone();
                        let section_count = shared.world.lock().unwrap().section_count();
                        let chunk = if protocol == ProtocolVersion::V1_20_1 {
                            Chunk::parse(&mut body, section_count)
                        } else if protocol == ProtocolVersion::V1_21_5 {
                            Chunk::parse_770(&mut body, section_count)
                        } else {
                            Chunk::parse_network(&mut body, section_count)
                        };
                        let parsed = match chunk {
                            Ok(chunk) => {
                                let coord = (chunk.x, chunk.z);
                                match read_chunk_signs(
                                    &mut body,
                                    chunk.x,
                                    chunk.z,
                                    protocol,
                                ) {
                                    Ok(signs) => replace_chunk_signs(shared, coord, signs),
                                    Err(error) => tracing::warn!(
                                        %error,
                                        chunk_x = chunk.x,
                                        chunk_z = chunk.z,
                                        "chunk block-entity parse failed"
                                    ),
                                }
                                shared.world.lock().unwrap().load_chunk(chunk);
                                Some(coord)
                            }
                            Err(e) => {
                                tracing::warn!("chunk parse failed: {e}");
                                None
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
                            if !crab_registry::block_name(s as u32)
                                .is_some_and(|name| name.ends_with("_sign"))
                            {
                                shared.signs.lock().unwrap().remove(&(bx, by, bz));
                            }
                            mark_dirty(shared, bx >> 4, bz >> 4);
                        }
                    }
                    id if id == ID_BLOCK_ENTITY_DATA => {
                        let _ = handle_block_entity_data(&raw, shared, protocol);
                    }
                    id if id == ID_SPAWN_ENTITY => {
                        let _ = handle_spawn_object(&raw, shared);
                    }
                    id if id == ID_SPAWN_PLAYER => {
                        let _ = handle_spawn_player(&raw, shared);
                    }
                    id if id == ID_REL_MOVE => {
                        let _ = handle_rel_move(&raw, shared, false);
                    }
                    id if id == ID_MOVE_LOOK => {
                        let _ = handle_rel_move(&raw, shared, true);
                    }
                    id if id == ID_ENTITY_ROTATION => {
                        let _ = handle_entity_rotation(&raw, shared);
                    }
                    id if id == ID_ENTITY_TELEPORT => {
                        let _ = handle_entity_teleport(&raw, shared);
                    }
                    id if id == ID_VEHICLE_MOVE => {
                        let _ = handle_vehicle_move(&raw, shared);
                    }
                    id if id == ID_ENTITY_HEAD_ROTATION => {
                        let _ = handle_entity_head_rotation(&raw, shared);
                    }
                    id if id == ID_ENTITY_VELOCITY => {
                        let _ = handle_entity_velocity(&raw, shared);
                    }
                    id if id == ID_ENTITY_EQUIPMENT => {
                        let _ = handle_entity_equipment(&raw, shared, protocol);
                    }
                    id if id == ID_SET_PASSENGERS => {
                        let _ = handle_set_passengers(&raw, shared);
                    }
                    id if id == ID_ENTITY_DESTROY => {
                        let _ = handle_entity_destroy(&raw, shared);
                    }
                    id if id == ID_ENTITY_METADATA => {
                        let _ = handle_entity_metadata(&raw, shared, protocol);
                    }
                    id if id == ID_ENTITY_ANIMATION => {
                        let mut b = raw.body.clone();
                        if let (Ok(entity_id), Ok(animation)) = (b.read_varint(), b.read_u8()) {
                            if matches!(animation, 0 | 3) {
                                if let Some(entity) =
                                    shared.entities.lock().unwrap().get_mut(&entity_id)
                                {
                                    entity.swing_sequence = entity.swing_sequence.wrapping_add(1);
                                }
                            }
                        }
                    }
                    id if id == ID_HURT_ANIMATION => {
                        let mut b = raw.body.clone();
                        if let Ok(entity_id) = b.read_varint() {
                            if let Some(entity) =
                                shared.entities.lock().unwrap().get_mut(&entity_id)
                            {
                                entity.hurt_sequence = entity.hurt_sequence.wrapping_add(1);
                            }
                        }
                    }
                    id if id == ID_EXPERIENCE => {
                        let mut b = raw.body.clone();
                        if let (Ok(bar), Ok(level)) = (b.read_f32(), b.read_varint()) {
                            let mut ps = shared.player.lock().unwrap();
                            ps.xp_bar = bar;
                            ps.xp_level = level;
                        }
                    }
                    id if id == ID_GAME_STATE => {
                        let mut b = raw.body.clone();
                        if let (Ok(reason), Ok(value)) = (b.read_u8(), b.read_f32()) {
                            match reason {
                                // Change game mode (so runtime /gamemode works).
                                3 => {
                                    shared.player.lock().unwrap().gamemode = value as u8;
                                    tracing::info!(gamemode = value as u8, "game mode changed");
                                }
                                // Rain starts/stops and its smoothly interpolated level.
                                1 => shared.environment.lock().unwrap().rain_level = 0.0,
                                2 => shared.environment.lock().unwrap().rain_level = 1.0,
                                7 => {
                                    shared.environment.lock().unwrap().rain_level =
                                        value.clamp(0.0, 1.0);
                                }
                                8 => {
                                    shared.environment.lock().unwrap().thunder_level =
                                        value.clamp(0.0, 1.0);
                                }
                                _ => {}
                            }
                        }
                    }
                    id if id == ID_UPDATE_TIME => {
                        let mut b = raw.body.clone();
                        if let (Ok(world_age), Ok(time_of_day)) = (b.read_i64(), b.read_i64()) {
                            let mut environment = shared.environment.lock().unwrap();
                            environment.world_age = world_age;
                            environment.time_of_day = time_of_day;
                        }
                    }
                    id if id == ID_SOUND || id == ID_ENTITY_SOUND => {
                        let mut b = raw.body.clone();
                        if let Ok(sound_id) = b.read_varint() {
                            let (event, fixed_range) = if sound_id == 0 {
                                let resource = b.read_string(32767).ok();
                                let range = if b.read_bool().unwrap_or(false) {
                                    b.read_f32().ok()
                                } else {
                                    None
                                };
                                (
                                    resource.map(|name| {
                                        name.strip_prefix("minecraft:")
                                            .unwrap_or(&name)
                                            .to_string()
                                    }),
                                    range,
                                )
                            } else {
                                (Some(format!("@protocol:{sound_id}")), None)
                            };
                            if let Some(event) = event {
                                let _category = b.read_varint();
                                if id == ID_SOUND {
                                    if let (Ok(x), Ok(y), Ok(z), Ok(volume)) =
                                        (b.read_i32(), b.read_i32(), b.read_i32(), b.read_f32())
                                    {
                                        queue_sound_at(
                                            shared,
                                            event,
                                            [
                                                f64::from(x) / 8.0,
                                                f64::from(y) / 8.0,
                                                f64::from(z) / 8.0,
                                            ],
                                            volume,
                                            fixed_range,
                                        );
                                    }
                                } else if let (Ok(entity_id), Ok(volume)) =
                                    (b.read_varint(), b.read_f32())
                                {
                                    let position = shared
                                        .entities
                                        .lock()
                                        .unwrap()
                                        .get(&entity_id)
                                        .map(|entity| [entity.x, entity.y, entity.z]);
                                    if let Some(position) = position {
                                        queue_sound_at(
                                            shared,
                                            event,
                                            position,
                                            volume,
                                            fixed_range,
                                        );
                                    }
                                }
                            }
                        }
                    }
                    id if id == ID_ENTITY_EFFECT => {
                        let mut b = raw.body.clone();
                        if let (
                            Ok(entity_id),
                            Ok(effect_id),
                            Ok(amplifier),
                            Ok(duration),
                            Ok(flags),
                        ) = (
                            b.read_varint(),
                            b.read_varint(),
                            b.read_i8(),
                            b.read_varint(),
                            b.read_i8(),
                        ) {
                            if entity_id == shared.player.lock().unwrap().entity_id {
                                shared.effects.lock().unwrap().insert(
                                    effect_id,
                                    ActiveEffect {
                                        amplifier: amplifier as u8,
                                        duration,
                                        ambient: flags & 0x01 != 0,
                                        show_particles: flags & 0x02 != 0,
                                        show_icon: flags & 0x04 != 0,
                                    },
                                );
                            }
                        }
                    }
                    id if id == ID_DECLARE_RECIPES => {
                        if let Ok(recipes) = parse_declared_recipes(&raw, protocol) {
                            *shared.crafting_recipes.lock().unwrap() = recipes.crafting;
                            *shared.stonecutter_recipes.lock().unwrap() = recipes.stonecutting;
                        }
                    }
                    id if id == ID_UNLOCK_RECIPES => {
                        let _ = handle_unlock_recipes(&raw, shared);
                    }
                    id if id == ID_RESOURCE_PACK => {
                        let result = if protocol.uses_nbt_components() {
                            handle_resource_pack_request_765(&raw, shared)
                        } else {
                            handle_resource_pack_request(&raw, shared)
                        };
                        let _ = result;
                    }
                    id if id == ID_REMOVE_ENTITY_EFFECT => {
                        let mut b = raw.body.clone();
                        if let (Ok(entity_id), Ok(effect_id)) =
                            (b.read_varint(), b.read_varint())
                        {
                            if entity_id == shared.player.lock().unwrap().entity_id {
                                shared.effects.lock().unwrap().remove(&effect_id);
                            }
                        }
                    }
                    id if id == ClientboundPlayerAbilities::ID => {
                        if let Ok(pkt) = raw.decode::<ClientboundPlayerAbilities>() {
                            let mut player = shared.player.lock().unwrap();
                            player.allow_flying = pkt.flags & 0x04 != 0;
                            player.flying = pkt.flags & 0x02 != 0;
                            player.flying_speed = pkt.flying_speed;
                            player.walking_speed = pkt.walking_speed;
                        }
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
                            shared.apply_core_event(ClientEvent::HealthUpdated {
                                health: pkt.health,
                                food: pkt.food,
                            });
                            // Took damage (and not the initial 20->20 sync): hurt sound.
                            if pkt.health < prev && pkt.health > 0.0 {
                                queue_sound(shared, crab_audio::hurt_event().to_string());
                            }
                            if pkt.health <= 0.0 && !dead {
                                dead = true;
                                tracing::info!("died — respawning");
                                conn.send(&ClientStatusCommand { action: 0 }).await?;
                            } else if pkt.health > 0.0 {
                                dead = false;
                            }
                        }
                    }
                    id if id == ID_RESPAWN => {
                        // Entities don't carry across a respawn/dimension change.
                        shared.entities.lock().unwrap().clear();
                        shared.player.lock().unwrap().vehicle = None;
                    }
                    id if id == OpenScreen::ID => {
                        let decoded: Result<OpenScreen> = if protocol.uses_nbt_components() {
                            decode_open_screen_765(&raw)
                        } else {
                            raw.decode::<OpenScreen>().map_err(Into::into)
                        };
                        if let Ok(pkt) = decoded {
                            handle_open_screen(shared, pkt);
                        }
                    }
                    id if id == ClientboundCloseContainer::ID => {
                        if let Ok(pkt) = raw.decode::<ClientboundCloseContainer>() {
                            handle_close_container(shared, pkt.window_id);
                        }
                    }
                    id if id == ID_CONTAINER_CONTENT => {
                        if protocol.uses_data_components() {
                            if let Ok((pkt, metadata)) =
                                decode_component_container_content(&raw, protocol)
                            {
                                let window_id = pkt.window_id;
                                handle_container_content(shared, pkt);
                                capture_component_content_metadata(shared, window_id, metadata);
                            }
                        } else {
                            let _ = capture_container_content_nbt(&raw, shared);
                            if let Ok(pkt) = raw.decode::<SetContainerContent>() {
                                handle_container_content(shared, pkt);
                            }
                        }
                    }
                    id if id == SetContainerData::ID => {
                        if let Ok(pkt) = raw.decode::<SetContainerData>() {
                            handle_container_data(shared, pkt);
                        }
                    }
                    id if id == ID_CONTAINER_SLOT => {
                        if protocol.uses_data_components() {
                            if let Ok((pkt, metadata)) =
                                decode_component_container_slot(&raw, protocol)
                            {
                                capture_component_slot_metadata(shared, &pkt, metadata);
                                handle_container_slot(shared, &pkt);
                            }
                        } else {
                            let _ = capture_container_slot_nbt(&raw, shared);
                            if let Ok(pkt) = raw.decode::<SetContainerSlot>() {
                                handle_container_slot(shared, &pkt);
                            }
                        }
                    }
                    id if id == ID_SET_HELD_ITEM => {
                        // 769 widened the clientbound hotbar index to a VarInt.
                        let mut body = raw.body.clone();
                        let slot = if matches!(
                            protocol,
                            ProtocolVersion::V1_21_4 | ProtocolVersion::V1_21_5
                        ) {
                            body.read_varint().ok().and_then(|slot| u8::try_from(slot).ok())
                        } else {
                            body.read_u8().ok()
                        };
                        if let Some(slot @ 0..=8) = slot {
                            shared.player.lock().unwrap().selected_slot = slot;
                        }
                    }
                    id if id == PlayDisconnect::ID => {
                        let reason = if protocol.uses_nbt_components() {
                            let mut body = raw.body.clone();
                            read_text_component(&mut body, protocol)?
                        } else {
                            let d: PlayDisconnect = raw.decode()?;
                            plain_text(&d.reason_json)
                        };
                        tracing::warn!("disconnected: {reason}");
                        break;
                    }
                    _ => {}
                }
            }
            _ = pos_tick.tick() => {
                shared.apply_core_event(ClientEvent::Tick);
                // Left-click ("attack") is held: hold-to-dig / continuous
                // attack. Right-click is held, with press/release edges below.
                let controls = {
                    let mut controls = shared.controls.lock().unwrap();
                    let snapshot = *controls;
                    controls.toggle_flight = false;
                    controls.swap_hands = false;
                    snapshot
                };
                let attack_held = controls.attack;
                let use_held = controls.use_item;
                let attack_edge = attack_held && !was_attacking;
                was_attacking = attack_held;
                let use_edge = use_held && !was_using;
                let release_use_edge = !use_held && was_using;
                was_using = use_held;
                let jump_edge = controls.jump && !was_jumping;
                was_jumping = controls.jump;
                let mut snapshot = { *shared.player.lock().unwrap() };
                let effects = {
                    let mut active = shared.effects.lock().unwrap();
                    for effect in active.values_mut() {
                        if effect.duration > 0 {
                            effect.duration -= 1;
                        }
                    }
                    active.retain(|_, effect| effect.duration != 0);
                    active.clone()
                };
                {
                    let mut action = shared.action_bar.lock().unwrap();
                    if let Some((_, ticks)) = action.as_mut() {
                        *ticks = ticks.saturating_sub(1);
                        if *ticks == 0 {
                            *action = None;
                        }
                    }
                    let mut title = shared.title.lock().unwrap();
                    title.remaining = title.remaining.saturating_sub(1);
                    let mut border = shared.world_border.lock().unwrap();
                    if border.lerp_remaining_ms > 0 {
                        let step = 50.min(border.lerp_remaining_ms);
                        let fraction = f64::from(step) / f64::from(border.lerp_remaining_ms);
                        border.diameter +=
                            (border.target_diameter - border.diameter) * fraction;
                        border.lerp_remaining_ms -= step;
                    }
                    let mut particles = shared.particles.lock().unwrap();
                    for particle in particles.iter_mut() {
                        particle.age += 1;
                        for axis in 0..3 {
                            particle.position[axis] += particle.velocity[axis] / 20.0;
                        }
                        particle.velocity[1] -= 0.04;
                        for velocity in &mut particle.velocity {
                            *velocity *= 0.96;
                        }
                    }
                    particles.retain(|particle| particle.age < particle.lifetime);
                }

                if snapshot.spawned {
                    if controls.swap_hands {
                        block_sequence += 1;
                        conn.send(&dig_packet(6, [0, 0, 0], 0, block_sequence))
                            .await?;
                        let mut inventory = shared.inventory.lock().unwrap();
                        swap_selected_with_offhand(&mut inventory, controls.selected_slot);
                    }
                    let in_water = {
                        let world = shared.world.lock().unwrap();
                        fluid_kind_at(&world, [snapshot.x, snapshot.y, snapshot.z], 1.8)
                            == Some(FluidKind::Water)
                    };
                    snapshot.swimming = in_water
                        && snapshot.sprinting
                        && controls.forward > 0.0
                        && !snapshot.sneaking;
                    let wearing_elytra = shared.inventory.lock().unwrap().get(6).is_some_and(|slot| {
                        slot.is_some_and(|item| item_name_for_slot(item.item_id) == Some("elytra"))
                    });
                    if jump_edge
                        && snapshot.vehicle.is_none()
                        && !snapshot.on_ground
                        && !in_water
                        && !snapshot.flying
                        && wearing_elytra
                    {
                        conn.send(&PlayerCommand {
                            entity_id: snapshot.entity_id,
                            action: 8,
                            jump_boost: 0,
                        })
                        .await?;
                        snapshot.gliding = true;
                    }
                    if snapshot.on_ground || in_water || snapshot.flying {
                        snapshot.gliding = false;
                    }
                    if controls.toggle_flight && snapshot.allow_flying {
                        snapshot.flying = !snapshot.flying;
                        conn.send(&ServerboundPlayerAbilities {
                            flags: if snapshot.flying { 0x02 } else { 0 },
                        })
                        .await?;
                    }
                    let can_stand = {
                        let world = shared.world.lock().unwrap();
                        crab_physics::can_occupy_player_in(
                            shared.context.registries,
                            &world,
                            [snapshot.x, snapshot.y, snapshot.z],
                            crab_physics::PLAYER_HEIGHT,
                        )
                    };
                    let sneaking = snapshot.vehicle.is_none()
                        && (controls.sneak || (snapshot.sneaking && !can_stand));
                    if sneaking != snapshot.sneaking {
                        conn.send(&PlayerCommand {
                            entity_id: snapshot.entity_id,
                            action: if sneaking { 0 } else { 1 },
                            jump_boost: 0,
                        })
                        .await?;
                        snapshot.sneaking = sneaking;
                    }
                    let sprinting = controls.sprint
                        && controls.forward > 0.0
                        && !sneaking
                        && (snapshot.food > 6
                            || snapshot.gamemode == 1
                            || snapshot.gamemode == 3);
                    if sprinting != snapshot.sprinting {
                        conn.send(&PlayerCommand {
                            entity_id: snapshot.entity_id,
                            action: if sprinting { 3 } else { 4 },
                            jump_boost: 0,
                        })
                        .await?;
                        snapshot.sprinting = sprinting;
                    }
                    let mut player = shared.player.lock().unwrap();
                    player.sneaking = snapshot.sneaking;
                    player.sprinting = snapshot.sprinting;
                    player.flying = snapshot.flying;
                    player.swimming = snapshot.swimming;
                    player.gliding = snapshot.gliding;
                }

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
                    shared.apply_core_event(ClientEvent::SelectedSlotChanged(
                        controls.selected_slot,
                    ));
                }

                for command in shared.commands.drain()? {
                    handle_client_command(conn, shared, protocol, command).await?;
                }

                if snapshot.spawned {
                    if let Some(vehicle_id) = snapshot.vehicle {
                        let vehicle_state = {
                            let mut entities = shared.entities.lock().unwrap();
                            entities.get_mut(&vehicle_id).map(|vehicle| {
                                let is_boat = u32::try_from(vehicle.type_id)
                                    .ok()
                                    .and_then(crab_registry::entity_name)
                                    .is_some_and(|name| matches!(name, "boat" | "chest_boat"));
                                if is_boat {
                                    let (position, yaw) = controlled_boat_step(
                                        [vehicle.x, vehicle.y, vehicle.z],
                                        vehicle.yaw,
                                        controls.forward,
                                        controls.strafe,
                                        tick_dt,
                                    );
                                    (vehicle.x, vehicle.y, vehicle.z) =
                                        (position[0], position[1], position[2]);
                                    vehicle.yaw = yaw;
                                }
                                (
                                    vehicle.x,
                                    vehicle.y,
                                    vehicle.z,
                                    vehicle.yaw,
                                    vehicle.height,
                                    is_boat,
                                )
                            })
                        };
                        if let Some((x, y, z, yaw, height, is_boat)) = vehicle_state {
                            let flags = u8::from(controls.jump) | (u8::from(controls.sneak) << 1);
                            if matches!(
                                protocol,
                                ProtocolVersion::V1_21_2
                                    | ProtocolVersion::V1_21_4
                                    | ProtocolVersion::V1_21_5
                            ) {
                                let mut input = 0u8;
                                input |= u8::from(controls.forward > 0.0);
                                input |= u8::from(controls.forward < 0.0) << 1;
                                input |= u8::from(controls.strafe > 0.0) << 2;
                                input |= u8::from(controls.strafe < 0.0) << 3;
                                input |= u8::from(controls.jump) << 4;
                                input |= u8::from(controls.sneak) << 5;
                                input |= u8::from(controls.sprint) << 6;
                                if matches!(
                                    protocol,
                                    ProtocolVersion::V1_21_4 | ProtocolVersion::V1_21_5
                                ) {
                                    conn.send_unmapped(&play769::PlayerInput { flags: input })
                                        .await?;
                                } else {
                                    conn.send_unmapped(&play768::PlayerInput { flags: input })
                                        .await?;
                                }
                            } else {
                                conn.send(&SteerVehicle {
                                    sideways: controls.strafe,
                                    forward: controls.forward,
                                    flags,
                                })
                                .await?;
                            }
                            if is_boat {
                                conn.send(&SteerBoat {
                                    left_paddle: controls.forward != 0.0 || controls.strafe > 0.0,
                                    right_paddle: controls.forward != 0.0 || controls.strafe < 0.0,
                                })
                                .await?;
                            }
                            if matches!(
                                protocol,
                                ProtocolVersion::V1_21_4 | ProtocolVersion::V1_21_5
                            ) {
                                conn.send_unmapped(&play769::VehicleMove {
                                    x,
                                    y,
                                    z,
                                    yaw,
                                    pitch: 0.0,
                                    on_ground: false,
                                })
                                .await?;
                            } else {
                                conn.send(&VehicleMove {
                                    x,
                                    y,
                                    z,
                                    yaw,
                                    pitch: 0.0,
                                })
                                .await?;
                            }
                            let mut player = shared.player.lock().unwrap();
                            (player.x, player.y, player.z) =
                                (x, y + f64::from(height) * 0.75, z);
                            player.vel = [0.0; 3];
                            player.on_ground = false;
                        } else {
                            shared.player.lock().unwrap().vehicle = None;
                            snapshot.vehicle = None;
                        }
                    }
                }

                if snapshot.spawned {
                    let yaw = f64::from(controls.yaw).to_radians();
                    let pitch = f64::from(controls.pitch).to_radians();
                    let eye_height = if snapshot.sneaking { 1.27 } else { 1.62 };
                    let eye = [snapshot.x, snapshot.y + eye_height, snapshot.z];
                    let dir = [
                        -yaw.sin() * pitch.cos(),
                        -pitch.sin(),
                        yaw.cos() * pitch.cos(),
                    ];
                    let hit = crab_physics::raycast_in(
                        shared.context.registries,
                        &shared.world.lock().unwrap(),
                        eye,
                        dir,
                        5.0,
                    );

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
                                    sneaking: snapshot.sneaking,
                                })
                                .await?;
                                attacked_entity = true;
                                queue_sound(shared, crab_audio::attack_event().to_string());
                                dig = cancel_dig(conn, dig, &mut block_sequence).await?;
                            }
                        }
                        // A fresh click swings the arm (the mining hit + break
                        // sounds play as digging progresses, not on every click).
                        if !attacked_entity {
                            conn.send(&SwingArm { hand: 0 }).await?;
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
                                        play_block_sound(shared, b, crab_audio::break_event);
                                        break_block_local(shared, b);
                                    } else if left % 4 == 0 {
                                        // Quieter mining "hit" sound while digging
                                        // (vanilla `block.*.hit`, not the break sound).
                                        play_block_sound(shared, b, crab_audio::hit_event);
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
                                    let ticks = adjusted_break_ticks(ticks, &effects);
                                    block_sequence += 1;
                                    conn.send(&dig_packet(0, target, face, block_sequence)).await?;
                                    if snapshot.gamemode == 1 || ticks == 0 {
                                        // Creative / instant: server breaks now.
                                        if snapshot.gamemode != 1 {
                                            block_sequence += 1;
                                            conn.send(&dig_packet(2, target, face, block_sequence))
                                                .await?;
                                        }
                                        play_block_sound(shared, target, crab_audio::break_event);
                                        break_block_local(shared, target);
                                    } else {
                                        dig = Some(DigProgress {
                                            block: target,
                                            face,
                                            ticks_left: ticks,
                                            ticks_total: ticks,
                                        });
                                    }
                                }
                            }
                        } else {
                            // Released, or not aiming at a block: cancel any dig.
                            dig = cancel_dig(conn, dig, &mut block_sequence).await?;
                        }
                    }

                    // Publish the destroy-stage overlay (crack 0..=9) for the
                    // renderer; `None` whenever we're not mid-dig.
                    *shared.dig.lock().unwrap() = dig.map(|d| {
                        let done = d.ticks_total.saturating_sub(d.ticks_left);
                        let stage = u8::try_from(
                            (done * 10)
                                .checked_div(d.ticks_total)
                                .unwrap_or(0)
                                .min(9),
                        )
                        .unwrap_or(9);
                        DigOverlay {
                            block: d.block,
                            stage,
                        }
                    });

                    // Right-click always reaches the server: target a block when
                    // aimed at one, otherwise use the held item in air.
                    if use_edge {
                        let held = {
                            let inv = shared.inventory.lock().unwrap();
                            inv.get(36 + controls.selected_slot as usize)
                                .copied()
                                .flatten()
                        };
                        if let Some(hit) = hit {
                            block_sequence += 1;
                            if protocol == ProtocolVersion::V1_21_5 {
                                conn.send_unmapped(&play770::UseItemOn {
                                    hand: 0,
                                    x: hit.block[0],
                                    y: hit.block[1],
                                    z: hit.block[2],
                                    direction: face_direction(hit.face),
                                    cursor: [0.5, 0.5, 0.5],
                                    inside_block: false,
                                    world_border_hit: false,
                                    sequence: block_sequence,
                                })
                                .await?;
                            } else if protocol == ProtocolVersion::V1_21_4 {
                                conn.send_unmapped(&play769::UseItemOn {
                                    hand: 0,
                                    x: hit.block[0],
                                    y: hit.block[1],
                                    z: hit.block[2],
                                    direction: face_direction(hit.face),
                                    cursor: [0.5, 0.5, 0.5],
                                    inside_block: false,
                                    world_border_hit: false,
                                    sequence: block_sequence,
                                })
                                .await?;
                            } else if protocol == ProtocolVersion::V1_21_2 {
                                conn.send_unmapped(&play768::UseItemOn {
                                    hand: 0,
                                    x: hit.block[0],
                                    y: hit.block[1],
                                    z: hit.block[2],
                                    direction: face_direction(hit.face),
                                    cursor: [0.5, 0.5, 0.5],
                                    inside_block: false,
                                    world_border_hit: false,
                                    sequence: block_sequence,
                                })
                                .await?;
                            } else {
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
                            }
                            let target_name = block_name_at(shared, hit.block);
                            let may_place = snapshot.sneaking
                                || target_name.is_none_or(|name| !is_interactable_block(name));
                            if let (true, Some(state)) = (may_place, placeable_state(held)) {
                                let p = hit.place_position();
                                shared
                                    .world
                                    .lock()
                                    .unwrap()
                                    .set_block_state(p[0], p[1], p[2], state);
                                mark_dirty(shared, p[0] >> 4, p[2] >> 4);
                                if let Some(name) = crab_registry::block_name(state) {
                                    queue_sound(shared, crab_audio::place_event(name));
                                }
                            }
                        } else {
                            block_sequence += 1;
                            if protocol == ProtocolVersion::V1_21_5 {
                                conn.send_unmapped(&play770::UseItem {
                                    hand: 0,
                                    sequence: block_sequence,
                                    yaw: controls.yaw,
                                    pitch: controls.pitch,
                                })
                                .await?;
                            } else if protocol == ProtocolVersion::V1_21_4 {
                                conn.send_unmapped(&play769::UseItem {
                                    hand: 0,
                                    sequence: block_sequence,
                                    yaw: controls.yaw,
                                    pitch: controls.pitch,
                                })
                                .await?;
                            } else if protocol == ProtocolVersion::V1_21_2 {
                                conn.send_unmapped(&play768::UseItem {
                                    hand: 0,
                                    sequence: block_sequence,
                                    yaw: controls.yaw,
                                    pitch: controls.pitch,
                                })
                                .await?;
                            } else {
                                conn.send(&UseItem {
                                    hand: 0,
                                    sequence: block_sequence,
                                })
                                .await?;
                            }
                        }
                    }
                    if release_use_edge {
                        block_sequence += 1;
                        conn.send(&dig_packet(5, [0, 0, 0], 0, block_sequence))
                            .await?;
                    }
                }

                if snapshot.spawned && snapshot.vehicle.is_none() {
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
                            let pose_height = if snapshot.swimming || snapshot.gliding {
                                0.6
                            } else if snapshot.sneaking {
                                1.5
                            } else {
                                1.8
                            };
                            let fluid = (!snapshot.flying)
                                .then(|| {
                                    fluid_kind_at(
                                        &world,
                                        [snapshot.x, snapshot.y, snapshot.z],
                                        pose_height,
                                    )
                                })
                                .flatten();
                            let speed = if snapshot.flying {
                                flight_speed(snapshot.flying_speed, snapshot.sprinting)
                            } else if snapshot.swimming {
                                5.0 * effect_movement_multiplier(&effects)
                            } else if snapshot.gliding {
                                10.0
                            } else {
                                movement_speed(snapshot.sneaking, snapshot.sprinting, fluid)
                                    * (f64::from(snapshot.walking_speed.max(0.0)) / 0.1)
                                    * effect_movement_multiplier(&effects)
                            };
                            if len > 1e-6 {
                                vx = vx / len * speed;
                                vz = vz / len * speed;
                            } else {
                                vx = 0.0;
                                vz = 0.0;
                            }
                            let mut vel = snapshot.vel;
                            vel[0] = vx;
                            vel[2] = vz;
                            let (gravity, terminal) = if snapshot.flying {
                                let vertical = (i32::from(controls.jump)
                                    - i32::from(controls.sneak))
                                    as f64;
                                vel[1] = vertical * speed;
                                (0.0, -1000.0)
                            } else {
                                match fluid {
                                Some(FluidKind::Water) => {
                                    vel[1] *= 0.8;
                                    if controls.jump {
                                        vel[1] = 3.0;
                                    } else if controls.sneak {
                                        vel[1] = -3.0;
                                    }
                                    (4.0, -3.0)
                                }
                                Some(FluidKind::Lava) => {
                                    vel[1] *= 0.5;
                                    if controls.jump {
                                        vel[1] = 2.0;
                                    } else if controls.sneak {
                                        vel[1] = -2.0;
                                    }
                                    (2.0, -2.0)
                                }
                                None => {
                                    if snapshot.gliding {
                                        let pitch = f64::from(controls.pitch).to_radians();
                                        vel[1] = (vel[1] - 0.15 + (-pitch.sin()).max(0.0) * 0.12)
                                            .max(-12.0);
                                        (2.5, -12.0)
                                    } else {
                                    if controls.jump && snapshot.on_ground {
                                        vel[1] =
                                            8.5 + 2.0 * f64::from(effect_level(&effects, 8));
                                    }
                                    let levitation = effect_level(&effects, 25);
                                    if levitation > 0 {
                                        let target = f64::from(levitation);
                                        vel[1] += (target - vel[1]) * 0.2;
                                        (0.0, -1000.0)
                                    } else if effect_level(&effects, 28) > 0 && vel[1] < 0.0 {
                                        (4.0, -2.5)
                                    } else {
                                        (crab_physics::GRAVITY, crab_physics::TERMINAL_VELOCITY)
                                    }
                                    }
                                }
                                }
                            };
                            if snapshot.flying && snapshot.gamemode == 3 {
                                crab_physics::StepResult {
                                    position: [
                                        snapshot.x + vel[0] * tick_dt,
                                        snapshot.y + vel[1] * tick_dt,
                                        snapshot.z + vel[2] * tick_dt,
                                    ],
                                    velocity: vel,
                                    on_ground: false,
                                }
                            } else {
                                crab_physics::step_player_with_forces_in(
                                    shared.context.registries,
                                    &world,
                                    [snapshot.x, snapshot.y, snapshot.z],
                                    vel,
                                    tick_dt,
                                    pose_height,
                                    gravity,
                                    terminal,
                                )
                            }
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
                                    queue_sound(shared, crab_audio::step_event(name));
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

/// Encodes one version-neutral presentation command at the protocol boundary.
/// Keeping this dispatch outside the simulation loop makes new commands and
/// protocol-specific representations independently testable.
async fn handle_client_command<S>(
    conn: &mut Connection<S>,
    shared: &Arc<Shared>,
    protocol: ProtocolVersion,
    command: ClientCommand,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    match command {
        ClientCommand::SetControls(controls) => {
            *shared.controls.lock().unwrap() = Controls {
                forward: controls.forward,
                strafe: controls.strafe,
                jump: controls.jump,
                sprint: controls.sprint,
                sneak: controls.sneak,
                attack: controls.attack,
                use_item: controls.use_item,
                yaw: controls.yaw,
                pitch: controls.pitch,
                selected_slot: controls.selected_slot,
                toggle_flight: controls.toggle_flight,
                swap_hands: controls.swap_hands,
            };
        }
        ClientCommand::SendChat(message) => {
            if let Some(command) = message.strip_prefix('/') {
                conn.send(&ChatCommand::new(command.to_owned())).await?;
            } else if protocol == ProtocolVersion::V1_21_5 {
                conn.send_unmapped(&play770::ClientChatMessage::unsigned(message.clone()))
                    .await?;
            } else {
                conn.send(&ClientChatMessage::unsigned(message.clone()))
                    .await?;
            }
            push_chat(shared, message);
        }
        ClientCommand::ResourcePackDecision(accepted) => {
            if !accepted {
                send_play_pack_status(conn, shared, protocol, 1).await?;
                return Ok(());
            }
            send_play_pack_status(conn, shared, protocol, 3).await?;
            let request = shared.resource_pack_request.lock().unwrap().clone();
            let base_jar = shared.base_resource_jar.lock().unwrap().clone();
            match request {
                Some(request) => match download_resource_pack(&request).await {
                    Ok(archive) => match activate_resource_pack(
                        shared,
                        request.uuid,
                        archive,
                        base_jar.as_deref(),
                    ) {
                        Ok(path) => {
                            *shared.cached_resource_pack.lock().unwrap() = Some(path);
                            if !shared.renderer_active.load(Ordering::SeqCst) {
                                send_play_pack_status(conn, shared, protocol, 0).await?;
                            }
                        }
                        Err(error) => {
                            tracing::warn!(%error, "resource-pack activation failed");
                            send_play_pack_status(conn, shared, protocol, 2).await?;
                        }
                    },
                    Err(error) => {
                        tracing::warn!(%error, "resource-pack download failed");
                        send_play_pack_status(conn, shared, protocol, 2).await?;
                    }
                },
                None => send_play_pack_status(conn, shared, protocol, 2).await?,
            }
        }
        ClientCommand::ResourcePackStatus(status) => {
            send_play_pack_status(conn, shared, protocol, status).await?;
        }
        ClientCommand::CloseContainer(window_id) => {
            conn.send(&CloseContainer { window_id }).await?;
        }
        ClientCommand::ChooseEnchantment {
            window_id,
            enchantment,
        } => {
            conn.send(&EnchantItem {
                window_id: window_id as i8,
                enchantment,
            })
            .await?;
        }
        ClientCommand::PressMenuButton {
            window_id,
            button_id,
        } => {
            conn.send(&ContainerButtonClick {
                window_id: window_id as i8,
                button_id,
            })
            .await?;
        }
        ClientCommand::RenameItem(name) => conn.send(&RenameItem { name }).await?,
        ClientCommand::EditBook { slot, pages, title } => {
            conn.send(&EditBook { slot, pages, title }).await?;
        }
        ClientCommand::PlaceRecipe {
            window_id,
            recipe,
            make_all,
        } => match (protocol, recipe) {
            (
                ProtocolVersion::V1_21_4 | ProtocolVersion::V1_21_5,
                RecipeKey::Numeric(recipe_id),
            ) => {
                conn.send_unmapped(&play769::PlaceRecipe {
                    window_id: i32::from(window_id),
                    recipe_id,
                    make_all,
                })
                .await?;
            }
            (ProtocolVersion::V1_21_2, RecipeKey::Numeric(recipe_id)) => {
                conn.send_unmapped(&play768::PlaceRecipe {
                    window_id: i32::from(window_id),
                    recipe_id,
                    make_all,
                })
                .await?;
            }
            (_, RecipeKey::Namespaced(recipe)) => {
                conn.send(&PlaceRecipe {
                    window_id,
                    recipe,
                    make_all,
                })
                .await?;
            }
            _ => tracing::warn!("recipe key does not match active protocol profile"),
        },
        ClientCommand::SelectBundleItem {
            slot_id,
            selected_item_index,
        } => {
            if matches!(
                protocol,
                ProtocolVersion::V1_21_4 | ProtocolVersion::V1_21_5
            ) {
                conn.send_unmapped(&play769::SelectBundleItem {
                    slot_id,
                    selected_item_index,
                })
                .await?;
            } else if protocol == ProtocolVersion::V1_21_2 {
                conn.send_unmapped(&play768::SelectBundleItem {
                    slot_id,
                    selected_item_index,
                })
                .await?;
            }
        }
        ClientCommand::ClickContainer {
            window_id,
            slot,
            button,
            mode,
        } => {
            let index = slot as usize;
            let predicted = if window_id == 0 {
                let state_id = *shared.window_state.lock().unwrap();
                let mut inventory = shared.inventory.lock().unwrap();
                let mut carried = shared.carried.lock().unwrap();
                if index >= inventory.len() {
                    None
                } else {
                    let changed =
                        apply_inventory_click(&mut inventory, &mut carried, index, button, mode);
                    Some((state_id, changed, *carried))
                }
            } else {
                let mut open = shared.container.lock().unwrap();
                let mut carried = shared.carried.lock().unwrap();
                match open
                    .as_mut()
                    .filter(|container| container.window_id == window_id)
                {
                    Some(container) if index < container.slots.len() => {
                        let container_slots = container.container_slot_count();
                        let changed = apply_container_click(
                            &mut container.slots,
                            &mut carried,
                            index,
                            button,
                            mode,
                            container_slots,
                            container.menu_type,
                        );
                        Some((container.state_id, changed, *carried))
                    }
                    _ => None,
                }
            };
            if let Some((state_id, changed, carried)) = predicted {
                if matches!(
                    protocol,
                    ProtocolVersion::V1_21
                        | ProtocolVersion::V1_21_2
                        | ProtocolVersion::V1_21_4
                        | ProtocolVersion::V1_21_5
                ) {
                    conn.send(&play767::ClickContainerComponents {
                        window_id,
                        state_id,
                        slot,
                        button,
                        mode,
                        changed,
                        carried,
                    })
                    .await?;
                } else if protocol == ProtocolVersion::V1_20_5 {
                    conn.send(&play766::ClickContainerComponents {
                        window_id,
                        state_id,
                        slot,
                        button,
                        mode,
                        changed,
                        carried,
                    })
                    .await?;
                } else {
                    conn.send(&ClickContainer {
                        window_id,
                        state_id,
                        slot,
                        button,
                        mode,
                        changed,
                        carried,
                    })
                    .await?;
                }
            }
        }
    }
    Ok(())
}

fn handle_join_game(
    raw: &crab_net::RawPacket,
    shared: &Arc<Shared>,
    protocol: ProtocolVersion,
) -> Result<()> {
    let mut b = raw.body.clone();
    let entity_id = b.read_i32()?;
    let _hardcore = b.read_bool()?;
    let (codec, dimension_type, game_mode) = match protocol {
        ProtocolVersion::V1_20_1 => {
            let game_mode = b.read_u8()?;
            let _prev_game_mode = b.read_i8()?;
            let world_count = b.read_varint()?.max(0);
            for _ in 0..world_count {
                let _ = b.read_string(32767)?;
            }
            (
                crab_protocol::nbt::read_nbt(&mut b)?,
                b.read_string(32767)?,
                game_mode,
            )
        }
        ProtocolVersion::V1_20_2 | ProtocolVersion::V1_20_3 => {
            let world_count = b.read_varint()?.max(0);
            for _ in 0..world_count {
                let _ = b.read_string(32767)?;
            }
            let _max_players = b.read_varint()?;
            let _view_distance = b.read_varint()?;
            let _simulation_distance = b.read_varint()?;
            let _reduced_debug = b.read_bool()?;
            let _respawn_screen = b.read_bool()?;
            let _limited_crafting = b.read_bool()?;
            let dimension_type = b.read_string(32767)?;
            let _world_name = b.read_string(32767)?;
            let _hashed_seed = b.read_i64()?;
            let game_mode = b.read_u8()?;
            let _prev_game_mode = b.read_i8()?;
            let codec = shared
                .registry_codec
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(crab_protocol::nbt::Nbt::End);
            (codec, dimension_type, game_mode)
        }
        ProtocolVersion::V1_20_5
        | ProtocolVersion::V1_21
        | ProtocolVersion::V1_21_2
        | ProtocolVersion::V1_21_4
        | ProtocolVersion::V1_21_5 => {
            let world_count = b.read_varint()?.max(0);
            for _ in 0..world_count {
                let _ = b.read_string(32767)?;
            }
            let _max_players = b.read_varint()?;
            let _view_distance = b.read_varint()?;
            let _simulation_distance = b.read_varint()?;
            let _reduced_debug = b.read_bool()?;
            let _respawn_screen = b.read_bool()?;
            let _limited_crafting = b.read_bool()?;
            let dimension_id = b.read_varint()?;
            let _world_name = b.read_string(32767)?;
            let _hashed_seed = b.read_i64()?;
            let game_mode = b.read_i8()? as u8;
            let _prev_game_mode = b.read_u8()?;
            let _debug = b.read_bool()?;
            let _flat = b.read_bool()?;
            if b.read_bool()? {
                let _death_dimension = b.read_string(32767)?;
                let _death_position = b.read_position()?;
            }
            let _portal_cooldown = b.read_varint()?;
            let _enforces_secure_chat = b.read_bool()?;
            let codec = shared
                .registry_codec
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(crab_protocol::nbt::Nbt::End);
            let dimension_type =
                registry_name_by_id(&codec, "minecraft:dimension_type", dimension_id)
                    .unwrap_or_else(|| "minecraft:overworld".to_string());
            (codec, dimension_type, game_mode)
        }
    };
    {
        let mut player = shared.player.lock().unwrap();
        player.entity_id = entity_id;
        player.gamemode = game_mode;
    }
    shared.apply_core_event(ClientEvent::PlayerSpawned { entity_id });
    let colors = crab_world::biome_colors(&codec);
    if let Some((min_y, height)) = crab_world::dimension_extent(&codec, &dimension_type) {
        let mut world = World::new(min_y, height);
        world.set_biome_colors(colors);
        *shared.world.lock().unwrap() = world;
        tracing::info!("dimension {dimension_type}: min_y={min_y} height={height}");
    } else {
        shared.world.lock().unwrap().set_biome_colors(colors);
    }
    Ok(())
}

fn merge_registry_data_766(shared: &Arc<Shared>, packet: configuration766::RegistryData) {
    use crab_protocol::nbt::Nbt;
    let values = packet
        .entries
        .into_iter()
        .enumerate()
        .filter_map(|(id, (name, element))| {
            let id = i32::try_from(id).ok()?;
            Some(Nbt::Compound(HashMap::from([
                ("name".to_string(), Nbt::String(name)),
                ("id".to_string(), Nbt::Int(id)),
                ("element".to_string(), element.unwrap_or(Nbt::End)),
            ])))
        })
        .collect();
    let registry = Nbt::Compound(HashMap::from([
        ("type".to_string(), Nbt::String(packet.id.clone())),
        ("value".to_string(), Nbt::List(values)),
    ]));
    let mut codec = shared.registry_codec.lock().unwrap();
    let root = codec.get_or_insert_with(|| Nbt::Compound(HashMap::new()));
    if !matches!(root, Nbt::Compound(_)) {
        *root = Nbt::Compound(HashMap::new());
    }
    if let Nbt::Compound(entries) = root {
        entries.insert(packet.id, registry);
    }
}

fn registry_name_by_id(codec: &crab_protocol::nbt::Nbt, registry: &str, id: i32) -> Option<String> {
    use crab_protocol::nbt::Nbt;
    let Nbt::List(entries) = codec.get(registry)?.get("value")? else {
        return None;
    };
    entries.iter().find_map(|entry| {
        let Nbt::Int(entry_id) = entry.get("id")? else {
            return None;
        };
        if *entry_id != id {
            return None;
        }
        let Nbt::String(name) = entry.get("name")? else {
            return None;
        };
        Some(name.clone())
    })
}

async fn send_play_pack_status<S>(
    conn: &mut Connection<S>,
    shared: &Arc<Shared>,
    protocol: ProtocolVersion,
    status: i32,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if protocol.uses_nbt_components() {
        let uuid = shared
            .resource_pack_request
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|request| request.uuid)
            .context("765 resource-pack status has no pack UUID")?;
        if protocol.uses_split_registry() {
            conn.send_unmapped(&play766::ResourcePackStatus { uuid, status })
                .await?;
        } else {
            conn.send_unmapped(&play765::ResourcePackStatus { uuid, status })
                .await?;
        }
    } else {
        conn.send(&ResourcePackStatus { status }).await?;
    }
    Ok(())
}

fn handle_spawn_object(raw: &crab_net::RawPacket, shared: &Arc<Shared>) -> Result<()> {
    let mut b = raw.body.clone();
    let id = b.read_varint()?;
    let _uuid = b.read_uuid()?;
    let kind = b.read_varint()?;
    let (x, y, z) = (b.read_f64()?, b.read_f64()?, b.read_f64()?);
    // Spawn Entity order: pitch, yaw, head-yaw (each a 256-step angle byte).
    let _pitch = b.read_u8()?;
    let yaw = angle_to_deg(b.read_u8()?);
    let head_yaw = angle_to_deg(b.read_u8()?);
    let data = b.read_varint()?;
    let velocity = [
        f64::from(b.read_i16()?) / 400.0,
        f64::from(b.read_i16()?) / 400.0,
        f64::from(b.read_i16()?) / 400.0,
    ];
    let (half_width, height) = crab_registry::entity_def(kind as u32)
        .map(|d| (d.width / 2.0, d.height))
        .unwrap_or((0.45, 1.3));
    let block_state = spawned_block_state(kind, data);
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
            yaw,
            head_yaw,
            velocity,
            equipment: [None; 6],
            vehicle: None,
            pose: 0,
            invisible: false,
            glowing: false,
            swing_sequence: 0,
            hurt_sequence: 0,
            item: None,
            block_state,
        },
    );
    Ok(())
}

/// Resolves Spawn Entity's overloaded object-data field only for falling
/// blocks. Other entity types use this integer for unrelated meanings (vehicle
/// variants, projectile owners, directions), so treating every value as a
/// block state would produce plausible but incorrect models.
fn spawned_block_state(kind: i32, data: i32) -> Option<u32> {
    if crab_registry::entity_name(u32::try_from(kind).ok()?) != Some("falling_block") {
        return None;
    }
    let state = u32::try_from(data).ok()?;
    crab_registry::block_for_state(state).map(|_| state)
}

/// Converts a Minecraft angle byte (256 steps = 360°) to degrees.
fn angle_to_deg(byte: u8) -> f32 {
    f32::from(byte) * 360.0 / 256.0
}

fn handle_spawn_player(raw: &crab_net::RawPacket, shared: &Arc<Shared>) -> Result<()> {
    let mut b = raw.body.clone();
    let id = b.read_varint()?;
    let _uuid = b.read_uuid()?;
    let (x, y, z) = (b.read_f64()?, b.read_f64()?, b.read_f64()?);
    // Spawn Player order: yaw, pitch.
    let yaw = angle_to_deg(b.read_u8()?);
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
            yaw,
            head_yaw: yaw,
            velocity: [0.0; 3],
            equipment: [None; 6],
            vehicle: None,
            pose: 0,
            invisible: false,
            glowing: false,
            swing_sequence: 0,
            hurt_sequence: 0,
            item: None,
            block_state: None,
        },
    );
    Ok(())
}

fn handle_entity_head_rotation(raw: &crab_net::RawPacket, shared: &Arc<Shared>) -> Result<()> {
    let mut b = raw.body.clone();
    let id = b.read_varint()?;
    let head_yaw = angle_to_deg(b.read_u8()?);
    if let Some(entity) = shared.entities.lock().unwrap().get_mut(&id) {
        entity.head_yaw = head_yaw;
    }
    Ok(())
}

fn handle_entity_velocity(raw: &crab_net::RawPacket, shared: &Arc<Shared>) -> Result<()> {
    let mut b = raw.body.clone();
    let id = b.read_varint()?;
    let velocity = [
        f64::from(b.read_i16()?) / 400.0,
        f64::from(b.read_i16()?) / 400.0,
        f64::from(b.read_i16()?) / 400.0,
    ];
    if let Some(entity) = shared.entities.lock().unwrap().get_mut(&id) {
        entity.velocity = velocity;
    }
    Ok(())
}

fn handle_entity_equipment(
    raw: &crab_net::RawPacket,
    shared: &Arc<Shared>,
    protocol: ProtocolVersion,
) -> Result<()> {
    let mut b = raw.body.clone();
    let id = b.read_varint()?;
    let mut updates = Vec::new();
    loop {
        let encoded_slot = b.read_u8()?;
        let slot = usize::from(encoded_slot & 0x7f);
        let item = read_slot_item(&mut b, protocol)?;
        updates.push((slot, item));
        if encoded_slot & 0x80 == 0 {
            break;
        }
    }
    if let Some(entity) = shared.entities.lock().unwrap().get_mut(&id) {
        for (slot, item) in updates {
            if let Some(destination) = entity.equipment.get_mut(slot) {
                *destination = item;
            }
        }
    }
    Ok(())
}

fn handle_set_passengers(raw: &crab_net::RawPacket, shared: &Arc<Shared>) -> Result<()> {
    let mut b = raw.body.clone();
    let vehicle_id = b.read_varint()?;
    let count = b.read_varint()?.max(0) as usize;
    let mut passengers = Vec::with_capacity(count);
    for _ in 0..count {
        passengers.push(b.read_varint()?);
    }
    let player_id = shared.player.lock().unwrap().entity_id;
    let player_mounted = passengers.contains(&player_id);
    let seat = {
        let mut entities = shared.entities.lock().unwrap();
        for entity in entities.values_mut() {
            if entity.vehicle == Some(vehicle_id) {
                entity.vehicle = None;
            }
        }
        for &passenger in &passengers {
            if passenger != player_id {
                if let Some(entity) = entities.get_mut(&passenger) {
                    entity.vehicle = Some(vehicle_id);
                }
            }
        }
        entities.get(&vehicle_id).map(|vehicle| {
            (
                vehicle.x,
                vehicle.y + f64::from(vehicle.height) * 0.75,
                vehicle.z,
            )
        })
    };
    let mut player = shared.player.lock().unwrap();
    if player_mounted {
        player.vehicle = Some(vehicle_id);
        if let Some((x, y, z)) = seat {
            (player.x, player.y, player.z) = (x, y, z);
        }
    } else if player.vehicle == Some(vehicle_id) {
        player.vehicle = None;
    }
    Ok(())
}

fn handle_vehicle_move(raw: &crab_net::RawPacket, shared: &Arc<Shared>) -> Result<()> {
    let mut b = raw.body.clone();
    let (x, y, z) = (b.read_f64()?, b.read_f64()?, b.read_f64()?);
    let (yaw, pitch) = (b.read_f32()?, b.read_f32()?);
    let vehicle_id = shared.player.lock().unwrap().vehicle;
    let Some(vehicle_id) = vehicle_id else {
        return Ok(());
    };
    let seat_y = {
        let mut entities = shared.entities.lock().unwrap();
        let Some(vehicle) = entities.get_mut(&vehicle_id) else {
            return Ok(());
        };
        (vehicle.x, vehicle.y, vehicle.z) = (x, y, z);
        vehicle.yaw = yaw;
        y + f64::from(vehicle.height) * 0.75
    };
    let mut player = shared.player.lock().unwrap();
    (player.x, player.y, player.z) = (x, seat_y, z);
    player.yaw = yaw;
    player.pitch = pitch;
    Ok(())
}

/// Parses Set Entity Metadata, extracting the fields we render: slime/magma
/// cube `size` (index 16, VarInt) and the dropped-item stack (index 8, Slot).
/// Other fields are skipped by type; parsing stops at an unsupported type.
fn handle_entity_metadata(
    raw: &crab_net::RawPacket,
    shared: &Arc<Shared>,
    protocol: ProtocolVersion,
) -> Result<()> {
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
                let value = b.read_i8()? as u8;
                if index == 0 {
                    if let Some(entity) = shared.entities.lock().unwrap().get_mut(&id) {
                        entity.invisible = value & 0x20 != 0;
                        entity.glowing = value & 0x40 != 0;
                    }
                }
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
            4 => {
                b.read_string(32767)?;
            }
            5 => {
                let _ = read_text_component(&mut b, protocol)?;
            }
            6 => {
                if b.read_bool()? {
                    let _ = read_text_component(&mut b, protocol)?;
                }
            }
            7 => {
                let item = read_slot_item(&mut b, protocol)?;
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
                let value = b.read_varint()?;
                if mtype == pose_metadata_type(protocol) && index == 6 {
                    if let Some(entity) = shared.entities.lock().unwrap().get_mut(&id) {
                        entity.pose = value;
                        match value {
                            3 | 4 => entity.height = 0.6,
                            5 => entity.height = 1.5,
                            _ => {
                                entity.height = crab_registry::entity_def(entity.type_id as u32)
                                    .map_or(entity.height, |definition| definition.height);
                            }
                        }
                    }
                }
            }
            13 => {
                if b.read_bool()? {
                    b.read_uuid()?;
                }
            }
            16 => {
                let _ = if protocol.uses_nbt_components() {
                    crab_protocol::nbt::read_anonymous_nbt(&mut b)?
                } else {
                    crab_protocol::nbt::read_nbt(&mut b)?
                };
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

/// Entity metadata serializer IDs gained the particle-list serializer in
/// 1.20.5, shifting Pose and every following serializer up by one.
fn pose_metadata_type(protocol: ProtocolVersion) -> i32 {
    if protocol.uses_data_components() {
        21
    } else {
        20
    }
}

/// Reads a metadata Slot, returning the contained item id (or None if empty).
fn read_slot_item<B: crab_protocol::BufExt>(
    b: &mut B,
    protocol: ProtocolVersion,
) -> Result<Option<i32>> {
    if protocol == ProtocolVersion::V1_21_5 {
        Ok(play770::read_component_slot(b)?
            .item
            .map(|item| item.item_id))
    } else if matches!(
        protocol,
        ProtocolVersion::V1_21_2 | ProtocolVersion::V1_21_4
    ) {
        Ok(play766::read_component_slot_768(b)?
            .item
            .map(|item| item.item_id))
    } else if protocol == ProtocolVersion::V1_21 {
        Ok(play766::read_component_slot_767(b)?
            .item
            .map(|item| item.item_id))
    } else if protocol == ProtocolVersion::V1_20_5 {
        Ok(play766::read_component_slot(b)?
            .item
            .map(|item| item.item_id))
    } else {
        Ok(read_recipe_slot(b)?.map(|item| item.item_id))
    }
}

fn read_recipe_slot<B: crab_protocol::BufExt>(b: &mut B) -> Result<Option<SlotItem>> {
    if b.read_bool()? {
        let item_id = b.read_varint()?;
        let count = b.read_i8()?;
        let _nbt = crab_protocol::nbt::read_nbt(b)?;
        Ok(Some(SlotItem { item_id, count }))
    } else {
        Ok(None)
    }
}

fn read_ingredient<B: crab_protocol::BufExt>(b: &mut B) -> Result<Vec<i32>> {
    let count = b.read_varint()?;
    if !(0..=4096).contains(&count) {
        bail!("invalid recipe ingredient alternative count {count}");
    }
    let mut items = Vec::with_capacity(count as usize);
    for _ in 0..count {
        if let Some(item) = read_recipe_slot(b)? {
            items.push(item.item_id);
        }
    }
    Ok(items)
}

fn parse_declared_recipes(
    raw: &crab_net::RawPacket,
    protocol: ProtocolVersion,
) -> Result<DeclaredRecipes> {
    if protocol.uses_data_components() {
        parse_declared_recipes_766(raw, protocol)
    } else {
        parse_declared_recipes_legacy(raw)
    }
}

fn parse_declared_recipes_legacy(raw: &crab_net::RawPacket) -> Result<DeclaredRecipes> {
    let mut b = raw.body.clone();
    let count = b.read_varint()?;
    if !(0..=65_536).contains(&count) {
        bail!("invalid declared recipe count {count}");
    }
    let mut stonecutting = Vec::new();
    let mut crafting = Vec::new();
    for _ in 0..count {
        let recipe_type = b.read_string(32767)?;
        let id = b.read_string(32767)?;
        match recipe_type.as_str() {
            "minecraft:crafting_shapeless" => {
                let _group = b.read_string(32767)?;
                let _category = b.read_varint()?;
                let ingredients = b.read_varint()?;
                if !(0..=4096).contains(&ingredients) {
                    bail!("invalid shapeless ingredient count {ingredients}");
                }
                let ingredients = (0..ingredients)
                    .map(|_| read_ingredient(&mut b))
                    .collect::<Result<Vec<_>>>()?;
                let result = read_recipe_slot(&mut b)?;
                crafting.push(CraftingRecipe {
                    id,
                    width: 0,
                    height: 0,
                    ingredients,
                    result,
                });
            }
            "minecraft:crafting_shaped" => {
                let width = b.read_varint()?;
                let height = b.read_varint()?;
                if !(0..=16).contains(&width) || !(0..=16).contains(&height) {
                    bail!("invalid shaped recipe dimensions {width}x{height}");
                }
                let _group = b.read_string(32767)?;
                let _category = b.read_varint()?;
                let ingredients = (0..width * height)
                    .map(|_| read_ingredient(&mut b))
                    .collect::<Result<Vec<_>>>()?;
                let result = read_recipe_slot(&mut b)?;
                let _show_notification = b.read_bool()?;
                crafting.push(CraftingRecipe {
                    id,
                    width: width as u8,
                    height: height as u8,
                    ingredients,
                    result,
                });
            }
            "minecraft:smelting"
            | "minecraft:blasting"
            | "minecraft:smoking"
            | "minecraft:campfire_cooking" => {
                let _group = b.read_string(32767)?;
                let _category = b.read_varint()?;
                let _ = read_ingredient(&mut b)?;
                let _ = read_recipe_slot(&mut b)?;
                let _experience = b.read_f32()?;
                let _cook_time = b.read_varint()?;
            }
            "minecraft:stonecutting" => {
                let _group = b.read_string(32767)?;
                let ingredients = read_ingredient(&mut b)?;
                let result = read_recipe_slot(&mut b)?;
                stonecutting.push(StonecutterRecipe {
                    id,
                    ingredients,
                    result,
                });
            }
            "minecraft:smithing_transform" => {
                for _ in 0..3 {
                    let _ = read_ingredient(&mut b)?;
                }
                let _ = read_recipe_slot(&mut b)?;
            }
            "minecraft:smithing_trim" => {
                for _ in 0..3 {
                    let _ = read_ingredient(&mut b)?;
                }
            }
            special
                if special.starts_with("minecraft:crafting_special_")
                    || special == "minecraft:crafting_decorated_pot" =>
            {
                let _category = b.read_varint()?;
            }
            other => bail!("unsupported declared recipe type {other}"),
        }
    }
    Ok(DeclaredRecipes {
        crafting,
        stonecutting,
    })
}

fn read_data_component_slot<B: crab_protocol::BufExt>(
    b: &mut B,
    protocol: ProtocolVersion,
) -> Result<play766::ComponentSlot> {
    if protocol == ProtocolVersion::V1_21_5 {
        Ok(play770::read_component_slot(b)?)
    } else if matches!(
        protocol,
        ProtocolVersion::V1_21_2 | ProtocolVersion::V1_21_4
    ) {
        Ok(play766::read_component_slot_768(b)?)
    } else if protocol == ProtocolVersion::V1_21 {
        Ok(play766::read_component_slot_767(b)?)
    } else {
        Ok(play766::read_component_slot(b)?)
    }
}

fn read_component_ingredient<B: crab_protocol::BufExt>(
    b: &mut B,
    protocol: ProtocolVersion,
) -> Result<Vec<i32>> {
    let count = b.read_varint()?;
    if !(0..=4096).contains(&count) {
        bail!("invalid component recipe ingredient alternative count {count}");
    }
    let mut items = Vec::with_capacity(count as usize);
    for _ in 0..count {
        if let Some(item) = read_data_component_slot(b, protocol)?.item {
            items.push(item.item_id);
        }
    }
    Ok(items)
}

fn read_component_recipe_slot<B: crab_protocol::BufExt>(
    b: &mut B,
    protocol: ProtocolVersion,
) -> Result<Option<SlotItem>> {
    Ok(read_data_component_slot(b, protocol)?.item)
}

fn parse_declared_recipes_766(
    raw: &crab_net::RawPacket,
    protocol: ProtocolVersion,
) -> Result<DeclaredRecipes> {
    let mut b = raw.body.clone();
    let count = b.read_varint()?;
    if !(0..=65_536).contains(&count) {
        bail!("invalid protocol 766 declared recipe count {count}");
    }
    let mut stonecutting = Vec::new();
    let mut crafting = Vec::new();
    for _ in 0..count {
        let id = b.read_string(32_767)?;
        let recipe_type = b.read_varint()?;
        match recipe_type {
            0 => {
                let _group = b.read_string(32_767)?;
                let _category = b.read_varint()?;
                let width = b.read_varint()?;
                let height = b.read_varint()?;
                if !(0..=16).contains(&width) || !(0..=16).contains(&height) {
                    bail!("invalid protocol 766 shaped dimensions {width}x{height}");
                }
                let ingredients = (0..width * height)
                    .map(|_| read_component_ingredient(&mut b, protocol))
                    .collect::<Result<Vec<_>>>()?;
                let result = read_component_recipe_slot(&mut b, protocol)?;
                let _show_notification = b.read_bool()?;
                crafting.push(CraftingRecipe {
                    id,
                    width: width as u8,
                    height: height as u8,
                    ingredients,
                    result,
                });
            }
            1 => {
                let _group = b.read_string(32_767)?;
                let _category = b.read_varint()?;
                let ingredients = b.read_varint()?;
                if !(0..=4096).contains(&ingredients) {
                    bail!("invalid protocol 766 shapeless ingredient count {ingredients}");
                }
                let ingredients = (0..ingredients)
                    .map(|_| read_component_ingredient(&mut b, protocol))
                    .collect::<Result<Vec<_>>>()?;
                let result = read_component_recipe_slot(&mut b, protocol)?;
                crafting.push(CraftingRecipe {
                    id,
                    width: 0,
                    height: 0,
                    ingredients,
                    result,
                });
            }
            2..=14 | 22 => {
                let _category = b.read_varint()?;
            }
            15..=18 => {
                let _group = b.read_string(32_767)?;
                let _category = b.read_varint()?;
                let _ingredient = read_component_ingredient(&mut b, protocol)?;
                let _result = read_component_recipe_slot(&mut b, protocol)?;
                let _experience = b.read_f32()?;
                let _cook_time = b.read_varint()?;
            }
            19 => {
                let _group = b.read_string(32_767)?;
                let ingredients = read_component_ingredient(&mut b, protocol)?;
                let result = read_component_recipe_slot(&mut b, protocol)?;
                stonecutting.push(StonecutterRecipe {
                    id,
                    ingredients,
                    result,
                });
            }
            20 => {
                for _ in 0..3 {
                    let _ = read_component_ingredient(&mut b, protocol)?;
                }
                let _ = read_component_recipe_slot(&mut b, protocol)?;
            }
            21 => {
                for _ in 0..3 {
                    let _ = read_component_ingredient(&mut b, protocol)?;
                }
            }
            other => bail!("unsupported protocol 766 recipe type {other}"),
        }
    }
    Ok(DeclaredRecipes {
        crafting,
        stonecutting,
    })
}

#[derive(Default)]
struct SlotDisplay768 {
    items: Vec<i32>,
    representative: Option<SlotItem>,
}

fn read_id_set_768<B: crab_protocol::BufExt>(body: &mut B) -> Result<Vec<i32>> {
    let encoded = body.read_varint()?;
    if encoded == 0 {
        let _tag = body.read_string(32_767)?;
        return Ok(Vec::new());
    }
    if !(1..=65_537).contains(&encoded) {
        bail!("invalid protocol 768 ID-set size {encoded}");
    }
    (0..encoded - 1)
        .map(|_| body.read_varint().map_err(Into::into))
        .collect()
}

fn read_slot_display_768<B: crab_protocol::BufExt>(
    body: &mut B,
    depth: usize,
) -> Result<SlotDisplay768> {
    if depth > 32 {
        bail!("protocol 768 slot display nesting is too deep");
    }
    let display_type = body.read_varint()?;
    Ok(match display_type {
        0 | 1 => SlotDisplay768::default(),
        2 => {
            let item_id = body.read_varint()?;
            SlotDisplay768 {
                items: vec![item_id],
                representative: Some(SlotItem { item_id, count: 1 }),
            }
        }
        3 => {
            let representative = play766::read_component_slot_768(body)?.item;
            SlotDisplay768 {
                items: representative.iter().map(|item| item.item_id).collect(),
                representative,
            }
        }
        4 => {
            let _tag = body.read_string(32_767)?;
            SlotDisplay768::default()
        }
        5 => {
            let base = read_slot_display_768(body, depth + 1)?;
            let _material = read_slot_display_768(body, depth + 1)?;
            let _pattern = read_slot_display_768(body, depth + 1)?;
            base
        }
        6 => {
            let input = read_slot_display_768(body, depth + 1)?;
            let _remainder = read_slot_display_768(body, depth + 1)?;
            input
        }
        7 => {
            let count = body.read_varint()?;
            if !(0..=4096).contains(&count) {
                bail!("invalid protocol 768 composite display size {count}");
            }
            let mut combined = SlotDisplay768::default();
            for _ in 0..count {
                let child = read_slot_display_768(body, depth + 1)?;
                if combined.representative.is_none() {
                    combined.representative = child.representative;
                }
                combined.items.extend(child.items);
            }
            combined.items.sort_unstable();
            combined.items.dedup();
            combined
        }
        other => bail!("invalid protocol 768 slot display type {other}"),
    })
}

fn read_recipe_display_768<B: crab_protocol::BufExt>(
    body: &mut B,
    display_id: i32,
) -> Result<(Option<CraftingRecipe>, Option<StonecutterRecipe>)> {
    let id = display_id.to_string();
    Ok(match body.read_varint()? {
        0 => {
            let count = body.read_varint()?;
            if !(0..=256).contains(&count) {
                bail!("invalid shapeless display ingredient count {count}");
            }
            let mut ingredients = Vec::with_capacity(count as usize);
            for _ in 0..count {
                ingredients.push(read_slot_display_768(body, 0)?.items);
            }
            let result = read_slot_display_768(body, 0)?.representative;
            let _station = read_slot_display_768(body, 0)?;
            (
                Some(CraftingRecipe {
                    id,
                    width: 0,
                    height: 0,
                    ingredients,
                    result,
                }),
                None,
            )
        }
        1 => {
            let width = body.read_varint()?;
            let height = body.read_varint()?;
            if !(0..=16).contains(&width) || !(0..=16).contains(&height) {
                bail!("invalid shaped display dimensions {width}x{height}");
            }
            let count = body.read_varint()?;
            if !(0..=256).contains(&count) {
                bail!("invalid shaped display ingredient count {count}");
            }
            let mut ingredients = Vec::with_capacity(count as usize);
            for _ in 0..count {
                ingredients.push(read_slot_display_768(body, 0)?.items);
            }
            let result = read_slot_display_768(body, 0)?.representative;
            let _station = read_slot_display_768(body, 0)?;
            (
                Some(CraftingRecipe {
                    id,
                    width: width as u8,
                    height: height as u8,
                    ingredients,
                    result,
                }),
                None,
            )
        }
        2 => {
            for _ in 0..4 {
                let _ = read_slot_display_768(body, 0)?;
            }
            let _duration = body.read_varint()?;
            let _experience = body.read_f32()?;
            (None, None)
        }
        3 => {
            let ingredient = read_slot_display_768(body, 0)?.items;
            let result = read_slot_display_768(body, 0)?.representative;
            let _station = read_slot_display_768(body, 0)?;
            (
                None,
                Some(StonecutterRecipe {
                    id,
                    ingredients: ingredient,
                    result,
                }),
            )
        }
        4 => {
            for _ in 0..5 {
                let _ = read_slot_display_768(body, 0)?;
            }
            (None, None)
        }
        other => bail!("invalid protocol 768 recipe display type {other}"),
    })
}

fn handle_recipe_book_add_768(raw: &crab_net::RawPacket, shared: &Arc<Shared>) -> Result<()> {
    let mut body = raw.body.clone();
    let count = body.read_varint()?;
    if !(0..=65_536).contains(&count) {
        bail!("invalid protocol 768 recipe-book add count {count}");
    }
    let mut crafting = Vec::new();
    let mut stonecutting = Vec::new();
    let mut unlocked = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let display_id = body.read_varint()?;
        let (crafting_recipe, stonecutter_recipe) = read_recipe_display_768(&mut body, display_id)?;
        let _group = body.read_varint()?;
        let _category = body.read_varint()?;
        if body.read_bool()? {
            let requirement_count = body.read_varint()?;
            if !(0..=256).contains(&requirement_count) {
                bail!("invalid recipe requirement count {requirement_count}");
            }
            for _ in 0..requirement_count {
                let _ = read_id_set_768(&mut body)?;
            }
        }
        let _flags = body.read_u8()?;
        if let Some(recipe) = crafting_recipe {
            crafting.push(recipe);
        }
        if let Some(recipe) = stonecutter_recipe {
            stonecutting.push(recipe);
        }
        unlocked.push(display_id.to_string());
    }
    let replace = body.read_bool()?;
    if replace {
        *shared.crafting_recipes.lock().unwrap() = crafting;
        *shared.stonecutter_recipes.lock().unwrap() = stonecutting;
        *shared.unlocked_recipes.lock().unwrap() = unlocked.into_iter().collect();
    } else {
        shared.crafting_recipes.lock().unwrap().extend(crafting);
        shared
            .stonecutter_recipes
            .lock()
            .unwrap()
            .extend(stonecutting);
        shared.unlocked_recipes.lock().unwrap().extend(unlocked);
    }
    Ok(())
}

fn handle_recipe_book_remove_768(raw: &crab_net::RawPacket, shared: &Arc<Shared>) -> Result<()> {
    let mut body = raw.body.clone();
    let count = body.read_varint()?;
    if !(0..=65_536).contains(&count) {
        bail!("invalid protocol 768 recipe-book remove count {count}");
    }
    let ids = (0..count)
        .map(|_| {
            body.read_varint()
                .map(|id| id.to_string())
                .map_err(Into::into)
        })
        .collect::<Result<HashSet<_>>>()?;
    shared
        .crafting_recipes
        .lock()
        .unwrap()
        .retain(|recipe| !ids.contains(&recipe.id));
    shared
        .stonecutter_recipes
        .lock()
        .unwrap()
        .retain(|recipe| !ids.contains(&recipe.id));
    shared
        .unlocked_recipes
        .lock()
        .unwrap()
        .retain(|id| !ids.contains(id));
    Ok(())
}

fn handle_unlock_recipes(raw: &crab_net::RawPacket, shared: &Arc<Shared>) -> Result<()> {
    let mut body = raw.body.clone();
    let action = body.read_varint()?;
    for _ in 0..8 {
        let _ = body.read_bool()?;
    }
    let recipes = read_recipe_ids(&mut body)?;
    if action == 0 {
        let _highlighted = read_recipe_ids(&mut body)?;
    }
    let mut unlocked = shared.unlocked_recipes.lock().unwrap();
    match action {
        0 => {
            *unlocked = recipes.into_iter().collect();
        }
        1 => unlocked.extend(recipes),
        2 => {
            for recipe in recipes {
                unlocked.remove(&recipe);
            }
        }
        _ => bail!("invalid unlock-recipes action {action}"),
    }
    Ok(())
}

fn read_recipe_ids<B: crab_protocol::BufExt>(body: &mut B) -> Result<Vec<String>> {
    let count = body.read_varint()?;
    if !(0..=65_536).contains(&count) {
        bail!("invalid unlocked recipe count {count}");
    }
    (0..count)
        .map(|_| body.read_string(32_767).map_err(Into::into))
        .collect()
}

fn handle_map_data(
    raw: &crab_net::RawPacket,
    shared: &Arc<Shared>,
    protocol: ProtocolVersion,
) -> Result<()> {
    let mut b = raw.body.clone();
    let map_id = b.read_varint()?;
    let scale = b.read_i8()?;
    let locked = b.read_bool()?;
    let markers = if b.read_bool()? {
        let count = b.read_varint()?;
        if !(0..=4096).contains(&count) {
            bail!("invalid map marker count {count}");
        }
        let mut markers = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let kind = b.read_varint()?;
            let x = b.read_i8()?;
            let z = b.read_i8()?;
            let direction = b.read_u8()?;
            let label = if b.read_bool()? {
                Some(read_text_component(&mut b, protocol)?)
            } else {
                None
            };
            markers.push(MapMarker {
                kind,
                x,
                z,
                direction,
                label,
            });
        }
        Some(markers)
    } else {
        None
    };
    let columns = usize::from(b.read_u8()?);
    let patch = if columns == 0 {
        None
    } else {
        let rows = usize::from(b.read_u8()?);
        let x = usize::from(b.read_u8()?);
        let y = usize::from(b.read_u8()?);
        let colors = b.read_byte_array()?;
        if x + columns > 128 || y + rows > 128 || colors.len() != columns * rows {
            bail!(
                "invalid map patch {columns}x{rows} at {x},{y} with {} bytes",
                colors.len()
            );
        }
        Some((x, y, columns, rows, colors))
    };

    let mut maps = shared.maps.lock().unwrap();
    let map = maps.entry(map_id).or_default();
    map.scale = scale;
    map.locked = locked;
    if let Some(markers) = markers {
        map.markers = markers;
    }
    if let Some((x, y, columns, rows, colors)) = patch {
        for row in 0..rows {
            let source = row * columns;
            let destination = (y + row) * 128 + x;
            map.colors[destination..destination + columns]
                .copy_from_slice(&colors[source..source + columns]);
        }
    }
    drop(maps);
    *shared.latest_map.lock().unwrap() = Some(map_id);
    Ok(())
}

fn read_chunk_signs<B: crab_protocol::BufExt>(
    body: &mut B,
    chunk_x: i32,
    chunk_z: i32,
    protocol: ProtocolVersion,
) -> Result<Vec<PositionedSign>> {
    let count = body.read_varint()?;
    if !(0..=65_536).contains(&count) {
        bail!("invalid chunk block-entity count {count}");
    }
    let mut signs = Vec::new();
    for _ in 0..count {
        let packed_xz = body.read_u8()?;
        let y = i32::from(body.read_i16()?);
        let _block_entity_type = body.read_varint()?;
        let data = if protocol == ProtocolVersion::V1_20_1 {
            crab_protocol::nbt::read_nbt(body)?
        } else {
            crab_protocol::nbt::read_anonymous_nbt(body)?
        };
        if let Some(sign) = sign_from_nbt(&data) {
            let x = chunk_x * 16 + i32::from(packed_xz >> 4);
            let z = chunk_z * 16 + i32::from(packed_xz & 0x0f);
            signs.push(((x, y, z), sign));
        }
    }
    Ok(signs)
}

fn replace_chunk_signs(
    shared: &Arc<Shared>,
    (chunk_x, chunk_z): (i32, i32),
    signs: Vec<PositionedSign>,
) {
    let mut stored = shared.signs.lock().unwrap();
    stored.retain(|&(x, _, z), _| x >> 4 != chunk_x || z >> 4 != chunk_z);
    stored.extend(signs);
}

fn handle_block_entity_data(
    raw: &crab_net::RawPacket,
    shared: &Arc<Shared>,
    protocol: ProtocolVersion,
) -> Result<()> {
    let mut body = raw.body.clone();
    let position = body.read_position()?;
    let _block_entity_type = body.read_varint()?;
    let data = if protocol == ProtocolVersion::V1_20_1 {
        crab_protocol::nbt::read_nbt(&mut body)?
    } else {
        crab_protocol::nbt::read_anonymous_nbt(&mut body)?
    };
    let mut signs = shared.signs.lock().unwrap();
    if let Some(sign) = sign_from_nbt(&data) {
        signs.insert(position, sign);
    } else {
        signs.remove(&position);
    }
    Ok(())
}

fn sign_from_nbt(data: &crab_protocol::nbt::Nbt) -> Option<SignState> {
    use crab_protocol::nbt::Nbt;

    let read_side = |key: &str| -> Option<([String; 4], bool)> {
        let side = data.get(key)?;
        let lines = match side.get("messages")? {
            Nbt::List(messages) => std::array::from_fn(|index| {
                messages
                    .get(index)
                    .and_then(|message| match message {
                        Nbt::String(json) => Some(plain_text(json)),
                        _ => None,
                    })
                    .unwrap_or_default()
            }),
            _ => return None,
        };
        let glowing = matches!(side.get("has_glowing_text"), Some(Nbt::Byte(value)) if *value != 0);
        Some((lines, glowing))
    };

    if let Some((front, front_glowing)) = read_side("front_text") {
        let (back, back_glowing) = read_side("back_text").unwrap_or_default();
        return Some(SignState {
            front,
            back,
            front_glowing,
            back_glowing,
        });
    }

    // Pre-1.20/converted worlds may still expose the four legacy root fields.
    let mut found = false;
    let front = std::array::from_fn(|index| match data.get(&format!("Text{}", index + 1)) {
        Some(Nbt::String(json)) => {
            found = true;
            plain_text(json)
        }
        _ => String::new(),
    });
    found.then_some(SignState {
        front,
        ..SignState::default()
    })
}

fn handle_resource_pack_request(raw: &crab_net::RawPacket, shared: &Arc<Shared>) -> Result<()> {
    let mut b = raw.body.clone();
    let url = b.read_string(32_767)?;
    let hash = b.read_string(40)?;
    let forced = b.read_bool()?;
    let prompt = if b.read_bool()? {
        Some(plain_text(&b.read_string(262_144)?))
    } else {
        None
    };
    set_resource_pack_request(shared, None, url, hash, forced, prompt);
    Ok(())
}

fn handle_resource_pack_request_765(raw: &crab_net::RawPacket, shared: &Arc<Shared>) -> Result<()> {
    let packet: play765::AddResourcePack = raw.decode()?;
    set_resource_pack_request(
        shared,
        Some(packet.uuid),
        packet.url,
        packet.hash,
        packet.forced,
        packet.prompt.as_ref().map(nbt_plain_text),
    );
    Ok(())
}

fn handle_remove_resource_pack_765(raw: &crab_net::RawPacket, shared: &Arc<Shared>) -> Result<()> {
    let mut body = raw.body.clone();
    let removed = body.read_bool()?.then(|| body.read_uuid()).transpose()?;
    {
        let mut layers = shared.resource_pack_layers.lock().unwrap();
        if let Some(uuid) = removed {
            layers.retain(|layer| layer.uuid != Some(uuid));
        } else {
            layers.clear();
        }
    }
    if removed.is_none()
        || shared
            .resource_pack_request
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|request| request.uuid)
            == removed
    {
        *shared.resource_pack_request.lock().unwrap() = None;
        *shared.resource_pack_prompt_open.lock().unwrap() = false;
    }
    let base = shared.base_resource_jar.lock().unwrap().clone();
    let rebuilt = rebuild_resource_pack_stack(shared, base.as_deref())?;
    shared
        .resource_pack_reload_ack
        .store(false, Ordering::SeqCst);
    *shared.cached_resource_pack.lock().unwrap() = rebuilt;
    Ok(())
}

fn set_resource_pack_request(
    shared: &Arc<Shared>,
    uuid: Option<uuid::Uuid>,
    url: String,
    hash: String,
    forced: bool,
    prompt: Option<String>,
) {
    *shared.resource_pack_request.lock().unwrap() = Some(ResourcePackRequest {
        uuid,
        url,
        hash,
        forced,
        prompt,
    });
    *shared.resource_pack_prompt_open.lock().unwrap() = true;
}

async fn download_resource_pack(request: &ResourcePackRequest) -> Result<PathBuf> {
    const MAX_PACK_BYTES: u64 = 100 * 1024 * 1024;
    let url = reqwest::Url::parse(&request.url).context("invalid resource-pack URL")?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("resource-pack URL must use HTTP or HTTPS");
    }
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?
        .get(url)
        .send()
        .await?
        .error_for_status()?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_PACK_BYTES)
    {
        bail!("resource pack exceeds 100 MiB limit");
    }
    let bytes = response.bytes().await?;
    if bytes.len() as u64 > MAX_PACK_BYTES {
        bail!("resource pack exceeds 100 MiB limit");
    }
    let digest = format!("{:x}", Sha1::digest(&bytes));
    let expected = request.hash.trim().to_ascii_lowercase();
    if !expected.is_empty() && expected != digest {
        bail!("resource-pack SHA-1 mismatch: expected {expected}, got {digest}");
    }
    validate_resource_pack(&bytes)?;
    let directory = std::env::temp_dir().join("crabcraft-resource-packs");
    std::fs::create_dir_all(&directory)?;
    let path = directory.join(format!("{digest}.zip"));
    std::fs::write(&path, &bytes)?;
    Ok(path)
}

fn activate_resource_pack(
    shared: &Arc<Shared>,
    uuid: Option<uuid::Uuid>,
    archive: PathBuf,
    base_jar: Option<&Path>,
) -> Result<PathBuf> {
    let previous = {
        let mut layers = shared.resource_pack_layers.lock().unwrap();
        let previous = layers.clone();
        if let Some(uuid) = uuid {
            layers.retain(|layer| layer.uuid != Some(uuid));
        } else {
            layers.clear();
        }
        layers.push(ResourcePackLayer { uuid, archive });
        previous
    };
    match rebuild_resource_pack_stack(shared, base_jar) {
        Ok(Some(path)) => {
            shared
                .resource_pack_reload_ack
                .store(true, Ordering::SeqCst);
            Ok(path)
        }
        Ok(None) => {
            *shared.resource_pack_layers.lock().unwrap() = previous;
            bail!("activated pack stack is empty")
        }
        Err(error) => {
            *shared.resource_pack_layers.lock().unwrap() = previous;
            Err(error)
        }
    }
}

fn rebuild_resource_pack_stack(
    shared: &Arc<Shared>,
    base_jar: Option<&Path>,
) -> Result<Option<PathBuf>> {
    let layers = shared.resource_pack_layers.lock().unwrap().clone();
    if layers.is_empty() {
        return Ok(base_jar.map(Path::to_path_buf));
    }
    let Some(base) = base_jar else {
        if shared.renderer_active.load(Ordering::SeqCst) {
            bail!("a windowed server resource pack requires CRABCRAFT_JAR for vanilla fallback assets");
        }
        return Ok(layers.last().map(|layer| layer.archive.clone()));
    };

    let directory = std::env::temp_dir().join("crabcraft-resource-packs");
    std::fs::create_dir_all(&directory)?;
    let mut digest = Sha1::new();
    for layer in &layers {
        digest.update(layer.archive.to_string_lossy().as_bytes());
    }
    let key = format!("{:x}", digest.finalize());
    let mut current = base.to_path_buf();
    for (index, layer) in layers.iter().enumerate() {
        let output = directory.join(format!("stack-{key}-{index}.zip"));
        current = layer_resource_pack(&current, &layer.archive, &output)?;
    }
    Ok(Some(current))
}

/// Ensures the download is a real vanilla resource-pack archive before it is
/// cached or acknowledged. We do not extract entries, so path traversal names
/// cannot escape the cache directory.
fn validate_resource_pack(bytes: &[u8]) -> Result<()> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).context("invalid resource-pack ZIP")?;
    let mut metadata = String::new();
    archive
        .by_name("pack.mcmeta")
        .context("resource pack has no root pack.mcmeta")?
        .read_to_string(&mut metadata)?;
    let value: serde_json::Value =
        serde_json::from_str(&metadata).context("invalid pack.mcmeta JSON")?;
    if value
        .pointer("/pack/pack_format")
        .and_then(serde_json::Value::as_u64)
        .is_none()
    {
        bail!("pack.mcmeta has no numeric pack.pack_format");
    }
    Ok(())
}

/// Creates a compact layered archive containing vanilla Minecraft assets from
/// `base_jar` with every matching server-pack entry replacing its base entry.
/// Existing loaders can then resolve parent models and omitted textures exactly
/// as they do against the normal client jar.
fn layer_resource_pack(base_jar: &Path, pack: &Path, output: &Path) -> Result<PathBuf> {
    let mut overlay = zip::ZipArchive::new(File::open(pack)?)?;
    let overlay_names: HashSet<String> = (0..overlay.len())
        .filter_map(|index| {
            overlay
                .by_index(index)
                .ok()
                .map(|entry| entry.name().to_owned())
        })
        .filter(|name| name.starts_with("assets/") && !name.ends_with('/'))
        .collect();
    if overlay_names.is_empty() {
        bail!("resource pack contains no assets");
    }

    let temporary = output.with_extension("zip.part");
    let mut writer = zip::ZipWriter::new(File::create(&temporary)?);
    let mut base = zip::ZipArchive::new(File::open(base_jar).context("open CRABCRAFT_JAR")?)?;
    for index in 0..base.len() {
        let entry = base.by_index(index)?;
        let name = entry.name().to_owned();
        if name.starts_with("assets/minecraft/")
            && !name.ends_with('/')
            && !overlay_names.contains(&name)
        {
            writer.raw_copy_file(entry)?;
        }
    }
    for index in 0..overlay.len() {
        let entry = overlay.by_index(index)?;
        if entry.name().starts_with("assets/") && !entry.name().ends_with('/') {
            writer.raw_copy_file(entry)?;
        }
    }
    writer.finish()?;
    if output.exists() {
        std::fs::remove_file(output)?;
    }
    std::fs::rename(&temporary, output)?;
    Ok(output.to_path_buf())
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

fn sync_local_rider(shared: &Arc<Shared>, vehicle_id: i32) {
    if shared.player.lock().unwrap().vehicle != Some(vehicle_id) {
        return;
    }
    let seat = shared
        .entities
        .lock()
        .unwrap()
        .get(&vehicle_id)
        .map(|vehicle| {
            (
                vehicle.x,
                vehicle.y + f64::from(vehicle.height) * 0.75,
                vehicle.z,
                vehicle.yaw,
            )
        });
    if let Some((x, y, z, yaw)) = seat {
        let mut player = shared.player.lock().unwrap();
        (player.x, player.y, player.z) = (x, y, z);
        player.yaw = yaw;
    }
}

/// Relative move (works for both `rel_entity_move` and `entity_move_look`;
/// trailing look bytes are ignored).
fn handle_rel_move(raw: &crab_net::RawPacket, shared: &Arc<Shared>, has_rot: bool) -> Result<()> {
    let mut b = raw.body.clone();
    let id = b.read_varint()?;
    let (dx, dy, dz) = (b.read_i16()?, b.read_i16()?, b.read_i16()?);
    let yaw = if has_rot {
        let yaw = angle_to_deg(b.read_u8()?);
        let _pitch = b.read_u8()?;
        Some(yaw)
    } else {
        None
    };
    if let Some(e) = shared.entities.lock().unwrap().get_mut(&id) {
        e.x += f64::from(dx) / 4096.0;
        e.y += f64::from(dy) / 4096.0;
        e.z += f64::from(dz) / 4096.0;
        if let Some(yaw) = yaw {
            e.yaw = yaw;
        }
    }
    sync_local_rider(shared, id);
    Ok(())
}

/// Update Entity Rotation (0x2d): yaw + pitch only.
fn handle_entity_rotation(raw: &crab_net::RawPacket, shared: &Arc<Shared>) -> Result<()> {
    let mut b = raw.body.clone();
    let id = b.read_varint()?;
    let yaw = angle_to_deg(b.read_u8()?);
    if let Some(e) = shared.entities.lock().unwrap().get_mut(&id) {
        e.yaw = yaw;
    }
    sync_local_rider(shared, id);
    Ok(())
}

fn handle_entity_teleport(raw: &crab_net::RawPacket, shared: &Arc<Shared>) -> Result<()> {
    let mut b = raw.body.clone();
    let id = b.read_varint()?;
    let (x, y, z) = (b.read_f64()?, b.read_f64()?, b.read_f64()?);
    let yaw = angle_to_deg(b.read_u8()?);
    if let Some(e) = shared.entities.lock().unwrap().get_mut(&id) {
        e.x = x;
        e.y = y;
        e.z = z;
        e.yaw = yaw;
    }
    Ok(())
}

fn handle_entity_destroy(raw: &crab_net::RawPacket, shared: &Arc<Shared>) -> Result<()> {
    let mut b = raw.body.clone();
    let count = b.read_varint()?.max(0);
    let mut entities = shared.entities.lock().unwrap();
    let mut removed = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let id = b.read_varint()?;
        entities.remove(&id);
        removed.push(id);
    }
    drop(entities);
    let mut player = shared.player.lock().unwrap();
    if player
        .vehicle
        .is_some_and(|vehicle| removed.contains(&vehicle))
    {
        player.vehicle = None;
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

fn decode_open_screen_765(raw: &crab_net::RawPacket) -> Result<OpenScreen> {
    let mut body = raw.body.clone();
    let window_id = body.read_varint()?;
    let menu_type = body.read_varint()?;
    let title = nbt_plain_text(&crab_protocol::nbt::read_anonymous_nbt(&mut body)?);
    Ok(OpenScreen {
        window_id,
        menu_type,
        title,
    })
}

fn handle_open_screen(shared: &Arc<Shared>, pkt: OpenScreen) {
    let Ok(window_id) = u8::try_from(pkt.window_id) else {
        return;
    };
    *shared.container.lock().unwrap() = Some(ContainerState {
        window_id,
        menu_type: pkt.menu_type,
        title: plain_text(&pkt.title),
        slots: Vec::new(),
        slot_metadata: Vec::new(),
        state_id: 0,
        properties: [0; 4],
    });
}

fn handle_container_data(shared: &Arc<Shared>, pkt: SetContainerData) {
    let Ok(property) = usize::try_from(pkt.property) else {
        return;
    };
    if property >= 4 {
        return;
    }
    if let Some(container) = shared
        .container
        .lock()
        .unwrap()
        .as_mut()
        .filter(|c| c.window_id == pkt.window_id)
    {
        container.properties[property] = pkt.value;
    }
}

fn handle_close_container(shared: &Arc<Shared>, window_id: u8) {
    let mut open = shared.container.lock().unwrap();
    if open.as_ref().is_some_and(|c| c.window_id == window_id) {
        *open = None;
        *shared.carried.lock().unwrap() = None;
    }
}

fn sync_player_inventory_from_container(shared: &Arc<Shared>, slots: &[Option<SlotItem>]) {
    if slots.len() < 36 {
        return;
    }
    let player_start = slots.len() - 36;
    let mut inv = shared.inventory.lock().unwrap();
    for (dst, item) in inv[9..45].iter_mut().zip(&slots[player_start..]) {
        *dst = *item;
    }
}

fn read_item_nbt<B: crab_protocol::BufExt>(
    body: &mut B,
) -> Result<Option<crab_protocol::nbt::Nbt>> {
    if !body.read_bool()? {
        return Ok(None);
    }
    let _item_id = body.read_varint()?;
    let _count = body.read_i8()?;
    let nbt = crab_protocol::nbt::read_nbt(body)?;
    Ok((!matches!(nbt, crab_protocol::nbt::Nbt::End)).then_some(nbt))
}

fn decode_component_container_content(
    raw: &crab_net::RawPacket,
    protocol: ProtocolVersion,
) -> Result<(SetContainerContent, Vec<Option<crab_protocol::nbt::Nbt>>)> {
    let mut body = raw.body.clone();
    let window_id = body.read_u8()?;
    let state_id = body.read_varint()?;
    let count = body.read_varint()?;
    if !(0..=65_536).contains(&count) {
        bail!("invalid component container slot count {count}");
    }
    let mut slots = Vec::with_capacity(count as usize);
    let mut metadata = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let decoded = read_data_component_slot(&mut body, protocol)?;
        slots.push(decoded.item);
        metadata.push(decoded.metadata);
    }
    let carried = read_data_component_slot(&mut body, protocol)?.item;
    Ok((
        SetContainerContent {
            window_id,
            state_id,
            slots,
            carried,
        },
        metadata,
    ))
}

fn decode_component_container_slot(
    raw: &crab_net::RawPacket,
    protocol: ProtocolVersion,
) -> Result<(SetContainerSlot, Option<crab_protocol::nbt::Nbt>)> {
    let mut body = raw.body.clone();
    let window_id = body.read_i8()?;
    let state_id = body.read_varint()?;
    let slot = body.read_i16()?;
    let decoded = read_data_component_slot(&mut body, protocol)?;
    Ok((
        SetContainerSlot {
            window_id,
            state_id,
            slot,
            item: decoded.item,
        },
        decoded.metadata,
    ))
}

fn capture_component_content_metadata(
    shared: &Arc<Shared>,
    window_id: u8,
    metadata: Vec<Option<crab_protocol::nbt::Nbt>>,
) {
    if window_id == 0 {
        let mut inventory_nbt = shared.inventory_nbt.lock().unwrap();
        *inventory_nbt = metadata;
        inventory_nbt.resize(PLAYER_INVENTORY_SLOTS, None);
    } else if let Some(container) = shared
        .container
        .lock()
        .unwrap()
        .as_mut()
        .filter(|container| container.window_id == window_id)
    {
        container.slot_metadata = metadata;
        let player_start = container.container_slot_count();
        if container.slot_metadata.len() >= player_start + 36 {
            let mut inventory_nbt = shared.inventory_nbt.lock().unwrap();
            for (destination, value) in inventory_nbt[9..45]
                .iter_mut()
                .zip(&container.slot_metadata[player_start..player_start + 36])
            {
                *destination = value.clone();
            }
        }
    }
}

fn capture_component_slot_metadata(
    shared: &Arc<Shared>,
    pkt: &SetContainerSlot,
    metadata: Option<crab_protocol::nbt::Nbt>,
) {
    if matches!(pkt.window_id, 0 | -2) {
        if let Ok(index) = usize::try_from(pkt.slot) {
            let mut inventory_nbt = shared.inventory_nbt.lock().unwrap();
            if index < inventory_nbt.len() {
                inventory_nbt[index] = metadata;
            }
        }
    } else if pkt.window_id > 0 {
        let Ok(index) = usize::try_from(pkt.slot) else {
            return;
        };
        if let Some(container) = shared
            .container
            .lock()
            .unwrap()
            .as_mut()
            .filter(|container| i8::try_from(container.window_id).ok() == Some(pkt.window_id))
        {
            if index >= container.slot_metadata.len() {
                container.slot_metadata.resize(index + 1, None);
            }
            container.slot_metadata[index] = metadata.clone();
            let player_start = container.container_slot_count();
            if (player_start..player_start + 36).contains(&index) {
                shared.inventory_nbt.lock().unwrap()[9 + index - player_start] = metadata;
            }
        }
    }
}

fn capture_container_content_nbt(raw: &crab_net::RawPacket, shared: &Arc<Shared>) -> Result<()> {
    let mut body = raw.body.clone();
    let window_id = body.read_u8()?;
    let _state_id = body.read_varint()?;
    let count = body.read_varint()?;
    if !(0..=65_536).contains(&count) {
        bail!("invalid container slot count {count}");
    }
    let metadata = (0..count)
        .map(|_| read_item_nbt(&mut body))
        .collect::<Result<Vec<_>>>()?;
    let _carried = read_item_nbt(&mut body)?;
    if window_id == 0 {
        let mut inventory_nbt = shared.inventory_nbt.lock().unwrap();
        *inventory_nbt = metadata;
        inventory_nbt.resize(PLAYER_INVENTORY_SLOTS, None);
    }
    Ok(())
}

fn capture_container_slot_nbt(raw: &crab_net::RawPacket, shared: &Arc<Shared>) -> Result<()> {
    let mut body = raw.body.clone();
    let window_id = body.read_i8()?;
    let _state_id = body.read_varint()?;
    let slot = body.read_i16()?;
    let metadata = read_item_nbt(&mut body)?;
    if matches!(window_id, 0 | -2) {
        if let Ok(index) = usize::try_from(slot) {
            let mut inventory_nbt = shared.inventory_nbt.lock().unwrap();
            if index < inventory_nbt.len() {
                inventory_nbt[index] = metadata;
            }
        }
    }
    Ok(())
}

/// Replaces the tracked player inventory from a full window snapshot (window 0).
fn handle_container_content(shared: &Arc<Shared>, pkt: SetContainerContent) {
    if pkt.window_id != 0 {
        let mut open = shared.container.lock().unwrap();
        if let Some(container) = open.as_mut().filter(|c| c.window_id == pkt.window_id) {
            container.state_id = pkt.state_id;
            container.slots = pkt.slots;
            *shared.carried.lock().unwrap() = pkt.carried;
            sync_player_inventory_from_container(shared, &container.slots);
        }
        return;
    }
    *shared.window_state.lock().unwrap() = pkt.state_id;
    *shared.carried.lock().unwrap() = pkt.carried;
    let mut inv = shared.inventory.lock().unwrap();
    *inv = pkt.slots;
    inv.resize(PLAYER_INVENTORY_SLOTS, None);
    tracing::info!("inventory: {}", inventory_summary(&inv));
}

/// Applies a single slot update to the tracked player inventory.
fn handle_container_slot(shared: &Arc<Shared>, pkt: &SetContainerSlot) {
    // Window -1, slot -1 is the protocol's cursor-stack correction.
    if pkt.window_id == -1 && pkt.slot == -1 {
        *shared.carried.lock().unwrap() = pkt.item;
        if let Some(container) = shared.container.lock().unwrap().as_mut() {
            container.state_id = pkt.state_id;
        } else {
            *shared.window_state.lock().unwrap() = pkt.state_id;
        }
        return;
    }
    // -2 addresses the player inventory directly without an open window.
    let Ok(idx) = usize::try_from(pkt.slot) else {
        return;
    };
    if pkt.window_id > 0 {
        let mut open = shared.container.lock().unwrap();
        if let Some(container) = open
            .as_mut()
            .filter(|c| i8::try_from(c.window_id).ok() == Some(pkt.window_id))
        {
            container.state_id = pkt.state_id;
            if idx < container.slots.len() {
                container.slots[idx] = pkt.item;
                let player_start = container.container_slot_count();
                if idx >= player_start && idx < player_start + 36 {
                    let inv_idx = 9 + idx - player_start;
                    shared.inventory.lock().unwrap()[inv_idx] = pkt.item;
                }
            }
        }
        return;
    }
    if pkt.window_id != 0 && pkt.window_id != -2 {
        return;
    }
    *shared.window_state.lock().unwrap() = pkt.state_id;
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

fn queue_sound_at(
    shared: &Arc<Shared>,
    event: String,
    position: [f64; 3],
    volume: f32,
    fixed_range: Option<f32>,
) {
    let player = *shared.player.lock().unwrap();
    let dx = position[0] - player.x;
    let dy = position[1] - player.y;
    let dz = position[2] - player.z;
    let distance = (dx * dx + dy * dy + dz * dz).sqrt();
    let range = f64::from(fixed_range.unwrap_or_else(|| (volume * 16.0).max(16.0)));
    if range <= 0.0 || distance >= range {
        return;
    }
    let gain = f64::from(volume.max(0.0)) * (1.0 - distance / range);
    queue_sound(shared, format!("@gain:{gain:.3}:{event}"));
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

/// The block state a held item would place on right-click, or `None` if it
/// isn't a real, non-air block. Empty hand (`None`), air (item id 0), zero-count
/// slots, and non-block items all yield `None`, so right-clicking them places
/// nothing and plays no sound — regardless of which hotbar slot is selected.
fn placeable_state(held: Option<SlotItem>) -> Option<u32> {
    let it = held.filter(|it| it.count > 0)?;
    let id = u32::try_from(it.item_id).ok().filter(|&id| id != 0)?;
    let name = crab_registry::item_name(id)?;
    let state = crab_registry::block_by_name(name)?.default_state;
    (!crab_registry::is_air(state)).then_some(state)
}

fn is_interactable_block(name: &str) -> bool {
    let bare = name.strip_prefix("minecraft:").unwrap_or(name);
    matches!(
        bare,
        "chest"
            | "trapped_chest"
            | "ender_chest"
            | "barrel"
            | "crafting_table"
            | "furnace"
            | "blast_furnace"
            | "smoker"
            | "hopper"
            | "dispenser"
            | "dropper"
            | "brewing_stand"
            | "beacon"
            | "anvil"
            | "chipped_anvil"
            | "damaged_anvil"
            | "enchanting_table"
            | "lectern"
            | "loom"
            | "cartography_table"
            | "stonecutter"
            | "grindstone"
            | "smithing_table"
            | "lever"
            | "note_block"
            | "jukebox"
            | "composter"
            | "cake"
            | "respawn_anchor"
            | "flower_pot"
    ) || bare.ends_with("_shulker_box")
        || bare.ends_with("_door")
        || bare.ends_with("_trapdoor")
        || bare.ends_with("_fence_gate")
        || bare.ends_with("_button")
        || bare.ends_with("_bed")
        || bare.ends_with("_cauldron")
        || bare.starts_with("potted_")
}

/// Maximum stack size for an item id (default 64).
fn stack_max(item_id: i32) -> i8 {
    u32::try_from(item_id)
        .ok()
        .and_then(crab_registry::item_def)
        .map_or(64, |d| d.stack_size.min(127) as i8)
}

/// Adds `idx` to the changed-slot list once.
fn mark_changed(changed: &mut Vec<usize>, idx: usize) {
    if !changed.contains(&idx) {
        changed.push(idx);
    }
}

/// Moves a stack through `targets`, first merging into compatible stacks and
/// then filling empty slots. This is the client prediction for shift-click.
fn merge_stack_into(
    inv: &mut [Option<SlotItem>],
    source_idx: usize,
    targets: impl IntoIterator<Item = usize> + Clone,
    changed: &mut Vec<usize>,
) {
    let Some(mut source) = inv[source_idx] else {
        return;
    };

    for idx in targets.clone() {
        if source.count <= 0 {
            break;
        }
        let Some(mut target) = inv[idx] else {
            continue;
        };
        if target.item_id != source.item_id {
            continue;
        }
        let space = (stack_max(target.item_id) - target.count).max(0);
        let moved = space.min(source.count);
        if moved > 0 {
            target.count += moved;
            source.count -= moved;
            inv[idx] = Some(target);
            mark_changed(changed, idx);
        }
    }

    for idx in targets {
        if source.count <= 0 {
            break;
        }
        if inv[idx].is_none() {
            let moved = source.count.min(stack_max(source.item_id));
            inv[idx] = Some(SlotItem {
                item_id: source.item_id,
                count: moved,
            });
            source.count -= moved;
            mark_changed(changed, idx);
        }
    }

    inv[source_idx] = (source.count > 0).then_some(source);
    mark_changed(changed, source_idx);
}

/// Applies a vanilla inventory click locally (the server stays authoritative
/// and corrects via 0x12/0x14). Mode 0 supports normal left/right clicks; mode
/// 1 quick-moves between the main inventory and hotbar (or into both from the
/// crafting/armour/offhand slots).
fn apply_inventory_click(
    inv: &mut [Option<SlotItem>],
    carried: &mut Option<SlotItem>,
    idx: usize,
    button: i8,
    mode: i32,
) -> Vec<(i16, Option<SlotItem>)> {
    let mut changed = Vec::new();
    if mode == 1 {
        match idx {
            9..=35 => merge_stack_into(inv, idx, 36..=44, &mut changed),
            36..=44 => merge_stack_into(inv, idx, 9..=35, &mut changed),
            // Crafting output/grid, armour and offhand prefer the last player
            // inventory slots, matching the vanilla player's quickMoveStack.
            0..=8 | 45 => merge_stack_into(inv, idx, (9..=44).rev(), &mut changed),
            _ => {}
        }
        return changed.into_iter().map(|i| (i as i16, inv[i])).collect();
    }

    let slot = inv[idx];
    let cur = *carried;
    if button == 0 {
        match (cur, slot) {
            (Some(mut c), Some(mut s)) if c.item_id == s.item_id => {
                // Merge the cursor stack into the slot, up to the max.
                let space = (stack_max(s.item_id) - s.count).max(0);
                let mv = space.min(c.count);
                s.count += mv;
                c.count -= mv;
                inv[idx] = Some(s);
                *carried = (c.count > 0).then_some(c);
            }
            _ => {
                inv[idx] = cur;
                *carried = slot;
            }
        }
    } else {
        match (cur, slot) {
            (None, Some(mut s)) => {
                // Pick up half (rounded up).
                let half = (s.count + 1) / 2;
                *carried = Some(SlotItem {
                    item_id: s.item_id,
                    count: half,
                });
                s.count -= half;
                inv[idx] = (s.count > 0).then_some(s);
            }
            (Some(mut c), None) => {
                inv[idx] = Some(SlotItem {
                    item_id: c.item_id,
                    count: 1,
                });
                c.count -= 1;
                *carried = (c.count > 0).then_some(c);
            }
            (Some(mut c), Some(mut s))
                if c.item_id == s.item_id && s.count < stack_max(s.item_id) =>
            {
                s.count += 1;
                c.count -= 1;
                inv[idx] = Some(s);
                *carried = (c.count > 0).then_some(c);
            }
            _ => {
                inv[idx] = cur;
                *carried = slot;
            }
        }
    }
    vec![(idx as i16, inv[idx])]
}

/// Applies a click in a server-owned menu. Normal clicks are universal;
/// shift-click moves container -> player (hotbar first) or player -> container.
fn apply_container_click(
    slots: &mut [Option<SlotItem>],
    carried: &mut Option<SlotItem>,
    idx: usize,
    button: i8,
    mode: i32,
    container_slots: usize,
    menu_type: i32,
) -> Vec<(i16, Option<SlotItem>)> {
    if mode != 1 {
        return apply_inventory_click(slots, carried, idx, button, 0);
    }
    let mut changed = Vec::new();
    if matches!(menu_type, 9 | 13 | 21) && container_slots == 3 {
        if idx < 3 {
            merge_stack_into(slots, idx, (3..slots.len()).rev(), &mut changed);
        } else if slots[idx].is_some_and(|item| is_smeltable(item.item_id)) {
            merge_stack_into(slots, idx, 0..1, &mut changed);
        } else if slots[idx].is_some_and(|item| is_furnace_fuel(item.item_id)) {
            merge_stack_into(slots, idx, 1..2, &mut changed);
        } else if idx < 30 {
            merge_stack_into(slots, idx, 30..39, &mut changed);
        } else {
            merge_stack_into(slots, idx, 3..30, &mut changed);
        }
        return changed.into_iter().map(|i| (i as i16, slots[i])).collect();
    }
    if menu_type == 11 && container_slots == 10 {
        if idx < 10 {
            merge_stack_into(slots, idx, (10..slots.len()).rev(), &mut changed);
        } else if idx < 37 {
            merge_stack_into(slots, idx, 37..46, &mut changed);
        } else {
            merge_stack_into(slots, idx, 10..37, &mut changed);
        }
        return changed.into_iter().map(|i| (i as i16, slots[i])).collect();
    }
    if idx < container_slots {
        merge_stack_into(
            slots,
            idx,
            (container_slots..slots.len()).rev(),
            &mut changed,
        );
    } else {
        merge_stack_into(slots, idx, 0..container_slots, &mut changed);
    }
    changed.into_iter().map(|i| (i as i16, slots[i])).collect()
}

fn item_name_for_slot(item_id: i32) -> Option<&'static str> {
    u32::try_from(item_id)
        .ok()
        .and_then(crab_registry::item_name)
}

fn is_furnace_fuel(item_id: i32) -> bool {
    let Some(name) = item_name_for_slot(item_id) else {
        return false;
    };
    matches!(
        name,
        "coal"
            | "charcoal"
            | "coal_block"
            | "blaze_rod"
            | "lava_bucket"
            | "dried_kelp_block"
            | "bamboo"
            | "stick"
            | "scaffolding"
    ) || name.ends_with("_planks")
        || name.ends_with("_log")
        || name.ends_with("_wood")
        || name.ends_with("_stem")
        || name.ends_with("_hyphae")
        || name.ends_with("_sapling")
        || name.ends_with("_boat")
        || name.ends_with("_sign")
        || name.ends_with("_fence")
        || name.ends_with("_fence_gate")
        || name.ends_with("_stairs")
        || name.ends_with("_slab")
        || name.ends_with("_pressure_plate")
        || name.ends_with("_button")
        || name.ends_with("_door")
        || name.ends_with("_trapdoor")
}

fn is_smeltable(item_id: i32) -> bool {
    let Some(name) = item_name_for_slot(item_id) else {
        return false;
    };
    matches!(
        name,
        "sand"
            | "red_sand"
            | "cobblestone"
            | "stone"
            | "clay_ball"
            | "clay"
            | "netherrack"
            | "ancient_debris"
            | "cactus"
            | "kelp"
            | "potato"
            | "chorus_fruit"
            | "wet_sponge"
            | "sea_pickle"
    ) || name.starts_with("raw_")
        || name.ends_with("_ore")
        || name.ends_with("_log")
        || name.ends_with("_wood")
        || name.ends_with("_stem")
        || name.ends_with("_hyphae")
        || name.starts_with("raw_")
        || (name.starts_with("iron_")
            && matches!(
                name.strip_prefix("iron_"),
                Some(
                    "sword"
                        | "pickaxe"
                        | "axe"
                        | "shovel"
                        | "hoe"
                        | "helmet"
                        | "chestplate"
                        | "leggings"
                        | "boots"
                        | "horse_armor"
                )
            ))
        || (name.starts_with("golden_")
            && matches!(
                name.strip_prefix("golden_"),
                Some(
                    "sword"
                        | "pickaxe"
                        | "axe"
                        | "shovel"
                        | "hoe"
                        | "helmet"
                        | "chestplate"
                        | "leggings"
                        | "boots"
                        | "horse_armor"
                )
            ))
}

/// Queues a block sound for `block` using `event` (e.g. [`crab_audio::break_event`]
/// or [`crab_audio::hit_event`]). Call before the block is removed.
fn play_block_sound(shared: &Arc<Shared>, block: [i32; 3], event: fn(&str) -> String) {
    if let Some(name) = block_name_at(shared, block) {
        queue_sound(shared, event(name));
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
    shared.apply_core_event(ClientEvent::ChatReceived(line.clone()));
    let mut log = shared.chat_log.lock().unwrap();
    log.push_back(line);
    while log.len() > 100 {
        log.pop_front();
    }
}

/// Very small chat-component flattener for readable logging.
pub(crate) fn plain_text(json: &str) -> String {
    serde_json::from_str(json)
        .ok()
        .map(|value| component_text(&value))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| json.to_string())
}

fn component_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(parts) => parts.iter().map(component_text).collect(),
        serde_json::Value::Object(component) => {
            let mut text = component
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            if let Some(key) = component
                .get("translate")
                .and_then(serde_json::Value::as_str)
            {
                text.push_str(match key {
                    "container.chest" => "Chest",
                    "container.chestDouble" => "Large Chest",
                    "container.enderchest" => "Ender Chest",
                    "container.barrel" => "Barrel",
                    "container.shulkerBox" => "Shulker Box",
                    "container.furnace" => "Furnace",
                    "container.blast_furnace" => "Blast Furnace",
                    "container.smoker" => "Smoker",
                    _ => key.rsplit('.').next().unwrap_or(key),
                });
            }
            if let Some(extra) = component.get("extra") {
                text.push_str(&component_text(extra));
            }
            text
        }
        _ => String::new(),
    }
}

fn nbt_plain_text(value: &crab_protocol::nbt::Nbt) -> String {
    use crab_protocol::nbt::Nbt;
    match value {
        Nbt::String(text) => text.clone(),
        Nbt::List(parts) => parts.iter().map(nbt_plain_text).collect(),
        Nbt::Compound(_) => {
            let mut text = value
                .get("text")
                .and_then(|child| match child {
                    Nbt::String(text) => Some(text.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            if let Some(Nbt::String(key)) = value.get("translate") {
                text.push_str(key.rsplit('.').next().unwrap_or(key));
            }
            if let Some(extra) = value.get("extra") {
                text.push_str(&nbt_plain_text(extra));
            }
            text
        }
        _ => String::new(),
    }
}

fn read_text_component<B: Buf>(body: &mut B, protocol: ProtocolVersion) -> Result<String> {
    if protocol.uses_nbt_components() {
        Ok(nbt_plain_text(&crab_protocol::nbt::read_anonymous_nbt(
            body,
        )?))
    } else {
        Ok(plain_text(&body.read_string(262_144)?))
    }
}

/// 1.20.3 score packets append an optional number-format discriminator. The
/// styled and fixed variants carry one additional text component.
fn skip_optional_number_format<B: Buf>(body: &mut B) -> Result<()> {
    if body.read_bool()? {
        let format = body.read_varint()?;
        if matches!(format, 1 | 2) {
            let _ = crab_protocol::nbt::read_anonymous_nbt(body)?;
        }
    }
    Ok(())
}

fn handle_reset_score_765(raw: &crab_net::RawPacket, shared: &Arc<Shared>) -> Result<()> {
    let mut body = raw.body.clone();
    let item = body.read_string(32767)?;
    let objective = body
        .read_bool()?
        .then(|| body.read_string(32767))
        .transpose()?;
    let mut scoreboard = shared.scoreboard.lock().unwrap();
    if let Some(objective) = objective {
        if let Some(scores) = scoreboard.scores.get_mut(&objective) {
            scores.remove(&item);
        }
    } else {
        for scores in scoreboard.scores.values_mut() {
            scores.remove(&item);
        }
    }
    Ok(())
}

fn handle_team_packet(
    raw: &crab_net::RawPacket,
    shared: &Arc<Shared>,
    protocol: ProtocolVersion,
) -> Result<()> {
    let mut body = raw.body.clone();
    let team_name = body.read_string(32767)?;
    let mode = body.read_i8()?;
    if mode == 1 {
        shared.scoreboard.lock().unwrap().teams.remove(&team_name);
        return Ok(());
    }

    let properties = if matches!(mode, 0 | 2) {
        let display_name = read_text_component(&mut body, protocol)?;
        let _friendly_fire = body.read_i8()?;
        let _name_tag_visibility = body.read_string(32767)?;
        let _collision_rule = body.read_string(32767)?;
        let formatting = body.read_varint()?;
        let prefix = read_text_component(&mut body, protocol)?;
        let suffix = read_text_component(&mut body, protocol)?;
        Some((display_name, formatting, prefix, suffix))
    } else {
        None
    };

    let members = if matches!(mode, 0 | 3 | 4) {
        let count = body.read_varint()?;
        if !(0..=65_536).contains(&count) {
            bail!("invalid team member count {count}");
        }
        (0..count)
            .map(|_| body.read_string(32767).map_err(Into::into))
            .collect::<Result<Vec<_>>>()?
    } else {
        Vec::new()
    };

    let mut scoreboard = shared.scoreboard.lock().unwrap();
    let team = scoreboard.teams.entry(team_name).or_default();
    if let Some((display_name, formatting, prefix, suffix)) = properties {
        team.display_name = display_name;
        team.formatting = formatting;
        team.prefix = prefix;
        team.suffix = suffix;
    }
    match mode {
        0 => team.members = members.into_iter().collect(),
        2 => {}
        3 => team.members.extend(members),
        4 => {
            for member in members {
                team.members.remove(&member);
            }
        }
        _ => bail!("invalid team mode {mode}"),
    }
    Ok(())
}

fn split_host_port(addr: &str) -> (String, u16) {
    match addr.rsplit_once(':') {
        Some((host, port)) => (host.to_string(), port.parse().unwrap_or(25565)),
        None => (addr.to_string(), 25565),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BufMut;
    use crab_protocol::BufMutExt;

    fn slot(item_id: i32, count: i8) -> Option<SlotItem> {
        Some(SlotItem { item_id, count })
    }

    #[test]
    fn only_real_blocks_are_placeable() {
        // Empty hand -> nothing (this is the no-op, silent case for any slot).
        assert_eq!(placeable_state(None), None);
        // Air (item id 0) -> nothing (was the phantom "stone place" leak).
        assert_eq!(placeable_state(slot(0, 1)), None);
        // A zero-count slot -> nothing.
        assert_eq!(placeable_state(slot(1, 0)), None);
        // A non-block item (diamond sword) -> nothing.
        assert_eq!(placeable_state(slot(797, 1)), None);
        // A real block (stone, item id 1) -> its non-air default state.
        let state = placeable_state(slot(1, 64)).expect("stone is placeable");
        assert!(!crab_registry::is_air(state));
        assert_eq!(
            crab_registry::block_by_name("stone").unwrap().default_state,
            state
        );
    }

    #[test]
    fn protocol_764_profiles_shift_ids_at_real_insertion_points() {
        assert_eq!(canonical_clientbound_764_id(0x03), ID_ENTITY_ANIMATION);
        assert_eq!(canonical_clientbound_764_id(0x25), ID_MAP_CHUNK);
        assert_eq!(canonical_clientbound_764_id(0x29), ID_JOIN_GAME);
        assert_eq!(canonical_clientbound_764_id(0x42), ID_RESOURCE_PACK);
        assert_eq!(canonical_clientbound_764_id(0x6f), ID_DECLARE_RECIPES);
        assert_eq!(canonical_clientbound_764_id(0x0c), -1);
        assert_eq!(serverbound_764_id(State::Play, PlaceRecipe::ID), 0x1e);
        assert_eq!(
            serverbound_764_id(State::Play, ResourcePackStatus::ID),
            0x27
        );
        assert_eq!(serverbound_764_id(State::Configuration, 0x03), 0x03);
    }

    #[test]
    fn protocol_765_profiles_cover_new_score_pack_and_tick_packets() {
        assert_eq!(canonical_clientbound_765_id(0x03), ID_ENTITY_ANIMATION);
        assert_eq!(canonical_clientbound_765_id(0x25), ID_MAP_CHUNK);
        assert_eq!(canonical_clientbound_765_id(0x29), ID_JOIN_GAME);
        assert_eq!(canonical_clientbound_765_id(0x42), -1); // reset score
        assert_eq!(canonical_clientbound_765_id(0x44), ID_RESOURCE_PACK);
        assert_eq!(canonical_clientbound_765_id(0x67), -1); // start configuration
        assert_eq!(canonical_clientbound_765_id(0x73), ID_DECLARE_RECIPES);
        assert_eq!(serverbound_765_id(State::Play, PlaceRecipe::ID), 0x1f);
        assert_eq!(
            serverbound_765_id(State::Play, ResourcePackStatus::ID),
            0x28
        );
        assert_eq!(serverbound_765_id(State::Configuration, 0x03), 0x03);
    }

    #[test]
    fn protocol_766_profiles_cover_configuration_and_play_insertions() {
        assert_eq!(canonical_clientbound_766_id(0x03), ID_ENTITY_ANIMATION);
        assert_eq!(canonical_clientbound_766_id(0x27), ID_MAP_CHUNK);
        assert_eq!(canonical_clientbound_766_id(0x2b), ID_JOIN_GAME);
        assert_eq!(canonical_clientbound_766_id(0x46), -1); // transfer
        assert_eq!(canonical_clientbound_766_id(0x69), -1); // start configuration
        assert_eq!(canonical_clientbound_766_id(0x79), -1); // recipe declarations moved
        assert_eq!(serverbound_766_id(State::Play, ClientChatMessage::ID), 0x06);
        assert_eq!(serverbound_766_id(State::Play, ClickContainer::ID), 0x0e);
        assert_eq!(serverbound_766_id(State::Play, PlaceRecipe::ID), 0x22);
        assert_eq!(serverbound_766_id(State::Configuration, 0x05), 0x06);
        // 1.21 keeps protocol 766's play packet map while changing selected
        // payloads and registries under protocol number 767.
        assert_eq!(ProtocolVersion::V1_21.number(), 767);
        assert_eq!(canonical_clientbound_766_id(0x2b), ID_JOIN_GAME);
        assert_eq!(serverbound_766_id(State::Play, ClickContainer::ID), 0x0e);
    }

    #[test]
    fn protocol_768_profiles_cover_bundle_packet_insertions() {
        assert_eq!(ProtocolVersion::V1_21_2.number(), 768);
        assert_eq!(canonical_clientbound_768_id(0x28), ID_MAP_CHUNK);
        assert_eq!(canonical_clientbound_768_id(0x2c), ID_JOIN_GAME);
        assert_eq!(
            canonical_clientbound_768_id(0x42),
            SynchronizePlayerPosition::ID
        );
        assert_eq!(canonical_clientbound_768_id(0x5d), ID_ENTITY_METADATA);
        assert_eq!(canonical_clientbound_768_id(0x70), -1); // start configuration
        assert_eq!(serverbound_768_id(State::Play, ClickContainer::ID), 0x10);
        assert_eq!(serverbound_768_id(State::Play, SetPlayerPosition::ID), 0x1c);
        assert_eq!(serverbound_768_id(State::Play, UseItem::ID), 0x3b);
        assert_eq!(serverbound_768_id(State::Configuration, 0x03), 0x04);
    }

    #[test]
    fn protocol_769_profiles_cover_pick_split_and_player_loaded() {
        assert_eq!(ProtocolVersion::V1_21_4.number(), 769);
        // Clientbound IDs are unchanged from 768.
        assert_eq!(canonical_clientbound_768_id(0x2c), ID_JOIN_GAME);
        assert_eq!(canonical_clientbound_768_id(0x63), ID_SET_HELD_ITEM);
        // Serverbound packets after split Pick Item move one slot; packets
        // after Player Loaded move two in total.
        assert_eq!(serverbound_769_id(State::Play, PlayerCommand::ID), 0x28);
        assert_eq!(serverbound_769_id(State::Play, SetHeldItem::ID), 0x33);
        assert_eq!(serverbound_769_id(State::Play, UseItem::ID), 0x3d);
        assert_eq!(serverbound_769_id(State::Configuration, 0x03), 0x04);
    }

    #[test]
    fn protocol_770_profiles_cover_removed_orb_and_game_test_packets() {
        assert_eq!(ProtocolVersion::V1_21_5.number(), 770);
        assert_eq!(canonical_clientbound_770_id(0x27), ID_MAP_CHUNK);
        assert_eq!(canonical_clientbound_770_id(0x2b), ID_JOIN_GAME);
        assert_eq!(
            canonical_clientbound_770_id(0x41),
            SynchronizePlayerPosition::ID
        );
        assert_eq!(canonical_clientbound_770_id(0x77), -1);
        assert_eq!(serverbound_770_id(State::Play, PlayerCommand::ID), 0x28);
        assert_eq!(serverbound_770_id(State::Play, SwingArm::ID), 0x3b);
        assert_eq!(serverbound_770_id(State::Play, UseItem::ID), 0x3f);
    }

    #[test]
    fn join_game_764_reads_configuration_era_field_order() {
        let shared = Arc::new(Shared::new());
        let mut body = Vec::new();
        body.put_i32(1234);
        body.put_bool(false);
        body.put_varint(1);
        body.put_string("minecraft:overworld");
        body.put_varint(20);
        body.put_varint(12);
        body.put_varint(8);
        body.put_bool(false);
        body.put_bool(true);
        body.put_bool(false);
        body.put_string("minecraft:overworld");
        body.put_string("minecraft:overworld");
        body.put_i64(42);
        body.push(1); // creative
        body.push((-1_i8) as u8);
        body.put_bool(false);
        body.put_bool(false);
        body.put_bool(false); // no death position
        body.put_varint(0);
        let raw = crab_net::RawPacket {
            id: ID_JOIN_GAME,
            body: body.into(),
        };

        handle_join_game(&raw, &shared, ProtocolVersion::V1_20_2).unwrap();
        let player = shared.player.lock().unwrap();
        assert_eq!(player.entity_id, 1234);
        assert_eq!(player.gamemode, 1);
    }

    #[test]
    fn teams_765_apply_nbt_prefixes_and_suffixes_to_names() {
        fn put_nbt_string(body: &mut Vec<u8>, text: &str) {
            body.push(8); // unnamed TAG_String
            body.put_u16(text.len() as u16);
            body.extend_from_slice(text.as_bytes());
        }

        let shared = Arc::new(Shared::new());
        let mut body = Vec::new();
        body.put_string("builders");
        body.push(0); // create
        put_nbt_string(&mut body, "Builders");
        body.push(3); // friendly fire flags
        body.put_string("always");
        body.put_string("always");
        body.put_varint(10);
        put_nbt_string(&mut body, "[");
        put_nbt_string(&mut body, "]");
        body.put_varint(1);
        body.put_string("Ferris");
        let raw = crab_net::RawPacket {
            id: ID_TEAMS,
            body: body.into(),
        };

        handle_team_packet(&raw, &shared, ProtocolVersion::V1_20_3).unwrap();
        assert_eq!(
            shared.scoreboard.lock().unwrap().decorated_name("Ferris"),
            "[Ferris]"
        );
    }

    #[test]
    fn offline_uuid_matches_java_name_uuid() {
        assert_eq!(
            offline_uuid("Notch").to_string(),
            "b50ad385-829d-3141-a216-7e7d7539ba7f"
        );
    }

    #[test]
    fn boat_controls_turn_and_advance_in_vehicle_yaw_space() {
        let (straight, yaw) = controlled_boat_step([10.0, 62.0, 20.0], 0.0, 1.0, 0.0, 0.05);
        assert!((straight[0] - 10.0).abs() < 1e-9);
        assert!((straight[2] - 20.2).abs() < 1e-9);
        assert_eq!(yaw, 0.0);

        let (turning, yaw) = controlled_boat_step([0.0, 0.0, 0.0], 0.0, 1.0, 1.0, 0.05);
        assert!((yaw - 3.5).abs() < 1e-6);
        assert!(turning[0] < 0.0);
        assert!(turning[2] > 0.0);
    }

    #[test]
    fn falling_block_spawn_data_is_validated_by_entity_type_and_registry() {
        let falling_block = crab_registry::entities()
            .iter()
            .find(|entity| entity.name == "falling_block")
            .expect("selected registry has falling_block")
            .id as i32;
        let stone = crab_registry::block_by_name("stone")
            .expect("selected registry has stone")
            .default_state as i32;
        let non_block = crab_registry::entities()
            .iter()
            .find(|entity| entity.name != "falling_block")
            .expect("selected registry has other entities")
            .id as i32;

        assert_eq!(
            spawned_block_state(falling_block, stone),
            Some(stone as u32)
        );
        assert_eq!(spawned_block_state(non_block, stone), None);
        assert_eq!(spawned_block_state(falling_block, -1), None);
        assert_eq!(spawned_block_state(falling_block, i32::MAX), None);
    }

    #[test]
    fn component_era_metadata_uses_shifted_pose_serializer() {
        assert_eq!(pose_metadata_type(ProtocolVersion::V1_20_1), 20);
        assert_eq!(pose_metadata_type(ProtocolVersion::V1_20_3), 20);
        assert_eq!(pose_metadata_type(ProtocolVersion::V1_20_5), 21);
        assert_eq!(pose_metadata_type(ProtocolVersion::V1_21), 21);
    }

    #[test]
    fn passenger_updates_mount_and_dismount_the_local_player() {
        let shared = Arc::new(Shared::new());
        shared.player.lock().unwrap().entity_id = 42;
        shared.entities.lock().unwrap().insert(
            7,
            Entity {
                x: 5.0,
                y: 63.0,
                z: -2.0,
                half_width: 0.6875,
                height: 0.5625,
                type_id: 9,
                scale: 1.0,
                yaw: 90.0,
                head_yaw: 90.0,
                velocity: [0.0; 3],
                equipment: [None; 6],
                vehicle: None,
                pose: 0,
                invisible: false,
                glowing: false,
                swing_sequence: 0,
                hurt_sequence: 0,
                item: None,
                block_state: None,
            },
        );
        let packet = |passengers: &[i32]| {
            let mut body = Vec::new();
            body.put_varint(7);
            body.put_varint(passengers.len() as i32);
            for &passenger in passengers {
                body.put_varint(passenger);
            }
            crab_net::RawPacket {
                id: ID_SET_PASSENGERS,
                body: body.into(),
            }
        };

        handle_set_passengers(&packet(&[42]), &shared).unwrap();
        let mounted = *shared.player.lock().unwrap();
        assert_eq!(mounted.vehicle, Some(7));
        assert_eq!((mounted.x, mounted.z), (5.0, -2.0));
        assert!(mounted.y > 63.0);

        handle_set_passengers(&packet(&[]), &shared).unwrap();
        assert_eq!(shared.player.lock().unwrap().vehicle, None);
    }

    #[test]
    fn declared_recipes_extract_stonecutter_inputs_and_results() {
        let mut body = Vec::new();
        body.put_varint(3);
        body.put_string("minecraft:crafting_special_repairitem");
        body.put_string("minecraft:repair");
        body.put_varint(0);
        body.put_string("minecraft:crafting_shapeless");
        body.put_string("minecraft:test_planks");
        body.put_string("");
        body.put_varint(0); // category
        body.put_varint(1); // one ingredient
        body.put_varint(1); // one ingredient alternative
        body.put_bool(true);
        body.put_varint(12);
        body.push(1);
        body.push(0);
        body.put_bool(true);
        body.put_varint(13);
        body.push(4);
        body.push(0);
        body.put_string("minecraft:stonecutting");
        body.put_string("minecraft:stone_slab_from_stone_stonecutting");
        body.put_string("");
        body.put_varint(1); // one ingredient alternative
        body.put_bool(true);
        body.put_varint(1); // stone
        body.push(1);
        body.push(0); // no NBT
        body.put_bool(true);
        body.put_varint(53); // stone slab
        body.push(2);
        body.push(0);
        let raw = crab_net::RawPacket {
            id: ID_DECLARE_RECIPES,
            body: body.into(),
        };

        let recipes = parse_declared_recipes(&raw, ProtocolVersion::V1_20_1).unwrap();
        assert_eq!(recipes.stonecutting.len(), 1);
        assert_eq!(recipes.stonecutting[0].ingredients, vec![1]);
        assert_eq!(recipes.stonecutting[0].result, slot(53, 2));
        assert_eq!(recipes.crafting.len(), 1);
        assert_eq!(recipes.crafting[0].id, "minecraft:test_planks");
        assert_eq!(recipes.crafting[0].ingredients, vec![vec![12]]);
        assert_eq!(recipes.crafting[0].result, slot(13, 4));
    }

    #[test]
    fn protocol_768_recipe_book_displays_feed_existing_recipe_ui() {
        let shared = Arc::new(Shared::new());
        let mut body = Vec::new();
        body.put_varint(2); // entries

        body.put_varint(17); // display id
        body.put_varint(1); // shaped
        body.put_varint(2); // width
        body.put_varint(1); // height
        body.put_varint(2); // ingredient display count
        body.put_varint(2); // item display
        body.put_varint(1);
        body.put_varint(2);
        body.put_varint(2);
        body.put_varint(2); // result item display
        body.put_varint(3);
        body.put_varint(0); // empty crafting station display
        body.put_varint(0); // no group (optvarint)
        body.put_varint(0); // category
        body.put_bool(false); // no requirements
        body.put_u8(0); // flags

        body.put_varint(18); // display id
        body.put_varint(3); // stonecutter
        body.put_varint(7); // composite ingredient display
        body.put_varint(2);
        body.put_varint(2);
        body.put_varint(4);
        body.put_varint(2);
        body.put_varint(5);
        body.put_varint(2); // result item display
        body.put_varint(6);
        body.put_varint(0); // empty station
        body.put_varint(0); // group
        body.put_varint(10); // stonecutter category
        body.put_bool(false);
        body.put_u8(0);
        body.put_bool(true); // replace

        let raw = crab_net::RawPacket {
            id: 0x44,
            body: body.into(),
        };
        handle_recipe_book_add_768(&raw, &shared).unwrap();
        let crafting = shared.crafting_recipes.lock().unwrap();
        assert_eq!(crafting.len(), 1);
        assert_eq!(crafting[0].id, "17");
        assert_eq!(crafting[0].width, 2);
        assert_eq!(crafting[0].ingredients, vec![vec![1], vec![2]]);
        assert_eq!(crafting[0].result, slot(3, 1));
        drop(crafting);
        let stonecutting = shared.stonecutter_recipes.lock().unwrap();
        assert_eq!(stonecutting.len(), 1);
        assert_eq!(stonecutting[0].id, "18");
        assert_eq!(stonecutting[0].ingredients, vec![4, 5]);
        assert_eq!(stonecutting[0].result, slot(6, 1));
        drop(stonecutting);
        assert_eq!(shared.unlocked_recipes.lock().unwrap().len(), 2);

        let mut remove = Vec::new();
        remove.put_varint(1);
        remove.put_varint(17);
        handle_recipe_book_remove_768(
            &crab_net::RawPacket {
                id: 0x45,
                body: remove.into(),
            },
            &shared,
        )
        .unwrap();
        assert!(shared.crafting_recipes.lock().unwrap().is_empty());
        assert_eq!(shared.stonecutter_recipes.lock().unwrap().len(), 1);
    }

    #[test]
    fn map_packets_apply_partial_pixels_and_markers() {
        let shared = Arc::new(Shared::new());
        let mut body = Vec::new();
        body.put_varint(7);
        body.push(2); // scale
        body.put_bool(true); // locked
        body.put_bool(true); // markers present
        body.put_varint(1);
        body.put_varint(0);
        body.push(10);
        body.push((-12_i8) as u8);
        body.push(3);
        body.put_bool(true);
        body.put_string(r#"{"text":"Home"}"#);
        body.push(2); // columns
        body.push(2); // rows
        body.push(4); // x
        body.push(5); // y
        body.put_varint(4);
        body.extend_from_slice(&[4, 8, 12, 16]);
        let raw = crab_net::RawPacket {
            id: ID_MAP_DATA,
            body: body.into(),
        };

        handle_map_data(&raw, &shared, ProtocolVersion::V1_20_1).unwrap();
        let maps = shared.maps.lock().unwrap();
        let map = &maps[&7];
        assert_eq!((map.scale, map.locked), (2, true));
        assert_eq!(map.markers[0].label.as_deref(), Some("Home"));
        assert_eq!(&map.colors[5 * 128 + 4..5 * 128 + 6], &[4, 8]);
        assert_eq!(&map.colors[6 * 128 + 4..6 * 128 + 6], &[12, 16]);
        assert_eq!(*shared.latest_map.lock().unwrap(), Some(7));
    }

    #[test]
    fn resource_pack_requests_preserve_forced_prompt_and_hash() {
        let shared = Arc::new(Shared::new());
        let mut body = Vec::new();
        body.put_string("https://example.invalid/server-pack.zip");
        body.put_string("0123456789abcdef0123456789abcdef01234567");
        body.put_bool(true);
        body.put_bool(true);
        body.put_string(r#"{"text":"Required visuals"}"#);
        let raw = crab_net::RawPacket {
            id: ID_RESOURCE_PACK,
            body: body.into(),
        };

        handle_resource_pack_request(&raw, &shared).unwrap();
        let request = shared
            .resource_pack_request
            .lock()
            .unwrap()
            .clone()
            .unwrap();
        assert!(request.forced);
        assert_eq!(request.prompt.as_deref(), Some("Required visuals"));
        assert_eq!(request.hash.len(), 40);
        assert!(*shared.resource_pack_prompt_open.lock().unwrap());
    }

    #[test]
    fn shift_click_merges_before_using_empty_slots() {
        let mut inv = vec![None; PLAYER_INVENTORY_SLOTS];
        inv[9] = slot(1, 10);
        inv[36] = slot(1, 60);
        let mut carried = None;

        let changed = apply_inventory_click(&mut inv, &mut carried, 9, 0, 1);

        assert_eq!(inv[9], None);
        assert_eq!(inv[36], slot(1, 64));
        assert_eq!(inv[37], slot(1, 6));
        assert_eq!(carried, None);
        assert_eq!(
            changed,
            vec![(36, slot(1, 64)), (37, slot(1, 6)), (9, None)]
        );
    }

    #[test]
    fn shift_click_moves_hotbar_stack_to_main_inventory() {
        let mut inv = vec![None; PLAYER_INVENTORY_SLOTS];
        inv[40] = slot(764, 3); // diamonds
        let mut carried = None;

        let changed = apply_inventory_click(&mut inv, &mut carried, 40, 0, 1);

        assert_eq!(inv[9], slot(764, 3));
        assert_eq!(inv[40], None);
        assert_eq!(changed, vec![(9, slot(764, 3)), (40, None)]);
    }

    #[test]
    fn shift_click_from_special_slot_prefers_hotbar_end() {
        let mut inv = vec![None; PLAYER_INVENTORY_SLOTS];
        inv[0] = slot(1, 1); // crafting result
        let mut carried = None;

        apply_inventory_click(&mut inv, &mut carried, 0, 0, 1);

        assert_eq!(inv[0], None);
        assert_eq!(inv[44], slot(1, 1));
    }

    #[test]
    fn server_snapshots_and_corrections_update_carried_stack() {
        let shared = Arc::new(Shared::new());
        handle_container_content(
            &shared,
            SetContainerContent {
                window_id: 0,
                state_id: 4,
                slots: vec![None; PLAYER_INVENTORY_SLOTS],
                carried: slot(764, 2),
            },
        );
        assert_eq!(*shared.carried.lock().unwrap(), slot(764, 2));

        handle_container_slot(
            &shared,
            &SetContainerSlot {
                window_id: -1,
                state_id: 5,
                slot: -1,
                item: slot(1, 12),
            },
        );
        assert_eq!(*shared.carried.lock().unwrap(), slot(1, 12));
        assert_eq!(*shared.window_state.lock().unwrap(), 5);
    }

    #[test]
    fn server_container_lifecycle_syncs_player_inventory() {
        let shared = Arc::new(Shared::new());
        handle_open_screen(
            &shared,
            OpenScreen {
                window_id: 7,
                menu_type: 2,
                title: r#"{"translate":"container.chest"}"#.to_string(),
            },
        );
        let mut slots = vec![None; 27 + 36];
        slots[0] = slot(1, 8);
        slots[27] = slot(764, 3);
        handle_container_content(
            &shared,
            SetContainerContent {
                window_id: 7,
                state_id: 12,
                slots,
                carried: None,
            },
        );
        let open = shared.container.lock().unwrap().clone().unwrap();
        assert_eq!(open.title, "Chest");
        assert_eq!(open.generic_rows(), Some(3));
        assert_eq!(open.slots[0], slot(1, 8));
        assert_eq!(shared.inventory.lock().unwrap()[9], slot(764, 3));

        handle_container_slot(
            &shared,
            &SetContainerSlot {
                window_id: 7,
                state_id: 13,
                slot: 27,
                item: slot(764, 4),
            },
        );
        assert_eq!(shared.inventory.lock().unwrap()[9], slot(764, 4));
        handle_close_container(&shared, 7);
        assert!(shared.container.lock().unwrap().is_none());
    }

    #[test]
    fn container_shift_click_moves_to_player_inventory() {
        let mut slots = vec![None; 27 + 36];
        slots[0] = slot(1, 10);
        slots[62] = slot(1, 60);
        let mut carried = None;

        let changed = apply_container_click(&mut slots, &mut carried, 0, 0, 1, 27, 2);

        assert_eq!(slots[0], None);
        assert_eq!(slots[62], slot(1, 64));
        assert_eq!(slots[61], slot(1, 6));
        assert_eq!(changed[0], (62, slot(1, 64)));
        assert_eq!(changed.last(), Some(&(0, None)));
    }

    #[test]
    fn furnace_properties_and_shift_click_routes_are_tracked() {
        let shared = Arc::new(Shared::new());
        handle_open_screen(
            &shared,
            OpenScreen {
                window_id: 8,
                menu_type: 13,
                title: r#"{"translate":"container.furnace"}"#.to_string(),
            },
        );
        handle_container_data(
            &shared,
            SetContainerData {
                window_id: 8,
                property: 2,
                value: 75,
            },
        );
        let open = shared.container.lock().unwrap().clone().unwrap();
        assert_eq!(open.furnace_texture(), Some("furnace"));
        assert_eq!(open.properties[2], 75);

        let mut slots = vec![None; 39];
        slots[3] = slot(51, 4); // iron ore -> input
        let mut carried = None;
        apply_container_click(&mut slots, &mut carried, 3, 0, 1, 3, 13);
        assert_eq!(slots[0], slot(51, 4));
        assert_eq!(slots[3], None);

        slots[4] = slot(762, 2); // coal -> fuel
        apply_container_click(&mut slots, &mut carried, 4, 0, 1, 3, 13);
        assert_eq!(slots[1], slot(762, 2));

        slots[5] = slot(764, 1); // diamond -> hotbar fallback
        apply_container_click(&mut slots, &mut carried, 5, 0, 1, 3, 13);
        assert_eq!(slots[30], slot(764, 1));
    }

    #[test]
    fn simple_workstation_menu_types_select_vanilla_layouts() {
        for (menu_type, texture, slots) in [
            (6, "dispenser", 9),
            (7, "anvil", 3),
            (10, "brewing_stand", 5),
            (11, "crafting_table", 10),
            (12, "enchanting_table", 2),
            (14, "grindstone", 3),
            (15, "hopper", 5),
            (17, "loom", 4),
            (20, "smithing", 4),
            (22, "cartography_table", 3),
            (23, "stonecutter", 2),
        ] {
            let state = ContainerState {
                window_id: 1,
                menu_type,
                title: String::new(),
                slots: vec![None; slots + 36],
                slot_metadata: vec![None; slots + 36],
                state_id: 0,
                properties: [0; 4],
            };
            assert_eq!(state.simple_container_texture(), Some(texture));
            assert_eq!(state.container_slot_count(), slots);
        }
    }

    #[test]
    fn crafting_table_shift_click_keeps_result_slot_server_owned() {
        let mut slots = vec![None; 46];
        slots[10] = slot(1, 8);
        let mut carried = None;
        apply_container_click(&mut slots, &mut carried, 10, 0, 1, 10, 11);
        assert_eq!(slots[0], None);
        assert_eq!(slots[1], None);
        assert_eq!(slots[37], slot(1, 8));

        slots[0] = slot(764, 1);
        apply_container_click(&mut slots, &mut carried, 0, 0, 1, 10, 11);
        assert_eq!(slots[0], None);
        assert_eq!(slots[45], slot(764, 1));
    }

    #[test]
    fn sprint_and_sneak_speeds_match_player_modes() {
        assert!((movement_speed(false, false, None) - 4.317).abs() < 1e-9);
        assert!((movement_speed(false, true, None) - 5.612).abs() < 1e-9);
        assert!((movement_speed(true, true, None) - 1.295).abs() < 1e-9);
        assert!((movement_speed(false, true, Some(FluidKind::Water)) - 3.0).abs() < 1e-9);
        assert!((movement_speed(false, false, Some(FluidKind::Lava)) - 1.0).abs() < 1e-9);
        assert!((flight_speed(0.05, false) - 10.92).abs() < 1e-6);
        assert!((flight_speed(0.05, true) - 21.84).abs() < 1e-6);
    }

    #[test]
    fn status_effects_adjust_movement_mining_and_jump_levels() {
        let effect = |amplifier| ActiveEffect {
            amplifier,
            duration: 200,
            ambient: false,
            show_particles: true,
            show_icon: true,
        };
        let mut effects = HashMap::new();
        effects.insert(1, effect(1)); // Speed II: +40%
        assert!((effect_movement_multiplier(&effects) - 1.4).abs() < 1e-9);
        effects.insert(2, effect(0)); // Slowness I: -15%
        assert!((effect_movement_multiplier(&effects) - 1.19).abs() < 1e-9);

        effects.clear();
        effects.insert(3, effect(0)); // Haste I
        assert_eq!(adjusted_break_ticks(12, &effects), 10);
        effects.clear();
        effects.insert(4, effect(0)); // Mining Fatigue I
        assert_eq!(adjusted_break_ticks(12, &effects), 40);
        effects.insert(8, effect(2));
        assert_eq!(effect_level(&effects, 8), 3);
    }

    #[test]
    fn player_fluid_detection_reads_loaded_world_states() {
        use crab_world::{Biomes, BlockStates, Section};
        let sections = (0..24)
            .map(|_| Section {
                block_count: 0,
                blocks: BlockStates::Uniform(0),
                biomes: Biomes::Uniform(0),
            })
            .collect();
        let mut world = World::overworld();
        world.load_chunk(Chunk {
            x: 0,
            z: 0,
            sections,
        });
        let water = crab_registry::block_by_name("water").unwrap().default_state;
        world.set_block_state(8, -59, 8, water);
        assert_eq!(
            fluid_kind_at(&world, [8.5, -59.0, 8.5], 1.8),
            Some(FluidKind::Water)
        );
        let lava = crab_registry::block_by_name("lava").unwrap().default_state;
        world.set_block_state(8, -59, 8, lava);
        assert_eq!(
            fluid_kind_at(&world, [8.5, -59.0, 8.5], 1.8),
            Some(FluidKind::Lava)
        );
    }

    #[test]
    fn interaction_blocks_suppress_normal_placement_prediction() {
        for name in [
            "minecraft:chest",
            "minecraft:furnace",
            "minecraft:oak_door",
            "minecraft:stone_button",
        ] {
            assert!(is_interactable_block(name));
        }
        assert!(!is_interactable_block("minecraft:stone"));
    }

    #[test]
    fn swap_hands_exchanges_selected_hotbar_and_offhand_slots() {
        let mut inventory = vec![None; 46];
        inventory[38] = slot(1, 12);
        inventory[45] = slot(764, 1);

        swap_selected_with_offhand(&mut inventory, 2);

        assert_eq!(inventory[38], slot(764, 1));
        assert_eq!(inventory[45], slot(1, 12));
    }

    #[test]
    fn resource_pack_validation_requires_vanilla_metadata() {
        let valid = zip_bytes(&[
            (
                "pack.mcmeta",
                br#"{"pack":{"pack_format":15,"description":"test"}}"#,
            ),
            ("assets/minecraft/textures/block/stone.png", b"texture"),
        ]);
        validate_resource_pack(&valid).unwrap();

        let missing = zip_bytes(&[("assets/minecraft/example", b"data")]);
        assert!(validate_resource_pack(&missing).is_err());
        let malformed = zip_bytes(&[("pack.mcmeta", br#"{"pack":{"description":"test"}}"#)]);
        assert!(validate_resource_pack(&malformed).is_err());
    }

    #[test]
    fn sign_nbt_retains_both_sides_and_flattens_components() {
        use crab_protocol::nbt::Nbt;
        let side = |messages: [&str; 4], glowing: i8| {
            Nbt::Compound(HashMap::from([
                (
                    "messages".to_string(),
                    Nbt::List(
                        messages
                            .into_iter()
                            .map(|message| Nbt::String(message.to_string()))
                            .collect(),
                    ),
                ),
                ("has_glowing_text".to_string(), Nbt::Byte(glowing)),
            ]))
        };
        let data = Nbt::Compound(HashMap::from([
            (
                "front_text".to_string(),
                side(
                    [
                        r#"{"text":"Hello ","extra":[{"text":"world"}]}"#,
                        r#""second""#,
                        r#"{"text":""}"#,
                        r#"{"translate":"container.chest"}"#,
                    ],
                    1,
                ),
            ),
            (
                "back_text".to_string(),
                side([r#"{"text":"Back"}"#, "{}", "{}", "{}"], 0),
            ),
        ]));
        let sign = sign_from_nbt(&data).unwrap();
        assert_eq!(
            sign.front,
            ["Hello world", "second", r#"{"text":""}"#, "Chest"]
        );
        assert_eq!(sign.back[0], "Back");
        assert!(sign.front_glowing);
        assert!(!sign.back_glowing);
    }

    #[test]
    fn layered_resource_pack_overrides_and_preserves_base_assets() {
        let root = std::env::temp_dir().join(format!(
            "crabcraft-pack-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let base = root.join("base.zip");
        let pack = root.join("pack.zip");
        let layered = root.join("layered.zip");
        std::fs::write(
            &base,
            zip_bytes(&[
                ("assets/minecraft/models/block/stone.json", b"base-model"),
                ("assets/minecraft/textures/block/stone.png", b"base-texture"),
                ("com/example/ignored.class", b"bytecode"),
            ]),
        )
        .unwrap();
        std::fs::write(
            &pack,
            zip_bytes(&[
                ("pack.mcmeta", br#"{"pack":{"pack_format":15}}"#),
                ("assets/minecraft/textures/block/stone.png", b"pack-texture"),
                ("assets/minecraft/textures/item/apple.png", b"pack-apple"),
            ]),
        )
        .unwrap();

        layer_resource_pack(&base, &pack, &layered).unwrap();
        let mut archive = zip::ZipArchive::new(File::open(&layered).unwrap()).unwrap();
        assert_eq!(
            zip_entry(&mut archive, "assets/minecraft/textures/block/stone.png"),
            b"pack-texture"
        );
        assert_eq!(
            zip_entry(&mut archive, "assets/minecraft/models/block/stone.json"),
            b"base-model"
        );
        assert_eq!(
            zip_entry(&mut archive, "assets/minecraft/textures/item/apple.png"),
            b"pack-apple"
        );
        assert!(archive.by_name("com/example/ignored.class").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn uuid_resource_pack_stack_rebuilds_after_middle_layer_removal() {
        let root = std::env::temp_dir().join(format!(
            "crabcraft-pack-stack-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let base = root.join("base.zip");
        let first = root.join("first.zip");
        let second = root.join("second.zip");
        std::fs::write(
            &base,
            zip_bytes(&[
                ("assets/minecraft/a.txt", b"base-a"),
                ("assets/minecraft/b.txt", b"base-b"),
            ]),
        )
        .unwrap();
        std::fs::write(&first, zip_bytes(&[("assets/minecraft/a.txt", b"first-a")])).unwrap();
        std::fs::write(
            &second,
            zip_bytes(&[("assets/minecraft/b.txt", b"second-b")]),
        )
        .unwrap();

        let shared = Arc::new(Shared::new());
        *shared.base_resource_jar.lock().unwrap() = Some(base.clone());
        let first_uuid = uuid::Uuid::new_v4();
        let second_uuid = uuid::Uuid::new_v4();
        activate_resource_pack(&shared, Some(first_uuid), first, Some(&base)).unwrap();
        let stacked =
            activate_resource_pack(&shared, Some(second_uuid), second, Some(&base)).unwrap();
        let mut archive = zip::ZipArchive::new(File::open(stacked).unwrap()).unwrap();
        assert_eq!(
            zip_entry(&mut archive, "assets/minecraft/a.txt"),
            b"first-a"
        );
        assert_eq!(
            zip_entry(&mut archive, "assets/minecraft/b.txt"),
            b"second-b"
        );

        let mut remove = Vec::new();
        remove.put_bool(true);
        remove.put_uuid(first_uuid);
        handle_remove_resource_pack_765(
            &crab_net::RawPacket {
                id: 0x43,
                body: remove.into(),
            },
            &shared,
        )
        .unwrap();
        let rebuilt = shared.cached_resource_pack.lock().unwrap().clone().unwrap();
        let mut archive = zip::ZipArchive::new(File::open(rebuilt).unwrap()).unwrap();
        assert_eq!(zip_entry(&mut archive, "assets/minecraft/a.txt"), b"base-a");
        assert_eq!(
            zip_entry(&mut archive, "assets/minecraft/b.txt"),
            b"second-b"
        );
        assert_eq!(shared.resource_pack_layers.lock().unwrap().len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut writer, bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn zip_entry<R: Read + std::io::Seek>(archive: &mut zip::ZipArchive<R>, name: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        archive
            .by_name(name)
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        bytes
    }
}
