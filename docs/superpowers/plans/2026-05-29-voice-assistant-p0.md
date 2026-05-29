# Voice Assistant P0 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A single always-listening Python process that loops listen → transcribe → refine → speak, proving the loop feels good and latency is acceptable.

**Architecture:** One `assistant.py`. A `sounddevice` callback feeds 16 kHz frames to a queue; the main loop validates each frame to 512 samples, runs Silero VAD, assembles utterances with a pre-roll ring buffer (no onset clipping), then transcribes (mlx-whisper), refines (oMLX via the OpenAI client, fed a bounded history window), and speaks (macOS `say` via an interruptible `TtsPlayer`). Pure-logic units are dependency-injected so they test without audio/model/server.

**Tech Stack:** Python 3.12, sounddevice, silero-vad (+torch), mlx-whisper, openai (→ oMLX `:8002`), macOS `say`, pytest.

**Spec:** `docs/superpowers/specs/2026-05-29-voice-assistant-p0-design.md`

**Reminder:** Commit locally only. Do NOT `git push` (org guardrail blocks it; the user pushes manually).

---

## File Structure

- Create: `assistant.py` — the whole app (constants, pure helpers, `Segmenter`, `TtsPlayer`, `transcribe`, `refine`, `warm_up`, `main`, `--once`).
- Create: `tests/test_assistant.py` — unit tests for the pure/injectable pieces.
- Create: `requirements.txt` — dependencies.
- Create: `README.md` — setup + run.
- Exists: `.gitignore` (already has `.venv/`, `__pycache__/`).

### Module-level constants (defined once in `assistant.py`, referenced by tasks below)

```python
SAMPLE_RATE = 16000
FRAME = 512                                  # samples per VAD window at 16 kHz (~32 ms)
PREROLL_MS = 250
PREROLL_FRAMES = max(1, round(PREROLL_MS / 1000 * SAMPLE_RATE / FRAME))  # ~8 frames
MIN_SILENCE_MS = 700                         # VAD end-of-turn silence; the felt "endpoint" tax
SPEECH_THRESHOLD = 0.5
WHISPER_REPO = "mlx-community/whisper-base-mlx"
HISTORY_MAXLEN = 40                          # bounded deque (~20 turns); fed into refine
OMLX_BASE_URL = os.environ.get("OMLX_BASE_URL", "http://127.0.0.1:8002/v1")
OMLX_API_KEY = os.environ.get("OMLX_API_KEY", "rdaz1234")
OMLX_MODEL = os.environ.get("OMLX_MODEL", "gemma-4-e4b-it-8bit")
SYSTEM_PROMPT = (
    "You are a refinement assistant. The user gives you a raw spoken utterance. "
    "Repeat it back, cleaned up: fix grammar, drop filler words and false starts, "
    "keep the meaning and tone. Reply with ONLY the refined sentence, nothing else."
)
```

---

## Task 1: Project scaffolding (env, deps, README)

**Files:**
- Create: `requirements.txt`
- Create: `README.md`

- [ ] **Step 1: Write `requirements.txt`**

```
sounddevice
numpy
silero-vad
mlx-whisper
openai
torch
pytest
```

- [ ] **Step 2: Write `README.md`**

````markdown
# azVoiceAssist — P0

Always-listening voice loop: listen → transcribe → refine → speak.

## Setup (once)

```bash
brew install portaudio
/opt/homebrew/bin/python3.12 -m venv .venv
source .venv/bin/activate
pip install --upgrade pip
pip install -r requirements.txt
export OMLX_API_KEY=rdaz1234        # oMLX must be running on :8002
```

First run downloads `whisper-base-mlx` (~150 MB). macOS will prompt for mic permission.

## Run

```bash
python assistant.py            # live mic loop; speak, pause, hear it refined
python assistant.py --once "um so like the meetin is uh tomorrow"   # headless, no mic
```

## Test

```bash
pytest -v
```
````

- [ ] **Step 3: Create the venv and install (verifies wheels resolve on this machine)**

