# Historical: Python loop + browser UI (P0 baseline)

> **Status:** Superseded. The Rust desktop app (see `02-shipped-rust-desktop.md`) is now
> the primary implementation. This doc is preserved as the P0 baseline reference.

The as-built system: one Python process (`assistant.py`) running the
`listen → transcribe → refine → speak` loop, with an optional in-process web UI
(`--ui`) served over SSE. TTS today is the macOS **`say`** command (system voices —
the part slated for upleveling).

## Architecture / components

```mermaid
flowchart TB
  subgraph proc["assistant.py — one Python process"]
    mic["mic<br/>sounddevice InputStream"] --> q["audio queue"]
    q --> vad["Silero VAD<br/>(VADIterator)"]
    vad --> seg["Segmenter<br/>(pre-roll ring)"]
    seg --> stt["STT<br/>mlx-whisper"]
    stt --> refine["refine()<br/>+ bounded history"]
    refine --> tts["TtsPlayer<br/>macOS say"]
    vad -. state .-> bus["UiBus"]
    refine -. turn .-> bus
    tts -. state .-> bus
  end
  refine <-->|"HTTP /v1/chat"| omlx["oMLX server :8002<br/>gemma-4-e4b-it-8bit"]
  bus -->|"SSE /events"| browser["Browser UI<br/>static/ (taichi, transcript)"]
  browser -->|"POST /control/*"| bus
  tts --> spk["speaker"]
```

## Dataflow — one turn

```mermaid
flowchart LR
  A["speech"] --> B["VAD 'end'<br/>+ pre-roll"]
  B --> C["utterance<br/>16 kHz f32"]
  C --> D["whisper → text"]
  D --> E["refine → oMLX<br/>→ refined text"]
  E --> F["say → audio"]
  F --> S["speaker"]
  E -. "turn event (SSE)" .-> G["browser:<br/>heard + refined bubbles"]
```

## Notes
- **TTS = macOS `say`** (system voices). Not neural; the uplevel target.
- oMLX is a separate long-lived HTTP service (the warm LLM).
- The browser UI is optional; without `--ui` the loop is pure CLI.
