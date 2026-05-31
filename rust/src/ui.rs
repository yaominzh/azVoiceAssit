use crossbeam_channel::{Receiver, Sender};
use crate::events::{ControlMsg, State, UiEvent};

pub struct VoiceApp {
    rx_ui: Receiver<UiEvent>,
    tx_ctrl: Sender<ControlMsg>,
    state: State,
    transcript: Vec<(String, String)>, // (heard, refined) pairs
}

impl VoiceApp {
    pub fn new(rx_ui: Receiver<UiEvent>, tx_ctrl: Sender<ControlMsg>) -> Self {
        Self {
            rx_ui,
            tx_ctrl,
            state: State::Listening,
            transcript: Vec::new(),
        }
    }
}

impl eframe::App for VoiceApp {
    /// Drain worker events and request repaint — called before `ui()` each frame.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(event) = self.rx_ui.try_recv() {
            match event {
                UiEvent::StateChanged(s) => self.state = s,
                UiEvent::Turn { heard, refined, timing } => {
                    eprintln!("{}", timing.format());
                    self.transcript.push((heard, refined));
                    if self.transcript.len() > 100 {
                        self.transcript.remove(0);
                    }
                }
                UiEvent::Cleared => self.transcript.clear(),
            }
        }
        // Request continuous repaint for animations
        ctx.request_repaint();
    }

    /// Draw the UI.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // State-dependent color
        let color = match self.state {
            State::Listening => egui::Color32::from_rgb(59, 130, 246),
            State::Thinking  => egui::Color32::from_rgb(168, 85, 247),
            State::Speaking  => egui::Color32::from_rgb(34, 197, 94),
            State::Muted     => egui::Color32::from_gray(100),
        };

        let t = ui.ctx().input(|i| i.time);

        // Alpha pulse for Listening state
        let alpha: u8 = match self.state {
            State::Listening => {
                let s = (t as f32 * std::f32::consts::TAU / 1.8).sin();
                ((0.5 + 0.5 * s) * 127.0 + 128.0).min(255.0) as u8
            }
            _ => 255,
        };
        let color_a = egui::Color32::from_rgba_unmultiplied(
            color.r(), color.g(), color.b(), alpha,
        );

        ui.vertical_centered(|ui| {
            ui.add_space(32.0);

            // Allocate space for the taichi glyph (with optional spinner overlay)
            let (rect, _) = ui.allocate_exact_size(
                egui::Vec2::splat(120.0),
                egui::Sense::hover(),
            );
            let painter = ui.painter();

            // Draw the taichi glyph
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "\u{262F}", // ☯
                egui::FontId::proportional(96.0),
                color_a,
            );

            // For Thinking state: overlay a spinning dot ring
            if self.state == State::Thinking {
                let angle = t as f32 * std::f32::consts::TAU / 1.4;
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

            ui.add_space(16.0);
        });

        // Transcript scroll area
        let screen_height = ui.ctx().content_rect().height();
        let remaining = (screen_height - 220.0).max(100.0);
        egui::ScrollArea::vertical()
            .max_height(remaining)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                let width = ui.available_width().min(640.0);
                ui.set_min_width(width);
                for (heard, refined) in &self.transcript {
                    ui.group(|ui| {
                        ui.label(
                            egui::RichText::new("heard")
                                .size(11.0)
                                .color(egui::Color32::from_gray(140)),
                        );
                        ui.label(
                            egui::RichText::new(heard)
                                .size(14.0)
                                .color(egui::Color32::from_gray(220)),
                        );
                    });
                    ui.group(|ui| {
                        ui.label(
                            egui::RichText::new("refined")
                                .size(11.0)
                                .color(egui::Color32::from_gray(140)),
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

        // Controls bar
        ui.separator();
        ui.horizontal(|ui| {
            let pad = (ui.available_width() - 270.0).max(0.0) / 2.0;
            ui.add_space(pad);
            if ui.button("\u{1F3A4} Mic").clicked() {
                let _ = self.tx_ctrl.send(ControlMsg::ToggleMic);
            }
            if ui.button("Clear").clicked() {
                let _ = self.tx_ctrl.send(ControlMsg::Clear);
            }
            if ui.button("Stop").clicked() {
                let _ = self.tx_ctrl.send(ControlMsg::Stop);
            }
        });
    }
}