Run:
```bash
brew install portaudio
/opt/homebrew/bin/python3.12 -m venv .venv
source .venv/bin/activate && pip install --upgrade pip && pip install -r requirements.txt
```
Expected: all packages install without error; `python -c "import sounddevice, silero_vad, mlx_whisper, openai, torch, numpy"` prints nothing and exits 0.

- [ ] **Step 4: Commit**

```bash
git add requirements.txt README.md
git commit -m "chore: scaffold P0 deps and README"
```

---

## Task 2: `validate_frame` (512-sample guard)

**Files:**
- Create: `assistant.py` (constants block above + this function)
- Test: `tests/test_assistant.py`

- [ ] **Step 1: Write the failing test**

```python
import numpy as np
import assistant


def test_validate_frame_passes_exact():
    chunk = np.ones(assistant.FRAME, dtype=np.float32)
    out = assistant.validate_frame(chunk)
    assert out is chunk


def test_validate_frame_pads_short():
    chunk = np.ones(300, dtype=np.float32)
    out = assistant.validate_frame(chunk)
    assert out.shape == (assistant.FRAME,)
    assert np.all(out[:300] == 1.0)
    assert np.all(out[300:] == 0.0)


def test_validate_frame_skips_oversized():
    chunk = np.ones(600, dtype=np.float32)
    assert assistant.validate_frame(chunk) is None
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pytest tests/test_assistant.py -k validate_frame -v`
Expected: FAIL — `AttributeError: module 'assistant' has no attribute 'validate_frame'` (or ImportError until the constants/imports exist).

- [ ] **Step 3: Write minimal implementation**

Create `assistant.py` starting with the imports + the constants block from "File Structure" above, then:

```python
#!/usr/bin/env python3
"""P0 voice assistant: listen -> transcribe -> refine -> speak, in one warm loop."""
import os
import sys
import time
import queue
import subprocess
from collections import deque

import numpy as np

# (constants block from the plan's File Structure section goes here)


def validate_frame(chunk):
    """Coerce a captured block to exactly FRAME samples, or None if unusable.

    Silero VADIterator requires exactly 512-sample windows at 16 kHz; an xrun or
    odd final block would otherwise raise in the per-frame path and kill the loop.
    """
    n = len(chunk)
    if n == FRAME:
        return chunk
    if n < FRAME:
        return np.pad(chunk, (0, FRAME - n))
    return None
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pytest tests/test_assistant.py -k validate_frame -v`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add assistant.py tests/test_assistant.py
git commit -m "feat: frame validation guard for VAD intake"
```

---

## Task 3: `Segmenter` (pre-roll + utterance assembly)

**Files:**
- Modify: `assistant.py`
- Test: `tests/test_assistant.py`

- [ ] **Step 1: Write the failing test**

```python
def _frame(value):
    return np.full(assistant.FRAME, value, dtype=np.float32)


def test_segmenter_emits_nothing_before_start():
    seg = assistant.Segmenter(preroll_frames=2)
    assert seg.push(_frame(0.0), None) is None
    assert seg.push(_frame(0.0), None) is None


def test_segmenter_prepends_preroll_on_start():
    seg = assistant.Segmenter(preroll_frames=2)
    # Two silence frames (value 1.0) fill the pre-roll ring.
    seg.push(_frame(1.0), None)
    seg.push(_frame(1.0), None)
    # Speech starts (value 2.0) and then ends.
    assert seg.push(_frame(2.0), {"start": 0}) is None
    utt = seg.push(_frame(2.0), {"end": 1})
    assert utt is not None
    # Pre-roll (1.0) must appear before the speech frames (2.0) — no onset clip.
    n = assistant.FRAME
    assert np.any(utt[:n] == 1.0)
    assert utt[-1] == 2.0
    # 2 pre-roll + start frame + end frame = 4 frames concatenated.
    assert utt.shape[0] == 4 * n


