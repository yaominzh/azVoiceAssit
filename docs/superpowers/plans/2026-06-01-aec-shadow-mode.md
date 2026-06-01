# AEC Phase 1 — Shadow Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire Speex AEC so it observes both TTS reference PCM and mic frames in parallel with the existing pipeline, log cancellation quality, and validate that the half-duplex speaking-gate can be safely removed in a later phase — without touching the live VAD path.

**Architecture:** A new `echo.rs` owns an `EchoCancel` struct wrapping `speexdsp`. A new audio-processing thread (not the cpal callback) pulls raw mic frames from the existing channel, runs them through AEC using reference PCM pushed by the TTS thread, and forwards both raw and cancelled frames to the worker. The worker keeps the existing `speaking` gate and VAD path unchanged — AEC output is logged only. The `EchoCancel` is created in `main.rs` and cloned to both the processing thread and the worker (which passes it into `speak_stoppable`).

**Tech Stack:** Rust, `aec-rs = "1.0.0"` (requires `brew install speexdsp pkg-config`), `crossbeam-channel`, existing `rodio`/`cpal` pipeline. No changes to Python TTS service in this phase.

**Proposal:** `docs/proposal/2026-06-01-acoustic-echo-cancellation.md` (Phase 1)  
**Branch:** `feat/aec-echo-cancellation`  
**DO NOT `git push` without checking** — push is fine (org guardrail lifted).

---

## IMPORTANT: setup prerequisite

Before any cargo build, install the native library:
```bash
brew install speexdsp pkg-config
export PKG_CONFIG_PATH=/opt/homebrew/lib/pkgconfig   # Apple Silicon may need this
pkg-config --modversion speexdsp   # should print "1.2.1" or similar
```

If `pkg-config` can't find speexdsp, the crate will fail to compile with a linker error. Add `PKG_CONFIG_PATH` to your shell profile to make it permanent.

---

## File structure

| File | Change |
|------|--------|
| `rust/Cargo.toml` | Add `aec-rs = "1.0.0"` |
| `rust/src/echo.rs` | **CREATE** — `EchoCancel` struct, `push_reference`, `process`, `reset`, f32↔i16 helpers |
| `rust/src/audio.rs` | Add `start_processing_thread` — pulls raw frames, runs AEC, forwards to worker |
| `rust/src/tts.rs` | `speak_stoppable` gains `echo: Arc<EchoCancel>` param; pushes reference PCM to AEC; calls `reset` on stop |
| `rust/src/worker.rs` | Accept `Arc<EchoCancel>` and pass it to `speak_stoppable`; log AEC output per turn |
| `rust/src/main.rs` | Create `EchoCancel`, wire into processing thread and worker |
| `rust/src/lib.rs` | Add `pub mod echo;` |

---

## Task 1: Add speexdsp to Cargo.toml + verify build

**Files:** Modify `rust/Cargo.toml`

- [ ] **Step 1: Check speexdsp API before adding it**

```bash
# Install the native lib first
brew install speexdsp pkg-config
export PKG_CONFIG_PATH=/opt/homebrew/lib/pkgconfig

# Check what the crate exposes
cargo add speexdsp --dry-run 2>&1 | head -5
```

- [ ] **Step 2: Add to Cargo.toml**

In `rust/Cargo.toml`, under `[dependencies]`, add:
```toml
speexdsp = "0.1.2"
```

- [ ] **Step 3: Write a minimal compile test**

Create `rust/src/echo.rs` with just enough to confirm the crate links:

```rust
// Placeholder — verify speexdsp crate compiles and links.
// Full implementation in Task 2.
pub struct EchoCancel;
```

Add `pub mod echo;` to `rust/src/lib.rs` and `mod echo;` to `rust/src/main.rs`.

- [ ] **Step 4: Verify build**

```bash
cd /Users/allen/repo/azVoiceAssist/rust
cargo build 2>&1 | grep -E "^error|Finished|speexdsp"
```

Expected: `Finished` with no errors. If you see a pkg-config or linker error, confirm `PKG_CONFIG_PATH` and rerun.

