//! The Device tab: sending the slice of the shown plate to a printer that
//! runs Moonraker (Klipper) or OctoPrint.
//!
//! The upload runs on a thread of its own, so the window stays responsive
//! while a large file goes over the network. The browser build has no
//! threads or raw sockets for this and explains that instead.

use eframe::egui::{self, RichText};
#[cfg(not(target_arch = "wasm32"))]
use web_time::Instant;

use crate::theme;

/// A message the Device tab wants the window to show.
#[derive(Debug, Clone)]
// The browser never uploads, so it never makes one.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub enum Message {
    /// What was sent.
    Info(String),
    /// The printer answered, but something needs attention.
    Warning(String),
    /// Nothing was sent, or the answer is an error.
    Error(String),
}

/// What the Device tab keeps: the printer address, the key, and the upload
/// in progress.
pub struct Device {
    /// Address as written, such as `moonraker://192.168.1.20:7125`.
    pub address: String,
    /// API key; never shown in clear.
    pub api_key: String,
    /// Whether the printer should start the file after the upload.
    pub start: bool,
    /// The upload in progress.
    #[cfg(not(target_arch = "wasm32"))]
    job: Option<Job>,
    /// The outcome of the last upload, shown below the button.
    last: Option<(bool, String)>,
}

#[cfg(not(target_arch = "wasm32"))]
struct Job {
    result: std::sync::mpsc::Receiver<
        Result<(slicer_send::Sent, Vec<slicer_core::Diagnostic>), slicer_core::Diagnostic>,
    >,
    started: Instant,
}

impl Device {
    /// The tab with the settings of the last run.
    pub fn new(address: String, api_key: String, start: bool) -> Self {
        Self {
            address,
            api_key,
            start,
            #[cfg(not(target_arch = "wasm32"))]
            job: None,
            last: None,
        }
    }

    /// The address, key and switch to keep between runs.
    pub fn stored(&self) -> (&str, &str, bool) {
        (&self.address, &self.api_key, self.start)
    }

    /// Draws the tab. `gcode` is the current slice's G-code and the name to
    /// send it as; `current` says whether the slice matches the plate.
    /// Returns what the window should tell the user.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        gcode: Option<(&[u8], String)>,
        current: bool,
    ) -> Vec<Message> {
        let mut messages = Vec::new();
        self.poll(&mut messages);
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            ui.add_space(18.0);
            ui.vertical(|ui| {
                ui.set_max_width(620.0);
                ui.heading("Device");
                ui.label(
                    RichText::new(
                        "Send the G-code of this plate to a printer that runs Moonraker (Klipper) or OctoPrint.",
                    )
                    .color(theme::TEXT_WEAK),
                );
                ui.add_space(12.0);
                egui::Grid::new("device_address")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Printer");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.address)
                                .hint_text("moonraker://192.168.1.20:7125")
                                .desired_width(430.0),
                        )
                        .on_hover_text(
                            "moonraker://host:port or octoprint://host:port; add +https for TLS",
                        );
                        ui.end_row();
                        ui.label("API key");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.api_key)
                                .password(true)
                                .hint_text("optional")
                                .desired_width(430.0),
                        )
                        .on_hover_text("Kept on this computer only, never written into a project");
                        ui.end_row();
                    });
                ui.add_space(6.0);
                ui.checkbox(&mut self.start, "Start printing after the upload");
                ui.add_space(10.0);
                self.controls(ui, gcode, current, &mut messages);
                ui.add_space(16.0);
                ui.label(
                    RichText::new(
                        "The file is uploaded to the printer's own G-code folder. Sending does not save a project.",
                    )
                    .small()
                    .color(theme::TEXT_WEAK),
                );
            });
        });
        messages
    }
}

impl Device {
    /// The send button, its progress and the state of the slice.
    fn controls(
        &mut self,
        ui: &mut egui::Ui,
        gcode: Option<(&[u8], String)>,
        current: bool,
        messages: &mut Vec<Message>,
    ) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let busy = self.job.is_some();
            let enabled = gcode.is_some() && !busy;
            let label = if busy { "Sending…" } else { "Send G-code" };
            let button = egui::Button::new(RichText::new(label).color(egui::Color32::WHITE))
                .fill(theme::ACCENT);
            if ui.add_enabled(enabled, button).clicked()
                && let Some((bytes, file)) = &gcode
                && let Err(message) = self.start_send(bytes.to_vec(), file.clone(), ui.ctx())
            {
                messages.push(Message::Error(message));
            }
            if let Some(job) = &self.job {
                ui.spinner();
                ui.label(
                    RichText::new(format!(
                        "Uploading… {:.1} s",
                        job.started.elapsed().as_secs_f64()
                    ))
                    .small()
                    .color(theme::TEXT_WEAK),
                );
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(250));
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = messages;
            ui.label(
                RichText::new(
                    "Sending to a printer is available in the desktop app, not in the browser.",
                )
                .color(ui.visuals().warn_fg_color),
            );
        }

        if gcode.is_none() {
            ui.add_space(6.0);
            ui.label(
                RichText::new("Slice the plate first; there is no G-code to send yet.")
                    .small()
                    .color(theme::TEXT_WEAK),
            );
        } else if !current {
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "The slice is older than the plate. Slice again to send the current version.",
                )
                .small()
                .color(ui.visuals().warn_fg_color),
            );
        }
        if let Some((good, text)) = &self.last {
            ui.add_space(8.0);
            let color = if *good {
                theme::TEXT
            } else {
                ui.visuals().error_fg_color
            };
            ui.label(RichText::new(text).color(color));
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Device {
    /// Starts an upload in the background.
    fn start_send(
        &mut self,
        gcode: Vec<u8>,
        file: String,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        use slicer_core::Diagnostics;
        use slicer_send::Target;

        let target = Target::parse(&self.address)
            .map_err(|error| error.message)?
            .with_api_key(Some(self.api_key.clone()));
        let start = self.start;
        let (sender, result) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut warnings = Diagnostics::new();
            let outcome = slicer_send::upload(&target, &file, &gcode, start, &mut warnings)
                .map(|sent| (sent, warnings.as_slice().to_vec()));
            let _ = sender.send(outcome);
            ctx.request_repaint();
        });
        self.job = Some(Job {
            result,
            started: Instant::now(),
        });
        Ok(())
    }

    /// Takes the outcome of a finished upload, if there is one.
    fn poll(&mut self, messages: &mut Vec<Message>) {
        use std::sync::mpsc::TryRecvError;

        let Some(job) = &self.job else {
            return;
        };
        let outcome = match job.result.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.job = None;
                let text = "The upload stopped unexpectedly.".to_owned();
                self.last = Some((false, text.clone()));
                messages.push(Message::Error(text));
                return;
            }
        };
        self.job = None;
        match outcome {
            Ok((sent, warnings)) => {
                let text = if sent.started {
                    format!("{} was uploaded and started on the printer.", sent.file)
                } else {
                    format!("{} was uploaded to the printer.", sent.file)
                };
                self.last = Some((true, text.clone()));
                messages.push(Message::Info(text));
                for warning in warnings {
                    self.last = Some((true, warning.message.clone()));
                    messages.push(Message::Warning(warning.message));
                }
            }
            Err(error) => {
                self.last = Some((false, error.message.clone()));
                messages.push(Message::Error(error.message));
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl Device {
    /// Nothing is uploaded in the browser, so there is never a result.
    fn poll(&mut self, _messages: &mut Vec<Message>) {}
}
