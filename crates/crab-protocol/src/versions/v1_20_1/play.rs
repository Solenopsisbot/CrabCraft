//! Play state (protocol 763) — the subset Crabcraft needs to hold a connection
//! and behave like a real player: keepalive, spawn/teleport, position, chat.
//!
//! Packet IDs below are confirmed against a live vanilla 1.20.1 server (the
//! anchors `Login`=0x28 and `SynchronizePlayerPosition`=0x3c were observed on
//! the wire, and the rest come from the same protocol table).
//!
//! > Chat note: 1.20.1 sends chat components as JSON **strings**; the move to
//! > NBT network components only happened in 1.20.3 (protocol 765).

use std::time::{SystemTime, UNIX_EPOCH};

use bytes::{Buf, BufMut};

use crate::error::ProtoError;
use crate::io::{BufExt, BufMutExt};
use crate::nbt;
use crate::packet::{Bound, Packet, State};

// ===========================================================================
// Clientbound
// ===========================================================================

/// `0x30` — opens a server-owned menu such as a chest or furnace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenScreen {
    pub window_id: i32,
    pub menu_type: i32,
    /// JSON chat component in 1.20.1.
    pub title: String,
}

impl Packet for OpenScreen {
    const ID: i32 = 0x30;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Clientbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_varint(self.window_id);
        dst.put_varint(self.menu_type);
        dst.put_string(&self.title);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            window_id: src.read_varint()?,
            menu_type: src.read_varint()?,
            title: src.read_string(262_144)?,
        })
    }
}

/// `0x11` — the server closes the currently open container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientboundCloseContainer {
    pub window_id: u8,
}

/// `0x13` — updates one numeric property of an open container. Furnaces use
/// properties 0..=3 for remaining burn time, total burn time, cook progress,
/// and total cook time respectively.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetContainerData {
    pub window_id: u8,
    pub property: i16,
    pub value: i16,
}

/// `0x34` — server-authoritative player ability flags and movement speeds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClientboundPlayerAbilities {
    pub flags: i8,
    pub flying_speed: f32,
    pub walking_speed: f32,
}

impl Packet for ClientboundPlayerAbilities {
    const ID: i32 = 0x34;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Clientbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_i8(self.flags);
        dst.put_f32(self.flying_speed);
        dst.put_f32(self.walking_speed);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            flags: src.read_i8()?,
            flying_speed: src.read_f32()?,
            walking_speed: src.read_f32()?,
        })
    }
}

impl Packet for SetContainerData {
    const ID: i32 = 0x13;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Clientbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_u8(self.window_id);
        dst.put_i16(self.property);
        dst.put_i16(self.value);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            window_id: src.read_u8()?,
            property: src.read_i16()?,
            value: src.read_i16()?,
        })
    }
}

impl Packet for ClientboundCloseContainer {
    const ID: i32 = 0x11;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Clientbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_u8(self.window_id);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            window_id: src.read_u8()?,
        })
    }
}

/// `0x23` — server pings; we must echo the same id back promptly or get kicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeepAlive {
    pub id: i64,
}

/// `0x32` — play-state latency probe; echo with [`Pong`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ping {
    pub id: i32,
}

impl Packet for Ping {
    const ID: i32 = 0x32;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Clientbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_i32(self.id);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            id: src.read_i32()?,
        })
    }
}

impl Packet for KeepAlive {
    const ID: i32 = 0x23;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Clientbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_i64(self.id);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            id: src.read_i64()?,
        })
    }
}

/// `0x3c` — authoritative (re)position. Flags mark each axis as relative.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SynchronizePlayerPosition {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    /// Bit flags: 0x01 X, 0x02 Y, 0x04 Z, 0x08 yaw, 0x10 pitch (set => relative).
    pub flags: u8,
    pub teleport_id: i32,
}

impl Packet for SynchronizePlayerPosition {
    const ID: i32 = 0x3c;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Clientbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_f64(self.x);
        dst.put_f64(self.y);
        dst.put_f64(self.z);
        dst.put_f32(self.yaw);
        dst.put_f32(self.pitch);
        dst.put_u8(self.flags);
        dst.put_varint(self.teleport_id);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            x: src.read_f64()?,
            y: src.read_f64()?,
            z: src.read_f64()?,
            yaw: src.read_f32()?,
            pitch: src.read_f32()?,
            flags: src.read_u8()?,
            teleport_id: src.read_varint()?,
        })
    }
}

