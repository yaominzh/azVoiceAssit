mod audio;
mod config;
mod echo;
mod events;
mod history;
mod refine;
mod segmenter;
mod settings;
mod state;
mod stt;
mod timing;
mod tts;
mod vad;
mod worker;

use std::sync::{Arc, atomic::AtomicBool};
use crossbeam_channel::bounded;
use events::{ControlMsg, UiEvent};
use tauri::Emitter;

/// Bridge state — thin wrapper holding only the ctrl channel sender.
struct AppBridge {
    tx_ctrl: crossbeam_channel::Sender<ControlMsg>,
}

// Testable helper used by all commands
fn send_ctrl(tx: &crossbeam_channel::Sender<ControlMsg>, msg: ControlMsg) -> Result<(), String> {
    tx.send(msg).map_err(|e| e.to_string())
}

#[tauri::command]
fn toggle_mic(bridge: tauri::State<AppBridge>) -> Result<(), String> {
    send_ctrl(&bridge.tx_ctrl, ControlMsg::ToggleMic)
}

#[tauri::command]
fn stop_tts(bridge: tauri::State<AppBridge>) -> Result<(), String> {
    send_ctrl(&bridge.tx_ctrl, ControlMsg::Stop)
}

#[tauri::command]
fn clear_transcript(bridge: tauri::State<AppBridge>) -> Result<(), String> {
    send_ctrl(&bridge.tx_ctrl, ControlMsg::Clear)
}

#[tauri::command]
fn get_settings() -> settings::AppSettings {
    settings::AppSettings::load()
}

#[tauri::command]
fn get_defaults() -> settings::AppSettings {
    settings::AppSettings::default()
}

#[tauri::command]
fn apply_settings(
    s: settings::AppSettings,
    bridge: tauri::State<AppBridge>,
) -> Result<(), String> {
    let validated = s.validate();
    validated.save()?;
    send_ctrl(&bridge.tx_ctrl, ControlMsg::SettingsChanged(validated))
}

#[tauri::command]
fn get_initial_state() -> serde_json::Value {
    let s = settings::AppSettings::load();
    serde_json::json!({ "state": "listening", "settings": s })
}

fn main() {
    // Reachability checks — fail loud and early
    let client = reqwest::blocking::Client::new();
    match client
        .get("http://127.0.0.1:8002/v1/models")
        .header("Authorization", format!("Bearer {}", config::OMLX_API_KEY))
        .timeout(std::time::Duration::from_secs(5))
        .send()
    {
        Err(e) => { eprintln!("oMLX not reachable at :8002: {e}"); std::process::exit(1); }
        Ok(r) if !r.status().is_success() => { eprintln!("oMLX error: {}", r.status()); std::process::exit(1); }
        _ => {}
    }
    if let Err(e) = client
        .get("http://127.0.0.1:8123/")
        .timeout(std::time::Duration::from_secs(5))
        .send()
    {
        eprintln!("TTS service not reachable at :8123: {e}");
        std::process::exit(1);
    }

    // Channels
    let (tx_raw, rx_raw) = bounded::<Vec<f32>>(256);
    let (tx_processed, rx_processed) = bounded::<Vec<f32>>(256);
    let (tx_ctrl, rx_ctrl) = bounded::<ControlMsg>(64);
    let (tx_ui, rx_ui) = bounded::<UiEvent>(256);

    // Shared state
    let shared = Arc::new(state::SharedState::new());
    let speaking = Arc::new(AtomicBool::new(false));

    // AEC engine
    let echo_arc = match echo::EchoCancel::new(config::FRAME, 4096) {
        Ok(ec) => { eprintln!("[aec] EchoCancel ready"); Arc::new(ec) }
        Err(e) => { eprintln!("[aec] EchoCancel init failed: {e}"); std::process::exit(1); }
    };

    // Capture + processing thread (start_capture takes 2 args — speaking gate removed in AEC Phase 2)
    let _stream = match audio::start_capture(tx_raw, shared.clone()) {
        Ok(s) => s,
        Err(e) => { eprintln!("Failed to start audio capture: {e}"); std::process::exit(1); }
    };
    let _processing = audio::start_processing_thread(rx_raw, tx_processed, Some(echo_arc.clone()));

    // Worker thread
    let (shared_w, speaking_w, echo_w) = (shared.clone(), speaking.clone(), echo_arc.clone());
    std::thread::spawn(move || worker::run(rx_processed, rx_ctrl, tx_ui, shared_w, speaking_w, echo_w));

    // Tauri app
    tauri::Builder::default()
        .setup(move |app| {
            // Bridge thread: drain rx_ui → app.emit()
            let app_handle = app.handle().clone();
            std::thread::spawn(move || {
                while let Ok(event) = rx_ui.recv() {
                    match event {
                        UiEvent::StateChanged(s) => {
                            app_handle.emit("state", events::StatePayload { value: s.label() }).ok();
                        }
                        UiEvent::Turn { heard, refined, timing, timestamp } => {
                            app_handle.emit("turn", events::TurnPayload {
                                heard, refined, timestamp,
                                endpoint_ms: timing.endpoint_ms,
                                stt_ms: timing.stt_ms,
                                refine_ms: timing.refine_ms,
                                reply_start_ms: timing.reply_start_ms,
                            }).ok();
                        }
                        UiEvent::Cleared => { app_handle.emit("clear", ()).ok(); }
                    }
                }
            });
            Ok(())
        })
        .manage(AppBridge { tx_ctrl })
        .invoke_handler(tauri::generate_handler![
            toggle_mic, stop_tts, clear_transcript,
            get_settings, get_defaults, apply_settings, get_initial_state,
        ])
        .run(tauri::generate_context!())
        .expect("error running tauri app");
}
