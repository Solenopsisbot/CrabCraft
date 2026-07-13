//! Play-state payloads whose IDs changed in protocol 770.

use bytes::{Buf, BufMut};
use std::collections::HashMap;

use crate::error::ProtoError;
use crate::io::{BufExt, BufMutExt};
use crate::nbt::{self, Nbt};
use crate::packet::{Bound, Packet, State};
use crate::versions::v1_20_1::play::SlotItem;
use crate::versions::v1_20_5::play::{
    bounded_count, skip_block_predicate, skip_consume_effect_768, skip_effect_detail,
    skip_firework_explosion, skip_holder_set, skip_inline_trim_material, skip_inline_trim_pattern,
    skip_optional, skip_registry_holder, skip_sound_holder, skip_string, ComponentSlot,
};

const MAX_COMPONENTS: i32 = 256;

/// Decodes a protocol 770 item stack and consumes its complete component patch.
///
/// 1.21.5 centralised tooltip visibility, added weapon/blocking and entity
/// variant components, and changed several existing payloads. Keeping this
/// decoder separate prevents those layouts from being guessed from 1.21.4.
pub fn read_component_slot<B: Buf>(src: &mut B) -> Result<ComponentSlot, ProtoError> {
    let count = src.read_varint()?;
    if count == 0 {
        return Ok(ComponentSlot {
            item: None,
            metadata: None,
        });
    }
    if !(1..=127).contains(&count) {
        return Err(ProtoError::InvalidEnum {
            type_name: "protocol 770 item stack count",
            value: i64::from(count),
        });
    }
    let item_id = src.read_varint()?;
    let added = src.read_varint()?;
    let removed = src.read_varint()?;
    if !(0..=MAX_COMPONENTS).contains(&added) || !(0..=MAX_COMPONENTS).contains(&removed) {
        return Err(ProtoError::InvalidEnum {
            type_name: "protocol 770 item component count",
            value: i64::from(added.max(removed)),
        });
    }

    let mut values = HashMap::new();
    for _ in 0..added {
        let kind = src.read_varint()?;
        skip_component(src, kind, &mut values)?;
    }
    for _ in 0..removed {
        let kind = src.read_varint()?;
        if !(0..=95).contains(&kind) {
            return Err(ProtoError::InvalidEnum {
                type_name: "protocol 770 removed item component type",
                value: i64::from(kind),
            });
        }
    }

    Ok(ComponentSlot {
        item: Some(SlotItem {
            item_id,
            count: i8::try_from(count).map_err(|_| ProtoError::InvalidEnum {
                type_name: "protocol 770 item stack count",
                value: i64::from(count),
            })?,
        }),
        metadata: (!values.is_empty()).then_some(Nbt::Compound(values)),
    })
}

