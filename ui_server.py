"""In-process web UI for the voice assistant: SSE state/transcript out, control POSTs in."""
import json
import os
import threading
from collections import deque
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def sse_format(event):
    """Frame a dict as one Server-Sent Event line block."""
    return f"data: {json.dumps(event)}\n\n"