/// `0x64` — a server/system chat line. `overlay` true means action-bar text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemChat {
    /// JSON-encoded chat component.
    pub content: String,
    pub overlay: bool,
}

impl Packet for SystemChat {
    const ID: i32 = 0x64;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Clientbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_string(&self.content);
        dst.put_bool(self.overlay);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            content: src.read_string(262144)?,
            overlay: src.read_bool()?,
        })
    }
}

/// `0x1a` — kicked while in-game; body is a JSON chat component.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayDisconnect {
    pub reason_json: String,
}

impl Packet for PlayDisconnect {
    const ID: i32 = 0x1a;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Clientbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_string(&self.reason_json);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            reason_json: src.read_string(262144)?,
        })
    }
}

// ===========================================================================
// Serverbound
// ===========================================================================

/// `0x20` — response to a play-state [`Ping`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pong {
    pub id: i32,
}

impl Packet for Pong {
    const ID: i32 = 0x20;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_i32(self.id);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            id: src.read_i32()?,
        })
    }
}

/// `0x0c` — acknowledges that the client closed a server-owned container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CloseContainer {
    pub window_id: u8,
}

/// `0x0e` — replaces the pages of a writable book, optionally signing it with
/// the supplied title. The slot is the selected hotbar index (0..8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditBook {
    pub slot: i32,
    pub pages: Vec<String>,
    pub title: Option<String>,
}

/// `0x1b` — asks the server to place a known recipe into a crafting grid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaceRecipe {
    pub window_id: i8,
    pub recipe: String,
    pub make_all: bool,
}

impl Packet for PlaceRecipe {
    const ID: i32 = 0x1b;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_i8(self.window_id);
        dst.put_string(&self.recipe);
        dst.put_bool(self.make_all);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            window_id: src.read_i8()?,
            recipe: src.read_string(32_767)?,
            make_all: src.read_bool()?,
        })
    }
}

impl Packet for EditBook {
    const ID: i32 = 0x0e;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_varint(self.slot);
        dst.put_varint(self.pages.len() as i32);
        for page in &self.pages {
            dst.put_string(page);
        }
        dst.put_bool(self.title.is_some());
        if let Some(title) = &self.title {
            dst.put_string(title);
        }
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        let slot = src.read_varint()?;
        let page_count = src.read_varint()?;
        if !(0..=100).contains(&page_count) {
            return Err(ProtoError::InvalidEnum {
                type_name: "EditBook.page_count",
                value: i64::from(page_count),
            });
        }
        let pages = (0..page_count)
            .map(|_| src.read_string(8_192))
            .collect::<Result<_, _>>()?;
        let title = src.read_bool()?.then(|| src.read_string(128)).transpose()?;
        Ok(Self { slot, pages, title })
    }
}

/// `0x1e` — changes a player action state. Action ids used here are 0/1 for
/// start/stop sneaking and 3/4 for start/stop sprinting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerCommand {
    pub entity_id: i32,
    pub action: i32,
    pub jump_boost: i32,
}

/// `0x32` — uses the held item without targeting a block (food, bows, buckets,
/// shields, throwable items, etc.).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UseItem {
    pub hand: i32,
    pub sequence: i32,
}

/// `0x1c` — toggles the client's flying ability state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServerboundPlayerAbilities {
    pub flags: i8,
}

/// `0x18` — moves the root vehicle currently controlled by the player.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VehicleMove {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
}

/// `0x19` — controls the two paddles of a boat or chest boat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SteerBoat {
    pub left_paddle: bool,
    pub right_paddle: bool,
}

/// `0x1f` — forwards movement input to a ridden entity. Flag bit 0 jumps and
/// bit 1 requests dismounting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SteerVehicle {
    pub sideways: f32,
    pub forward: f32,
    pub flags: u8,
}

impl Packet for VehicleMove {
    const ID: i32 = 0x18;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_f64(self.x);
        dst.put_f64(self.y);
        dst.put_f64(self.z);
        dst.put_f32(self.yaw);
        dst.put_f32(self.pitch);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            x: src.read_f64()?,
            y: src.read_f64()?,
            z: src.read_f64()?,
            yaw: src.read_f32()?,
            pitch: src.read_f32()?,
        })
    }
}

impl Packet for SteerBoat {
    const ID: i32 = 0x19;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_bool(self.left_paddle);
        dst.put_bool(self.right_paddle);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            left_paddle: src.read_bool()?,
            right_paddle: src.read_bool()?,
        })
    }
}

