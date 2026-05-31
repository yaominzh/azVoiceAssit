# Bug Post-Mortem: Rust VAD / Audio Pipeline — "the app hears nothing"

**Date:** 2026-05-31
**Component:** Rust desktop app (`rust/`) — `audio.rs`, `vad.rs`, `worker.rs`, `main.rs`
**Symptom:** Speaking into the mic produced **no turns** — the ☯ stayed on "listening",
no `heard`/`refined` text, no speech back. The Python P0 worked fine on the same machine.

This wasn't one bug. It was **six**, stacked, each masking the next. The final and
hardest was a Silero ONNX usage error that produced near-zero speech probability on
clear speech. Below is the chain, how each was isolated, and the lessons.

---

## TL;DR of the chain

| # | Layer | Symptom | Root cause | Fix |
|---|-------|---------|-----------|-----|
| 1 | cpal config | app exits at startup | `BufferSize::Fixed(512)` unsupported on macOS CoreAudio | `BufferSize::Default` |
| 2 | sample rate | stream config rejected | mic supports 44.1–96 kHz, not 16 kHz | capture native + **resample to 16 kHz** |
| 3 | worker loop | mic toggle won't turn back on | `rx_audio.recv()` blocks forever when muted → ctrl never drained | `crossbeam select!` on audio **and** ctrl |
| 4 | macOS TCC | captured peak ≈ 0.005 (silence) | agent-launched binary lacked mic permission; OS feeds silence, no prompt | run from a user terminal (grant the prompt) |
| 5 | device / capture | still silent from one path | hand-built `StreamConfig` delivered silence; AirPods vs External Mic confusion | use `device.default_input_config()` |
| 6 | **Silero ONNX** | **capture good (peak 0.34) but VAD prob ≈ 0.001** | **v5 model needs 64-sample context prepended (576-sample input); we fed bare 512** | prepend `CTX=64` samples, carry across calls |

---

## The debugging method (what actually worked)

The breakthrough came from **isolating each stage with its own measurement**, instead of
guessing. Concretely:

1. **Measure at the boundary.** Added a throwaway `examples/mic_test.rs` that records 8 s
   from the *same* cpal device, prints **peak/RMS**, writes a WAV, and `afplay`s it back.
   This split "is the mic capturing?" from "is the pipeline processing?" cleanly.
2. **Compare against a known-good reference.** The Python P0 worked, so we ran the *same
   captured audio* through **Python `silero-vad` (torch)** → prob **0.9999**. That proved the
   audio was fine and the bug was in our Rust VAD usage.
3. **Bisect the runtime.** Ran the *same* `silero_vad.onnx` via **Python `onnxruntime`** with
   our exact input construction → prob **0.0016** (same as Rust!). This was the key pivot:
   the bug was **not** Rust-specific — it was how the ONNX model was being fed. (Torch model
   0.9999, ONNX model 0.0016 on identical samples.)
4. **Read the model's real contract.** Python ORT exposes input shapes:
   `input [None,None]`, `state [2,None,128]`, `sr scalar []`. Confirmed names/shapes matched.
5. **Test the hypothesis in the fast loop (Python).** Prepended the previous 64 samples to
   each 512-frame (→ 576) → prob jumped to **0.9999**. Fix confirmed *before* touching Rust.
6. **Port the one-line insight to Rust.**

Lesson: when a port misbehaves, **diff it against the reference implementation at the
narrowest possible boundary** (same bytes in, compare outputs). We found the root cause in
~3 targeted measurements after a lot of upstream guessing.

---

## The bugs in detail

### 1–2. cpal stream config (capture wouldn't even start, then wrong rate)
- `cpal::BufferSize::Fixed(512)` → `The requested stream configuration is not supported by
  the device.` macOS CoreAudio wants `BufferSize::Default`.
- The mic's supported range was **44100–96000 Hz** — 16 kHz isn't offered. Capturing at the
  native rate and resampling in-app (linear interpolation, 44100→16000 = 2.756×) was required.
  *Gotcha:* a naive point-sampler aliases on non-integer ratios; use linear interpolation.

### 3. Mute-toggle deadlock
The worker did `rx_audio.recv()` (blocking). When muted, the capture callback drops all
frames → the audio channel goes empty → `recv()` blocks **forever** → the worker never
returns to drain `rx_ctrl`, so the *second* mic-toggle (unmute) was never processed.
**Fix:** `crossbeam_channel::select!` on both `rx_audio` and `rx_ctrl`, so control messages
are handled even when no audio flows.

