use ndarray::{Array2, Array3};
use ort::{session::Session, value::Tensor};
use crate::segmenter::VadEvent;
use crate::config::{FRAME, SAMPLE_RATE, MIN_SILENCE_MS, SPEECH_THRESHOLD};

pub struct Vad {
    session: Session,
    state: Array3<f32>,        // [2, 1, 128] recurrent state
    silence_frames: u32,       // consecutive silent frames counter
    speech_active: bool,
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
            silence_frames: 0,
            speech_active: false,
        })
    }

    /// Feed one FRAME of audio. Returns Some(Start) when speech begins,
    /// Some(End) after MIN_SILENCE_MS of silence post-speech, None otherwise.
    pub fn accept(&mut self, frame: &[f32]) -> Result<Option<VadEvent>, String> {
        // Build owned tensors (required when passing named inputs via inputs! macro)
        let audio = Tensor::<f32>::from_array(
            Array2::from_shape_vec([1, FRAME], frame.to_vec())
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

        // Silence threshold in frames
        let silence_threshold = (MIN_SILENCE_MS as f32 / 1000.0
            * SAMPLE_RATE as f32
            / FRAME as f32) as u32;

        // State machine
        if prob >= SPEECH_THRESHOLD {
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
        self.silence_frames = 0;
        self.speech_active = false;
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
}