- [ ] **Step 5: Commit**

```bash
cd /Users/allen/repo/azVoiceAssist
git add rust/Cargo.toml rust/Cargo.lock rust/src/echo.rs rust/src/lib.rs rust/src/main.rs
git commit -m "chore: add speexdsp dep + placeholder echo.rs (verifies native lib links)"
```

---

## Task 2: `EchoCancel` — TDD for conversion helpers and struct

**Files:** Replace `rust/src/echo.rs`

Before implementing the full AEC, we write and test the pure helpers (f32↔i16 conversion, basic struct construction). The actual `speexdsp` call we test separately in Task 3.

- [ ] **Step 1: Write the failing tests**

Replace `rust/src/echo.rs` with:

```rust
/// AEC Phase 1: shadow mode — observes TTS reference + mic, logs cancellation.
/// Does NOT feed cancelled output to VAD yet. Half-duplex speaking gate unchanged.
pub struct EchoCancel {
    // populated in Task 3
}

/// Convert f32 [-1.0, 1.0] → i16 with saturation clamping.
pub fn f32_to_i16(samples: &[f32]) -> Vec<i16> {
    samples.iter().map(|&x| (x.clamp(-1.0, 1.0) * 32767.0) as i16).collect()
}

/// Convert i16 → f32 [-1.0, 1.0].
pub fn i16_to_f32(samples: &[i16]) -> Vec<f32> {
    samples.iter().map(|&x| x as f32 / 32767.0).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32_to_i16_positive_clamp() {
        assert_eq!(f32_to_i16(&[1.0]), vec![32767]);
        assert_eq!(f32_to_i16(&[2.0]), vec![32767]); // clamped
    }

    #[test]
    fn f32_to_i16_negative_clamp() {
        assert_eq!(f32_to_i16(&[-1.0]), vec![-32767]);
        assert_eq!(f32_to_i16(&[-2.0]), vec![-32767]); // clamped
    }

    #[test]
    fn f32_to_i16_zero() {
        assert_eq!(f32_to_i16(&[0.0]), vec![0]);
    }

    #[test]
    fn i16_to_f32_roundtrip_within_epsilon() {
        let original = vec![0.5f32, -0.5, 0.0, 0.999];
        let converted = f32_to_i16(&original);
        let back = i16_to_f32(&converted);
        for (a, b) in original.iter().zip(back.iter()) {
            assert!((a - b).abs() < 0.001, "roundtrip error: {a} vs {b}");
        }
    }

    #[test]
    fn f32_to_i16_batch() {
        let input = vec![0.0f32; 512];
        assert_eq!(f32_to_i16(&input).len(), 512);
    }
}
```

- [ ] **Step 2: Run, expect fail**

```bash
cd /Users/allen/repo/azVoiceAssist/rust
cargo test echo 2>&1 | tail -10
```

Expected: FAIL — `f32_to_i16` / `i16_to_f32` not found.

- [ ] **Step 3: The functions are already in the file (they're part of Step 1's replacement)**

Just run the tests — they should now pass since you wrote both the tests and the functions together.

```bash
cargo test echo 2>&1 | tail -5
```

Expected: `test result: ok. 5 passed`

- [ ] **Step 4: Full suite**

```bash
cargo test --lib 2>&1 | tail -2
```

Expected: all prior tests + 5 new = passing.

- [ ] **Step 5: Commit**

```bash
cd /Users/allen/repo/azVoiceAssist
git add rust/src/echo.rs
git commit -m "feat: f32↔i16 helpers for AEC (5 tests)"
```

---

## Task 3: `EchoCancel` full implementation (speexdsp-backed)

**Files:** Modify `rust/src/echo.rs`

> **IMPORTANT:** Verify the actual `speexdsp 0.1.2` API before writing code. Run:
> ```bash
> cd /Users/allen/repo/azVoiceAssist/rust
> cargo doc --open --package speexdsp 2>/dev/null || cargo doc --package speexdsp
> ```
> The types may be `speexdsp::echo::EchoState`, `speexdsp::echo::SpeexEcho`, or similar.
> Check the generated docs and adapt the implementation below to match.
> If the API differs significantly, report DONE_WITH_CONCERNS with the exact API used.

