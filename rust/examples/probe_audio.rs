#[allow(deprecated)]
use cpal::traits::{DeviceTrait, HostTrait};
fn main() {
    let host = cpal::default_host();
    let dev = host.default_input_device().expect("no input");
    println!("device: {:?}", dev.name());
    for cfg in dev.supported_input_configs().unwrap() {
        println!("  channels={} min_sr={} max_sr={} buf={:?}",
            cfg.channels(), cfg.min_sample_rate(), cfg.max_sample_rate(), cfg.buffer_size());
    }
}
