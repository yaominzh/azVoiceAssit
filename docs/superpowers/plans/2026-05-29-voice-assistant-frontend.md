# Voice Assistant Front-End Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A dark "Zen Center" browser UI for the existing voice loop — a ☯ that reflects state, a heard→refined transcript, and three controls (mic toggle, clear, stop-speaking).

**Architecture:** In-process stdlib HTTP server (daemon thread) streams state/transcript over SSE and accepts control POSTs. A `UiBus` seam decouples the loop from the server; a `NullBus` preserves the exact CLI behavior when `--ui` is off. Zero new dependencies.

**Tech Stack:** Python stdlib (`http.server`, `threading`, `json`), vanilla HTML/CSS/JS (`EventSource`), pytest.

**Spec:** `docs/superpowers/specs/2026-05-29-voice-assistant-frontend-design.md`

**Reminder:** Commit locally only. Do NOT `git push` (org guardrail; the user pushes manually). Branch is `feat/frontend-ui`. Use `.venv/bin/pytest` / `.venv/bin/python` (shell `source` doesn't persist across Bash calls).

---

## File Structure

- Create: `ui_server.py` — `sse_format`, `NullBus`, `_Subscriber`, `UiBus`, `resolve_static`, `control_action`, `make_handler`, `make_server`. One focused file: the UI transport + bus.
- Create: `static/index.html`, `static/app.js`, `static/style.css` — the Zen Center dark page.
- Modify: `assistant.py` — `main()` emits via a bus and starts the server under `--ui`.
- Modify: `tests/test_assistant.py` — append `ui_server` tests (or a new `tests/test_ui_server.py`; this plan appends to the existing file for simplicity).

The audio thread reads only plain bools (`speaking`, `bus.listening_enabled`) — never the lock-protected subscriber list — to stay realtime-safe.

---

## Task 1: `sse_format` (SSE wire framing)

**Files:** Create `ui_server.py`; Test `tests/test_assistant.py`.

- [ ] **Step 1: Write the failing test**

```python
import json
import ui_server


def test_sse_format_frames_event():
    out = ui_server.sse_format({"type": "state", "value": "thinking"})
    assert out == 'data: {"type": "state", "value": "thinking"}\n\n'
    # Round-trips back to the same dict.
    assert json.loads(out[len("data: "):].strip()) == {"type": "state", "value": "thinking"}
```

- [ ] **Step 2: Run it, expect fail**

Run: `.venv/bin/pytest tests/test_assistant.py -k sse_format -v`
Expected: FAIL — `ModuleNotFoundError: No module named 'ui_server'`.

- [ ] **Step 3: Create `ui_server.py` with the imports + function**

```python
"""In-process web UI for the voice assistant: SSE state/transcript out, control POSTs in."""
import json
import os
import threading
from collections import deque
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def sse_format(event):
    """Frame a dict as one Server-Sent Event line block."""
    return f"data: {json.dumps(event)}\n\n"
```

- [ ] **Step 4: Run it, expect pass**

Run: `.venv/bin/pytest tests/test_assistant.py -k sse_format -v` → 1 passed.

- [ ] **Step 5: Commit**

```bash
git add ui_server.py tests/test_assistant.py
git commit -m "feat: ui_server sse_format wire framing"
```

---

## Task 2: `NullBus` (CLI-mode bus, preserves output)

**Files:** Modify `ui_server.py`; Test `tests/test_assistant.py`.

- [ ] **Step 1: Write the failing test**

```python
def test_nullbus_prints_three_lines_and_is_otherwise_noop(capsys):
    timing = {"endpoint": 700, "stt": 100, "refine": 200, "reply_start": 300}
    bus = ui_server.NullBus(lambda t: f"TIMING {t['stt']}/{t['refine']}")
    bus.set_state("thinking")      # no-op
    bus.clear()                    # no-op
    bus.push_turn("raw words", "Clean words.", timing)
    out = capsys.readouterr().out
    assert out == "  heard:   raw words\n  refined: Clean words.\nTIMING 100/200\n"
    assert bus.listening_enabled is True
```

- [ ] **Step 2: Run it, expect fail**

Run: `.venv/bin/pytest tests/test_assistant.py -k nullbus -v`
Expected: FAIL — `AttributeError: module 'ui_server' has no attribute 'NullBus'`.

- [ ] **Step 3: Add `NullBus` to `ui_server.py`**

```python
class NullBus:
    """CLI-mode bus: reproduces today's stdout exactly; everything else is inert.

    The timing formatter is injected (assistant.format_timing) to avoid a circular
    import (assistant imports this module, not the reverse)."""

    listening_enabled = True

    def __init__(self, timing_formatter):
        self._fmt = timing_formatter

    def set_state(self, value):
        pass

    def push_turn(self, heard, refined, timing):
        print(f"  heard:   {heard}")
        print(f"  refined: {refined}")
        print(self._fmt(timing))

    def clear(self):
        pass
```

- [ ] **Step 4: Run it, expect pass**

Run: `.venv/bin/pytest tests/test_assistant.py -k nullbus -v` → 1 passed. Then full suite `.venv/bin/pytest -q` (should be 14 now).

- [ ] **Step 5: Commit**

```bash
git add ui_server.py tests/test_assistant.py
git commit -m "feat: NullBus preserving exact CLI output"
```

---

## Task 3: `_Subscriber` + `UiBus`

**Files:** Modify `ui_server.py`; Test `tests/test_assistant.py`.

- [ ] **Step 1: Write the failing tests**

```python
class _FakePlayer:
    def __init__(self):
        self.stopped = False
    def stop(self):
        self.stopped = True


def test_uibus_subscribe_gets_current_state_snapshot():
    bus = ui_server.UiBus(history=deque(), player=_FakePlayer())
    bus.set_state("speaking")
    sub = bus.subscribe()
    assert sub.drain(timeout=0) == [{"type": "state", "value": "speaking"}]


def test_uibus_set_state_and_push_turn_broadcast():
    bus = ui_server.UiBus(history=deque(), player=_FakePlayer())
    sub = bus.subscribe()
    sub.drain(timeout=0)  # discard the initial snapshot
    bus.set_state("thinking")
    bus.push_turn("h", "r", {"endpoint": 700, "stt": 1, "refine": 2, "reply_start": 3})
    assert sub.drain(timeout=0) == [
        {"type": "state", "value": "thinking"},
        {"type": "turn", "heard": "h", "refined": "r",
         "timing": {"endpoint": 700, "stt": 1, "refine": 2, "reply_start": 3}},
    ]


def test_uibus_clear_resets_history_and_broadcasts():
    hist = deque([{"role": "user", "content": "x"}])
    bus = ui_server.UiBus(history=hist, player=_FakePlayer())
    sub = bus.subscribe(); sub.drain(timeout=0)
    bus.clear()
    assert len(hist) == 0
    assert sub.drain(timeout=0) == [{"type": "clear"}]


def test_uibus_toggle_mic_updates_flag_and_idle_state():
    bus = ui_server.UiBus(history=deque(), player=_FakePlayer())
    sub = bus.subscribe(); sub.drain(timeout=0)   # state is "listening" by default
    bus.toggle_mic()
    assert bus.listening_enabled is False
    assert sub.drain(timeout=0) == [{"type": "state", "value": "muted"}]


def test_uibus_toggle_mic_mid_turn_does_not_change_symbol():
    bus = ui_server.UiBus(history=deque(), player=_FakePlayer())
    bus.set_state("speaking")
    sub = bus.subscribe(); sub.drain(timeout=0)
    bus.toggle_mic()              # mid-turn: flag flips, but no state event
    assert bus.listening_enabled is False
    assert sub.drain(timeout=0) == []


def test_uibus_stop_speaking_calls_player():
    player = _FakePlayer()
    bus = ui_server.UiBus(history=deque(), player=player)
    bus.stop_speaking()
    assert player.stopped is True


def test_uibus_overflow_drops_oldest_keeps_latest_state():
    bus = ui_server.UiBus(history=deque(), player=_FakePlayer())
    sub = ui_server._Subscriber(maxlen=2)
    sub.push({"type": "state", "value": "a"})
    sub.push({"type": "state", "value": "b"})
    sub.push({"type": "state", "value": "c"})   # overflow drops "a"
    assert sub.drain(timeout=0) == [
        {"type": "state", "value": "b"},
        {"type": "state", "value": "c"},
    ]
```

- [ ] **Step 2: Run it, expect fail**

Run: `.venv/bin/pytest tests/test_assistant.py -k uibus -v`
Expected: FAIL — no attribute `UiBus` / `_Subscriber`.

- [ ] **Step 3: Add `_Subscriber` and `UiBus` to `ui_server.py`**

```python
class _Subscriber:
    """One SSE client's bounded, drop-oldest event buffer with a wake condition."""

    def __init__(self, maxlen=256):
        self.queue = deque(maxlen=maxlen)   # append() drops oldest when full
        self.cond = threading.Condition()

    def push(self, event):
        with self.cond:
            self.queue.append(event)
            self.cond.notify()

    def drain(self, timeout=1.0):
        with self.cond:
            if not self.queue:
                self.cond.wait(timeout)
            items = list(self.queue)
            self.queue.clear()
            return items


class UiBus:
    """Shared state between the voice loop and the web server.

    The loop calls set_state/push_turn and reads listening_enabled. The server
    calls subscribe/unsubscribe, toggle_mic, clear, stop_speaking."""

    def __init__(self, history, player):
        self.history = history
        self.player = player
        self.listening_enabled = True
        self.current_state = "listening"
        self._subs = []
        self._lock = threading.Lock()

    def subscribe(self):
        sub = _Subscriber()
        with self._lock:
            self._subs.append(sub)
        sub.push({"type": "state", "value": self.current_state})  # snapshot on connect
        return sub

    def unsubscribe(self, sub):
        with self._lock:
            if sub in self._subs:
                self._subs.remove(sub)

    def _broadcast(self, event):
        with self._lock:
            subs = list(self._subs)
        for s in subs:
            s.push(event)

    def set_state(self, value):
        self.current_state = value
        self._broadcast({"type": "state", "value": value})

    def push_turn(self, heard, refined, timing):
        self._broadcast({"type": "turn", "heard": heard, "refined": refined, "timing": timing})

    def clear(self):
        self.history.clear()
        self._broadcast({"type": "clear"})

    def toggle_mic(self):
        self.listening_enabled = not self.listening_enabled
        # Only repaint the symbol when idle; mid-turn states win (spec #6).
        if self.current_state in ("listening", "muted"):
            self.set_state("listening" if self.listening_enabled else "muted")

    def stop_speaking(self):
        self.player.stop()
```

- [ ] **Step 4: Run it, expect pass**

Run: `.venv/bin/pytest tests/test_assistant.py -k "uibus or subscriber" -v` → all pass. Then `.venv/bin/pytest -q`.

- [ ] **Step 5: Commit**

```bash
git add ui_server.py tests/test_assistant.py
git commit -m "feat: UiBus + bounded drop-oldest subscriber with state snapshot"
```

---

## Task 4: routing helpers (`resolve_static`, `control_action`)

**Files:** Modify `ui_server.py`; Test `tests/test_assistant.py`.

- [ ] **Step 1: Write the failing tests**

```python
def test_resolve_static_whitelist():
    assert ui_server.resolve_static("/") == "index.html"
    assert ui_server.resolve_static("/index.html") == "index.html"
    assert ui_server.resolve_static("/app.js") == "app.js"
    assert ui_server.resolve_static("/style.css") == "style.css"
    # Anything else (incl. traversal attempts) is rejected.
    assert ui_server.resolve_static("/../assistant.py") is None
    assert ui_server.resolve_static("/secret") is None


def test_control_action_routes_to_bus():
    bus = ui_server.UiBus(history=deque([1]), player=_FakePlayer())
    assert ui_server.control_action("/control/mic", bus) == 204
    assert bus.listening_enabled is False
    assert ui_server.control_action("/control/clear", bus) == 204
    assert len(bus.history) == 0
    assert ui_server.control_action("/control/stop", bus) == 204
    assert bus.player.stopped is True
    assert ui_server.control_action("/control/bogus", bus) == 400
```

- [ ] **Step 2: Run it, expect fail**

Run: `.venv/bin/pytest tests/test_assistant.py -k "resolve_static or control_action" -v` → FAIL (no attributes).

- [ ] **Step 3: Add the helpers to `ui_server.py`**

```python
_STATIC = {"/": "index.html", "/index.html": "index.html",
           "/app.js": "app.js", "/style.css": "style.css"}


def resolve_static(path):
    """Map a request path to a whitelisted static filename, or None. No filesystem
    lookup → no `..` traversal risk."""
    return _STATIC.get(path)


def control_action(path, bus):
    """Apply a control POST to the bus. Returns an HTTP status code."""
    if path == "/control/mic":
        bus.toggle_mic(); return 204
    if path == "/control/clear":
        bus.clear(); return 204
    if path == "/control/stop":
        bus.stop_speaking(); return 204
    return 400
```

- [ ] **Step 4: Run it, expect pass**

Run: `.venv/bin/pytest tests/test_assistant.py -k "resolve_static or control_action" -v` → pass. Then `.venv/bin/pytest -q`.

- [ ] **Step 5: Commit**

```bash
git add ui_server.py tests/test_assistant.py
git commit -m "feat: static whitelist + control routing helpers"
```

---

## Task 5: HTTP handler + `make_server` (fail-loud bind)

**Files:** Modify `ui_server.py`; Test `tests/test_assistant.py`.

- [ ] **Step 1: Write the failing test (bind behavior)**

```python
def test_make_server_binds_and_rejects_inuse_port():
    bus = ui_server.UiBus(history=deque(), player=_FakePlayer())
    s1 = ui_server.make_server(bus, host="127.0.0.1", port=0, static_dir=".")
    port = s1.server_address[1]
    try:
        # Binding the same port again must raise (fail-loud, not silent).
        import pytest
        with pytest.raises(OSError):
            ui_server.make_server(bus, host="127.0.0.1", port=port, static_dir=".")
    finally:
        s1.server_close()
```

- [ ] **Step 2: Run it, expect fail**

Run: `.venv/bin/pytest tests/test_assistant.py -k make_server -v` → FAIL (no attribute).

- [ ] **Step 3: Add the handler + `make_server` to `ui_server.py`**

```python
_CONTENT_TYPES = {".html": "text/html", ".js": "text/javascript", ".css": "text/css"}


def make_handler(bus, static_dir):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass  # quiet

        def do_GET(self):
            if self.path == "/events":
                self._serve_events()
                return
            filename = resolve_static(self.path)
            if filename is None:
                self.send_error(404)
                return
            try:
                with open(os.path.join(static_dir, filename), "rb") as f:
                    body = f.read()
            except OSError:
                self.send_error(404)
                return
            ext = os.path.splitext(filename)[1]
            self.send_response(200)
            self.send_header("Content-Type", _CONTENT_TYPES.get(ext, "application/octet-stream"))
            self.end_headers()
            self.wfile.write(body)

        def do_POST(self):
            status = control_action(self.path, bus)
            self.send_response(status)
            self.end_headers()

        def _serve_events(self):
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-cache")
            self.end_headers()
            sub = bus.subscribe()
            try:
                while True:
                    for event in sub.drain(timeout=1.0):
                        self.wfile.write(sse_format(event).encode())
                    self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError, ValueError):
                pass
            finally:
                bus.unsubscribe(sub)

    return Handler


def make_server(bus, host="127.0.0.1", port=8765, static_dir=None):
    """Bind the server (raises OSError on a taken port — caller fails loudly)."""
    if static_dir is None:
        static_dir = os.path.join(os.path.dirname(__file__), "static")
    return ThreadingHTTPServer((host, port), make_handler(bus, static_dir))
```

- [ ] **Step 4: Run it, expect pass**

Run: `.venv/bin/pytest tests/test_assistant.py -k make_server -v` → pass. Then `.venv/bin/pytest -q`.

- [ ] **Step 5: Commit**

```bash
git add ui_server.py tests/test_assistant.py
git commit -m "feat: HTTP handler (static/SSE/control) + fail-loud make_server"
```

---

## Task 6: Front-end static files (Zen Center, dark)

**Files:** Create `static/index.html`, `static/app.js`, `static/style.css`. No unit tests (manual e2e in Task 8).

- [ ] **Step 1: Create `static/index.html`**

```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Voice Assistant</title>
  <link rel="stylesheet" href="/style.css">
</head>
<body>
  <main id="app">
    <div id="top">
      <div id="taichi" class="listening">☯</div>
      <div id="status">listening</div>
    </div>
    <div id="transcript"></div>
    <div id="controls">
      <button id="mic">🎙 Mic</button>
      <button id="clear">Clear</button>
      <button id="stop">Stop</button>
    </div>
  </main>
  <script src="/app.js"></script>
</body>
</html>
```

- [ ] **Step 2: Create `static/style.css`**

```css
:root { color-scheme: dark; }
* { box-sizing: border-box; }
body { margin: 0; background: #0b1020; color: #e5e7eb;
       font-family: -apple-system, system-ui, sans-serif; }
#app { display: flex; flex-direction: column; height: 100vh; }
#top { display: flex; flex-direction: column; align-items: center; gap: 8px; padding: 32px 0 16px; }
#taichi { font-size: 96px; line-height: 1; }
#taichi.listening { color: #3b82f6; animation: pulse 1.8s ease-in-out infinite; }
#taichi.thinking  { color: #a855f7; animation: spin 1.4s linear infinite; }
#taichi.speaking  { color: #22c55e; animation: glow 1.2s ease-in-out infinite; }
#taichi.muted     { color: #64748b; opacity: .5; }
#status { text-transform: uppercase; letter-spacing: .12em; font-size: 13px; font-weight: 600; opacity: .7; }
#transcript { flex: 1; overflow-y: auto; padding: 16px 20px; max-width: 640px;
              width: 100%; margin: 0 auto; }
.bubble { padding: 10px 14px; border-radius: 12px; margin: 8px 0; line-height: 1.4; }
.bubble.heard   { background: rgba(148,163,184,.16); }
.bubble.refined { background: rgba(59,130,246,.20); }
.bubble .who { font-size: 11px; text-transform: uppercase; letter-spacing: .08em; opacity: .55; margin-bottom: 2px; }
#controls { display: flex; gap: 10px; justify-content: center; padding: 16px; border-top: 1px solid rgba(148,163,184,.15); }
#controls button { background: rgba(148,163,184,.14); color: #e5e7eb; border: 1px solid rgba(148,163,184,.3);
                   border-radius: 9px; padding: 9px 16px; font-size: 13px; cursor: pointer; }
#controls button:hover { background: rgba(148,163,184,.26); }
@keyframes spin { from { transform: rotate(0); } to { transform: rotate(360deg); } }
@keyframes pulse { 0%,100% { opacity: .5; transform: scale(.96); } 50% { opacity: 1; transform: scale(1.04); } }
@keyframes glow { 0%,100% { text-shadow: 0 0 0 rgba(34,197,94,0); } 50% { text-shadow: 0 0 24px rgba(34,197,94,.8); } }
```

- [ ] **Step 3: Create `static/app.js`**

```javascript
const yy = document.getElementById("taichi");
const st = document.getElementById("status");
const tr = document.getElementById("transcript");

function escapeHtml(s) {
  return s.replace(/[&<>]/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" }[c]));
}
function addBubble(cls, text) {
  const d = document.createElement("div");
  d.className = "bubble " + cls;
  d.innerHTML = '<div class="who">' + cls + "</div>" + escapeHtml(text);
  tr.appendChild(d);
  tr.scrollTop = tr.scrollHeight;
}

const es = new EventSource("/events");
es.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.type === "state") {
    yy.className = m.value;
    st.textContent = m.value;
  } else if (m.type === "turn") {
    addBubble("heard", m.heard);
    addBubble("refined", m.refined);
  } else if (m.type === "clear") {
    tr.innerHTML = "";
  }
};

const post = (path) => fetch(path, { method: "POST" });
document.getElementById("mic").onclick = () => post("/control/mic");
document.getElementById("clear").onclick = () => post("/control/clear");
document.getElementById("stop").onclick = () => post("/control/stop");
```

- [ ] **Step 4: Sanity check files load**

Run: `.venv/bin/python -c "import os; [print(os.path.exists('static/'+f)) for f in ('index.html','app.js','style.css')]"`
Expected: three `True`.

- [ ] **Step 5: Commit**

```bash
git add static/
git commit -m "feat: Zen Center dark front-end (HTML/CSS/JS)"
```

---

## Task 7: Wire the bus + `--ui` into `assistant.py`

**Files:** Modify `assistant.py`.

- [ ] **Step 1: Add imports + `--ui` startup in `main()`**

Add near the top imports of `assistant.py`:
```python
import threading
import ui_server
```

Replace the body of `main()` from the start through the `state = {"speaking": False}` line. The current code is:

```python
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
```

Replace it with:

```python
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

    if "--ui" in sys.argv:
        bus = ui_server.UiBus(history, player)
        port = int(os.environ.get("UI_PORT", "8765"))
        try:
            server = ui_server.make_server(bus, port=port)   # binds here; fail loud
        except OSError as e:
            print(f"UI port {port} unavailable ({e}). Set UI_PORT to a free port.")
            return
        threading.Thread(target=server.serve_forever, daemon=True).start()
        print(f"UI at http://localhost:{port}")
    else:
        bus = ui_server.NullBus(
            lambda t: format_timing(t["endpoint"], t["stt"], t["refine"], t["reply_start"]))

    def cb(indata, frames, time_, status):
        if not state["speaking"] and bus.listening_enabled:
            audio_q.put(indata[:, 0].copy())
```

- [ ] **Step 2: Replace the turn pipeline's prints/state with bus calls**

The current `while True:` body keeps its two guard scopes. Make these changes inside the loop:

Set the idle state right before the `while True:` line (after the `print("Listening. ...")`):
```python
    bus.set_state("listening" if bus.listening_enabled else "muted")
    with sd.InputStream(samplerate=SAMPLE_RATE, channels=1, blocksize=FRAME,
                        dtype="float32", callback=cb):
        while True:
```

Inside Guard scope 2, replace the timing/print/speak block. The current block is:
```python
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
```

Replace with:
```python
            try:
                bus.set_state("thinking")
                t0 = time.perf_counter()
                text = transcribe(utterance)
                t1 = time.perf_counter()
                if not text:
                    continue
                refined = refine(text, history, chat_fn)
                t2 = time.perf_counter()
                bus.push_turn(text, refined, {
                    "endpoint": MIN_SILENCE_MS,
                    "stt": round((t1 - t0) * 1000),
                    "refine": round((t2 - t1) * 1000),
                    "reply_start": round((t2 - t0) * 1000)})
                state["speaking"] = True
                bus.set_state("speaking")
                player.speak(refined)
            except Exception as e:
                print(f"[error] turn: {e}")
            finally:
                state["speaking"] = False
                vad.reset_states()
                with audio_q.mutex:
                    audio_q.queue.clear()
                bus.set_state("listening" if bus.listening_enabled else "muted")
```

Note: `--once` still returns early before any of this, so `--ui` is irrelevant to `--once` (spec #10). In CLI mode the `NullBus` makes `set_state` a no-op and `push_turn` print the same three lines as before, so CLI output is unchanged.

- [ ] **Step 3: Verify the full suite still passes (no regressions)**

Run: `.venv/bin/pytest -q`
Expected: **24 tests pass** (original 12 + 12 new: sse_format 1, nullbus 1, uibus/subscriber 7, routing 2, make_server 1). Importing `assistant` still works (it now imports `ui_server`, which is stdlib-only).

- [ ] **Step 4: Verify CLI output is unchanged via `--once` and a dead-port UI message**

Run: `.venv/bin/python assistant.py --once "um the meetin is uh tomorrow"`
Expected: prints `heard:` and `refined:` lines exactly as before (oMLX running).

Run: `UI_PORT=8002 .venv/bin/python assistant.py --ui` then immediately Ctrl-C if it proceeds — expected: it prints `UI port 8002 unavailable (...)` and returns (8002 is oMLX). DO NOT leave it running.

- [ ] **Step 5: Commit**

```bash
git add assistant.py
git commit -m "feat: wire UiBus/NullBus + --ui server into main loop"
```

---

## Task 8: End-to-end manual verification (human at the mic)

**Files:** none (verification only). Requires oMLX on `:8002`, mic permission, and headphones recommended (avoids the echo storms seen in P0).

- [ ] **Step 1:** Run `.venv/bin/python assistant.py --ui`. Expected: `Loading models...`, then `UI at http://localhost:8765`, then `Listening.`.
- [ ] **Step 2:** Open the URL. The ☯ should be blue/pulsing ("listening").
- [ ] **Step 3:** Speak a sentence, pause. Expected: ☯ → purple/spin (thinking) → green/glow (speaking); a heard bubble then a refined bubble appear; the Mac speaks the refined text.
- [ ] **Step 4:** Open a second browser tab *while it's mid-speaking* → it should immediately show the correct current state (snapshot-on-connect), not a default.
- [ ] **Step 5:** Click **Mic** → ☯ dims to grey ("muted"), speaking into the mic produces no new turns; click again → back to blue, turns resume.
- [ ] **Step 6:** Click **Stop** during playback → the Mac's speech cuts off immediately.
- [ ] **Step 7:** Click **Clear** → transcript empties; confirm the next turn starts fresh (no carried context).
- [ ] **Step 8:** Note any issues. Acceptable known limitations: one parked thread per open `/events` tab; echo storms without headphones (deferred AEC).

---

## Notes for the implementer

- **Do NOT `git push`** — commit locally; the user pushes manually.
- `ui_server.py` imports nothing from `assistant.py` (one-directional); `NullBus` gets its timing formatter injected to avoid a circular import.
- The audio callback reads only `state["speaking"]` and `bus.listening_enabled` (plain bools) — never the subscriber list — to stay realtime-safe.
- Tests run from the repo root so `import ui_server` / `import assistant` resolve (the existing root `conftest.py` handles sys.path).
- Tasks 6 (static files) and 8 (manual) have no automated tests; everything else is TDD.
