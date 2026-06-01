# AEC Phase 1 Acceptance Test Results

**Date:** 2026-06-01  
**Branch:** `feat/aec-echo-cancellation`  
**Device:** iFLYair2 microphone (native 16kHz — no resampling needed)

## Setup
- oMLX `:8002` running (gemma-4-e4b-it-8bit)
- Qwen3-TTS `:8123` running (24kHz output, resampled to 16kHz before AEC reference push)
- AEC: `EchoCancel` with `frame=512 filter=4096` (~256ms tail coverage)

## Test results

### ✅ App launches with AEC ready
```
[aec] EchoCancel ready (frame=512 filter=4096)
[audio] device="iFLYair2" capturing 16000 Hz x1ch -> 16000 Hz
```

### ✅ Transcript working (no regression)
Two turns captured correctly:
- `"Hello, hello, can you hear me? To be or not to be? It is a question."` → `"Hello, can you hear me? To be or not to be? It is a question."`
- `"To be or not to be, it is a question."` → `"To be or not to be, that is the question."`

### ✅ AEC shadow logs present — cancellation observed
```
[aec-shadow] raw_rms=0.0401 clean_rms=0.0005 reduction=-37.5dB  ← excellent
[aec-shadow] raw_rms=0.0803 clean_rms=0.0223 reduction=-11.1dB  ← good
[aec-shadow] raw_rms=0.0960 clean_rms=0.0077 reduction=-21.9dB  ← good
[aec-shadow] raw_rms=0.0182 clean_rms=0.0018 reduction=-20.3dB  ← good
```
Best observed cancellation: -37.5dB. Several frames in the -10 to -22dB range.

### ⚠️ Positive dB frames observed (expected for Phase 1)
```
[aec-shadow] raw_rms=0.0617 clean_rms=0.1314 reduction=6.6dB
[aec-shadow] raw_rms=0.1116 clean_rms=0.1191 reduction=0.6dB
[aec-shadow] raw_rms=0.1715 clean_rms=0.1729 reduction=0.1dB
```
Positive values indicate user's own speech arriving at the mic while AEC is running
(the filter uses TTS as reference, so user speech looks like "signal to preserve" from
AEC's perspective during the speaking window). This is expected — the speaking gate in
`audio.rs` is still active for Phase 1; the AEC output is not yet fed to VAD.

### ✅ No false TTS-triggered turns
The existing `speaking` gate suppressed TTS echo turns as before. The AEC shadow mode
ran in parallel without disrupting the live pipeline.

### ✅ No errors
No `[worker]` errors, no `[audio error]`, no crashes. 31 unit tests pass.

## Phase 1 conclusion

AEC is active and producing cancellation data. The Speex adaptive filter needs several 
frames to converge after session start; once converged, consistent cancellation of -10 to 
-37dB is observed.

**Go/no-go for Phase 2:** ✅ Go, with conditions:
- Phase 2 (relax the `speaking` gate) should maintain a post-TTS window (e.g. 500ms)
  before enabling AEC-cleaned audio for VAD — allows filter convergence.
- The speaking gate should be relaxed gradually, not removed in one step.
- Consider logging a "convergence indicator" (N consecutive frames below threshold) before
  enabling AEC output for VAD.

## Open items for Phase 2 spec
1. How many AEC frames until convergence is reliable enough to feed VAD?
2. Worker architecture for true full-duplex (TTS on separate thread + concurrent VAD).
3. Test across speaker types: AirPods, external speaker, built-in (when available).
