# Tauri GUI — Deep Blue Frost, Floating Window Design Spec

**Date:** 2026-06-02  
**Status:** Approved (rev 2 — third-party review incorporated)  
**Branch:** `feat/gui-enhancement`  
**Builds on:** the existing Rust desktop app (`rust/`) — full audio pipeline preserved

## Context

The egui UI looks like a developer tool. The Python P0 browser UI (seen in
`docs/images/python_p0.png`) was preferred because it had full CSS styling — rounded
cards, proper spacing, designed feel. Tauri (Rust shell + native WebView) gives the same
full CSS power while keeping the entire audio/AEC/barge-in pipeline untouched.

**Why not TypeScript full-stack:** The AEC layer (`speexdsp` / `aec-rs`) has no
maintained Node.js bindings. The Silero VAD 512-sample context window, AEC reference
resampling, and barge-in `clean_rms` threshold are all proved/working. Tauri is the
correct split: Rust owns audio/AI, WebView owns rendering.

## Decisions

| Area | Decision |
|------|----------|
| Framework | **Tauri v2** |
| Style | **Deep Blue Frost** — `backdrop-filter: blur(32px)`, dark navy + blue accent |
| Window | **Floating, always-on-top, borderless** (`alwaysOnTop: true`, `decorations: false`, `transparent: true`) |
| Draggable | `data-tauri-drag-region` on the title bar |
| Frontend JS | **Vanilla JS**, no bundler — Tauri `withGlobalTauri: true` exposes API at `window.__TAURI__` |
| Worker→UI bridge | Separate bridge thread in `main.rs` draining `rx_ui` → `app.emit(...)` |
| UI→Rust commands | Thin adapters that send `ControlMsg` over `tx_ctrl` |
| Settings UI | In-page panel (same WebView, `<div>` toggle) with `get_settings` / `apply_settings` commands |
| Initial state | `get_initial_state` command called on `DOMContentLoaded` |
| Close affordance | Visible close button in drag bar + `Esc` / `Cmd+W` keybinding |

## Architecture — the clean bridge

```text
Rust audio/AI pipeline — unchanged

worker.rs
  emits  UiEvent  over  tx_ui    (unchanged from today)
  receives  ControlMsg  over  rx_ctrl  (unchanged from today)

main.rs / Tauri bridge layer
  owns tx_ctrl, rx_ui
  spawns worker
  spawns bridge thread: rx_ui.recv() → app.emit("state"|"turn"|"clear")
  registers commands: toggle_mic/stop_tts/clear_transcript/get_settings/apply_settings/get_initial_state
  manages Tauri window config

Frontend  (static/ HTML/CSS/JS, extended from Python P0 assets)
  window.__TAURI__.event.listen("state"|"turn"|"clear")
  window.__TAURI__.core.invoke("toggle_mic"|...)
```

`worker.rs` never imports Tauri. It remains UI-framework-neutral. The bridge thread is
the only coupling point.

## What changes

| File | Change |
|------|--------|
| `rust/Cargo.toml` | Remove `eframe`, `egui`; add `tauri = "2"`, `tauri-build` in build-deps |
| `rust/build.rs` | **New**: `fn main() { tauri_build::build() }` |
| `rust/src/main.rs` | Replace `eframe::run_native()` with `tauri::Builder` setup; spawn bridge thread; register commands |
| `rust/src/ui.rs` | **Deleted** — frontend logic moves to `static/app.js` |
| `rust/src/events.rs` | Keep `UiEvent`/`ControlMsg`/`State` as-is; **add** `#[derive(Serialize,Clone)]` payload structs for `app.emit()` alongside them |
| `static/index.html` | Restyled Deep Blue Frost; add `data-tauri-drag-region`; relative asset paths `./style.css`; wire `window.__TAURI__` |
| `static/style.css` | Deep Blue Frost CSS (see §Visual spec) |
| `static/app.js` | Replace `EventSource`/`fetch` with `window.__TAURI__` listen/invoke |
| `tauri.conf.json` | **New**: window config + capability config + `withGlobalTauri: true` |
| `capabilities/default.json` | **New**: Tauri v2 capability for core/event/window APIs |

