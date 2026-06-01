# AEC Phase 2 — True Barge-in Design Spec

**Date:** 2026-06-01  
**Status:** Approved; pending implementation plan  
**Branch:** `feat/aec-phase2-gate-relaxation`  
**Builds on:** AEC Phase 1 (`docs/proposal/2026-06-01-acoustic-echo-cancellation.md`)  
**Phase 1 acceptance test:** `docs/bugfix/2026-06-01-aec-phase1-results.md`

## Context

Phase 1 (shadow mode) proved that Speex AEC cancels TTS echo at -10 to -37dB on this
hardware. The `speaking` flag that gates the audio capture is no longer needed for echo
prevention — it is now an obstacle to barge-in.

Phase 2 removes that gate and restructures the worker so TTS runs non-blocking, enabling
the user to speak and interrupt the assistant mid-sentence.

## Goal

The user can speak at any point — including while the app is playing TTS. The moment
VAD detects speech onset, TTS stops and the app processes the new utterance. No changes
to the AEC pipeline, no new crates, no async runtime.

## Decisions

| Area | Decision |
|------|----------|
| Gate removal | Remove `speaking.load()` check from cpal callback in `audio.rs` |
| TTS threading | Spawn a short-lived `std::thread` per utterance; worker returns to VAD loop immediately |
| Barge-in trigger | `VadEvent::Start` while `speaking=true` → `stop_tts.store(true)` immediately |
| Stale-frame drain | **Remove** `while rx_audio.try_recv().is_ok() {}` — AEC-cleaned frames are valid |
| `speaking` AtomicBool | Kept as a worker signal (TTS in-flight), no longer used in capture gate |

## Changes

### `rust/src/audio.rs` — remove capture gate

The cpal callback currently drops frames when `speaking` is true:
```rust
if !shared.listening_enabled.load(Ordering::Relaxed) || speaking.load(Ordering::Relaxed) {
    return;
}
```

Remove the `|| speaking.load(Ordering::Relaxed)` clause:
```rust
if !shared.listening_enabled.load(Ordering::Relaxed) {
    return;
}
```

Also remove the `speaking: Arc<AtomicBool>` parameter from `start_capture` since it is no
longer used there.

### `rust/src/worker.rs` — spawn TTS thread, add barge-in trigger, remove drain

**a) Barge-in trigger on `VadEvent::Start`**

After the VAD call, before the segmenter, check for barge-in:

```rust
let event = match vad.accept(&frame) { ... };

// Barge-in: if speech starts while TTS is playing, stop TTS immediately.
if event == Some(crate::segmenter::VadEvent::Start)
    && speaking.load(Ordering::SeqCst)
{
    stop_tts.store(true, Ordering::SeqCst);
}
```

**b) Spawn TTS on a separate thread**

Replace the current blocking section:
```rust
speaking.store(true, Ordering::SeqCst);
stop_tts.store(false, Ordering::SeqCst);
let _ = crate::tts::speak_stoppable(&client, &refined, &stop_tts, &rx_ctrl, Some(&echo));
speaking.store(false, Ordering::SeqCst);
reset_to_idle(&shared, &tx_ui, &mut vad);
while rx_audio.try_recv().is_ok() {}
```

With the non-blocking version:
```rust
{
    // Clone everything the TTS thread needs — it must be self-contained.
    let stop_c    = stop_tts.clone();
    let speaking_c = speaking.clone();
    let echo_c    = echo.clone();
    let client_c  = client.clone();
    let refined_c = refined.clone();
    // rx_ctrl is cloned so TTS thread can drain Stop messages during playback.
    // The worker's select! loop also reads rx_ctrl — Stop messages are idempotent.
    let rx_ctrl_c = rx_ctrl.clone();

    speaking.store(true, Ordering::SeqCst);
    stop_tts.store(false, Ordering::SeqCst);

    std::thread::spawn(move || {
        let _ = crate::tts::speak_stoppable(
            &client_c, &refined_c, &stop_c, &rx_ctrl_c, Some(&echo_c));
        echo_c.reset();
        speaking_c.store(false, Ordering::SeqCst);
    });
}
// Worker loops back immediately — does NOT block or drain.
reset_to_idle(&shared, &tx_ui, &mut vad);
// Stale-frame drain REMOVED: AEC-cleaned frames during TTS are valid for VAD.
```

