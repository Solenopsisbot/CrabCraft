//! Play-state payloads introduced or changed by protocol 768.

use bytes::{Buf, BufMut};

use crate::error::ProtoError;
use crate::io::{BufExt, BufMutExt};
use crate::packet::{Bound, Packet, State};

/// Play-state Client Information uses the same settings payload at a different
/// packet ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientInformation(pub super::configuration::ClientInformation);

impl Packet for ClientInformation {
    const ID: i32 = 0x0c;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        self.0.encode(dst)
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self(super::configuration::ClientInformation::decode(src)?))
    }
}

/// Clientbound teleport/correction. Protocol 768 moved the teleport ID first,
/// added authoritative velocity, and widened relative flags to nine bits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SynchronizePlayerPosition {
    pub teleport_id: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub velocity: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub flags: u32,
}

impl Packet for SynchronizePlayerPosition {
    const ID: i32 = 0x42;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Clientbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_varint(self.teleport_id);
        dst.put_f64(self.x);
        dst.put_f64(self.y);
        dst.put_f64(self.z);
        for value in self.velocity {
            dst.put_f64(value);
        }
        dst.put_f32(self.yaw);
        dst.put_f32(self.pitch);
        dst.put_u32(self.flags);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            teleport_id: src.read_varint()?,
            x: src.read_f64()?,
            y: src.read_f64()?,
            z: src.read_f64()?,
            velocity: [src.read_f64()?, src.read_f64()?, src.read_f64()?],
            yaw: src.read_f32()?,
            pitch: src.read_f32()?,
            flags: src.read_i32()? as u32,
        })
    }
}

/// Protocol 768 replaces analogue Steer Vehicle with current input key bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerInput {
    pub flags: u8,
}

/// `0x3b` — uses the held item with the view rotation added in protocol 768.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UseItem {
    pub hand: i32,
    pub sequence: i32,
    pub yaw: f32,
    pub pitch: f32,
}

impl Packet for UseItem {
    const ID: i32 = 0x3b;
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

/// Chunk-batch acknowledgement shifted by the new Select Bundle Item packet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChunkBatchReceived {
    pub chunks_per_tick: f32,
}

/// Recipe placement switched from a namespaced string to a numeric display ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaceRecipe {
    pub window_id: i32,
    pub recipe_id: i32,
    pub make_all: bool,
}

/// `0x02` — select an item inside the bundle under the inventory cursor.
///
/// Protocol 768 introduced this packet alongside interactive bundle tooltips.
/// `selected_item_index == -1` clears the bundle selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectBundleItem {
    pub slot_id: i32,
    pub selected_item_index: i32,
}

impl Packet for SelectBundleItem {
    const ID: i32 = 0x02;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_varint(self.slot_id);
        dst.put_varint(self.selected_item_index);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            slot_id: src.read_varint()?,
            selected_item_index: src.read_varint()?,
        })
    }
}

impl Packet for PlaceRecipe {
    const ID: i32 = 0x24;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_varint(self.window_id);
        dst.put_varint(self.recipe_id);
        dst.put_bool(self.make_all);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            window_id: src.read_varint()?,
            recipe_id: src.read_varint()?,
            make_all: src.read_bool()?,
        })
    }
}

impl Packet for ChunkBatchReceived {
    const ID: i32 = 0x09;
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

impl Packet for PlayerInput {
    const ID: i32 = 0x28;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_u8(self.flags);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            flags: src.read_u8()?,
        })
    }
}

