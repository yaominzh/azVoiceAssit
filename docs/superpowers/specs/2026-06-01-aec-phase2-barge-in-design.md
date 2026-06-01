# AEC Phase 2 — True Barge-in Design Spec

**Date:** 2026-06-01  
**Status:** Draft — rev 2 (incorporates third-party review)
**Branch:** `feat/aec-phase2-gate-relaxation`  
**Builds on:** AEC Phase 1 (`docs/proposal/2026-06-01-acoustic-echo-cancellation.md`)  
**Phase 1 results:** `docs/bugfix/2026-06-01-aec-phase1-results.md`

## Context

Phase 1 (shadow mode) proved Speex AEC cancels TTS echo at -10 to -37dB on this hardware.
The `speaking` flag that gates the audio capture is no longer needed for echo prevention —
it is now an obstacle to barge-in. Phase 2 removes the gate and restructures TTS as
non-blocking, enabling the user to speak and interrupt the assistant mid-sentence.

**Why full gate removal is justified now:** Phase 1 explicitly recommended gradual
relaxation as a safety net for *unproven* AEC. Phase 1 proved the AEC. Full removal is
the right move; the barge-in confidence guard (§Barge-in trigger) replaces "gradual
relaxation" with a signal-quality check.

## Goal

User can speak at any point — including while the app is playing TTS. The moment VAD
detects confident speech onset, TTS stops and the app processes the new utterance.

## Ownership model

```
Capture thread    → gated only by listening_enabled (no speaking gate)
Processing thread → always emits AEC-cleaned frames
Worker thread     → sole owner of: rx_ctrl, VAD/segmenter, UI state, active TTS handle
TTS thread        → owns playback only; receives text, per-gen stop_flag, echo, done_tx
```

`rx_ctrl` is consumed **only by the worker**. TTS thread has no channel access.

## Key design decisions

| Area | Decision |
|------|----------|
| Capture gate | Remove `speaking.load()` from cpal callback; keep `listening_enabled` gate |
| TTS threading | Spawn per-utterance thread; worker returns to VAD loop immediately |
| Barge-in trigger | `VadEvent::Start` + `clean_rms > BARGE_IN_THRESHOLD` → set active generation's stop flag |
| Per-generation stop flag | Each TTS spawn gets a fresh `Arc<AtomicBool>`; generation counter prevents stale clears |
| TTS completion | TTS thread sends `TtsDone(generation)` over a channel; worker updates state and UI |
| `rx_ctrl` ownership | Worker owns `rx_ctrl` exclusively; remove it from `speak_stoppable` signature |
| UI state during TTS | Stays `Speaking` until `TtsDone(active_gen)` arrives at worker |
| VAD/segmenter on barge-in | Do NOT reset — segmenter keeps accumulating the user's speech |
| Stale-frame drain | **Removed** — AEC-cleaned frames during TTS are VAD-valid |
| `echo.reset()` | Owned by `speak_stoppable` only (remove duplicate from spawned-thread code) |

## New types

```rust
// Sent from TTS thread back to worker on completion (natural end or stop).
enum WorkerMsg {
    TtsDone(u64),  // generation that completed
}

// Active TTS handle, owned by worker
struct TtsHandle {
    stop: Arc<AtomicBool>,   // per-generation stop flag
    generation: u64,
}
```

## Changes

### `rust/src/tts.rs` — remove `rx_ctrl` parameter

`speak_stoppable` currently drains `rx_ctrl` for Stop messages. With the worker alive
during TTS, the worker must be the sole consumer. Remove `rx_ctrl` from the signature and
check only `stop_flag`:

```rust
pub fn speak_stoppable(
    client: &reqwest::blocking::Client,
    text: &str,
    stop_flag: &AtomicBool,
    echo: Option<&Arc<EchoCancel>>,  // rx_ctrl parameter removed
) -> Result<(), String> {
    // Check stop before even making the HTTP request
    if stop_flag.load(Ordering::SeqCst) { return Ok(()); }

    let bytes = client.post(TTS_URL).json(&build_tts_body(text))
        .timeout(Duration::from_secs(60)).send()...;

    // Check again after HTTP completes (barge-in may have arrived during request)
    if stop_flag.load(Ordering::SeqCst) {
        if let Some(ec) = echo { ec.reset(); }
        return Ok(());
    }

    // ... existing playback + polling loop, stop_flag check only (no rx_ctrl) ...
}
```

### `rust/src/audio.rs` — remove `speaking` gate

Remove `|| speaking.load(Ordering::Relaxed)` from the cpal callback. Remove `speaking`
parameter from `start_capture`. `listening_enabled` gate stays.

### `rust/src/worker.rs` — non-blocking TTS, generation isolation, completion channel

**New state in `worker::run`:**
```rust
let mut tts_gen: u64 = 0;
let mut active_tts: Option<TtsHandle> = None;
let (tts_done_tx, tts_done_rx) = bounded::<u64>(8); // TtsDone(generation) channel
const BARGE_IN_THRESHOLD: f32 = 0.02; // min clean_rms to treat as user speech
```

**Barge-in trigger (after VAD, before segmenter):**
```rust
let event = match vad.accept(&frame) { ... };

// Barge-in: confident speech onset while TTS is in-flight.
if event == Some(VadEvent::Start) {
    if let Some(ref handle) = active_tts {
        // Require minimum RMS to avoid stopping on AEC leakage
        let clean_rms = (frame.iter().map(|x| x*x).sum::<f32>()
            / frame.len() as f32).sqrt();
        if clean_rms > BARGE_IN_THRESHOLD {
            handle.stop.store(true, Ordering::SeqCst);
            // Do NOT reset VAD/segmenter — continue accumulating user's speech
        }
    }
}
```

