//! Packets introduced specifically by protocol 764's play state.

use bytes::{Buf, BufMut};

use crate::error::ProtoError;
use crate::io::BufExt;
use crate::packet::{Bound, Packet, State};

/// `0x07` — reports the client's desired chunks-per-tick after a server batch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChunkBatchReceived {
    pub chunks_per_tick: f32,
}

impl Packet for ChunkBatchReceived {
    const ID: i32 = 0x07;
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