def test_segmenter_resets_between_utterances():
    seg = assistant.Segmenter(preroll_frames=1)
    seg.push(_frame(1.0), {"start": 0})
    seg.push(_frame(1.0), {"end": 1})
    # Second utterance should not include the first one's buffered frames.
    seg.push(_frame(3.0), {"start": 0})
    utt = seg.push(_frame(3.0), {"end": 1})
    assert np.all(np.isin(np.unique(utt), [1.0, 3.0]))  # only preroll(1.0)+speech(3.0)
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pytest tests/test_assistant.py -k segmenter -v`
Expected: FAIL — `AttributeError: module 'assistant' has no attribute 'Segmenter'`.

- [ ] **Step 3: Write minimal implementation**

Add to `assistant.py`:

```python
class Segmenter:
    """Assembles utterances from (frame, vad_event) pairs, with onset pre-roll.

    VADIterator reports 'start' AFTER speech onset, so we keep a ring buffer of
    recent frames and seed the utterance with it to avoid clipping leading phonemes.
    push() returns a concatenated utterance on the 'end' event, else None.
    """

    def __init__(self, preroll_frames):
        self.preroll = deque(maxlen=preroll_frames)
        self.buffer = []
        self.collecting = False

    def push(self, frame, event):
        if not self.collecting:
            self.preroll.append(frame)
        if event and "start" in event:
            self.collecting = True
            self.buffer = list(self.preroll)   # includes the just-appended onset frame
            return None
        if self.collecting:
            self.buffer.append(frame)
            if event and "end" in event:
                self.collecting = False
                utterance = np.concatenate(self.buffer)
                self.buffer = []
                return utterance
        return None
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pytest tests/test_assistant.py -k segmenter -v`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add assistant.py tests/test_assistant.py
git commit -m "feat: VAD utterance segmenter with onset pre-roll"
```

---

## Task 4: `refine` with bounded history + injected chat function

**Files:**
- Modify: `assistant.py`
- Test: `tests/test_assistant.py`

- [ ] **Step 1: Write the failing test**

```python
def test_refine_appends_user_then_assistant_and_uses_window():
    captured = {}

    def fake_chat(messages):
        captured["messages"] = list(messages)
        return "Clean sentence."

    history = deque(maxlen=40)
    out = assistant.refine("um the meetin tomorrow", history, fake_chat)

    assert out == "Clean sentence."
    # System prompt first, then the user turn already appended before the call.
    assert captured["messages"][0] == {"role": "system", "content": assistant.SYSTEM_PROMPT}
    assert captured["messages"][-1] == {"role": "user", "content": "um the meetin tomorrow"}
    # History now records both sides.
    assert list(history)[-2:] == [
        {"role": "user", "content": "um the meetin tomorrow"},
        {"role": "assistant", "content": "Clean sentence."},
    ]


def test_refine_history_is_bounded():
    history = deque(maxlen=4)   # 2 turns
    assistant.refine("one", history, lambda m: "r1")
    assistant.refine("two", history, lambda m: "r2")
    assistant.refine("three", history, lambda m: "r3")
    assert len(history) == 4
    # Oldest ("one"/"r1") dropped; newest two turns retained.
    assert {m["content"] for m in history} == {"two", "r2", "three", "r3"}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pytest tests/test_assistant.py -k refine -v`
Expected: FAIL — `AttributeError: module 'assistant' has no attribute 'refine'`.

- [ ] **Step 3: Write minimal implementation**

Add to `assistant.py`:

```python
def refine(text, history, chat_fn):
    """Refine one utterance. Appends the turn to the bounded history window and
    feeds that window to chat_fn (so refine is context-aware but capped)."""
    history.append({"role": "user", "content": text})
    messages = [{"role": "system", "content": SYSTEM_PROMPT}] + list(history)
    out = chat_fn(messages).strip()
    history.append({"role": "assistant", "content": out})
    return out
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pytest tests/test_assistant.py -k refine -v`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add assistant.py tests/test_assistant.py
git commit -m "feat: context-aware refine over a bounded history window"
```

---

## Task 5: `format_timing` (latency readout)

**Files:**
- Modify: `assistant.py`
- Test: `tests/test_assistant.py`

- [ ] **Step 1: Write the failing test**

```python
def test_format_timing():
    line = assistant.format_timing(endpoint_ms=700, stt_ms=240, refine_ms=180, reply_start_ms=430)
    assert line == "⏱ endpoint ~700ms · stt 240ms · refine 180ms · reply-start +430ms"
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pytest tests/test_assistant.py -k format_timing -v`
Expected: FAIL — `AttributeError: ... 'format_timing'`.

- [ ] **Step 3: Write minimal implementation**

Add to `assistant.py`:

```python
def format_timing(endpoint_ms, stt_ms, refine_ms, reply_start_ms):
    """One-line per-turn latency readout. reply_start = end-of-speech -> TTS start
    (NOT playback duration). endpoint = the felt VAD silence tax."""
    return (
        f"⏱ endpoint ~{endpoint_ms}ms · stt {stt_ms}ms · "
        f"refine {refine_ms}ms · reply-start +{reply_start_ms}ms"
    )
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pytest tests/test_assistant.py -k format_timing -v`
Expected: 1 passed.

- [ ] **Step 5: Commit**

```bash
git add assistant.py tests/test_assistant.py
git commit -m "feat: latency readout isolating reply-start from playback"
```

---

## Task 6: `TtsPlayer` (interruptible speak/stop)

**Files:**
- Modify: `assistant.py`
- Test: `tests/test_assistant.py`

- [ ] **Step 1: Write the failing test**

```python
import threading
import time


def test_tts_player_stop_interrupts():
    # Use `sleep` as a stand-in for `say` so the test is deterministic and silent.
    player = assistant.TtsPlayer(cmd_prefix=("sleep",))
    start = time.perf_counter()
    t = threading.Thread(target=player.speak, args=("5",))
    t.start()
    time.sleep(0.2)
    player.stop()
    t.join(timeout=2)
    elapsed = time.perf_counter() - start
    assert not t.is_alive()
    assert elapsed < 2.0   # interrupted, did not wait the full 5s


def test_tts_player_speak_completes():
    player = assistant.TtsPlayer(cmd_prefix=("sleep",))
    start = time.perf_counter()
    player.speak("0.05")
    assert time.perf_counter() - start < 1.0
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pytest tests/test_assistant.py -k tts_player -v`
Expected: FAIL — `AttributeError: ... 'TtsPlayer'`.

- [ ] **Step 3: Write minimal implementation**

Add to `assistant.py`:

```python
class TtsPlayer:
    """Speaks text via macOS `say`, interruptibly. .speak() blocks until the
    utterance finishes or .stop() kills it. The interruptible primitive that a
    future barge-in feature builds on (live barge-in itself is out of P0 scope)."""

    def __init__(self, cmd_prefix=("say",)):
        self.cmd_prefix = tuple(cmd_prefix)
        self._proc = None

    def speak(self, text):
        self._proc = subprocess.Popen([*self.cmd_prefix, text])
        try:
            self._proc.wait()
        finally:
            self._proc = None

    def stop(self):
        proc = self._proc
        if proc and proc.poll() is None:
            proc.terminate()
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pytest tests/test_assistant.py -k tts_player -v`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add assistant.py tests/test_assistant.py
git commit -m "feat: interruptible TtsPlayer"
```

---

## Task 7: `transcribe` + `warm_up` + oMLX chat wiring

**Files:**
- Modify: `assistant.py`
- Test: `tests/test_assistant.py` (warm_up via injection; transcribe is a thin wrapper verified manually in Task 9)

- [ ] **Step 1: Write the failing test**