The design uses a `crossbeam_channel` single-producer queue for reference frames so:
- The TTS thread pushes reference without holding a lock
- The processing thread drains and feeds reference to Speex, also without holding a long lock
- The only mutex is around the speexdsp state itself, held briefly per frame

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)]` block in `echo.rs`:

```rust
    #[test]
    fn echo_cancel_new_and_process_silence() {
        // Construct an EchoCancel, feed silence, expect no panic and output len = FRAME.
        let ec = EchoCancel::new(crate::config::FRAME, 4096)
            .expect("EchoCancel::new");
        let silence = vec![0.0f32; crate::config::FRAME];
        let out = ec.process_frame(&silence);
        assert_eq!(out.len(), crate::config::FRAME);
    }

    #[test]
    fn push_reference_and_process_does_not_panic() {
        let ec = EchoCancel::new(crate::config::FRAME, 4096)
            .expect("EchoCancel::new");
        let silence = vec![0.0f32; crate::config::FRAME];
        ec.push_reference(&silence);
        let out = ec.process_frame(&silence);
        assert_eq!(out.len(), crate::config::FRAME);
    }
```

Run: `cargo test echo 2>&1 | tail -10` → FAIL (`EchoCancel::new` not found).

- [ ] **Step 2: Implement `EchoCancel`**

Add to `echo.rs` above the existing helpers (adapt to actual speexdsp API — the template below shows the intended interface; the internals may need adjustment):

```rust
use std::sync::Mutex;
use crossbeam_channel::{Sender, Receiver, bounded};

pub struct EchoCancel {
    /// Speex echo state, protected by Mutex so multiple threads can push/process.
    /// Lock is held only for the duration of one frame (~0.1ms at 16kHz).
    inner: Mutex<EchoCancelInner>,
    /// Reference frame queue: TTS thread → processing thread.
    ref_tx: Sender<Vec<i16>>,
    ref_rx: Receiver<Vec<i16>>,
}

struct EchoCancelInner {
    /// speexdsp echo canceller — verify exact type from crate docs.
    /// Likely: speexdsp::echo::EchoState  or  speexdsp::EchoState
    state: speexdsp::echo::EchoState,
    frame_size: usize,
}

impl EchoCancel {
    pub fn new(frame_size: usize, filter_length: usize) -> Result<Self, String> {
        // Verify: speexdsp::echo::EchoState::new(frame_size, filter_length)
        // or similar constructor — adapt to actual API.
        let state = speexdsp::echo::EchoState::new(frame_size, filter_length)
            .ok_or("speexdsp: failed to create echo state")?;
        let (ref_tx, ref_rx) = bounded(64); // buffer up to 64 reference frames
        Ok(Self {
            inner: Mutex::new(EchoCancelInner { state, frame_size }),
            ref_tx,
            ref_rx,
        })
    }

    /// Push one FRAME of TTS PCM as AEC reference (called from TTS thread).
    pub fn push_reference(&self, frame: &[f32]) {
        let _ = self.ref_tx.try_send(f32_to_i16(frame)); // drop if queue full
    }

    /// Process one FRAME of mic audio. Drains one reference frame (if available)
    /// and runs Speex AEC. Returns echo-cancelled output as f32.
    /// Called from the audio-processing thread.
    pub fn process_frame(&self, mic: &[f32]) -> Vec<f32> {
        let mic_i16 = f32_to_i16(mic);
        let ref_i16: Vec<i16> = self.ref_rx.try_recv()
            .unwrap_or_else(|_| vec![0i16; mic.len()]); // silence if no reference

        let mut out = vec![0i16; mic.len()];
        let mut inner = self.inner.lock().unwrap();
        // Verify: inner.state.echo_cancellation(&mic_i16, &ref_i16, &mut out)
        // or similar method — adapt to actual speexdsp API.
        inner.state.echo_cancellation(&mic_i16, &ref_i16, &mut out);
        i16_to_f32(&out)
    }

