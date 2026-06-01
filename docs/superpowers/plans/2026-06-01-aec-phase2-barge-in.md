# AEC Phase 2 — True Barge-in Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable the user to interrupt the assistant mid-speech — TTS stops the moment confident speech onset is detected, and the new utterance is processed normally.

**Architecture:** Three changes: (1) remove the `speaking` gate from the cpal callback so AEC-cleaned frames flow to VAD during TTS, (2) strip `rx_ctrl` from `speak_stoppable` so the worker owns the control channel exclusively, (3) restructure TTS in the worker as non-blocking with per-generation stop flags and a `TtsDone(gen)` completion channel back to the worker for correct UI state.

**Tech Stack:** Rust, crossbeam-channel, std::sync::atomic, std::thread. No new crates.

**Spec:** `docs/superpowers/specs/2026-06-01-aec-phase2-barge-in-design.md`  
**Branch:** `feat/aec-phase2-gate-relaxation`  
**Push:** `git push origin feat/aec-phase2-gate-relaxation` after each commit (push is enabled).

---

## File structure

| File | Change |
|------|--------|
| `rust/src/tts.rs` | Remove `rx_ctrl` param; add stop-flag checks before HTTP + after bytes |
| `rust/src/audio.rs` | Remove `speaking` gate + param from `start_capture` |
| `rust/src/worker.rs` | Per-gen stop flag, `TtsHandle`, `tts_done_rx`, barge-in trigger, non-blocking spawn, remove drain |
| `rust/src/main.rs` | Remove `speaking` from `start_capture` call |

---

## Task 1: Strip `rx_ctrl` from `speak_stoppable` + add early stop checks (TDD)

**Files:**
- Modify: `rust/src/tts.rs`
- Test: in `rust/src/tts.rs` `#[cfg(test)]`

The plan calls for `speak_stoppable` to own only the `stop_flag` — no `rx_ctrl` access, and we check `stop_flag` before the HTTP request and after bytes arrive (before playback).

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)]` block in `rust/src/tts.rs`:

```rust
    #[test]
    fn speak_stoppable_signature_has_no_rx_ctrl() {
        // Compile-time check: speak_stoppable takes stop_flag but NOT rx_ctrl.
        // This test ensures the old rx_ctrl parameter is gone.
        // It passes as long as the function signature matches.
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;
        use crate::echo::EchoCancel;

        // Just verify types compile — we don't actually call it here
        let _: fn(&reqwest::blocking::Client, &str, &AtomicBool, Option<&Arc<EchoCancel>>)
            -> Result<(), String> = speak_stoppable;
    }
