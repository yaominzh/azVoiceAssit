from collections import deque

import numpy as np
import assistant


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
