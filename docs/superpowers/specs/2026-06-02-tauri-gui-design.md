# Tauri GUI — Deep Blue Frost, Floating Window Design Spec

**Date:** 2026-06-02  
**Status:** Approved; pending implementation plan  
**Branch:** `feat/gui-enhancement`  
**Builds on:** the existing Rust desktop app (`rust/`) — full audio pipeline preserved

## Context

The current egui UI looks like a developer tool. The Python P0 browser UI (seen in
`docs/images/python_p0.png`) was preferred because it had full CSS styling — rounded
cards, proper spacing, designed feel. egui cannot reach that quality level without
fighting the framework.

**Decision:** Replace egui with Tauri (Rust shell + WebView) while keeping the entire
audio/AEC/barge-in pipeline completely untouched. This gives full CSS power including
`backdrop-filter: blur()` for a frosted glass transparent floating window.

**Why not TypeScript full-stack:** The AEC layer (`speexdsp` via `aec-rs`) has no
maintained Node.js bindings. The Silero VAD 512-sample context window fix, the
24kHz→16kHz resampling, the barge-in `clean_rms` threshold — all proved and working in
Rust. Rewriting the pipeline in TS would re-spend months on already-solved problems.
Tauri is the correct split: Rust owns audio/AI, WebView owns rendering.

## Decisions

| Area | Decision |
|------|----------|
| Framework | **Tauri v2** (Rust shell + WebView) |
| Style | **Deep Blue Frost** — `backdrop-filter: blur(32px)`, dark navy + blue accent |
| Window behavior | **Floating, always-on-top, borderless** (`alwaysOnTop: true`, `decorations: false`, `transparent: true`) |
| Draggable | `data-tauri-drag-region` attribute on the title bar element |
| Frontend language | **Vanilla HTML/CSS/JS** — reuse and extend existing `static/` assets |
| Worker → UI events | `app_handle.emit("turn", payload)` replaces `tx_ui.send(UiEvent::...)` |
| UI → Rust commands | `invoke("toggle_mic")` etc. via `#[tauri::command]` functions |

## What changes

| File | Change |
|------|--------|
| `rust/Cargo.toml` | Remove `eframe`, `egui`; add `tauri = "2"` |
| `rust/src/main.rs` | Replace `eframe::run_native()` with `tauri::Builder::default()...run()` |
| `rust/src/ui.rs` | **Deleted** — frontend logic moves to `static/app.js` |
| `rust/src/worker.rs` | Replace `tx_ui.send(UiEvent::...)` with `app_handle.emit(...)` calls |
| `rust/src/events.rs` | `UiEvent` enum becomes Tauri event payload structs (serde JSON); `ControlMsg` becomes Tauri commands |
| `static/index.html` | Restyled to Deep Blue Frost; add `data-tauri-drag-region`; wire Tauri JS API |
| `static/style.css` | Deep Blue Frost: `backdrop-filter`, rgba backgrounds, border-radius 22px, pill buttons |
| `static/app.js` | Replace `EventSource("/events")` with `listen("turn", handler)` Tauri JS API; replace `fetch("/control/...")` with `invoke("toggle_mic")` etc. |
| `tauri.conf.json` | **New**: window config (`transparent`, `alwaysOnTop`, `decorations: false`, `width: 380`, `height: 680`) |

## What is completely unchanged

The entire audio/AI pipeline — `audio.rs`, `vad.rs`, `echo.rs`, `segmenter.rs`,
`history.rs`, `timing.rs`, `state.rs`, `refine.rs`, `tts.rs`, `stt.rs`, `settings.rs`,
`worker.rs` logic — all untouched. The 36 tests continue to pass.

## Event/command mapping

### Worker → UI (events)

```rust
// In worker.rs, replace:
let _ = tx_ui.send(UiEvent::Turn { heard, refined, timing, timestamp });
// With:
app_handle.emit("turn", serde_json::json!({
    "heard": heard, "refined": refined,
    "timestamp": timestamp,
    "timing": { "endpoint_ms": ..., "stt_ms": ..., "refine_ms": ..., "reply_start_ms": ... }
})).ok();

// State changes:
app_handle.emit("state", serde_json::json!({ "value": "listening" })).ok();

// Clear:
app_handle.emit("cleared", ()).ok();
```

### UI → Rust (commands)

```rust
#[tauri::command]
fn toggle_mic(state: tauri::State<SharedAppState>) { ... }

#[tauri::command]
fn stop_tts(state: tauri::State<SharedAppState>) { ... }

#[tauri::command]
fn clear_transcript(state: tauri::State<SharedAppState>) { ... }

#[tauri::command]
fn apply_settings(settings: AppSettings, state: tauri::State<SharedAppState>) { ... }
```

### JS (app.js)

```js
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";

await listen("turn", (e) => updateTranscript(e.payload));
await listen("state", (e) => updateState(e.payload.value));
await listen("cleared", () => clearTranscript());

document.getElementById("mic").onclick = () => invoke("toggle_mic");
document.getElementById("stop").onclick = () => invoke("stop_tts");
document.getElementById("clear").onclick = () => invoke("clear_transcript");
```

## Visual spec (Deep Blue Frost)

```css
/* Window */
background: rgba(10, 12, 28, 0.62);
backdrop-filter: blur(32px);
-webkit-backdrop-filter: blur(32px);
border: 1px solid rgba(255, 255, 255, 0.10);
border-radius: 22px;
background-image: radial-gradient(ellipse at top, rgba(59,130,246,.07) 0%, transparent 60%);
box-shadow: 0 12px 48px rgba(0,0,0,.6), inset 0 1px 0 rgba(255,255,255,.08);

/* ☯ accent */
color: #60a5fa;
filter: drop-shadow(0 0 18px rgba(96,165,250,.45));

/* Cards */
background: rgba(255,255,255,.045);
border: 1px solid rgba(255,255,255,.07);
border-radius: 12px;

/* Buttons */
background: rgba(255,255,255,.06);
border-radius: 20px;

/* Refined text */
color: #93c5fd;
text-shadow: 0 0 10px rgba(147,197,253,.2);
```

## Tauri window config (`tauri.conf.json`)

```json
{
  "app": {
    "windows": [{
      "label": "main",
      "title": "Voice Assistant",
      "width": 380,
      "height": 680,
      "resizable": true,
      "transparent": true,
      "decorations": false,
      "alwaysOnTop": true,
      "center": true
    }]
  }
}
```

## Testing

- `cargo test --lib` — existing 36 tests pass (pipeline unchanged)
- `cargo tauri dev` — opens the floating frosted window; speak a sentence, verify
  heard/refined transcript, timing badge, state cycling
- Barge-in: speak mid-TTS → `[barge-in]` log; TTS cuts off; next turn processes
- Settings popup: click ⚙ → floating Tauri window (separate `Window::new(...)` or
  same WebView panel)
- macOS: verify `backdrop-filter` blur shows desktop behind window

## Out of scope

RAG knowledge grounding, menu-bar/tray icon, global hotkey, compact orb mode,
React/framework upgrade (vanilla JS sufficient).