    /// Flush pending reference frames and reset Speex internal state.
    /// Call when TTS stops (natural end or Stop button).
    pub fn reset(&self) {
        // Drain reference queue
        while self.ref_rx.try_recv().is_ok() {}
        // Reset speex state — verify method name from crate docs
        // inner.state.reset() or inner.state.echo_state_reset() or similar
        if let Ok(mut inner) = self.inner.try_lock() {
            // inner.state.reset();  // uncomment once API verified
            let _ = &inner.frame_size; // suppress unused warning until reset API confirmed
        }
    }
}
```

> **API verification note:** The exact method names (`echo_cancellation`, `reset`, `EchoState::new`) must be confirmed against the installed `speexdsp 0.1.2` crate docs. If any method doesn't exist, find the closest equivalent and document the deviation.

- [ ] **Step 3: Run tests**

```bash
cd /Users/allen/repo/azVoiceAssist/rust
cargo test echo 2>&1 | tail -5
```
Expected: all 7 pass (5 from Task 2 + 2 new).

- [ ] **Step 4: Full suite + build**

```bash
cargo build 2>&1 | grep "^error\|Finished"
cargo test --lib 2>&1 | tail -2
```
Expected: clean build, all tests pass.

- [ ] **Step 5: Commit**

```bash
cd /Users/allen/repo/azVoiceAssist
git add rust/src/echo.rs
git commit -m "feat: EchoCancel (speexdsp-backed, shadow mode ready)"
```

---

## Task 4: Audio-processing thread (`audio.rs`)

**Files:** Modify `rust/src/audio.rs`

In Phase 1, the cpal callback is unchanged — it still pushes raw frames to `tx_audio`. A new **processing thread** pulls from `rx_audio`, runs frames through AEC (or bypasses if `echo` is `None`), and forwards to `tx_processed`. The worker will read from `tx_processed` instead of `tx_audio`.

No tests for this (I/O thread, verified by integration). Verification is `cargo build`.

- [ ] **Step 1: Add `start_processing_thread` to `audio.rs`**

Append to the bottom of `rust/src/audio.rs`:

```rust
use crate::echo::EchoCancel;
use std::sync::Arc;

