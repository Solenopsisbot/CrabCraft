//! Auditable packet-ID profiles. Body codecs remain in `crab-protocol`; these
//! maps are used only where the body is byte-for-byte identical.

use crab_protocol::packet::{Packet, State};
use crab_protocol::versions::v1_20_1::play::{
    KeepAlive, Ping, SynchronizePlayerPosition, SystemChat,
};

use crate::ProtocolVersion;

// Canonical protocol-763 IDs consumed by the client core adapter.
const SPAWN_ENTITY: i32 = 0x01;
const ENTITY_ANIMATION: i32 = 0x04;
const BLOCK_ENTITY_DATA: i32 = 0x08;
const BLOCK_CHANGE: i32 = 0x0a;
const BOSS_BAR: i32 = 0x0b;
const CLEAR_TITLES: i32 = 0x0e;
const CONTAINER_CONTENT: i32 = 0x12;
const CONTAINER_SLOT: i32 = 0x14;
const UNLOAD_CHUNK: i32 = 0x1e;
const GAME_STATE: i32 = 0x1f;
const HURT_ANIMATION: i32 = 0x21;
const INIT_WORLD_BORDER: i32 = 0x22;
const MAP_CHUNK: i32 = 0x24;
const PARTICLES: i32 = 0x26;
const JOIN_GAME: i32 = 0x28;
const MAP_DATA: i32 = 0x29;
const REL_MOVE: i32 = 0x2b;
const MOVE_LOOK: i32 = 0x2c;
const ENTITY_ROTATION: i32 = 0x2d;
const VEHICLE_MOVE: i32 = 0x2e;
const PLAYER_REMOVE: i32 = 0x39;
const PLAYER_INFO: i32 = 0x3a;
const REMOVE_ENTITY_EFFECT: i32 = 0x3f;
const RESPAWN: i32 = 0x41;
const ENTITY_HEAD_ROTATION: i32 = 0x42;
const ACTION_BAR: i32 = 0x46;
const WORLD_BORDER_CENTER: i32 = 0x47;
const WORLD_BORDER_LERP: i32 = 0x48;
const WORLD_BORDER_SIZE: i32 = 0x49;
const WORLD_BORDER_WARNING_DELAY: i32 = 0x4a;
const WORLD_BORDER_WARNING_REACH: i32 = 0x4b;
const SET_HELD_ITEM: i32 = 0x4d;
const SCOREBOARD_DISPLAY: i32 = 0x51;
const ENTITY_METADATA: i32 = 0x52;
const ENTITY_VELOCITY: i32 = 0x54;
const ENTITY_EQUIPMENT: i32 = 0x55;
const EXPERIENCE: i32 = 0x56;
const UPDATE_HEALTH: i32 = 0x57;
const SCOREBOARD_OBJECTIVE: i32 = 0x58;
const SET_PASSENGERS: i32 = 0x59;
const TEAMS: i32 = 0x5a;
const SCOREBOARD_SCORE: i32 = 0x5b;
const TITLE_SUBTITLE: i32 = 0x5d;
const UPDATE_TIME: i32 = 0x5e;
const TITLE_TEXT: i32 = 0x5f;
const TITLE_TIME: i32 = 0x60;
const ENTITY_SOUND: i32 = 0x61;
const SOUND: i32 = 0x62;
const PLAYER_LIST_HEADER: i32 = 0x65;
const COLLECT_ITEM: i32 = 0x67;
const ENTITY_TELEPORT: i32 = 0x68;
const ENTITY_EFFECT: i32 = 0x6c;
const DECLARE_RECIPES: i32 = 0x6d;
const RESOURCE_PACK: i32 = 0x40;

impl ProtocolVersion {
    /// Maps a canonical protocol-763 serverbound ID only where the packet body
    /// is known to be unchanged. Changed bodies must use their version codec.
    #[must_use]
    pub fn serverbound_id(self, state: State, canonical: i32) -> i32 {
        match self {
            Self::V1_20_1 => canonical,
            Self::V1_20_2 => serverbound_764(state, canonical),
            Self::V1_20_3 => serverbound_765(state, canonical),
            Self::V1_20_5 | Self::V1_21 => serverbound_766(state, canonical),
            Self::V1_21_2 => serverbound_768(state, canonical),
            Self::V1_21_4 => serverbound_769(state, canonical),
            Self::V1_21_5 => serverbound_770(state, canonical),
        }
    }

    /// Function pointer accepted by `crab-net`'s framing connection.
    #[must_use]
    pub const fn serverbound_mapper(self) -> Option<fn(State, i32) -> i32> {
        match self {
            Self::V1_20_1 => None,
            Self::V1_20_2 => Some(serverbound_764),
            Self::V1_20_3 => Some(serverbound_765),
            Self::V1_20_5 | Self::V1_21 => Some(serverbound_766),
            Self::V1_21_2 => Some(serverbound_768),
            Self::V1_21_4 => Some(serverbound_769),
            Self::V1_21_5 => Some(serverbound_770),
        }
    }

