# Rust Desktop App P0 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A native Rust desktop app (egui) that re-proves the listen→transcribe→refine→speak loop with parity to the shipped Python app, reusing oMLX for refine and adding Qwen3-TTS (MLX-Audio) for speech.

**Architecture:** One Rust binary: egui on the main thread, a worker thread running cpal→Silero(ONNX)→whisper-rs→oMLX→TTS, wired by `crossbeam-channel` + an `AtomicBool`. Two local model services over HTTP: oMLX (`:8002`, reused) and a thin persistent Python MLX-Audio TTS server. Pure logic ported 1:1 from the Python (segmenter pre-roll, history window, timing, state machine) and unit-tested; I/O is integration-verified.

**Tech Stack:** Rust (`eframe`/`egui`, `cpal`, `rodio`, `ort`+`ndarray`, `whisper-rs`, `reqwest` blocking, `serde_json`, `crossbeam-channel`); Python MLX-Audio (`mlx-audio` + `fastapi`/`uvicorn`) for the TTS sidecar.

**Spec:** `docs/superpowers/specs/2026-05-30-rust-desktop-p0-design.md` · **Diagrams:** `docs/archi/02-target-rust-desktop.md`

**Reminder:** Commit locally only — do NOT `git push` (org guardrail; user pushes manually). Branch `feat/rust-desktop-p0`.

---

## IMPORTANT: two code categories in this plan

- **Pure-logic tasks (2–6)** — complete, compile-ready Rust + `cargo test` (test-first TDD). These port directly from the proven Python and carry the real bugs.
- **I/O / ML tasks (1, 7–11)** — exact crates, the specific calls, and the wiring are given, but **the implementer must confirm function signatures against the pinned crate version** (these crates move fast). Verification is `cargo build` + a runtime smoke step, not a unit test. If a documented API differs, report DONE_WITH_CONCERNS and adapt — do not invent.

Pin exact crate versions in Task 1 and keep them fixed for the whole P0.

## File Structure (`rust/` crate)

```
rust/
  Cargo.toml
  src/
    main.rs        # eframe entry; startup checks; spawn worker; run UI
    config.rs      # constants (sample rate, frame, ports, model ids, prompt, paths)
    events.rs      # State enum, UiEvent, ControlMsg (channel message types)
    segmenter.rs   # Segmenter (pre-roll ring) — PURE, tested
    timing.rs      # TurnTiming + format line — PURE, tested
    history.rs     # bounded history + oMLX messages builder — PURE, tested
    state.rs       # SharedState (current_state, listening_enabled) + transitions — PURE, tested
    refine.rs      # oMLX chat: pure body builder (tested) + blocking reqwest call
    tts.rs         # Qwen3-TTS client: pure body builder (tested) + reqwest + rodio playback
    vad.rs         # Silero VAD via ort/ONNX  (I/O)
    stt.rs         # whisper-rs wrapper       (I/O)
    audio.rs       # cpal capture             (I/O)
    worker.rs      # pipeline orchestration   (I/O assembly)
    ui.rs          # egui app                 (I/O)
tts_service/
  server.py        # persistent MLX-Audio Qwen3-TTS server: POST /tts {text} -> wav
  requirements.txt
```

The Python app stays untouched; this is additive.

---

## Task 1: Cargo scaffold + config + event types

**Files:** Create `rust/Cargo.toml`, `rust/src/main.rs`, `rust/src/config.rs`, `rust/src/events.rs`.

- [ ] **Step 1: Create `rust/Cargo.toml`** (pin versions; confirm latest-compatible at build time)

```toml
[package]
name = "azva"
version = "0.1.0"
edition = "2021"

[dependencies]
eframe = "0.29"
egui = "0.29"
cpal = "0.15"
rodio = "0.19"
ort = "2.0"
ndarray = "0.16"
whisper-rs = "0.12"
reqwest = { version = "0.12", features = ["blocking", "json"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
crossbeam-channel = "0.5"
```

- [ ] **Step 2: Create `rust/src/config.rs`**

