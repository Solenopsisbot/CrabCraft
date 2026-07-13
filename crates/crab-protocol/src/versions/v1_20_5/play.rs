use bytes::{Buf, BufMut};
use std::collections::HashMap;
use uuid::Uuid;

use crate::error::ProtoError;
use crate::io::{BufExt, BufMutExt};
use crate::nbt::{self, Nbt};
use crate::packet::{Bound, Packet, State};
use crate::versions::v1_20_1::play::SlotItem;

const MAX_COMPONENTS: i32 = 256;
const MAX_COMPONENT_ENTRIES: i32 = 65_536;

/// A 1.20.5+ item stack and the data components useful to the client UI.
#[derive(Clone, Debug, PartialEq)]
pub struct ComponentSlot {
    pub item: Option<SlotItem>,
    /// Selected component values represented as a compound for the existing
    /// item-tooltip, map, book, and custom-model-data consumers.
    pub metadata: Option<Nbt>,
}

fn bounded_count<B: Buf>(src: &mut B, type_name: &'static str) -> Result<usize, ProtoError> {
    let count = src.read_varint()?;
    if !(0..=MAX_COMPONENT_ENTRIES).contains(&count) {
        return Err(ProtoError::InvalidEnum {
            type_name,
            value: i64::from(count),
        });
    }
    Ok(count as usize)
}

fn skip_string<B: Buf>(src: &mut B) -> Result<(), ProtoError> {
    let _ = src.read_string(32_767)?;
    Ok(())
}

fn skip_optional<B: Buf>(
    src: &mut B,
    f: impl FnOnce(&mut B) -> Result<(), ProtoError>,
) -> Result<(), ProtoError> {
    if src.read_bool()? {
        f(src)?;
    }
    Ok(())
}

fn skip_holder_set<B: Buf>(src: &mut B) -> Result<(), ProtoError> {
    let count = src.read_varint()?;
    if count == 0 {
        skip_string(src)
    } else if (1..=MAX_COMPONENT_ENTRIES + 1).contains(&count) {
        for _ in 0..count - 1 {
            let _ = src.read_varint()?;
        }
        Ok(())
    } else {
        Err(ProtoError::InvalidEnum {
            type_name: "component holder-set size",
            value: i64::from(count),
        })
    }
}

fn skip_block_predicate<B: Buf>(src: &mut B) -> Result<(), ProtoError> {
    skip_optional(src, skip_holder_set)?;
    skip_optional(src, |src| {
        for _ in 0..bounded_count(src, "block predicate property count")? {
            skip_string(src)?;
            if src.read_bool()? {
                skip_string(src)?;
            } else {
                skip_string(src)?;
                skip_string(src)?;
            }
        }
        Ok(())
    })?;
    let _ = nbt::read_anonymous_nbt(src)?;
    Ok(())
}

fn skip_effect_detail<B: Buf>(src: &mut B, depth: usize) -> Result<(), ProtoError> {
    if depth > 32 {
        return Err(ProtoError::NbtTooDeep);
    }
    let _ = src.read_varint()?;
    let _ = src.read_varint()?;
    let _ = src.read_bool()?;
    let _ = src.read_bool()?;
    let _ = src.read_bool()?;
    if src.read_bool()? {
        skip_effect_detail(src, depth + 1)?;
    }
    Ok(())
}

fn skip_firework_explosion<B: Buf>(src: &mut B) -> Result<(), ProtoError> {
    let _ = src.read_varint()?;
    for _ in 0..bounded_count(src, "firework color count")? {
        let _ = src.read_i32()?;
    }
    for _ in 0..bounded_count(src, "firework fade color count")? {
        let _ = src.read_i32()?;
    }
    let _ = src.read_bool()?;
    let _ = src.read_bool()?;
    Ok(())
}

fn skip_inline_trim_material<B: Buf>(src: &mut B) -> Result<(), ProtoError> {
    skip_string(src)?;
    let _ = src.read_varint()?;
    for _ in 0..bounded_count(src, "trim override count")? {
        skip_string(src)?;
        skip_string(src)?;
    }
    let _ = nbt::read_anonymous_nbt(src)?;
    Ok(())
}

