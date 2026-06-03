# Third-Party Review (v2) — Tauri GUI Implementation Plan

**Reviewed doc:** `docs/superpowers/plans/2026-06-02-tauri-gui.md` (updated)  
**Date:** 2026-06-02  
**Grounded against:** `rust/src/audio.rs`, `rust/src/main.rs`, `rust/src/worker.rs`, `rust/src/lib.rs`, `rust/src/timing.rs`, `rust/src/events.rs`, `rust/src/settings.rs`  
**Supersedes:** `docs/thirdpartyreview/2026-06-02-tauri-gui-plan-review.md`

## Verdict

The updated plan resolves **all** blockers and should-fix items from the v1 review. One **new** build-stopping issue was introduced by the `main.rs` rewrite, plus one stale note. Fix those and the plan is ready to execute.

## Resolved since v1

- **`lib.rs`:** File-structure table (line 25) and Task 3 Step 1 now remove `pub mod ui;` before deleting `ui.rs`, with rationale.
- **`Emitter`:** `main.rs` now imports `use tauri::{Emitter, Manager};` (line 285).
- **`frontendDist`:** `tauri.conf.json` adds `"build": { "frontendDist": "./static" }` (lines 88–90).
- **Close API:** `app.js` uses `getCurrentWindow()` (line 831).
- **Close permission:** `capabilities/default.json` adds `core:window:allow-close` (line 129).
- **Transparency:** `macOSPrivateApi: true` added (line 93).
- **Defaults button:** new `get_defaults` command returns `AppSettings::default()`, registered (line 411), and the JS Defaults handler calls it (line 909) — no more dead call or wrong empty prompt.
- **Command tests:** Task 4 dropped `mock_state` in favor of a `send_ctrl` helper that both commands and tests call.
- **Barge-in test:** Task 6 Step 4 now states Phase 2 is merged and tests both voice barge-in and the Stop button. Confirmed consistent with current code — `start_capture` no longer takes `speaking`, and `worker.rs` contains the `TtsHandle` / `clean_rms` barge-in logic.

## New blocker (introduced by the rewrite)

### `start_capture` is called with 3 args but takes 2 — won't compile

The plan's replacement `main.rs` (line 372) has:

```rust
let _stream = match audio::start_capture(tx_raw, shared.clone(), speaking.clone()) {
```

But the current signature is two-argument (Phase 2 removed the capture-side speaking gate):

```rust
pub fn start_capture(
    tx: Sender<Vec<f32>>,
    shared: Arc<SharedState>,
) -> Result<cpal::Stream, String> {
```

So Task 3 Step 3 (`cargo build` → expected `Finished`) will fail with a wrong-number-of-arguments error.

**Fix:** drop the third argument:

```rust
let _stream = match audio::start_capture(tx_raw, shared.clone()) {
```

`speaking` is still legitimately created and passed to `worker::run` (line 380 matches the current 6-arg signature), so only the `start_capture` call needs trimming. `speaking` is now effectively diagnostic-only since capture no longer reads it — fine for this plan, just don't pass it to `start_capture`.

## Nit

### Stale `mock_state` note contradicts the revised Task 4

The "Notes for the implementer" (line 999) still says:

> Task 4 `tauri::test::mock_state`: Available in Tauri v2 test utilities. If the crate doesn't expose it, extract command logic into helper functions...

Task 4 was rewritten to use the `send_ctrl` helper and no longer references `mock_state`. Update or delete this note so it doesn't send an implementer down the wrong path.

## Overall recommendation

The plan is one line away from ready. Everything from the v1 review is resolved; the only build-stopping issue is the extra `speaking.clone()` argument in the `start_capture` call. Fix that (and tidy the stale `mock_state` note) and this is good to execute.
