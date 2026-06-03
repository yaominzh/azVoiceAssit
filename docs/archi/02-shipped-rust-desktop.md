# Shipped Architecture — Rust Desktop App (main branch, as of 2026-06-02)

**Status:** Shipped and verified. PRs #1 (browser UI), #2 (AEC Phase 1), #3 (AEC Phase 2
/ barge-in) all merged to main. 41 tests pass. The egui UI is functional but visually
tool-like; the Tauri GUI upgrade is in progress on `feat/gui-enhancement`.

## Architecture / components

```mermaid
flowchart TB
  subgraph app["Rust desktop app — single binary (egui)"]
    subgraph ui["egui UI thread"]
      yy["☯ taichi (state + animation)"]
      tr["transcript (heard + refined + timestamp)"]
      ctl["controls: mic / stop / clear / ⚙"]
      set["settings panel (in-window)"]
    end
    subgraph capture["capture path"]
      cpal["cpal capture\n(default device, native rate)"] --> resamp["downsample\n(linear interp → 16kHz)"]
      resamp --> aec_proc["AEC processing thread\n(Speex via aec-rs)"]
    end
    subgraph pipeline["worker thread"]
      vad["Silero VAD\n(ort/ONNX, 576-sample context)"]
      seg["Segmenter\n(pre-roll ring, onset fix)"]
      stt["whisper-rs\n(ggml-base.en, Metal)"]
      refine["refine\n(history window, 0=stateless)"]
      tts_client["TTS client\n(rodio playback)"]
      barge["barge-in\n(per-gen stop flag,\nclean_rms threshold)"]
    end
    aec_proc --> vad
    vad --> seg --> stt --> refine --> tts_client
    barge -.->|"stop active TTS"| tts_client
    ui <-->|"crossbeam channels\n+ AtomicBool"| pipeline
    pipeline -->|"rx_ui events"| ui
    ui -->|"ControlMsg"| pipeline
  end

  refine <-->|"HTTP /v1/chat\nBearer rdaz1234"| omlx["oMLX service :8002\ngemma-4-e4b-it-8bit"]
  tts_client <-->|"HTTP POST /tts"| qwen3["Qwen3-TTS service :8123\nMLX-Audio (Python)\n'friendly colleague' voice"]
  tts_client --> spk["speaker\n(rodio)"]

  subgraph aec_layer["AEC layer (Phase 1 shadow + Phase 2 barge-in)"]
    echo_cancel["EchoCancel\n(aec-rs / Speex)"]
    ref_feed["TTS PCM reference\n24kHz → 16kHz resampled"]
  end
  ref_feed --> echo_cancel
  echo_cancel --> vad

  subgraph settings_store["Settings (~/.config/azva/settings.json)"]
    sp["system_prompt"]
    sil["silence_ms (300–5000)"]
    thr["speech_threshold (0.1–0.9)"]
    hist["history_turns (0=stateless)"]
  end
  settings_store -.-> pipeline
```

## Dataflow — one turn (with AEC)

```mermaid
flowchart LR
  A["mic input\n(native rate)"] --> B["downsample → 16kHz"]
  B --> C["AEC processing\n(echo cancelled)"]
  C --> D["Silero VAD\n(576-sample context)"]
  D --> E["Segmenter\n(pre-roll)"]
  E --> F["whisper-rs → text"]
  F --> G["refine → oMLX\n(bounded history window)"]
  G --> H["Qwen3-TTS :8123 → WAV"]
  H --> I["rodio → speaker"]
  H -. "reference PCM" .-> C
  G -. "state/turn events" .-> UI["egui UI\n(☯ + transcript + timing)"]
  G -. "barge-in trigger\n(VadEvent::Start +\nclean_rms > 0.02)" .-> STOP["stop active TTS"]
```

## Key design decisions (shipped)

| Component | Decision | Rationale |
|-----------|----------|-----------|
| VAD | Silero ONNX with **64-sample context** prepended per frame | Without it, prob ≈ 0.001 on real speech (discovered in testing) |
| Capture rate | Device native → resample (linear interp) to 16kHz | `BufferSize::Fixed(512)` rejected by macOS CoreAudio |
| AEC | Phase 1 shadow mode (logs only) + Phase 2 barge-in | Speex AEC; `speaking` gate removed; per-gen stop flags |
| Barge-in | `VadEvent::Start` + `clean_rms > 0.02` → stop active TTS | Threshold prevents false triggers from AEC echo leakage |
| History | `deque(maxlen=40)`, **0 = stateless by default** | Stateless avoids refine slowdown after many turns |
| TTS voice | `"a clear natural male voice, calm, mid-range pitch"` | Spike-validated; Qwen3-TTS MLX-Audio service |
| Settings | `~/.config/azva/settings.json`, runtime-applied via `ControlMsg::SettingsChanged` | No restart required |

## What is NOT yet shipped (in progress)

- **Tauri GUI** (`feat/gui-enhancement`) — Deep Blue Frost transparent floating window replacing egui
- **RAG knowledge grounding** — deferred, separate spec