impl Packet for SteerVehicle {
    const ID: i32 = 0x1f;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_f32(self.sideways);
        dst.put_f32(self.forward);
        dst.put_u8(self.flags);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            sideways: src.read_f32()?,
            forward: src.read_f32()?,
            flags: src.read_u8()?,
        })
    }
}

impl Packet for ServerboundPlayerAbilities {
    const ID: i32 = 0x1c;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_i8(self.flags);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            flags: src.read_i8()?,
        })
    }
}

impl Packet for UseItem {
    const ID: i32 = 0x32;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_varint(self.hand);
        dst.put_varint(self.sequence);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            hand: src.read_varint()?,
            sequence: src.read_varint()?,
        })
    }
}

impl Packet for PlayerCommand {
    const ID: i32 = 0x1e;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_varint(self.entity_id);
        dst.put_varint(self.action);
        dst.put_varint(self.jump_boost);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            entity_id: src.read_varint()?,
            action: src.read_varint()?,
            jump_boost: src.read_varint()?,
        })
    }
}

impl Packet for CloseContainer {
    const ID: i32 = 0x0c;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_u8(self.window_id);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            window_id: src.read_u8()?,
        })
    }
}

/// `0x12` — our reply to [`KeepAlive`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeepAliveResponse {
    pub id: i64,
}

impl Packet for KeepAliveResponse {
    const ID: i32 = 0x12;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_i64(self.id);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            id: src.read_i64()?,
        })
    }
}

/// `0x00` — acknowledge a [`SynchronizePlayerPosition`] by echoing its id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfirmTeleport {
    pub teleport_id: i32,
}

impl Packet for ConfirmTeleport {
    const ID: i32 = 0x00;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_varint(self.teleport_id);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            teleport_id: src.read_varint()?,
        })
    }
}

/// `0x14` — report absolute position (we're standing still in this milestone).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SetPlayerPosition {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub on_ground: bool,
}

impl Packet for SetPlayerPosition {
    const ID: i32 = 0x14;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_f64(self.x);
        dst.put_f64(self.y);
        dst.put_f64(self.z);
        dst.put_bool(self.on_ground);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            x: src.read_f64()?,
            y: src.read_f64()?,
            z: src.read_f64()?,
            on_ground: src.read_bool()?,
        })
    }
}

/// `0x15` — report absolute position + look (used to confirm spawn).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SetPlayerPositionRotation {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
}

impl Packet for SetPlayerPositionRotation {
    const ID: i32 = 0x15;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_f64(self.x);
        dst.put_f64(self.y);
        dst.put_f64(self.z);
        dst.put_f32(self.yaw);
        dst.put_f32(self.pitch);
        dst.put_bool(self.on_ground);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            x: src.read_f64()?,
            y: src.read_f64()?,
            z: src.read_f64()?,
            yaw: src.read_f32()?,
            pitch: src.read_f32()?,
            on_ground: src.read_bool()?,
        })
    }
}

/// `0x05` — send a chat message.
///
/// Since 1.19 this packet carries "secure chat" metadata (timestamp, salt, an
/// optional 256-byte signature, and a last-seen-messages acknowledgement
/// bitset). Offline-mode servers accept unsigned messages, so
/// [`ClientChatMessage::unsigned`] fills in empty/zero values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientChatMessage {
    pub message: String,
    pub timestamp: i64,
    pub salt: i64,
    /// Fixed 256-byte signature, present only for signed (online-mode) chat.
    pub signature: Option<[u8; 256]>,
    pub message_count: i32,
    /// Acknowledged "last seen" messages — a fixed BitSet(20) => 3 bytes.
    pub acknowledged: [u8; 3],
}

impl ClientChatMessage {
    /// Builds an unsigned message suitable for offline-mode servers.
    pub fn unsigned(message: String) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        Self {
            message,
            timestamp,
            salt: 0,
            signature: None,
            message_count: 0,
            acknowledged: [0; 3],
        }
    }
}

