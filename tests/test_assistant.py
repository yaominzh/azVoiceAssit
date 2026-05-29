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