## What is completely unchanged

`audio.rs`, `vad.rs`, `echo.rs`, `segmenter.rs`, `history.rs`, `timing.rs`, `state.rs`,
`refine.rs`, `tts.rs`, `stt.rs`, `settings.rs`, `worker.rs` logic — all untouched.
36 tests continue to pass.

## Tauri bridge — concrete types

### `AppBridge` (Tauri managed state)

```rust
struct AppBridge {
    tx_ctrl: crossbeam_channel::Sender<crate::events::ControlMsg>,
}
```

`worker.rs` keeps its existing `tx_ctrl` / `rx_ctrl` / `tx_ui` / `rx_ui` channels.
`AppBridge` holds a clone of `tx_ctrl` for command handlers.

### Bridge thread (in `main.rs`)

```rust
std::thread::spawn({
    let app = app_handle.clone();
    move || {
        while let Ok(event) = rx_ui.recv() {
            match event {
                UiEvent::StateChanged(s) =>
                    app.emit("state", serde_json::json!({"value": s.label()})).ok(),
                UiEvent::Turn { heard, refined, timing, timestamp } =>
                    app.emit("turn", serde_json::json!({
                        "heard": heard, "refined": refined,
                        "timestamp": timestamp,
                        "timing": { "endpoint_ms": timing.endpoint_ms, "stt_ms": timing.stt_ms,
                                    "refine_ms": timing.refine_ms, "reply_start_ms": timing.reply_start_ms }
                    })).ok(),
                UiEvent::Cleared =>
                    app.emit("clear", ()).ok(),
            };
        }
    }
});
```

### Tauri commands

```rust
#[tauri::command]
fn toggle_mic(bridge: tauri::State<AppBridge>) -> Result<(), String> {
    bridge.tx_ctrl.send(ControlMsg::ToggleMic).map_err(|e| e.to_string())
}

#[tauri::command]
fn stop_tts(bridge: tauri::State<AppBridge>) -> Result<(), String> {
    bridge.tx_ctrl.send(ControlMsg::Stop).map_err(|e| e.to_string())
}

#[tauri::command]
fn clear_transcript(bridge: tauri::State<AppBridge>) -> Result<(), String> {
    bridge.tx_ctrl.send(ControlMsg::Clear).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_settings() -> crate::settings::AppSettings {
    crate::settings::AppSettings::load()
}

#[tauri::command]
fn apply_settings(settings: crate::settings::AppSettings, bridge: tauri::State<AppBridge>)
    -> Result<(), String>
{
    let validated = settings.validate();
    validated.save()?;
    bridge.tx_ctrl.send(ControlMsg::SettingsChanged(validated)).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_initial_state() -> serde_json::Value {
    let s = crate::settings::AppSettings::load();
    serde_json::json!({
        "state": "listening",
        "settings": s,
    })
}
```

### JS (app.js) — `window.__TAURI__`

```js
const { listen } = window.__TAURI__.event;
const { invoke } = window.__TAURI__.core;

document.addEventListener("DOMContentLoaded", async () => {
    const init = await invoke("get_initial_state");
    applySettings(init.settings);
    updateState(init.state);

    await listen("state", (e) => updateState(e.payload.value));
    await listen("turn",  (e) => addTurn(e.payload));
    await listen("clear", ()  => clearTranscript());
});

document.getElementById("mic").onclick   = () => invoke("toggle_mic");
document.getElementById("stop").onclick  = () => invoke("stop_tts");
document.getElementById("clear").onclick = () => invoke("clear_transcript");
```

## Tauri v2 project wiring

