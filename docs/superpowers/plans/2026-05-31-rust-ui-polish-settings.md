# Rust UI Polish + Settings Panel — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add per-turn timing display, UTC timestamps, a faster Thinking dot-ring, and a runtime settings panel (system prompt + VAD knobs, persisted to `~/.config/azva/settings.json`) to the Rust desktop voice assistant.

**Architecture:** New `settings.rs` owns `AppSettings` (serde, load/save/validate). `events.rs` gains `UiEvent::Turn { timestamp }` and `ControlMsg::SettingsChanged`. `vad.rs` gains `set_thresholds`. `worker.rs` loads settings at startup and handles `SettingsChanged` at runtime. `ui.rs` renders timing/timestamps/gear button/settings panel — all pure `ui.label`/`Slider`/`TextEdit`, no egui fighting.

**Tech Stack:** Rust, egui 0.34, serde_json (already in Cargo.toml), std::time::SystemTime (no new deps).

**Branch:** `feat/rust-ui-polish-settings`. **DO NOT `git push` without checking** (previously there was an org guardrail; it's now lifted — push is fine).

---

## File structure

| File | Change |
|------|--------|
| `rust/src/settings.rs` | **CREATE** — `AppSettings`, `load`, `save`, `validate`, `Default` |
| `rust/src/config.rs` | **MODIFY** — add `SETTINGS_PATH` constant |
| `rust/src/events.rs` | **MODIFY** — add `timestamp: String` to `UiEvent::Turn`; add `ControlMsg::SettingsChanged(AppSettings)` |
| `rust/src/vad.rs` | **MODIFY** — add `silence_ms`/`speech_threshold` fields; add `set_thresholds` |
| `rust/src/worker.rs` | **MODIFY** — load settings at startup; handle `SettingsChanged`; pass `system_prompt` and thresholds to VAD/refine; add `format_timestamp`; add `timestamp` to `UiEvent::Turn` |
| `rust/src/ui.rs` | **MODIFY** — `VoiceApp` gets `last_timing`, `show_settings`, `draft`, `applied`; render timing badge, timestamps, faster dot-ring, gear button, settings panel |
| `rust/src/lib.rs` | **MODIFY** — add `pub mod settings;` |
| `rust/src/main.rs` | **MODIFY** — add `mod settings;` |

---

## Task 1: `AppSettings` — pure struct, TDD

**Files:**
- Create: `rust/src/settings.rs`
- Modify: `rust/src/config.rs` (add `SETTINGS_PATH`)
- Modify: `rust/src/lib.rs` and `rust/src/main.rs` (add `pub mod settings` / `mod settings`)
- Test: already in `rust/src/settings.rs` `#[cfg(test)]`

- [ ] **Step 1: Add `SETTINGS_PATH` to `rust/src/config.rs`**

Append one line to the existing constants:
```rust
pub const SETTINGS_PATH: &str = "~/.config/azva/settings.json";
```

Wait — `~` is not expanded by Rust's stdlib. Use this instead:
```rust
pub fn settings_path() -> std::path::PathBuf {
    dirs_or_home().join(".config").join("azva").join("settings.json")
}

fn dirs_or_home() -> std::path::PathBuf {
    // No extra crates: read $HOME directly.
    std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
}
```

Add this to the bottom of `rust/src/config.rs`. (It returns a `PathBuf`, not a `&str`, so callers do `config::settings_path()`.)

- [ ] **Step 2: Write the failing tests**

Create `rust/src/settings.rs` with tests first, no `AppSettings` impl yet:

```rust
use crate::config::{MIN_SILENCE_MS, SPEECH_THRESHOLD, SYSTEM_PROMPT};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AppSettings {
    pub system_prompt: String,
    pub silence_ms: u32,
    pub speech_threshold: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_config_constants() {
        let s = AppSettings::default();
        assert_eq!(s.system_prompt, SYSTEM_PROMPT);
        assert_eq!(s.silence_ms, MIN_SILENCE_MS);
        assert!((s.speech_threshold - SPEECH_THRESHOLD).abs() < 1e-6);
    }

    #[test]
    fn validate_clamps_out_of_range() {
        let s = AppSettings {
            system_prompt: "x".into(),
            silence_ms: 99_999,
            speech_threshold: 5.0,
        };
        let v = s.validate();
        assert_eq!(v.silence_ms, 2000);
        assert!((v.speech_threshold - 0.9).abs() < 1e-6);
    }

    #[test]
    fn validate_leaves_valid_values_unchanged() {
        let s = AppSettings {
            system_prompt: "ok".into(),
            silence_ms: 700,
            speech_threshold: 0.5,
        };
        let v = s.clone().validate();
        assert_eq!(v, s);
    }

    #[test]
    fn roundtrip_json() {
        let s = AppSettings::default();
        let json = serde_json::to_string(&s).unwrap();
        let back: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join("azva_test_settings");
        let path = dir.join("settings.json");
        let s = AppSettings {
            system_prompt: "Test prompt".into(),
            silence_ms: 500,
            speech_threshold: 0.6,
        };
        s.save_to(&path).unwrap();
        let loaded = AppSettings::load_from(&path);
        assert_eq!(loaded, s);
    }
}
```

- [ ] **Step 3: Run, expect fail**

```bash
cd /Users/allen/repo/azVoiceAssist/rust
cargo test settings 2>&1 | tail -15
```
Expected: FAIL — methods `default`, `validate`, `save_to`, `load_from` not found.

- [ ] **Step 4: Implement `AppSettings`**

Add the implementation above the `#[cfg(test)]` block in `rust/src/settings.rs`:

```rust
impl Default for AppSettings {
    fn default() -> Self {
        Self {
            system_prompt: crate::config::SYSTEM_PROMPT.to_string(),
            silence_ms: crate::config::MIN_SILENCE_MS,
            speech_threshold: crate::config::SPEECH_THRESHOLD,
        }
    }
}

impl AppSettings {
    /// Clamp numeric fields to valid ranges. Called after deserialization.
    pub fn validate(mut self) -> Self {
        self.silence_ms = self.silence_ms.clamp(300, 2000);
        self.speech_threshold = self.speech_threshold.clamp(0.1, 0.9);
        self
    }

    /// Load from the given path, validating after. Returns Default on any error.
    pub fn load_from(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
            .validate()
    }

    /// Load from the canonical settings path.
    pub fn load() -> Self {
        Self::load_from(&crate::config::settings_path())
    }

    /// Persist to the given path, creating parent dirs as needed.
    pub fn save_to(&self, path: &std::path::Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }

    /// Persist to the canonical settings path.
    pub fn save(&self) -> Result<(), String> {
        self.save_to(&crate::config::settings_path())
    }
}
```

- [ ] **Step 5: Wire the module**

Add `pub mod settings;` to `rust/src/lib.rs`.
Add `mod settings;` to `rust/src/main.rs`.

- [ ] **Step 6: Run tests, expect pass**

```bash
cd /Users/allen/repo/azVoiceAssist/rust
cargo test settings 2>&1 | tail -5
```
Expected: `test result: ok. 5 passed`.

Then run the full suite:
```bash
cargo test --lib 2>&1 | tail -3
```
Expected: all 14 + 5 = 19 passed.

- [ ] **Step 7: Commit**

```bash
cd /Users/allen/repo/azVoiceAssist
git add rust/src/settings.rs rust/src/config.rs rust/src/lib.rs rust/src/main.rs
git commit -m "feat: AppSettings (load/save/validate/roundtrip) — 5 tests"
```

---

## Task 2: `Vad::set_thresholds` + add fields to `Vad`

**Files:**
- Modify: `rust/src/vad.rs`

Currently `vad.rs` reads `MIN_SILENCE_MS` and `SPEECH_THRESHOLD` directly from `config.rs` inside `accept()` on every call. We need to store them as instance fields so `set_thresholds` can update them at runtime.

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)]` block in `rust/src/vad.rs`:

```rust
    #[test]
    fn set_thresholds_updates_fields() {
        let model_path = concat!(env!("CARGO_MANIFEST_DIR"), "/models/silero_vad.onnx");
        let mut vad = Vad::load(model_path).expect("load VAD");
        vad.set_thresholds(1000, 0.7);
        assert_eq!(vad.silence_ms, 1000);
        assert!((vad.speech_threshold - 0.7).abs() < 1e-6);
    }