```rust
pub const SAMPLE_RATE: u32 = 16_000;
pub const FRAME: usize = 512;                  // Silero window at 16 kHz
pub const PREROLL_MS: usize = 250;
pub const PREROLL_FRAMES: usize = (PREROLL_MS * SAMPLE_RATE as usize) / (1000 * FRAME); // ~7
pub const MIN_SILENCE_MS: u32 = 700;
pub const SPEECH_THRESHOLD: f32 = 0.5;
pub const HISTORY_MAXLEN: usize = 40;          // ~20 turns

pub const OMLX_URL: &str = "http://127.0.0.1:8002/v1/chat/completions";
pub const OMLX_MODEL: &str = "gemma-4-e4b-it-8bit";
pub const OMLX_API_KEY: &str = "rdaz1234";
pub const TTS_URL: &str = "http://127.0.0.1:8123/tts";

pub const SYSTEM_PROMPT: &str = "You are a refinement assistant. The user gives you a raw spoken utterance. Repeat it back, cleaned up: fix grammar, drop filler words and false starts, keep the meaning and tone. Reply with ONLY the refined sentence, nothing else.";

pub const WHISPER_MODEL_PATH: &str = "models/ggml-base.en.bin";
pub const SILERO_MODEL_PATH: &str = "models/silero_vad.onnx";
```

- [ ] **Step 3: Create `rust/src/events.rs`**

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State { Listening, Thinking, Speaking, Muted }

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::Listening => "listening",
            State::Thinking => "thinking",
            State::Speaking => "speaking",
            State::Muted => "muted",
        }
    }
}

use crate::timing::TurnTiming;

#[derive(Clone, Debug)]
pub enum UiEvent {
    StateChanged(State),
    Turn { heard: String, refined: String, timing: TurnTiming },
    Cleared,
}

#[derive(Clone, Copy, Debug)]
pub enum ControlMsg { ToggleMic, Clear, Stop }
```

- [ ] **Step 4: Create `rust/src/main.rs`** (minimal stub; replaced in Task 10)

```rust
mod config;

fn main() {
    println!("azva scaffold OK");
}
```

**Module wiring rule (applies to every task):** a `.rs` file is only compiled once a `mod <name>;` line declares it. So `events.rs` (created in this task) and the modules from Tasks 2–10 are NOT compiled until you add their `mod` line. **As you complete each task, add that module's `mod` line to `main.rs`** (and any `#[cfg(test)] mod tests` runs once declared). This keeps every task's `cargo build`/`cargo test` green. Note `events.rs` references `timing::TurnTiming`, so declare `mod events;` only after Task 3 exists. `cargo test <name>` compiles the crate including the named module's tests regardless of main wiring, so the pure-logic tasks (2–6) test cleanly on their own.

- [ ] **Step 5: Verify it builds**

Run: `cd rust && cargo build 2>&1 | tail -5`
Expected: compiles (warnings about unused are fine). If a pinned crate version fails to resolve, bump to the latest compatible and note it.

- [ ] **Step 6: Commit**

```bash
git add rust/Cargo.toml rust/src/
git commit -m "chore: rust crate scaffold (config + event types)"
```

---

## Task 2: `Segmenter` (pre-roll) — PURE, TDD

**Files:** Create `rust/src/segmenter.rs`. Port of the Python `Segmenter` + its tests.

- [ ] **Step 1: Write the failing tests** (append to `segmenter.rs`)

```rust
use std::collections::VecDeque;

pub struct Segmenter {
    preroll: VecDeque<Vec<f32>>,
    preroll_cap: usize,
    buffer: Vec<Vec<f32>>,
    collecting: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(v: f32) -> Vec<f32> { vec![v; 4] }

    #[test]
    fn emits_nothing_before_start() {
        let mut s = Segmenter::new(2);
        assert!(s.push(frame(0.0), None).is_none());
        assert!(s.push(frame(0.0), None).is_none());
    }

    #[test]
    fn prepends_preroll_on_start() {
        let mut s = Segmenter::new(2);
        s.push(frame(1.0), None);
        s.push(frame(1.0), None);
        assert!(s.push(frame(2.0), Some(VadEvent::Start)).is_none());
        let utt = s.push(frame(2.0), Some(VadEvent::End)).unwrap();
        // 2 preroll + start + end = 4 frames of 4 samples = 16
        assert_eq!(utt.len(), 16);
        assert_eq!(utt[0], 1.0);          // preroll present (no onset clip)
        assert_eq!(*utt.last().unwrap(), 2.0);
    }

    #[test]
    fn resets_between_utterances() {
        let mut s = Segmenter::new(1);
        s.push(frame(1.0), Some(VadEvent::Start));
        s.push(frame(1.0), Some(VadEvent::End));
        s.push(frame(3.0), Some(VadEvent::Start));
        let utt = s.push(frame(3.0), Some(VadEvent::End)).unwrap();
        assert!(utt.iter().all(|&x| x == 3.0));   // no leak from first utterance
    }
}
```

