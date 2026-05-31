use serde_json::{json, Value};
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub fn build_tts_body(text: &str) -> Value {
    json!({ "text": text })
}

/// Fetch wav bytes from the TTS service and block until playback finishes.
///
/// rodio 0.22.x API: DeviceSinkBuilder::open_default_sink() -> MixerDeviceSink,
/// Player::connect_new(mixer) -> Player, Decoder::try_from(cursor) -> Result.
pub fn speak(client: &reqwest::blocking::Client, text: &str) -> Result<(), String> {
    let stop = AtomicBool::new(false);
    speak_stoppable(client, text, &stop)
}

/// Like `speak`, but polls `stop_flag` every 50 ms and returns early if set.
pub fn speak_stoppable(
    client: &reqwest::blocking::Client,
    text: &str,
    stop_flag: &AtomicBool,
) -> Result<(), String> {
    let bytes = client
        .post(crate::config::TTS_URL)
        .json(&build_tts_body(text))
        .timeout(Duration::from_secs(60))
        .send()
        .map_err(|e| format!("tts send: {e}"))?
        .bytes()
        .map_err(|e| format!("tts bytes: {e}"))?;

    let handle = rodio::DeviceSinkBuilder::open_default_sink()
        .map_err(|e| format!("audio out: {e}"))?;
    let player = rodio::Player::connect_new(handle.mixer());
    let src = rodio::Decoder::try_from(Cursor::new(bytes.to_vec()))
        .map_err(|e| format!("decode: {e}"))?;
    player.append(src);

    // Poll until playback finishes or stop_flag is set
    loop {
        if stop_flag.load(Ordering::SeqCst) {
            player.stop();
            return Ok(());
        }
        if player.empty() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tts_body_has_text() {
        assert_eq!(build_tts_body("hi"), json!({"text": "hi"}));
    }

    #[test]
    fn tts_body_empty_text() {
        assert_eq!(build_tts_body(""), json!({"text": ""}));
    }

    #[test]
    fn tts_body_special_chars() {
        let body = build_tts_body("Hello, world! It's a test.");
        assert_eq!(body["text"], "Hello, world! It's a test.");
    }
}