impl Packet for ClientChatMessage {
    const ID: i32 = 0x05;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_string(&self.message);
        dst.put_i64(self.timestamp);
        dst.put_i64(self.salt);
        dst.put_bool(self.signature.is_some());
        if let Some(sig) = &self.signature {
            dst.put_slice(sig); // fixed 256 bytes, not length-prefixed
        }
        dst.put_varint(self.message_count);
        dst.put_slice(&self.acknowledged); // fixed 3 bytes
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        let message = src.read_string(256)?;
        let timestamp = src.read_i64()?;
        let salt = src.read_i64()?;
        let signature = if src.read_bool()? {
            let bytes = src.read_bytes(256)?;
            let arr: [u8; 256] = bytes
                .try_into()
                .map_err(|_| ProtoError::UnexpectedEof { needed: 0 })?;
            Some(arr)
        } else {
            None
        };
        let message_count = src.read_varint()?;
        let ack = src.read_bytes(3)?;
        let acknowledged: [u8; 3] = ack
            .try_into()
            .map_err(|_| ProtoError::UnexpectedEof { needed: 0 })?;
        Ok(Self {
            message,
            timestamp,
            salt,
            signature,
            message_count,
            acknowledged,
        })
    }
}

/// `0x04` — run a chat command (the leading `/` is stripped). 1.20.1 requires
/// commands to use this packet rather than a chat message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatCommand {
    pub command: String,
    pub timestamp: i64,
    pub salt: i64,
}

impl ChatCommand {
    /// Builds an unsigned command (offline-mode). `command` excludes the `/`.
    pub fn new(command: String) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        Self {
            command,
            timestamp,
            salt: 0,
        }
    }
}

impl Packet for ChatCommand {
    const ID: i32 = 0x04;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_string(&self.command);
        dst.put_i64(self.timestamp);
        dst.put_i64(self.salt);
        dst.put_varint(0); // no argument signatures
        dst.put_varint(0); // message count
        dst.put_slice(&[0u8; 3]); // acknowledged bitset
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        let command = src.read_string(256)?;
        let timestamp = src.read_i64()?;
        let salt = src.read_i64()?;
        let sigs = src.read_varint()?.max(0);
        for _ in 0..sigs {
            let _name = src.read_string(16)?;
            let _sig = src.read_bytes(256)?;
        }
        let _message_count = src.read_varint()?;
        let _ack = src.read_bytes(3)?;
        Ok(Self {
            command,
            timestamp,
            salt,
        })
    }
}

/// `0x08` — tell the server our client settings. Some servers expect this
/// before they treat us as fully joined.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientInformation {
    pub locale: String,
    pub view_distance: u8,
    pub chat_mode: i32,
    pub chat_colors: bool,
    pub displayed_skin_parts: u8,
    pub main_hand: i32,
    pub enable_text_filtering: bool,
    pub allow_server_listings: bool,
}

impl ClientInformation {
    /// Reasonable defaults for a headless client.
    pub fn sensible_defaults() -> Self {
        Self {
            locale: "en_us".to_string(),
            view_distance: 8,
            chat_mode: 0, // enabled
            chat_colors: true,
            displayed_skin_parts: 0x7f, // all parts
            main_hand: 1,               // right
            enable_text_filtering: false,
            allow_server_listings: true,
        }
    }
}

impl Packet for ClientInformation {
    const ID: i32 = 0x08;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_string(&self.locale);
        dst.put_u8(self.view_distance);
        dst.put_varint(self.chat_mode);
        dst.put_bool(self.chat_colors);
        dst.put_u8(self.displayed_skin_parts);
        dst.put_varint(self.main_hand);
        dst.put_bool(self.enable_text_filtering);
        dst.put_bool(self.allow_server_listings);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            locale: src.read_string(16)?,
            view_distance: src.read_u8()?,
            chat_mode: src.read_varint()?,
            chat_colors: src.read_bool()?,
            displayed_skin_parts: src.read_u8()?,
            main_hand: src.read_varint()?,
            enable_text_filtering: src.read_bool()?,
            allow_server_listings: src.read_bool()?,
        })
    }
}

/// `0x1d` — block dig/break action. `status`: 0 = start, 1 = cancel, 2 =
/// finish (creative breaks on start). `face` is the dug face (0=down..5=east).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerDigging {
    pub status: i32,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub face: i8,
    pub sequence: i32,
}

impl Packet for PlayerDigging {
    const ID: i32 = 0x1d;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_varint(self.status);
        dst.put_position(self.x, self.y, self.z);
        dst.put_i8(self.face);
        dst.put_varint(self.sequence);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        let status = src.read_varint()?;
        let (x, y, z) = src.read_position()?;
        Ok(Self {
            status,
            x,
            y,
            z,
            face: src.read_i8()?,
            sequence: src.read_varint()?,
        })
    }
}

/// `0x31` — use item on a block (place). `direction` is the clicked face
/// (0=down..5=east); `cursor` is the hit point on that face (0..1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UseItemOn {
    pub hand: i32,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub direction: i32,
    pub cursor: [f32; 3],
    pub inside_block: bool,
    pub sequence: i32,
}