fn skip_inline_trim_pattern<B: Buf>(src: &mut B) -> Result<(), ProtoError> {
    skip_string(src)?;
    let _ = src.read_varint()?;
    let _ = nbt::read_anonymous_nbt(src)?;
    let _ = src.read_bool()?;
    Ok(())
}

fn skip_registry_holder<B: Buf>(
    src: &mut B,
    inline: impl FnOnce(&mut B) -> Result<(), ProtoError>,
) -> Result<(), ProtoError> {
    if src.read_varint()? == 0 {
        inline(src)?;
    }
    Ok(())
}

fn skip_sound_holder<B: Buf>(src: &mut B) -> Result<(), ProtoError> {
    skip_registry_holder(src, |src| {
        skip_string(src)?;
        skip_optional(src, |src| {
            let _ = src.read_f32()?;
            Ok(())
        })
    })
}

fn skip_component<B: Buf>(
    src: &mut B,
    kind: i32,
    metadata: &mut HashMap<String, Nbt>,
    protocol_767: bool,
) -> Result<(), ProtoError> {
    if protocol_767 && kind == 42 {
        if src.read_bool()? {
            skip_registry_holder(src, |src| {
                skip_sound_holder(src)?;
                let _ = nbt::read_anonymous_nbt(src)?;
                let _ = src.read_f32()?;
                let _ = src.read_varint()?;
                Ok(())
            })?;
        } else {
            skip_string(src)?;
        }
        let _ = src.read_bool()?;
        return Ok(());
    }
    let kind = if protocol_767 && kind >= 43 {
        kind - 1
    } else {
        kind
    };
    match kind {
        0 => {
            if let Nbt::Compound(values) = nbt::read_anonymous_nbt(src)? {
                metadata.extend(values);
            }
        }
        1..=3 | 8 | 13 | 16 | 26 | 28 | 41 | 49 => {
            let value = src.read_varint()?;
            if kind == 26 {
                metadata.insert("map".into(), Nbt::Int(value));
            }
        }
        4 | 18 => {
            let _ = src.read_bool()?;
        }
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
        7 => {
            for _ in 0..bounded_count(src, "lore line count")? {
                let _ = nbt::read_anonymous_nbt(src)?;
            }
        }
        9 | 23 => {
            for _ in 0..bounded_count(src, "enchantment count")? {
                let _ = src.read_varint()?;
                let _ = src.read_varint()?;
            }
            let _ = src.read_bool()?;
        }
        10 | 11 => {
            for _ in 0..bounded_count(src, "block predicate count")? {
                skip_block_predicate(src)?;
            }
            let _ = src.read_bool()?;
        }
        12 => {
            for _ in 0..bounded_count(src, "attribute count")? {
                let _ = src.read_varint()?;
                if !protocol_767 {
                    let _ = src.read_uuid()?;
                }
                skip_string(src)?;
                let _ = src.read_f64()?;
                let _ = src.read_varint()?;
                let _ = src.read_varint()?;
            }
            let _ = src.read_bool()?;
        }
        14 | 15 | 17 | 21 => {}
        19 | 27 | 36..=39 | 42 | 54 | 55 => {
            let value = nbt::read_anonymous_nbt(src)?;
            if kind == 39 {
                metadata.insert("BlockEntityTag".into(), value);
            }
        }
        20 => {
            let _ = src.read_varint()?;
            let _ = src.read_f32()?;
            let _ = src.read_bool()?;
            let _ = src.read_f32()?;
            let _ = read_component_slot_version(src, protocol_767)?;
            for _ in 0..bounded_count(src, "food effect count")? {
                let _ = src.read_varint()?;
                let _ = src.read_f32()?;
            }
        }
        22 => {
            for _ in 0..bounded_count(src, "tool rule count")? {
                skip_holder_set(src)?;
                skip_optional(src, |s| {
                    let _ = s.read_f32()?;
                    Ok(())
                })?;
                skip_optional(src, |s| {
                    let _ = s.read_bool()?;
                    Ok(())
                })?;
            }
            let _ = src.read_f32()?;
            let _ = src.read_varint()?;
        }
        24 => {
            let _ = src.read_i32()?;
            let _ = src.read_bool()?;
        }
        25 => {
            let _ = src.read_i32()?;
        }
        29 | 30 | 51 => {
            for _ in 0..bounded_count(src, "nested item count")? {
                let _ = read_component_slot_version(src, protocol_767)?;
            }
        }
        31 => {
            skip_optional(src, |s| {
                let _ = s.read_varint()?;
                Ok(())
            })?;
            skip_optional(src, |s| {
                let _ = s.read_i32()?;
                Ok(())
            })?;
            for _ in 0..bounded_count(src, "potion effect count")? {
                let _ = src.read_varint()?;
                skip_effect_detail(src, 0)?;
            }
            skip_optional(src, skip_string)?;
        }
        32 => {
            for _ in 0..bounded_count(src, "stew effect count")? {
                let _ = src.read_varint()?;
                let _ = src.read_varint()?;
            }
        }
        33 => {
            for _ in 0..bounded_count(src, "book page count")? {
                skip_string(src)?;
                skip_optional(src, skip_string)?;
            }
        }
        34 => {
            skip_string(src)?;
            skip_optional(src, skip_string)?;
            skip_string(src)?;
            let _ = src.read_varint()?;
            for _ in 0..bounded_count(src, "written book page count")? {
                let _ = nbt::read_anonymous_nbt(src)?;
                let _ = nbt::read_anonymous_nbt(src)?;
            }
            let _ = src.read_bool()?;
        }
        35 => {
            skip_registry_holder(src, skip_inline_trim_material)?;
            skip_registry_holder(src, skip_inline_trim_pattern)?;
            let _ = src.read_bool()?;
        }
        40 => skip_registry_holder(src, |s| {
            skip_sound_holder(s)?;
            let _ = s.read_f32()?;
            let _ = s.read_f32()?;
            let _ = nbt::read_anonymous_nbt(s)?;
            Ok(())
        })?,
        43 => {
            skip_optional(src, |s| {
                skip_string(s)?;
                let _ = s.read_position()?;
                Ok(())
            })?;
            let _ = src.read_bool()?;
        }
        44 => skip_firework_explosion(src)?,
        45 => {
            let _ = src.read_varint()?;
            for _ in 0..bounded_count(src, "firework explosion count")? {
                skip_firework_explosion(src)?;
            }
        }
        46 => {
            skip_optional(src, skip_string)?;
            skip_optional(src, |s| {
                let _ = s.read_uuid()?;
                Ok(())
            })?;
            for _ in 0..bounded_count(src, "profile property count")? {
                skip_string(src)?;
                skip_string(src)?;
                skip_optional(src, skip_string)?;
            }
        }
        47 => skip_string(src)?,
        48 => {
            for _ in 0..bounded_count(src, "banner layer count")? {
                skip_registry_holder(src, |s| {
                    skip_string(s)?;
                    skip_string(s)
                })?;
                let _ = src.read_varint()?;
            }
        }
        50 => {
            for _ in 0..bounded_count(src, "pot decoration count")? {
                let _ = src.read_varint()?;
            }
        }
        52 => {
            for _ in 0..bounded_count(src, "block state property count")? {
                skip_string(src)?;
                skip_string(src)?;
            }
        }
        53 => {
            for _ in 0..bounded_count(src, "bee count")? {
                let _ = nbt::read_anonymous_nbt(src)?;
                let _ = src.read_varint()?;
                let _ = src.read_varint()?;
            }
        }
        _ => {
            return Err(ProtoError::InvalidEnum {
                type_name: "item component type",
                value: i64::from(kind),
            })
        }
    }
    Ok(())
}