fn skip_component<B: Buf>(
    src: &mut B,
    kind: i32,
    metadata: &mut HashMap<String, Nbt>,
) -> Result<(), ProtoError> {
    match kind {
        0 => {
            if let Nbt::Compound(values) = nbt::read_anonymous_nbt(src)? {
                metadata.extend(values);
            }
        }
        1..=3 | 9 | 16 | 27 | 39 | 54 | 64 | 72..=88 | 90..=95 => {
            let value = src.read_varint()?;
            if kind == 37 {
                metadata.insert("map".into(), Nbt::Int(value));
            }
        }
        4 | 17 | 19 | 30 => {}
        5 | 6 => {
            let value = nbt::read_anonymous_nbt(src)?;
            metadata.insert(
                if kind == 5 {
                    "custom_name"
                } else {
                    "item_name"
                }
                .into(),
                value,
            );
        }
        7 | 24 | 31 | 56 | 62 => skip_string(src)?,
        8 => {
            for _ in 0..bounded_count(src, "protocol 770 lore line count")? {
                let _ = nbt::read_anonymous_nbt(src)?;
            }
        }
        10 | 34 => {
            for _ in 0..bounded_count(src, "protocol 770 enchantment count")? {
                let _ = src.read_varint()?;
                let _ = src.read_varint()?;
            }
        }
        11 | 12 => {
            for _ in 0..bounded_count(src, "protocol 770 block predicate count")? {
                skip_block_predicate(src)?;
            }
        }
        13 => {
            for _ in 0..bounded_count(src, "protocol 770 attribute count")? {
                let _ = src.read_varint()?;
                skip_string(src)?;
                let _ = src.read_f64()?;
                let _ = src.read_varint()?;
                let _ = src.read_varint()?;
            }
        }
        14 => {
            for _ in 0..bounded_count(src, "protocol 770 custom model float count")? {
                let _ = src.read_f32()?;
            }
            for _ in 0..bounded_count(src, "protocol 770 custom model flag count")? {
                let _ = src.read_bool()?;
            }
            for _ in 0..bounded_count(src, "protocol 770 custom model string count")? {
                skip_string(src)?;
            }
            for _ in 0..bounded_count(src, "protocol 770 custom model color count")? {
                let _ = src.read_i32()?;
            }
        }
        15 => {
            let _ = src.read_bool()?;
            for _ in 0..bounded_count(src, "protocol 770 hidden component count")? {
                let component = src.read_varint()?;
                if !(0..=95).contains(&component) {
                    return Err(ProtoError::InvalidEnum {
                        type_name: "protocol 770 hidden item component type",
                        value: i64::from(component),
                    });
                }
            }
        }
        18 => {
            let _ = src.read_bool()?;
        }
        20 => {
            let _ = src.read_varint()?;
            let _ = src.read_f32()?;
            let _ = src.read_bool()?;
        }
        21 => {
            let _ = src.read_f32()?;
            let _ = src.read_varint()?;
            skip_sound_holder(src)?;
            let _ = src.read_bool()?;
            for _ in 0..bounded_count(src, "protocol 770 consume effect count")? {
                skip_consume_effect_768(src)?;
            }
        }
        22 => {
            let _ = read_component_slot(src)?;
        }
        23 => {
            let _ = src.read_f32()?;
            skip_optional(src, skip_string)?;
        }
        25 => {
            for _ in 0..bounded_count(src, "protocol 770 tool rule count")? {
                skip_holder_set(src)?;
                skip_optional(src, |src| {
                    let _ = src.read_f32()?;
                    Ok(())
                })?;
                skip_optional(src, |src| {
                    let _ = src.read_bool()?;
                    Ok(())
                })?;
            }
            let _ = src.read_f32()?;
            let _ = src.read_varint()?;
            let _ = src.read_bool()?;
        }
        26 => {
            let _ = src.read_varint()?;
            let _ = src.read_f32()?;
        }
        28 => {
            let _ = src.read_varint()?;
            skip_sound_holder(src)?;
            skip_optional(src, skip_string)?;
            skip_optional(src, skip_string)?;
            skip_optional(src, skip_holder_set)?;
            let _ = src.read_bool()?;
            let _ = src.read_bool()?;
            let _ = src.read_bool()?;
            let _ = src.read_bool()?;
        }
        29 => skip_holder_set(src)?,
        32 => {
            for _ in 0..bounded_count(src, "protocol 770 death protection effect count")? {
                skip_consume_effect_768(src)?;
            }
        }
        33 => skip_blocks_attacks(src)?,
        35 | 36 => {
            let _ = src.read_i32()?;
        }
        37 => {
            let value = src.read_varint()?;
            metadata.insert("map".into(), Nbt::Int(value));
        }
        38 | 48..=51 | 57 | 69 | 70 => {
            let value = nbt::read_anonymous_nbt(src)?;
            if kind == 51 {
                metadata.insert("BlockEntityTag".into(), value);
            }
        }
        40 | 66 => {
            for _ in 0..bounded_count(src, "protocol 770 nested item count")? {
                let _ = read_component_slot(src)?;
            }
        }
        41 => {
            let mut contents = Vec::new();
            for _ in 0..bounded_count(src, "protocol 770 bundle item count")? {
                let nested = read_component_slot(src)?;
                if let Some(item) = nested.item {
                    let mut entry = HashMap::new();
                    entry.insert("id".to_string(), Nbt::Int(item.item_id));
                    entry.insert("count".to_string(), Nbt::Byte(item.count));
                    contents.push(Nbt::Compound(entry));
                }
            }
            metadata.insert("bundle_contents".to_string(), Nbt::List(contents));
        }
        42 => {
            skip_optional(src, |src| {
                let _ = src.read_varint()?;
                Ok(())
            })?;
            skip_optional(src, |src| {
                let _ = src.read_i32()?;
                Ok(())
            })?;
            for _ in 0..bounded_count(src, "protocol 770 potion effect count")? {
                let _ = src.read_varint()?;
                skip_effect_detail(src, 0)?;
            }
            skip_optional(src, skip_string)?;
        }
        43 => {
            let _ = src.read_f32()?;
        }
        44 => {
            for _ in 0..bounded_count(src, "protocol 770 stew effect count")? {
                let _ = src.read_varint()?;
                let _ = src.read_varint()?;
            }
        }
        45 => {
            for _ in 0..bounded_count(src, "protocol 770 book page count")? {
                skip_string(src)?;
                skip_optional(src, skip_string)?;
            }
        }
        46 => {
            skip_string(src)?;
            skip_optional(src, skip_string)?;
            skip_string(src)?;
            let _ = src.read_varint()?;
            for _ in 0..bounded_count(src, "protocol 770 written book page count")? {
                let _ = nbt::read_anonymous_nbt(src)?;
                let _ = nbt::read_anonymous_nbt(src)?;
            }
            let _ = src.read_bool()?;
        }
        47 => {
            skip_registry_holder(src, skip_inline_trim_material)?;
            skip_registry_holder(src, skip_inline_trim_pattern)?;
        }
        52 => skip_inline_or_named_holder(src, |src| {
            skip_registry_holder(src, |src| {
                skip_sound_holder(src)?;
                let _ = src.read_f32()?;
                let _ = src.read_f32()?;
                let _ = nbt::read_anonymous_nbt(src)?;
                Ok(())
            })
        })?,
        53 => skip_inline_or_named_holder(src, |src| {
            skip_registry_holder(src, skip_inline_trim_material)
        })?,
        55 => skip_inline_or_named_holder(src, |src| {
            skip_registry_holder(src, |src| {
                skip_sound_holder(src)?;
                let _ = nbt::read_anonymous_nbt(src)?;
                let _ = src.read_f32()?;
                let _ = src.read_varint()?;
                Ok(())
            })
        })?,
        58 => {
            skip_optional(src, |src| {
                skip_string(src)?;
                let _ = src.read_position()?;
                Ok(())
            })?;
            let _ = src.read_bool()?;
        }
        59 => skip_firework_explosion(src)?,
        60 => {
            let _ = src.read_varint()?;
            for _ in 0..bounded_count(src, "protocol 770 firework explosion count")? {
                skip_firework_explosion(src)?;
            }
        }
        61 => {
            skip_optional(src, skip_string)?;
            skip_optional(src, |src| {
                let _ = src.read_uuid()?;
                Ok(())
            })?;
            for _ in 0..bounded_count(src, "protocol 770 profile property count")? {
                skip_string(src)?;
                skip_string(src)?;
                skip_optional(src, skip_string)?;
            }
        }
        63 => {
            for _ in 0..bounded_count(src, "protocol 770 banner layer count")? {
                skip_registry_holder(src, |src| {
                    skip_string(src)?;
                    skip_string(src)
                })?;
                let _ = src.read_varint()?;
            }
        }
        65 => {
            for _ in 0..bounded_count(src, "protocol 770 pot decoration count")? {
                let _ = src.read_varint()?;
            }
        }
        67 => {
            for _ in 0..bounded_count(src, "protocol 770 block state property count")? {
                skip_string(src)?;
                skip_string(src)?;
            }
        }
        68 => {
            for _ in 0..bounded_count(src, "protocol 770 bee count")? {
                let _ = nbt::read_anonymous_nbt(src)?;
                let _ = src.read_varint()?;
                let _ = src.read_varint()?;
            }
        }
        71 => skip_sound_holder(src)?,
        89 => skip_registry_holder(src, |src| {
            let _ = src.read_i32()?;
            let _ = src.read_i32()?;
            skip_string(src)?;
            skip_optional(src, |src| {
                let _ = nbt::read_anonymous_nbt(src)?;
                Ok(())
            })?;
            skip_optional(src, |src| {
                let _ = nbt::read_anonymous_nbt(src)?;
                Ok(())
            })
        })?,
        _ => {
            return Err(ProtoError::InvalidEnum {
                type_name: "protocol 770 item component type",
                value: i64::from(kind),
            })
        }
    }
    Ok(())
}

