use ndarray::{Array2, Array3};
use ort::{session::Session, value::Tensor};
use crate::segmenter::VadEvent;
use crate::config::{FRAME, SAMPLE_RATE};

/// Silero v5 maintains a context window: each 512-sample frame must be prefixed
/// with the previous 64 samples (→ 576-sample model input). The torch model does
/// this internally; the ONNX caller must do it explicitly, or speech probability
/// stays ~0. (Confirmed: bare 512 → prob 0.0016; with 64-sample context → 0.9999.)
const CTX: usize = 64;

pub struct Vad {
    session: Session,
    state: Array3<f32>,        // [2, 1, 128] recurrent state
    context: Vec<f32>,         // last CTX samples of the previous frame
    silence_frames: u32,       // consecutive silent frames counter
    speech_active: bool,
    pub silence_ms: u32,          // runtime-adjustable silence threshold
    pub speech_threshold: f32,    // runtime-adjustable speech probability threshold
}

impl Vad {
    pub fn load(model_path: &str) -> Result<Self, String> {
        let session = Session::builder()
            .map_err(|e| format!("ort builder: {e}"))?
            .commit_from_file(model_path)
            .map_err(|e| format!("ort load: {e}"))?;
        Ok(Self {
            session,
            state: Array3::zeros([2, 1, 128]),
            context: vec![0.0; CTX],
            silence_frames: 0,
            speech_active: false,
            silence_ms: crate::config::MIN_SILENCE_MS,
            speech_threshold: crate::config::SPEECH_THRESHOLD,
        })
    }

    /// Feed one FRAME of audio. Returns Some(Start) when speech begins,
    /// Some(End) after MIN_SILENCE_MS of silence post-speech, None otherwise.
    pub fn accept(&mut self, frame: &[f32]) -> Result<Option<VadEvent>, String> {
        // Prepend the 64-sample context to the 512-sample frame → 576-sample input.
        let mut input_vec = Vec::with_capacity(CTX + frame.len());
        input_vec.extend_from_slice(&self.context);
        input_vec.extend_from_slice(frame);
        let in_len = input_vec.len();
        let audio = Tensor::<f32>::from_array(
            Array2::from_shape_vec([1, in_len], input_vec)
                .map_err(|e| format!("audio shape: {e}"))?,
        )
        .map_err(|e| format!("audio tensor: {e}"))?;

        let sr = Tensor::<i64>::from_array(
            ndarray::array![[SAMPLE_RATE as i64]],
        )
        .map_err(|e| format!("sr tensor: {e}"))?;

        let state_tensor = Tensor::<f32>::from_array(self.state.clone())
            .map_err(|e| format!("state tensor: {e}"))?;

        // Named inputs — model expects input / state / sr
        // The named form of inputs! returns Vec directly (not a Result)
        let inputs = ort::inputs![
            "input" => audio,
            "state" => state_tensor,
            "sr"    => sr,
        ];

        let outputs = self.session.run(inputs)
            .map_err(|e| format!("run: {e}"))?;

        // Extract speech probability: output shape [1, 1]
        let (_, prob_slice) = outputs["output"]
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("extract output: {e}"))?;
        let prob = prob_slice[0];

        // Extract updated recurrent state: stateN shape [2, 1, 128]
        let (_, state_slice) = outputs["stateN"]
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("extract stateN: {e}"))?;
        // state_slice is flat [2*1*128 = 256] — copy back into self.state
        self.state
            .as_slice_mut()
            .expect("state is contiguous")
            .copy_from_slice(state_slice);

        // Carry the last CTX samples of this frame as context for the next call.
        self.context = frame[frame.len() - CTX..].to_vec();

        // Silence threshold in frames
        let silence_threshold = (self.silence_ms as f32 / 1000.0
            * SAMPLE_RATE as f32
            / FRAME as f32) as u32;

        // State machine
        if prob >= self.speech_threshold {
            self.silence_frames = 0;
            if !self.speech_active {
                self.speech_active = true;
                return Ok(Some(VadEvent::Start));
            }
        } else if self.speech_active {
            self.silence_frames += 1;
            if self.silence_frames >= silence_threshold {
                self.speech_active = false;
                self.silence_frames = 0;
                return Ok(Some(VadEvent::End));
            }
        }
        Ok(None)
    }

    pub fn reset(&mut self) {
        self.state = Array3::zeros([2, 1, 128]);
        self.context = vec![0.0; CTX];
        self.silence_frames = 0;
        self.speech_active = false;
    }

    pub fn set_thresholds(&mut self, silence_ms: u32, speech_threshold: f32) {
        self.silence_ms = silence_ms;
        self.speech_threshold = speech_threshold;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_produces_no_start() {
        let model_path = concat!(env!("CARGO_MANIFEST_DIR"), "/models/silero_vad.onnx");
        let mut vad = Vad::load(model_path).expect("load VAD model");
        let silence = vec![0.0f32; FRAME];
        // Feed 10 frames of silence — should never produce Start
        for _ in 0..10 {
            let event = vad.accept(&silence).expect("accept");
            assert!(
                event != Some(VadEvent::Start),
                "silence should not trigger speech start"
            );
        }
    }

    #[test]
    fn set_thresholds_updates_fields() {
        let model_path = concat!(env!("CARGO_MANIFEST_DIR"), "/models/silero_vad.onnx");
        let mut vad = Vad::load(model_path).expect("load VAD");
        vad.set_thresholds(1000, 0.7);
        assert_eq!(vad.silence_ms, 1000);
        assert!((vad.speech_threshold - 0.7).abs() < 1e-6);
    }
}