impl Packet for UseItemOn {
    const ID: i32 = 0x31;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_varint(self.hand);
        dst.put_position(self.x, self.y, self.z);
        dst.put_varint(self.direction);
        dst.put_f32(self.cursor[0]);
        dst.put_f32(self.cursor[1]);
        dst.put_f32(self.cursor[2]);
        dst.put_bool(self.inside_block);
        dst.put_varint(self.sequence);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        let hand = src.read_varint()?;
        let (x, y, z) = src.read_position()?;
        Ok(Self {
            hand,
            x,
            y,
            z,
            direction: src.read_varint()?,
            cursor: [src.read_f32()?, src.read_f32()?, src.read_f32()?],
            inside_block: src.read_bool()?,
            sequence: src.read_varint()?,
        })
    }
}

/// A simple item stack (no NBT) for creative slot setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotItem {
    pub item_id: i32,
    pub count: i8,
}

/// `0x2b` — set a creative-mode inventory slot (used to hold a block to place).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetCreativeSlot {
    pub slot: i16,
    pub item: Option<SlotItem>,
}

impl Packet for SetCreativeSlot {
    const ID: i32 = 0x2b;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_i16(self.slot);
        match self.item {
            Some(item) => {
                dst.put_bool(true);
                dst.put_varint(item.item_id);
                dst.put_i8(item.count);
                dst.put_u8(0x00); // optionalNbt: none (TAG_End)
            }
            None => dst.put_bool(false),
        }
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        let slot = src.read_i16()?;
        let item = if src.read_bool()? {
            let item_id = src.read_varint()?;
            let count = src.read_i8()?;
            let _nbt = nbt::read_nbt(src)?; // optionalNbt (0x00 => End)
            Some(SlotItem { item_id, count })
        } else {
            None
        };
        Ok(Self { slot, item })
    }
}

/// `0x57` — current health, food, and saturation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SetHealth {
    pub health: f32,
    pub food: i32,
    pub saturation: f32,
}

impl Packet for SetHealth {
    const ID: i32 = 0x57;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Clientbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_f32(self.health);
        dst.put_varint(self.food);
        dst.put_f32(self.saturation);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            health: src.read_f32()?,
            food: src.read_varint()?,
            saturation: src.read_f32()?,
        })
    }
}

/// `0x07` — client status action (0 = perform respawn, 1 = request stats).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientCommand {
    pub action: i32,
}

impl Packet for ClientCommand {
    const ID: i32 = 0x07;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_varint(self.action);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            action: src.read_varint()?,
        })
    }
}

/// `0x0b` — click a slot in an open container (or the player inventory). For a
/// normal left-click swap: `mode` = 0, `button` = 0, `changed` lists the slots
/// we changed (their new contents), and `carried` is the new cursor stack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClickContainer {
    pub window_id: u8,
    pub state_id: i32,
    pub slot: i16,
    pub button: i8,
    pub mode: i32,
    pub changed: Vec<(i16, Option<SlotItem>)>,
    pub carried: Option<SlotItem>,
}

impl Packet for ClickContainer {
    const ID: i32 = 0x0b;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_u8(self.window_id);
        dst.put_varint(self.state_id);
        dst.put_i16(self.slot);
        dst.put_i8(self.button);
        dst.put_varint(self.mode);
        dst.put_varint(self.changed.len() as i32);
        for (loc, item) in &self.changed {
            dst.put_i16(*loc);
            write_slot(dst, *item);
        }
        write_slot(dst, self.carried);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        let window_id = src.read_u8()?;
        let state_id = src.read_varint()?;
        let slot = src.read_i16()?;
        let button = src.read_i8()?;
        let mode = src.read_varint()?;
        let count = src.read_varint()?.max(0) as usize;
        let mut changed = Vec::with_capacity(count.min(64));
        for _ in 0..count {
            let loc = src.read_i16()?;
            changed.push((loc, read_slot(src)?));
        }
        let carried = read_slot(src)?;
        Ok(Self {
            window_id,
            state_id,
            slot,
            button,
            mode,
            changed,
            carried,
        })
    }
}

/// `0x2f` — swing an arm (cosmetic; sent alongside an attack). `hand`: 0 = main.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwingArm {
    pub hand: i32,
}

impl Packet for SwingArm {
    const ID: i32 = 0x2f;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_varint(self.hand);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            hand: src.read_varint()?,
        })
    }
}

