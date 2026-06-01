# Proposal: Acoustic Echo Cancellation (AEC) for Full-Duplex Voice

**Date:** 2026-06-01 (rev 2 — incorporates third-party review)
**Status:** Proposal — revised, pending approval  
**Author:** brainstorming session (Allen + Claude)  
**Repo:** `feat/rust-ui-polish-settings` branch (Rust desktop app)

---

## Problem

When the assistant plays its TTS response through the speaker, the microphone picks up
that audio. The VAD (Silero) fires on it, the STT transcribes the app's own words, and
the assistant "refines" its own output — a feedback loop. Example: app says *"The meeting
is tomorrow"* → mic hears it → app transcribes it → sends back to oMLX → responds again.

**Current workaround:** a half-duplex `speaking: Arc<AtomicBool>` flag that mutes the mic
while TTS is playing. It breaks down at the reverb *tail* — the moment TTS ends, the flag
clears but the room still rings for 200–800 ms and that reverb triggers VAD.

---

## Goal

Eliminate TTS-triggered false turns, both during playback and during the reverb tail,
without breaking the current half-duplex flow. True full-duplex barge-in (user speaks
while app speaks) is a separate, later goal that depends on a worker architecture change
not in scope here.

---

## Options Considered

### Option A — Post-TTS mute extension *(useful but insufficient)*
Keep the mic muted for an additional configurable window (e.g. 500–1500 ms) after TTS ends
to let the reverb die down before re-enabling VAD.

**Pros:** zero new deps, one-liner.  
**Cons:** fixed trade-off — too short = echo leaks; too long = user must wait. Does not
help during playback if the mute gate fails. Not a permanent solution.  
**Role in this plan:** still valuable as a safety net (Phase 0, already available via
Settings silence_ms), but not the primary fix.

### Option B — Echo fingerprinting / content filter *(belt-and-suspenders only)*
Store the last 1–3 refined texts the app spoke. If the next STT output closely matches,
discard the turn.

**Pros:** no audio processing.  
**Cons:** STT still runs (wasted compute), fuzzy matching unreliable, doesn't help
during playback, doesn't generalize.  
**Role:** optional add-on in a later phase; not the primary fix.

### Option C — Phased AEC *(selected)*
Real-time AEC via `speexdsp`. Cancels TTS echo from the mic signal using the TTS PCM as
a reference. Implemented in phases so each phase is provable before the next.

---

## Phased Implementation Plan

The reviewer's phased approach is correct and accepted in full:

| Phase | What | Speaking gate | VAD during TTS |
|-------|------|--------------|----------------|
| 0 | Post-TTS tail mute (Settings slider) | kept | no (current) |
| 1 | **AEC shadow mode** — wire reference + capture, measure | kept | no (current) |
| 2 | AEC feedback prevention — relax gate after proven stable | optional | no (current) |
| 3 | Worker restructure — concurrent TTS + VAD consumer | removed | **yes** |
| 4 | Barge-in — detected user speech triggers `stop_tts` | — | yes |

**This spec covers Phase 1 only.** Phases 2–4 have their own spec/plan cycles.

---

## Phase 1 — AEC Shadow Mode: Technical Design

### What Phase 1 does

Wire the Speex AEC engine so it receives both the TTS reference PCM and the mic signal,
and produces a cancellation output. **The existing `speaking` gate is kept active.** The
AEC output is logged/measured but not yet used in the VAD path. This lets us validate
cancellation quality, timing alignment, and callback behaviour before touching the live
pipeline.

### Architecture

```
Qwen3-TTS WAV bytes
    │
    ▼
decode PCM → resample to 16 kHz (or configure TTS to output 16 kHz)
                     │ reference frames
                     ▼
            ┌─────────────────────────────────────────┐
            │   EchoCancel  (Arc + lock-free design)   │
            └─────────────────────────────────────────┘
                     ▲ mic frames (after gate; from processing thread)
                     │ cancelled frames (logged, not yet fed to VAD)
                     ▼
            [measurement / log only — Phase 1]
            [VAD still receives raw frames via existing path]
```

### New file: `rust/src/echo.rs`

```rust
pub struct EchoCancel {
    state: SpeexEchoState,   // speexdsp internal (i16-based API)
    frame_size: usize,
    filter_length: usize,
    reference_buf: VecDeque<i16>,  // pre-queued reference frames
}

impl EchoCancel {
    pub fn new(frame_size: usize, filter_length: usize) -> Result<Self, String>;

    /// Feed TTS PCM that was just played to the speaker (far-end reference).
    /// Called from TTS thread. Lock-free: pushes to an internal queue.
    pub fn push_reference(&self, frame: &[i16]);

    /// Process one mic frame (near-end). Returns echo-cancelled output.
    /// Called from the audio-processing thread (NOT the cpal callback).
    pub fn process(&mut self, mic: &[i16]) -> Vec<i16>;

    /// Call when TTS stops (Stop button or natural end) to flush stale reference.
    pub fn reset_reference(&mut self);
}
```

**f32 ↔ i16 conversion** (speexdsp uses i16 PCM):
```rust
fn f32_to_i16(samples: &[f32]) -> Vec<i16> {
    samples.iter().map(|&x| (x.clamp(-1.0, 1.0) * 32767.0) as i16).collect()
}
fn i16_to_f32(samples: &[i16]) -> Vec<f32> {
    samples.iter().map(|&x| x as f32 / 32767.0).collect()
}
```

### Real-time safety: AEC outside the cpal callback

**The cpal audio callback must remain lock-free.** The callback only pushes raw f32 frames
to the existing `tx_audio` channel (unchanged). A new **audio-processing thread** sits
between the channel and the worker: it pulls frames, runs them through AEC, and forwards
the output to a second channel that the worker reads. In Phase 1, both the raw and
cancelled frames are forwarded so the existing VAD path is unaffected.

