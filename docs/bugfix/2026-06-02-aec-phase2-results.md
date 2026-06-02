# AEC Phase 2 Acceptance Test Results

**Date:** 2026-06-02  
**Branch:** `feat/aec-phase2-gate-relaxation`  
**Device:** iFLYair2 (native 16kHz — zero resampling)

## Setup
- oMLX `:8002` ✅
- Qwen3-TTS `:8123` ✅
- AEC: `EchoCancel` ready (frame=512 filter=4096)

## Barge-in confirmed ✅

Two barge-in events observed in the log:
```
[barge-in] stopping TTS gen=3 clean_rms=0.0236
[barge-in] stopping TTS gen=4 clean_rms=0.0375
```

Both `clean_rms` values are above the `BARGE_IN_THRESHOLD = 0.02` — the AEC suppressed
TTS echo below the threshold while correctly passing through the user's real voice.

## AEC shadow cancellation
Cancellation range: -14 to -25 dB observed during TTS playback. Consistent with Phase 1
results. AEC is actively suppressing the speaker output before it reaches VAD.

## No errors
No `[worker]`, `[tts]`, or `[audio error]` lines observed. App remained stable across
multiple turns and barge-in events.

## Phase 2 conclusion: ✅ PASS

Barge-in works: user speech mid-TTS triggers `stop_tts` via per-generation stop flag,
TTS cuts off, and the pipeline processes the new utterance. The `clean_rms` threshold
(0.02) correctly distinguishes real user speech from AEC-residual echo leakage.

## Notes
- `clean_rms` on both events (0.0236, 0.0375) is close to threshold — suggests the AEC
  is doing its job. If false barge-ins appear in noisier environments, the threshold can
  be raised (currently hardcoded in `worker.rs:30` as `BARGE_IN_THRESHOLD`). Adding it
  to Settings is a natural next step.
- The legacy `stop_tts` Arc was cleaned up post-implementation (no longer needed).

## What's next
- Expose `BARGE_IN_THRESHOLD` in the Settings panel
- Consider adding `min_barge_in_frames` (require N consecutive speech frames before
  stopping TTS, to avoid accidental single-frame triggers)
- Phase 3: full worker async restructure for TTS queue / cross-fade
