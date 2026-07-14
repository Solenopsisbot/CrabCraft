//! Renderer- and transport-independent client orchestration primitives.
//!
//! [`ClientCore`] is a deterministic reducer: callers supply semantic events,
//! user commands, and ticks; the core owns authoritative session state and
//! returns explicit effects. Network and presentation adapters remain outside
//! this crate.

mod command;
mod protocol;
mod replay;
mod screen;
mod state;
mod wire;

pub use command::{ClientCommand, CommandQueue, CommandQueueError, Controls, RecipeKey};
pub use protocol::{ProtocolParseError, ProtocolVersion, SessionContext};
pub use replay::{
    Replay, ReplayError, ReplayFrame, ReplayInput, ReplayRecorder, REPLAY_FORMAT_VERSION,
};
pub use screen::{ScreenStack, UiScreen};
pub use state::{ClientCore, ClientEvent, ClientSnapshot, ConnectionPhase, CoreEffect};
