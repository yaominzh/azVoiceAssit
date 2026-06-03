# azVoiceAssist — Setup & Run Guide

A local, always-listening voice assistant: **listen → transcribe → refine → speak**.
There are two front-ends sharing the same idea, plus two local model services.

```
┌─ Model services (local, long-lived) ──────────────────────────┐
│  oMLX            :8002   LLM (gemma) — refines the transcript  │
│  Qwen3-TTS       :8123   MLX-Audio speech synthesis            │
└────────────────────────────────────────────────────────────────┘
        ▲                              ▲
        │ HTTP                         │ HTTP
┌───────┴──────────────┐    ┌──────────┴───────────────┐
│ Python app           │    │ Rust desktop app          │
│ assistant.py         │    │ rust/  (egui native win)  │
│  • CLI               │    │  • whisper-rs STT         │
│  • --ui  (browser    │    │  • Silero VAD (ONNX)      │
│    :8765, SSE)        │    │  • cpal capture           │
│  • mlx-whisper STT    │    │                           │
│  • Silero VAD (torch) │    │                           │
└───────────────────────┘    └───────────────────────────┘
```

Architecture diagrams: `docs/archi/`. Design specs & plans: `docs/superpowers/`.

---

## Prerequisites (macOS, Apple Silicon)

- **oMLX** running on `:8002` (OpenAI-compatible LLM server). Models: `gemma-4-e4b-it-8bit`
  (default). API key (set via env var). This is external to this repo — start it separately.
- **Homebrew packages:** `brew install portaudio cmake`
  - `portaudio` — Python mic capture (`sounddevice`).
  - `cmake` — builds `whisper.cpp` for the Rust app (`whisper-rs`).
- **Python 3.12** (`/opt/homebrew/bin/python3.12`). The system default 3.14 has spotty ML wheels.
- **Rust toolchain** (`cargo`) — for the desktop app.
- A **working microphone**. A Mac mini has no built-in mic; use a USB/3.5mm/Bluetooth mic.

---

## 1. Qwen3-TTS service (`:8123`) — needed by both apps for nice speech

Persistent MLX-Audio server. Loads `mlx-community/Qwen3-TTS-12Hz-1.7B-VoiceDesign-8bit`
once (~6 GB RAM, ~2× realtime on Apple Silicon).

```bash
cd tts_service
python3.11 -m venv .venv          # or any python the mlx wheels support
.venv/bin/pip install -r requirements.txt
.venv/bin/uvicorn server:app --port 8123
```

First start downloads the model (~1.8 GB, one-time). Verify:

```bash
curl -s -X POST http://127.0.0.1:8123/tts \
  -H 'Content-Type: application/json' -d '{"text":"Hello."}' -o /tmp/t.wav && afplay /tmp/t.wav
```

Voice is set in `tts_service/server.py` via `INSTRUCT` (currently a calm mid-range male voice).

---

## 2. Python app (`assistant.py`)

```bash
brew install portaudio
/opt/homebrew/bin/python3.12 -m venv .venv
source .venv/bin/activate
pip install --upgrade pip
pip install -r requirements.txt          # sounddevice, silero-vad, mlx-whisper, openai, torch, ...
export OMLX_API_KEY=YOUR_API_KEY
```

First run downloads the Whisper weights (`whisper-base-mlx`, ~150 MB).

Run:
```bash
python assistant.py                       # CLI: prints heard/refined + timing
python assistant.py --ui                  # browser UI at http://localhost:8765 (SSE + controls)
python assistant.py --once "um the meetin is uh tomorrow"   # headless one-shot, no mic
```

- `--ui` serves the dark "Zen Center" page: ☯ state, heard/refined transcript, Mic/Clear/Stop.
  Port override: `UI_PORT=9000`. **Must not collide with oMLX `:8002`.**
- TTS in the Python app: currently macOS `say` (swap to the `:8123` service is future work).

---

## 3. Rust desktop app (`rust/`)

Native egui window (no browser). Reuses oMLX (`:8002`) and the Qwen3-TTS service (`:8123`).

```bash
cd rust
mkdir -p models
# Whisper model (English-only base, ~141 MB)
curl -L -o models/ggml-base.en.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin
# Silero VAD ONNX (~2 MB)
curl -L -o models/silero_vad.onnx \
  https://github.com/snakers4/silero-vad/raw/master/src/silero_vad/data/silero_vad.onnx

cargo build                                # first build compiles whisper.cpp (needs cmake)
cargo run                                  # opens the desktop window
```

> **Run `cargo run` from your own terminal (Terminal.app / iTerm), NOT via an editor/agent.**
> See "Microphone permission" below — this matters.

Diagnostics (handy if audio misbehaves):
```bash
cargo run --example list_devices          # list input devices + the system default
cargo run --example mic_test              # record 8s from default input, report level, play it back
```

---

## Microphone permission (macOS TCC) — important

macOS grants mic access **per launching application**, and a CLI binary inherits the
permission of whatever launched it. If the controlling app lacks mic access, the OS feeds
the binary **silent audio with no prompt** (you'll see captured peak ≈ 0).

- **Run the apps from a real terminal you control** (Terminal.app / iTerm2). The first mic
  access shows a "would like to access the microphone" prompt — click **OK**.
- Launching from an editor/agent whose host app lacks mic permission → silent capture.
- Confirm signal independently: **System Settings → Sound → Input** — the level meter should
  move when you speak on the selected device.

## Audio device notes

- The app uses the **system default input device**. Pick it in **Sound → Input**.
- On this setup, the live mic is **"External Microphone"** (3.5 mm); **AirPods** connect as a
  24 kHz input but were not reliable here — verify with `mic_test` / the Sound input meter.
- The Rust app opens the device at its native rate and **resamples to 16 kHz** internally.

## Ports

| Service        | Port  | Notes                                   |
|----------------|-------|-----------------------------------------|
| oMLX (LLM)     | 8002  | external; `Authorization: Bearer YOUR_API_KEY` |
| Qwen3-TTS      | 8123  | `tts_service/server.py`                 |
| Python `--ui`  | 8765  | browser UI (`UI_PORT` to override)      |
| Rust desktop   | —     | native window, no port                  |

## Tests

```bash
pytest -v          # Python: 24 tests (loop logic, SSE/bus, controls)
cd rust && cargo test --lib   # Rust: 14 tests (segmenter, history, state, timing, vad-silence, ...)
```

## Known limitations (deferred)

- **Echo:** without headphones, the assistant can hear its own TTS (no acoustic echo
  cancellation). Use headphones, or expect occasional self-triggering.
- **Live barge-in** (interrupting mid-speech) is not implemented; the player is interruptible
  (Stop button) but full-duplex barge-in needs AEC + a concurrent listener.
- **RAG / knowledge grounding** (speaking book + domain corpus) is the planned next phase —
  see `docs/superpowers/specs/` and `docs/archi/02-target-rust-desktop.md` (Phase 2).