**c) `run()` signature unchanged** — `speaking: Arc<AtomicBool>` stays as a parameter of
`worker::run`. It is still needed to check for barge-in and to signal the TTS thread.
The only change is removing it from `start_capture` in `audio.rs` — `main.rs` still
creates it and still passes it to `worker::run`.

### `rust/src/main.rs` — update `start_capture` call

Remove the `speaking` argument from `start_capture`:
```rust
let _stream = match audio::start_capture(tx_raw, shared.clone()) { ... };
```

`speaking` is still created and passed to `worker::run`.

### No changes to
`echo.rs`, `tts.rs`, `settings.rs`, `vad.rs`, `segmenter.rs` — all unchanged.

## State machine

```
Idle (listening)
   │ utterance detected
   ▼
Turn pipeline (STT → refine)
   │ refined text ready
   ▼
TTS thread spawned; speaking=true; worker loops back to Idle
   │
   ├─ User stays silent → TTS plays to end → speaking=false → Idle
   │
   └─ User speaks mid-TTS (VadEvent::Start):
         stop_tts=true → TTS thread stops → speaking=false
         User's utterance accumulates in segmenter
         VadEvent::End → utterance ready → new turn pipeline
```

## Error handling

- If the TTS thread panics, `speaking` stays `true` permanently → worker sees TTS as
  always in-flight. Fix: `speaking_c.store(false)` is in the TTS thread body, not a
  `finally`. Use `std::panic::catch_unwind` around `speak_stoppable` call inside the
  spawned thread, or rely on the worker's 30s `reqwest` timeout which would surface as
  an error before the TTS thread is spawned.
- If barge-in fires and TTS takes >100ms to stop (slow poll), the user's utterance is
  already being collected by the segmenter. No data is lost.

## Testing

**Unit test — barge-in state machine (pure logic, no I/O):**
```rust
// Simulate: speaking=true, VadEvent::Start arrives → stop_tts becomes true
fn test_barge_in_sets_stop_flag() {
    let speaking = Arc::new(AtomicBool::new(true));
    let stop_tts = Arc::new(AtomicBool::new(false));
    // The condition from worker.rs:
    let event = Some(crate::segmenter::VadEvent::Start);
    if event == Some(crate::segmenter::VadEvent::Start) && speaking.load(Ordering::SeqCst) {
        stop_tts.store(true, Ordering::SeqCst);
    }
    assert!(stop_tts.load(Ordering::SeqCst));
    assert!(speaking.load(Ordering::SeqCst)); // still true until TTS thread clears it
}

fn test_barge_in_does_not_fire_when_not_speaking() {
    let speaking = Arc::new(AtomicBool::new(false));
    let stop_tts = Arc::new(AtomicBool::new(false));
    let event = Some(crate::segmenter::VadEvent::Start);
    if event == Some(crate::segmenter::VadEvent::Start) && speaking.load(Ordering::SeqCst) {
        stop_tts.store(true, Ordering::SeqCst);
    }
    assert!(!stop_tts.load(Ordering::SeqCst)); // should NOT fire
}
```

**Manual acceptance tests:**
1. Speak during TTS → TTS cuts off immediately, app transcribes new sentence, responds.
2. Don't speak → TTS completes naturally, no regression.
3. Speak immediately after TTS ends → normal turn, no false trigger from reverb tail
   (AEC handles this, gate is gone).
4. Press Stop during TTS → still works (stop_tts mechanism unchanged).
5. Mic toggle (mute) → still works (listening_enabled gate still in capture callback).
6. Multiple barge-ins in a row → stable; no deadlock; `speaking` always returns to false.

## Out of scope

TTS queuing / cross-fade, `async` restructure, Phase 3 worker architecture.