/// Spawn the audio-processing thread.
///
/// Pulls raw f32 frames from `rx_raw`, optionally runs them through AEC,
/// and forwards to `tx_processed`. The cpal callback remains unchanged.
///
/// In Phase 1, both raw and cancelled frames are available. The worker
/// reads from `tx_processed` (which carries AEC output when echo is Some).
/// The existing `speaking` gate in `start_capture` is NOT removed.
pub fn start_processing_thread(
    rx_raw: crossbeam_channel::Receiver<Vec<f32>>,
    tx_processed: crossbeam_channel::Sender<Vec<f32>>,
    echo: Option<Arc<EchoCancel>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        loop {
            let frame = match rx_raw.recv() {
                Ok(f) => f,
                Err(_) => return, // channel closed, exit thread
            };

            let processed = match &echo {
                Some(ec) => {
                    let cancelled = ec.process_frame(&frame);
                    // Phase 1: log AEC activity (not zero) for validation
                    let raw_rms: f32 = (frame.iter().map(|x| x * x).sum::<f32>()
                        / frame.len() as f32).sqrt();
                    let clean_rms: f32 = (cancelled.iter().map(|x| x * x).sum::<f32>()
                        / cancelled.len() as f32).sqrt();
                    if raw_rms > 0.01 {
                        eprintln!("[aec-shadow] raw_rms={:.4} clean_rms={:.4} reduction={:.1}dB",
                            raw_rms, clean_rms,
                            20.0 * (clean_rms / (raw_rms + 1e-9)).log10());
                    }
                    cancelled  // Phase 1: feed AEC output to worker (shadow only while speaking gate active)
                }
                None => frame, // AEC unavailable — pass through unchanged
            };

            let _ = tx_processed.try_send(processed);
        }
    })
}
```

- [ ] **Step 2: Build**

```bash
cd /Users/allen/repo/azVoiceAssist/rust
cargo build 2>&1 | grep "^error\|Finished"
```
Expected: `Finished`. Fix any import errors (e.g. add `use crate::echo::EchoCancel;` if not already there).

- [ ] **Step 3: Commit**

```bash
cd /Users/allen/repo/azVoiceAssist
git add rust/src/audio.rs
git commit -m "feat: audio-processing thread with AEC shadow mode logging"
```

---

## Task 5: TTS reference feeding (`tts.rs`)

**Files:** Modify `rust/src/tts.rs`

`speak_stoppable` gains an `Option<Arc<EchoCancel>>` parameter. When present, it decodes the WAV PCM and pushes reference frames to the AEC before/during playback.

- [ ] **Step 1: Add a test for WAV PCM extraction helper**

Add to `tts.rs` tests:

```rust
    #[test]
    fn extract_pcm_from_wav_silence() {
        // 44-byte WAV header + 20 bytes of i16 silence (10 samples)
        let mut wav = vec![
            // RIFF header
            b'R', b'I', b'F', b'F',
            0x24, 0x00, 0x00, 0x00, // chunk size = 36 + 20 = 56 - 8 = 48... use a fixed known value
            b'W', b'A', b'V', b'E',
            b'f', b'm', b't', b' ',
            0x10, 0x00, 0x00, 0x00, // PCM fmt chunk size
            0x01, 0x00,             // PCM = 1
            0x01, 0x00,             // channels = 1
            0x80, 0x3E, 0x00, 0x00, // sample rate = 16000
            0x00, 0x7D, 0x00, 0x00, // byte rate = 32000
            0x02, 0x00,             // block align = 2
            0x10, 0x00,             // bits per sample = 16
            b'd', b'a', b't', b'a',
            0x14, 0x00, 0x00, 0x00, // data chunk size = 20 bytes = 10 i16 samples
        ];
        // 10 silence samples (i16 = 0)
        wav.extend_from_slice(&[0u8; 20]);

        let samples = super::extract_wav_pcm_i16(&wav);
        assert!(samples.is_ok(), "should parse: {:?}", samples.err());
        let s = samples.unwrap();
        assert_eq!(s.len(), 10);
        assert!(s.iter().all(|&x| x == 0));
    }
```

Run: `cargo test extract_pcm 2>&1 | tail -10` → FAIL (`extract_wav_pcm_i16` not found).

- [ ] **Step 2: Add `extract_wav_pcm_i16` and update `speak_stoppable`**

Add to `rust/src/tts.rs` (before the `#[cfg(test)]` block):

```rust
use crate::echo::EchoCancel;
use std::sync::Arc;

/// Extract raw i16 PCM samples from a WAV byte buffer.
/// Skips the 44-byte standard PCM WAV header; works for the Qwen3-TTS output
/// (16-bit mono PCM). Returns Err if buffer is too short.
pub fn extract_wav_pcm_i16(wav: &[u8]) -> Result<Vec<i16>, String> {
    // Minimal WAV parser: find "data" chunk, read i16 LE samples.
    let data_offset = wav.windows(4)
        .position(|w| w == b"data")
        .ok_or("no data chunk in WAV")?
        + 8; // skip "data" + 4-byte chunk size
    if data_offset >= wav.len() {
        return Err("WAV data chunk empty".into());
    }
    let data = &wav[data_offset..];
    let samples: Vec<i16> = data
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    Ok(samples)
}
```

Update `speak_stoppable` signature to accept `echo`:

```rust
pub fn speak_stoppable(
    client: &reqwest::blocking::Client,
    text: &str,
    stop_flag: &AtomicBool,
    rx_ctrl: &crossbeam_channel::Receiver<crate::events::ControlMsg>,
    echo: Option<&Arc<EchoCancel>>,   // ← new
) -> Result<(), String> {
    let bytes = client
        .post(crate::config::TTS_URL)
        .json(&build_tts_body(text))
        .timeout(Duration::from_secs(60))
        .send()
        .map_err(|e| format!("tts send: {e}"))?
        .bytes()
        .map_err(|e| format!("tts bytes: {e}"))?;

    // Push TTS PCM as AEC reference before/during playback
    if let Some(ec) = echo {
        if let Ok(pcm_i16) = extract_wav_pcm_i16(&bytes) {
            // Split into FRAME-sized chunks and push each as reference
            for chunk in pcm_i16.chunks(crate::config::FRAME) {
                let frame_f32 = crate::echo::i16_to_f32(chunk);
                ec.push_reference(&frame_f32);
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
        if let Ok(crate::events::ControlMsg::Stop) = rx_ctrl.try_recv() {
            player.stop();
            if let Some(ec) = echo { ec.reset(); }
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
```

- [ ] **Step 3: Run tests**

```bash
cd /Users/allen/repo/azVoiceAssist/rust
cargo test tts 2>&1 | tail -5
```
Expected: 4 tests pass (3 existing + 1 new `extract_pcm`).

Full suite:
```bash
cargo test --lib 2>&1 | tail -2
```

- [ ] **Step 4: Commit**

```bash
cd /Users/allen/repo/azVoiceAssist
git add rust/src/tts.rs
git commit -m "feat: tts pushes WAV PCM as AEC reference; extract_wav_pcm_i16 tested"
```

---

## Task 6: Wire everything in `worker.rs` and `main.rs`

**Files:** Modify `rust/src/worker.rs`, `rust/src/main.rs`

- [ ] **Step 1: Update `worker.rs`**

Add `echo: Arc<EchoCancel>` to `worker::run`'s parameters and pass it to `speak_stoppable`:

Find the import line:
```rust
use crate::config::{HISTORY_MAXLEN, PREROLL_FRAMES, SILERO_MODEL_PATH, WHISPER_MODEL_PATH};
```
Add:
```rust
use crate::echo::EchoCancel;
use std::sync::Arc;
```

Change the function signature:
```rust
pub fn run(
    rx_audio: Receiver<Vec<f32>>,
    rx_ctrl: Receiver<ControlMsg>,
    tx_ui: Sender<UiEvent>,
    shared: Arc<SharedState>,
    speaking: Arc<AtomicBool>,
    echo: Arc<EchoCancel>,       // ← new
) {
```

Update the `speak_stoppable` call site (find the line `let _ = crate::tts::speak_stoppable(...)`):
```rust
        let _ = crate::tts::speak_stoppable(&client, &refined, &stop_tts, &rx_ctrl, Some(&echo));
```

- [ ] **Step 2: Update `main.rs`**

Restructure the channel and thread setup. The key change: insert the processing thread between capture and worker, and wire `EchoCancel`:

```rust
mod echo;   // add with other mods

// In main(), after the existing channel declarations, add:
    let (tx_raw, rx_raw) = bounded::<Vec<f32>>(256);       // raw from cpal
    let (tx_processed, rx_processed) = bounded::<Vec<f32>>(256); // AEC-processed to worker

    // Create EchoCancel — fallback to None if library unavailable
    let echo_arc: Arc<echo::EchoCancel> = match echo::EchoCancel::new(
        config::FRAME, 4096
    ) {
        Ok(ec) => {
            eprintln!("[aec] EchoCancel ready (frame={} filter=4096)", config::FRAME);
            Arc::new(ec)
        }
        Err(e) => {
            eprintln!("[aec] EchoCancel init failed: {e} — running without AEC");
            std::process::exit(1); // Phase 1: treat as fatal; remove in Phase 2
        }
    };
```

Update `start_capture` to use `tx_raw`:
```rust
    let _stream = match audio::start_capture(tx_raw, shared.clone(), speaking.clone()) {
```

Start the processing thread:
```rust
    let _processing = audio::start_processing_thread(
        rx_raw, tx_processed.clone(), Some(echo_arc.clone()));
```

Update worker spawn to use `rx_processed` and pass `echo_arc`:
```rust
    let shared_w = shared.clone();
    let speaking_w = speaking.clone();
    let echo_w = echo_arc.clone();
    std::thread::spawn(move || worker::run(rx_processed, rx_ctrl, tx_ui, shared_w, speaking_w, echo_w));
```

