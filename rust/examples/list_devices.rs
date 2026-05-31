use cpal::traits::{DeviceTrait, HostTrait};
#[allow(deprecated)]
fn main() {
    let host = cpal::default_host();
    println!("=== Default input: {:?}", host.default_input_device().map(|d| d.name().unwrap_or_default()));
    println!("\n=== All input devices:");
    for dev in host.input_devices().unwrap() {
        if let Ok(name) = dev.name() {
            for cfg in dev.supported_input_configs().unwrap() {
                println!("  [{}] channels={} sr={}-{} buf={:?}", name, cfg.channels(), cfg.min_sample_rate(), cfg.max_sample_rate(), cfg.buffer_size());
            }
        }
    }
}
