use crate::echo::EchoCancel;
use serde_json::{json, Value};
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub fn build_tts_body(text: &str) -> Value {
    json!({ "text": text })
}

/// Fetch wav bytes from the TTS service and play them, polling `stop_flag` every
/// 50 ms to allow interruption mid-playback (barge-in).
///
/// `rx_ctrl` is intentionally absent: the worker owns the channel exclusively
/// during TTS so barge-in Stop signals arrive via `stop_flag` (set by the worker
/// before calling this function or by a concurrent watcher thread).
///
/// rodio 0.22.x API: DeviceSinkBuilder::open_default_sink() -> MixerDeviceSink,
/// Player::connect_new(mixer) -> Player, Decoder::try_from(cursor) -> Result.
pub fn speak_stoppable(
    client: &reqwest::blocking::Client,
    text: &str,
    stop_flag: &AtomicBool,
    echo: Option<&Arc<crate::echo::EchoCancel>>,
) -> Result<(), String> {
    // Check stop before even making the HTTP request (barge-in may have arrived)
    if stop_flag.load(Ordering::SeqCst) {
        if let Some(ec) = echo { ec.reset(); }
        return Ok(());
    }

    let bytes = client
        .post(crate::config::TTS_URL)
        .json(&build_tts_body(text))
        .timeout(Duration::from_secs(60))
        .send()
        .map_err(|e| format!("tts send: {e}"))?
        .bytes()
        .map_err(|e| format!("tts bytes: {e}"))?;

    // Check again after HTTP completes (barge-in may have arrived during request)
    if stop_flag.load(Ordering::SeqCst) {
        if let Some(ec) = echo { ec.reset(); }
        return Ok(());
    }

    // Push TTS PCM as AEC reference (resampled 24kHz→16kHz)
    if let Some(ec) = echo {
        if let Ok(pcm_i16) = extract_wav_pcm_i16(&bytes) {
            let raw_f32 = crate::echo::i16_to_f32(&pcm_i16);
            let resampled = crate::audio::downsample(
                &raw_f32, 24_000, crate::config::SAMPLE_RATE);
            for chunk in resampled.chunks(crate::config::FRAME) {
                ec.push_reference(chunk);
            }
        }
    }

    let handle = rodio::DeviceSinkBuilder::open_default_sink()
        .map_err(|e| format!("audio out: {e}"))?;
    let player = rodio::Player::connect_new(handle.mixer());
    let src = rodio::Decoder::try_from(Cursor::new(bytes.to_vec()))
        .map_err(|e| format!("decode: {e}"))?;
    player.append(src);

    loop {
        if stop_flag.load(Ordering::SeqCst) {
            player.stop();
            if let Some(ec) = echo { ec.reset(); }
            return Ok(());
        }
        if player.empty() {
            if let Some(ec) = echo { ec.reset(); }
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Extract raw i16 PCM samples from a WAV byte buffer.
/// Searches for the "data" chunk and reads all i16 LE samples after it.
/// Works for the Qwen3-TTS output (16-bit mono PCM).
pub fn extract_wav_pcm_i16(wav: &[u8]) -> Result<Vec<i16>, String> {
    let data_offset = wav
        .windows(4)
        .position(|w| w == b"data")
        .ok_or_else(|| "no 'data' chunk in WAV".to_string())?
        + 8; // skip "data" (4 bytes) + chunk size (4 bytes)
    if data_offset >= wav.len() {
        return Err("WAV data chunk is empty".to_string());
    }
    let samples = wav[data_offset..]
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    Ok(samples)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speak_stoppable_signature_has_no_rx_ctrl() {
        // Compile-time check: speak_stoppable takes stop_flag but NOT rx_ctrl.
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;
        use crate::echo::EchoCancel;
        let _: fn(&reqwest::blocking::Client, &str, &AtomicBool, Option<&Arc<EchoCancel>>)
            -> Result<(), String> = speak_stoppable;
    }

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

    #[test]
    fn extract_pcm_from_wav_silence() {
        // Build a minimal valid 16-bit mono PCM WAV: 44-byte header + 20 bytes data (10 i16 samples)
        let mut wav: Vec<u8> = Vec::new();
        // RIFF header
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&48u32.to_le_bytes()); // chunk size = 36 + 12 = 48
        wav.extend_from_slice(b"WAVE");
        // fmt chunk
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes()); // chunk size
        wav.extend_from_slice(&1u16.to_le_bytes());  // PCM
        wav.extend_from_slice(&1u16.to_le_bytes());  // channels = 1
        wav.extend_from_slice(&16000u32.to_le_bytes()); // sample rate
        wav.extend_from_slice(&32000u32.to_le_bytes()); // byte rate
        wav.extend_from_slice(&2u16.to_le_bytes());  // block align
        wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        // data chunk
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&20u32.to_le_bytes()); // 20 bytes = 10 i16 samples
        wav.extend_from_slice(&[0u8; 20]);           // 10 silence samples

        let samples = extract_wav_pcm_i16(&wav);
        assert!(samples.is_ok(), "should parse: {:?}", samples.err());
        let s = samples.unwrap();
        assert_eq!(s.len(), 10);
        assert!(s.iter().all(|&x| x == 0));
    }
}