```python
def test_warm_up_calls_both_models():
    calls = {"chat": 0, "stt": 0, "stt_len": None}

    def fake_chat(messages):
        calls["chat"] += 1
        return "ok"

    def fake_transcribe(audio):
        calls["stt"] += 1
        calls["stt_len"] = len(audio)
        return ""

    assistant.warm_up(fake_chat, fake_transcribe)
    assert calls["chat"] == 1
    assert calls["stt"] == 1
    assert calls["stt_len"] == assistant.SAMPLE_RATE   # 1s of silence
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pytest tests/test_assistant.py -k warm_up -v`
Expected: FAIL — `AttributeError: ... 'warm_up'`.

- [ ] **Step 3: Write minimal implementation**

Add to `assistant.py`:

```python
import mlx_whisper                       # add near the top imports
from openai import OpenAI                # add near the top imports


def transcribe(audio):
    return mlx_whisper.transcribe(audio, path_or_hf_repo=WHISPER_REPO)["text"].strip()


def make_chat_fn(client):
    """Bind an oMLX-backed chat function: messages -> assistant string."""
    def chat_fn(messages):
        resp = client.chat.completions.create(
            model=OMLX_MODEL, messages=messages, temperature=0.3)
        return resp.choices[0].message.content
    return chat_fn


def warm_up(chat_fn, transcribe_fn):
    """Pre-warm BOTH models so turn-1 latency ~= later turns. The chat call also
    serves as the oMLX reachability check (raises if the server is down)."""
    chat_fn([{"role": "user", "content": "hi"}])
    transcribe_fn(np.zeros(SAMPLE_RATE, dtype=np.float32))
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pytest tests/test_assistant.py -k warm_up -v`
Expected: 1 passed.

- [ ] **Step 5: Commit**

```bash
git add assistant.py tests/test_assistant.py
git commit -m "feat: transcribe, oMLX chat fn, and dual-model warm-up"
```

---

## Task 8: `main()` loop + `--once` CLI wiring

**Files:**
- Modify: `assistant.py`

- [ ] **Step 1: Write `--once` headless path + `main()` (no automated test; manual checks in Task 9)**

Add to `assistant.py`:

```python
import sounddevice as sd                 # add near the top imports
import torch                             # add near the top imports
from silero_vad import load_silero_vad, VADIterator   # add near the top imports


def run_once(text, client, speak=False):
    """Headless: refine one line of text with no microphone. For deterministic
    manual/integration checks of the refine + oMLX wiring."""
    history = deque(maxlen=HISTORY_MAXLEN)
    refined = refine(text, history, make_chat_fn(client))
    print(f"  heard:   {text}")
    print(f"  refined: {refined}")
    if speak:
        TtsPlayer().speak(refined)
    return refined


def main():
    client = OpenAI(base_url=OMLX_BASE_URL, api_key=OMLX_API_KEY)

    if len(sys.argv) >= 3 and sys.argv[1] == "--once":
        run_once(sys.argv[2], client, speak="--speak" in sys.argv)
        return

    chat_fn = make_chat_fn(client)
    print("Loading models...")
    vad = VADIterator(load_silero_vad(), threshold=SPEECH_THRESHOLD,
                      sampling_rate=SAMPLE_RATE, min_silence_duration_ms=MIN_SILENCE_MS)
    try:
        warm_up(chat_fn, transcribe)
    except Exception as e:
        print(f"oMLX not reachable at {OMLX_BASE_URL} — is it running? ({e})")
        return

    history = deque(maxlen=HISTORY_MAXLEN)
    seg = Segmenter(PREROLL_FRAMES)
    player = TtsPlayer()
    audio_q = queue.Queue()
    state = {"speaking": False}

    def cb(indata, frames, time_, status):
        if not state["speaking"]:
            audio_q.put(indata[:, 0].copy())

    print("Listening. Speak, then pause. Ctrl-C to quit.")
    with sd.InputStream(samplerate=SAMPLE_RATE, channels=1, blocksize=FRAME,
                        dtype="float32", callback=cb):
        while True:
            chunk = audio_q.get()

            # Guard scope 1: frame intake + VAD feed (runs every frame).
            try:
                frame = validate_frame(chunk)
                if frame is None:
                    continue
                event = vad(torch.from_numpy(frame))
            except Exception as e:
                print(f"[error] vad: {e}")
                vad.reset_states()
                continue

            utterance = seg.push(frame, event)
            if utterance is None:
                continue

            # Guard scope 2: the turn pipeline.
            try:
                t0 = time.perf_counter()
                text = transcribe(utterance)
                t1 = time.perf_counter()
                if not text:
                    continue
                refined = refine(text, history, chat_fn)
                t2 = time.perf_counter()
                print(f"  heard:   {text}")
                print(f"  refined: {refined}")
                print(format_timing(
                    endpoint_ms=MIN_SILENCE_MS,
                    stt_ms=round((t1 - t0) * 1000),
                    refine_ms=round((t2 - t1) * 1000),
                    reply_start_ms=round((t2 - t0) * 1000)))
                state["speaking"] = True
                player.speak(refined)
            except Exception as e:
                print(f"[error] turn: {e}")
            finally:
                state["speaking"] = False
                vad.reset_states()
                with audio_q.mutex:
                    audio_q.queue.clear()


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\nbye")
```