```

- [ ] **Step 2: Run, expect fail**

```bash
cd /Users/allen/repo/azVoiceAssist/rust
cargo test set_thresholds 2>&1 | tail -10
```
Expected: FAIL — `silence_ms` / `speech_threshold` fields don't exist yet.

- [ ] **Step 3: Add fields to `Vad` and implement `set_thresholds`**

In `rust/src/vad.rs`, add two fields to the `Vad` struct:
```rust
pub struct Vad {
    session: Session,
    state: Array3<f32>,
    context: Vec<f32>,
    silence_frames: u32,
    speech_active: bool,
    pub silence_ms: u32,         // ← new; runtime-adjustable
    pub speech_threshold: f32,   // ← new; runtime-adjustable
}
```

Update `Vad::load` to initialise them from config:
```rust
    pub fn load(model_path: &str) -> Result<Self, String> {
        let session = Session::builder()
            .map_err(|e| format!("ort builder: {e}"))?
            .commit_from_file(model_path)
            .map_err(|e| format!("ort load: {e}"))?;
        Ok(Self {
            session,
            state: Array3::zeros([2, 1, 128]),
            context: vec![0.0; CTX],
            silence_frames: 0,
            speech_active: false,
            silence_ms: crate::config::MIN_SILENCE_MS,
            speech_threshold: crate::config::SPEECH_THRESHOLD,
        })
    }