/// `0x28` — select a hotbar slot (0..=8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetHeldItem {
    pub slot: i16,
}

/// `0x0a` — selects one of the three enchanting-table offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnchantItem {
    pub window_id: i8,
    pub enchantment: i8,
}

/// `0x0a` — presses a numeric button in the current menu. Enchanting offers,
/// loom patterns, and stonecutter recipes all use this packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContainerButtonClick {
    pub window_id: i8,
    pub button_id: i8,
}

/// `0x23` — updates the text field in an open anvil menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenameItem {
    pub name: String,
}

/// `0x24` — reports progress for a server-requested resource pack. Vanilla
/// status ids: 0 loaded, 1 declined, 2 failed download, 3 accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourcePackStatus {
    pub status: i32,
}

impl Packet for ResourcePackStatus {
    const ID: i32 = 0x24;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_varint(self.status);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            status: src.read_varint()?,
        })
    }
}

impl Packet for RenameItem {
    const ID: i32 = 0x23;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_string(&self.name);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            name: src.read_string(50)?,
        })
    }
}

impl Packet for EnchantItem {
    const ID: i32 = 0x0a;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_i8(self.window_id);
        dst.put_i8(self.enchantment);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            window_id: src.read_i8()?,
            enchantment: src.read_i8()?,
        })
    }
}

impl Packet for ContainerButtonClick {
    const ID: i32 = 0x0a;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_i8(self.window_id);
        dst.put_i8(self.button_id);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            window_id: src.read_i8()?,
            button_id: src.read_i8()?,
        })
    }
}

impl Packet for SetHeldItem {
    const ID: i32 = 0x28;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_i16(self.slot);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            slot: src.read_i16()?,
        })
    }
}

/// How the player is interacting with an entity in [`InteractEntity`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Interaction {
    /// Right-click interact (e.g. mount, trade). `hand`: 0 = main.
    Interact { hand: i32 },
    /// Left-click attack.
    Attack,
    /// Right-click at a precise point on the entity. `hand`: 0 = main.
    InteractAt { x: f32, y: f32, z: f32, hand: i32 },
}

/// `0x10` — interact with (or attack) an entity by id.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InteractEntity {
    pub target: i32,
    pub interaction: Interaction,
    pub sneaking: bool,
}

impl Packet for InteractEntity {
    const ID: i32 = 0x10;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_varint(self.target);
        match self.interaction {
            Interaction::Interact { hand } => {
                dst.put_varint(0);
                dst.put_varint(hand);
            }
            Interaction::Attack => dst.put_varint(1),
            Interaction::InteractAt { x, y, z, hand } => {
                dst.put_varint(2);
                dst.put_f32(x);
                dst.put_f32(y);
                dst.put_f32(z);
                dst.put_varint(hand);
            }
        }
        dst.put_bool(self.sneaking);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        let target = src.read_varint()?;
        let interaction = match src.read_varint()? {
            0 => Interaction::Interact {
                hand: src.read_varint()?,
            },
            1 => Interaction::Attack,
            2 => Interaction::InteractAt {
                x: src.read_f32()?,
                y: src.read_f32()?,
                z: src.read_f32()?,
                hand: src.read_varint()?,
            },
            other => {
                return Err(ProtoError::InvalidEnum {
                    type_name: "InteractEntity.type",
                    value: i64::from(other),
                })
            }
        };
        Ok(Self {
            target,
            interaction,
            sneaking: src.read_bool()?,
        })
    }
}

/// Writes an inventory `Slot`: present flag, then id/count/(empty NBT).
fn write_slot<B: BufMut>(dst: &mut B, item: Option<SlotItem>) {
    match item {
        Some(it) => {
            dst.put_bool(true);
            dst.put_varint(it.item_id);
            dst.put_i8(it.count);
            dst.put_u8(0x00); // optionalNbt: none (TAG_End)
        }
        None => dst.put_bool(false),
    }
}

/// Reads an inventory `Slot` (any item NBT is parsed and discarded).
fn read_slot<B: Buf>(src: &mut B) -> Result<Option<SlotItem>, ProtoError> {
    if src.read_bool()? {
        let item_id = src.read_varint()?;
        let count = src.read_i8()?;
        let _nbt = nbt::read_nbt(src)?;
        Ok(Some(SlotItem { item_id, count }))
    } else {
        Ok(None)
    }
}