/// Decodes a protocol 766 item stack and consumes its complete component patch.
pub fn read_component_slot<B: Buf>(src: &mut B) -> Result<ComponentSlot, ProtoError> {
    read_component_slot_version(src, false)
}

/// Decodes the protocol 767 variant, whose item count is a VarInt and whose
/// component registry includes `jukebox_playable` at index 42.
pub fn read_component_slot_767<B: Buf>(src: &mut B) -> Result<ComponentSlot, ProtoError> {
    read_component_slot_version(src, true)
}

/// Decodes the protocol 768 component registry. 1.21.2 inserted ten component
/// types and changed several payloads, so translating IDs without consuming the
/// new schemas would desynchronize every following slot in the packet.
pub fn read_component_slot_768<B: Buf>(src: &mut B) -> Result<ComponentSlot, ProtoError> {
    let count = src.read_varint()?;
    if count == 0 {
        return Ok(ComponentSlot {
            item: None,
            metadata: None,
        });
    }
    if !(1..=127).contains(&count) {
        return Err(ProtoError::InvalidEnum {
            type_name: "protocol 768 item stack count",
            value: i64::from(count),
        });
    }
    let item_id = src.read_varint()?;
    let added = src.read_varint()?;
    let removed = src.read_varint()?;
    if !(0..=MAX_COMPONENTS).contains(&added) || !(0..=MAX_COMPONENTS).contains(&removed) {
        return Err(ProtoError::InvalidEnum {
            type_name: "protocol 768 item component count",
            value: i64::from(added.max(removed)),
        });
    }
    let mut values = HashMap::new();
    for _ in 0..added {
        let kind = src.read_varint()?;
        skip_component_768(src, kind, &mut values)?;
    }
    for _ in 0..removed {
        let _ = src.read_varint()?;
    }
    Ok(ComponentSlot {
        item: Some(SlotItem {
            item_id,
            count: i8::try_from(count).map_err(|_| ProtoError::InvalidEnum {
                type_name: "protocol 768 item stack count",
                value: i64::from(count),
            })?,
        }),
        metadata: (!values.is_empty()).then_some(Nbt::Compound(values)),
    })
}

