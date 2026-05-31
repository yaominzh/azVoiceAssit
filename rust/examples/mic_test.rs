use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};
use std::io::Write;

fn write_wav(path: &str, samples: &[i16], rate: u32) {
    let mut f = std::fs::File::create(path).unwrap();
    let data_len = (samples.len() * 2) as u32;
    f.write_all(b"RIFF").unwrap();
    f.write_all(&(36 + data_len).to_le_bytes()).unwrap();
    f.write_all(b"WAVE").unwrap();
    f.write_all(b"fmt ").unwrap();
    f.write_all(&16u32.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap();
    f.write_all(&rate.to_le_bytes()).unwrap();
    f.write_all(&(rate * 2).to_le_bytes()).unwrap();
    f.write_all(&2u16.to_le_bytes()).unwrap();
    f.write_all(&16u16.to_le_bytes()).unwrap();
    f.write_all(b"data").unwrap();
    f.write_all(&data_len.to_le_bytes()).unwrap();
    for s in samples { f.write_all(&s.to_le_bytes()).unwrap(); }
}

#[allow(deprecated)]
fn main() {
    let host = cpal::default_host();
    let dev = host.default_input_device().expect("no input");
    let cfg = dev.default_input_config().expect("cfg");
    let rate = cfg.sample_rate();
    println!("Recording from {:?} at {} Hz for 8s...", dev.name(), rate);
    let buf = Arc::new(Mutex::new(Vec::<f32>::new()));
    let b2 = buf.clone();
    let stream = dev.build_input_stream(
        &cfg.config(),
        move |data: &[f32], _: &_| b2.lock().unwrap().extend_from_slice(data),
        move |e| eprintln!("err {e}"), None,
    ).unwrap();
    stream.play().unwrap();
    std::thread::sleep(std::time::Duration::from_secs(8));
    drop(stream);
    let samples = buf.lock().unwrap();
    let peak = samples.iter().cloned().fold(0.0f32, |a, b| a.max(b.abs()));
    let rms = (samples.iter().map(|x| x * x).sum::<f32>() / samples.len().max(1) as f32).sqrt();
    println!("captured {} samples, peak={:.4}, rms={:.4}", samples.len(), peak, rms);
    let i16s: Vec<i16> = samples.iter().map(|x| (x.clamp(-1.0, 1.0) * 32767.0) as i16).collect();
    write_wav("/tmp/mic_test.wav", &i16s, rate);
    println!("playing back...");
    std::process::Command::new("afplay").arg("/tmp/mic_test.wav").status().ok();
}