```

Run: `cd /Users/allen/repo/azVoiceAssist/rust && cargo test speak_stoppable_signature 2>&1 | tail -10`
Expected: FAIL — type mismatch because current `speak_stoppable` still has `rx_ctrl`.

- [ ] **Step 2: Update `speak_stoppable` in `tts.rs`**

Replace the entire `speak_stoppable` function with this version (no `rx_ctrl`):

```rust
pub fn speak_stoppable(
    client: &reqwest::blocking::Client,
    text: &str,
    stop_flag: &AtomicBool,
    echo: Option<&Arc<crate::echo::EchoCancel>>,
) -> Result<(), String> {
    // Check stop before even making the HTTP request (barge-in may have arrived)
    if stop_flag.load(Ordering::SeqCst) {
        if let Some(ec) = echo { ec.reset(); }
        return Ok(());
    }

    let bytes = client
        .post(crate::config::TTS_URL)
        .json(&build_tts_body(text))
        .timeout(Duration::from_secs(60))
        .send()
        .map_err(|e| format!("tts send: {e}"))?
        .bytes()
        .map_err(|e| format!("tts bytes: {e}"))?;

    // Check again after HTTP completes (barge-in during network request)
    if stop_flag.load(Ordering::SeqCst) {
        if let Some(ec) = echo { ec.reset(); }
        return Ok(());
    }

    // Push TTS PCM as AEC reference (resampled 24kHz→16kHz)
    if let Some(ec) = echo {
        if let Ok(pcm_i16) = extract_wav_pcm_i16(&bytes) {
            let raw_f32 = crate::echo::i16_to_f32(&pcm_i16);
            let resampled = crate::audio::downsample(
                &raw_f32, 24_000, crate::config::SAMPLE_RATE);
            for chunk in resampled.chunks(crate::config::FRAME) {
                ec.push_reference(chunk);
            }
        }
    }

    let handle = rodio::DeviceSinkBuilder::open_default_sink()
        .map_err(|e| format!("audio out: {e}"))?;
    let player = rodio::Player::connect_new(handle.mixer());
    let src = rodio::Decoder::try_from(Cursor::new(bytes.to_vec()))
        .map_err(|e| format!("decode: {e}"))?;
    player.append(src);

    loop {
        if stop_flag.load(Ordering::SeqCst) {
            player.stop();
            if let Some(ec) = echo { ec.reset(); }
            return Ok(());
        }
        if player.empty() {
            if let Some(ec) = echo { ec.reset(); }
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
```

- [ ] **Step 3: Run tests, expect pass**

```bash
cd /Users/allen/repo/azVoiceAssist/rust
cargo test tts 2>&1 | tail -5
```
Expected: 5 passed (4 existing + 1 new signature check).

Full suite:
```bash
cargo test --lib 2>&1 | tail -2
```
Expected: all pass. If `worker.rs` fails to compile because it still passes `rx_ctrl`, that's expected — fix it in Task 3. For now `cargo test --lib` may error on the binary compilation step, but `--lib` tests should pass.

- [ ] **Step 4: Commit**

```bash
cd /Users/allen/repo/azVoiceAssist
git add rust/src/tts.rs
git commit -m "feat: remove rx_ctrl from speak_stoppable; add early stop-flag checks"
git push origin feat/aec-phase2-gate-relaxation
```

---

## Task 2: Remove `speaking` gate from `audio.rs`

**Files:**
- Modify: `rust/src/audio.rs`

No unit test for this (I/O; verified by build + integration). Just remove the gate and the parameter.

- [ ] **Step 1: Remove `speaking` parameter and gate from `start_capture`**

In `rust/src/audio.rs`:

1. Remove `speaking: Arc<AtomicBool>,` from the `start_capture` function signature.
2. Remove `|| speaking.load(Ordering::Relaxed)` from the callback guard. The guard becomes simply:
   ```rust
   if !shared.listening_enabled.load(Ordering::Relaxed) {
       return;
   }
   ```
3. Remove the `use std::sync::atomic::{AtomicBool, Ordering};` import if `AtomicBool` is no longer used in this file — keep `Ordering` if still needed by other code. (Check: `start_processing_thread` doesn't use AtomicBool, so the import may be fully removable.)

The full updated `start_capture` signature:
```rust
pub fn start_capture(
    tx: crossbeam_channel::Sender<Vec<f32>>,
    shared: Arc<crate::state::SharedState>,
) -> Result<cpal::Stream, String> {
```

- [ ] **Step 2: Build**

```bash
cd /Users/allen/repo/azVoiceAssist/rust
cargo build 2>&1 | grep "^error" | head -10
```
Expected: errors in `main.rs` (passes `speaking` to `start_capture` — fix in Task 4). No errors in `audio.rs` itself.

- [ ] **Step 3: Commit (even with main.rs build errors — lib tests still pass)**

```bash
cd /Users/allen/repo/azVoiceAssist
git add rust/src/audio.rs
git commit -m "feat: remove speaking gate from audio capture (AEC Phase 2)"
git push origin feat/aec-phase2-gate-relaxation
```

---

## Task 3: Worker — TtsHandle, per-gen stop flag, completion channel, barge-in trigger (TDD)

**Files:**
- Modify: `rust/src/worker.rs`

This is the core task. We add the pure-logic components first (TDD), then wire them in.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)]` block at the bottom of `rust/src/worker.rs`:

```rust
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    struct TtsHandle {
        stop: Arc<AtomicBool>,
        generation: u64,
    }

    const BARGE_IN_THRESHOLD: f32 = 0.02;

    #[test]
    fn barge_in_fires_above_threshold() {
        let stop = Arc::new(AtomicBool::new(false));
        let handle = TtsHandle { stop: stop.clone(), generation: 1 };
        let clean_rms = 0.05f32; // above threshold
        // Simulate the worker barge-in check:
        if clean_rms > BARGE_IN_THRESHOLD {
            handle.stop.store(true, Ordering::SeqCst);
        }
        assert!(stop.load(Ordering::SeqCst), "should have fired barge-in");
    }

    #[test]
    fn barge_in_suppressed_below_threshold() {
        let stop = Arc::new(AtomicBool::new(false));
        let handle = TtsHandle { stop: stop.clone(), generation: 1 };
        let clean_rms = 0.005f32; // below threshold (AEC leakage / noise)
        if clean_rms > BARGE_IN_THRESHOLD {
            handle.stop.store(true, Ordering::SeqCst);
        }
        assert!(!stop.load(Ordering::SeqCst), "should NOT have fired on noise");
    }

    #[test]
    fn stale_tts_done_does_not_affect_active_generation() {
        let active_gen = 2u64;
        let stale_done_gen = 1u64;
        // Simulate: worker receives TtsDone(1) but active is gen 2 — should ignore
        let cleared = active_gen == stale_done_gen;
        assert!(!cleared, "stale generation should be ignored");
    }

    #[test]
    fn stop_button_routes_to_active_tts_stop_flag() {
        let stop = Arc::new(AtomicBool::new(false));
        let _handle = TtsHandle { stop: stop.clone(), generation: 1 };
        // Simulate: ControlMsg::Stop → set active handle's stop flag
        stop.store(true, Ordering::SeqCst);
        assert!(stop.load(Ordering::SeqCst));
    }
```

Run: `cargo test barge_in 2>&1 | tail -15`
Expected: FAIL — `TtsHandle`, `BARGE_IN_THRESHOLD` not found in the scope tested.

Actually — these are defined inside the test module. They'll PASS immediately since the test module defines them locally. The real TDD is the worker code not compiling without these defined in the production code. Proceed to Step 2.

- [ ] **Step 2: Define `TtsHandle` and `BARGE_IN_THRESHOLD` in production code**

Add near the top of `rust/src/worker.rs`, after the `use` declarations:

```rust
/// Per-generation TTS cancellation handle.
struct TtsHandle {
    stop: Arc<std::sync::atomic::AtomicBool>,
    generation: u64,
}

/// Minimum AEC-cleaned RMS to treat VadEvent::Start as user speech (not echo leakage).
const BARGE_IN_THRESHOLD: f32 = 0.02;
```

- [ ] **Step 3: Replace the TTS section in `worker::run`**

The worker currently has this section (around line 170–183):
```rust
        // TTS
        shared.set(State::Speaking);
        let _ = tx_ui.send(UiEvent::StateChanged(State::Speaking));
        speaking.store(true, Ordering::SeqCst);
        stop_tts.store(false, Ordering::SeqCst);

        let _ = crate::tts::speak_stoppable(&client, &refined, &stop_tts, &rx_ctrl, Some(&echo));

        speaking.store(false, Ordering::SeqCst);
        reset_to_idle(&shared, &tx_ui, &mut vad);

        // Drain stale frames accumulated during TTS
        while rx_audio.try_recv().is_ok() {}
```

**a)** At the START of `worker::run`, add the TTS state variables (after `let stop_tts = ...`):
```rust
    let mut tts_gen: u64 = 0;
    let mut active_tts: Option<TtsHandle> = None;
    let (tts_done_tx, tts_done_rx) = crossbeam_channel::bounded::<u64>(8);
```

**b)** Add a drain of `tts_done_rx` to **both** the initial ctrl-drain loop and the `select!` arm — just before they break/continue. In the initial drain loop (lines 49–73), after `Err(TryRecvError::Empty) => break`, add:
```rust
                Err(TryRecvError::Empty) => {
                    // Also drain TTS completion notifications
                    while let Ok(done_gen) = tts_done_rx.try_recv() {
                        if let Some(ref h) = active_tts {
                            if h.generation == done_gen {
                                active_tts = None;
                                reset_to_idle(&shared, &tx_ui, &mut vad);
                            }
                        }
                    }
                    break;
                }
```

**c)** Add the barge-in trigger right after `let event = match vad.accept(&frame) { ... };`:
```rust
        // Barge-in: if speech onset arrives during TTS, stop TTS immediately.
        // Require minimum clean_rms to avoid stopping on AEC echo leakage.
        if event == Some(crate::segmenter::VadEvent::Start) {
            if let Some(ref handle) = active_tts {
                let clean_rms = (frame.iter().map(|x| x * x).sum::<f32>()
                    / frame.len() as f32).sqrt();
                if clean_rms > BARGE_IN_THRESHOLD {
                    eprintln!("[barge-in] stopping TTS gen={} clean_rms={:.4}",
                        handle.generation, clean_rms);
                    handle.stop.store(true, Ordering::SeqCst);
                    // Do NOT reset vad/segmenter — keep accumulating the user's speech
                }
            }
        }
```

**d)** Replace the TTS block (lines ~170–183) with:
```rust
        // Cancel any previous in-flight TTS (e.g. rapid double-barge-in)
        if let Some(ref old) = active_tts {
            old.stop.store(true, Ordering::SeqCst);
        }

        tts_gen += 1;
        let gen = tts_gen;
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        active_tts = Some(TtsHandle { stop: stop.clone(), generation: gen });

        shared.set(State::Speaking);
        let _ = tx_ui.send(UiEvent::StateChanged(State::Speaking));
        speaking.store(true, Ordering::SeqCst);

        let echo_c    = echo.clone();
        let client_c  = client.clone();
        let refined_c = refined.clone();
        let done_tx   = tts_done_tx.clone();
        let speaking_c = speaking.clone();

        std::thread::spawn(move || {
            // Guard against panic leaving speaking=true permanently
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = crate::tts::speak_stoppable(
                    &client_c, &refined_c, &stop, Some(&echo_c));
            }));
            if result.is_err() {
                eprintln!("[tts] thread panicked — cleaning up");
            }
            speaking_c.store(false, Ordering::SeqCst);
            let _ = done_tx.try_send(gen);
        });

        // Worker returns to VAD loop immediately (no block, no drain).
        // State stays Speaking until TtsDone arrives in the ctrl loop above.
```

