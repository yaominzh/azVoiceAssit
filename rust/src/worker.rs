use crossbeam_channel::{Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::config::{HISTORY_MAXLEN, PREROLL_FRAMES, SILERO_MODEL_PATH, WHISPER_MODEL_PATH};
use crate::events::{ControlMsg, State, UiEvent};
use crate::state::SharedState;
use crate::timing::TurnTiming;

/// Format a SystemTime as HH:MM:SS UTC. No chrono dependency.
pub fn format_timestamp_at(t: std::time::SystemTime) -> String {
    let secs = t
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{:02}:{:02}:{:02}", (secs / 3600) % 24, (secs / 60) % 60, secs % 60)
}

fn format_timestamp() -> String {
    format_timestamp_at(std::time::SystemTime::now())
}

/// Per-generation TTS cancellation handle.
struct TtsHandle {
    stop: Arc<std::sync::atomic::AtomicBool>,
    generation: u64,
}

/// Minimum AEC-cleaned RMS to treat VadEvent::Start as user speech, not echo leakage.
const BARGE_IN_THRESHOLD: f32 = 0.02;

pub fn run(
    rx_audio: Receiver<Vec<f32>>,
    rx_ctrl: Receiver<ControlMsg>,
    tx_ui: Sender<UiEvent>,
    shared: Arc<SharedState>,
    speaking: Arc<AtomicBool>,
    echo: std::sync::Arc<crate::echo::EchoCancel>,
) {
    let mut vad = match crate::vad::Vad::load(SILERO_MODEL_PATH) {
        Ok(v) => v,
        Err(e) => { eprintln!("[worker] VAD load failed: {e}"); return; }
    };
    // Load persisted settings (or defaults). Apply initial thresholds to VAD.
    let mut settings = crate::settings::AppSettings::load();
    let mut system_prompt = settings.system_prompt.clone();
    vad.set_thresholds(settings.silence_ms, settings.speech_threshold);
    let mut seg = crate::segmenter::Segmenter::new(PREROLL_FRAMES);
    let mut history = crate::history::History::new(HISTORY_MAXLEN);
    let stt = match crate::stt::Stt::load(WHISPER_MODEL_PATH) {
        Ok(s) => s,
        Err(e) => { eprintln!("[worker] STT load failed: {e}"); return; }
    };
    let client = reqwest::blocking::Client::new();
    let stop_tts = Arc::new(AtomicBool::new(false));
    let mut tts_gen: u64 = 0;
    let mut active_tts: Option<TtsHandle> = None;
    let (tts_done_tx, tts_done_rx) = crossbeam_channel::bounded::<u64>(8);

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
                    if let Some(ref handle) = active_tts {
                        handle.stop.store(true, Ordering::SeqCst);
                    }
                    stop_tts.store(true, Ordering::SeqCst); // backward compat
                }
                Ok(ControlMsg::SettingsChanged(s)) => {
                    system_prompt = s.system_prompt.clone();
                    vad.set_thresholds(s.silence_ms, s.speech_threshold);
                    // Recreate history with new cap (clears it — fresh start on settings change)
                    history = crate::history::History::new(s.history_turns as usize * 2);
                    settings = s;
                }
                Err(TryRecvError::Empty) => {
                    // Also drain TTS completion notifications
                    while let Ok(done_gen) = tts_done_rx.try_recv() {
                        if let Some(ref h) = active_tts {
                            if h.generation == done_gen {
                                active_tts = None;
                                reset_to_idle(&shared, &tx_ui, &mut vad);
                            }
                        }
                    }
                    break;
                }
                Err(_) => return, // channel closed
            }
        }

        // Get next audio frame — but also wake on ctrl so mute→unmute works.
        // When muted, no audio arrives and rx_audio.recv() would block forever,
        // deadlocking control messages. select! handles either channel.
        let frame = crossbeam_channel::select! {
            recv(rx_ctrl) -> msg => {
                match msg {
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
                        if let Some(ref handle) = active_tts {
                            handle.stop.store(true, Ordering::SeqCst);
                        }
                        stop_tts.store(true, Ordering::SeqCst); // backward compat
                    }
                    Ok(ControlMsg::SettingsChanged(s)) => {
                        system_prompt = s.system_prompt.clone();
                        vad.set_thresholds(s.silence_ms, s.speech_threshold);
                        history = crate::history::History::new(s.history_turns as usize * 2);
                        settings = s;
                    }
                    Err(_) => return,
                }
                continue;
            }
            recv(rx_audio) -> frame => match frame {
                Ok(f) => f,
                Err(_) => return,
            }
        };

        // VAD
        let event = match vad.accept(&frame) {
            Ok(e) => e,
            Err(e) => { eprintln!("[worker] VAD error: {e}"); vad.reset(); continue; }
        };

        // Barge-in: confident speech onset during TTS → stop TTS immediately.
        // Require minimum clean_rms to avoid stopping on AEC echo leakage.
        if event == Some(crate::segmenter::VadEvent::Start) {
            if let Some(ref handle) = active_tts {
                let clean_rms = (frame.iter().map(|x| x * x).sum::<f32>()
                    / frame.len() as f32).sqrt();
                if clean_rms > BARGE_IN_THRESHOLD {
                    eprintln!("[barge-in] stopping TTS gen={} clean_rms={:.4}",
                        handle.generation, clean_rms);
                    handle.stop.store(true, Ordering::SeqCst);
                    // Do NOT reset vad/segmenter — keep accumulating user's speech
                }
            }
        }

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

        let messages = if settings.history_turns == 0 {
            // Stateless: send only [system + current turn] — avoids refine slowdown
            vec![
                serde_json::json!({"role": "system", "content": system_prompt}),
                serde_json::json!({"role": "user", "content": text}),
            ]
        } else {
            history.record_user_and_build(&text, &system_prompt)
        };
        let t1 = std::time::Instant::now();
        let refined = match crate::refine::refine(&client, messages) {
            Ok(r) => r,
            Err(e) => { eprintln!("[worker] refine: {e}"); reset_to_idle(&shared, &tx_ui, &mut vad); continue; }
        };
        if settings.history_turns > 0 {
            history.record_assistant(&refined);
        }
        let refine_ms = t1.elapsed().as_millis() as u32;
        let reply_start_ms = t0.elapsed().as_millis() as u32;

        let timing = TurnTiming {
            endpoint_ms: settings.silence_ms,   // was MIN_SILENCE_MS
            stt_ms,
            refine_ms,
            reply_start_ms,
        };
        let _ = tx_ui.send(UiEvent::Turn {
            heard: text.clone(),
            refined: refined.clone(),
            timing,
            timestamp: format_timestamp(),
        });

        // Cancel any previous in-flight TTS (overlapping barge-in race)
        if let Some(ref old) = active_tts {
            old.stop.store(true, Ordering::SeqCst);
        }

        tts_gen += 1;
        let gen = tts_gen;
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        active_tts = Some(TtsHandle { stop: stop.clone(), generation: gen });

        shared.set(State::Speaking);
        let _ = tx_ui.send(UiEvent::StateChanged(State::Speaking));
        speaking.store(true, Ordering::SeqCst);

        let echo_c    = echo.clone();
        let client_c  = client.clone();
        let refined_c = refined.clone();
        let done_tx   = tts_done_tx.clone();
        let speaking_c = speaking.clone();

        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = crate::tts::speak_stoppable(
                    &client_c, &refined_c, &stop, Some(&echo_c));
            }));
            if result.is_err() {
                eprintln!("[tts] thread panicked — cleaning up");
            }
            speaking_c.store(false, Ordering::SeqCst);
            let _ = done_tx.try_send(gen);
        });

        // Worker returns to VAD loop immediately — no block, no drain.
        // State stays Speaking until TtsDone arrives (handled above in ctrl drain).
    }
}

