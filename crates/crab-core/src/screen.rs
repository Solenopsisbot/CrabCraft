use serde::{Deserialize, Serialize};

/// Renderer-neutral presentation modes. Detailed screen models remain in the
/// frontend, while focus, nesting, and dismissal behavior live here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UiScreen {
    Gameplay,
    Inventory,
    Chat,
    Pause,
    Options,
    Controls,
    Book,
    ServerContainer,
    ResourcePackPrompt,
}

/// Testable modal screen stack shared by graphical presentation adapters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenStack {
    screens: Vec<UiScreen>,
}

impl Default for ScreenStack {
    fn default() -> Self {
        Self {
            screens: vec![UiScreen::Gameplay],
        }
    }
}

impl ScreenStack {
    #[must_use]
    pub fn top(&self) -> UiScreen {
        self.screens.last().copied().unwrap_or(UiScreen::Gameplay)
    }

    #[must_use]
    pub fn contains(&self, screen: UiScreen) -> bool {
        self.screens.contains(&screen)
    }

    /// Opens a screen once, preserving its parent for Escape/back navigation.
    pub fn open(&mut self, screen: UiScreen) {
        if screen == UiScreen::Gameplay {
            self.close_all();
        } else if !self.contains(screen) {
            self.screens.push(screen);
        }
    }

    /// Removes a screen and every child opened above it.
    pub fn close(&mut self, screen: UiScreen) {
        if let Some(index) = self
            .screens
            .iter()
            .position(|candidate| *candidate == screen)
        {
            self.screens.truncate(index);
        }
        if self.screens.is_empty() {
            self.screens.push(UiScreen::Gameplay);
        }
    }

    pub fn toggle(&mut self, screen: UiScreen) {
        if self.contains(screen) {
            self.close(screen);
        } else {
            self.open(screen);
        }
    }

    pub fn dismiss_top(&mut self) -> UiScreen {
        if self.screens.len() > 1 {
            self.screens.pop();
        }
        self.top()
    }

    pub fn close_all(&mut self) {
        self.screens.clear();
        self.screens.push(UiScreen::Gameplay);
    }

    #[must_use]
    pub fn captures_input(&self) -> bool {
        self.top() != UiScreen::Gameplay
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_screens_close_back_to_their_parent() {
        let mut screens = ScreenStack::default();
        screens.open(UiScreen::Pause);
        screens.open(UiScreen::Options);
        assert_eq!(screens.top(), UiScreen::Options);
        assert_eq!(screens.dismiss_top(), UiScreen::Pause);
        screens.close(UiScreen::Pause);
        assert_eq!(screens.top(), UiScreen::Gameplay);
        assert!(!screens.captures_input());
    }
}
