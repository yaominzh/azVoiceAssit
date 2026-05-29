# Voice Assistant P0 — Design Spec

**Date:** 2026-05-29
**Status:** Approved (design, rev 2 — incorporates code-review fixes); pending implementation plan

## Context

We are building a long-lived, always-listening voice assistant. The P0 goal is to
**prove out the loop**, not the intelligence. The whole app is one persistent
Python process running a continuous `listen → transcribe → refine → speak → repeat`
loop. The session is "long-lived" because the loop never exits, the refine model
stays warm, and a bounded conversation history persists in memory.

The v0 task is deliberately trivial — "repeat what I said, but refined" (clean up
grammar, drop filler words and false starts). This keeps all focus on loop
mechanics and latency. Swapping the refine prompt for something smarter later is a
one-line change.

Success criteria (the bar): the loop **feels good** to talk to across many turns,
and round-trip **latency is acceptable**. The P0 makes latency observable
(per-stage timing that isolates responsiveness from playback) and robust (errors
never break the loop).

## Decisions (locked from brainstorming + code review)

| Area | Decision |
|------|----------|
| STT | `mlx-whisper` (Apple Silicon native, no torch), **pre-warmed at startup** |
| VAD / turn-taking | Silero `silero-vad` (`VADIterator`), hands-free, with **pre-roll** to avoid onset clipping |
| Refine | oMLX server on `:8002` (OpenAI-compatible), model `gemma-4-e4b-it-8bit`, via `openai` client, **fed a bounded history window** |
| History | `collections.deque(maxlen=~40 msgs / ~20 turns)` — **passed into refine** (used, not dead; bounded so no leak) |
| TTS | macOS `say`, wrapped in an interruptible player |
| Barge-in | **Deferred** (structural: needs a concurrent consumer thread + AEC) — P0 only guarantees an interruptible player |
| Runtime | Resilient (frame-intake guard **and** per-turn guard) + latency timing that isolates time-to-reply |
| Code structure | Single `assistant.py` (~170 lines): focused functions + one small `TtsPlayer` class |

## Architecture

One process, one `main()` loop.

- A `sounddevice.InputStream` callback (audio thread) pushes float32 frames
  (16 kHz mono) onto a `queue.Queue`, continuously.
- The main loop pulls frames → **validates each to exactly 512 samples** (pad a
  short final block with zeros; skip a malformed oversized block) → feeds Silero
  `VADIterator`.
- A small **pre-roll ring buffer** (~250 ms of recent frames) is always retained.
  `VADIterator` reports `start` *after* speech onset, so on `start` the utterance
  buffer is seeded with the pre-roll — otherwise the leading phoneme(s) are clipped
  and transcription degrades. Frames accumulate until the `end` event.
- On `end`, the turn pipeline runs: `transcribe → refine → speak`.

**Startup warm-up (both models):** the process makes one throwaway oMLX call *and*
one `transcribe(np.zeros(16000, dtype=np.float32))`. The Whisper call forces weight
load + MLX graph compile up front. With both pre-warmed, turn-1 latency ≈ later
turns (without this, the first `stt` would spike and contradict the timing
readout).

### Barge-in is deferred, and it is NOT a one-flag change

While TTS plays, the single-consumer main loop is blocked inside
`TtsPlayer.speak()` and the capture callback drops frames (half-duplex echo guard,
so the assistant never transcribes its own voice). P0 guarantees only that the
player is **interruptible** — `TtsPlayer.stop()` kills playback.

Live barge-in requires two structural additions, both deferred:
1. A **concurrent consumer thread** that keeps running VAD on incoming audio *while
   TTS plays* and calls `stop()` on detected speech — the current blocking
   single-consumer loop is the part that would be restructured.
2. **Acoustic echo cancellation** (WebRTC APM / speexdsp) so the mic doesn't
   trigger on the Mac's own output.

Continuous-capable capture is necessary but not sufficient. The interruptible
player is the one primitive P0 delivers toward that future.

## Components (one file, small parts)

- **Config / env:** `OMLX_BASE_URL`, `OMLX_API_KEY`, `OMLX_MODEL`, `WHISPER_REPO`,
  VAD params (`min_silence_ms`, speech threshold), `PREROLL_MS`, `HISTORY_MAXLEN`,
  `SYSTEM_PROMPT`.
