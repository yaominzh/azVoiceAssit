use std::sync::atomic::{AtomicBool, Ordering};
use crate::events::State;

/// Pure model of the UI-control state machine (the UiBus analog).
pub struct SharedState {
    pub listening_enabled: AtomicBool,
    current: std::sync::Mutex<State>,
}

impl SharedState {
    pub fn new() -> Self {
        Self {
            listening_enabled: AtomicBool::new(true),
            current: std::sync::Mutex::new(State::Listening),
        }
    }

    pub fn current(&self) -> State {
        *self.current.lock().unwrap()
    }

    pub fn set(&self, s: State) {
        *self.current.lock().unwrap() = s;
    }

    pub fn idle_state(&self) -> State {
        if self.listening_enabled.load(Ordering::SeqCst) {
            State::Listening
        } else {
            State::Muted
        }
    }

    /// Flip the mic. Returns Some(new idle state) to repaint ONLY when idle; None mid-turn.
    pub fn toggle_mic(&self) -> Option<State> {
        let now = !self.listening_enabled.load(Ordering::SeqCst);
        self.listening_enabled.store(now, Ordering::SeqCst);
        let cur = self.current();
        if cur == State::Listening || cur == State::Muted {
            let s = if now { State::Listening } else { State::Muted };
            self.set(s);
            Some(s)
        } else {
            None
        }
    }
}

impl Default for SharedState {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_mic_when_idle_repaints_muted_then_listening() {
        let s = SharedState::new();
        assert_eq!(s.current(), State::Listening);
        assert_eq!(s.toggle_mic(), Some(State::Muted));
        assert_eq!(s.listening_enabled.load(Ordering::SeqCst), false);
        assert_eq!(s.toggle_mic(), Some(State::Listening));
        assert_eq!(s.listening_enabled.load(Ordering::SeqCst), true);
    }

    #[test]
    fn toggle_mic_mid_turn_does_not_repaint() {
        let s = SharedState::new();
        s.set(State::Speaking);
        assert_eq!(s.toggle_mic(), None);
        assert_eq!(s.listening_enabled.load(Ordering::SeqCst), false);
    }

    #[test]
    fn idle_state_reflects_mic() {
        let s = SharedState::new();
        s.toggle_mic(); // disable
        assert_eq!(s.idle_state(), State::Muted);
        s.toggle_mic(); // re-enable
        assert_eq!(s.idle_state(), State::Listening);
    }
}