fn skip_inline_or_named_holder<B: Buf>(
    src: &mut B,
    holder: impl FnOnce(&mut B) -> Result<(), ProtoError>,
) -> Result<(), ProtoError> {
    if src.read_bool()? {
        holder(src)
    } else {
        skip_string(src)
    }
}

fn skip_blocks_attacks<B: Buf>(src: &mut B) -> Result<(), ProtoError> {
    let _ = src.read_f32()?;
    let _ = src.read_f32()?;
    for _ in 0..bounded_count(src, "protocol 770 damage reduction count")? {
        let _ = src.read_f32()?;
        skip_optional(src, skip_holder_set)?;
        let _ = src.read_f32()?;
        let _ = src.read_f32()?;
    }
    let _ = src.read_f32()?;
    let _ = src.read_f32()?;
    let _ = src.read_f32()?;
    skip_optional(src, skip_string)?;
    skip_optional(src, skip_sound_holder)?;
    skip_optional(src, skip_sound_holder)
}

/// `0x07` — sends a chat message with the protocol 770 checksum trailer.
///
/// Protocol 770 retains the protocol 763 secure-chat fields and appends a
/// single checksum byte. Unsigned offline-mode messages use a zero checksum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientChatMessage {
    pub inner: crate::versions::v1_20_1::play::ClientChatMessage,
    pub checksum: u8,
}

