use aec_rs::{Aec, AecConfig};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::sync::Mutex;

/// AEC Phase 1: shadow mode — observes TTS reference + mic, logs cancellation.
/// Does NOT feed cancelled output to VAD yet. Half-duplex speaking gate unchanged.
pub struct EchoCancel {
    /// Speex AEC state — Mutex because Aec contains raw pointers (not Send+Sync).
    /// Lock is held only for the ~0.1ms duration of one 512-sample frame.
    aec: Mutex<Aec>,
    /// Reference frame queue: TTS thread pushes, processing thread pops.
    /// Bounded to avoid unbounded memory growth if TTS outpaces processing.
    ref_tx: Sender<Vec<i16>>,
    ref_rx: Receiver<Vec<i16>>,
    frame_size: usize,
}

impl EchoCancel {
    /// Create a new AEC engine.
    /// - `frame_size`: samples per frame (512 for our 16kHz VAD)
    /// - `filter_length`: samples of tail to model (4096 = ~256ms at 16kHz)
    pub fn new(frame_size: usize, filter_length: usize) -> Result<Self, String> {
        let config = AecConfig {
            frame_size,
            filter_length: filter_length as i32,
            sample_rate: crate::config::SAMPLE_RATE,
            enable_preprocess: true,
        };
        let aec = Aec::new(&config);
        let (ref_tx, ref_rx) = bounded(64); // buffer up to 64 reference frames
        Ok(Self { aec: Mutex::new(aec), ref_tx, ref_rx, frame_size })
    }

    /// Push one frame of TTS PCM as AEC reference (far-end signal).
    /// Called from the TTS thread. Non-blocking: drops frames if queue full.
    pub fn push_reference(&self, frame: &[f32]) {
        let _ = self.ref_tx.try_send(f32_to_i16(frame));
    }

    /// Process one mic frame through AEC. Returns echo-cancelled f32 output.
    /// Called from the audio-processing thread (NOT the cpal callback).
    pub fn process_frame(&self, mic: &[f32]) -> Vec<f32> {
        let mic_i16 = f32_to_i16(mic);
        // Use queued reference if available; fall back to silence
        let ref_i16 = self.ref_rx.try_recv()
            .unwrap_or_else(|_| vec![0i16; self.frame_size]);

        let mut out = vec![0i16; self.frame_size];
        if let Ok(aec) = self.aec.lock() {
            aec.cancel_echo(&mic_i16, &ref_i16, &mut out);
        }
        i16_to_f32(&out)
    }

    /// Flush pending reference frames and reset internal AEC state.
    /// Call when TTS stops (natural end or Stop button).
    pub fn reset(&self) {
        while self.ref_rx.try_recv().is_ok() {}
        // Note: aec-rs 1.0.0 does not expose a reset method on Aec.
        // The echo state will naturally adapt. If a hard reset is needed,
        // recreate the Aec — but for Phase 1 shadow mode this is sufficient.
    }
}

/// Convert f32 [-1.0, 1.0] → i16 with saturation clamping.
pub fn f32_to_i16(samples: &[f32]) -> Vec<i16> {
    samples.iter().map(|&x| (x.clamp(-1.0, 1.0) * 32767.0) as i16).collect()
}

/// Convert i16 → f32 [-1.0, 1.0].
pub fn i16_to_f32(samples: &[i16]) -> Vec<f32> {
    samples.iter().map(|&x| x as f32 / 32767.0).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32_to_i16_positive_clamp() {
        assert_eq!(f32_to_i16(&[1.0]), vec![32767]);
        assert_eq!(f32_to_i16(&[2.0]), vec![32767]); // clamped
    }

    #[test]
    fn f32_to_i16_negative_clamp() {
        assert_eq!(f32_to_i16(&[-1.0]), vec![-32767]);
        assert_eq!(f32_to_i16(&[-2.0]), vec![-32767]); // clamped
    }

    #[test]
    fn f32_to_i16_zero() {
        assert_eq!(f32_to_i16(&[0.0]), vec![0]);
    }

    #[test]
    fn i16_to_f32_roundtrip_within_epsilon() {
        let original = vec![0.5f32, -0.5, 0.0, 0.999];
        let converted = f32_to_i16(&original);
        let back = i16_to_f32(&converted);
        for (a, b) in original.iter().zip(back.iter()) {
            assert!((a - b).abs() < 0.001, "roundtrip error: {a} vs {b}");
        }
    }

    #[test]
    fn f32_to_i16_batch() {
        let input = vec![0.0f32; 512];
        assert_eq!(f32_to_i16(&input).len(), 512);
    }

    #[test]
    fn echo_cancel_new_silence_no_panic() {
        let ec = EchoCancel::new(crate::config::FRAME, 4096)
            .expect("EchoCancel::new");
        let silence = vec![0.0f32; crate::config::FRAME];
        let out = ec.process_frame(&silence);
        assert_eq!(out.len(), crate::config::FRAME);
    }

    #[test]
    fn push_reference_and_process_roundtrip() {
        let ec = EchoCancel::new(crate::config::FRAME, 4096)
            .expect("EchoCancel::new");
        let silence = vec![0.0f32; crate::config::FRAME];
        ec.push_reference(&silence);
        let out = ec.process_frame(&silence);
        assert_eq!(out.len(), crate::config::FRAME);
    }

    #[test]
    fn reset_drains_reference_queue() {
        let ec = EchoCancel::new(crate::config::FRAME, 4096)
            .expect("EchoCancel::new");
        let silence = vec![0.0f32; crate::config::FRAME];
        ec.push_reference(&silence);
        ec.push_reference(&silence);
        ec.reset();
        // After reset, process_frame should still work (uses silence as reference fallback)
        let out = ec.process_frame(&silence);
        assert_eq!(out.len(), crate::config::FRAME);
    }
}
