#!/usr/bin/env python3
"""P0 voice assistant: listen -> transcribe -> refine -> speak, in one warm loop."""
import os
import sys
import time
import queue
import subprocess
import threading
from collections import deque

import numpy as np
import ui_server
import mlx_whisper                       # add near the top imports
from openai import OpenAI                # add near the top imports
import sounddevice as sd                 # add near the top imports
import torch                             # add near the top imports
from silero_vad import load_silero_vad, VADIterator   # add near the top imports

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


def run_once(text, client, speak=False):
    """Headless: refine one line of text with no microphone. For deterministic
    manual/integration checks of the refine + oMLX wiring."""
    history = deque(maxlen=HISTORY_MAXLEN)
    refined = refine(text, history, make_chat_fn(client))
    print(f"  heard:   {text}")
    print(f"  refined: {refined}")
    if speak:
        TtsPlayer().speak(refined)
    return refined


def main():
    client = OpenAI(base_url=OMLX_BASE_URL, api_key=OMLX_API_KEY)

    if len(sys.argv) >= 3 and sys.argv[1] == "--once":
        run_once(sys.argv[2], client, speak="--speak" in sys.argv)
        return

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

    print("Listening. Speak, then pause. Ctrl-C to quit.")
    bus.set_state("listening" if bus.listening_enabled else "muted")
    with sd.InputStream(samplerate=SAMPLE_RATE, channels=1, blocksize=FRAME,
                        dtype="float32", callback=cb):
        while True:
            chunk = audio_q.get()

            # Guard scope 1: frame intake + VAD feed (runs every frame).
            try:
                frame = validate_frame(chunk)
                if frame is None:
                    continue
                event = vad(torch.from_numpy(frame))
            except Exception as e:
                print(f"[error] vad: {e}")
                vad.reset_states()
                continue

            utterance = seg.push(frame, event)
            if utterance is None:
                continue

            # Guard scope 2: the turn pipeline.
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


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\nbye")
