use crate::timing::TurnTiming;
use crate::settings::AppSettings;

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
            State::Thinking  => "thinking",
            State::Speaking  => "speaking",
            State::Muted     => "muted",
        }
    }
}

#[derive(Clone, Debug)]
pub enum UiEvent {
    StateChanged(State),
    Turn { heard: String, refined: String, timing: TurnTiming, timestamp: String },
    Cleared,
}

/// NOTE: ControlMsg is no longer Copy because AppSettings contains a String.
/// Use .clone() at any call site that previously relied on Copy semantics.
#[derive(Clone, Debug)]
pub enum ControlMsg {
    ToggleMic,
    Clear,
    Stop,
    SettingsChanged(AppSettings),
}
