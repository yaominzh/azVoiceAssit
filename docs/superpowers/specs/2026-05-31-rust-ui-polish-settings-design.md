# Rust UI — Polish + Settings Panel Design Spec

**Date:** 2026-05-31
**Status:** Approved (rev 2 — third-party review incorporated); pending implementation plan
**Builds on:** `docs/superpowers/specs/2026-05-30-rust-desktop-p0-design.md`

## Context

The Rust desktop P0 proved the loop works. The UI is functional but minimal: timing is
printed to stderr (invisible in the app), turns have no timestamps, the Thinking ☯ uses
a dot-ring overlay instead of real rotation, and there is no way to change the system
prompt without recompiling. This phase adds targeted polish and a runtime-configurable
settings panel — staying within egui's natural aesthetic rather than fighting it.

**Scope:** two independent sub-features:
1. **Polish & usability** — timing in-UI, timestamps, faster dot-ring, gear button.
2. **Settings panel** — in-window drawer exposing system prompt, VAD silence timeout, and
   speech threshold. Settings persisted to `~/.config/azva/settings.json` and applied to
   the running worker without restart.

## Decisions

| Area | Decision |
|------|----------|
| Timing display | `ui.label` (green monospace) below ☯ — replaces `eprintln!` in `logic()` |
| Timestamps | `std::time::SystemTime` formatted as `HH:MM:SS` **UTC** — no new crate (see §Timestamps) |
| ☯ Thinking animation | Existing dot-ring, faster period (1.0 s vs 1.4 s) — no glyph rotation |
| Settings UX | `bool show_settings` on `VoiceApp`; panel appears below control bar instantly |
| Settings persistence | `serde_json` → `~/.config/azva/settings.json` (created on first save) |
| Settings scope | System prompt, silence timeout ms, speech threshold |
| Apply mechanism | `ControlMsg::SettingsChanged(AppSettings)` to worker — no restart required |
| Apply error handling | Only mark applied + send to worker if `save()` succeeds; log to stderr on failure |
| Draft freshness | Draft is refreshed from `applied` each time the panel opens |
| Startup | Worker loads settings from disk on startup; `validate()` on load; falls back to `config.rs` defaults |
| Revert to defaults | "Defaults" button resets draft to `AppSettings::default()` (one line, zero cost) |

## File changes

- `rust/src/settings.rs` — **new**: `AppSettings` struct (serde), `load()`, `save()`, `validate()`, `Default`.
- `rust/src/events.rs` — add `ControlMsg::SettingsChanged(AppSettings)`; add `timestamp: String` to `UiEvent::Turn`.
- `rust/src/ui.rs` — polish + settings panel; `VoiceApp` gets `last_timing`, `show_settings`, `draft`, `applied`.
- `rust/src/worker.rs` — handle `SettingsChanged`; load settings at startup; use settings values in refine/VAD.
- `rust/src/vad.rs` — add `Vad::set_thresholds(&mut self, silence_ms: u32, speech_threshold: f32)`.
- `rust/src/config.rs` — add `SETTINGS_PATH`; existing `SYSTEM_PROMPT`, `MIN_SILENCE_MS`, `SPEECH_THRESHOLD` become fallback defaults only.
- `rust/Cargo.toml` — `serde` (derive, already present) + `serde_json` (already present). No new crates.

## `AppSettings` struct

```rust
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AppSettings {
    pub system_prompt: String,
    pub silence_ms: u32,        // VAD end-of-turn silence, 300–2000
    pub speech_threshold: f32,  // Silero threshold, 0.1–0.9
}

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
    /// Load from SETTINGS_PATH; validate ranges; fall back to Default on any error.
    pub fn load() -> Self {
        std::fs::read_to_string(crate::config::SETTINGS_PATH)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
            .validate()
    }

    /// Clamp all numeric fields to their valid ranges (review #3).
    /// Called after load() so a corrupted JSON can't crash the VAD.
    fn validate(mut self) -> Self {
        self.silence_ms = self.silence_ms.clamp(300, 2000);
        self.speech_threshold = self.speech_threshold.clamp(0.1, 0.9);
        self
    }

    /// Write to SETTINGS_PATH, creating ~/.config/azva/ if needed.
    pub fn save(&self) -> Result<(), String> {
        let path = std::path::Path::new(crate::config::SETTINGS_PATH);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }
}
```

## `Vad::set_thresholds` (review #2 — previously missing)

```rust
impl Vad {
    pub fn set_thresholds(&mut self, silence_ms: u32, speech_threshold: f32) {
        self.silence_ms = silence_ms;
        self.speech_threshold = speech_threshold;
    }
}
```