- [ ] **Step 2: Run, expect fail**

Run: `cd rust && cargo test segmenter 2>&1 | tail -15`
Expected: FAIL — `VadEvent`/`Segmenter::new`/`push` not found.

- [ ] **Step 3: Implement** (prepend above the tests in `segmenter.rs`)

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VadEvent { Start, End }

impl Segmenter {
    pub fn new(preroll_frames: usize) -> Self {
        Self { preroll: VecDeque::new(), preroll_cap: preroll_frames,
               buffer: Vec::new(), collecting: false }
    }

    /// Returns a flattened utterance (pre-roll + speech) on End, else None.
    pub fn push(&mut self, frame: Vec<f32>, event: Option<VadEvent>) -> Option<Vec<f32>> {
        match event {
            Some(VadEvent::Start) => {
                self.collecting = true;
                self.buffer = self.preroll.drain(..).collect();
                self.buffer.push(frame);
                None
            }
            _ if self.collecting => {
                self.buffer.push(frame);
                if event == Some(VadEvent::End) {
                    self.collecting = false;
                    let utt: Vec<f32> = self.buffer.drain(..).flatten().collect();
                    Some(utt)
                } else { None }
            }
            _ => {
                if self.preroll.len() == self.preroll_cap && self.preroll_cap > 0 {
                    self.preroll.pop_front();
                }
                if self.preroll_cap > 0 { self.preroll.push_back(frame); }
                None
            }
        }
    }
}
```

- [ ] **Step 4: Run, expect pass**

Run: `cd rust && cargo test segmenter 2>&1 | tail -5` → `test result: ok. 3 passed`.

- [ ] **Step 5: Commit**

```bash
git add rust/src/segmenter.rs
git commit -m "feat: rust Segmenter with onset pre-roll (ported + tested)"
```

---

## Task 3: `TurnTiming` — PURE, TDD

**Files:** Create `rust/src/timing.rs`.

- [ ] **Step 1: Write the failing test**

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TurnTiming {
    pub endpoint_ms: u32,
    pub stt_ms: u32,
    pub refine_ms: u32,
    pub reply_start_ms: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formats_line() {
        let t = TurnTiming { endpoint_ms: 700, stt_ms: 240, refine_ms: 180, reply_start_ms: 430 };
        assert_eq!(t.format(), "endpoint ~700ms · stt 240ms · refine 180ms · reply-start +430ms");
    }
}
```

- [ ] **Step 2: Run, expect fail**

Run: `cd rust && cargo test timing 2>&1 | tail -10` → FAIL (`format` not found).

- [ ] **Step 3: Implement** (add above tests)

```rust
impl TurnTiming {
    pub fn format(&self) -> String {
        format!("endpoint ~{}ms · stt {}ms · refine {}ms · reply-start +{}ms",
                self.endpoint_ms, self.stt_ms, self.refine_ms, self.reply_start_ms)
    }
}
```

- [ ] **Step 4: Run, expect pass** → `1 passed`.

- [ ] **Step 5: Commit**

```bash
git add rust/src/timing.rs
git commit -m "feat: rust TurnTiming format"
```

---

## Task 4: `history` + oMLX messages builder — PURE, TDD

**Files:** Create `rust/src/history.rs`.

- [ ] **Step 1: Write the failing tests**

