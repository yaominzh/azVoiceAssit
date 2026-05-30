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
