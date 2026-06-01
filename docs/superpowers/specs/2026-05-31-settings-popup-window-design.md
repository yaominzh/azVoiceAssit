# Settings — Floating Popup Window Design Spec

**Date:** 2026-05-31
**Status:** Approved; pending implementation
**Replaces:** the inline settings drawer in `docs/superpowers/specs/2026-05-31-rust-ui-polish-settings-design.md` (§Settings panel)
**Scope:** `rust/src/ui.rs` only — one file, ~10 lines changed

## Context

The integrated settings drawer rendered as an inline vertical block inside the
`CentralPanel`, competing for vertical space with the transcript and breaking the
layout when open. The fix: replace the inline block with an `egui::Window` — a
floating, draggable panel that renders on top of the main window independently.

## Decision

Use `egui::Window::new("Settings")` with `.open(&mut self.show_settings)` (which
adds a free ✕ close button and sets `show_settings = false` on click).
Non-modal — the main window (transcript, controls, voice loop) stays fully
functional while settings is open.

## Change

**Remove from `ui.rs`:**
- The `if self.show_settings { ui.vertical(...) }` inline block at the bottom of `ui()`.
- The `let reserved = if self.show_settings { 260.0 } else { 120.0 };` scroll-height
  calculation — the transcript always fills remaining space now.

**Add to `ui.rs`** (called with `ui.ctx()` — can go anywhere inside `ui()` after
the main layout):

```rust
egui::Window::new("Settings")
    .open(&mut self.show_settings)
    .resizable(false)
    .default_width(340.0)
    .show(ui.ctx(), |ui| {
        ui.label(egui::RichText::new("System Prompt").size(11.0).color(egui::Color32::from_gray(160)));
        ui.add(egui::TextEdit::multiline(&mut self.draft.system_prompt)
            .desired_rows(4).desired_width(f32::INFINITY));
        ui.add_space(6.0);
        ui.label(egui::RichText::new(format!("Silence timeout: {} ms", self.draft.silence_ms))
            .size(11.0).color(egui::Color32::from_gray(160)));
        ui.add(egui::Slider::new(&mut self.draft.silence_ms, 300_u32..=2000).show_value(false));
        ui.add_space(4.0);
        ui.label(egui::RichText::new(format!("Speech threshold: {:.2}", self.draft.speech_threshold))
            .size(11.0).color(egui::Color32::from_gray(160)));
        ui.add(egui::Slider::new(&mut self.draft.speech_threshold, 0.1_f32..=0.9_f32).show_value(false));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Apply").clicked() {
                match self.draft.save() {
                    Ok(()) => {
                        self.applied = self.draft.clone();
                        let _ = self.tx_ctrl.send(ControlMsg::SettingsChanged(self.draft.clone()));
                        self.show_settings = false;
                    }
                    Err(e) => eprintln!("Settings save failed: {e}"),
                }
            }
            if ui.button("Defaults").clicked() { self.draft = AppSettings::default(); }
            if ui.button("Cancel").clicked() { self.show_settings = false; }
        });
    });
```

The gear button's open logic stays exactly the same (clones applied → draft on open).

## What does NOT change

`VoiceApp` fields (`draft`, `applied`, `show_settings`), `AppSettings`, `worker.rs`,
`events.rs`, `vad.rs`, `settings.rs` — all untouched.

## Testing

- `cargo build` + `cargo test --lib` (22 pass — no logic change).
- Manual: click ⚙ → floating window appears, draggable, ✕ closes it, main window
  stays live (speak during open → turn appears). Apply changes prompt → confirmed on
  next turn.