### 4. macOS microphone permission (TCC) — the great misdirection
Captured peak was ~0.005 (silence) even while speaking. We initially dismissed permission
("Python worked"), but the crucial detail: macOS grants mic access **per launching app**, and
a CLI binary inherits its launcher's grant. When the binary was launched by the agent/editor
(whose host lacked mic access), the OS delivered **silent audio with no prompt**. Running the
*same binary* from the user's own iTerm2 → `mic_test` captured real speech (peak 0.09).
**Lesson:** "silent capture, no error, no prompt" on macOS is the signature of a missing TCC
grant on the *launching process* — verify with **Sound → Input** level meter (OS-level,
app-independent).

### 5. Capture path: hand-built config vs default config
A hand-built `StreamConfig { channels:1, sample_rate, buffer_size:Default }` silently
delivered near-zero audio, while `mic_test` (using `device.default_input_config().config()`)
captured fine. Switching `start_capture` to the device's own default config (then downmix to
mono + resample) fixed capture. Also: a **Mac mini has no built-in mic** — the working input
was "External Microphone"; AirPods enumerated as a 24 kHz input but weren't reliable here.

### 6. Silero v5 ONNX needs a context window (the real root cause)
Even with perfect capture (peak 0.34, clearly audible speech), `vad.accept()` returned
prob ≈ 0.001 — so no `Start` event, no utterance, no turn.

Silero VAD v5 is **stateful across frames**: each 512-sample window must be **prefixed with
the previous 64 samples**, giving a **576-sample model input**. The PyTorch model
(`load_silero_vad()`) manages this context internally; the **ONNX export does not** — the
caller must do it. Feeding bare 512-sample windows yields garbage-low probabilities.

```
bare 512-sample input        → prob 0.0016   (both Rust ORT and Python ORT)
64-sample context + 512 (576) → prob 0.9999   (matches the torch model)
```

**Fix** (`vad.rs`): keep a `context: Vec<f32>` (64 zeros initially); each call build
`[context ++ frame]` (576 samples) as the model input; after inference set
`context = frame[len-64..]`; `reset()` clears it.

---

## Why the unit test didn't catch #6

The VAD unit test asserted *"10 frames of silence produce no `Start`."* A **permanently-zero**
VAD (prob always ≈ 0) **passes that test** — silence and speech both stay below the 0.5
threshold. So the test validated the cheap direction (no false positives) but never the
expensive one (**detects real speech**).

**Takeaway for future ML-glue code:** a detector needs a **positive** test, not just a
negative one. Bundle a tiny real-speech fixture and assert `max_prob > 0.5`. Without a
positive assertion, a broken-but-quiet model looks "passing." (This fixture is left as a
follow-up — it needs a small committed audio sample.)

---

## General lessons

1. **Isolate at boundaries; measure, don't guess.** Peak/RMS at capture, prob at VAD, text at
   STT. Each `eprintln!` probe pinned a layer. Most of the wall-clock cost was *before* we
   started measuring.
2. **Diff against the working reference at the narrowest boundary.** Python torch vs Python
   ONNX vs Rust ONNX on identical samples localized the bug to "ONNX feeding," not "Rust."
3. **macOS audio has two silent failure modes that look identical to bugs:** TCC permission
   (launcher-scoped) and wrong/again-silent input device. The **Sound → Input meter** is the
   fastest app-independent check.
4. **Fast-moving crates need runtime-verified contracts.** `cpal`, `ort`, `whisper-rs`,
   `rodio`, `egui` all had APIs that differed from the plan; the plan flagged these as
   "confirm at build time," which paid off.
5. **Negative-only tests give false confidence for detectors/classifiers.** Always add a
   positive case.

---

## Files touched by the fixes

- `rust/src/audio.rs` — default-config capture, mono downmix, linear resample to 16 kHz.
- `rust/src/vad.rs` — 64-sample Silero context window (the root-cause fix).
- `rust/src/worker.rs` — `select!` on audio+ctrl (mute-toggle deadlock).
- `rust/src/main.rs` — (unrelated) reachability checks, dark theme.
- `rust/examples/mic_test.rs`, `list_devices.rs`, `vad_diag.rs` — diagnostics kept for reuse.
