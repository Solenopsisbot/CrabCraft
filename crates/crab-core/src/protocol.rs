use std::str::FromStr;

use crab_registry::RegistrySet;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A Java Edition wire protocol supported by Crabcraft.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProtocolVersion {
    #[default]
    V1_20_1,
    V1_20_2,
    V1_20_3,
    V1_20_5,
    V1_21,
    V1_21_2,
    V1_21_4,
    V1_21_5,
}

impl ProtocolVersion {
    /// All supported profiles in ascending wire-version order.
    pub const ALL: [Self; 8] = [
        Self::V1_20_1,
        Self::V1_20_2,
        Self::V1_20_3,
        Self::V1_20_5,
        Self::V1_21,
        Self::V1_21_2,
        Self::V1_21_4,
        Self::V1_21_5,
    ];

    /// Numeric protocol identifier carried in the handshake.
    #[must_use]
    pub const fn number(self) -> i32 {
        match self {
            Self::V1_20_1 => 763,
            Self::V1_20_2 => 764,
            Self::V1_20_3 => 765,
            Self::V1_20_5 => 766,
            Self::V1_21 => 767,
            Self::V1_21_2 => 768,
            Self::V1_21_4 => 769,
            Self::V1_21_5 => 770,
        }
    }

    /// Whether text components use anonymous/network NBT in play packets.
    #[must_use]
    pub const fn uses_nbt_components(self) -> bool {
        self.number() >= 765
    }

    /// Whether item stacks use typed data-component patches.
    #[must_use]
    pub const fn uses_data_components(self) -> bool {
        self.number() >= 766
    }

    /// Whether configuration registries arrive as separate registry packets.
    #[must_use]
    pub const fn uses_split_registry(self) -> bool {
        self.uses_data_components()
    }

    /// Immutable generated registries associated with this wire profile.
    #[must_use]
    pub const fn registries(self) -> RegistrySet {
        RegistrySet::for_protocol(self.number())
    }
}

impl FromStr for ProtocolVersion {
    type Err = ProtocolParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "763" | "1.20" | "1.20.1" => Ok(Self::V1_20_1),
            "764" | "1.20.2" => Ok(Self::V1_20_2),
            "765" | "1.20.3" | "1.20.4" => Ok(Self::V1_20_3),
            "766" | "1.20.5" | "1.20.6" => Ok(Self::V1_20_5),
            "767" | "1.21" | "1.21.1" => Ok(Self::V1_21),
            "768" | "1.21.2" | "1.21.3" => Ok(Self::V1_21_2),
            "769" | "1.21.4" => Ok(Self::V1_21_4),
            "770" | "1.21.5" => Ok(Self::V1_21_5),
            _ => Err(ProtocolParseError(value.to_owned())),
        }
    }
}

/// An unsupported protocol number or version label.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("unsupported protocol {0}; expected 763..=770 or a supported version label")]
pub struct ProtocolParseError(pub String);

/// Immutable context shared by all adapters belonging to one session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionContext {
    pub protocol: ProtocolVersion,
    pub registries: RegistrySet,
}

impl SessionContext {
    #[must_use]
    pub const fn new(protocol: ProtocolVersion) -> Self {
        Self {
            protocol,
            registries: protocol.registries(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_parse_aliases_and_own_independent_registries() {
        let legacy = SessionContext::new("1.20.1".parse().unwrap());
        let modern = SessionContext::new("770".parse().unwrap());
        assert_eq!(legacy.protocol.number(), 763);
        assert_eq!(modern.protocol.number(), 770);
        assert!(legacy.registries.block_by_name("pale_oak_log").is_none());
        assert!(modern.registries.block_by_name("pale_oak_log").is_some());
    }
}