    /// Converts an unchanged clientbound wire ID to its canonical protocol-763
    /// ID. `None` means the packet is inserted, removed, changed, or unsupported
    /// and therefore must not be decoded with a canonical body codec.
    #[must_use]
    pub fn canonical_clientbound_id(self, wire: i32) -> Option<i32> {
        let canonical = match self {
            Self::V1_20_1 => wire,
            Self::V1_20_2 => clientbound_764(wire),
            Self::V1_20_3 => clientbound_765(wire),
            Self::V1_20_5 | Self::V1_21 => clientbound_766(wire),
            Self::V1_21_2 | Self::V1_21_4 => clientbound_768(wire),
            Self::V1_21_5 => clientbound_770(wire),
        };
        (canonical >= 0).then_some(canonical)
    }
}

fn serverbound_764(state: State, canonical: i32) -> i32 {
    if state != State::Play {
        return canonical;
    }
    match canonical {
        0x00..=0x06 => canonical,
        0x07..=0x09 => canonical + 1,
        0x0a..=0x1a => canonical + 2,
        0x1b..=0x32 => canonical + 3,
        _ => canonical,
    }
}

fn serverbound_765(state: State, canonical: i32) -> i32 {
    if state != State::Play {
        return canonical;
    }
    match canonical {
        0x00..=0x06 => canonical,
        0x07..=0x09 => canonical + 1,
        0x0a..=0x0c => canonical + 2,
        0x0d..=0x1a => canonical + 3,
        0x1b..=0x32 => canonical + 4,
        _ => canonical,
    }
}

fn serverbound_766(state: State, canonical: i32) -> i32 {
    if state == State::Configuration {
        return match canonical {
            0x01..=0x05 => canonical + 1,
            _ => canonical,
        };
    }
    if state != State::Play {
        return canonical;
    }
    match canonical {
        0x00..=0x04 => canonical,
        0x05..=0x06 => canonical + 1,
        0x07..=0x09 => canonical + 2,
        0x0a..=0x0c => canonical + 3,
        0x0d => canonical + 5,
        0x0e..=0x1a => canonical + 6,
        0x1b..=0x32 => canonical + 7,
        _ => canonical,
    }
}

fn serverbound_768(state: State, canonical: i32) -> i32 {
    if state == State::Configuration {
        return serverbound_766(state, canonical);
    }
    if state != State::Play {
        return canonical;
    }
    match canonical {
        0x00..=0x01 => canonical,
        0x02..=0x04 => canonical + 1,
        0x05..=0x06 => canonical + 2,
        0x07 => canonical + 3,
        0x08..=0x09 => canonical + 4,
        0x0a..=0x0c => canonical + 5,
        0x0d => canonical + 7,
        0x0e..=0x1a => canonical + 8,
        0x1b..=0x1e | 0x20..=0x32 => canonical + 9,
        _ => canonical,
    }
}

fn serverbound_769(state: State, canonical: i32) -> i32 {
    let mapped = serverbound_768(state, canonical);
    if state != State::Play {
        return mapped;
    }
    match mapped {
        0x23..=0x28 => mapped + 1,
        0x29.. => mapped + 2,
        _ => mapped,
    }
}

fn serverbound_770(state: State, canonical: i32) -> i32 {
    let mapped = serverbound_769(state, canonical);
    if state != State::Play {
        return mapped;
    }
    match mapped {
        0x39..=0x3a => mapped + 1,
        0x3b.. => mapped + 2,
        _ => mapped,
    }
}

fn clientbound_764(wire: i32) -> i32 {
    match wire {
        0x00..=0x02 => wire,
        0x03..=0x0b => wire + 1,
        0x0c | 0x0d | 0x34 | 0x65 => -1,
        0x0e..=0x33 => wire - 1,
        0x35..=0x64 => wire - 2,
        0x66..=0x6d => wire - 3,
        0x6e..=0x70 => wire - 2,
        _ => wire,
    }
}

fn clientbound_765(wire: i32) -> i32 {
    match wire {
        0x00..=0x02 => wire,
        0x03..=0x0b => wire + 1,
        0x0c | 0x0d | 0x34 | 0x42 | 0x43 | 0x67 | 0x6e | 0x6f => -1,
        0x0e..=0x33 => wire - 1,
        0x35..=0x41 => wire - 2,
        0x44 => RESOURCE_PACK,
        0x45..=0x66 => wire - 4,
        0x68..=0x6d => wire - 5,
        0x70..=0x71 => wire - 7,
        0x72..=0x74 => wire - 6,
        _ => wire,
    }
}