**Drain `tts_done_rx` in the ctrl-drain loop** (before select!):
```rust
while let Ok(gen) = tts_done_rx.try_recv() {
    if let Some(ref handle) = active_tts {
        if handle.generation == gen {
            active_tts = None;
            // TTS complete — update state
            reset_to_idle(&shared, &tx_ui, &mut vad);
        }
    }
}
```

**Spawn TTS non-blocking:**
```rust
// Cancel any previous in-flight TTS (overlapping barge-in)
if let Some(ref old) = active_tts {
    old.stop.store(true, Ordering::SeqCst);
    // Do not wait — old thread will clear itself via tts_done_tx
}

tts_gen += 1;
let gen = tts_gen;
let stop = Arc::new(AtomicBool::new(false));
active_tts = Some(TtsHandle { stop: stop.clone(), generation: gen });

let echo_c   = echo.clone();
let client_c = client.clone();
let refined_c = refined.clone();
let done_tx  = tts_done_tx.clone();
let shared_c = shared.clone();
let tx_ui_c  = tx_ui.clone();

shared.set(State::Speaking);
let _ = tx_ui.send(UiEvent::StateChanged(State::Speaking));

std::thread::spawn(move || {
    // Check if already cancelled before even starting
    if !stop.load(Ordering::SeqCst) {
        let _ = crate::tts::speak_stoppable(&client_c, &refined_c, &stop, Some(&echo_c));
    }
    // echo.reset() already called inside speak_stoppable on all exit paths
    let _ = done_tx.try_send(gen);
});
// Worker returns to VAD loop immediately — NO reset_to_idle here.
// NO stale-frame drain — AEC-cleaned frames are VAD-valid.
```

**`ControlMsg::Stop` routes to active TTS via worker** (already consumed by worker's
ctrl loop; just update to use active_tts handle):
```rust
Ok(ControlMsg::Stop) => {
    if let Some(ref handle) = active_tts {
        handle.stop.store(true, Ordering::SeqCst);
    }
}
```

### `rust/src/main.rs`

- Remove `speaking` from `start_capture` call (parameter removed from function).
- `speaking` Arc is still created and passed to `worker::run` for barge-in check.

## Error handling

- **TTS thread panic:** `done_tx.try_send(gen)` in the thread body (after the call) runs
  even if `speak_stoppable` panics via the stack unwind, **except if the panic is
  `abort`-type**. Wrap in `std::panic::catch_unwind` to be safe:
  ```rust
  let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
      let _ = crate::tts::speak_stoppable(...);
  }));
  let _ = done_tx.try_send(gen); // always runs
  ```
- **Old TTS finishes after new TTS starts:** `TtsDone(old_gen)` arrives; worker checks
  `handle.generation == gen`; mismatches are ignored. State is not corrupted.
- **`speak_stoppable` HTTP error:** returns `Err(e)`, logged; `done_tx.try_send(gen)`
  fires normally, worker cleans up.

## Testing

**Unit tests (pure logic, no I/O):**

```rust
fn test_barge_in_sets_stop_flag_with_sufficient_rms() {
    let stop = Arc::new(AtomicBool::new(false));
    let handle = TtsHandle { stop: stop.clone(), generation: 1 };
    let clean_rms = 0.05f32; // above threshold
    if clean_rms > BARGE_IN_THRESHOLD { handle.stop.store(true, Ordering::SeqCst); }
    assert!(stop.load(Ordering::SeqCst));
}

fn test_barge_in_does_not_fire_below_rms_threshold() {
    let stop = Arc::new(AtomicBool::new(false));
    let handle = TtsHandle { stop: stop.clone(), generation: 1 };
    let clean_rms = 0.005f32; // below threshold
    if clean_rms > BARGE_IN_THRESHOLD { handle.stop.store(true, Ordering::SeqCst); }
    assert!(!stop.load(Ordering::SeqCst));
}

fn test_generation_mismatch_does_not_clear_active() {
    // TtsDone from old generation should not affect active generation
    let active_gen = 2u64;
    let done_gen = 1u64; // stale
    // Simulate: if handle.generation == done_gen { clear } — should NOT fire
    assert_ne!(active_gen, done_gen);
}

fn test_stop_button_routes_to_active_tts() {
    let stop = Arc::new(AtomicBool::new(false));
    let _handle = TtsHandle { stop: stop.clone(), generation: 1 };
    // Worker ctrl loop: ControlMsg::Stop → handle.stop = true
    stop.store(true, Ordering::SeqCst);
    assert!(stop.load(Ordering::SeqCst));
}
```

**Manual acceptance tests:**
1. Speak during TTS → TTS cuts off, app transcribes new sentence, responds.
2. Don't speak → TTS completes naturally, UI shows Speaking then Listening.
3. Multiple barge-ins in a row → no overlapping audio, no stuck state.
4. Speak with `clean_rms < BARGE_IN_THRESHOLD` (background noise) → TTS continues.
5. Stop button during TTS → still works.
6. Mic toggle (mute) during TTS → still works.
7. Settings change during TTS → worker consumes it (not TTS thread).

## Out of scope

TTS queue / cross-fade, async/tokio restructure, Phase 3+ features.