```rust
use std::collections::VecDeque;
use serde_json::{json, Value};

#[derive(Clone)]
pub struct History {
    turns: VecDeque<Value>,     // each is {"role":..., "content":...}
    cap: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_user_then_build_messages_includes_system_and_window() {
        let mut h = History::new(40);
        let msgs = h.record_user_and_build("um the meeting tomorrow", "SYS");
        assert_eq!(msgs[0], serde_json::json!({"role":"system","content":"SYS"}));
        assert_eq!(*msgs.last().unwrap(),
                   serde_json::json!({"role":"user","content":"um the meeting tomorrow"}));
        h.record_assistant("The meeting is tomorrow.");
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn history_is_bounded() {
        let mut h = History::new(4);   // 2 turns
        for (u, a) in [("one","r1"),("two","r2"),("three","r3")] {
            h.record_user_and_build(u, "SYS");
            h.record_assistant(a);
        }
        assert_eq!(h.len(), 4);
        let contents: Vec<String> = h.iter_contents();
        assert!(!contents.contains(&"one".to_string()));   // oldest dropped
        assert!(contents.contains(&"three".to_string()));
    }
}
```

- [ ] **Step 2: Run, expect fail**

Run: `cd rust && cargo test history 2>&1 | tail -15` → FAIL.

- [ ] **Step 3: Implement** (add above tests)

```rust
impl History {
    pub fn new(cap: usize) -> Self { Self { turns: VecDeque::new(), cap } }
    pub fn len(&self) -> usize { self.turns.len() }

    fn push(&mut self, v: Value) {
        if self.turns.len() == self.cap && self.cap > 0 { self.turns.pop_front(); }
        if self.cap > 0 { self.turns.push_back(v); }
    }

    /// Append the user turn, then return [system] + window for the oMLX request.
    pub fn record_user_and_build(&mut self, text: &str, system: &str) -> Vec<Value> {
        self.push(json!({"role":"user","content":text}));
        let mut msgs = vec![json!({"role":"system","content":system})];
        msgs.extend(self.turns.iter().cloned());
        msgs
    }

    pub fn record_assistant(&mut self, text: &str) {
        self.push(json!({"role":"assistant","content":text}));
    }

    pub fn clear(&mut self) { self.turns.clear(); }

    #[cfg(test)]
    pub fn iter_contents(&self) -> Vec<String> {
        self.turns.iter()
            .map(|v| v["content"].as_str().unwrap_or("").to_string()).collect()
    }
}
```

- [ ] **Step 4: Run, expect pass** → `2 passed`.

- [ ] **Step 5: Commit**

```bash
git add rust/src/history.rs
git commit -m "feat: rust bounded history + oMLX messages builder"
```

---

## Task 5: `SharedState` (mic toggle precedence, clear) — PURE, TDD

**Files:** Create `rust/src/state.rs`.

- [ ] **Step 1: Write the failing tests**

```rust
use std::sync::atomic::{AtomicBool, Ordering};
use crate::events::State;

/// Pure model of the UI-control state machine (the UiBus analog).
pub struct SharedState {
    pub listening_enabled: AtomicBool,
    current: std::sync::Mutex<State>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_mic_when_idle_repaints_muted_then_listening() {
        let s = SharedState::new();
        assert_eq!(s.current(), State::Listening);
        assert_eq!(s.toggle_mic(), Some(State::Muted));   // idle -> muted
        assert_eq!(s.listening_enabled.load(Ordering::SeqCst), false);
        assert_eq!(s.toggle_mic(), Some(State::Listening)); // back on
        assert_eq!(s.listening_enabled.load(Ordering::SeqCst), true);
    }

    #[test]
    fn toggle_mic_mid_turn_does_not_repaint() {
        let s = SharedState::new();
        s.set(State::Speaking);
        assert_eq!(s.toggle_mic(), None);                 // mid-turn: flag flips, no repaint
        assert_eq!(s.listening_enabled.load(Ordering::SeqCst), false);
    }

    #[test]
    fn idle_state_reflects_mic() {
        let s = SharedState::new();
        s.toggle_mic();                       // now disabled
        assert_eq!(s.idle_state(), State::Muted);
        s.toggle_mic();                       // enabled
        assert_eq!(s.idle_state(), State::Listening);
    }
}
```

- [ ] **Step 2: Run, expect fail**

Run: `cd rust && cargo test state 2>&1 | tail -15` → FAIL.

- [ ] **Step 3: Implement** (add above tests)