- [ ] **Step 2: Verify the full test suite still passes (no regressions from the new imports)**

Run: `pytest -v`
Expected: all tests from Tasks 2–7 pass; new imports (`sounddevice`, `torch`, `silero_vad`) load cleanly.

- [ ] **Step 3: Verify `--once` against live oMLX**

Run: `python assistant.py --once "um so like i think the meetin is uh tomorrow"`
Expected: prints a `heard:` line and a `refined:` line whose text is a cleaned sentence (e.g., "I think the meeting is tomorrow."). Requires oMLX running on `:8002`.

- [ ] **Step 4: Commit**

```bash
git add assistant.py
git commit -m "feat: wire main loop, dual guards, timing, and --once CLI"
```

---

## Task 9: End-to-end manual verification

**Files:** none (verification only).

- [ ] **Step 1: Run the live loop**

Run: `python assistant.py`
Expected: `Loading models...` then `Listening. Speak, then pause.` (grant mic permission if prompted).

- [ ] **Step 2: Speak a messy sentence and verify the round trip**

Say *"um so like I think the, the meeting is uh tomorrow"*, then pause ~1s.
Expected: console shows `heard:` (raw), `refined:` (cleaned), and a `⏱ endpoint ... stt ... refine ... reply-start ...` line; the Mac speaks the cleaned sentence.

- [ ] **Step 3: Verify warm-start (turn-1 ≈ turn-2)**

Speak a second sentence without restarting.
Expected: `reply-start` on turn 2 is within the same ballpark as turn 1 (proves both models were pre-warmed; no cold-start spike).

- [ ] **Step 4: Verify no onset clipping**

Speak a sentence that starts with a hard consonant (e.g., *"Bring the report Friday"*).
Expected: the `heard:` transcript includes the leading word (pre-roll working), not "ring the report".

- [ ] **Step 5: Verify resilience**

Stop oMLX (or point `OMLX_BASE_URL` at a dead port) mid-session and speak.
Expected: a `[error] turn:` line prints and the loop keeps listening (does not crash). Restart oMLX and confirm the next turn works.

- [ ] **Step 6: Note results**

Record observed `reply-start` latency and whether turn-taking felt good. If sluggish, the first knobs are `MIN_SILENCE_MS` (endpoint feel) and `WHISPER_REPO` (`whisper-tiny-mlx` for speed). No code change needed to try them.

---

## Notes for the implementer

- **Do NOT `git push`** — org guardrail blocks it; commits stay local, the user pushes manually.
- Keep heavy work inside functions/`main()`, never at import time, so `tests/` import `assistant` without opening a stream or loading a model.
- `tests/` needs `assistant.py` importable: run pytest from the repo root (no `tests/__init__.py` required).
- The `--once` path and Task 9 require oMLX live on `:8002` with `OMLX_API_KEY` set.
