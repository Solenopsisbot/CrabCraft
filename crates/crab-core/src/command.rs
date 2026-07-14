use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Mutex;
use thiserror::Error;

/// Latest continuous player-control intent. Adapters should coalesce this
/// state rather than enqueueing every mouse or keyboard sample.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Controls {
    pub forward: f32,
    pub strafe: f32,
    pub jump: bool,
    pub sprint: bool,
    pub sneak: bool,
    pub attack: bool,
    pub use_item: bool,
    pub yaw: f32,
    pub pitch: f32,
    pub selected_slot: u8,
    pub toggle_flight: bool,
    pub swap_hands: bool,
}

/// Version-neutral recipe identity used by UI and replay code.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecipeKey {
    Namespaced(String),
    Numeric(i32),
}

/// Typed, bounded commands flowing from presentation into the client core.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClientCommand {
    SetControls(Controls),
    SendChat(String),
    ResourcePackDecision(bool),
    ResourcePackStatus(i32),
    ClickContainer {
        window_id: u8,
        slot: i16,
        button: i8,
        mode: i32,
    },
    CloseContainer(u8),
    ChooseEnchantment {
        window_id: u8,
        enchantment: i8,
    },
    PressMenuButton {
        window_id: u8,
        button_id: i8,
    },
    RenameItem(String),
    EditBook {
        slot: i32,
        pages: Vec<String>,
        title: Option<String>,
    },
    PlaceRecipe {
        window_id: i8,
        recipe: RecipeKey,
        make_all: bool,
    },
    SelectBundleItem {
        slot_id: i32,
        selected_item_index: i32,
    },
}

/// Bounded multi-producer command boundary shared by presentation adapters and
/// the single-owner session task. A full queue is reported instead of silently
/// allocating from untrusted or runaway input.
#[derive(Debug)]
pub struct CommandQueue {
    capacity: usize,
    pending: Mutex<VecDeque<ClientCommand>>,
}

impl CommandQueue {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "command queue capacity must be non-zero");
        Self {
            capacity,
            pending: Mutex::new(VecDeque::with_capacity(capacity)),
        }
    }

    /// Attempts to enqueue a command without blocking a UI thread.
    pub fn try_push(&self, command: ClientCommand) -> Result<(), CommandQueueError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| CommandQueueError::Poisoned)?;
        if pending.len() >= self.capacity {
            return Err(CommandQueueError::Full);
        }
        pending.push_back(command);
        Ok(())
    }

    /// Drains commands matching a session-state predicate while preserving the
    /// order of both selected and deferred commands.
    pub fn take_matching(
        &self,
        mut predicate: impl FnMut(&ClientCommand) -> bool,
    ) -> Result<Vec<ClientCommand>, CommandQueueError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| CommandQueueError::Poisoned)?;
        let mut selected = Vec::new();
        let mut deferred = VecDeque::with_capacity(pending.len());
        while let Some(command) = pending.pop_front() {
            if predicate(&command) {
                selected.push(command);
            } else {
                deferred.push_back(command);
            }
        }
        *pending = deferred;
        Ok(selected)
    }

    pub fn drain(&self) -> Result<Vec<ClientCommand>, CommandQueueError> {
        self.take_matching(|_| true)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.pending.lock().map_or(0, |pending| pending.len())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum CommandQueueError {
    #[error("client command queue is full")]
    Full,
    #[error("client command queue lock is poisoned")]
    Poisoned,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_queue_preserves_deferred_command_order() {
        let queue = CommandQueue::new(3);
        queue
            .try_push(ClientCommand::SendChat("one".into()))
            .unwrap();
        queue
            .try_push(ClientCommand::ResourcePackDecision(true))
            .unwrap();
        queue
            .try_push(ClientCommand::SendChat("two".into()))
            .unwrap();
        assert_eq!(
            queue.try_push(ClientCommand::CloseContainer(1)),
            Err(CommandQueueError::Full)
        );
        assert_eq!(
            queue
                .take_matching(|command| matches!(command, ClientCommand::ResourcePackDecision(_)))
                .unwrap(),
            vec![ClientCommand::ResourcePackDecision(true)]
        );
        assert_eq!(
            queue.drain().unwrap(),
            vec![
                ClientCommand::SendChat("one".into()),
                ClientCommand::SendChat("two".into())
            ]
        );
    }
}
