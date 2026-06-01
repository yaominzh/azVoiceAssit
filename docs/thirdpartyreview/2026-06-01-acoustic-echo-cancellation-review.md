# Third-Party Review — Acoustic Echo Cancellation Proposal

**Reviewed doc:** `docs/proposal/2026-06-01-acoustic-echo-cancellation.md`  
**Date:** 2026-06-01  
**Grounded against:** `rust/src/main.rs`, `rust/src/audio.rs`, `rust/src/worker.rs`, `rust/src/tts.rs`, `docs/superpowers/specs/2026-05-30-rust-desktop-p0-design.md`

## Verdict

The proposal identifies the right long-term direction: **real AEC is the correct foundation for full-duplex and eventual barge-in**. However, the current design is too optimistic about SpeexDSP timing, real-time constraints, and how much of the existing half-duplex architecture can be removed.

The biggest issue: **AEC alone does not make the current app full-duplex**, because the worker is still blocked inside `speak_stoppable()` during TTS and is not running VAD on incoming frames.

I would not approve this as-is. I would approve a revised version that treats AEC as a phased capability and keeps the existing mute/speaking gate until AEC is proven in shadow mode.

## Should-fix

### 1. AEC does not enable barge-in with the current worker architecture

The proposal says AEC lets clean audio reach VAD "at all times" and enables the user to speak while the app is speaking.

But current `worker.rs` calls:

```rust
crate::tts::speak_stoppable(...)
```

synchronously during the turn pipeline. While this is running, the worker is not consuming `rx_audio`, not running VAD, and not segmenting user speech. After TTS, it drains stale audio frames.

So even if `audio.rs` sends AEC-clean frames during TTS, they will sit in the channel or be dropped. **No live barge-in can happen unless TTS playback and VAD consumption become concurrent.**

Recommendation:

- **Keep AEC scoped to feedback-loop prevention first.**
- State clearly that **true barge-in requires a worker restructuring**:
  - TTS playback on a separate thread/task, or
  - a dedicated VAD/audio-consumer thread that keeps running during TTS.
- Do not claim VAD will fire during TTS with the current architecture.

### 2. Do not remove the `speaking` gate immediately

The proposal says to remove the `speaking: Arc<AtomicBool>` gate from `audio.rs`.

That is risky. If AEC is misaligned, ineffective, or unavailable, removing the gate reintroduces the exact feedback loop the app currently avoids.

Recommendation:

- Keep `speaking` gate as a fallback.
- Add AEC behind a setting/feature flag.
- Run AEC in **shadow mode** first:
  - process mic + reference,
  - log/measure whether TTS-only playback still triggers VAD,
  - but keep the existing gate active.
- Only remove or relax the gate after manual acceptance tests pass.

A safer initial strategy:

- **Phase 0:** add a short post-TTS tail mute, e.g. 500 ms, as an immediate mitigation.
- **Phase 1:** wire AEC in shadow mode.
- **Phase 2:** enable AEC-clean audio while speaking, but still keep fallback suppression.
- **Phase 3:** restructure worker for actual barge-in.

### 3. `EchoCancel` ownership/wiring conflicts with current startup order

The proposal says:

> Create `EchoCancel` in `worker.rs`, pass it to both `start_capture()` and `speak_stoppable()`.

But current `main.rs` starts capture **before** spawning the worker:

```rust
let _stream = audio::start_capture(...);
std::thread::spawn(move || worker::run(...));
```

If `start_capture()` needs `Arc<Mutex<EchoCancel>>`, the canceller cannot be created inside `worker::run()` unless the startup order changes.

Recommendation:

- Create `EchoCancel` in `main.rs`.
- Pass a clone to `audio::start_capture(...)`.
- Pass another clone to `worker::run(...)`.
- Worker then passes it into `tts::speak_stoppable(...)`.

### 4. The real-time callback should not block on `Arc<Mutex<EchoCancel>>`

The proposal suggests:

```rust
let clean = echo.lock().unwrap().process(&frame);
```

inside the CPAL callback.

That is high-risk. Audio callbacks should avoid blocking locks, heap allocation, panics, and long work. If the TTS/reference side holds the mutex, the capture callback can stall and cause dropouts. If the mutex is poisoned, `unwrap()` can panic inside the audio callback.

The current audio callback already does allocation/resampling, but AEC locking makes it worse.

Recommendation:

- Avoid blocking `Mutex::lock()` in the callback.
- Prefer one of:
  - move AEC processing out of the callback into the worker/audio-processing thread;
  - use `try_lock()` and fall back to raw/muted/drop behavior if unavailable;
  - design `EchoCancel` so only one thread calls the Speex object and the reference path uses a lock-free queue.
- If the callback must process AEC, explicitly document that this is a v0 compromise and add instrumentation for callback overruns/dropouts.

### 5. Reference/playback timing is under-specified

The proposal says the TTS thread will feed reference PCM "in sync with rodio playback." This is the hardest part of AEC.

Current `tts.rs` gives WAV bytes to `rodio::Decoder`, appends the source to a `rodio::Player`, then polls `player.empty()`. There is no sample-accurate callback indicating which samples have actually reached the output device.

If reference is fed too early, too late, too fast, or continues after playback stops, AEC quality will degrade badly.

Recommendation:

- Specify the actual synchronization strategy:
  - wrap the `rodio::Source` to tee samples into the AEC/reference queue as rodio pulls them, or
  - use Speex's expected `playback/capture` API correctly if it buffers far-end frames internally, or
  - run the audio output yourself through CPAL for tighter timing.
- Account for device output latency.
- On `Stop`, immediately stop feeding reference and flush/reset pending reference frames.

### 6. SpeexDSP delay-estimation claim is questionable

The proposal says:

