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

/// `0x23` — server pings; we must echo the same id back promptly or get kicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeepAlive {
    pub id: i64,
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
    fn system_chat_roundtrips() {
        roundtrip(&SystemChat {
            content: r#"{"text":"Ferris joined the game","color":"yellow"}"#.into(),
            overlay: false,
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
