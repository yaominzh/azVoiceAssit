# Voice Assistant Front-End — Design Spec

**Date:** 2026-05-29
**Status:** Approved (design, rev 2 — incorporates third-party review); pending implementation plan
**Builds on:** `2026-05-29-voice-assistant-p0-design.md` (the CLI loop in `assistant.py`)

## Context

The P0 voice loop (`listen → transcribe → refine → speak`) works but is terminal-only:
it prints `heard:`/`refined:`/timing to stdout and speaks via `say`. We want a small
graphical front-end that makes the assistant's state legible at a glance and adds basic
controls.

The UI shows three things (the user's brief):
1. A **taichi ☯** symbol whose color/animation reflects the current state.
2. The **transcript** — what was heard and the refined text.
3. The **status** — one of: listening, thinking, speaking (plus a muted indicator).

Plus a small **control panel**: a mic (listening) toggle, clear-transcript, and
stop-speaking.

Guiding principle (same as P0): start small and prove the concept. A browser page driven
by the existing process beats a desktop shell for v0 — the desktop wrapper (menu-bar /
Tauri) is deferred until the concept is proven.

## Decisions (locked from brainstorming + third-party review)

| Area | Decision |
|------|----------|
| Host | **In-process** web server (background daemon thread) — required because controls touch live loop objects |
| Transport | **SSE** (server→browser) for state/transcript + **POST** (browser→loop) for controls |
| Web stack | Python **stdlib `http.server`** (ThreadingHTTPServer) — **zero new dependencies** |
| Decoupling seam | A **`UiBus`** the loop emits to; `NullBus` (prints) when UI off → CLI + tests unchanged |
| Launch | **Opt-in `--ui` flag**; plain `python assistant.py` stays pure CLI; `--once` ignores `--ui` |
| Port | Default **8765**, overridable via `UI_PORT` env. **Must not** collide with oMLX's `:8002` |
| Layout | **Zen Center** — ☯ + status on top, transcript scrolls below, control bar pinned at bottom |
| Theme | **Dark** |
| State → ☯ | listening = blue/pulse · thinking = purple/spin · speaking = green/glow · muted = dimmed grey |
| Controls | mic toggle (= mute mic = pause listening), clear transcript (+ reset history), stop-speaking |

## Architecture

One process. The voice loop runs as today on the main thread; a **daemon thread** runs the
HTTP server. They share a single **`UiBus`** instance.

- The loop emits through the bus only: `bus.set_state("listening"|"thinking"|"speaking")`
  and `bus.push_turn(heard, refined, timing)`. It reads `bus.listening_enabled`.
- **Fail-loud startup (review #1):** when `--ui` is passed, the socket is **bound on the
  main thread** before the loop starts (`ThreadingHTTPServer((host, port), handler)`); a
  bind failure (port in use) prints a clear message and exits — mirroring the oMLX
  reachability check — instead of dying silently in the daemon. Only after a successful
  bind is `serve_forever()` handed to the daemon thread. The resolved `http://localhost:<port>`
  URL is **printed on startup** (review #12).
- When `--ui` is absent, the loop uses a **`NullBus`** so the CLI path and existing tests are
  unaffected.

This is the one structural change to `assistant.py`: replace the inline `print`/state
points with bus calls, and gate the audio callback on `bus.listening_enabled`.

## The bus (UiBus / NullBus)

`UiBus` responsibilities:
- Holds **`current_state`** (str), the list of subscriber queues (lock-protected), the
  `listening_enabled` **plain bool**, and references to the `TtsPlayer` (for stop) and the
  refine **history deque** (for clear).
- `set_state(s)`: updates `current_state` and broadcasts a `state` event.
- `push_turn(heard, refined, timing)`: broadcasts a `turn` event.
- `subscribe()`: registers a new client queue **and immediately enqueues the current
  `state`** so a browser connecting mid-`speaking`/`muted` renders correctly (review #2).
- `clear()`: empties the history deque and broadcasts a `clear` event.
- Each subscriber queue is a bounded `deque(maxlen=256)` with **drop-oldest** on overflow.
  Drop-oldest always retains the newest event, so the latest `state` is delivered
  (last-write-wins is satisfied without coalescing — review #3). Per-turn flooding is not a
  realistic single-user concern.

`NullBus` (CLI mode) — pinned so CLI output/tests don't drift (review #4):
- `push_turn(heard, refined, timing)` prints the **three existing lines**: `  heard:   …`,
  `  refined: …`, and `format_timing(...)`.
- `set_state` and `clear` are **no-ops**.
- `listening_enabled` is always **`True`**.

**Audio-thread safety (review #5):** the existing local `speaking` echo-guard bool **stays**;
`listening_enabled` is **additive**. The callback does
`if not speaking and bus.listening_enabled: audio_q.put(...)`. The audio thread reads only
these bools — it never touches the lock-protected subscriber queues — to stay realtime-safe.

## File structure

- `assistant.py` — the loop, refactored to emit via the bus; adds `--ui`/`UI_PORT` handling.
- `ui_server.py` — **new**: `UiBus`, `NullBus`, `sse_format`, and the HTTP request handler
  (static serving + `/events` SSE + `/control/*` POST). One focused responsibility.
- `static/index.html`, `static/app.js`, `static/style.css` — **new**: the Zen Center dark
  page. `app.js` opens an `EventSource` to `/events`, updates the ☯ class + status word,
  appends transcript bubbles, and POSTs control actions.

## Data flow

```
voice loop ──► UiBus.set_state / push_turn ──► per-client queues ──► SSE /events ──► browser
browser control click ──► POST /control/{mic|clear|stop} ──► UiBus ──► loop reacts
```

SSE framing is produced by a pure **`sse_format(event: dict) -> str`** → `"data: {json}\n\n"`
(testable; review #13). Events:
- `{"type":"state","value":"listening|thinking|speaking|muted"}`
- `{"type":"turn","heard":"…","refined":"…","timing":{"endpoint":…,"stt":…,"refine":…,"reply_start":…}}`
  — includes `endpoint` for parity with the CLI readout (review #9).
- `{"type":"clear"}`

## Controls (browser → POST → loop)

- **`POST /control/mic`** — toggles `bus.listening_enabled`; the audio callback drops frames
  when disabled.
  **Mute precedence (review #6):** `muted` is shown **only when the loop is idle/listening**.
  Toggling mic off mid-`thinking`/`speaking` does not interrupt the in-flight turn; the ☯
  shows `muted` once the loop returns to the listening state.
- **`POST /control/clear`** — `bus.clear()`: empties the on-screen transcript **and resets the
  refine history deque** (fresh context), broadcasts `{"type":"clear"}`.
  **Benign race (review #7):** `history.clear()` runs on the server thread while the main
  thread may be inside `refine()` (`history.append`). deque ops are individually GIL-atomic,
  so this never crashes; worst case a lone assistant message remains, which self-corrects on
  the next turn.
- **`POST /control/stop`** — calls `TtsPlayer.stop()` (the interruptible primitive from P0).

All controls return `204` on success, `400` on an unknown path.

## Static file serving

Serve **only the three whitelisted files** (`index.html`, `app.js`, `style.css`); any other
path → 404. This avoids hand-rolled `..` path-traversal risk (review #8) without depending on
`SimpleHTTPRequestHandler` filesystem mapping.

## Error handling

The UI is non-critical and must never take down the voice loop:
- Server runs as a **daemon thread**; an exception there does not stop the loop (and bind
  failures are caught on the main thread at startup — see Architecture).
- A disconnected browser raises `BrokenPipeError` on write → that subscriber queue is pruned;
  other clients and the loop continue.
- Bounded per-client `deque(maxlen=256)`, drop-oldest, so a slow/dead client never blocks the
  loop's bus calls.
- Control POSTs validate the path; unknown → 400.
- **Known v0 limitation (review #11):** `ThreadingHTTPServer` parks one worker thread per open
  `/events` connection (dead ones reaped on next write). Acceptable for single-user v0.

## Testing

- **`UiBus` unit tests** (no network): `set_state`/`push_turn` enqueue the right JSON to all
  subscribers; `subscribe` immediately enqueues the current state; `clear` broadcasts and
  resets the provided history deque; `listening_enabled` toggle; overflow drops oldest while
  retaining the latest state event; subscriber prune.
- **`NullBus` test:** `push_turn` prints the three expected lines; `set_state`/`clear` are
  no-ops; `listening_enabled is True` — guaranteeing unchanged CLI output.
- **`sse_format` test:** asserts exact `data: {json}\n\n` wire framing (review #13).
- **Control-handler tests:** drive handlers with a fake bus + fake player; `/control/mic`
  flips the flag, `/control/stop` calls `player.stop()`, `/control/clear` empties the history
  deque and broadcasts `clear`, unknown path → 400.
- **Manual e2e:** `python assistant.py --ui`, open the printed URL, speak — ☯ cycles
  listening→thinking→speaking, transcript shows heard→refined with timing; exercise all three
  controls (mic toggle stops/starts capture and shows muted when idle, clear empties
  transcript + context, stop cuts off playback mid-utterance). Also verify a browser opened
  mid-`speaking` renders the correct state immediately, and that launching `--ui` on an
  in-use port fails with a clear message.

## Environment / setup

No new Python dependencies (stdlib only). Page served at `http://localhost:8765` (default;
`UI_PORT` overrides; must avoid `:8002`). Same venv as P0. oMLX still required for refine.

## Out of scope (deferred)

Menu-bar / Tauri desktop shell, multi-client auth, transcript persistence/replay across
restarts, websockets (SSE + POST is sufficient), and live barge-in (still gated on the
AEC/concurrent-consumer work from the P0 spec).
