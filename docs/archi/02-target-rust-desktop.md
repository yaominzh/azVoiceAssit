# Target Architecture — Native Rust desktop app (next phase)

**Decisions so far (brainstorm in progress):** full Rust rewrite of the pipeline;
**egui** native UI; **oMLX reused over HTTP** (the LLM is NOT reimplemented);
**whisper-rs** (whisper.cpp, Metal) for STT; **cpal** for capture; **Silero VAD via
ONNX (`ort`)**; **TTS pluggable** — the uplevel point (candidates below). **RAG is a
later phase** (shown dotted).

## Architecture / components

```mermaid
flowchart TB
  subgraph app["Rust desktop app — single binary"]
    subgraph ui["egui UI thread"]
      yy["taichi (state)"]
      tr["transcript"]
      ctl["controls: mic / clear / stop"]
    end
    subgraph core["audio + inference thread(s)"]
      mic["cpal capture"] --> vad["Silero VAD<br/>(ort / ONNX)"]
      vad --> seg["segmenter<br/>(pre-roll ring)"]
      seg --> stt["whisper-rs<br/>(whisper.cpp, Metal)"]
      stt --> refine["refine client"]
      refine --> tts["TTS client"]
    end
    core <-->|"channels / shared state"| ui
  end
  refine <-->|"HTTP /v1/chat"| omlx["oMLX service :8002<br/>gemma (local)"]
  tts <-->|"HTTP / CLI"| mlxtts["Qwen3-TTS<br/>MLX-Audio service (local)"]
  tts --> spk["speaker"]

  subgraph future["Phase 2 — RAG (deferred)"]
    kb["knowledge base<br/>speaking book + domain corpus"]
    emb["bge-m3 embeddings"]
    vstore["vector store"]
  end
  kb -. "ingest (MinerU)" .-> emb
  emb -. "index" .-> vstore
  refine -. "retrieve context" .-> vstore
```

## Dataflow — one turn

```mermaid
flowchart LR
  A["speech"] --> B["cpal frames"]
  B --> C["Silero VAD<br/>end + pre-roll"]
  C --> D["whisper-rs → text"]
  D --> E["refine → oMLX → text"]
  E --> F["TTS → Qwen3-TTS (MLX) → audio"]
  F --> S["speaker"]
  E -. "state / turn" .-> UI["egui: taichi + transcript"]
  D -. "Phase 2: retrieve" .-> R["RAG context"]
  R -. "augment prompt" .-> E
```

## Component reuse vs rewrite

| Stage | Current (Python) | Target (Rust) | Status |
|-------|------------------|---------------|--------|
| Audio capture | sounddevice | **cpal** | rewrite |
| VAD | Silero (torch) | **Silero via `ort`/ONNX** | rewrite |
| Segmenter/pre-roll | pure Python | pure Rust | rewrite (port logic) |
| STT | mlx-whisper | **whisper-rs (whisper.cpp)** | rewrite |
| LLM / refine | oMLX HTTP client | **reqwest → oMLX** | **reuse service** |
| TTS | macOS `say` | **Qwen3-TTS via MLX-Audio** (local service, like oMLX) | **uplevel (new service)** |
| UI | web + SSE | **egui (in-process)** | rewrite (no SSE needed) |
| RAG | — | bge-m3 + vector store | **Phase 2** |

## TTS — decision: Qwen3-TTS via MLX-Audio

Chosen for best quality + voice cloning, fully local on Apple Silicon. Runs as a
**local model service** (Python/MLX-Audio) the Rust app calls over HTTP/CLI —
**architecturally a twin of oMLX**. So the runtime is: Rust app + oMLX (LLM) +
MLX-Audio (TTS), all local. (Rejected: Piper/Kokoro in-process ONNX — simpler/single-
binary but lower quality, no cloning; macOS `say` — robotic, current baseline.)
