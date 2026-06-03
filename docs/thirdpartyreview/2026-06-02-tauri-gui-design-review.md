# Third-Party Review — Tauri GUI Design Spec

**Reviewed doc:** `docs/superpowers/specs/2026-06-02-tauri-gui-design.md`  
**Date:** 2026-06-02  
**Grounded against:** `rust/src/main.rs`, `rust/src/ui.rs`, `rust/src/worker.rs`, `rust/src/events.rs`, `rust/src/settings.rs`, `static/index.html`, `static/app.js`, `static/style.css`, `rust/Cargo.toml`

## Verdict

The product direction is good: Tauri is a sensible way to keep the proven Rust audio/AEC/STT/TTS pipeline while replacing the egui UI with a more polished CSS-driven interface.

However, I would **not approve the spec as-is** because it understates the integration work and has a few architectural contradictions:

- It says the pipeline and `worker.rs` logic are untouched, but also says to replace `tx_ui.send(...)` with `app_handle.emit(...)`.
- It says vanilla HTML/CSS/JS, but uses bare imports from `@tauri-apps/api/...`, which requires a frontend packaging decision.
- It says `ControlMsg` becomes Tauri commands, but the worker still needs a control channel.
- It deletes `ui.rs`, but does not define a clean replacement for the existing settings/transcript state behavior.

The spec is close, but it needs a clearer Rust/Tauri bridge design.

## Should-fix

### 1. Do not push `app_handle.emit(...)` directly into `worker.rs`

The spec says:

```rust
app_handle.emit("turn", serde_json::json!(...)).ok();
```

inside `worker.rs`.

That couples the audio/AI worker to Tauri and contradicts:

> full audio pipeline preserved

Today `worker.rs` receives:

```rust
tx_ui: Sender<UiEvent>
```

and emits domain events. That is a good seam.

**Recommendation:** keep `worker.rs` emitting `UiEvent` over `tx_ui`, and add a Tauri bridge thread in `main.rs`:

```rust
std::thread::spawn(move || {
    while let Ok(event) = rx_ui.recv() {
        match event {
            UiEvent::StateChanged(s) => {
                app.emit("state", StatePayload { value: s.label() }).ok();
            }
            UiEvent::Turn { heard, refined, timing, timestamp } => {
                app.emit("turn", TurnPayload { heard, refined, timing, timestamp }).ok();
            }
            UiEvent::Cleared => {
                app.emit("cleared", ()).ok();
            }
        }
    }
});
```

This preserves the worker architecture and makes Tauri just another UI transport.

### 2. Tauri commands should send `ControlMsg`, not replace it

The spec says:

> `ControlMsg` becomes Tauri commands

But the worker still owns:

- mic toggle state transitions,
- history clear,
- stop flag handling,
- settings application.

Commands should be thin adapters that send messages to the existing `tx_ctrl`.

**Recommendation:**

Keep `ControlMsg` and define commands like:

```rust
#[tauri::command]
fn toggle_mic(state: tauri::State<AppBridge>) -> Result<(), String> {
    state.tx_ctrl.send(ControlMsg::ToggleMic).map_err(|e| e.to_string())
}
```

Same for:

- `stop_tts` → `ControlMsg::Stop`
- `clear_transcript` → `ControlMsg::Clear`
- `apply_settings` → save/validate settings, then `ControlMsg::SettingsChanged(settings)`

This avoids duplicating business logic in Tauri command handlers.

### 3. Define `SharedAppState` / bridge state concretely

The spec uses:

```rust
tauri::State<SharedAppState>
```

but does not define it.

**Recommendation:** define a minimal bridge type:

```rust
struct AppBridge {
    tx_ctrl: crossbeam_channel::Sender<ControlMsg>,
}
```

If settings need synchronous readback, include methods or expose commands like:

```rust
#[tauri::command]
fn get_settings() -> AppSettings {
    AppSettings::load()
}
```

Avoid putting `SharedState`, `history`, or worker internals into Tauri state unless necessary.

### 4. Vanilla JS import strategy is underspecified

The spec says:

```js
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
```

But current `static/index.html` uses:

```html
<script src="/app.js"></script>
```

Bare package imports do not work in a plain browser script without a bundler/import map. Tauri v2 frontend usage needs a concrete choice.

**Recommendation:** choose one:

- **Bundled frontend:** add Vite or equivalent, install `@tauri-apps/api`, use `type="module"`, and configure Tauri `frontendDist`.
- **No bundler:** enable global Tauri API if supported/configured and use `window.__TAURI__.core.invoke` / `window.__TAURI__.event.listen`.

Since the spec explicitly says vanilla/no framework, the no-bundler path should be specified precisely.

### 5. Tauri v2 project layout/config is incomplete

The spec says add `tauri.conf.json` and `tauri = "2"` to `rust/Cargo.toml`.

For Tauri v2, the implementation usually also needs:

- `tauri-build` in `[build-dependencies]`
- `build.rs` with `tauri_build::build()`
- `tauri::generate_context!()`
- `tauri::generate_handler![...]`
- correct config location relative to the Cargo manifest
- frontend asset path / dev URL configuration
- likely `tauri` features depending on APIs used

**Recommendation:** add a "Tauri project wiring" section that names these files and decisions. Otherwise `cargo tauri dev` will not be implementation-ready.

