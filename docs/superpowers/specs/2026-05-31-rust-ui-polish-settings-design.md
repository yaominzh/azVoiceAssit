# Rust UI — Polish + Settings Panel Design Spec

**Date:** 2026-05-31
**Status:** Approved; pending implementation plan
**Builds on:** `docs/superpowers/specs/2026-05-30-rust-desktop-p0-design.md`

## Context

The Rust desktop P0 proved the loop works. The UI is functional but minimal: timing is
printed to stderr (invisible in the app), turns have no timestamps, the Thinking ☯ uses
a dot-ring overlay instead of real rotation, and there is no way to change the system
prompt without recompiling. This phase adds targeted polish and a runtime-configurable
settings panel — staying within egui's natural aesthetic rather than fighting it.

**Scope confirmed in brainstorming:** two independent sub-features:
1. **Polish & usability** — timing in-UI, timestamps, real ☯ rotation, gear button.
2. **Settings panel** — in-window drawer exposing system prompt, VAD silence timeout, and
   speech threshold. Settings persisted to `~/.config/azva/settings.json` and applied to
   the running worker without restart.

## Decisions

| Area | Decision |
|------|----------|
| Timing display | `ui.label` (green monospace) below ☯ — replaces `eprintln!` in `logic()` |
| Timestamps | `std::time::SystemTime` formatted as `HH:MM:SS` — no new crate |
| ☯ Thinking rotation | `painter.with_clip_rect(...).text(...)` + `Rot2::from_angle(t * TAU / 1.4)` |
| Settings UX | `bool show_settings` on `VoiceApp`; panel appears below control bar instantly (no animation — egui-native) |
| Settings persistence | `serde_json` → `~/.config/azva/settings.json` (created on first save) |
| Settings scope | System prompt (multiline), silence timeout ms (slider), speech threshold (slider) |
| Apply mechanism | `ControlMsg::SettingsChanged(AppSettings)` to worker — no restart required |
| Startup | Worker loads settings from disk (if exists) on startup; falls back to `config.rs` defaults |

## File changes

- `rust/src/settings.rs` — **new**: `AppSettings` struct (serde), `load()`, `save()`, `default()`.
- `rust/src/events.rs` — add `ControlMsg::SettingsChanged(AppSettings)`; add `timestamp: String` to `UiEvent::Turn`.
- `rust/src/ui.rs` — polish + settings panel rendering; `VoiceApp` gets `last_timing`, `show_settings`, `draft: AppSettings`.
- `rust/src/worker.rs` — handle `SettingsChanged`; load settings at startup; pass system prompt + thresholds from settings into refine/VAD.
- `rust/src/config.rs` — `SETTINGS_PATH` constant; existing `SYSTEM_PROMPT`, `MIN_SILENCE_MS`, `SPEECH_THRESHOLD` become fallback defaults only.
- `rust/Cargo.toml` — add `serde` features `derive` (already present) and confirm `serde_json` is available (already present via `refine.rs`).

## `AppSettings` struct

```rust
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AppSettings {
    pub system_prompt: String,
    pub silence_ms: u32,      // VAD end-of-turn silence threshold
    pub speech_threshold: f32, // Silero speech probability threshold
}

impl Default for AppSettings { /* reads from config.rs constants */ }
impl AppSettings {
    pub fn load() -> Self { /* read SETTINGS_PATH; fall back to Default on any error */ }
    pub fn save(&self) -> Result<(), String> { /* create ~/.config/azva/ if needed; write JSON */ }
}
```

## Polish details

### Timing in-UI
`UiEvent::Turn` already carries `TurnTiming`. `VoiceApp::logic()` saves the formatted
string to `self.last_timing: Option<String>` on each `Turn` event. In `ui()`, if
`last_timing` is `Some(s)`, render it as:
```
ui.label(RichText::new(s).monospace().color(Color32::from_rgb(16,185,129)).size(10))
```
directly below the status label, above the transcript. Replaces the current `eprintln!`.