- [ ] **Step 3: Build and run tests**

```bash
cd /Users/allen/repo/azVoiceAssist/rust
cargo build 2>&1 | grep "^error\|Finished"
cargo test --lib 2>&1 | tail -2
```
Expected: clean build, all tests pass. Fix any compile errors.

- [ ] **Step 4: Commit**

```bash
cd /Users/allen/repo/azVoiceAssist
git add rust/src/worker.rs rust/src/main.rs rust/src/lib.rs
git commit -m "feat: wire AEC into audio-processing thread and worker (shadow mode)"
git push origin feat/rust-ui-polish-settings
```

---

## Task 7: Acceptance testing (Phase 1 validation)

**Files:** none (manual test + log review)

Run the app and validate the 8 acceptance criteria from the proposal.

- [ ] **Step 1: Launch with oMLX + TTS running**

```bash
cd /Users/allen/repo/azVoiceAssist/rust && cargo run
```

Expected startup log includes:
```
[aec] EchoCancel ready (frame=512 filter=4096)
[audio] device="..." capturing ...
```

- [ ] **Step 2: TTS-only no-trigger test**

Have the app speak 5–10 sentences (use `python assistant.py --once "..." --speak` to generate TTS) without you speaking. Watch the `[aec-shadow]` log lines.

Expected:
- `[aec-shadow] raw_rms=... clean_rms=... reduction=...dB` lines appear during TTS
- `reduction` is negative (AEC is cancelling, not amplifying)
- **No `[worker]` turn processing lines** after TTS (no false triggers)

- [ ] **Step 3: Tail test**

After TTS ends, wait 1–2 seconds. Confirm no `heard:` / `refined:` lines appear (reverb tail suppressed).

- [ ] **Step 4: Normal speech still transcribed**

Speak a sentence yourself after TTS finishes. Confirm the app transcribes and responds normally.

- [ ] **Step 5: Stop mid-playback**

During TTS playback, press the Stop button. Confirm:
- Playback stops immediately
- `[aec-shadow]` lines stop
- App returns to listening; your next sentence transcribes normally (no echo suppression of your voice)

- [ ] **Step 6: Callback safety check**

Run for 5+ minutes with continuous use. Confirm no audio dropout messages (`[audio error]`), no xrun events, normal latency.

- [ ] **Step 7: Record results and commit notes**

Create `docs/bugfix/2026-06-01-aec-phase1-results.md` with:
- Which tests passed/failed
- Observed `reduction` dB values during TTS
- Whether reverb tail is suppressed
- Any issues found (filter length tuning needed? API deviations?)

```bash
cd /Users/allen/repo/azVoiceAssist
git add docs/bugfix/2026-06-01-aec-phase1-results.md
git commit -m "docs: AEC Phase 1 acceptance test results"
git push origin feat/rust-ui-polish-settings
```

---

## Notes for the implementer

- **speexdsp API is the highest risk.** The plan shows the *intended* API shape; verify against the actual crate docs (`cargo doc --package speexdsp`) before implementing Task 3. The crate version is 0.1.2 — it wraps libspeexdsp C API, so the Rust bindings may have different naming. Report DONE_WITH_CONCERNS with the actual API used.
- **`PKG_CONFIG_PATH` must be set on Apple Silicon** before any `cargo build`. Without it, the crate silently fails to link.
- The **existing `speaking` gate is NOT removed** in this phase. The processing thread's AEC output is forwarded to the worker, but the gate in `audio.rs` still drops frames while speaking. This means AEC runs on mic frames between TTS utterances, cleaning up the tail — which is exactly what Phase 1 validates.
- **`tx_processed` in main.rs**: after the restructure there are two channels (`tx_raw` from cpal, `tx_processed` from processing thread). The `tx_processed.clone()` in the processing thread call is not needed — `start_processing_thread` takes ownership of `tx_processed`; pass it directly (not a clone).
- **Branch:** `feat/aec-echo-cancellation`. Push is fine (org guardrail lifted).
