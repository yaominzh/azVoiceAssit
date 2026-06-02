# Third-Party Review — AEC Phase 2 Barge-in Design Spec

**Reviewed doc:** `docs/superpowers/specs/2026-06-01-aec-phase2-barge-in-design.md`  
**Date:** 2026-06-01  
**Grounded against:** `rust/src/audio.rs`, `rust/src/worker.rs`, `rust/src/tts.rs`, `rust/src/echo.rs`, `rust/src/vad.rs`, `docs/bugfix/2026-06-01-aec-phase1-results.md`

## Verdict

The direction is mostly right: removing the capture-side `speaking` gate plus making TTS non-blocking is the correct architectural move for real barge-in.

However, I would **not approve the spec as written** because it misses several race conditions and directly contradicts Phase 1's own go/no-go conditions. The biggest risks are:

- **A new turn can start while old TTS is still playing**, causing overlapping TTS threads.
- **UI state becomes inaccurate** because the worker resets to idle immediately after spawning TTS.
- **`rx_ctrl` is consumed by two threads**, so mic/settings/clear commands can be swallowed by the TTS thread.
- **Phase 1 explicitly recommended gradual gate relaxation**, but this spec removes the gate in one step.

## Should-fix

### 1. This can spawn overlapping TTS threads

The proposed worker flow returns to VAD immediately after spawning TTS. That means a user can barge in, produce a new utterance, and complete `STT → refine → spawn new TTS` **before the old TTS thread has actually stopped**.

`stop_tts.store(true)` requests stop, but `speak_stoppable()` only polls every 50ms and may also be blocked before playback starts during the TTS HTTP request. Then the worker reaches the next TTS section and does:

```rust
speaking.store(true, Ordering::SeqCst);
stop_tts.store(false, Ordering::SeqCst);
std::thread::spawn(...)
```

That clears the same shared stop flag the old TTS thread is relying on.

**Recommendation:**

- Treat TTS as a single in-flight resource.
- Before spawning new TTS:
  - signal stop on existing TTS,
  - wait for it to acknowledge/finish, or
  - use a per-utterance cancellation token/generation ID so new TTS cannot clear the old stop request.
- Prefer a `TtsHandle` model:
  - `stop: Arc<AtomicBool>`
  - `done_rx` / `JoinHandle`
  - `generation: u64`

### 2. One shared `stop_tts` flag is unsafe for multiple TTS generations

The current spec keeps a single `Arc<AtomicBool>` created once in `worker::run`.

That worked when TTS was blocking because only one call to `speak_stoppable()` could exist at a time. After threading, multiple calls can overlap unless carefully prevented. A single flag introduces cross-talk:

- New utterance resets `stop_tts=false`.
- Old TTS resumes or fails to stop.
- Stop button may stop the wrong generation.
- A stale TTS thread can clear `speaking=false` after a newer TTS has started.

**Recommendation:**

- Allocate a **fresh stop flag per TTS thread**.
- Track the active TTS generation in the worker.
- A TTS thread should only clear `speaking=false` if it is still the active generation.

Example conceptually:

```rust
let gen = next_tts_gen;
active_gen.store(gen);
let stop = Arc::new(AtomicBool::new(false));

std::thread::spawn(move || {
    let _ = speak_stoppable(..., &stop, ...);
    if active_gen.load(Ordering::SeqCst) == gen {
        speaking.store(false, Ordering::SeqCst);
    }
});
```

The exact implementation can vary, but the spec should require generation isolation.

### 3. `rx_ctrl` must not be consumed by both worker and TTS thread

The spec says clone `rx_ctrl` and let the TTS thread drain Stop messages, while the worker also reads `rx_ctrl`.

Crossbeam receivers are competing consumers. A cloned receiver does **not** broadcast messages. The TTS thread can accidentally consume:

- `ToggleMic`
- `Clear`
- `SettingsChanged`

Current `tts.rs` already documents this bad behavior:

```rust
// other messages are silently dropped here and will be re-processed on the next worker loop iteration
// (they won't arrive again...)
```

That was already problematic but limited to blocking TTS. In Phase 2, it becomes worse because the worker is now alive and should own controls.

