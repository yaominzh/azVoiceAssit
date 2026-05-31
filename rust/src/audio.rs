use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::Sender;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use crate::config::{SAMPLE_RATE, FRAME};

/// Open the default mic, chunk frames to FRAME size, send when enabled and not speaking.
/// Returns the Stream — caller must keep it alive.
pub fn start_capture(
    tx: Sender<Vec<f32>>,
    enabled: Arc<AtomicBool>,
    speaking: Arc<AtomicBool>,
) -> Result<cpal::Stream, String> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| "no default input device".to_string())?;

    let config = cpal::StreamConfig {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        buffer_size: cpal::BufferSize::Fixed(FRAME as u32),
    };

    let mut chunk_buf: Vec<f32> = Vec::with_capacity(FRAME * 2);

    let stream = device
        .build_input_stream(
            &config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                if !enabled.load(Ordering::Relaxed) || speaking.load(Ordering::Relaxed) {
                    return;
                }
                chunk_buf.extend_from_slice(data);
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