fn skip_consume_effect_768<B: Buf>(src: &mut B) -> Result<(), ProtoError> {
    match src.read_varint()? {
        0 => {
            for _ in 0..bounded_count(src, "consume potion effect count")? {
                let _ = src.read_varint()?;
                skip_effect_detail(src, 0)?;
            }
            let _ = src.read_f32()?;
        }
        1 => skip_holder_set(src)?,
        2 => {}
        3 => {
            let _ = src.read_f32()?;
        }
        4 => skip_sound_holder(src)?,
        value => {
            return Err(ProtoError::InvalidEnum {
                type_name: "consume effect type",
                value: i64::from(value),
            })
        }
    }
    Ok(())
}

fn skip_component_768<B: Buf>(
    src: &mut B,
    kind: i32,
    metadata: &mut HashMap<String, Nbt>,
) -> Result<(), ProtoError> {
    match kind {
        7 | 25 | 31 => skip_string(src)?,
        14 => {
            for _ in 0..bounded_count(src, "custom model float count")? {
                let _ = src.read_f32()?;
            }
            for _ in 0..bounded_count(src, "custom model flag count")? {
                let _ = src.read_bool()?;
            }
            for _ in 0..bounded_count(src, "custom model string count")? {
                skip_string(src)?;
            }
            for _ in 0..bounded_count(src, "custom model color count")? {
                let _ = src.read_i32()?;
            }
        }
        21 => {
            let _ = src.read_varint()?;
            let _ = src.read_f32()?;
            let _ = src.read_bool()?;
        }
        22 => {
            let _ = src.read_f32()?;
            let _ = src.read_varint()?;
            skip_sound_holder(src)?;
            let _ = src.read_bool()?;
            for _ in 0..bounded_count(src, "consume effect count")? {
                skip_consume_effect_768(src)?;
            }
        }
        23 => {
            let _ = read_component_slot_768(src)?;
        }
        24 => {
            let _ = src.read_f32()?;
            skip_optional(src, skip_string)?;
        }
        27 => {
            let _ = src.read_varint()?;
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
        }
        29 => skip_holder_set(src)?,
        30 => {}
        32 => {
            for _ in 0..bounded_count(src, "death protection effect count")? {
                skip_consume_effect_768(src)?;
            }
        }
        40 => {
            let mut contents = Vec::new();
            for _ in 0..bounded_count(src, "protocol 768 bundle item count")? {
                let nested = read_component_slot_768(src)?;
                if let Some(item) = nested.item {
                    let mut entry = HashMap::new();
                    entry.insert("id".to_string(), Nbt::Int(item.item_id));
                    entry.insert("count".to_string(), Nbt::Byte(item.count));
                    contents.push(Nbt::Compound(entry));
                }
            }
            metadata.insert("bundle_contents".to_string(), Nbt::List(contents));
        }
        39 | 62 => {
            for _ in 0..bounded_count(src, "protocol 768 nested item count")? {
                let _ = read_component_slot_768(src)?;
            }
        }
        _ => {
            let translated = match kind {
                0..=6 => kind,
                8..=13 => kind - 1,
                15..=20 => kind - 1,
                26 => 22,
                33..=38 => kind - 10,
                41..=61 => kind - 10,
                63..=66 => kind - 10,
                _ => {
                    return Err(ProtoError::InvalidEnum {
                        type_name: "protocol 768 item component type",
                        value: i64::from(kind),
                    })
                }
            };
            skip_component(src, translated, metadata, true)?;
        }
    }
    Ok(())
}

