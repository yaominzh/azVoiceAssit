use crate::timing::TurnTiming;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Listening,
    Thinking,
    Speaking,
    Muted,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::Listening => "listening",
            State::Thinking => "thinking",
            State::Speaking => "speaking",
            State::Muted => "muted",
        }
    }
}

#[derive(Clone, Debug)]
pub enum UiEvent {
    StateChanged(State),
    Turn { heard: String, refined: String, timing: TurnTiming },
    Cleared,
}

#[derive(Clone, Copy, Debug)]
pub enum ControlMsg {
    ToggleMic,
    Clear,
    Stop,
}