impl ClientChatMessage {
    /// Builds an unsigned message suitable for offline-mode servers.
    pub fn unsigned(message: String) -> Self {
        Self {
            inner: crate::versions::v1_20_1::play::ClientChatMessage::unsigned(message),
            checksum: 0,
        }
    }
}

impl Packet for ClientChatMessage {
    const ID: i32 = 0x07;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        self.inner.encode(dst)?;
        dst.put_u8(self.checksum);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            inner: crate::versions::v1_20_1::play::ClientChatMessage::decode(src)?,
            checksum: src.read_u8()?,
        })
    }
}

/// `0x3f` — uses the held item with the current view rotation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UseItem {
    pub hand: i32,
    pub sequence: i32,
    pub yaw: f32,
    pub pitch: f32,
}

impl Packet for UseItem {
    const ID: i32 = 0x3f;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_varint(self.hand);
        dst.put_varint(self.sequence);
        dst.put_f32(self.yaw);
        dst.put_f32(self.pitch);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            hand: src.read_varint()?,
            sequence: src.read_varint()?,
            yaw: src.read_f32()?,
            pitch: src.read_f32()?,
        })
    }
}

/// `0x3e` — uses an item on a targeted block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UseItemOn {
    pub hand: i32,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub direction: i32,
    pub cursor: [f32; 3],
    pub inside_block: bool,
    pub world_border_hit: bool,
    pub sequence: i32,
}