fn read_component_slot_version<B: Buf>(
    src: &mut B,
    protocol_767: bool,
) -> Result<ComponentSlot, ProtoError> {
    let count = if protocol_767 {
        src.read_varint()?
    } else {
        i32::from(src.read_i8()?)
    };
    if count == 0 {
        return Ok(ComponentSlot {
            item: None,
            metadata: None,
        });
    }
    if !(1..=127).contains(&count) {
        return Err(ProtoError::InvalidEnum {
            type_name: "item stack count",
            value: i64::from(count),
        });
    }
    let item_id = src.read_varint()?;
    let added = src.read_varint()?;
    let removed = src.read_varint()?;
    if !(0..=MAX_COMPONENTS).contains(&added) || !(0..=MAX_COMPONENTS).contains(&removed) {
        return Err(ProtoError::InvalidEnum {
            type_name: "item component count",
            value: i64::from(added.max(removed)),
        });
    }
    let mut values = HashMap::new();
    for _ in 0..added {
        let kind = src.read_varint()?;
        skip_component(src, kind, &mut values, protocol_767)?;
    }
    for _ in 0..removed {
        let _ = src.read_varint()?;
    }
    Ok(ComponentSlot {
        item: Some(SlotItem {
            item_id,
            count: i8::try_from(count).map_err(|_| ProtoError::InvalidEnum {
                type_name: "item stack count",
                value: i64::from(count),
            })?,
        }),
        metadata: (!values.is_empty()).then_some(Nbt::Compound(values)),
    })
}

/// Writes a component-free protocol 766 item stack.
pub fn write_component_slot<B: BufMut>(dst: &mut B, item: Option<SlotItem>) {
    match item {
        Some(item) => {
            dst.put_i8(item.count);
            dst.put_varint(item.item_id);
            dst.put_varint(0);
            dst.put_varint(0);
        }
        None => dst.put_i8(0),
    }
}

/// Writes the protocol 767 VarInt-count item stack representation.
pub fn write_component_slot_767<B: BufMut>(dst: &mut B, item: Option<SlotItem>) {
    match item {
        Some(item) => {
            dst.put_varint(i32::from(item.count));
            dst.put_varint(item.item_id);
            dst.put_varint(0);
            dst.put_varint(0);
        }
        None => dst.put_varint(0),
    }
}

/// Protocol 766 container click body using component-patch item stacks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClickContainerComponents {
    pub window_id: u8,
    pub state_id: i32,
    pub slot: i16,
    pub button: i8,
    pub mode: i32,
    pub changed: Vec<(i16, Option<SlotItem>)>,
    pub carried: Option<SlotItem>,
}