/// `0x12` — the full contents of an open window (window 0 = player inventory).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetContainerContent {
    pub window_id: u8,
    pub state_id: i32,
    pub slots: Vec<Option<SlotItem>>,
    pub carried: Option<SlotItem>,
}

impl Packet for SetContainerContent {
    const ID: i32 = 0x12;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Clientbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_u8(self.window_id);
        dst.put_varint(self.state_id);
        dst.put_varint(self.slots.len() as i32);
        for slot in &self.slots {
            write_slot(dst, *slot);
        }
        write_slot(dst, self.carried);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        let window_id = src.read_u8()?;
        let state_id = src.read_varint()?;
        let count = src.read_varint()?.max(0) as usize;
        let mut slots = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            slots.push(read_slot(src)?);
        }
        let carried = read_slot(src)?;
        Ok(Self {
            window_id,
            state_id,
            slots,
            carried,
        })
    }
}

/// `0x14` — a single slot update within a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetContainerSlot {
    pub window_id: i8,
    pub state_id: i32,
    pub slot: i16,
    pub item: Option<SlotItem>,
}

impl Packet for SetContainerSlot {
    const ID: i32 = 0x14;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Clientbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_i8(self.window_id);
        dst.put_varint(self.state_id);
        dst.put_i16(self.slot);
        write_slot(dst, self.item);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            window_id: src.read_i8()?,
            state_id: src.read_varint()?,
            slot: src.read_i16()?,
            item: read_slot(src)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip<P: Packet + PartialEq + std::fmt::Debug>(pkt: &P) {
        let mut buf = Vec::new();
        pkt.encode(&mut buf).unwrap();
        let mut slice: &[u8] = &buf;
        let decoded = P::decode(&mut slice).unwrap();
        assert_eq!(&decoded, pkt);
        assert_eq!(slice.remaining(), 0, "decoder must consume the whole body");
    }

    #[test]
    fn keepalive_roundtrips() {
        roundtrip(&KeepAlive { id: -42 });
        roundtrip(&KeepAliveResponse { id: 9_000_000_000 });
        roundtrip(&Ping { id: -123 });
        roundtrip(&Pong { id: -123 });
    }

    #[test]
    fn sync_position_roundtrips() {
        roundtrip(&SynchronizePlayerPosition {
            x: 1.5,
            y: -60.0,
            z: 2048.25,
            yaw: 90.0,
            pitch: -12.5,
            flags: 0b0001_1010,
            teleport_id: 7,
        });
    }

    #[test]
    fn movement_packets_roundtrip() {
        roundtrip(&ConfirmTeleport { teleport_id: 7 });
        roundtrip(&SetPlayerPosition {
            x: 1.0,
            y: 2.0,
            z: 3.0,
            on_ground: true,
        });
        roundtrip(&SetPlayerPositionRotation {
            x: 1.0,
            y: 2.0,
            z: 3.0,
            yaw: 45.0,
            pitch: 0.0,
            on_ground: false,
        });
    }

    #[test]
    fn vehicle_control_packets_roundtrip() {
        roundtrip(&VehicleMove {
            x: 12.5,
            y: 63.0,
            z: -4.25,
            yaw: 135.0,
            pitch: 0.0,
        });
        roundtrip(&SteerBoat {
            left_paddle: true,
            right_paddle: false,
        });
        roundtrip(&SteerVehicle {
            sideways: -1.0,
            forward: 0.75,
            flags: 1,
        });
    }

    #[test]
    fn system_chat_roundtrips() {
        roundtrip(&SystemChat {
            content: r#"{"text":"Ferris joined the game","color":"yellow"}"#.into(),
            overlay: false,
        });
    }

    #[test]
    fn chat_command_roundtrips() {
        roundtrip(&ChatCommand {
            command: "time set day".into(),
            timestamp: 123,
            salt: 7,
        });
    }

    #[test]
    fn unsigned_chat_roundtrips() {
        let pkt = ClientChatMessage::unsigned("hello world".into());
        assert!(pkt.signature.is_none());
        roundtrip(&pkt);
    }

    #[test]
    fn signed_chat_roundtrips() {
        let pkt = ClientChatMessage {
            message: "signed".into(),
            timestamp: 123,
            salt: 456,
            signature: Some([0xab; 256]),
            message_count: 3,
            acknowledged: [0xff, 0x0f, 0x00],
        };
        roundtrip(&pkt);
    }

    #[test]
    fn client_information_roundtrips() {
        roundtrip(&ClientInformation::sensible_defaults());
    }

