# Voice Assistant P0 — Design Spec

**Date:** 2026-05-29
**Status:** Approved (design); pending implementation plan

## Context

We are building a long-lived, always-listening voice assistant. The P0 goal is to
**prove out the loop**, not the intelligence. The whole app is one persistent
Python process running a continuous `listen → transcribe → refine → speak → repeat`
loop. The session is "long-lived" because the loop never exits, conversation
history accumulates in memory, and the refine model stays warm.

The v0 task is deliberately trivial — "repeat what I said, but refined" (clean up
grammar, drop filler words and false starts). This keeps all focus on loop
mechanics and latency. Swapping the refine prompt for something smarter later is a
one-line change.

Success criteria (the bar): the loop **feels good** to talk to across many turns,
and round-trip **latency is acceptable**. Both are subjective, so the P0 makes them
observable (per-stage timing) and robust (errors don't break the loop).

## Decisions (locked from brainstorming)

| Area | Decision |
|------|----------|
| STT | `mlx-whisper` (Apple Silicon native, no torch) |
| VAD / turn-taking | Silero `silero-vad` (`VADIterator`), **hands-free now, barge-in-ready** |
| Refine | Existing **oMLX** server on `:8002` (OpenAI-compatible), model `gemma-4-e4b-it-8bit`, via `openai` client |
| TTS | macOS `say`, wrapped in an interruptible player |
| Runtime | **Resilient** (per-turn try/except, keep looping) **+ per-stage latency timing** |
| Code structure | **Single `assistant.py`** (~150 lines): focused functions + one small `TtsPlayer` class |

## Architecture

One process, one `main()` loop.

- A `sounddevice.InputStream` callback (audio thread) pushes 512-sample float32
  frames (16 kHz mono) onto a `queue.Queue`, continuously, for the life of the
  process.
- The main loop pulls frames → feeds Silero `VADIterator` → buffers audio between
  `start` and `end` speech events → on `end`, runs the turn pipeline:
  `transcribe → refine → speak`.
- TTS is a `TtsPlayer` wrapping a killable `say` subprocess.

**Half-duplex concession (the one barge-in seam):** while TTS is playing, a
`speaking` flag causes the capture callback to drop frames, so the assistant never
transcribes its own voice. This flag — plus the already-cancellable player — is the
*single, clearly-marked* spot that live barge-in will later replace (with acoustic
echo cancellation + speech-during-playback detection). Capture is already
continuous, so no rewrite is needed to get there.

### Why not full-duplex now

True barge-in requires acoustic echo cancellation (otherwise the mic hears the
Mac's own TTS and false-triggers). Real AEC (WebRTC APM / speexdsp) is a fragile,
heavy dependency. It is explicitly deferred; the architecture leaves the seam.

## Components (one file, small parts)

- **Config / env:** `OMLX_BASE_URL`, `OMLX_API_KEY`, `OMLX_MODEL` (defaults baked
  for local oMLX), `WHISPER_REPO`, VAD params (`min_silence_ms`, speech threshold),
  `SYSTEM_PROMPT`.
- **`transcribe(audio: np.ndarray) -> str`** — `mlx_whisper.transcribe(...)`.
- **`refine(text: str, history: list) -> str`** — calls oMLX via the `openai`
  client. Appends `{user}` and `{assistant}` turns to `history` (the long-lived
  record), but the refine *request* sends only the system prompt + current
  utterance, so output doesn't drift on past turns. Flipping to history-aware is
  one line — the intended seam for smarter behavior.
- **`class TtsPlayer`** — `.speak(text)` launches `subprocess.Popen(["say", ...])`
  and blocks until it finishes or is interrupted; `.stop()` terminates the
  subprocess. This is the barge-in seam.
- **`main()`** — opens the capture stream, runs the VAD loop, wraps each turn in
  `try/except`, prints timing.

## Data flow

```
mic → InputStream callback → frame queue → VADIterator
   → utterance buffer (np.concatenate on 'end')
   → transcribe() → refine() → TtsPlayer.speak()
   → history.append(...)   (record only)
```

## Error handling

- Each turn is wrapped in `try/except`: on failure, log `[error] <stage>: <msg>`,
  reset VAD state, clear the frame queue, and keep listening. A hiccup (oMLX
  timeout, empty audio, mic glitch) never breaks the loop.
- Startup performs the oMLX warm-up call; if the server is unreachable, exit early
  with a clear message (`oMLX not reachable at <url> — is it running?`).

## Latency instrumentation

`time.perf_counter` brackets each stage. After each turn, print one line, e.g.:

```
⏱ stt 240ms · refine 180ms · speak 90ms
```

This makes the "is latency acceptable?" success criterion answerable at a glance,
and the first vs. subsequent turns confirm the model stays warm (no cold start).

## Testing strategy

- **Headless `--once "raw text"` entrypoint:** runs `refine()` (and optionally
  `speak`) with **no microphone**. Makes the refine logic + oMLX wiring
  deterministically testable and TDD-able before any audio work. (Live oMLX already
  returns clean output for a messy test string — verified via curl during planning.)
- **VAD segmentation unit:** the buffer-between-start/end logic is pure given a
  sequence of VAD events; feed synthetic frame/event sequences and assert it emits
  exactly one buffered utterance per speech span.
- **Manual end-to-end:** run `assistant.py`, speak *"um so like I think the, the
  meeting is uh tomorrow"*, then pause. Expect console `heard:`/`refined:` lines,
  the Mac speaking the cleaned sentence, a timing line, and a second turn that stays
  warm (latency ≈ first turn).
- **Barge-in seam check:** confirm `TtsPlayer.stop()` actually kills playback
  mid-utterance (proves the seam works even though live barge-in is deferred).

## Environment / setup (verified)

- Use **python3.12** (`/opt/homebrew/bin/python3.12`) for the venv — default
  python3 is 3.14 with spotty ML wheels.
- `brew install portaudio` (required for mic capture; not currently installed).
- `pip install sounddevice numpy silero-vad mlx-whisper openai torch`
  (`silero-vad` pulls `torch`, used only for the tiny VAD model; mlx-whisper does
  not use torch). First run downloads `whisper-base-mlx` (~150 MB) from HuggingFace.
- macOS prompts for **microphone permission** for the host terminal/VSCode on first
  run. oMLX must be running on `:8002`.
- oMLX verified live: auth `Bearer rdaz1234`; `/v1/models` →
  `gemma-4-26b-a4b-it-4bit`, `gemma-4-31b-it-8bit`, `gemma-4-e4b-it-8bit` (chosen).

## Out of scope (deferred)

Live barge-in + AEC, streaming STT/LLM, nicer TTS (Piper/Kokoro), history-aware
refinement, menu-bar / Tauri shell, and using gemma-4's native audio-input
capability to replace mlx-whisper.