```rust
impl SharedState {
    pub fn new() -> Self {
        Self { listening_enabled: AtomicBool::new(true),
               current: std::sync::Mutex::new(State::Listening) }
    }
    pub fn current(&self) -> State { *self.current.lock().unwrap() }
    pub fn set(&self, s: State) { *self.current.lock().unwrap() = s; }

    pub fn idle_state(&self) -> State {
        if self.listening_enabled.load(Ordering::SeqCst) { State::Listening } else { State::Muted }
    }

    /// Flip the mic. Returns Some(new idle state) to repaint ONLY when idle; None mid-turn.
    pub fn toggle_mic(&self) -> Option<State> {
        let now = !self.listening_enabled.load(Ordering::SeqCst);
        self.listening_enabled.store(now, Ordering::SeqCst);
        let cur = self.current();
        if cur == State::Listening || cur == State::Muted {
            let s = if now { State::Listening } else { State::Muted };
            self.set(s);
            Some(s)
        } else { None }
    }
}
```

- [ ] **Step 4: Run, expect pass** → `3 passed`. Then `cd rust && cargo test 2>&1 | tail -5` (all pure tests green).

- [ ] **Step 5: Commit**

```bash
git add rust/src/state.rs
git commit -m "feat: rust SharedState mic/idle transitions (mute precedence)"
```

---

## Task 6: `refine` — oMLX client (pure body builder TDD + reqwest call)

**Files:** Create `rust/src/refine.rs`.

- [ ] **Step 1: Write the failing test (pure body builder)**

```rust
use serde_json::{json, Value};

pub fn build_omlx_body(messages: Vec<Value>) -> Value {
    json!({ "model": crate::config::OMLX_MODEL, "messages": messages, "temperature": 0.3 })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn body_has_model_messages_temperature() {
        let msgs = vec![json!({"role":"user","content":"hi"})];
        let body = build_omlx_body(msgs.clone());
        assert_eq!(body["model"], crate::config::OMLX_MODEL);
        assert_eq!(body["messages"], json!(msgs));
        assert_eq!(body["temperature"], 0.3);
    }
}
```

- [ ] **Step 2: Run, expect fail** → `cd rust && cargo test refine 2>&1 | tail -10`.

- [ ] **Step 3: Implement the call** (add below the builder; **confirm reqwest blocking API at build time**)

```rust
use std::time::Duration;

pub fn refine(client: &reqwest::blocking::Client, messages: Vec<Value>)
    -> Result<String, String>
{
    let resp = client.post(crate::config::OMLX_URL)
        .bearer_auth(crate::config::OMLX_API_KEY)
        .json(&build_omlx_body(messages))
        .timeout(Duration::from_secs(30))
        .send().map_err(|e| format!("oMLX send: {e}"))?;
    let v: Value = resp.json().map_err(|e| format!("oMLX json: {e}"))?;
    v["choices"][0]["message"]["content"].as_str()
        .map(|s| s.trim().to_string())
        .ok_or_else(|| "oMLX: missing choices[0].message.content".into())
}
```

- [ ] **Step 4: Run, expect pass** (unit) → `1 passed`. Build check: `cargo build`.

- [ ] **Step 5: Live smoke (oMLX must be running)**

Add a temporary `#[test] #[ignore]` or a small `examples/refine_smoke.rs` that calls `refine` with `[{system},{user:"um the meetin is uh tomorrow"}]` and prints the result. Run `cargo run --example refine_smoke`. Expected: a cleaned sentence. Remove the example after, or keep under `examples/`.

- [ ] **Step 6: Commit**

```bash
git add rust/src/refine.rs
git commit -m "feat: rust oMLX refine client (body builder tested)"
```

---

## Task 7: Qwen3-TTS — Python MLX-Audio service + Rust client + playback

**Files:** Create `tts_service/server.py`, `tts_service/requirements.txt`, `rust/src/tts.rs`.

- [ ] **Step 1: Create `tts_service/requirements.txt`**

```
mlx_audio
fastapi
uvicorn
```
(Verified install: `pip install mlx_audio fastapi uvicorn`. `soundfile` is only needed if you switch to the in-memory `generate_voice_design` fallback.)