```

Add `set_thresholds` after `reset()`:
```rust
    pub fn set_thresholds(&mut self, silence_ms: u32, speech_threshold: f32) {
        self.silence_ms = silence_ms;
        self.speech_threshold = speech_threshold;
    }
```

Update `accept()` to use instance fields instead of the config constants. Find this block:
```rust
        // Silence threshold in frames
        let silence_threshold = (MIN_SILENCE_MS as f32 / 1000.0
            * SAMPLE_RATE as f32
            / FRAME as f32) as u32;

        // State machine
        if prob >= SPEECH_THRESHOLD {
```
Replace with:
```rust
        // Silence threshold in frames — uses runtime-adjustable fields
        let silence_threshold = (self.silence_ms as f32 / 1000.0
            * SAMPLE_RATE as f32
            / FRAME as f32) as u32;

        // State machine
        if prob >= self.speech_threshold {
```

Also remove `MIN_SILENCE_MS` and `SPEECH_THRESHOLD` from the `use crate::config::` import at the top of `vad.rs` since they're no longer used as constants there. Keep `FRAME` and `SAMPLE_RATE`.

- [ ] **Step 4: Run, expect pass**

```bash
cd /Users/allen/repo/azVoiceAssist/rust
cargo test 2>&1 | tail -3
```
Expected: all 20 pass.

- [ ] **Step 5: Commit**

```bash
cd /Users/allen/repo/azVoiceAssist
git add rust/src/vad.rs
git commit -m "feat: Vad::set_thresholds + runtime silence_ms/speech_threshold fields"
```

---

## Task 3: `format_timestamp` + update `UiEvent::Turn` + `ControlMsg`

**Files:**
- Modify: `rust/src/events.rs`
- Modify: `rust/src/worker.rs` (add `format_timestamp` function + timestamp to Turn send)

- [ ] **Step 1: Write the failing test for `format_timestamp`**

Add this test to `rust/src/worker.rs` (inside a `#[cfg(test)]` block at the bottom, or a new `tests` module):

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn format_timestamp_known_epoch() {
        // UNIX epoch 0 = 00:00:00 UTC 1970-01-01.
        // 3661 seconds = 1h 1m 1s UTC.
        use std::time::{Duration, UNIX_EPOCH};
        let t = UNIX_EPOCH + Duration::from_secs(3661);
        assert_eq!(super::format_timestamp_at(t), "01:01:01");
    }

    #[test]
    fn format_timestamp_midnight_rollover() {
        use std::time::{Duration, UNIX_EPOCH};
        // 86400 seconds = exactly one day → 00:00:00
        let t = UNIX_EPOCH + Duration::from_secs(86400);
        assert_eq!(super::format_timestamp_at(t), "00:00:00");
    }
}
```

- [ ] **Step 2: Run, expect fail**

```bash
cd /Users/allen/repo/azVoiceAssist/rust
cargo test format_timestamp 2>&1 | tail -10
```
Expected: FAIL — `format_timestamp_at` not found.

- [ ] **Step 3: Add `format_timestamp_at` to `worker.rs`**

Add this free function near the top of `worker.rs` (after the `use` declarations):
```rust
/// Format a SystemTime as HH:MM:SS UTC. No chrono dep.
pub fn format_timestamp_at(t: std::time::SystemTime) -> String {
    let secs = t
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{:02}:{:02}:{:02}", (secs / 3600) % 24, (secs / 60) % 60, secs % 60)
}

fn format_timestamp() -> String {
    format_timestamp_at(std::time::SystemTime::now())
}
```

- [ ] **Step 4: Update `events.rs` — add `timestamp` to `Turn` and add `SettingsChanged`**

Edit `rust/src/events.rs` to:
1. Import `AppSettings` from settings.
2. Add `timestamp` field to `UiEvent::Turn`.
3. Add `SettingsChanged` variant to `ControlMsg`.

Full updated file:
```rust
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

#[derive(Clone, Debug)]
pub enum ControlMsg {
    ToggleMic,
    Clear,
    Stop,
    SettingsChanged(AppSettings),
}
```

Note: `ControlMsg` loses `Copy` because `AppSettings` contains a `String`. Remove `#[derive(Clone, Copy, Debug)]` and use just `#[derive(Clone, Debug)]`. Fix any `.copied()` call-sites if the compiler flags them.

- [ ] **Step 5: Update `worker.rs` — wire `timestamp` into `UiEvent::Turn`**

Find the `tx_ui.send(UiEvent::Turn { ... })` call in `worker.rs` (around line 122) and add `timestamp`:
```rust
        let _ = tx_ui.send(UiEvent::Turn {
            heard: text.clone(),
            refined: refined.clone(),
            timing,
            timestamp: format_timestamp(),
        });
```

- [ ] **Step 6: Fix compile errors and run full suite**

```bash
cd /Users/allen/repo/azVoiceAssist/rust
cargo build 2>&1 | grep "^error" | head -20
```

Fix any remaining compile errors (likely `ui.rs` destructuring `UiEvent::Turn` without `timestamp`). For now, just add `timestamp: _` to any match arm in `ui.rs` that destructures `Turn`:
```rust
UiEvent::Turn { heard, refined, timing, timestamp: _ } => { ... }
```
(The full ui.rs changes come in Task 5.)

Then:
```bash
cargo test 2>&1 | tail -3
```
Expected: all 22 pass.

- [ ] **Step 7: Commit**

```bash
cd /Users/allen/repo/azVoiceAssist
git add rust/src/events.rs rust/src/worker.rs
git commit -m "feat: format_timestamp (UTC, tested), timestamp in UiEvent::Turn, SettingsChanged"
```

---

## Task 4: Wire settings into worker startup + `SettingsChanged` handler

**Files:**
- Modify: `rust/src/worker.rs`

- [ ] **Step 1: Update `worker.rs` to load settings on startup and handle `SettingsChanged`**

At the top of `worker::run`, after the existing variable setup, add:
```rust
    // Load persisted settings (or defaults). Apply initial thresholds to VAD.
    let mut settings = crate::settings::AppSettings::load();
    let mut system_prompt = settings.system_prompt.clone();
    vad.set_thresholds(settings.silence_ms, settings.speech_threshold);
```

Remove the direct use of `SYSTEM_PROMPT` from the `use crate::config::` import line (we now use the runtime `system_prompt` variable instead). Keep other imports.

Update `history.record_user_and_build` to use the variable instead of the constant:
```rust
        let messages = history.record_user_and_build(&text, &system_prompt);
```
(Previously `SYSTEM_PROMPT` — now `&system_prompt`.)

Also update `MIN_SILENCE_MS` in the `TurnTiming` struct — the timing endpoint should reflect the runtime setting:
```rust
        let timing = TurnTiming {
            endpoint_ms: settings.silence_ms,   // was MIN_SILENCE_MS
            stt_ms,
            refine_ms,
            reply_start_ms,
        };
```

Add `SettingsChanged` to both ctrl-drain loops (the initial `loop` and the `select!` arm). In the `loop { match rx_ctrl.try_recv() }` block, add:
```rust
                Ok(ControlMsg::SettingsChanged(s)) => {
                    system_prompt = s.system_prompt.clone();
                    vad.set_thresholds(s.silence_ms, s.speech_threshold);
                    settings = s;
                }
```

And in the `select! { recv(rx_ctrl) -> msg }` arm, add the same match arm.

- [ ] **Step 2: Build and verify no new test failures**

```bash
cd /Users/allen/repo/azVoiceAssist/rust
cargo build 2>&1 | grep "^error" | head -10
cargo test 2>&1 | tail -3
```
Expected: builds clean, 22 passed.

- [ ] **Step 3: Commit**

```bash
cd /Users/allen/repo/azVoiceAssist
git add rust/src/worker.rs
git commit -m "feat: worker loads AppSettings on startup, handles SettingsChanged at runtime"
```

---

## Task 5: `ui.rs` — timing badge, timestamps, faster dot-ring, gear + settings panel

**Files:**
- Modify: `rust/src/ui.rs`

This is the biggest UI change. Do it in one step since all parts are tightly coupled to `VoiceApp`'s new fields.

- [ ] **Step 1: Replace `ui.rs` entirely**

```rust
use crossbeam_channel::{Receiver, Sender};
use crate::events::{ControlMsg, State, UiEvent};
use crate::settings::AppSettings;

pub struct VoiceApp {
    rx_ui: Receiver<UiEvent>,
    tx_ctrl: Sender<ControlMsg>,
    state: State,
    transcript: Vec<(String, String, String)>, // (heard, refined, timestamp)
    last_timing: Option<String>,
    show_settings: bool,
    draft: AppSettings,
    applied: AppSettings,
}

impl VoiceApp {
    pub fn new(rx_ui: Receiver<UiEvent>, tx_ctrl: Sender<ControlMsg>) -> Self {
        let settings = AppSettings::load();
        Self {
            rx_ui,
            tx_ctrl,
            state: State::Listening,
            transcript: Vec::new(),
            last_timing: None,
            show_settings: false,
            draft: settings.clone(),
            applied: settings,
        }
    }
}

impl eframe::App for VoiceApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(event) = self.rx_ui.try_recv() {
            match event {
                UiEvent::StateChanged(s) => self.state = s,
                UiEvent::Turn { heard, refined, timing, timestamp } => {
                    self.last_timing = Some(timing.format());
                    self.transcript.push((heard, refined, timestamp));
                    if self.transcript.len() > 100 {
                        self.transcript.remove(0);
                    }
                }
                UiEvent::Cleared => {
                    self.transcript.clear();
                    self.last_timing = None;
                }
            }
        }
        ctx.request_repaint();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let t = ui.ctx().input(|i| i.time);

        let color = match self.state {
            State::Listening => egui::Color32::from_rgb(59, 130, 246),
            State::Thinking  => egui::Color32::from_rgb(168, 85, 247),
            State::Speaking  => egui::Color32::from_rgb(34, 197, 94),
            State::Muted     => egui::Color32::from_gray(100),
        };

        let alpha: u8 = match self.state {
            State::Listening => {
                let s = (t as f32 * std::f32::consts::TAU / 1.8).sin();
                ((0.5 + 0.5 * s) * 127.0 + 128.0).min(255.0) as u8
            }
            State::Speaking => {
                let s = (t as f32 * std::f32::consts::TAU / 1.2).sin();
                ((0.5 + 0.5 * s) * 127.0 + 128.0).min(255.0) as u8
            }
            _ => 255,
        };
        let color_a = egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha);

        ui.vertical_centered(|ui| {
            ui.add_space(24.0);

            let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(120.0), egui::Sense::hover());
            let painter = ui.painter();

            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "\u{262F}",
                egui::FontId::proportional(96.0),
                color_a,
            );

            // Thinking: faster dot-ring (1.0 s period, was 1.4 s)
            if self.state == State::Thinking {
                let angle = t as f32 * std::f32::consts::TAU / 1.0;
                let radius = 54.0_f32;
                let dot_color = egui::Color32::from_rgba_unmultiplied(168, 85, 247, 200);
                for i in 0..8u32 {
                    let a = angle + i as f32 * std::f32::consts::TAU / 8.0;
                    let dot_pos = rect.center()
                        + egui::Vec2::new(a.cos() * radius, a.sin() * radius);
                    let dot_r = (3.0 - i as f32 * 0.25).max(0.5);
                    painter.circle_filled(dot_pos, dot_r, dot_color);
                }
            }

            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(self.state.label().to_uppercase())
                    .size(13.0)
                    .color(egui::Color32::from_rgba_unmultiplied(229, 231, 235, 178)),
            );

            // Timing badge — shown after first turn, updated each turn
            if let Some(timing) = &self.last_timing {
                ui.label(
                    egui::RichText::new(timing)
                        .monospace()
                        .size(10.0)
                        .color(egui::Color32::from_rgb(16, 185, 129)),
                );
            }

            ui.add_space(8.0);
        });

        // Transcript
        let screen_height = ui.ctx().content_rect().height();
        let reserved = if self.show_settings { 260.0 } else { 120.0 };
        let remaining = (screen_height - reserved).max(80.0);
        egui::ScrollArea::vertical()
            .max_height(remaining)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                let width = ui.available_width().min(640.0);
                ui.set_min_width(width);
                for (heard, refined, ts) in &self.transcript {
                    ui.group(|ui| {
                        ui.label(
                            egui::RichText::new(format!("heard  {ts}"))
                                .size(10.0)
                                .color(egui::Color32::from_gray(120)),
                        );
                        ui.label(
                            egui::RichText::new(heard)
                                .size(14.0)
                                .color(egui::Color32::from_gray(200)),
                        );
                    });
                    ui.group(|ui| {
                        ui.label(
                            egui::RichText::new("refined")
                                .size(10.0)
                                .color(egui::Color32::from_gray(120)),
                        );
                        ui.label(
                            egui::RichText::new(refined)
                                .size(14.0)
                                .color(egui::Color32::from_rgb(147, 197, 253)),
                        );
                    });
                    ui.add_space(4.0);
                }
            });

        // Controls + gear button
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("\u{1F3A4} Mic").clicked() {
                let _ = self.tx_ctrl.send(ControlMsg::ToggleMic);
            }
            if ui.button("\u{23F9} Stop").clicked() {
                let _ = self.tx_ctrl.send(ControlMsg::Stop);
            }
            if ui.button("\u{1F5D1} Clear").clicked() {
                let _ = self.tx_ctrl.send(ControlMsg::Clear);
            }
            // Right-align the gear
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("\u{2699}").clicked() {
                    self.show_settings = !self.show_settings;
                    if self.show_settings {
                        self.draft = self.applied.clone(); // fresh copy on open
                    }
                }
            });
        });

        // Settings panel (appears below controls when show_settings = true)
        if self.show_settings {
            ui.separator();
            ui.vertical(|ui| {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("System Prompt")
                        .size(11.0)
                        .color(egui::Color32::from_gray(160)),
                );
                ui.add(
                    egui::TextEdit::multiline(&mut self.draft.system_prompt)
                        .desired_rows(4)
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!("Silence timeout: {} ms", self.draft.silence_ms))
                        .size(11.0)
                        .color(egui::Color32::from_gray(160)),
                );
                ui.add(egui::Slider::new(&mut self.draft.silence_ms, 300_u32..=2000).show_value(false));
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(format!("Speech threshold: {:.2}", self.draft.speech_threshold))
                        .size(11.0)
                        .color(egui::Color32::from_gray(160)),
                );
                ui.add(egui::Slider::new(&mut self.draft.speech_threshold, 0.1_f32..=0.9_f32).show_value(false));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Apply").clicked() {
                        match self.draft.save() {
                            Ok(()) => {
                                self.applied = self.draft.clone();
                                let _ = self.tx_ctrl.send(
                                    ControlMsg::SettingsChanged(self.draft.clone()));
                                self.show_settings = false;
                            }
                            Err(e) => eprintln!("Settings save failed: {e}"),
                        }
                    }
                    if ui.button("Defaults").clicked() {
                        self.draft = AppSettings::default();
                    }
                    if ui.button("Cancel").clicked() {
                        self.show_settings = false;
                    }
                });
            });
        }
    }
}
```

- [ ] **Step 2: Build**

```bash
cd /Users/allen/repo/azVoiceAssist/rust
cargo build 2>&1 | grep "^error" | head -20
```
Expected: clean (0 errors). Fix any lingering import or destructure issues.

- [ ] **Step 3: Run all tests**

```bash
cargo test 2>&1 | tail -3
```
Expected: 22 passed (no regressions — `ui.rs` has no unit tests).

- [ ] **Step 4: Commit**

```bash
cd /Users/allen/repo/azVoiceAssist
git add rust/src/ui.rs
git commit -m "feat: timing badge, UTC timestamps, faster dot-ring, gear + settings panel in UI"
```

---

## Task 6: End-to-end manual verification

**Files:** none. Requires oMLX (`:8002`) + Qwen3-TTS (`:8123`) running.

- [ ] **Step 1: Launch the app**

```bash
cd /Users/allen/repo/azVoiceAssist/rust && cargo run
```

- [ ] **Step 2: Verify timing badge**
Speak a sentence, pause. Below the ☯ a green monospace line should appear:
`↻ 0.7s  stt 90ms  refine 430ms`
(or similar). The eprintln is gone.

- [ ] **Step 3: Verify timestamps**
The `heard` label now shows `heard  09:41:22` (UTC). Two turns should show different times.

- [ ] **Step 4: Verify dot-ring speed**
During Thinking the orbit spins noticeably faster than before (~40% faster).

- [ ] **Step 5: Open settings**
Click ⚙ in the top-right of the control bar. A settings panel appears with the system prompt text area, two sliders, and Apply/Defaults/Cancel buttons.

- [ ] **Step 6: Change system prompt and Apply**
Edit the prompt to: `Reply in ALL CAPS. No other text.`
Click **Apply**. Speak a sentence. The refined output should be uppercase.

- [ ] **Step 7: Verify Defaults**
Open settings, click **Defaults**. The system prompt should reset to the original "You are a refinement assistant..." text.

- [ ] **Step 8: Verify persistence**
Quit with Cmd-Q. Relaunch `cargo run`. Open settings — the last-saved settings should still be there (loaded from `~/.config/azva/settings.json`).

- [ ] **Step 9: Commit any final fixes, push**

```bash
cd /Users/allen/repo/azVoiceAssist
git push -u origin feat/rust-ui-polish-settings
```

---

## Notes for the implementer

- **Branch:** `feat/rust-ui-polish-settings`. Never commit to `main` directly.
- `ControlMsg` loses `Copy` in Task 3 because `AppSettings` contains a `String`. The compiler will flag any `.copied()` or implicit-copy use sites — change them to `.clone()` or restructure.
- `format_timestamp_at` is `pub` (not `pub(crate)`) so it's accessible from the test module without a `super::` path issue.
- The settings path uses `$HOME` expansion manually — no `dirs` crate needed.
- **Two-stage test run pattern:** always run `cargo test settings`, `cargo test vad`, etc. targeted first, then `cargo test 2>&1 | tail -3` for the full suite.
