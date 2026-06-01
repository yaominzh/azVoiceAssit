# Proposal: Acoustic Echo Cancellation (AEC) for Full-Duplex Voice

**Date:** 2026-06-01  
**Status:** Proposal — pending review and approval  
**Author:** brainstorming session (Allen + Claude)  
**Repo:** `feat/rust-ui-polish-settings` branch (Rust desktop app)

---

## Problem

When the assistant plays its TTS response through the speaker, the microphone picks up
that audio. The VAD (Silero) fires on it, the STT transcribes the app's own words, and
the assistant tries to "refine" its own output — creating a feedback loop. Example: app
says *"The meeting is tomorrow"*, mic hears it, app transcribes *"The meeting is
tomorrow"*, sends it back to oMLX, and responds again.

**Current workaround:** a half-duplex `speaking` flag that mutes the mic while TTS is
playing. This breaks down at the reverb *tail* — the moment TTS ends, the flag clears
but the room still rings for 200–800 ms, and that reverb triggers VAD.

---

## Proposed Solution: Full-Duplex Acoustic Echo Cancellation

Replace the half-duplex mute gate with a real-time AEC engine that sits between the
microphone and the VAD. The AEC receives two inputs:

1. **Reference signal** — the TTS PCM that was sent to the speaker (what we *want* to
   cancel)
2. **Captured signal** — what the microphone actually recorded (user speech + echo of
   TTS)

The AEC outputs a **clean signal** with the TTS echo subtracted. This clean signal feeds
the VAD and STT pipeline, so neither the TTS audio nor its room reverb ever triggers a
turn.

### Architecture Overview

```
Qwen3-TTS HTTP response
    │
    ▼
WAV bytes → decode PCM → resample to 16 kHz
                              │
                              ▼ reference frames (pushed by TTS thread)
                     ┌────────────────────────────┐
                     │   EchoCancel               │
                     │   (Arc<Mutex<SpeexEcho>>)  │
                     └────────────────────────────┘
                              ▲ mic frames (16 kHz, from cpal callback)
                              │
                              ▼ cancelled clean frames
                         VAD → STT → Refine → TTS
```

The `EchoCancel` object is shared between two threads:
- **TTS thread** — feeds reference PCM frames as playback progresses
- **Audio callback** — passes every mic frame through the canceller before it reaches
  the worker pipeline

---

## Options Considered

### Option A — Post-TTS mute extension *(rejected as primary)*
After TTS ends, keep the mic muted for a configurable window (e.g. 500–1500 ms) to let
the reverb die down before re-enabling VAD.

**Pros:** zero new dependencies, one-liner change.  
**Cons:** fixed trade-off — too short = echo leaks through; too long = user has to wait
before speaking. Does not enable full-duplex or barge-in.

### Option B — Echo fingerprinting / content filter *(rejected as primary)*
Store the last 1–3 refined texts the app just spoke. If the next STT output closely
matches a stored text (string similarity), discard the turn.

**Pros:** no audio processing, runs after STT.  
**Cons:** STT still runs (wasted compute), fuzzy matching isn't reliable, doesn't help
with barge-in.

### Option C — Full AEC *(selected)*
Real-time AEC via `speexdsp` (Speex DSP library). Cancels the TTS echo from the mic
signal in real time, enabling full-duplex operation.

**Pros:** solves the problem correctly and permanently; foundation for barge-in.  
**Cons:** new native dependency (`speexdsp`), more implementation complexity, timing
synchronization between TTS playback and reference feeding.

---

## Technical Design

### New file: `rust/src/echo.rs`

```rust
pub struct EchoCancel {
    state: speexdsp::echo::SpeexEcho,
    frame_size: usize,
}

impl EchoCancel {
    pub fn new(frame_size: usize, filter_length: usize) -> Self { ... }

    /// Called by TTS thread: feed the PCM that was just played to the speaker.
    pub fn push_reference(&mut self, frame: &[f32]) { ... }

    /// Called by audio callback: cancel echo from mic frame, return clean signal.
    pub fn process(&mut self, mic: &[f32]) -> Vec<f32> { ... }
}
```

