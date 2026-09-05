//! Play-state payloads changed by protocol 771.

use bytes::{Buf, BufMut};

use crate::error::ProtoError;
use crate::io::{BufExt, BufMutExt};
use crate::packet::{Bound, Packet, State};
use crate::versions::v1_20_5::play as play766;
use crate::versions::v1_20_5::play::ComponentSlot;
use crate::versions::v1_21_2::play as play768;
use crate::versions::v1_21_4::play as play769;
use crate::versions::v1_21_5::play as play770;

/// Protocol 772's command packet contains only the command text. The
/// signature/timestamp trailer used by older play versions was removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatCommand(pub String);

impl ChatCommand {
    pub fn new(command: String) -> Self {
        Self(command)
    }
}

/// Protocol 772 client status moved behind the newly registered chat packets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientCommand(pub i32);

impl Packet for ClientCommand {
    const ID: i32 = 0x0c;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_varint(self.0);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self(src.read_varint()?))
    }
}

impl Packet for ChatCommand {
    const ID: i32 = 0x04;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_string(&self.0);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self(src.read_string(256)?))
    }
}

macro_rules! shifted_packet {
    ($(#[$meta:meta])* $name:ident($inner:path), $id:expr) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq)]
        pub struct $name(pub $inner);

        impl Packet for $name {
            const ID: i32 = $id;
            const STATE: State = State::Play;
            const BOUND: Bound = Bound::Serverbound;

            fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
                self.0.encode(dst)
            }

            fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
                Ok(Self(<$inner>::decode(src)?))
            }
        }
    };
}

shifted_packet!(
    /// Play-state client settings shifted by the Change Game Mode insertion.
    ClientInformation(play768::ClientInformation),
    0x0d
);
shifted_packet!(
    /// Chunk-batch acknowledgement shifted by the Change Game Mode insertion.
    ChunkBatchReceived(play768::ChunkBatchReceived),
    0x0a
);
shifted_packet!(
    /// Initial-terrain acknowledgement shifted by the Change Game Mode insertion.
    PlayerLoaded(play769::PlayerLoaded),
    0x2b
);
shifted_packet!(
    /// Numeric recipe placement shifted by the Change Game Mode insertion.
    PlaceRecipe(play769::PlaceRecipe),
    0x26
);
shifted_packet!(
    /// Player input shifted by the Change Game Mode insertion.
    PlayerInput(play769::PlayerInput),
    0x2a
);
shifted_packet!(
    /// Root-vehicle movement shifted by the Change Game Mode insertion.
    VehicleMove(play769::VehicleMove),
    0x21
);
shifted_packet!(
    /// Block use shifted after the protocol 771 tail reorder.
    UseItemOn(play770::UseItemOn),
    0x3f
);
shifted_packet!(
    /// Air use shifted after the protocol 771 tail reorder.
    UseItem(play770::UseItem),
    0x40
);
shifted_packet!(
    /// UUID resource-pack response shifted by the Change Game Mode insertion.
    ResourcePackStatus(play766::ResourcePackStatus),
    0x30
);
shifted_packet!(
    /// Play-to-configuration acknowledgement shifted by the insertion.
    ConfigurationAcknowledged(play766::ConfigurationAcknowledged),
    0x0f
);

/// `0x08` — protocol 771 chat message with the checksum trailer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientChatMessage(pub play770::ClientChatMessage);

impl ClientChatMessage {
    /// Builds an unsigned offline-mode chat message.
    pub fn unsigned(message: String) -> Self {
        Self(play770::ClientChatMessage::unsigned(message))
    }
}

impl Packet for ClientChatMessage {
    const ID: i32 = 0x08;
    const STATE: State = State::Play;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        self.0.encode(dst)
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self(play770::ClientChatMessage::decode(src)?))
    }
}

/// Decodes a protocol 771 item stack.
///
/// The component IDs remain stable from protocol 770, while attribute modifier
/// display data and equippable shearing fields extend two existing payloads.
pub fn read_component_slot<B: Buf>(src: &mut B) -> Result<ComponentSlot, ProtoError> {
    crate::versions::v1_21_5::play::read_component_slot_version(src, true)
}

#[cfg(test)]
mod tests {
    use bytes::BufMut;

    use super::*;
    use crate::{BufExt, BufMutExt};

    #[test]
    fn protocol_771_extended_components_preserve_alignment() {
        let mut bytes = Vec::new();
        bytes.put_varint(1);
        bytes.put_varint(1);
        bytes.put_varint(2);
        bytes.put_varint(0);

        bytes.put_varint(13); // attribute modifiers
        bytes.put_varint(1);
        bytes.put_varint(20);
        bytes.put_string("minecraft:movement_speed");
        bytes.put_f64(0.1);
        bytes.put_varint(0);
        bytes.put_varint(1);
        bytes.put_varint(2); // overridden display component
        bytes.put_u8(0); // empty anonymous NBT

        bytes.put_varint(28); // equippable
        bytes.put_varint(5);
        bytes.put_varint(1); // sound registry reference
        bytes.put_bool(false);
        bytes.put_bool(false);
        bytes.put_bool(false);
        bytes.put_bool(true);
        bytes.put_bool(true);
        bytes.put_bool(true);
        bytes.put_bool(false);
        bytes.put_bool(true); // shearable
        bytes.put_varint(1); // shearing sound registry reference
        bytes.put_u8(0xaa);

        let mut input = bytes.as_slice();
        let decoded = read_component_slot(&mut input).unwrap();
        assert_eq!(decoded.item.unwrap().item_id, 1);
        assert_eq!(input.read_u8().unwrap(), 0xaa);
    }
}