- **`transcribe(audio: np.ndarray) -> str`** — `mlx_whisper.transcribe(...)`.
- **`refine(text: str, history: deque) -> str`** — appends the user turn to
  `history`, calls oMLX with `[system] + list(history)` (the bounded window — so
  refine is context-aware but the prompt and memory stay capped), appends the
  assistant reply to `history`, returns it. `deque(maxlen=...)` auto-drops the
  oldest, so this is the use-it-and-bound-it resolution. Note: feeding history can
  let refinement drift on prior turns; the tight window limits this.
- **`class TtsPlayer`** — `.speak(text)` launches `subprocess.Popen(["say", ...])`
  and blocks until it finishes or is interrupted; `.stop()` terminates the
  subprocess. The interruptible primitive.
- **`main()`** — opens the capture stream; runs the frame/VAD loop; on `end`, runs
  the turn pipeline with timing + error guards.

## Data flow

```
mic → InputStream callback → frame queue
   → validate frame to 512 samples
   → maintain pre-roll ring buffer (~250ms)
   → VADIterator
      • on 'start': seed utterance buffer with pre-roll
      • accumulate frames
      • on 'end': utterance = concat(pre-roll + buffered frames)
   → transcribe() → refine(history) → TtsPlayer.speak()
```

## Error handling (two guard scopes)

- **Frame-intake + VAD-feed guard:** `vad(chunk)` runs per-frame, *before* any turn
  boundary, and silero-vad requires exactly 512-sample windows — an xrun or odd
  final block would otherwise raise here and escape a turn-scoped guard, killing the
  loop. So frames are size-validated/padded (above) **and** this path has its own
  try/except that logs and continues.
- **Per-turn guard:** `transcribe → refine → speak` is wrapped in try/except: on
  failure, log `[error] <stage>: <msg>`, reset VAD state, clear the frame queue,
  keep listening. Hiccups (oMLX timeout, empty audio) never break the loop.
- **Startup:** the oMLX warm-up call doubles as a reachability check; if unreachable,
  exit early with a clear message (`oMLX not reachable at <url> — is it running?`).

## Latency instrumentation (isolates responsiveness from playback)

The metric that determines whether it "feels good" is **end-of-speech → assistant
starts speaking**, never playback duration (`say` blocks for the full 2–4 s
utterance, so timing the `speak()` call measures audio *length*, not latency).

Brackets: `end` event → STT done → refine done → TTS **start**. Per turn, print:

```
⏱ endpoint ~700ms · stt 240ms · refine 180ms · reply-start +430ms
```

- **endpoint** — the `min_silence_ms` pause-tax the user must produce before VAD
  fires `end`. This is the dominant *felt* latency and the first knob to tune, so it
  is surfaced explicitly (not hidden).
- **reply-start** — total end-of-speech → TTS start (= stt + refine + overhead). The
  true responsiveness number.
- Playback duration is intentionally **not** reported as latency.

## Testing strategy

- **Headless `--once "raw text"` entrypoint:** runs `refine()` (and optionally
  `speak`) with no microphone. Makes the refine logic + oMLX wiring + history
  windowing deterministically testable and TDD-able before any audio work. (Live
  oMLX already returns clean output for a messy test string — verified via curl.)
- **VAD segmentation + pre-roll unit:** the buffer logic is pure given a sequence of
  VAD events; feed synthetic frame/event sequences and assert it emits exactly one
  utterance per speech span *with the pre-roll prepended* (assert leading frames are
  present).
- **Frame-validation unit:** feed a short/oversized block and assert it is
  padded/skipped rather than raising.
- **Manual end-to-end:** run `assistant.py`, speak *"um so like I think the, the
  meeting is uh tomorrow"*, then pause. Expect console `heard:`/`refined:` lines,
  the Mac speaking the cleaned sentence, and a timing line. With both models
  pre-warmed, turn-1 `reply-start` ≈ later turns (the warm-up's purpose).
- **Interruptible-player check:** confirm `TtsPlayer.stop()` kills playback when
  called directly. This proves the kill *primitive* — it does **not** exercise live
  barge-in, which is deferred.

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

Live barge-in (concurrent consumer thread + AEC), streaming STT/LLM, nicer TTS
(Piper/Kokoro), menu-bar / Tauri shell, and using gemma-4's native audio-input
capability to replace mlx-whisper.
