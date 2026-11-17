//! The Calibration menu: a dialog for the values of a test print, which
//! then replaces the plate with its test models. Slicing the plate gives
//! the calibration G-code.

use eframe::egui::{self, RichText};
use slicer_gcode::calibration::{Calibration, Kind};
use slicer_ui::calibration::{Finish, PlateCalibration};
use slicer_ui::job::{SliceRequest, SliceResult};

use super::{App, Level, Tab};
use crate::theme;

/// State of the Calibration menu.
#[derive(Default)]
pub(super) struct Calibrate {
    /// The open dialog: the kind of print with start, end and step.
    dialog: Option<(Kind, [f64; 3])>,
    /// The calibration whose test models were put on the plate.
    active: Option<PlateCalibration>,
    /// What the G-code of the running slice needs at the end.
    finish: Option<Finish>,
}

/// What the dialog says about a kind of print.
fn help(kind: Kind) -> &'static str {
    match kind {
        Kind::Temperature => {
            "A tower with one block per nozzle temperature, the start value at the bottom. \
             Use the temperature of the block that looks best."
        }
        Kind::Retraction => {
            "Two towers; the retraction length changes every 5 mm of height, the start value \
             at the bottom. Use the shortest length that leaves no strings."
        }
        Kind::Flow => {
            "Flat blocks side by side, each with its own share of the flow ratio. Pick the \
             block with the smoothest top and multiply the flow ratio by 1 + its percentage / 100."
        }
    }
}

impl App {
    /// The Calibration menu of the top bar.
    pub(super) fn calibration_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button(RichText::new("  Calibration  ").color(theme::TEXT), |ui| {
            for kind in Kind::ALL {
                if ui.button(format!("{}…", kind.title())).clicked() {
                    let settings = self.current.as_ref().unwrap_or(&self.base);
                    self.calibrate.dialog = Some((kind, kind.default_range(settings)));
                }
            }
        });
    }

    /// The dialog for start, end and step, if open.
    pub(super) fn calibration_window(&mut self, ctx: &egui::Context) {
        let Some((kind, mut range)) = self.calibrate.dialog else {
            return;
        };
        let (mut open, mut put, mut cancel) = (true, false, false);
        let speed = if kind == Kind::Retraction { 0.05 } else { 1.0 };
        let checked = egui::Window::new(kind.title())
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.set_max_width(320.0);
                ui.label(help(kind));
                egui::Grid::new("calibration_range")
                    .num_columns(2)
                    .show(ui, |ui| {
                        for (value, label) in range.iter_mut().zip(["Start", "End", "Step"]) {
                            ui.label(label);
                            ui.add(
                                egui::DragValue::new(value)
                                    .speed(speed)
                                    .suffix(format!(" {}", kind.unit())),
                            );
                            ui.end_row();
                        }
                    });
                let checked = Calibration::new(kind, range[0], range[1], range[2]);
                match &checked {
                    Ok(calibration) => ui.label(calibration.summary()),
                    Err(error) => ui.colored_label(ui.visuals().warn_fg_color, error.to_string()),
                };
                ui.horizontal(|ui| {
                    put = ui
                        .add_enabled(checked.is_ok(), egui::Button::new("Put on plate"))
                        .on_hover_text("Replaces the models on the plate")
                        .clicked();
                    cancel = ui.button("Cancel").clicked();
                });
                checked
            })
            .and_then(|response| response.inner);
        self.calibrate.dialog = (open && !cancel && !put).then_some((kind, range));
        if put && let Some(Ok(calibration)) = checked {
            self.put_calibration(calibration);
        }
    }

    /// Replaces the plate with the test models of `calibration`.
    fn put_calibration(&mut self, calibration: Calibration) {
        let settings = self.current.as_ref().unwrap_or(&self.base).clone();
        let summary = format!(
            "{}: {}. Slice the plate for its G-code.",
            calibration.kind().title(),
            calibration.summary()
        );
        match PlateCalibration::put_on(calibration, &mut self.plate, &settings) {
            Ok(active) => {
                self.calibrate.active = Some(active);
                self.sliced = None;
                self.job = None;
                self.tab = Tab::Plate;
                self.note(Level::Info, summary);
            }
            Err(error) => self.note(Level::Error, error.message),
        }
    }

    /// `request` with the settings of the calibration whose test models
    /// are on the plate, if any; remembers what its G-code needs at the
    /// end.
    pub(super) fn calibrated(&mut self, mut request: SliceRequest) -> SliceRequest {
        self.calibrate.finish = None;
        let prepared = (self.calibrate.active.as_ref())
            .and_then(|active| active.prepare(&self.plate, &request.settings));
        if let Some((settings, finish)) = prepared {
            request.settings = settings;
            self.calibrate.finish = Some(finish);
        }
        request
    }

    /// `result` with the calibration G-code, if the slice was of test
    /// models.
    pub(super) fn finish_calibration(&mut self, mut result: SliceResult) -> SliceResult {
        if let Some(finish) = self.calibrate.finish.take() {
            finish.apply(&mut result.output);
        }
        result
    }
}
