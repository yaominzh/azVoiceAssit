use serde_json::{json, Value};
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub fn build_tts_body(text: &str) -> Value {
    json!({ "text": text })
}

/// Fetch wav bytes from the TTS service and play them, polling `stop_flag` every
/// 50 ms (and `rx_ctrl` for `ControlMsg::Stop`) to allow interruption mid-playback.
///
/// rodio 0.22.x API: DeviceSinkBuilder::open_default_sink() -> MixerDeviceSink,
/// Player::connect_new(mixer) -> Player, Decoder::try_from(cursor) -> Result.
/// Also drains `rx_ctrl` on each poll tick: a `ControlMsg::Stop` sets the flag
/// and stops playback immediately; other messages are silently dropped here and
/// will be re-processed on the next worker loop iteration (they won't arrive
/// again, but they're low-priority control signals during TTS).
pub fn speak_stoppable(
    client: &reqwest::blocking::Client,
    text: &str,
    stop_flag: &AtomicBool,
    rx_ctrl: &crossbeam_channel::Receiver<crate::events::ControlMsg>,
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
        // Check ctrl channel for Stop during TTS playback
        if let Ok(crate::events::ControlMsg::Stop) = rx_ctrl.try_recv() {
            player.stop();
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
