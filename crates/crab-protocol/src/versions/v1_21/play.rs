//! Play-state payloads changed by protocol 767.

use bytes::{Buf, BufMut};

use crate::error::ProtoError;
use crate::io::{BufExt, BufMutExt};
use crate::packet::{Bound, Packet, State};
use crate::versions::v1_20_1::play::SlotItem;
use crate::versions::v1_20_5::play::{read_component_slot_767, write_component_slot_767};

/// Protocol 767 container click using VarInt-count component item stacks.
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
            write_component_slot_767(dst, *item);
        }
        write_component_slot_767(dst, self.carried);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        let window_id = src.read_u8()?;
        let state_id = src.read_varint()?;
        let slot = src.read_i16()?;
        let button = src.read_i8()?;
        let mode = src.read_varint()?;
        let count = src.read_varint()?;
        if !(0..=65_536).contains(&count) {
            return Err(ProtoError::InvalidEnum {
                type_name: "protocol 767 changed slot count",
                value: i64::from(count),
            });
        }
        let mut changed = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let slot = src.read_i16()?;
            changed.push((slot, read_component_slot_767(src)?.item));
        }
        let carried = read_component_slot_767(src)?.item;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_767_click_uses_varint_stack_counts() {
        let packet = ClickContainerComponents {
            window_id: 1,
            state_id: 2,
            slot: 3,
            button: 0,
            mode: 0,
            changed: vec![(
                3,
                Some(SlotItem {
                    item_id: 1093,
                    count: 1,
                }),
            )],
            carried: None,
        };
        let mut bytes = Vec::new();
        packet.encode(&mut bytes).unwrap();
        let decoded = ClickContainerComponents::decode(&mut bytes.as_slice()).unwrap();
        assert_eq!(decoded, packet);
    }

    #[test]
    fn jukebox_component_keeps_following_components_aligned() {
        let mut bytes = Vec::new();
        bytes.put_varint(1); // stack count is a VarInt in 767
        bytes.put_varint(1); // item id
        bytes.put_varint(2); // two added components
        bytes.put_varint(0);
        bytes.put_varint(42); // jukebox_playable
        bytes.put_bool(false); // direct song identifier
        bytes.put_string("minecraft:precipice");
        bytes.put_bool(true); // show in tooltip
        bytes.put_varint(26); // map_id precedes the inserted component
        bytes.put_varint(9);
        bytes.put_u8(0xaa);

        let mut input = bytes.as_slice();
        let decoded = read_component_slot_767(&mut input).unwrap();
        assert_eq!(
            decoded.metadata.unwrap().get("map"),
            Some(&crate::nbt::Nbt::Int(9))
        );
        assert_eq!(input.read_u8().unwrap(), 0xaa);
    }
}