> `speexdsp`'s AEC includes internal delay estimation, so it tolerates the small acoustic delay automatically.

This should not be stated confidently without verifying the exact Rust crate/API. Speex AEC generally expects reasonably synchronized far-end and near-end streams. It can tolerate some delay with proper buffering/tail length, but it is not magic delay estimation like a full WebRTC APM pipeline.

Recommendation:

- Reword to: "Speex requires the reference and capture streams to be closely aligned; we will validate and tune buffering empirically."
- If automatic delay handling is a must, evaluate `webrtc-audio-processing` more seriously.

### 7. Filter length conflicts with the problem statement

The problem says the reverb tail can be **200–800 ms**.

The proposed filter length is:

```text
2048 samples at 16 kHz = ~128 ms
```

That does not cover the stated tail. A 128 ms tail may handle direct echo and short reflections, but likely not the 200–800 ms case.

Recommendation:

- Test at 2048, 4096, 8192, maybe 16000 samples.
- Do not claim 2048 handles typical room reverb unless measured.
- Make filter length configurable internally first, not necessarily as a user-facing setting.

## Consider

### 8. SpeexDSP sample format/API may not be `f32`

The proposal models:

```rust
pub fn push_reference(&mut self, frame: &[f32])
pub fn process(&mut self, mic: &[f32]) -> Vec<f32>
```

But many SpeexDSP bindings operate on 16-bit PCM or have a specific API shape. The proposal already notes "crate API differences," but this is central enough to move earlier.

Recommendation:

- Verify the actual crate API before approving the design.
- Specify conversion:
  - `f32 [-1.0, 1.0]` → `i16`,
  - clipping/saturation behavior,
  - output conversion back to `f32` for VAD/STT.

### 9. The linear interpolation resampler may be inadequate for AEC reference quality

The proposal says to reuse the linear interpolation resampler from `audio.rs`.

That may be acceptable for P0 capture, but AEC reference alignment and phase/frequency fidelity matter. A weak resampler can reduce cancellation quality.

Recommendation:

- Prefer making the TTS service output 16 kHz mono directly if possible.
- Otherwise consider a real resampler crate, e.g. `rubato`, if AEC quality is poor.
- At minimum, keep the resampler as a replaceable seam.

### 10. Stop behavior needs explicit handling

Current `speak_stoppable()` supports interruption via `stop_flag` and `ControlMsg::Stop`.

With AEC, stopping playback mid-utterance introduces edge cases:

- Reference frames already queued but never played.
- AEC still suppresses user speech based on stale reference.
- Speex adaptive state may retain bad assumptions.

Recommendation:

- On stop:
  - stop playback,
  - stop reference feeding,
  - flush pending reference frames,
  - consider resetting the echo canceller state.

### 11. The mic toggle must remain independent

The proposal says remove the `speaking` gate from `audio.rs`, but `shared.listening_enabled` should remain.

Recommendation:

- Explicitly say:
  - remove/relax only the TTS echo guard;
  - keep user mic mute behavior unchanged.
- If mic is muted, do not feed frames to VAD/STT. You may still choose whether AEC state should observe mic frames, but that should be deliberate.

### 12. Native dependency risk is larger than stated

`brew install speexdsp` may not be sufficient on Apple Silicon if the Rust crate needs `pkg-config` paths.

Recommendation:

- Document:
  - `brew install speexdsp pkg-config`
  - possible `PKG_CONFIG_PATH=/opt/homebrew/lib/pkgconfig`
- Confirm whether the crate statically links, dynamically links, or requires runtime library availability.

### 13. macOS AGC is not a mitigation

The risks table says:

> mic AGC is macOS-handled

This is not necessarily good. AGC, noise suppression, and nonlinear processing can make echo cancellation harder because the echo path changes over time.

Recommendation:

- Replace that mitigation with:
  - monitor clipping,
  - keep TTS output volume moderate,
  - log/measure near-end levels,
  - test across built-in speaker, external speaker, headphones.

## Missing tests / acceptance criteria

The proposal needs concrete success criteria. Suggested additions:

- **TTS-only no-trigger test:** play 10 assistant utterances at normal volume with no user speech; expect zero completed VAD/STT turns after TTS.
- **Tail test:** after TTS stops, verify no VAD trigger for 1 second of room decay.
- **Double-talk test:** user speaks while TTS is playing; verify user speech is still detectable once worker architecture supports it.
- **Stop test:** press Stop mid-playback; verify no stale reference suppresses subsequent user speech.
- **Fallback test:** if AEC init fails, app still runs with the existing `speaking` gate.
- **Device matrix:** built-in speakers, AirPods/headphones, external speaker if available.
- **Metrics:** log VAD events during TTS, dropped audio frames, callback processing time, and maybe crude echo reduction level before/after AEC.

## Suggested revised direction

I would revise the proposal around this phased plan:

1. **Immediate fix:** add configurable post-TTS tail mute, e.g. 500 ms.
2. **AEC shadow mode:** wire reference + capture into AEC but keep `speaking` gate active; collect logs.
3. **AEC feedback prevention:** allow AEC-clean frames after TTS/tail only when proven stable.
4. **Full-duplex architecture:** make TTS playback non-blocking or add a concurrent VAD consumer.
5. **Barge-in:** once VAD runs during TTS on AEC-clean audio, wire detected speech to `stop_tts`.

## Overall recommendation

Do not approve the current proposal as an implementation plan yet. Approve the **goal**, but require revisions for:

- **Concurrency:** AEC does not fix blocked VAD during TTS.
- **Safety:** keep the `speaking` gate/fallback initially.
- **Timing:** specify exact reference/playback synchronization.
- **Realtime behavior:** avoid blocking mutex use in the CPAL callback.
- **Validation:** add measurable acceptance tests before removing half-duplex protection.