The object is wrapped in `Arc<Mutex<EchoCancel>>` and shared between the TTS thread
and the audio callback thread.

### Changes to `tts.rs`

After receiving the WAV bytes from Qwen3-TTS:
1. Decode the WAV PCM (24 kHz mono from the TTS service)
2. Resample from 24 kHz → 16 kHz (same linear interpolation used in `audio.rs`)
3. Feed the resampled frames to `echo.push_reference()` in sync with rodio playback

The `speak_stoppable` function gains an `Arc<Mutex<EchoCancel>>` parameter.

### Changes to `audio.rs`

- Remove the `speaking: Arc<AtomicBool>` gate
- Accept `Arc<Mutex<EchoCancel>>` instead
- In the callback: `let clean = echo.lock().unwrap().process(&frame);` — feed clean
  frame to the worker instead of raw mic frame

### Changes to `worker.rs`

- Create `EchoCancel` at startup, wrap in `Arc<Mutex<_>>`
- Pass it to both `start_capture()` and `speak_stoppable()`
- Remove `speaking` AtomicBool and the mute gate logic

### New dependency

```toml
speexdsp = "0.2"   # or latest — wraps libspeexdsp
```

**Setup prerequisite:** `brew install speexdsp` (macOS).

### AEC parameters

| Parameter | Value | Notes |
|-----------|-------|-------|
| `frame_size` | 512 samples | Matches our VAD frame at 16 kHz (~32 ms) |
| `filter_length` | 2048 samples | ~128 ms tail coverage — handles typical room reverb |
| Input rate | 16 kHz | Both mic and reference resampled to 16 kHz |

`speexdsp`'s AEC includes internal delay estimation, so it tolerates the small
acoustic delay (speaker → mic, typically <50 ms) automatically.

---

## What This Enables

1. **No more TTS feedback loop** — the AEC cancels both playback audio and its reverb
   tail. The `speaking` flag workaround is no longer needed.
2. **Foundation for barge-in** — with echo-clean audio reaching the VAD at all times,
   the user can speak while the app is speaking and VAD will correctly fire on their
   voice. *Note: interrupting TTS playback on barge-in is a separate concern (already
   have `TtsPlayer.stop()`); this proposal solves the signal side.*
3. **Better latency feel** — no more mandatory silence window after TTS; user can
   respond immediately.

---

## Risks and Mitigations

| Risk | Mitigation |
|------|-----------|
| `speexdsp` crate API differences | Verify at build time; document actual API used |
| Timing misalignment (reference vs capture) | AEC internal delay estimation handles <100ms misalignment; test with varying speaker-mic distances |
| `Arc<Mutex<>>` contention | Lock held only for ~1ms per 32ms frame; contention negligible |
| brew dependency on macOS | Document in SETUP.md; one-time install |
| AEC degrades at very high volume or clipping | Qwen3-TTS output is normalized; mic AGC is macOS-handled |

---

## Open Questions for Review

1. Is `speexdsp` the right library, or should we use `webrtc-audio-processing` (heavier
   but self-contained, no brew dependency)?
2. Should barge-in (interrupt TTS on user speech detection) be in scope for this phase,
   or follow-on?
3. The filter length (2048 = 128 ms) — is this sufficient for the room/speaker setup
   being used? May need tuning.
4. Should the AEC filter length be exposed as a Settings knob?

---

## Estimated Effort

- `echo.rs` + `speexdsp` wiring: 1 day
- `tts.rs` reference feeding + timing: 1 day  
- `audio.rs` / `worker.rs` / `main.rs` wiring + removing the old mute gate: 0.5 day
- Testing and tuning AEC filter parameters: 0.5–1 day

**Total: 3–4 days**