```
cpal callback → tx_raw → [audio-processing thread] → AEC → tx_cancelled
                                                          ↘ tx_raw (for VAD, unchanged)
```

This removes all locking from the hot callback path.

### Ownership: `EchoCancel` created in `main.rs`

`main.rs` creates `EchoCancel`, wraps it in `Arc`, and passes a clone to:
- `audio::start_processing_thread(...)` (new)
- `worker::run(...)` (which passes it into `tts::speak_stoppable(...)`)

```rust
// main.rs
let echo = Arc::new(EchoCancel::new(FRAME, FILTER_LENGTH)?);
let _processing_thread = audio::start_processing_thread(rx_raw, tx_cancelled, echo.clone());
std::thread::spawn(move || worker::run(rx_cancelled, rx_ctrl, tx_ui, shared, speaking, echo));
```

### TTS reference feeding and timing

The proposal's original timing approach was under-specified. The revised approach:

1. Qwen3-TTS is configured to output **16 kHz mono WAV** directly (eliminating the
   resampling quality risk for the reference signal). Update `tts_service/server.py` to
   use `SAMPLE_RATE = 16000`.
2. In `tts::speak_stoppable()`, after fetching WAV bytes and before handing to rodio:
   - Decode raw PCM from the WAV
   - Split into `FRAME`-sized chunks
   - Push each chunk to `echo.push_reference(chunk)` sequentially
3. The reference is pushed just before playback starts. Speex handles the residual
   speaker-to-mic delay (typically <50 ms) within its internal buffer.

**On Stop** (user presses Stop or `stop_flag` fires):
```rust
echo.reset_reference();  // flush stale reference, reset canceller state
```

### Filter length

The problem states reverb tail of 200–800 ms. Filter lengths and their coverage:

| Filter samples | Coverage at 16 kHz | Notes |
|---------------|-------------------|-------|
| 2048 | ~128 ms | Too short for problem statement |
| 4096 | ~256 ms | Covers most desktop setups |
| 8192 | ~512 ms | Conservative, higher CPU |
| 16000 | ~1000 ms | Maximum sensible |

**Default: 4096** (tunable; not exposed to users in Phase 1, configurable in code).
Empirically validated during Phase 1 acceptance tests.

### AEC API and sample format

The `speexdsp` Rust crate uses **i16 PCM**. The proposal's f32 interface was aspirational;
the implementation wraps the conversion. **Verify the actual crate API before coding**
(the Rust wrapper may differ from the C API documentation).

### Setup prerequisite (updated for Apple Silicon)

```bash
brew install speexdsp pkg-config
# Apple Silicon may also need:
export PKG_CONFIG_PATH=/opt/homebrew/lib/pkgconfig
```

Document in `docs/SETUP.md`. Confirm static vs. dynamic linking in `Cargo.toml`.

### What is NOT in Phase 1

- AEC output used in the VAD/STT path (Phase 2)
- Removal of `speaking` gate (Phase 2)
- User-mutable mic toggle behaviour changed (unchanged)
- Worker restructuring for concurrent TTS + VAD (Phase 3)
- Barge-in (Phase 4)

---

## Acceptance Criteria (Phase 1)

Phase 1 is complete when all of the following pass:

| Test | Expected |
|------|----------|
| **TTS-only no-trigger** | Play 10 assistant utterances at normal volume, no user speech → 0 completed VAD/STT turns |
| **Tail test** | After TTS stops, verify no VAD trigger for 1 s of room decay |
| **Reference alignment** | Log AEC cancellation ratio (or ERLE) during TTS; confirm positive cancellation (not zero) |
| **Callback safety** | No audio dropouts, no xrun events, callback processing time <1 ms |
| **Stop edge case** | Press Stop mid-playback; no stale reference suppresses subsequent user speech |
| **Fallback** | If AEC init fails (e.g. speexdsp not installed), app still runs with existing `speaking` gate |
| **Filter length** | Measure at 2048/4096/8192 samples; select minimum that achieves no-trigger on tail test |
| **Device matrix** | Passes on: AirPods (24 kHz captured mic), External Microphone (44.1 kHz), DJI Mic Mini (16 kHz native) |

---

## Revised Risks

| Risk | Mitigation |
|------|-----------|
| speexdsp Rust API differs from proposal | Verify crate API before starting Phase 1 implementation |
| Reference/capture timing misalignment | Shadow mode lets us measure and tune before going live |
| Mutex contention on AEC state | Architecture uses lock-free queue for reference; Mutex only in processing thread |
| cpal callback safety | AEC moved to processing thread; callback remains lock-free |
| Resampling degrades reference quality | Configure TTS to output 16 kHz directly (no reference resampling) |
| Speex does not fully cancel 200–800 ms tail | Start at 4096; measure; use 8192/16000 if needed |
| brew/pkg-config setup complexity | Documented; fallback to existing gate if library unavailable |
| macOS AGC interferes with AEC | Monitor near-end clipping; keep TTS volume moderate; test across devices |

---

## Open Questions (carried forward for Phase 2 decision)

1. **Phase 2 trigger:** what metric from Phase 1 shadow logs qualifies as "proven stable"
   enough to relax the `speaking` gate?
2. **`webrtc-audio-processing` vs `speexdsp`:** if shadow mode shows inadequate
   cancellation quality, should we switch?
3. **Phase 3 worker architecture:** TTS on separate thread, or dedicated VAD-consumer
   thread? (Separate spec when Phase 2 is complete.)
4. **Resampler quality:** if Qwen3-TTS cannot output 16 kHz, should we use `rubato`
   instead of our existing linear interpolation?
