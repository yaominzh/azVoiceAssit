pub const SAMPLE_RATE: u32 = 16_000;
pub const FRAME: usize = 512;
pub const PREROLL_MS: usize = 250;
pub const PREROLL_FRAMES: usize = (PREROLL_MS * SAMPLE_RATE as usize) / (1000 * FRAME);
pub const MIN_SILENCE_MS: u32 = 700;
pub const SPEECH_THRESHOLD: f32 = 0.5;
pub const HISTORY_MAXLEN: usize = 40;

pub const OMLX_URL: &str = "http://127.0.0.1:8002/v1/chat/completions";
pub const OMLX_MODEL: &str = "gemma-4-e4b-it-8bit";
pub const OMLX_API_KEY: &str = "rdaz1234";
pub const TTS_URL: &str = "http://127.0.0.1:8123/tts";

pub const SYSTEM_PROMPT: &str = "You are a refinement assistant. The user gives you a raw spoken utterance. Repeat it back, cleaned up: fix grammar, drop filler words and false starts, keep the meaning and tone. Reply with ONLY the refined sentence, nothing else.";

pub const WHISPER_MODEL_PATH: &str = "models/ggml-base.en.bin";
pub const SILERO_MODEL_PATH: &str = "models/silero_vad.onnx";

/// Returns the path to the settings JSON file (~/.config/azva/settings.json).
/// Uses $HOME to avoid shell expansion issues.
pub fn settings_path() -> std::path::PathBuf {
    std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join(".config")
        .join("azva")
        .join("settings.json")
}
