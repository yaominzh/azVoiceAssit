use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::Sender;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use crate::config::{SAMPLE_RATE, FRAME};
use crate::state::SharedState;

/// Find the lowest supported sample rate >= SAMPLE_RATE, favouring 44100 on macOS.
fn pick_capture_rate(device: &cpal::Device) -> Result<u32, String> {
    let cfgs: Vec<_> = device.supported_input_configs()
        .map_err(|e| format!("supported configs: {e}"))?.collect();
    // Prefer 16kHz if supported, then 44.1kHz, then lowest >= SAMPLE_RATE
    for &target in &[SAMPLE_RATE, 44_100u32, 48_000u32] {
        if cfgs.iter().any(|c| c.min_sample_rate() <= target && target <= c.max_sample_rate()) {
            return Ok(target);
        }
    }
    // Fall back to lowest available
    cfgs.iter()
        .map(|c| c.min_sample_rate())
        .min()
        .ok_or_else(|| "no input configs".into())
}

/// Linear downsampler: drops samples to go from `src_rate` to `dst_rate`.
fn downsample(buf: &[f32], src_rate: u32, dst_rate: u32) -> Vec<f32> {
    if src_rate == dst_rate { return buf.to_vec(); }
    let ratio = src_rate as f64 / dst_rate as f64;
    let out_len = ((buf.len() as f64) / ratio) as usize;
    (0..out_len).map(|i| {
        let src_idx = (i as f64 * ratio) as usize;
        buf[src_idx.min(buf.len() - 1)]
    }).collect()
}

/// Open the default mic, resample to SAMPLE_RATE, chunk to FRAME size.
/// Returns the Stream — caller must keep it alive.
pub fn start_capture(
    tx: Sender<Vec<f32>>,
    shared: Arc<SharedState>,
    speaking: Arc<AtomicBool>,
) -> Result<cpal::Stream, String> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| "no default input device".to_string())?;

    let capture_rate = pick_capture_rate(&device)?;
    eprintln!("[audio] capturing at {} Hz, resampling to {} Hz", capture_rate, SAMPLE_RATE);

    let config = cpal::StreamConfig {
        channels: 1,
        sample_rate: capture_rate,
        buffer_size: cpal::BufferSize::Default,
    };

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
                let resampled = downsample(data, capture_rate, target_rate);
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
