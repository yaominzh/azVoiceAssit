mod audio;
mod config;
mod settings;
mod events;
mod history;
mod refine;
mod segmenter;
mod state;
mod stt;
mod timing;
mod tts;
mod ui;
mod vad;
mod worker;
mod echo;

use std::sync::{Arc, atomic::AtomicBool};
use crossbeam_channel::bounded;
use crate::events::{ControlMsg, UiEvent};

fn main() -> eframe::Result<()> {
    // Reachability checks — fail loud and early
    let client = reqwest::blocking::Client::new();
    match client
        .get("http://127.0.0.1:8002/v1/models")
        .header("Authorization", format!("Bearer {}", config::OMLX_API_KEY))
        .timeout(std::time::Duration::from_secs(5))
        .send()
    {
        Err(e) => {
            eprintln!("oMLX not reachable at :8002: {e}");
            std::process::exit(1);
        }
        Ok(r) if !r.status().is_success() => {
            eprintln!("oMLX error: {}", r.status());
            std::process::exit(1);
        }
        _ => {}
    }
    // TTS service: GET / returns 404 (no root route), so only treat a connection
    // failure as "not reachable" — a response of any status means the server is up.
    if let Err(e) = client
        .get("http://127.0.0.1:8123/")
        .timeout(std::time::Duration::from_secs(5))
        .send()
    {
        eprintln!("TTS service not reachable at :8123: {e}");
        std::process::exit(1);
    }

    // Two channels: raw (cpal → processing thread), processed (processing thread → worker)
    let (tx_raw, rx_raw) = bounded::<Vec<f32>>(256);
    let (tx_processed, rx_processed) = bounded::<Vec<f32>>(256);
    let (tx_ctrl, rx_ctrl) = bounded::<ControlMsg>(64);
    let (tx_ui, rx_ui) = bounded::<UiEvent>(256);

    // Shared state
    let shared = Arc::new(state::SharedState::new());
    let speaking = Arc::new(AtomicBool::new(false));

    // Create AEC engine (Phase 1: shadow mode)
    let echo_arc = match echo::EchoCancel::new(config::FRAME, 4096) {
        Ok(ec) => {
            eprintln!("[aec] EchoCancel ready (frame={} filter=4096)", config::FRAME);
            Arc::new(ec)
        }
        Err(e) => {
            eprintln!("[aec] EchoCancel init failed: {e}");
            std::process::exit(1);
        }
    };

    // Start capture — keeps stream alive for process lifetime
    let _stream = match audio::start_capture(tx_raw, shared.clone()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to start audio capture: {e}");
            std::process::exit(1);
        }
    };

    // Audio processing thread: raw frames → AEC → processed frames
    let _processing = audio::start_processing_thread(
        rx_raw, tx_processed, Some(echo_arc.clone()));

    // Worker thread
    let shared_w = shared.clone();
    let speaking_w = speaking.clone();
    let echo_w = echo_arc.clone();
    std::thread::spawn(move || worker::run(rx_processed, rx_ctrl, tx_ui, shared_w, speaking_w, echo_w));

    // Run egui window — blocks until the window is closed
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Voice Assistant")
            .with_inner_size([480.0, 700.0])
            .with_min_inner_size([360.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Voice Assistant",
        options,
        Box::new(move |cc| {
            let mut style = (*cc.egui_ctx.global_style()).clone();
            style.visuals.panel_fill = egui::Color32::from_rgb(0x0B, 0x10, 0x20);
            style.visuals.window_fill = egui::Color32::from_rgb(0x0B, 0x10, 0x20);
            cc.egui_ctx.set_global_style(style);
            Ok(Box::new(ui::VoiceApp::new(rx_ui, tx_ctrl)))
        }),
    )
}