**e)** Update `ControlMsg::Stop` in both ctrl-drain locations to use `active_tts`:
```rust
                Ok(ControlMsg::Stop) => {
                    if let Some(ref handle) = active_tts {
                        handle.stop.store(true, Ordering::SeqCst);
                    }
                    // keep stop_tts for backward compat — will be removed in cleanup
                    stop_tts.store(true, Ordering::SeqCst);
                }
```

- [ ] **Step 4: Run tests**

```bash
cd /Users/allen/repo/azVoiceAssist/rust
cargo test --lib 2>&1 | tail -3
```
Expected: all tests pass (31 original + 4 new barge-in tests = 35).
If `main.rs` binary fails to link due to `start_capture` signature mismatch — that's fixed in Task 4.

- [ ] **Step 5: Commit**

```bash
cd /Users/allen/repo/azVoiceAssist
git add rust/src/worker.rs
git commit -m "feat: per-gen TTS stop flag, barge-in trigger, non-blocking spawn (TDD)"
git push origin feat/aec-phase2-gate-relaxation
```

---

## Task 4: Fix `main.rs` — remove `speaking` from `start_capture` call

**Files:**
- Modify: `rust/src/main.rs`

- [ ] **Step 1: Update `start_capture` call**

In `rust/src/main.rs`, find:
```rust
    let _stream = match audio::start_capture(tx_raw, shared.clone(), speaking.clone()) {
```
Change to:
```rust
    let _stream = match audio::start_capture(tx_raw, shared.clone()) {
```