### 6. Static asset paths need Tauri-safe treatment

Current `static/index.html` has:

```html
<link rel="stylesheet" href="/style.css">
<script src="/app.js"></script>
```

Absolute `/style.css` works for an HTTP server, but may not behave the same from Tauri bundled assets.

**Recommendation:** use relative paths:

```html
<link rel="stylesheet" href="./style.css">
<script type="module" src="./app.js"></script>
```

or define the frontend build/dev server path explicitly.

### 7. Settings UI behavior is under-specified

Current egui settings include:

- system prompt text area
- silence timeout slider
- speech threshold slider
- history turns slider
- Apply / Defaults / Cancel
- save to `~/.config/azva/settings.json`
- send `ControlMsg::SettingsChanged`

The Tauri spec only says:

> Settings popup: click ⚙ → floating Tauri window or same WebView panel

That is not enough to preserve existing behavior.

**Recommendation:** define:

- whether settings are an in-page panel or secondary Tauri window;
- how initial settings load into JS;
- commands:
  - `get_settings`
  - `apply_settings`
  - maybe `reset_settings_draft` frontend-only;
- validation bounds matching `AppSettings::validate()`:
  - `silence_ms: 300..=5000`
  - `speech_threshold: 0.1..=0.9`
  - `history_turns: 0..=20`

### 8. Initial state snapshot is missing

With event listeners, the UI only sees future events. If the WebView loads after the worker already emitted initial state, the frontend may show stale/default state.

**Recommendation:** add commands:

```rust
#[tauri::command]
fn get_initial_state(...) -> InitialUiState
```

At minimum include:

- current state label;
- settings;
- maybe empty transcript unless transcript persistence is intentionally out of scope.

Alternatively emit a state event from `setup()` after the frontend is ready, but a pull command is more robust.

## Consider

### 9. Keep `events.rs` as domain events plus add payload structs

The spec says:

> `UiEvent` enum becomes Tauri event payload structs

That is more invasive than needed.

**Recommendation:** keep:

```rust
UiEvent
ControlMsg
State
```

and add Tauri payload structs alongside them:

```rust
#[derive(Serialize, Clone)]
struct StatePayload<'a> {
    value: &'a str,
}
```

This preserves testability and lets `worker.rs` stay UI-framework-neutral.

### 10. Event payload names should be consistent

Spec uses:

- event: `"cleared"`
- command: `clear_transcript`
- current enum: `UiEvent::Cleared`
- old SSE type: `"clear"`

Any is fine, but standardize in the spec and tests.

Suggested event names:

- `"state"`
- `"turn"`
- `"clear"`

For v0, simple names are fine.

### 11. Transparent/frosted window needs platform fallback

`backdrop-filter` plus transparent WebView can vary by macOS/WebKit behavior. The spec should define a fallback if blur is weak or transparency is unavailable.

**Recommendation:**

- keep the dark translucent background usable without blur;
- test contrast with blur disabled;
- include manual acceptance criteria for readability.

### 12. Always-on-top/borderless needs close/drag affordances

Borderless always-on-top windows need app-level controls or keyboard behavior. Current spec includes drag region, but not close/minimize.

**Recommendation:**

- add a visible close button or at least `Esc`/`Cmd+W` behavior;
- define whether the window can be moved from the full title area only;
- ensure controls inside drag region are not accidentally draggable.

### 13. Permissions/capabilities are missing

Tauri v2 has a capability/permission model. Commands and event APIs may need capability configuration depending on setup.

**Recommendation:**

- mention adding the needed Tauri capabilities for core/event/window APIs;
- keep the command surface minimal:
  - `toggle_mic`
  - `stop_tts`
  - `clear_transcript`
  - `get_settings`
  - `apply_settings`
  - `get_initial_state`

### 14. Testing should include command bridge tests

Current testing section says existing Rust tests pass and manual GUI checks.

Add tests or lightweight checks for:

- command sends correct `ControlMsg`;
- `AppSettings` payload round-trips through serde;
- `UiEvent` → Tauri payload mapping produces expected JSON;
- frontend handles `state`, `turn`, and `clear` events.

## Suggested revised architecture

I would revise the design to this:

```text
Rust audio/AI pipeline unchanged

worker.rs
  emits UiEvent over tx_ui
  receives ControlMsg over rx_ctrl

main.rs / tauri bridge
  owns tx_ctrl
  spawns worker
  spawns UiEvent -> app.emit(...) bridge
  registers commands that send ControlMsg

frontend
  listen("state" | "turn" | "clear")
  invoke("toggle_mic" | "stop_tts" | "clear_transcript" | "get_settings" | "apply_settings")
```

This keeps the Tauri migration mostly at the shell/UI boundary instead of leaking Tauri into the pipeline.

## Overall recommendation

Approve the **visual/product direction**, but revise the implementation design before planning.

The main fixes are:

- **Keep `worker.rs` framework-neutral** with `UiEvent`/`ControlMsg`.
- **Add a Tauri bridge layer** instead of emitting directly from worker.
- **Specify the Tauri v2 project wiring** (`build.rs`, config location, capabilities, frontend dist/dev path).
- **Choose a real vanilla JS import strategy.**
- **Fully specify settings and initial-state behavior.**