`Vad` currently stores `silence_frames` as a derived value; this method should update the
source `silence_ms` and `speech_threshold` fields directly (the silence frame threshold is
recomputed inside `accept()` on each call, so no special recalculation needed).

## Polish details

### Timing in-UI
`VoiceApp::logic()` stores the formatted timing string in `self.last_timing: Option<String>`
on each `UiEvent::Turn`. In `ui()`, render below the status label:
```rust
if let Some(t) = &self.last_timing {
    ui.label(RichText::new(t).monospace().color(Color32::from_rgb(16, 185, 129)).size(10));
}
```
This replaces the current `eprintln!` entirely.

### Timestamps (UTC — review #1)
Timestamps are **UTC** (no `chrono` dependency). This is documented explicitly so users
are not surprised by the mismatch with local time. Adding `chrono` is deferred to a
follow-up if local time is requested.

```rust
fn format_timestamp() -> String {
    // Returns UTC time (HH:MM:SS). No chrono dep.
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    format!("{:02}:{:02}:{:02}", (secs / 3600) % 24, (secs / 60) % 60, secs % 60)
}
```

`UiEvent::Turn` gains `timestamp: String`. Transcript stored as
`Vec<(String, String, String)>` — `(heard, refined, timestamp)`. Timestamp rendered
inline with the "heard" label.

### ☯ Thinking animation
The **existing dot-ring** is kept. Change: period from 1.4 s → **1.0 s** (faster spin
signals "busy" more clearly). No glyph rotation — egui can't rotate text glyphs without
a custom mesh, and the dot-ring communicates the same intent cleanly.

## Settings panel

### `VoiceApp` state
```rust
pub struct VoiceApp {
    // existing fields...
    last_timing: Option<String>,
    show_settings: bool,
    draft: AppSettings,    // editable copy, refreshed on each open
    applied: AppSettings,  // last successfully applied settings
}
```

### Opening the panel (review #4 — fresh draft on open)
```rust
if ui.small_button("⚙").clicked() {
    self.show_settings = !self.show_settings;
    if self.show_settings {
        self.draft = self.applied.clone(); // always start from current applied
    }
}
```

### Rendering
```rust
if self.show_settings {
    ui.separator();
    ui.vertical(|ui| {
        ui.label("System Prompt");
        ui.add(TextEdit::multiline(&mut self.draft.system_prompt).desired_rows(4));
        ui.label(format!("Silence timeout: {} ms", self.draft.silence_ms));
        ui.add(Slider::new(&mut self.draft.silence_ms, 300_u32..=2000).show_value(false));
        ui.label(format!("Speech threshold: {:.2}", self.draft.speech_threshold));
        ui.add(Slider::new(&mut self.draft.speech_threshold, 0.1_f32..=0.9).show_value(false));

        ui.horizontal(|ui| {
            // Apply: only mark+send if save succeeds (review #5)
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
                self.show_settings = false; // draft discarded (re-cloned on next open)
            }
        });
    });
}
```

### Worker handling
```rust
Ok(ControlMsg::SettingsChanged(s)) => {
    system_prompt = s.system_prompt.clone();
    vad.set_thresholds(s.silence_ms, s.speech_threshold);
    settings = s;
}
```

Worker loads settings on startup:
```rust
let mut settings = AppSettings::load();
let mut system_prompt = settings.system_prompt.clone();
// Pass initial thresholds to VAD after load
vad.set_thresholds(settings.silence_ms, settings.speech_threshold);
```

## Testing

- **`format_timestamp` unit test** — feed a known UNIX epoch → assert expected UTC `HH:MM:SS`.
- **`AppSettings::validate()`** — out-of-range inputs are clamped; in-range inputs are unchanged.
- **`AppSettings` roundtrip** — `save()` to a temp path, `load()` it back, assert equality.
- **`AppSettings::default()`** — values match `config.rs` constants exactly.
- **`Vad::set_thresholds`** — construct a `Vad`, call `set_thresholds(500, 0.7)`, assert fields updated.
- **`SettingsChanged` in worker** — send the message via ctrl channel; assert `system_prompt` and VAD thresholds update.
- **Manual** — change prompt to `"Reply in ALL CAPS."`, Apply, speak → output is uppercase.

## Out of scope

Menu-bar/tray, global hotkey, compact orb mode, font/theme, `chrono` local timestamps
(use UTC for now), settings version field, visual Apply feedback, RAG knowledge grounding.
