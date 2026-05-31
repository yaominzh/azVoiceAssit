# Rust Desktop App — P0 Design Spec

**Date:** 2026-05-30
**Status:** Approved (design); pending implementation plan
**Builds on:** the shipped Python loop + browser UI (`assistant.py`, `ui_server.py`, `static/`).
**Diagrams:** `docs/archi/02-target-rust-desktop.md`

## Context

Next phase, part 1 of 2 (the other is **RAG knowledge grounding**, deferred to its own
later cycle). We are re-platforming the voice assistant as a **native Rust desktop app**
— the user wants to "live in it daily" rather than a browser tab.

Decision (from brainstorming): a **full Rust rewrite of the pipeline + UI**, keeping the
two heavy ML models as **local services called over HTTP** (the pattern already proven
with oMLX). This P0 re-proves the `listen → transcribe → refine → speak` loop in Rust with
**parity** to the shipped app — same discipline as the original P0: prove the loop in the
new stack before adding desktop polish.

## Decisions (locked from brainstorming)

| Area | Decision |
|------|----------|
| Platform | Full **Rust** rewrite of pipeline + UI (single app) |
| UI | **egui** (`eframe`), porting the approved Zen Center dark layout |
| Audio capture | **cpal** |
| VAD | **Silero via `ort`/ONNX** (+`ndarray`) |
| Segmenter | port the pre-roll ring logic from Python to Rust |
| STT | **whisper-rs** (whisper.cpp, Metal) |
| Refine / LLM | **reuse oMLX** over HTTP (`reqwest`) — NOT reimplemented |
| TTS | **Qwen3-TTS via MLX-Audio**, a local persistent service (HTTP), played with `rodio` |
| In-process wiring | channels (`crossbeam-channel`) + shared atomics (no SSE — it's native) |
| P0 scope | minimal **loop parity**; desktop-presence + RAG deferred |

## Architecture

One Rust app, two threads, two local model services.

- **UI thread** — `eframe`/egui. Renders the ☯ (state-driven color/animation), the
  heard→refined transcript, and mic/clear/stop controls. Receives `state`/`turn`/`clear`
  events from the worker over a channel and repaints.
- **Worker thread** — owns the pipeline: pulls audio frames, runs VAD → segmenter → STT →
  refine → TTS, and emits UI events. Receives control commands from the UI over a channel.
- **cpal capture callback** (its own thread) — pushes frames to the worker; gated by a
  shared `AtomicBool listening_enabled` (the additive echo-guard; also suppressed while TTS
  plays). The callback touches only atomics + the lock-free frame channel — realtime-safe.
- **oMLX service** `:8002` — refine (reused, unchanged).
- **Qwen3-TTS service** — a small **persistent** local MLX-Audio HTTP server: `POST /tts
  {text}` → wav bytes. Persistent so the model loads once, not per utterance (mirrors why
  oMLX is long-lived). Rust plays the returned wav with `rodio`.

The `UiBus` concept from the web version collapses to plain channels + atomics here — no
HTTP/SSE inside the app, since UI and pipeline share a process.

## Components / crates

- `eframe` + `egui` — native window + UI.
- `cpal` — microphone capture (16 kHz mono f32).
- `ort` + `ndarray` — Silero VAD ONNX inference.
- `whisper-rs` — STT (ships/loads a GGML whisper model, e.g. `ggml-base.en.bin`).
- `reqwest` + `serde`/`serde_json` — oMLX chat client and TTS client.
- `rodio` — play the TTS wav.
- `crossbeam-channel` — UI ↔ worker messaging.

## Data flow (one turn)

```
cpal frames → [AtomicBool gate] → frame channel → worker:
  Silero VAD (end + pre-roll) → utterance
  → whisper-rs → text
  → refine: reqwest → oMLX → refined text   (bounded history window)
  → TTS: reqwest → Qwen3-TTS(MLX) → wav → rodio → speaker
worker → UI channel: {state}, {turn: heard,refined,timing}, {clear}
UI → worker channel: {toggle_mic}, {clear}, {stop}
```

## Behavior parity (with the shipped app)

- **States:** `listening` (blue/pulse) · `thinking` (purple/spin) · `speaking` (green/glow)
  · `muted` (dim grey). Same mapping as the web UI.
- **Pre-roll** ring buffer so onsets aren't clipped (port the `Segmenter` logic + its tests).
- **Bounded history** window fed into refine (port the deque-window behavior).
- **Per-turn timing**: end-of-speech → TTS start (reply-start), not playback duration;
  surface the endpoint (VAD silence) tax.
- **Controls:** mic toggle (= pause listening, shows `muted` when idle), clear (empties
  transcript **and** resets history), stop (interrupts current TTS playback).
- **Echo-guard:** capture suppressed while TTS is playing (atomic flag), additive with the
  mic toggle.

## Error handling

- The two model services are external; if oMLX or the TTS service is unreachable, the app
  shows a clear status (not a crash) and keeps the UI responsive. A startup reachability
  check for both (like the Python `warm_up`) fails loudly with an actionable message.
- Worker pipeline errors are caught per-turn and surfaced; the loop keeps running.
- whisper/VAD model files missing → clear startup error telling the user how to fetch them.

## Testing strategy

Mirror the Python approach — pure logic is unit-tested, I/O is manual:
- `cargo test` units: **segmenter + pre-roll** (synthetic frame/event sequences → one
  utterance with pre-roll prepended), **history-window** (bounded, fed correctly to the
  refine request body), **timing formatting**, **control/state transitions** (toggle_mic
  idle-vs-mid-turn precedence, clear resets history).
- Client request-shaping units: the oMLX chat body and the TTS request are built by pure
  functions, tested without network (inject a fake transport).
- Manual e2e (the "Rust P0 verification"): launch oMLX + the TTS service + the app; speak;
  watch the egui window cycle states, show heard→refined, hear Qwen3-TTS audio; exercise
  mic/clear/stop.

## Environment / setup

- Rust toolchain (`cargo`), macOS Apple Silicon. New `Cargo.toml` with the crates above.
- `whisper-rs` needs a GGML model file downloaded once.
- **oMLX** running on `:8002` (existing). **Qwen3-TTS MLX-Audio service** running locally
  (set up as part of this P0 — a small persistent server loading a Qwen3-TTS model).
- The Python app remains in the repo unchanged; the Rust app is additive (new `rust/` or
  crate dir, TBD in the plan).

## Out of scope (deferred)

Menu-bar / tray icon, global hotkey, always-on-top compact "orb" mode, app packaging
(.app/.dmg, launch-at-login), and **all of RAG** (knowledge grounding — the separate next
sub-project: ingest the speaking book + domain corpus with MinerU, embed with bge-m3,
retrieve into refine).