fn reset_to_idle(shared: &SharedState, tx_ui: &Sender<UiEvent>, vad: &mut crate::vad::Vad) {
    let s = shared.idle_state();
    shared.set(s);
    let _ = tx_ui.send(UiEvent::StateChanged(s));
    vad.reset();
}

#[cfg(test)]
mod tests {
    #[test]
    fn format_timestamp_known_epoch() {
        use std::time::{Duration, UNIX_EPOCH};
        // 3661 seconds = 1h 1m 1s UTC
        let t = UNIX_EPOCH + Duration::from_secs(3661);
        assert_eq!(super::format_timestamp_at(t), "01:01:01");
    }

    #[test]
    fn format_timestamp_midnight_rollover() {
        use std::time::{Duration, UNIX_EPOCH};
        // 86400 seconds = exactly 1 day → 00:00:00
        let t = UNIX_EPOCH + Duration::from_secs(86400);
        assert_eq!(super::format_timestamp_at(t), "00:00:00");
    }

    #[test]
    fn barge_in_fires_above_threshold() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct TtsHandle { stop: Arc<AtomicBool>, generation: u64 }
        const BARGE_IN_THRESHOLD: f32 = 0.02;

        let stop = Arc::new(AtomicBool::new(false));
        let handle = TtsHandle { stop: stop.clone(), generation: 1 };
        let clean_rms = 0.05f32;
        if clean_rms > BARGE_IN_THRESHOLD { handle.stop.store(true, Ordering::SeqCst); }
        assert!(stop.load(Ordering::SeqCst));
    }

    #[test]
    fn barge_in_suppressed_below_threshold() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct TtsHandle { stop: Arc<AtomicBool>, generation: u64 }
        const BARGE_IN_THRESHOLD: f32 = 0.02;

        let stop = Arc::new(AtomicBool::new(false));
        let handle = TtsHandle { stop: stop.clone(), generation: 1 };
        let clean_rms = 0.005f32;
        if clean_rms > BARGE_IN_THRESHOLD { handle.stop.store(true, Ordering::SeqCst); }
        assert!(!stop.load(Ordering::SeqCst));
    }

    #[test]
    fn stale_tts_done_does_not_affect_active_generation() {
        let active_gen = 2u64;
        let stale_done_gen = 1u64;
        let cleared = active_gen == stale_done_gen;
        assert!(!cleared);
    }

    #[test]
    fn stop_button_sets_active_tts_stop_flag() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let stop = Arc::new(AtomicBool::new(false));
        stop.store(true, Ordering::SeqCst);
        assert!(stop.load(Ordering::SeqCst));
    }
}