### Timestamps
`UiEvent::Turn` gains `timestamp: String` (pre-formatted `HH:MM:SS`). Formatted in the
worker at the moment the turn completes (just before `tx_ui.send`):

```rust
fn format_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{:02}:{:02}:{:02}", h, m, s)
}
```

Transcript stored as `Vec<(String, String, String)>` — `(heard, refined, timestamp)`.
Timestamp rendered inline with the "heard" label: `ui.label(RichText::new(format!("heard  {ts}")).size(10).color(gray))`.

### ☯ real rotation (Thinking)
Replace the dot-ring overlay with a rotation transform on the glyph itself:

```rust
if self.state == State::Thinking {
    let angle = t as f32 * std::f32::consts::TAU / 1.4;
    let rot = egui::emath::Rot2::from_angle(angle);
    let center = rect.center();
    let offset = rot * egui::Vec2::new(0.0, 0.0); // center stays fixed
    painter.text(
        center + offset,
        egui::Align2::CENTER_CENTER,
        "☯",
        egui::FontId::proportional(96.0),
        color_a,
    );
} else {
    painter.text(rect.center(), egui::Align2::CENTER_CENTER, "☯",
        egui::FontId::proportional(96.0), color_a);
}
```

**Decision:** keep the existing dot-ring approach but make it faster (1.0s period vs 1.4s)
and tighten the orbit radius slightly. The static glyph remains; the spinning orbit
communicates "Thinking" clearly and avoids egui glyph-rotation complexity entirely.
Remove the placeholder Rot2 code above — the dot-ring *is* the Thinking animation.

## Settings panel

### State on `VoiceApp`
```rust
pub struct VoiceApp {
    // ...existing fields...
    last_timing: Option<String>,
    show_settings: bool,
    draft: AppSettings,       // editable copy while drawer is open
    applied: AppSettings,     // last-applied settings (for cancel/reset)
}
```

### Rendering
Below the control bar, when `show_settings`:
```
ui.separator();
ui.vertical(|ui| {
    ui.label("System Prompt");
    ui.add(TextEdit::multiline(&mut self.draft.system_prompt).desired_rows(4));
    ui.label("Silence timeout (ms)");
    ui.add(Slider::new(&mut self.draft.silence_ms, 300..=2000));
    ui.label("Speech threshold");
    ui.add(Slider::new(&mut self.draft.speech_threshold, 0.1..=0.9));
    if ui.button("Apply").clicked() {
        if let Err(e) = self.draft.save() { eprintln!("settings save: {e}"); }
        self.applied = self.draft.clone();
        let _ = self.tx_ctrl.send(ControlMsg::SettingsChanged(self.draft.clone()));
        self.show_settings = false;
    }
    if ui.button("Cancel").clicked() {
        self.draft = self.applied.clone();
        self.show_settings = false;
    }
});
```

### Worker handling
```rust
Ok(ControlMsg::SettingsChanged(s)) => {
    system_prompt = s.system_prompt.clone();
    // Pass updated thresholds into vad for next accept() call
    vad.set_thresholds(s.silence_ms, s.speech_threshold);
    settings = s;
}
```

`Vad` needs a `set_thresholds(silence_ms, threshold)` method that updates its fields.

## Testing

- **`format_timestamp` unit test** — known epoch value → expected `HH:MM:SS` string.
- **`AppSettings` roundtrip** — serialize to a temp file, load it back, assert equality.
- **`AppSettings::default()`** — values match `config.rs` constants.
- **`SettingsChanged` handled by worker** — inject the message via the ctrl channel; assert the worker's `system_prompt` string and VAD thresholds update correctly (requires a mock/injectable VAD — or just test the `Vad::set_thresholds` method in isolation).
- Manual: change system prompt to `"Reply in ALL CAPS."`, click Apply, speak — confirm the refined output is uppercase.

## Out of scope

Menu-bar/tray, global hotkey, compact orb mode, font customization, theme switching,
RAG knowledge grounding (separate phase).