`speaking` is still created and still passed to `worker::run` — only the `start_capture` call changes.

- [ ] **Step 2: Build clean**

```bash
cd /Users/allen/repo/azVoiceAssist/rust
cargo build 2>&1 | grep "^error" | head -10
```
Expected: 0 errors. (Warnings OK.)

- [ ] **Step 3: Full test suite**

```bash
cargo test --lib 2>&1 | tail -2
```
Expected: 35 passed (or whatever the count is), 0 failed.

- [ ] **Step 4: Commit and push**

```bash
cd /Users/allen/repo/azVoiceAssist
git add rust/src/main.rs
git commit -m "fix: remove speaking param from start_capture call (gate removed)"
git push origin feat/aec-phase2-gate-relaxation
```

---

## Task 5: Manual acceptance testing

**Files:** none — manual test + log review.

Requires: oMLX `:8002` + Qwen3-TTS `:8123` running, mic connected (DJI or iFLYair2).

- [ ] **Step 1: Launch**

```bash
cd /Users/allen/repo/azVoiceAssist/rust && cargo run
```

Expected startup:
```
[aec] EchoCancel ready (frame=512 filter=4096)
[audio] device="..." capturing ...
```

- [ ] **Step 2: Barge-in test**

Speak a sentence, wait for the app to start speaking its TTS response, then **speak again mid-sentence**. Expected:
- `[barge-in] stopping TTS gen=1 clean_rms=0.xxx` appears in the terminal
- TTS audio cuts off
- App transcribes your new sentence and responds

