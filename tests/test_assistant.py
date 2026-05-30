import json
import threading
import time
from collections import deque

import numpy as np
import assistant
import ui_server


def test_validate_frame_passes_exact():
    chunk = np.ones(assistant.FRAME, dtype=np.float32)
    out = assistant.validate_frame(chunk)
    assert out is chunk


def test_validate_frame_pads_short():
    chunk = np.ones(300, dtype=np.float32)
    out = assistant.validate_frame(chunk)
    assert out.shape == (assistant.FRAME,)
    assert np.all(out[:300] == 1.0)
    assert np.all(out[300:] == 0.0)


def test_validate_frame_skips_oversized():
    chunk = np.ones(600, dtype=np.float32)
    assert assistant.validate_frame(chunk) is None


def _frame(value):
    return np.full(assistant.FRAME, value, dtype=np.float32)


def test_segmenter_emits_nothing_before_start():
    seg = assistant.Segmenter(preroll_frames=2)
    assert seg.push(_frame(0.0), None) is None
    assert seg.push(_frame(0.0), None) is None


def test_segmenter_prepends_preroll_on_start():
    seg = assistant.Segmenter(preroll_frames=2)
    # Two silence frames (value 1.0) fill the pre-roll ring.
    seg.push(_frame(1.0), None)
    seg.push(_frame(1.0), None)
    # Speech starts (value 2.0) and then ends.
    assert seg.push(_frame(2.0), {"start": 0}) is None
    utt = seg.push(_frame(2.0), {"end": 1})
    assert utt is not None
    # Pre-roll (1.0) must appear before the speech frames (2.0) — no onset clip.
    n = assistant.FRAME
    assert np.any(utt[:n] == 1.0)
    assert utt[-1] == 2.0
    # 2 pre-roll + start frame + end frame = 4 frames concatenated.
    assert utt.shape[0] == 4 * n


def test_segmenter_resets_between_utterances():
    seg = assistant.Segmenter(preroll_frames=1)
    seg.push(_frame(1.0), {"start": 0})
    seg.push(_frame(1.0), {"end": 1})
    # Second utterance should not include the first one's buffered frames.
    seg.push(_frame(3.0), {"start": 0})
    utt = seg.push(_frame(3.0), {"end": 1})
    assert np.all(np.isin(np.unique(utt), [1.0, 3.0]))  # only preroll(1.0)+speech(3.0)


def test_refine_appends_user_then_assistant_and_uses_window():
    captured = {}

    def fake_chat(messages):
        captured["messages"] = list(messages)
        return "Clean sentence."

    history = deque(maxlen=40)
    out = assistant.refine("um the meetin tomorrow", history, fake_chat)

    assert out == "Clean sentence."
    # System prompt first, then the user turn already appended before the call.
    assert captured["messages"][0] == {"role": "system", "content": assistant.SYSTEM_PROMPT}
    assert captured["messages"][-1] == {"role": "user", "content": "um the meetin tomorrow"}
    # History now records both sides.
    assert list(history)[-2:] == [
        {"role": "user", "content": "um the meetin tomorrow"},
        {"role": "assistant", "content": "Clean sentence."},
    ]


def test_refine_history_is_bounded():
    history = deque(maxlen=4)   # 2 turns
    assistant.refine("one", history, lambda m: "r1")
    assistant.refine("two", history, lambda m: "r2")
    assistant.refine("three", history, lambda m: "r3")
    assert len(history) == 4
    # Oldest ("one"/"r1") dropped; newest two turns retained.
    assert {m["content"] for m in history} == {"two", "r2", "three", "r3"}


def test_format_timing():
    line = assistant.format_timing(endpoint_ms=700, stt_ms=240, refine_ms=180, reply_start_ms=430)
    assert line == "⏱ endpoint ~700ms · stt 240ms · refine 180ms · reply-start +430ms"


def test_tts_player_stop_interrupts():
    # Use `sleep` as a stand-in for `say` so the test is deterministic and silent.
    player = assistant.TtsPlayer(cmd_prefix=("sleep",))
    start = time.perf_counter()
    t = threading.Thread(target=player.speak, args=("5",))
    t.start()
    time.sleep(0.2)
    player.stop()
    t.join(timeout=2)
    elapsed = time.perf_counter() - start
    assert not t.is_alive()
    assert elapsed < 2.0   # interrupted, did not wait the full 5s


def test_tts_player_speak_completes():
    player = assistant.TtsPlayer(cmd_prefix=("sleep",))
    start = time.perf_counter()
    player.speak("0.05")
    assert time.perf_counter() - start < 1.0


def test_warm_up_calls_both_models():
    calls = {"chat": 0, "stt": 0, "stt_len": None}

    def fake_chat(messages):
        calls["chat"] += 1
        return "ok"

    def fake_transcribe(audio):
        calls["stt"] += 1
        calls["stt_len"] = len(audio)
        return ""

    assistant.warm_up(fake_chat, fake_transcribe)
    assert calls["chat"] == 1
    assert calls["stt"] == 1
    assert calls["stt_len"] == assistant.SAMPLE_RATE   # 1s of silence


def test_sse_format_frames_event():
    out = ui_server.sse_format({"type": "state", "value": "thinking"})
    assert out == 'data: {"type": "state", "value": "thinking"}\n\n'
    # Round-trips back to the same dict.
    assert json.loads(out[len("data: "):].strip()) == {"type": "state", "value": "thinking"}


def test_nullbus_prints_three_lines_and_is_otherwise_noop(capsys):
    timing = {"endpoint": 700, "stt": 100, "refine": 200, "reply_start": 300}
    bus = ui_server.NullBus(lambda t: f"TIMING {t['stt']}/{t['refine']}")
    bus.set_state("thinking")      # no-op
    bus.clear()                    # no-op
    bus.push_turn("raw words", "Clean words.", timing)
    out = capsys.readouterr().out
    assert out == "  heard:   raw words\n  refined: Clean words.\nTIMING 100/200\n"
    assert bus.listening_enabled is True


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