impl Packet for ClickContainerComponents {
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
        for (slot, item) in &self.changed {
            dst.put_i16(*slot);
            write_component_slot(dst, *item);
        }
        write_component_slot(dst, self.carried);
        Ok(())
    }
    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        let window_id = src.read_u8()?;
        let state_id = src.read_varint()?;
        let slot = src.read_i16()?;
        let button = src.read_i8()?;
        let mode = src.read_varint()?;
        let mut changed = Vec::new();
        for _ in 0..bounded_count(src, "changed slot count")? {
            let slot = src.read_i16()?;
            changed.push((slot, read_component_slot(src)?.item));
        }
        let carried = read_component_slot(src)?.item;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfigurationAcknowledged;

impl Packet for ConfigurationAcknowledged {
    const ID: i32 = 0x0c;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;
    fn encode<B: BufMut>(&self, _dst: &mut B) -> Result<(), ProtoError> {
        Ok(())
    }
    fn decode<B: Buf>(_src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self)
    }
}

/// Protocol 766 wire packet `0x08`, acknowledging a completed chunk batch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChunkBatchReceived {
    pub chunks_per_tick: f32,
}

impl Packet for ChunkBatchReceived {
    const ID: i32 = 0x08;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;
    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_f32(self.chunks_per_tick);
        Ok(())
    }
    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            chunks_per_tick: src.read_f32()?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourcePackStatus {
    pub uuid: Uuid,
    pub status: i32,
}

impl Packet for ResourcePackStatus {
    const ID: i32 = 0x2b;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;
    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_uuid(self.uuid);
        dst.put_varint(self.status);
        Ok(())
    }
    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            uuid: src.read_uuid()?,
            status: src.read_varint()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_slots_roundtrip_plain_stacks() {
        let mut bytes = Vec::new();
        write_component_slot(
            &mut bytes,
            Some(SlotItem {
                item_id: 845,
                count: 3,
            }),
        );
        let mut input = bytes.as_slice();
        let decoded = read_component_slot(&mut input).unwrap();
        assert_eq!(decoded.item.unwrap().item_id, 845);
        assert_eq!(decoded.item.unwrap().count, 3);
        assert_eq!(decoded.metadata, None);
        assert!(input.is_empty());
    }

    #[test]
    fn component_slots_preserve_name_and_map_metadata() {
        let mut bytes = Vec::new();
        bytes.put_i8(1); // count
        bytes.put_varint(907); // filled map
        bytes.put_varint(2); // added components
        bytes.put_varint(0); // removed components
        bytes.put_varint(5); // custom_name
        bytes.put_u8(8); // anonymous TAG_String
        bytes.put_u16(10);
        bytes.put_slice(b"Crab Blade");
        bytes.put_varint(26); // map_id
        bytes.put_varint(42);
        bytes.put_u8(0xaa); // prove the component patch stayed aligned

        let mut input = bytes.as_slice();
        let decoded = read_component_slot(&mut input).unwrap();
        let metadata = decoded.metadata.unwrap();
        assert_eq!(
            metadata.get("custom_name"),
            Some(&Nbt::String("Crab Blade".into()))
        );
        assert_eq!(metadata.get("map"), Some(&Nbt::Int(42)));
        assert_eq!(input.read_u8().unwrap(), 0xaa);
    }

    #[test]
    fn component_container_click_roundtrips() {
        let packet = ClickContainerComponents {
            window_id: 2,
            state_id: 7,
            slot: 4,
            button: 0,
            mode: 0,
            changed: vec![(
                4,
                Some(SlotItem {
                    item_id: 1,
                    count: 32,
                }),
            )],
            carried: None,
        };
        let mut bytes = Vec::new();
        packet.encode(&mut bytes).unwrap();
        let decoded = ClickContainerComponents::decode(&mut bytes.as_slice()).unwrap();
        assert_eq!(decoded, packet);
    }
}
