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

/// Tauri event payloads — framework-neutral structs emitted by the bridge thread.
/// Kept separate from UiEvent/ControlMsg so worker.rs stays unaware of Tauri.
#[derive(Clone, serde::Serialize)]
pub struct StatePayload {
    pub value: &'static str,
}

#[derive(Clone, serde::Serialize)]
pub struct TurnPayload {
    pub heard: String,
    pub refined: String,
    pub timestamp: String,
    pub endpoint_ms: u32,
    pub stt_ms: u32,
    pub refine_ms: u32,
    pub reply_start_ms: u32,
}
