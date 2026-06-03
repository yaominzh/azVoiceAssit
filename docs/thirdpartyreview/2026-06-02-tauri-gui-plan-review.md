# Third-Party Review — Tauri GUI Implementation Plan

**Reviewed doc:** `docs/superpowers/plans/2026-06-02-tauri-gui.md`  
**Date:** 2026-06-02  
**Grounded against:** `rust/src/lib.rs`, `rust/src/main.rs`, `rust/src/worker.rs`, `rust/src/timing.rs`, `rust/src/events.rs`, `rust/src/settings.rs`, `static/index.html`, `static/style.css`, `static/app.js`, `rust/Cargo.toml`  
**Related:** `docs/thirdpartyreview/2026-06-02-tauri-gui-design-review.md` (spec review)

## Verdict

The task breakdown (scaffold → serde → main.rs/bridge → command tests → frontend → manual E2E) is well-sequenced with TDD discipline, real verification commands, and per-task commits. It correctly keeps the audio/AI pipeline framework-neutral.

However, the plan is **not ready to execute as written**. There are **5 concrete blockers** that will stop the build or break the running app, plus smaller issues. Most are inherited from rev-2 spec gaps that were not corrected here. The blockers cluster early (Tasks 1–3), so a worker would stall almost immediately on the `lib.rs` / `egui` compile failure.

## Blockers

### 1. `lib.rs` still declares `pub mod ui;` — deleting `ui.rs` breaks the lib target

Biggest miss. The plan deletes `rust/src/ui.rs` (Task 3, Step 1) and removes `mod ui;` from `main.rs`, but never touches `rust/src/lib.rs`, which currently has:

```
pub mod ui;
```

Consequences:

- After Task 1 removes `egui`/`eframe` from `Cargo.toml`, the **lib target won't compile** because `ui.rs` still imports `eframe`/`egui`. `cargo test --lib` in Task 1's verification (and Task 2's "37 passed") fails before reaching Task 3.
- After Task 3 deletes `ui.rs`, the lib target fails with `file not found for module ui`.

**Fix:** add a step (in Task 1 or top of Task 3) to remove `pub mod ui;` from `rust/src/lib.rs` in the same change that removes egui / deletes `ui.rs`. The lib and binary share these module files, so cleanup must cover both.

### 2. Bridge thread calls `app_handle.emit(...)` but only imports `tauri::Manager`

Task 3's `main.rs` imports:

```rust
use tauri::Manager;
```

But `emit` on an `AppHandle` comes from the `tauri::Emitter` trait in Tauri v2. Without it the bridge thread fails to compile, so Task 3 Step 3 will not reach `Finished`.

**Fix:**

```rust
use tauri::{Emitter, Manager};
```

### 3. `tauri.conf.json` has no `build.frontendDist` — the WebView has nothing to load

The config defines `app.windows` and `bundle` but no `build` section. Tauri won't know where `static/` lives, so `cargo tauri dev` / `cargo run` will not serve `static/index.html`.

**Fix:** add a build block with path relative to the conf file location (repo root):

```json
"build": {
  "frontendDist": "./static"
},
```

### 4. `getCurrent()` is Tauri v1 API — close button and Esc/Cmd+W will throw

`static/app.js` uses:

```js
const { getCurrent } = window.__TAURI__.window;
...
getCurrent().close();
```

In Tauri v2 the current-window getter is `getCurrentWindow()`. `getCurrent` is undefined, so the close button and keyboard handler throw at runtime (Task 6 Step 6 fails).

**Fix:**

```js
const { getCurrentWindow } = window.__TAURI__.window;
...
getCurrentWindow().close();
```

### 5. Window close permission missing from capabilities

`capabilities/default.json` lists `core:window:default`, which is mostly read-only getters. Calling `.close()` needs the close permission explicitly, or the IPC call is denied.

**Fix:**

```json
"permissions": [
  "core:default",
  "core:event:default",
  "core:window:default",
  "core:window:allow-close"
]
```

## Should-fix

### 6. Transparent frost needs `macOSPrivateApi: true`

The goal is the deep-blue frost effect, but `transparent: true` on macOS requires private-API opt-in, or you get an opaque background (the CSS `@supports` fallback then masks the failure). Task 6 Step 2 treats "solid dark background" as acceptable — meaning the headline visual silently didn't work.

**Fix:** add to the `app` block in `tauri.conf.json`:

```json
"app": {
  "withGlobalTauri": true,
  "macOSPrivateApi": true,
  "windows": [ ... ]
}
```

### 7. "Defaults" button sets the wrong default system prompt

In `app.js`, `sp-defaults` does:

```js
const defaults = await invoke("get_settings");  // result ignored
applySettingsDraft({ system_prompt: "", silence_ms: 700, speech_threshold: 0.5, history_turns: 0 });
```

Two problems:

- The `get_settings` call is dead — result is discarded.
- The hardcoded `system_prompt: ""` does **not** match the real default, which is `SYSTEM_PROMPT` (`rust/src/settings.rs`). Clicking Defaults then Apply would wipe the system prompt to empty.

**Fix:** either add a command that returns `AppSettings::default()` and use it, or drop the dead `get_settings` call and hardcode the real default prompt. A `get_defaults` command is cleanest and keeps JS in sync with Rust.

## Consider

### 8. `tauri::test::mock_state` likely does not exist in v2

Task 4 leans on `tauri::test::mock_state`, and the notes assert it's "available in Tauri v2 test utilities." That's optimistic — it generally isn't exposed that way, and `tauri::test` requires the `test` feature. The included `toggle_mic_direct` fallback is good, but tests a *duplicate* function, not the real `#[tauri::command]`.

**Recommendation:** plan for the fallback as the primary path — extract the one-line body into a testable helper (e.g. `fn send_ctrl(tx, msg)`) and have both the command and the test call it. Keep the command a trivial wrapper. Don't gate "41 passed" on `mock_state`.

### 9. Barge-in manual test (Task 6 Step 4) may not pass with current pipeline

Task 6 Step 4 expects "speak while TTS is playing → TTS cuts off." But the current worker plays TTS synchronously and gates mic input via `speaking` during playback:

```rust
let _ = crate::tts::speak_stoppable(&client, &refined, &stop_tts, &rx_ctrl, Some(&echo));
speaking.store(false, Ordering::SeqCst);
reset_to_idle(&shared, &tx_ui, &mut vad);
while rx_audio.try_recv().is_ok() {}
```

Voice-triggered barge-in depends on the AEC Phase 2 work (not reflected in this code). The **Stop button** works (it sends `ControlMsg::Stop`), but automatic mid-TTS voice interruption likely won't.

**Recommendation:** reword Step 4 to test Stop-button interruption, and mark voice barge-in as dependent on Phase 2 being merged — otherwise this step reads as a failure that isn't this plan's fault.

### 10. `bundle.icon: []` may break bundling later

Empty icon array is fine for `dev`/`run` but `tauri build` typically requires at least one icon. Out of scope for this plan (manual dev only), but worth a note so it's not a surprise later.

## Required before execution

- **Remove `pub mod ui;` from `lib.rs`** alongside the egui removal / `ui.rs` deletion.
- **Import `tauri::Emitter`** in `main.rs`.
- **Add `build.frontendDist`** to `tauri.conf.json`.
- **Use `getCurrentWindow()`** and **add `core:window:allow-close`**.
- **Add `macOSPrivateApi: true`** for the frost effect.

Fix those and the plan is good to go.