impl Packet for UseItemOn {
    const ID: i32 = 0x3e;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_varint(self.hand);
        dst.put_position(self.x, self.y, self.z);
        dst.put_varint(self.direction);
        for cursor in self.cursor {
            dst.put_f32(cursor);
        }
        dst.put_bool(self.inside_block);
        dst.put_bool(self.world_border_hit);
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
            world_border_hit: src.read_bool()?,
            sequence: src.read_varint()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_770_component_changes_preserve_slot_alignment() {
        let mut bytes = Vec::new();
        bytes.put_varint(1); // stack count
        bytes.put_varint(1155); // mace
        bytes.put_varint(8); // added component count
        bytes.put_varint(0); // removed component count
        bytes.put_varint(4); // unbreakable is now void
        bytes.put_varint(10); // enchantments no longer carries show-in-tooltip
        bytes.put_varint(1);
        bytes.put_varint(2);
        bytes.put_varint(4);
        bytes.put_varint(15); // central tooltip display
        bytes.put_bool(false);
        bytes.put_varint(2);
        bytes.put_varint(10);
        bytes.put_varint(33);
        bytes.put_varint(25); // tool gained creative-destruction flag
        bytes.put_varint(0);
        bytes.put_f32(1.0);
        bytes.put_varint(1);
        bytes.put_bool(true);
        bytes.put_varint(26); // weapon
        bytes.put_varint(2);
        bytes.put_f32(0.5);
        bytes.put_varint(33); // blocks_attacks
        bytes.put_f32(0.2);
        bytes.put_f32(1.0);
        bytes.put_varint(1);
        bytes.put_f32(90.0);
        bytes.put_bool(true);
        bytes.put_varint(2); // one direct ID in an IDSet
        bytes.put_varint(7);
        bytes.put_f32(0.0);
        bytes.put_f32(1.0);
        bytes.put_f32(3.0);
        bytes.put_f32(1.0);
        bytes.put_f32(0.25);
        bytes.put_bool(true);
        bytes.put_string("minecraft:bypasses_shield");
        bytes.put_bool(false);
        bytes.put_bool(false);
        bytes.put_varint(37); // map id
        bytes.put_varint(19);
        bytes.put_varint(89); // painting variant holder
        bytes.put_varint(1); // registry reference
        bytes.put_u8(0xaa);

        let mut input = bytes.as_slice();
        let decoded = read_component_slot(&mut input).unwrap();
        assert_eq!(decoded.item.unwrap().item_id, 1155);
        assert_eq!(decoded.metadata.unwrap().get("map"), Some(&Nbt::Int(19)));
        assert_eq!(input.read_u8().unwrap(), 0xaa);
    }

    #[test]
    fn protocol_770_interaction_packets_roundtrip() {
        let packet = ClientChatMessage::unsigned("hello from 1.21.5".to_string());
        let mut bytes = Vec::new();
        packet.encode(&mut bytes).unwrap();
        assert_eq!(
            packet,
            ClientChatMessage::decode(&mut bytes.as_slice()).unwrap()
        );
        assert_eq!(bytes.last(), Some(&0));

        let packet = UseItem {
            hand: 0,
            sequence: 9,
            yaw: 35.0,
            pitch: -11.0,
        };
        bytes.clear();
        packet.encode(&mut bytes).unwrap();
        assert_eq!(packet, UseItem::decode(&mut bytes.as_slice()).unwrap());

        let packet = UseItemOn {
            hand: 1,
            x: -3,
            y: 70,
            z: 8,
            direction: 2,
            cursor: [0.25, 0.5, 0.75],
            inside_block: false,
            world_border_hit: true,
            sequence: 10,
        };
        bytes.clear();
        packet.encode(&mut bytes).unwrap();
        assert_eq!(packet, UseItemOn::decode(&mut bytes.as_slice()).unwrap());
    }
}