**Recommendation:**

- Do **not** pass `rx_ctrl` to TTS thread in Phase 2.
- Worker should be the sole control-message consumer.
- On `ControlMsg::Stop`, worker sets the active TTS stop flag.
- Refactor `speak_stoppable()` so it only receives `stop_flag`, not `rx_ctrl`.

This is a key design correction.

### 4. UI state is wrong: worker resets to idle while TTS is still playing

The proposed code does:

```rust
std::thread::spawn(...);
reset_to_idle(&shared, &tx_ui, &mut vad);
```

But TTS is audibly playing. The UI will show `listening` instead of `speaking`, even though `speaking=true`.

The old design showed `State::Speaking` while playback was active. Phase 2 should preserve that, or deliberately introduce a new visual state like `speaking/listening`.

**Recommendation:**

- Keep UI state as `Speaking` while TTS is in-flight, or add an explicit "speaking but interruptible" state/indicator.
- On TTS thread completion, send the state back to idle.
- If the worker thread alone owns UI state, have TTS thread send a `TtsDone(generation)` message back to worker instead of directly mutating state.
- Avoid calling `reset_to_idle()` immediately after spawn unless the UI semantics intentionally change.

### 5. TTS thread should not send UI/state changes directly without ownership rules

The proposed spawned thread clears `speaking`, but does not update `SharedState` or the UI. Adding UI sends from that thread may be tempting, but then state ownership gets split between worker and TTS thread.

**Recommendation:**

- Keep state transitions owned by the worker where possible.
- Add an internal worker channel for TTS completion:
  - TTS thread sends `TtsFinished(generation)`.
  - Worker validates generation and then updates:
    - `speaking=false`
    - `State::Listening` / `State::Muted`
    - UI event.

This also solves stale completion from older TTS generations.

### 6. Phase 1 conditions are not incorporated

Phase 1 results explicitly say:

- Maintain a post-TTS window before enabling AEC-cleaned audio for VAD.
- Relax the speaking gate gradually, not in one step.
- Consider a convergence indicator before enabling AEC output for VAD.

The Phase 2 spec does the opposite: remove the gate fully and immediately.

**Recommendation:**

Either revise Phase 2 to incorporate Phase 1's conditions, or explicitly justify why they are no longer needed.

A safer design:

- Remove capture gate, yes.
- But add a worker-side VAD eligibility guard:
  - during TTS, accept VAD only after N AEC frames or a cancellation threshold is met;
  - after TTS, optionally keep a short suppression/tail window unless AEC confidence is good.
- Log barge-in decisions:
  - `raw_rms`
  - `clean_rms`
  - reduction dB
  - whether VAD start was treated as barge-in.

### 7. `VadEvent::Start` while speaking may be residual echo, not user speech

Phase 1 showed positive dB frames and said some frames may represent user speech, but it did not prove that **every** `VadEvent::Start` during TTS is user speech. With the gate removed, any AEC miss can fire `VadEvent::Start`, which will cut off TTS.

**Recommendation:**

- Add a barge-in confidence rule, at least initially:
  - require `clean_rms` above threshold,
  - require raw-to-clean behavior consistent with near-end speech,
  - or require 2 consecutive speech frames before stopping TTS.
- If keeping "stop immediately on first Start," mark it as aggressive and require manual validation.

### 8. Segmenter/VAD reset behavior after barge-in is under-specified

The spec says:

> User's utterance accumulates in segmenter

That is plausible, because the start frame is passed to `seg.push(frame, event)` after stopping TTS. But the old TTS might still be playing for up to the polling interval, and residual AEC/adaptation state can affect the first few frames.

**Recommendation:**

- Specify whether `vad.reset()` should happen on barge-in or not.
- Likely **do not reset VAD/segmenter on barge-in**, because that could lose onset/pre-roll.
- But do reset/flush AEC reference on confirmed TTS stop, not on `VadEvent::Start` itself, unless measured.

### 9. `catch_unwind` note is incomplete

The spec recognizes that panic can leave `speaking=true`, but the proposed fix is vague. Also `catch_unwind` may require `AssertUnwindSafe` around captured values.

