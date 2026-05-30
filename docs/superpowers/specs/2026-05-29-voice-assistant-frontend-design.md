# Voice Assistant Front-End — Design Spec

**Date:** 2026-05-29
**Status:** Approved (design); pending implementation plan
**Builds on:** `2026-05-29-voice-assistant-p0-design.md` (the CLI loop in `assistant.py`)

## Context

The P0 voice loop (`listen → transcribe → refine → speak`) works but is terminal-only:
it prints `heard:`/`refined:`/timing to stdout and speaks via `say`. We want a small
graphical front-end that makes the assistant's state legible at a glance and adds basic
controls.

The UI shows three things (the user's brief):
1. A **taichi ☯** symbol whose color/animation reflects the current state.
2. The **transcript** — what was heard and the refined text.
3. The **status** — one of: listening, thinking, speaking.

Plus a small **control panel**: a mic (listening) toggle, clear-transcript, and
stop-speaking.

Guiding principle (same as P0): start small and prove the concept. A browser page driven
by the existing process beats a desktop shell for v0 — the desktop wrapper (menu-bar /
Tauri) is deferred until the concept is proven.

## Decisions (locked from brainstorming)

| Area | Decision |
|------|----------|
| Host | **In-process** web server (background daemon thread in the assistant) — required because controls touch live loop objects |
| Transport | **SSE** (server→browser) for state/transcript + **POST** (browser→loop) for controls |
| Web stack | Python **stdlib `http.server`** (ThreadingHTTPServer) — **zero new dependencies** |
| Decoupling seam | A **`UiBus`** the loop emits to; no-op/print bus when UI is off (CLI + tests unchanged) |
| Launch | **Opt-in `--ui` flag**; plain `python assistant.py` stays pure CLI |
| Layout | **Zen Center** — ☯ + status on top, transcript scrolls below, control bar pinned at bottom |
| Theme | **Dark** |
| State → ☯ | listening = blue/pulse · thinking = purple/spin · speaking = green/glow · mic-off = dimmed grey |
| Controls | mic toggle (= mute mic = pause listening), clear transcript (+ reset history), stop-speaking |

## Architecture

One process. The voice loop runs as today on the main thread; a **daemon thread** runs the
HTTP server. They share a single **`UiBus`** instance.

- The loop emits through the bus only: `bus.set_state("listening"|"thinking"|"speaking")`
  and `bus.push_turn(heard, refined, timing)`. It reads `bus.listening_enabled`.
- The bus holds: the current state, the list of SSE subscriber queues (thread-safe), the
  `listening_enabled` flag, a reference to the `TtsPlayer` (for stop), and a reference to
  the history deque (for clear).
- When `--ui` is absent, the loop uses a **`NullBus`** that just prints (today's behavior),
  so the CLI path and all existing tests are unaffected.

This is the one structural change to `assistant.py`: replace the inline `print`/state
points with bus calls, and gate the audio callback on `bus.listening_enabled` (mirroring
the existing `speaking` echo-guard flag).

## File structure

- `assistant.py` — the loop, refactored to emit via the bus; adds `--ui` handling. Stays
  the home of the audio/STT/refine/TTS logic.
- `ui_server.py` — **new**: `UiBus`, `NullBus`, and the HTTP request handler (static file
  serving + `/events` SSE + `/control/*` POST). One focused file with one responsibility.
- `static/index.html`, `static/app.js`, `static/style.css` — **new**: the Zen Center dark
  page. `app.js` opens an `EventSource` to `/events`, updates the ☯ class + status word,
  appends transcript bubbles, and POSTs control actions.

## Data flow

```
voice loop ──► UiBus.set_state / push_turn ──► per-client queues ──► SSE /events ──► browser
browser control click ──► POST /control/{mic|clear|stop} ──► UiBus ──► loop reacts
```

SSE events (one JSON object per `data:` line):
- `{"type":"state","value":"listening|thinking|speaking|muted"}`
- `{"type":"turn","heard":"…","refined":"…","timing":{"stt":…,"refine":…,"reply_start":…}}`
- `{"type":"clear"}`

## Controls (browser → POST → loop)

- **`POST /control/mic`** — toggles `bus.listening_enabled`. The audio callback drops frames
  when disabled (same mechanism as the `speaking` echo-guard). Bus emits `state:"muted"`
  (dimmed ☯) when off; returns to `listening` when on.
- **`POST /control/clear`** — clears the on-screen transcript **and resets the refine
  history deque** (fresh conversation context). Bus broadcasts `{"type":"clear"}`.
- **`POST /control/stop`** — calls `TtsPlayer.stop()` (the interruptible primitive from P0).

All controls return `204` on success, `400` on an unknown path.

## Error handling

The UI is non-critical and must never take down the voice loop:
- Server runs as a **daemon thread**; an exception there does not stop the loop.
- A disconnected browser raises `BrokenPipeError` on write → that subscriber queue is
  pruned; other clients and the loop continue.
- The loop's bus calls are cheap and must not block on slow/absent clients (bounded
  per-client queue; drop oldest if a client stalls).
- Control POSTs validate the path; unknown → 400.

## Testing

- **`UiBus` unit tests** (no network): `set_state`/`push_turn` enqueue the right JSON to all
  subscribers; `subscribe`/unsubscribe; `clear` broadcasts and resets the provided history
  deque; `listening_enabled` toggle; a stalled subscriber gets the oldest event dropped, not
  the loop blocked.
- **Control-handler tests**: drive the handler functions with a fake bus + fake player;
  assert `/control/mic` flips the flag, `/control/stop` calls `player.stop()`,
  `/control/clear` empties the history deque and broadcasts `clear`, unknown path → 400.
- **`NullBus` test**: confirms `set_state`/`push_turn` are safe no-ops (loop unaffected when
  `--ui` off).
- **Manual e2e**: `python assistant.py --ui`, open the page, speak — ☯ cycles
  listening→thinking→speaking, transcript shows heard→refined, timing appears; exercise all
  three controls (mic toggle stops/starts capture, clear empties transcript + context, stop
  cuts off playback mid-utterance).

## Environment / setup

No new Python dependencies (stdlib only). The page is served at `http://localhost:<port>`
(default port a constant, overridable by env, e.g. `UI_PORT`). Same venv as P0. oMLX still
required for refine.

## Out of scope (deferred)

Menu-bar / Tauri desktop shell, multi-client auth, transcript persistence/replay across
restarts, websockets (SSE + POST is sufficient), and live barge-in (still gated on the
AEC/concurrent-consumer work from the P0 spec).