/// Use Item On gained a world-border-hit bit before its sequence number.
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
    const ID: i32 = 0x3a;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_varint(self.hand);
        dst.put_position(self.x, self.y, self.z);
        dst.put_varint(self.direction);
        for value in self.cursor {
            dst.put_f32(value);
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
    use crate::nbt::Nbt;
    use crate::versions::v1_20_5::play::read_component_slot_768;

    #[test]
    fn protocol_768_position_carries_velocity_and_wide_flags() {
        let packet = SynchronizePlayerPosition {
            teleport_id: 7,
            x: 1.0,
            y: 2.0,
            z: 3.0,
            velocity: [0.1, -0.2, 0.3],
            yaw: 45.0,
            pitch: -10.0,
            flags: 0x1ff,
        };
        let mut bytes = Vec::new();
        packet.encode(&mut bytes).unwrap();
        assert_eq!(
            packet,
            SynchronizePlayerPosition::decode(&mut bytes.as_slice()).unwrap()
        );
    }

    #[test]
    fn protocol_768_input_and_block_use_roundtrip() {
        let input = PlayerInput { flags: 0b0111_0001 };
        let mut bytes = Vec::new();
        input.encode(&mut bytes).unwrap();
        assert_eq!(input, PlayerInput::decode(&mut bytes.as_slice()).unwrap());

        let use_item = UseItem {
            hand: 0,
            sequence: 4,
            yaw: 30.0,
            pitch: -8.0,
        };
        bytes.clear();
        use_item.encode(&mut bytes).unwrap();
        assert_eq!(use_item, UseItem::decode(&mut bytes.as_slice()).unwrap());

        let use_on = UseItemOn {
            hand: 0,
            x: -2,
            y: 64,
            z: 5,
            direction: 1,
            cursor: [0.5; 3],
            inside_block: false,
            world_border_hit: true,
            sequence: 12,
        };
        bytes.clear();
        use_on.encode(&mut bytes).unwrap();
        assert_eq!(use_on, UseItemOn::decode(&mut bytes.as_slice()).unwrap());

        let recipe = PlaceRecipe {
            window_id: 2,
            recipe_id: 417,
            make_all: true,
        };
        bytes.clear();
        recipe.encode(&mut bytes).unwrap();
        assert_eq!(recipe, PlaceRecipe::decode(&mut bytes.as_slice()).unwrap());

        let selection = SelectBundleItem {
            slot_id: 36,
            selected_item_index: 3,
        };
        bytes.clear();
        selection.encode(&mut bytes).unwrap();
        assert_eq!(
            selection,
            SelectBundleItem::decode(&mut bytes.as_slice()).unwrap()
        );
    }

    #[test]
    fn protocol_768_inserted_components_preserve_slot_alignment() {
        let mut bytes = Vec::new();
        bytes.put_varint(1); // count
        bytes.put_varint(1135); // mace in the 1.21.3 item registry
        bytes.put_varint(5); // added component count
        bytes.put_varint(0); // removed component count
        bytes.put_varint(7); // item_model
        bytes.put_string("minecraft:mace");
        bytes.put_varint(14); // custom_model_data arrays
        bytes.put_varint(1);
        bytes.put_f32(1.5);
        bytes.put_varint(1);
        bytes.put_bool(true);
        bytes.put_varint(1);
        bytes.put_string("variant");
        bytes.put_varint(1);
        bytes.put_i32(0x123456);
        bytes.put_varint(21); // food
        bytes.put_varint(4);
        bytes.put_f32(0.6);
        bytes.put_bool(false);
        bytes.put_varint(22); // consumable with no effects
        bytes.put_f32(1.6);
        bytes.put_varint(1);
        bytes.put_varint(1); // direct sound holder id
        bytes.put_bool(true);
        bytes.put_varint(0);
        bytes.put_varint(36); // map_id after all inserted components
        bytes.put_varint(9);
        bytes.put_u8(0xaa);

        let mut input = bytes.as_slice();
        let slot = read_component_slot_768(&mut input).unwrap();
        assert_eq!(slot.item.unwrap().item_id, 1135);
        assert_eq!(slot.metadata.unwrap().get("map"), Some(&Nbt::Int(9)));
        assert_eq!(input.read_u8().unwrap(), 0xaa);
    }

    #[test]
    fn protocol_768_bundle_contents_are_retained_for_tooltips() {
        let mut bytes = Vec::new();
        bytes.put_varint(1); // outer stack count
        bytes.put_varint(1152); // bundle-like item id; identity is registry-owned
        bytes.put_varint(1); // added component count
        bytes.put_varint(0); // removed component count
        bytes.put_varint(40); // bundle_contents
        bytes.put_varint(1); // one nested item
        bytes.put_varint(5); // nested stack count
        bytes.put_varint(42); // nested item id
        bytes.put_varint(0); // no added components
        bytes.put_varint(0); // no removed components

        let decoded = read_component_slot_768(&mut bytes.as_slice()).unwrap();
        let contents = decoded
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("bundle_contents"));
        let crate::nbt::Nbt::List(contents) = contents.unwrap() else {
            panic!("bundle contents should be retained as a list");
        };
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0].get("id"), Some(&crate::nbt::Nbt::Int(42)));
        assert_eq!(contents[0].get("count"), Some(&crate::nbt::Nbt::Byte(5)));
    }
}