    #[test]
    fn player_digging_roundtrips() {
        roundtrip(&PlayerDigging {
            status: 0,
            x: 10,
            y: -60,
            z: -7,
            face: 1,
            sequence: 42,
        });
    }

    #[test]
    fn use_item_on_roundtrips() {
        roundtrip(&UseItemOn {
            hand: 0,
            x: 1,
            y: 2,
            z: 3,
            direction: 1,
            cursor: [0.5, 0.5, 0.5],
            inside_block: false,
            sequence: 7,
        });
        roundtrip(&EditBook {
            slot: 4,
            pages: vec!["first page".to_string(), "second page".to_string()],
            title: Some("Crabcraft".to_string()),
        });
        roundtrip(&PlaceRecipe {
            window_id: 2,
            recipe: "minecraft:oak_planks".to_string(),
            make_all: true,
        });
    }

    #[test]
    fn set_health_and_client_command_roundtrip() {
        roundtrip(&SetHealth {
            health: 12.5,
            food: 17,
            saturation: 4.0,
        });
        roundtrip(&ClientCommand { action: 0 });
    }

    #[test]
    fn set_held_item_roundtrips() {
        roundtrip(&SetHeldItem { slot: 4 });
        roundtrip(&EnchantItem {
            window_id: 3,
            enchantment: 2,
        });
        roundtrip(&ContainerButtonClick {
            window_id: 4,
            button_id: 17,
        });
        roundtrip(&RenameItem {
            name: "Ferris's Pickaxe".into(),
        });
        roundtrip(&ResourcePackStatus { status: 3 });
    }

    #[test]
    fn click_container_roundtrips() {
        roundtrip(&ClickContainer {
            window_id: 0,
            state_id: 5,
            slot: 36,
            button: 0,
            mode: 0,
            changed: vec![
                (36, None),
                (
                    9,
                    Some(SlotItem {
                        item_id: 1,
                        count: 64,
                    }),
                ),
            ],
            carried: Some(SlotItem {
                item_id: 764,
                count: 1,
            }),
        });
    }

    #[test]
    fn combat_packets_roundtrip() {
        roundtrip(&SwingArm { hand: 0 });
        roundtrip(&InteractEntity {
            target: 42,
            interaction: Interaction::Attack,
            sneaking: false,
        });
        roundtrip(&InteractEntity {
            target: 7,
            interaction: Interaction::Interact { hand: 1 },
            sneaking: true,
        });
        roundtrip(&InteractEntity {
            target: 99,
            interaction: Interaction::InteractAt {
                x: 0.1,
                y: 0.9,
                z: -0.3,
                hand: 0,
            },
            sneaking: false,
        });
    }

    #[test]
    fn inventory_packets_roundtrip() {
        roundtrip(&SetContainerContent {
            window_id: 0,
            state_id: 1,
            slots: vec![
                None,
                Some(SlotItem {
                    item_id: 764,
                    count: 5,
                }),
                None,
            ],
            carried: None,
        });
        roundtrip(&SetContainerSlot {
            window_id: 0,
            state_id: 2,
            slot: 36,
            item: Some(SlotItem {
                item_id: 1,
                count: 64,
            }),
        });
        roundtrip(&SetContainerSlot {
            window_id: -1,
            state_id: 0,
            slot: -1,
            item: None,
        });
    }

    #[test]
    fn container_lifecycle_packets_roundtrip() {
        roundtrip(&OpenScreen {
            window_id: 4,
            menu_type: 2,
            title: r#"{"translate":"container.chest"}"#.to_string(),
        });
        roundtrip(&ClientboundCloseContainer { window_id: 4 });
        roundtrip(&CloseContainer { window_id: 4 });
        roundtrip(&SetContainerData {
            window_id: 4,
            property: 2,
            value: 117,
        });
        roundtrip(&PlayerCommand {
            entity_id: 42,
            action: 3,
            jump_boost: 0,
        });
        roundtrip(&UseItem {
            hand: 0,
            sequence: 19,
        });
        roundtrip(&ClientboundPlayerAbilities {
            flags: 0x06,
            flying_speed: 0.05,
            walking_speed: 0.1,
        });
        roundtrip(&ServerboundPlayerAbilities { flags: 0x02 });
    }

    #[test]
    fn set_creative_slot_roundtrips() {
        roundtrip(&SetCreativeSlot {
            slot: 36,
            item: Some(SlotItem {
                item_id: 1,
                count: 64,
            }),
        });
        roundtrip(&SetCreativeSlot {
            slot: 36,
            item: None,
        });
    }
}
