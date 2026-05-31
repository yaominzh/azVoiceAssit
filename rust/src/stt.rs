use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::config::WHISPER_MODEL_PATH;

pub struct Stt {
    ctx: WhisperContext,
}

impl Stt {
    /// Load a whisper model from the given path (falls back to `WHISPER_MODEL_PATH`).
    pub fn load(path: &str) -> Result<Self, String> {
        let ctx = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .map_err(|e| format!("whisper load: {e:?}"))?;
        Ok(Self { ctx })
    }

    /// Load using the compile-time default model path.
    pub fn load_default() -> Result<Self, String> {
        Self::load(WHISPER_MODEL_PATH)
    }

    /// Transcribe 16 kHz mono f32 audio. Returns trimmed text or empty string.
    pub fn transcribe(&self, audio: &[f32]) -> Result<String, String> {
        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| format!("whisper state: {e:?}"))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some("en"));
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        state
            .full(params, audio)
            .map_err(|e| format!("whisper full: {e:?}"))?;

        // In 0.16.x the iterator API is the idiomatic way to collect segments.
        // `full_n_segments()` returns c_int directly (no Result).
        // `state.as_iter()` yields `WhisperSegment` objects; `.to_str()` gives &str.
        let mut result = String::new();
        for segment in state.as_iter() {
            match segment.to_str() {
                Ok(text) => result.push_str(text),
                Err(e) => eprintln!("whisper segment text error: {e:?}"),
            }
        }

        Ok(result.trim().to_string())
    }
}
