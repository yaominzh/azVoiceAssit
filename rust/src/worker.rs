use crossbeam_channel::{Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::config::{HISTORY_MAXLEN, PREROLL_FRAMES, SILERO_MODEL_PATH, SYSTEM_PROMPT, WHISPER_MODEL_PATH, MIN_SILENCE_MS};
use crate::events::{ControlMsg, State, UiEvent};
use crate::state::SharedState;
use crate::timing::TurnTiming;

pub fn run(
    rx_audio: Receiver<Vec<f32>>,
    rx_ctrl: Receiver<ControlMsg>,
    tx_ui: Sender<UiEvent>,
    shared: Arc<SharedState>,
    speaking: Arc<AtomicBool>,
) {
    let mut vad = match crate::vad::Vad::load(SILERO_MODEL_PATH) {
        Ok(v) => v,
        Err(e) => { eprintln!("[worker] VAD load failed: {e}"); return; }
    };
    let mut seg = crate::segmenter::Segmenter::new(PREROLL_FRAMES);
    let mut history = crate::history::History::new(HISTORY_MAXLEN);
    let stt = match crate::stt::Stt::load(WHISPER_MODEL_PATH) {
        Ok(s) => s,
        Err(e) => { eprintln!("[worker] STT load failed: {e}"); return; }
    };
    let client = reqwest::blocking::Client::new();
    let stop_tts = Arc::new(AtomicBool::new(false));

    loop {
        // Drain control messages first
        loop {
            match rx_ctrl.try_recv() {
                Ok(ControlMsg::ToggleMic) => {
                    if let Some(s) = shared.toggle_mic() {
                        let _ = tx_ui.send(UiEvent::StateChanged(s));
                    }
                }
                Ok(ControlMsg::Clear) => {
                    history.clear();
                    let _ = tx_ui.send(UiEvent::Cleared);
                }
                Ok(ControlMsg::Stop) => {
                    stop_tts.store(true, Ordering::SeqCst);
                }
                Err(TryRecvError::Empty) => break,
                Err(_) => return, // channel closed
            }
        }

        // Get next audio frame
        let frame = match rx_audio.recv() {
            Ok(f) => f,
            Err(_) => return,
        };

        // VAD
        let event = match vad.accept(&frame) {
            Ok(e) => e,
            Err(e) => { eprintln!("[worker] VAD error: {e}"); vad.reset(); continue; }
        };

        // Segmenter
        let utterance = match seg.push(frame, event) {
            Some(u) => u,
            None => continue,
        };

        // Turn pipeline
        shared.set(State::Thinking);
        let _ = tx_ui.send(UiEvent::StateChanged(State::Thinking));

        let t0 = std::time::Instant::now();
        let text = match stt.transcribe(&utterance) {
            Ok(t) => t,
            Err(e) => { eprintln!("[worker] STT: {e}"); reset_to_idle(&shared, &tx_ui, &mut vad); continue; }
        };
        let stt_ms = t0.elapsed().as_millis() as u32;

        if text.is_empty() {
            reset_to_idle(&shared, &tx_ui, &mut vad);
            continue;
        }

        let messages = history.record_user_and_build(&text, SYSTEM_PROMPT);
        let t1 = std::time::Instant::now();
        let refined = match crate::refine::refine(&client, messages) {
            Ok(r) => r,
            Err(e) => { eprintln!("[worker] refine: {e}"); reset_to_idle(&shared, &tx_ui, &mut vad); continue; }
        };
        history.record_assistant(&refined);
        let refine_ms = t1.elapsed().as_millis() as u32;
        let reply_start_ms = t0.elapsed().as_millis() as u32;

        let timing = TurnTiming {
            endpoint_ms: MIN_SILENCE_MS,
            stt_ms,
            refine_ms,
            reply_start_ms,
        };
        let _ = tx_ui.send(UiEvent::Turn {
            heard: text.clone(),
            refined: refined.clone(),
            timing,
        });

        // TTS
        shared.set(State::Speaking);
        let _ = tx_ui.send(UiEvent::StateChanged(State::Speaking));
        speaking.store(true, Ordering::SeqCst);
        stop_tts.store(false, Ordering::SeqCst);

        let _ = crate::tts::speak_stoppable(&client, &refined, &stop_tts, &rx_ctrl);

        speaking.store(false, Ordering::SeqCst);
        reset_to_idle(&shared, &tx_ui, &mut vad);

        // Drain stale frames accumulated during TTS
        while rx_audio.try_recv().is_ok() {}
    }
}

fn reset_to_idle(shared: &SharedState, tx_ui: &Sender<UiEvent>, vad: &mut crate::vad::Vad) {
    let s = shared.idle_state();
    shared.set(s);
    let _ = tx_ui.send(UiEvent::StateChanged(s));
    vad.reset();
}