fn clientbound_766(wire: i32) -> i32 {
    match wire {
        0x00..=0x02 => wire,
        0x03..=0x0b => wire + 1,
        0x0c | 0x0d | 0x16 | 0x1b | 0x36 | 0x44..=0x46 | 0x69 | 0x6b | 0x71..=0x73 | 0x79 => -1,
        0x0e..=0x15 => wire - 1,
        0x17..=0x1a => wire - 2,
        0x1c..=0x35 => wire - 3,
        0x37..=0x43 => wire - 4,
        0x47..=0x68 => wire - 6,
        0x6a => wire - 7,
        0x6c..=0x70 => wire - 8,
        0x74..=0x75 => wire - 11,
        0x76..=0x78 => wire - 10,
        _ => wire,
    }
}

fn clientbound_768(wire: i32) -> i32 {
    match wire {
        0x03 => ENTITY_ANIMATION,
        0x01 => SPAWN_ENTITY,
        0x07 => BLOCK_ENTITY_DATA,
        0x09 => BLOCK_CHANGE,
        0x0a => BOSS_BAR,
        0x0f => CLEAR_TITLES,
        0x13 => CONTAINER_CONTENT,
        0x15 => CONTAINER_SLOT,
        0x22 => UNLOAD_CHUNK,
        0x23 => GAME_STATE,
        0x25 => HURT_ANIMATION,
        0x26 => INIT_WORLD_BORDER,
        0x27 => KeepAlive::ID,
        0x28 => MAP_CHUNK,
        0x2a => PARTICLES,
        0x2c => JOIN_GAME,
        0x2d => MAP_DATA,
        0x2f => REL_MOVE,
        0x30 => MOVE_LOOK,
        0x32 => ENTITY_ROTATION,
        0x33 => VEHICLE_MOVE,
        0x37 => Ping::ID,
        0x3f => PLAYER_REMOVE,
        0x40 => PLAYER_INFO,
        0x42 => SynchronizePlayerPosition::ID,
        0x47 => ENTITY_DESTROY,
        0x48 => REMOVE_ENTITY_EFFECT,
        0x4c => RESPAWN,
        0x4d => ENTITY_HEAD_ROTATION,
        0x51 => ACTION_BAR,
        0x52 => WORLD_BORDER_CENTER,
        0x53 => WORLD_BORDER_LERP,
        0x54 => WORLD_BORDER_SIZE,
        0x55 => WORLD_BORDER_WARNING_DELAY,
        0x56 => WORLD_BORDER_WARNING_REACH,
        0x5c => SCOREBOARD_DISPLAY,
        0x5d => ENTITY_METADATA,
        0x5f => ENTITY_VELOCITY,
        0x60 => ENTITY_EQUIPMENT,
        0x61 => EXPERIENCE,
        0x62 => UPDATE_HEALTH,
        0x63 => SET_HELD_ITEM,
        0x64 => SCOREBOARD_OBJECTIVE,
        0x65 => SET_PASSENGERS,
        0x67 => TEAMS,
        0x68 => SCOREBOARD_SCORE,
        0x6a => TITLE_SUBTITLE,
        0x6b => UPDATE_TIME,
        0x6c => TITLE_TEXT,
        0x6d => TITLE_TIME,
        0x6e => ENTITY_SOUND,
        0x6f => SOUND,
        0x73 => SystemChat::ID,
        0x74 => PLAYER_LIST_HEADER,
        0x76 => COLLECT_ITEM,
        0x77 => ENTITY_TELEPORT,
        0x7d => ENTITY_EFFECT,
        0x7e => DECLARE_RECIPES,
        _ => -1,
    }
}

const ENTITY_DESTROY: i32 = 0x3e;

fn clientbound_770(wire: i32) -> i32 {
    match wire {
        0x00..=0x01 => clientbound_768(wire),
        0x02..=0x76 => clientbound_768(wire + 1),
        0x77 => -1,
        _ => clientbound_768(wire),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_profile_maps_known_real_insertion_points() {
        assert_eq!(
            ProtocolVersion::V1_20_2.canonical_clientbound_id(0x25),
            Some(MAP_CHUNK)
        );
        assert_eq!(
            ProtocolVersion::V1_21_2.canonical_clientbound_id(0x2c),
            Some(JOIN_GAME)
        );
        assert_eq!(
            ProtocolVersion::V1_21_5.canonical_clientbound_id(0x2b),
            Some(JOIN_GAME)
        );
        assert_eq!(
            ProtocolVersion::V1_21_5.canonical_clientbound_id(0x77),
            None
        );
        assert_eq!(
            ProtocolVersion::V1_21_5.serverbound_id(State::Play, 0x32),
            0x3f
        );
    }
}
