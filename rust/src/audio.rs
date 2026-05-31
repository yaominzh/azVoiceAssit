use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::Sender;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use crate::config::{SAMPLE_RATE, FRAME};
use crate::state::SharedState;

/// Linear interpolation resampler: avoids aliasing from point-sampling.
/// Handles non-integer ratios correctly (e.g. 44100→16000 = 2.75625×).
fn downsample(buf: &[f32], src_rate: u32, dst_rate: u32) -> Vec<f32> {
    if src_rate == dst_rate { return buf.to_vec(); }
    let ratio = src_rate as f64 / dst_rate as f64;
    let out_len = ((buf.len() as f64) / ratio) as usize;
    (0..out_len).map(|i| {
        let pos = i as f64 * ratio;
        let lo = pos.floor() as usize;
        let hi = (lo + 1).min(buf.len() - 1);
        let t = (pos - pos.floor()) as f32;
        buf[lo] * (1.0 - t) + buf[hi] * t
    }).collect()
}

/// Open the default mic via its OWN default config (the proven-working path, same
/// as the mic_test diagnostic), downmix to mono, resample to SAMPLE_RATE, chunk to
/// FRAME size. Returns the Stream — caller must keep it alive.
pub fn start_capture(
    tx: Sender<Vec<f32>>,
    shared: Arc<SharedState>,
    speaking: Arc<AtomicBool>,
) -> Result<cpal::Stream, String> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| "no default input device".to_string())?;

    // Use the device's reported default config — this is what cpal guarantees works
    // (a hand-built StreamConfig was silently delivering silence on this machine).
    let supported = device
        .default_input_config()
        .map_err(|e| format!("default_input_config: {e}"))?;
    let capture_rate = supported.sample_rate();
    let channels = supported.channels() as usize;
    let config: cpal::StreamConfig = supported.config();
    eprintln!(
        "[audio] device={:?} capturing {} Hz x{}ch -> {} Hz",
        device.name().unwrap_or_default(), capture_rate, channels, SAMPLE_RATE
    );

    let target_rate = SAMPLE_RATE;
    let mut chunk_buf: Vec<f32> = Vec::with_capacity(FRAME * 4);

    let stream = device
        .build_input_stream(
            &config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                if !shared.listening_enabled.load(Ordering::Relaxed)
                    || speaking.load(Ordering::Relaxed)
                {
                    return;
                }
                // Downmix interleaved channels to mono (take channel 0).
                let mono: Vec<f32> = if channels > 1 {
                    data.iter().step_by(channels).copied().collect()
                } else {
                    data.to_vec()
                };
                let resampled = downsample(&mono, capture_rate, target_rate);
                chunk_buf.extend_from_slice(&resampled);
                while chunk_buf.len() >= FRAME {
                    let frame: Vec<f32> = chunk_buf.drain(..FRAME).collect();
                    let _ = tx.try_send(frame);
                }
            },
            move |err| eprintln!("[audio error] {err}"),
            None,
        )
        .map_err(|e| format!("build stream: {e}"))?;

    stream.play().map_err(|e| format!("play stream: {e}"))?;
    Ok(stream)
}