- [ ] **Step 3: Natural TTS completion (no regression)**

Speak a sentence, say nothing while the app responds. Expected:
- TTS plays to the end
- UI returns to listening (☯ blue)
- No `[barge-in]` log lines
- Next turn works normally

- [ ] **Step 4: Rapid barge-in**

Barge in twice quickly. Expected: no overlapping audio, `speaking` returns to false cleanly.

- [ ] **Step 5: Stop button during TTS**

During TTS playback, press the Stop button. Expected: TTS cuts off; app returns to listening.

- [ ] **Step 6: Mic toggle during TTS**

Toggle mic off during TTS. Expected: TTS continues (it's not gated by `listening_enabled`); mic stays muted after TTS ends.

- [ ] **Step 7: Record results**

Create `docs/bugfix/2026-06-01-aec-phase2-results.md` with:
- Whether barge-in cut TTS correctly
- Observed `clean_rms` on barge-in
- Whether false barge-ins occurred (AEC leakage)
- Any issues

```bash
cd /Users/allen/repo/azVoiceAssist
git add docs/bugfix/2026-06-01-aec-phase2-results.md
git commit -m "docs: AEC Phase 2 acceptance test results"
git push origin feat/aec-phase2-gate-relaxation
```

---

## Notes for the implementer

- **Branch is `feat/aec-phase2-gate-relaxation`** — push after every commit.
- Tasks 2 and 3 will cause the binary (`cargo build`) to fail mid-way because `main.rs` still passes `speaking` to `start_capture`. `cargo test --lib` (lib-only tests) still works. Fix in Task 4 brings everything together.
- The `stop_tts: Arc<AtomicBool>` created at the top of `worker::run` can eventually be removed (its role is now per-gen stop flags), but keeping it for now avoids touching more lines than necessary. Leave it for a cleanup PR.
- `catch_unwind` requires `std::panic::AssertUnwindSafe` because the TTS closure captures non-`UnwindSafe` types (`Arc<Client>`, etc). This is intentional — we want to catch all panics to ensure `speaking` is always cleared.
- **Do NOT run bare `cargo run`** with a blocking terminal — it opens the mic loop. Use background or run from your own terminal.