**`rust/Cargo.toml` additions:**
```toml
[dependencies]
tauri = { version = "2", features = ["devtools"] }
serde = { version = "1", features = ["derive"] }   # already present
serde_json = "1"                                    # already present

[build-dependencies]
tauri-build = { version = "2", features = [] }
```

**`rust/build.rs`** (new file):
```rust
fn main() { tauri_build::build() }
```

**`tauri.conf.json`** (at repo root or `rust/` — check `tauri info` for expected location):
```json
{
  "productName": "VoiceAssistant",
  "version": "0.1.0",
  "identifier": "com.azva.voiceassistant",
  "app": {
    "withGlobalTauri": true,
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
  },
  "bundle": {
    "active": true,
    "targets": "all",
    "icon": []
  }
}
```

**`capabilities/default.json`** (new, required by Tauri v2):
```json
{
  "identifier": "default",
  "description": "default capability",
  "windows": ["main"],
  "permissions": [
    "core:default",
    "core:event:default",
    "core:window:default"
  ]
}
```

## Visual spec (Deep Blue Frost)

```css
/* Window background */
background: rgba(10, 12, 28, 0.62);
backdrop-filter: blur(32px);
-webkit-backdrop-filter: blur(32px);
border: 1px solid rgba(255,255,255,.10);
border-radius: 22px;
background-image: radial-gradient(ellipse at top, rgba(59,130,246,.07) 0%, transparent 60%);
box-shadow: 0 12px 48px rgba(0,0,0,.6), inset 0 1px 0 rgba(255,255,255,.08);

/* Fallback if backdrop-filter unavailable */
background: rgba(10, 12, 28, 0.90);

/* ☯ glyph */
color: #60a5fa;
filter: drop-shadow(0 0 18px rgba(96,165,250,.45));

/* Cards (heard/refined) */
background: rgba(255,255,255,.045);
border: 1px solid rgba(255,255,255,.07);
border-radius: 12px;

/* Buttons (pill shape) */
background: rgba(255,255,255,.06);
border-radius: 20px;

/* Refined text */
color: #93c5fd;
```

## Settings panel (in-page, same WebView)

- Rendered as a `<div id="settings-panel">` toggled by the ⚙ button (same approach as Python P0)
- **Loads current values via `get_settings` command** on panel open
- Sliders with bounds matching `AppSettings::validate()`:
  - `silence_ms`: 300–5000 ms
  - `speech_threshold`: 0.1–0.9
  - `history_turns`: 0–20
- `system_prompt`: `<textarea>` (4 rows)
- Apply → `invoke("apply_settings", { settings: draft })`
- Defaults → reset draft to defaults in JS
- Cancel → discard draft

## Close/drag affordances

- Title bar drag region: 28px tall strip at top with `data-tauri-drag-region`
- Close button: visible `×` in top-left of drag bar → `window.__TAURI__.window.getCurrent().close()`
- Keyboard: `Esc` or `Cmd+W` → close window (handle via `keydown` in JS)
- Drag region and buttons must be separate — buttons inside drag region need `pointer-events: none` on the drag div or explicit click handling

## Asset paths

All `static/` asset references use relative paths:
```html
<link rel="stylesheet" href="./style.css">
<script src="./app.js" defer></script>
```

## Testing

- `cargo test --lib` — 36 tests pass (pipeline unchanged)
- Add unit tests:
  - `toggle_mic` command sends `ControlMsg::ToggleMic` over `tx_ctrl`
  - `AppSettings` round-trips through serde JSON (already tested in `settings.rs`)
  - `UiEvent::Turn` → payload JSON has expected fields
- `cargo tauri dev` — opens floating frosted window; speak a sentence, verify transcript, timing, state cycling
- Barge-in: speak mid-TTS → TTS cuts off; next turn processes
- Verify `backdrop-filter` blur shows desktop through window on macOS
- Verify close button / `Cmd+W` / `Esc` dismisses window
- Verify readability with blur disabled (fallback background)

## Out of scope

RAG, menu-bar/tray, global hotkey, React/bundler upgrade.
