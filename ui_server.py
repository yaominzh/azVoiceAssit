"""In-process web UI for the voice assistant: SSE state/transcript out, control POSTs in."""
import json
import os
import threading
from collections import deque
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def sse_format(event):
    """Frame a dict as one Server-Sent Event line block."""
    return f"data: {json.dumps(event)}\n\n"


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