- [ ] **Step 2: Create `tts_service/server.py`** — **API VERIFIED via spike (2026-05-30):**
  - Model: `mlx-community/Qwen3-TTS-12Hz-1.7B-VoiceDesign-8bit` (instruct-based; no preset speaker needed).
  - `generate_audio(...)` writes a file (returns `None`); pass `stt_model=None` to skip an unneeded ~1.5 GB whisper download; output lands at `<output_path>/<file_prefix>_000.wav`; audio is 24 kHz mono.
  - **Persistence:** call `load_model(path)` ONCE at startup and pass the loaded module as `model=` to `generate_audio` so it is not reloaded per request. (Confirm at impl: if passing the Module doesn't reuse it, fall back to the lower-level `model.generate_voice_design(text=..., instruct=...)` which returns audio samples.)

```python
"""Persistent Qwen3-TTS server (MLX-Audio). POST /tts {"text": "..."} -> audio/wav.

Verified working: mlx_audio + Qwen3-TTS VoiceDesign-8bit on Apple Silicon,
~2x realtime, ~6GB peak RAM. Run: uvicorn server:app --port 8123
"""
import glob
import os
import tempfile
from fastapi import FastAPI
from fastapi.responses import Response
from pydantic import BaseModel
from mlx_audio.tts.utils import load_model
from mlx_audio.tts.generate import generate_audio

MODEL_PATH = "mlx-community/Qwen3-TTS-12Hz-1.7B-VoiceDesign-8bit"
INSTRUCT = "a warm, clear voice, calm and natural"

app = FastAPI()
model = load_model(MODEL_PATH)   # loaded once at startup (heavy)

class Req(BaseModel):
    text: str

@app.post("/tts")
def tts(req: Req):
    with tempfile.TemporaryDirectory() as d:
        generate_audio(text=req.text, model=model, instruct=INSTRUCT,
                       stt_model=None, output_path=d, file_prefix="o",
                       audio_format="wav", save=True, verbose=False)
        wav = sorted(glob.glob(os.path.join(d, "o*.wav")))[0]
        with open(wav, "rb") as f:
            data = f.read()
    return Response(content=data, media_type="audio/wav")
```

> Memory note: Qwen3-TTS peaks ~6 GB; it runs alongside oMLX's gemma — ensure enough RAM, or use a smaller oMLX model.

- [ ] **Step 3: Run the service + curl smoke test**

```bash
cd tts_service && python -m venv .venv && . .venv/bin/activate && pip install -r requirements.txt
uvicorn server:app --port 8123 &
sleep 30   # first model load
curl -s -X POST http://127.0.0.1:8123/tts -H 'Content-Type: application/json' \
  -d '{"text":"Hello, this is a test."}' -o /tmp/tts.wav && afplay /tmp/tts.wav
```
Expected: a `tts.wav` that plays intelligible speech. (First model download may take a while.)

- [ ] **Step 4: Write the failing test for the Rust client body builder (`rust/src/tts.rs`)**

```rust
use serde_json::{json, Value};

pub fn build_tts_body(text: &str) -> Value { json!({ "text": text }) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tts_body_has_text() {
        assert_eq!(build_tts_body("hi"), json!({"text":"hi"}));
    }
}
```

Run `cd rust && cargo test tts 2>&1 | tail -8` → fails then (after Step 5) passes.

- [ ] **Step 5: Implement fetch + playback** (add below; **confirm rodio Sink/Decoder API at build time**)

```rust
use std::io::Cursor;
use std::time::Duration;

/// Fetch wav bytes from the TTS service and block until playback finishes (or stop()).
pub fn speak(client: &reqwest::blocking::Client, text: &str) -> Result<(), String> {
    let bytes = client.post(crate::config::TTS_URL)
        .json(&build_tts_body(text))
        .timeout(Duration::from_secs(60))
        .send().map_err(|e| format!("tts send: {e}"))?
        .bytes().map_err(|e| format!("tts bytes: {e}"))?;
    let (_stream, handle) = rodio::OutputStream::try_default()
        .map_err(|e| format!("audio out: {e}"))?;
    let sink = rodio::Sink::try_new(&handle).map_err(|e| format!("sink: {e}"))?;
    let src = rodio::Decoder::new(Cursor::new(bytes.to_vec()))
        .map_err(|e| format!("decode: {e}"))?;
    sink.append(src);
    sink.sleep_until_end();   // interruptible via a shared Sink::stop in worker (Task 10)
    Ok(())
}
```

- [ ] **Step 6: cargo build + commit**

```bash
git add tts_service/ rust/src/tts.rs
git commit -m "feat: Qwen3-TTS MLX-Audio service + rust client/playback"
```

---

## Task 8: Silero VAD (ort/ONNX) + cpal capture — I/O

**Files:** Create `rust/src/vad.rs`, `rust/src/audio.rs`. Also fetch `models/silero_vad.onnx`.

> These wrap external runtimes; **confirm `ort` 2.x session/run API and Silero input/output tensor names + shapes against the model card.** Silero v5 ONNX takes a 512-sample f32 chunk + a state tensor + sample-rate; returns a speech probability + new state. Verify exact I/O names (`input`, `state`, `sr` / `output`, `stateN`) from the silero-vad repo for the version you download.

- [ ] **Step 1: Fetch the model**

```bash
cd rust && mkdir -p models
curl -L -o models/silero_vad.onnx \
  https://github.com/snakers4/silero-vad/raw/master/src/silero_vad/data/silero_vad.onnx
```

- [ ] **Step 2: Implement `vad.rs`** — a `Vad` struct holding the `ort::Session` + state, with `fn accept(&mut self, frame: &[f32]) -> Option<VadEvent>` that thresholds the probability and applies `MIN_SILENCE_MS` hysteresis to emit `Start`/`End` (mirror `VADIterator`'s min-silence logic). Reuse `crate::segmenter::VadEvent`. Keep the ONNX-run code isolated here.

  Verification: a unit-style test feeding `FRAME` of zeros several times returns no `Start` (silence). Run `cargo test vad`. (Speech detection itself is manual.)

- [ ] **Step 3: Implement `audio.rs`** — `fn start_capture(tx: Sender<Vec<f32>>, enabled: Arc<AtomicBool>, speaking: Arc<AtomicBool>)` opens a `cpal` input stream at 16 kHz mono f32, and in the callback, **if `enabled` and not `speaking`**, chunks the incoming samples into `FRAME`-sized `Vec<f32>` and sends them on `tx`. Returns the `Stream` (keep it alive). Confirm `cpal` device/config API.

  Verification: `cargo build`; a manual run in Task 11 confirms frames flow.

- [ ] **Step 4: Commit**

```bash
git add rust/src/vad.rs rust/src/audio.rs rust/models/.gitignore
git commit -m "feat: Silero VAD (ort) + cpal capture"
```
(Add `rust/models/` to `.gitignore` — don't commit model blobs.)

---

## Task 9: STT (whisper-rs) — I/O

**Files:** Create `rust/src/stt.rs`. Fetch `models/ggml-base.en.bin`.

> **Confirm `whisper-rs` 0.12 API** (`WhisperContext::new_with_params`, `FullParams`, `state.full(...)`, segment extraction). The model is English-only base.

- [ ] **Step 1: Fetch the model**

```bash
cd rust && curl -L -o models/ggml-base.en.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin
```

- [ ] **Step 2: Implement `stt.rs`** — `struct Stt { ctx: WhisperContext }` with `Stt::load(path)` and `fn transcribe(&self, audio: &[f32]) -> String` that runs whisper on the 16 kHz f32 samples and concatenates segment text, trimmed. Greedy params, language "en".

  Verification: `cargo build`. A manual smoke (Task 11) transcribes real speech; optionally an `examples/stt_smoke.rs` that loads a known wav.

- [ ] **Step 3: Commit**

```bash
git add rust/src/stt.rs
git commit -m "feat: whisper-rs STT wrapper (base.en)"
```

---

## Task 10: Worker pipeline + egui UI + main wiring — I/O assembly

**Files:** Create `rust/src/worker.rs`, `rust/src/ui.rs`; replace `rust/src/main.rs`.

- [ ] **Step 1: `worker.rs`** — `fn run(rx_audio, rx_ctrl, tx_ui, shared: Arc<SharedState>)`:
  - Owns `Vad`, `Segmenter::new(PREROLL_FRAMES)`, `History::new(HISTORY_MAXLEN)`, `Stt::load(...)`, a `reqwest::blocking::Client`, and a shared `rodio::Sink` handle for stop.
  - Loop: drain `rx_ctrl` (ToggleMic → `shared.toggle_mic()` → if `Some(s)` send `UiEvent::StateChanged(s)`; Clear → `history.clear()` + `tx_ui.send(Cleared)`; Stop → `sink.stop()`).
  - Pull a frame from `rx_audio`; `vad.accept(&frame)`; `segmenter.push(frame, event)`.
  - On utterance: `set Thinking`; time `stt.transcribe`; if empty → back to idle; `refine` (build via `history.record_user_and_build`, call `refine::refine`, then `history.record_assistant`); build `TurnTiming`; `tx_ui.send(Turn{...})`; set `speaking` atomic + `set Speaking`; `tts::speak`; clear `speaking`; `tx_ui.send(StateChanged(shared.idle_state()))`.
  - Wrap the per-turn work so an error logs + returns to idle (parity with Python guard scopes).

- [ ] **Step 2: `ui.rs`** — an `eframe::App`. Holds the latest `State`, a `Vec<(String,String)>` transcript, and `Sender<ControlMsg>`. In `update()`: drain `rx_ui` (StateChanged/Turn/Cleared) into local state and `ctx.request_repaint()`; draw the ☯ glyph colored per state (blue/purple/green/grey) with a simple animation (rotate on Thinking, alpha-pulse on Listening), the status label, the transcript (heard/refined rows), and Mic/Clear/Stop buttons that `tx_ctrl.send(...)`.

- [ ] **Step 3: `main.rs`** — startup:
  - reachability check: `reqwest` GET oMLX `/v1/models` and the TTS `/` (or a cheap probe); if either fails, show a clear error dialog/log and exit (parity with Python warm-up).
  - create channels + `Arc<SharedState>` + `Arc<AtomicBool> speaking`; `audio::start_capture(...)`; spawn `worker::run` on a thread; `eframe::run_native(...)` with the egui app.

- [ ] **Step 4: Build + full test suite still green**

Run: `cd rust && cargo build && cargo test 2>&1 | tail -5`
Expected: builds; all pure-logic tests (Tasks 2–6) still pass.

- [ ] **Step 5: Commit**

```bash
git add rust/src/worker.rs rust/src/ui.rs rust/src/main.rs
git commit -m "feat: worker pipeline + egui UI + main wiring"
```

---

## Task 11: End-to-end manual verification (human at the mic)

**Files:** none. Requires oMLX (`:8002`), the TTS service (`:8123`), whisper + silero models present, mic permission, headphones recommended.

- [ ] **Step 1:** Start oMLX and the TTS service (`uvicorn server:app --port 8123`).
- [ ] **Step 2:** `cd rust && cargo run`. Expected: a native window with a blue pulsing ☯ ("listening"); startup reachability check passed.
- [ ] **Step 3:** Speak a sentence, pause. Expect: ☯ → purple/spin (thinking) → green/glow (speaking); heard + refined rows appear; **Qwen3-TTS voice** plays the refined text (clearly nicer than `say`).
- [ ] **Step 4:** Mic button → ☯ dims to grey, capture stops; click again → resumes.
- [ ] **Step 5:** Stop button mid-playback → audio cuts off.
- [ ] **Step 6:** Clear → transcript empties + next turn starts fresh.
- [ ] **Step 7:** Note latency (reply-start) and voice quality vs the Python/`say` baseline. Record issues.

---

## Notes for the implementer

- **Do NOT `git push`** — commit locally; the user pushes manually.
- Pure-logic tasks (2–6) are real TDD with compile-ready code. I/O/ML tasks (1,7–10) give exact crates + calls but **you must confirm signatures against the installed crate versions** and report DONE_WITH_CONCERNS if an API differs — do not guess silently.
- `.gitignore`: add `rust/target/`, `rust/models/`, `tts_service/.venv/`.
- Keep the audio callback lock-free (atomics + channel only).
- Two services must be running for end-to-end: oMLX and the TTS server.
