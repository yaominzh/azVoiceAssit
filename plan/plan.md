> ⚠️ **SUPERSEDED — do not implement from this file.**
> The canonical design is
> [`docs/superpowers/specs/2026-05-29-voice-assistant-p0-design.md`](../docs/superpowers/specs/2026-05-29-voice-assistant-p0-design.md).
> This early draft is missing the fixes from code review (Whisper warm-up,
> pre-roll, real latency metric, frame-intake guard, history-into-refine,
> corrected barge-in framing) and its inline code is stale. Kept only for history.

# P0: Long-lived Voice Assistant Loop

## Context

Goal is to prove out the **loop**, not the intelligence. The whole app is one
persistent Python process running a continuous `listen → transcribe → refine →
speak → repeat` loop. The session is "long-lived" because the loop never exits,
the conversation history accumulates in memory, and the refine model stays warm.

The v0 task ("repeat what I said, but refined") is deliberately trivial so all
focus stays on getting the loop + latency solid. Swapping the refine prompt for
something smarter later is a one-line change.

**Decisions locked in:**
- STT = `mlx-whisper` (Apple Silicon native, no torch)
- VAD = Silero (`silero-vad` pip pkg) for clean turn-taking
- **Refine = the already-deployed oMLX server on :8002** (OpenAI-compatible),
  NOT Ollama. oMLX runs as its own long-lived process with the model resident,
  so it *is* the "always-warm model on standby" requirement — for free. We just
  point the loop at it with the `openai` client.
- TTS = macOS `say`

## Environment facts (verified)

- Project dir `/Users/allen/repo/azVoiceAssist` is **empty** (greenfield, no git).
- Default `python3` is **3.14** → ML wheels are spotty. **Use python3.12**
  (`/opt/homebrew/bin/python3.12`, wheel-friendly) for the venv.
- **portaudio is NOT installed** — required for mic capture. Must `brew install portaudio`.
- **oMLX is live and confirmed working** at `http://127.0.0.1:8002/v1`
  (also LAN `192.168.8.30:8002`). Auth = `Authorization: Bearer rdaz1234`.
  `/v1/models` returns: `gemma-4-26b-a4b-it-4bit`, `gemma-4-31b-it-8bit`,
  **`gemma-4-e4b-it-8bit`** (chosen — smallest, fastest for testing).
  Smoke test of the refine prompt against it already returns clean output.
- `say` and `ffmpeg` are present. No Ollama pull needed (Ollama dropped from plan).

## Setup steps (run once, before the script)

```bash
brew install portaudio
cd /Users/allen/repo/azVoiceAssist
/opt/homebrew/bin/python3.12 -m venv .venv
source .venv/bin/activate
pip install --upgrade pip
pip install sounddevice numpy silero-vad mlx-whisper openai torch
export OMLX_API_KEY=rdaz1234        # script reads this; keeps the key out of source
```

> `silero-vad` pulls in `torch` (only for the tiny VAD model — mlx-whisper does
> not use torch). First run downloads the Whisper weights (`whisper-base-mlx`,
> ~150MB) from HuggingFace automatically. macOS prompts for **microphone
> permission** for the host terminal/VSCode on first run. oMLX must be running.

## File to create

Single file: `/Users/allen/repo/azVoiceAssist/assistant.py` (~120 lines).
Plus `requirements.txt` and a short `README.md` with the setup steps above.

### The five stages, mapped to code

| Stage | Implementation |
|-------|----------------|
| Mic capture | `sounddevice.InputStream`, 16kHz mono, 512-sample blocks |
| VAD (turn-taking) | Silero `VADIterator` — emits `start`/`end` events on speech edges |
| STT | `mlx_whisper.transcribe(audio_np, path_or_hf_repo=...)` |
| Refine | `openai` client → oMLX `/v1/chat/completions`, model `gemma-4-e4b-it-8bit` |
| TTS | `subprocess.run(["say", text])` |

### Key design points

- **Warm model = the oMLX server.** Because oMLX is a separate persistent process
  with the model resident, the loop never pays a cold start after the first call.
  A one-line warm-up ping at startup forces the model load before the first real turn.
- **Echo guard:** a `speaking` flag drops mic frames while `say` is talking, and
  the audio queue is cleared afterward, so the assistant never transcribes itself.
- **History as the long-lived record:** every utterance + refinement is appended to
  a `history` list (the persistent-state trait). The refine *call* itself only passes
  the current utterance, so refinement doesn't drift on past turns. Flipping to
  history-aware behavior later is one line — this is the seam for smarter behavior.
- **Config via env:** base URL / key / model read from `OMLX_*` env vars with sane
  defaults, so retargeting to the LAN IP or a bigger gemma is zero code change.

