use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ClientCommand, ClientCore, ClientEvent, ClientSnapshot, SessionContext};

/// Current JSON replay envelope version.
pub const REPLAY_FORMAT_VERSION: u32 = 1;

/// One deterministic input in a semantic replay.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ReplayInput {
    Event(ClientEvent),
    Command(ClientCommand),
}

/// An input and the expected coherent state after applying it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplayFrame {
    pub input: ReplayInput,
    pub expected: ClientSnapshot,
}

/// Credential- and asset-free semantic session fixture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Replay {
    pub format_version: u32,
    pub protocol: crate::ProtocolVersion,
    pub frames: Vec<ReplayFrame>,
}

impl Replay {
    /// Serializes the stable, human-reviewable replay envelope.
    pub fn to_json(&self) -> Result<String, ReplayError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Parses a replay and rejects unsupported envelope versions.
    pub fn from_json(json: &str) -> Result<Self, ReplayError> {
        let replay: Self = serde_json::from_str(json)?;
        if replay.format_version != REPLAY_FORMAT_VERSION {
            return Err(ReplayError::UnsupportedVersion(replay.format_version));
        }
        Ok(replay)
    }

    /// Replays every frame and verifies its expected snapshot.
    pub fn verify(&self) -> Result<(), ReplayError> {
        let mut core = ClientCore::new(SessionContext::new(self.protocol));
        for (index, frame) in self.frames.iter().enumerate() {
            match &frame.input {
                ReplayInput::Event(event) => core.apply_event(event.clone()),
                ReplayInput::Command(command) => {
                    let _ = core.apply_command(command.clone());
                }
            }
            if core.snapshot() != &frame.expected {
                return Err(ReplayError::SnapshotMismatch { frame: index });
            }
        }
        Ok(())
    }
}

/// Builds a deterministic semantic replay using a private mirror core. Chat is
/// redacted before it reaches the mirror so opt-in diagnostic captures do not
/// retain server or player messages.
#[derive(Debug)]
pub struct ReplayRecorder {
    replay: Replay,
    mirror: ClientCore,
}

impl ReplayRecorder {
    #[must_use]
    pub fn new(context: SessionContext) -> Self {
        Self {
            replay: Replay {
                format_version: REPLAY_FORMAT_VERSION,
                protocol: context.protocol,
                frames: Vec::new(),
            },
            mirror: ClientCore::new(context),
        }
    }

    pub fn record_event(&mut self, event: ClientEvent) {
        let event = sanitize_event(event);
        self.mirror.apply_event(event.clone());
        self.replay.frames.push(ReplayFrame {
            input: ReplayInput::Event(event),
            expected: self.mirror.snapshot().clone(),
        });
    }

    pub fn record_command(&mut self, command: ClientCommand) {
        let command = sanitize_command(command);
        let _ = self.mirror.apply_command(command.clone());
        self.replay.frames.push(ReplayFrame {
            input: ReplayInput::Command(command),
            expected: self.mirror.snapshot().clone(),
        });
    }

    #[must_use]
    pub fn replay(&self) -> &Replay {
        &self.replay
    }
}

fn sanitize_event(event: ClientEvent) -> ClientEvent {
    match event {
        ClientEvent::ChatReceived(_) => ClientEvent::ChatReceived("<redacted>".to_owned()),
        event => event,
    }
}

fn sanitize_command(command: ClientCommand) -> ClientCommand {
    match command {
        ClientCommand::SendChat(_) => ClientCommand::SendChat("<redacted>".to_owned()),
        ClientCommand::EditBook { slot, pages, title } => ClientCommand::EditBook {
            slot,
            pages: vec!["<redacted>".to_owned(); pages.len()],
            title: title.map(|_| "<redacted>".to_owned()),
        },
        ClientCommand::RenameItem(_) => ClientCommand::RenameItem("<redacted>".to_owned()),
        command => command,
    }
}

#[derive(Debug, Error)]
pub enum ReplayError {
    #[error("invalid replay JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported replay format version {0}")]
    UnsupportedVersion(u32),
    #[error("snapshot mismatch at replay frame {frame}")]
    SnapshotMismatch { frame: usize },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConnectionPhase, ProtocolVersion};

    #[test]
    fn semantic_replay_roundtrips_and_verifies() {
        let protocol = ProtocolVersion::V1_20_1;
        let mut core = ClientCore::new(SessionContext::new(protocol));
        let event = ClientEvent::ConnectionPhaseChanged(ConnectionPhase::Play);
        core.apply_event(event.clone());
        let replay = Replay {
            format_version: REPLAY_FORMAT_VERSION,
            protocol,
            frames: vec![ReplayFrame {
                input: ReplayInput::Event(event),
                expected: core.snapshot().clone(),
            }],
        };
        let decoded = Replay::from_json(&replay.to_json().unwrap()).unwrap();
        assert_eq!(decoded, replay);
        decoded.verify().unwrap();
    }

    #[test]
    fn recorder_redacts_user_text_and_remains_verifiable() {
        let context = SessionContext::new(ProtocolVersion::V1_21_5);
        let mut recorder = ReplayRecorder::new(context);
        recorder.record_event(ClientEvent::ChatReceived("server secret".to_owned()));
        recorder.record_command(ClientCommand::SendChat("player secret".to_owned()));
        let json = recorder.replay().to_json().unwrap();
        assert!(!json.contains("server secret"));
        assert!(!json.contains("player secret"));
        recorder.replay().verify().unwrap();
    }
}