**Recommendation:**

- Require a guard pattern so `speaking` is cleared on all exits.
- Better: TTS thread should send completion over a channel in a `catch_unwind` wrapper, and worker owns final state cleanup.
- Also handle normal `Err` from `speak_stoppable()` by clearing state and logging.

## Consider

### 10. Stop latency is bounded by a 50ms poll plus blocking TTS request time

During playback, 50ms polling is probably acceptable. But before playback starts, `speak_stoppable()` can block on:

```rust
client.post(...).timeout(Duration::from_secs(60)).send()
```

If the user speaks while the TTS HTTP request is still pending, the stop flag is set, but `speak_stoppable()` does not check it until after bytes arrive.

**Recommendation:**

- Check `stop_flag`:
  - before sending the TTS request,
  - after receiving bytes,
  - before starting playback.
- Consider shorter TTS request timeout or cancellation in later phases.

### 11. `echo.reset()` is duplicated

Current `speak_stoppable()` already calls `ec.reset()` on stop and natural end. The spec's spawned thread also calls:

```rust
echo_c.reset();
```

This is probably harmless, but redundant.

**Recommendation:**

- Keep reset responsibility in one place.
- If `speak_stoppable()` owns playback/reference lifecycle, let it own AEC reset.

### 12. Capture channel backpressure/drop behavior needs validation

Removing the capture gate means frames flow continuously during TTS. Current channels are bounded and use `try_send`, so frames can be silently dropped.

This is probably acceptable for real-time behavior, but barge-in depends on not dropping the crucial onset frames.

**Recommendation:**

- Add logging/metrics for dropped raw/processed frames.
- Consider prioritizing recent frames or increasing channel sizing if onset drops happen.

### 13. Tests are too thin for the new concurrency

The proposed unit tests only check a boolean condition. They won't catch the real bugs above.

Add pure-ish tests for:

- **Generation isolation:** old TTS completion cannot clear `speaking` for newer TTS.
- **Stop routing:** worker consumes `ControlMsg::Stop` and active TTS stop flag flips.
- **No cloned control receiver swallowing:** `ToggleMic`, `Clear`, `SettingsChanged` remain worker-owned.
- **State transition:** UI remains `Speaking` while TTS active, then returns to idle on active generation completion.
- **Barge-in trigger:** `VadEvent::Start` during active TTS stops only the active generation.
- **Sequential barge-ins:** repeated interruption does not create overlapping audible playback.

## Nits

### 14. `Phase 3 worker architecture` out-of-scope wording is stale

The spec says Phase 2 restructures worker for non-blocking TTS, but Out of scope says "Phase 3 worker architecture." That is confusing.

**Recommendation:**

- Rename out-of-scope to "larger async/task architecture" or "TTS queue/cross-fade."
- Phase 2 already includes a worker architecture change.

### 15. Status says Approved, but review found blockers

If this doc is still under review, consider downgrading:

```markdown
**Status:** Draft / pending review
```

until the concurrency model is fixed.

## Suggested revised design

I'd revise around this simpler ownership model:

- **Capture thread:** gated only by `listening_enabled`; no `speaking` gate.
- **Audio processing thread:** always emits AEC-cleaned frames.
- **Worker thread:** sole owner of:
  - controls,
  - VAD/segmenter,
  - UI state,
  - active TTS handle/generation.
- **TTS thread:** owns playback only; receives:
  - text,
  - per-generation `stop_flag`,
  - `echo`,
  - completion sender.
- **Control channel:** consumed only by worker.
- **TTS completion channel:** worker receives `TtsDone(generation)`.
- **Barge-in:** worker sees `VadEvent::Start` while active TTS exists, sets that generation's stop flag, and continues feeding the segmenter.

## Overall recommendation

Approve the **goal**, but revise the implementation design before building it.

The core must-fix items are:

- **Per-TTS generation isolation**
- **Single owner for `rx_ctrl`**
- **Correct UI/state ownership**
- **No overlapping TTS threads**
- **Explicit handling of Phase 1's gradual-relaxation conditions**