### Proposed `assistant.py`

```python
#!/usr/bin/env python3
"""P0 voice assistant: listen -> transcribe -> refine -> speak, in one warm loop."""
import os
import queue
import subprocess
import numpy as np
import sounddevice as sd
import torch
import mlx_whisper
from openai import OpenAI
from silero_vad import load_silero_vad, VADIterator

SAMPLE_RATE = 16000
FRAME = 512                       # samples per VAD window at 16kHz (~32ms)
WHISPER_REPO = "mlx-community/whisper-base-mlx"

OMLX_BASE_URL = os.environ.get("OMLX_BASE_URL", "http://127.0.0.1:8002/v1")
OMLX_API_KEY = os.environ.get("OMLX_API_KEY", "rdaz1234")
OMLX_MODEL = os.environ.get("OMLX_MODEL", "gemma-4-e4b-it-8bit")
SYSTEM_PROMPT = (
    "You are a refinement assistant. The user gives you a raw spoken utterance. "
    "Repeat it back, cleaned up: fix grammar, drop filler words and false starts, "
    "keep the meaning and tone. Reply with ONLY the refined sentence, nothing else."
)

client = OpenAI(base_url=OMLX_BASE_URL, api_key=OMLX_API_KEY)


def transcribe(audio: np.ndarray) -> str:
    return mlx_whisper.transcribe(audio, path_or_hf_repo=WHISPER_REPO)["text"].strip()


def refine(text: str, history: list) -> str:
    history.append({"role": "user", "content": text})           # long-lived record
    resp = client.chat.completions.create(
        model=OMLX_MODEL,
        messages=[{"role": "system", "content": SYSTEM_PROMPT},
                  {"role": "user", "content": text}],           # refine = stateless for now
        temperature=0.3,
    )
    out = resp.choices[0].message.content.strip()
    history.append({"role": "assistant", "content": out})
    return out


def main():
    print("Loading models...")
    vad = VADIterator(load_silero_vad(), sampling_rate=SAMPLE_RATE)
    client.chat.completions.create(                              # warm the oMLX model
        model=OMLX_MODEL, messages=[{"role": "user", "content": "hi"}], max_tokens=1)

    history, audio_q = [], queue.Queue()
    state = {"speaking": False}

    def cb(indata, frames, time_, status):
        if not state["speaking"]:
            audio_q.put(indata[:, 0].copy())

    print("Listening. Speak, then pause. Ctrl-C to quit.")
    buffer, collecting = [], False
    with sd.InputStream(samplerate=SAMPLE_RATE, channels=1, blocksize=FRAME,
                        dtype="float32", callback=cb):
        while True:
            chunk = audio_q.get()
            event = vad(torch.from_numpy(chunk))                 # None | {'start':..} | {'end':..}
            if event and "start" in event:
                collecting, buffer = True, [chunk]
            elif collecting:
                buffer.append(chunk)
                if event and "end" in event:
                    collecting = False
                    vad.reset_states()
                    text = transcribe(np.concatenate(buffer))
                    if not text:
                        continue
                    print(f"  heard:   {text}")
                    refined = refine(text, history)
                    print(f"  refined: {refined}")
                    state["speaking"] = True
                    subprocess.run(["say", refined])
                    state["speaking"] = False
                    with audio_q.mutex:                          # drop frames captured while speaking
                        audio_q.queue.clear()


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\nbye")
```

## Verification (end-to-end)

1. Complete setup; confirm oMLX is up:
   `curl -s http://127.0.0.1:8002/v1/models -H "Authorization: Bearer $OMLX_API_KEY"`
   (already verified — returns the three gemma models).
2. `python assistant.py` → wait for `Listening.`
3. Say a messy sentence, e.g. *"um so like I think the, the meeting is uh tomorrow"*,
   then pause. Expect:
   - console prints `heard:` (raw) and `refined:` (cleaned),
   - the Mac speaks back roughly *"I think the meeting is tomorrow."*
     (the exact refine prompt already produced "The meeting is tomorrow." in testing.)
4. Speak again without restarting → confirms the loop continues and oMLX stays warm
   (second turn latency ≈ first, no cold start).
5. Note round-trip latency. Easy knobs if sluggish: Whisper model size
   (`whisper-tiny-mlx`) and the oMLX model (e4b-8bit is already the smallest loaded).

## Out of scope for P0 (explicitly deferred)

Menu-bar / Tauri shell, barge-in (interrupting mid-speech), streaming STT/LLM,
nicer TTS (Piper/Kokoro), history-aware refinement, and using gemma-4's native
audio-input capability for STT (could later replace mlx-whisper entirely).
