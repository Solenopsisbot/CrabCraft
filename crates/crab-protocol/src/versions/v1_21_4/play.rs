//! Play-state payloads introduced or changed by protocol 769.

use bytes::{Buf, BufMut};

use crate::error::ProtoError;
use crate::io::{BufExt, BufMutExt};
use crate::packet::{Bound, Packet, State};

/// `0x29` — current movement input key bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerInput {
    pub flags: u8,
}

impl Packet for PlayerInput {
    const ID: i32 = 0x29;
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

/// `0x2a` — acknowledges that the client has loaded the initial terrain.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlayerLoaded;

impl Packet for PlayerLoaded {
    const ID: i32 = 0x2a;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, _dst: &mut B) -> Result<(), ProtoError> {
        Ok(())
    }

    fn decode<B: Buf>(_src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self)
    }
}

/// `0x25` — places a numeric recipe-display ID into the current menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaceRecipe {
    pub window_id: i32,
    pub recipe_id: i32,
    pub make_all: bool,
}

impl Packet for PlaceRecipe {
    const ID: i32 = 0x25;
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

/// `0x02` — selects a nested item in the hovered bundle.
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

/// `0x3d` — uses the held item, including the view rotation required since 768.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UseItem {
    pub hand: i32,
    pub sequence: i32,
    pub yaw: f32,
    pub pitch: f32,
}

impl Packet for UseItem {
    const ID: i32 = 0x3d;
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

/// `0x3c` — uses an item on a block. Its payload is unchanged from 768.
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
    const ID: i32 = 0x3c;
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

/// `0x20` — controls a root vehicle; 769 appends the on-ground flag.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VehicleMove {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
}

impl Packet for VehicleMove {
    const ID: i32 = 0x20;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_769_changed_packets_roundtrip() {
        let mut bytes = Vec::new();
        let input = PlayerInput { flags: 0x55 };
        input.encode(&mut bytes).unwrap();
        assert_eq!(input, PlayerInput::decode(&mut bytes.as_slice()).unwrap());

        bytes.clear();
        PlayerLoaded.encode(&mut bytes).unwrap();
        assert!(bytes.is_empty());

        let recipe = PlaceRecipe {
            window_id: 3,
            recipe_id: 91,
            make_all: true,
        };
        recipe.encode(&mut bytes).unwrap();
        assert_eq!(recipe, PlaceRecipe::decode(&mut bytes.as_slice()).unwrap());

        bytes.clear();
        let use_item = UseItem {
            hand: 1,
            sequence: 7,
            yaw: 45.0,
            pitch: -12.0,
        };
        use_item.encode(&mut bytes).unwrap();
        assert_eq!(use_item, UseItem::decode(&mut bytes.as_slice()).unwrap());

        bytes.clear();
        let vehicle = VehicleMove {
            x: 1.0,
            y: 2.0,
            z: 3.0,
            yaw: 90.0,
            pitch: 4.0,
            on_ground: true,
        };
        vehicle.encode(&mut bytes).unwrap();
        assert_eq!(vehicle, VehicleMove::decode(&mut bytes.as_slice()).unwrap());
    }
}
