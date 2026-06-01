use crossbeam_channel::{Receiver, Sender};
use crate::events::{ControlMsg, State, UiEvent};
use crate::settings::AppSettings;

pub struct VoiceApp {
    rx_ui: Receiver<UiEvent>,
    tx_ctrl: Sender<ControlMsg>,
    state: State,
    transcript: Vec<(String, String, String)>, // (heard, refined, timestamp)
    last_timing: Option<String>,
    show_settings: bool,
    draft: AppSettings,
    applied: AppSettings,
}

impl VoiceApp {
    pub fn new(rx_ui: Receiver<UiEvent>, tx_ctrl: Sender<ControlMsg>) -> Self {
        let settings = AppSettings::load();
        Self {
            rx_ui,
            tx_ctrl,
            state: State::Listening,
            transcript: Vec::new(),
            last_timing: None,
            show_settings: false,
            draft: settings.clone(),
            applied: settings,
        }
    }
}

impl eframe::App for VoiceApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(event) = self.rx_ui.try_recv() {
            match event {
                UiEvent::StateChanged(s) => self.state = s,
                UiEvent::Turn { heard, refined, timing, timestamp } => {
                    self.last_timing = Some(timing.format());
                    self.transcript.push((heard, refined, timestamp));
                    if self.transcript.len() > 100 {
                        self.transcript.remove(0);
                    }
                }
                UiEvent::Cleared => {
                    self.transcript.clear();
                    self.last_timing = None;
                }
            }
        }
        ctx.request_repaint();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let t = ui.ctx().input(|i| i.time);

        let color = match self.state {
            State::Listening => egui::Color32::from_rgb(59, 130, 246),
            State::Thinking  => egui::Color32::from_rgb(168, 85, 247),
            State::Speaking  => egui::Color32::from_rgb(34, 197, 94),
            State::Muted     => egui::Color32::from_gray(100),
        };

        let alpha: u8 = match self.state {
            State::Listening => {
                let s = (t as f32 * std::f32::consts::TAU / 1.8).sin();
                ((0.5 + 0.5 * s) * 127.0 + 128.0).min(255.0) as u8
            }
            State::Speaking => {
                let s = (t as f32 * std::f32::consts::TAU / 1.2).sin();
                ((0.5 + 0.5 * s) * 127.0 + 128.0).min(255.0) as u8
            }
            _ => 255,
        };
        let color_a = egui::Color32::from_rgba_unmultiplied(
            color.r(), color.g(), color.b(), alpha,
        );

        ui.vertical_centered(|ui| {
            ui.add_space(24.0);

            let (rect, _) = ui.allocate_exact_size(
                egui::Vec2::splat(120.0),
                egui::Sense::hover(),
            );
            let painter = ui.painter();

            // Draw the taichi glyph
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "\u{262F}",
                egui::FontId::proportional(96.0),
                color_a,
            );

            // Thinking: faster dot-ring (1.0s period, was 1.4s)
            if self.state == State::Thinking {
                let angle = t as f32 * std::f32::consts::TAU / 1.0;
                let radius = 54.0_f32;
                let dot_color = egui::Color32::from_rgba_unmultiplied(168, 85, 247, 200);
                for i in 0..8u32 {
                    let a = angle + i as f32 * std::f32::consts::TAU / 8.0;
                    let dot_pos = rect.center()
                        + egui::Vec2::new(a.cos() * radius, a.sin() * radius);
                    let dot_r = (3.0 - i as f32 * 0.25).max(0.5);
                    painter.circle_filled(dot_pos, dot_r, dot_color);
                }
            }

            ui.add_space(4.0);

            // Status label
            ui.label(
                egui::RichText::new(self.state.label().to_uppercase())
                    .size(13.0)
                    .color(egui::Color32::from_rgba_unmultiplied(229, 231, 235, 178)),
            );

            // Timing badge — shown after first turn, updated each turn
            if let Some(timing) = &self.last_timing {
                ui.label(
                    egui::RichText::new(timing)
                        .monospace()
                        .size(10.0)
                        .color(egui::Color32::from_rgb(16, 185, 129)),
                );
            }

            ui.add_space(8.0);
        });

        // Controls bar + gear — rendered BEFORE the scroll area so it stays pinned
        // at the top of the lower section regardless of transcript length.
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("\u{1F3A4} Mic").clicked() {
                let _ = self.tx_ctrl.send(ControlMsg::ToggleMic);
            }
            if ui.button("\u{23F9} Stop").clicked() {
                let _ = self.tx_ctrl.send(ControlMsg::Stop);
            }
            if ui.button("\u{1F5D1} Clear").clicked() {
                let _ = self.tx_ctrl.send(ControlMsg::Clear);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("\u{2699}").clicked() {
                    self.show_settings = !self.show_settings;
                    if self.show_settings {
                        self.draft = self.applied.clone(); // fresh copy on open
                    }
                }
            });
        });

        // Transcript scroll area fills all remaining space below the controls
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .show(ui, |ui| {
                let width = ui.available_width().min(640.0);
                ui.set_min_width(width);
                for (heard, refined, ts) in &self.transcript {
                    ui.group(|ui| {
                        ui.label(
                            egui::RichText::new(format!("heard  {ts}"))
                                .size(10.0)
                                .color(egui::Color32::from_gray(120)),
                        );
                        ui.label(
                            egui::RichText::new(heard)
                                .size(14.0)
                                .color(egui::Color32::from_gray(200)),
                        );
                    });
                    ui.group(|ui| {
                        ui.label(
                            egui::RichText::new("refined")
                                .size(10.0)
                                .color(egui::Color32::from_gray(120)),
                        );
                        ui.label(
                            egui::RichText::new(refined)
                                .size(14.0)
                                .color(egui::Color32::from_rgb(147, 197, 253)),
                        );
                    });
                    ui.add_space(4.0);
                }
            });

        // Settings — floating popup window (non-modal, draggable, independent of layout).
        // We skip .open() to avoid a double-borrow of &mut self; the Cancel button
        // and Apply close it explicitly. The window is shown only when show_settings=true.
        if self.show_settings {
            egui::Window::new("Settings")
                .resizable(false)
                .default_width(340.0)
                .show(ui.ctx(), |ui| {
                    ui.label(egui::RichText::new("System Prompt").size(11.0).color(egui::Color32::from_gray(160)));
                    ui.add(egui::TextEdit::multiline(&mut self.draft.system_prompt)
                        .desired_rows(4).desired_width(f32::INFINITY));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!("Silence timeout: {} ms", self.draft.silence_ms))
                            .size(11.0).color(egui::Color32::from_gray(160)));
                        ui.label(egui::RichText::new("\u{2139}")   // ℹ
                            .size(11.0).color(egui::Color32::from_rgb(99, 102, 241)))
                            .on_hover_text("How long you must pause before the turn ends.\n300ms = very snappy (cuts off mid-sentence pauses).\n700ms = default, good for normal speech.\n2000–5000ms = allows long pauses mid-thought.");
                    });
                    ui.add(egui::Slider::new(&mut self.draft.silence_ms, 300_u32..=5000).show_value(false));
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(format!("Speech threshold: {:.2}", self.draft.speech_threshold))
                        .size(11.0).color(egui::Color32::from_gray(160)))
                        .on_hover_text("How confident the VAD must be before starting a turn.\n0.5 = Silero default. Lower = more sensitive (picks up quiet speech,\nmore false triggers). Higher = stricter (misses faint speech).");
                    ui.add(egui::Slider::new(&mut self.draft.speech_threshold, 0.1_f32..=0.9_f32).show_value(false));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let label = if self.draft.history_turns == 0 {
                            "Context turns: off (stateless)".to_string()
                        } else {
                            format!("Context turns: {} (last {} turn{})",
                                self.draft.history_turns, self.draft.history_turns,
                                if self.draft.history_turns == 1 { "" } else { "s" })
                        };
                        ui.label(egui::RichText::new(label).size(11.0).color(egui::Color32::from_gray(160)));
                        ui.label(egui::RichText::new("\u{2139}").size(11.0).color(egui::Color32::from_rgb(99, 102, 241)))
                            .on_hover_text("How many past turns to include as context in each refine call.\n0 = stateless (fastest, fixes slowdown after many turns).\n1–5 = conversational context (refine can reference prior turns).\nNote: higher values make oMLX calls slower as sessions grow long.");
                    });
                    ui.add(egui::Slider::new(&mut self.draft.history_turns, 0_u32..=20).show_value(false));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Apply").clicked() {
                            match self.draft.save() {
                                Ok(()) => {
                                    self.applied = self.draft.clone();
                                    let _ = self.tx_ctrl.send(ControlMsg::SettingsChanged(self.draft.clone()));
                                    self.show_settings = false;
                                }
                                Err(e) => eprintln!("Settings save failed: {e}"),
                            }
                        }
                        if ui.button("Defaults").clicked() { self.draft = AppSettings::default(); }
                        if ui.button("Cancel").clicked() { self.show_settings = false; }
                    });
                });
        }
    }
}
