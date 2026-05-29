#!/usr/bin/env python3
"""P0 voice assistant: listen -> transcribe -> refine -> speak, in one warm loop."""
import os
import sys
import time
import queue
import subprocess
from collections import deque

import numpy as np
import mlx_whisper                       # add near the top imports
from openai import OpenAI                # add near the top imports

# ---------------------------------------------------------------------------
# CONSTANTS
# ---------------------------------------------------------------------------
SAMPLE_RATE = 16000
FRAME = 512                                  # samples per VAD window at 16 kHz (~32 ms)
PREROLL_MS = 250
PREROLL_FRAMES = max(1, round(PREROLL_MS / 1000 * SAMPLE_RATE / FRAME))  # ~8 frames
MIN_SILENCE_MS = 700                         # VAD end-of-turn silence; the felt "endpoint" tax
SPEECH_THRESHOLD = 0.5
WHISPER_REPO = "mlx-community/whisper-base-mlx"
HISTORY_MAXLEN = 40                          # bounded deque (~20 turns); fed into refine
OMLX_BASE_URL = os.environ.get("OMLX_BASE_URL", "http://127.0.0.1:8002/v1")
OMLX_API_KEY = os.environ.get("OMLX_API_KEY", "rdaz1234")
OMLX_MODEL = os.environ.get("OMLX_MODEL", "gemma-4-e4b-it-8bit")
SYSTEM_PROMPT = (
    "You are a refinement assistant. The user gives you a raw spoken utterance. "
    "Repeat it back, cleaned up: fix grammar, drop filler words and false starts, "
    "keep the meaning and tone. Reply with ONLY the refined sentence, nothing else."
)


def validate_frame(chunk):
    """Coerce a captured block to exactly FRAME samples, or None if unusable.

    Silero VADIterator requires exactly 512-sample windows at 16 kHz; an xrun or
    odd final block would otherwise raise in the per-frame path and kill the loop.
    """
    n = len(chunk)
    if n == FRAME:
        return chunk
    if n < FRAME:
        return np.pad(chunk, (0, FRAME - n))
    return None


class Segmenter:
    """Assembles utterances from (frame, vad_event) pairs, with onset pre-roll.

    VADIterator reports 'start' AFTER speech onset, so we keep a ring buffer of
    recent frames and seed the utterance with it to avoid clipping leading phonemes.
    push() returns a concatenated utterance on the 'end' event, else None.
    """

    def __init__(self, preroll_frames):
        self.preroll = deque(maxlen=preroll_frames)
        self.buffer = []
        self.collecting = False

    def push(self, frame, event):
        if event and "start" in event:
            self.collecting = True
            self.buffer = list(self.preroll)   # silence frames before onset
            self.buffer.append(frame)          # the start frame itself
            self.preroll.clear()               # so the next utterance doesn't inherit this silence
        elif self.collecting:
            self.buffer.append(frame)
            if event and "end" in event:
                self.collecting = False
                utterance = np.concatenate(self.buffer)
                self.buffer = []
                return utterance
        else:
            self.preroll.append(frame)
        return None


def format_timing(endpoint_ms, stt_ms, refine_ms, reply_start_ms):
    """One-line per-turn latency readout. reply_start = end-of-speech -> TTS start
    (NOT playback duration). endpoint = the felt VAD silence tax."""
    return (
        f"⏱ endpoint ~{endpoint_ms}ms · stt {stt_ms}ms · "
        f"refine {refine_ms}ms · reply-start +{reply_start_ms}ms"
    )


def refine(text, history, chat_fn):
    """Refine one utterance. Appends the turn to the bounded history window and
    feeds that window to chat_fn (so refine is context-aware but capped)."""
    history.append({"role": "user", "content": text})
    messages = [{"role": "system", "content": SYSTEM_PROMPT}] + list(history)
    out = chat_fn(messages).strip()
    history.append({"role": "assistant", "content": out})
    return out


def transcribe(audio):
    return mlx_whisper.transcribe(audio, path_or_hf_repo=WHISPER_REPO)["text"].strip()


def make_chat_fn(client):
    """Bind an oMLX-backed chat function: messages -> assistant string."""
    def chat_fn(messages):
        resp = client.chat.completions.create(
            model=OMLX_MODEL, messages=messages, temperature=0.3)
        return resp.choices[0].message.content
    return chat_fn


def warm_up(chat_fn, transcribe_fn):
    """Pre-warm BOTH models so turn-1 latency ~= later turns. The chat call also
    serves as the oMLX reachability check (raises if the server is down)."""
    chat_fn([{"role": "user", "content": "hi"}])
    transcribe_fn(np.zeros(SAMPLE_RATE, dtype=np.float32))


class TtsPlayer:
    """Speaks text via macOS `say`, interruptibly. .speak() blocks until the
    utterance finishes or .stop() kills it. The interruptible primitive that a
    future barge-in feature builds on (live barge-in itself is out of P0 scope)."""

    def __init__(self, cmd_prefix=("say",)):
        self.cmd_prefix = tuple(cmd_prefix)
        self._proc = None

    def speak(self, text):
        self._proc = subprocess.Popen([*self.cmd_prefix, text])
        try:
            self._proc.wait()
        finally:
            self._proc = None

    def stop(self):
        proc = self._proc
        if proc and proc.poll() is None:
            proc.terminate()
