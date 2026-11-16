//! The window: side panel with model, settings and slicing, and a plate
//! view that shows either the model's outline or one sliced layer.

use std::path::{Path, PathBuf};

use eframe::egui::{
    self, Align2, Color32, FontId, PointerButton, Pos2, Rect, RichText, Sense, Shape, Stroke, Vec2,
};
use slicer_core::{Diagnostic, Severity};
use slicer_profiles::Settings;
use slicer_ui::job::{
    SliceJob, SliceRequest, SliceResult, encode_output, format_duration, output_name,
    with_output_extension,
};
use slicer_ui::plate::{
    Placement, Plate, SCALE_RANGE, bed_center, check_placement, load_model, placed_outline,
};
use slicer_ui::profiles::replace_profiles;
use slicer_ui::settings::{CommonSettings, SettingsFile, load_settings};
use slicer_ui::settings_list::SettingsEditor;
use slicer_ui::view::{PlateView, kind_name, kinds_in, path_color};

use crate::platform;
use crate::settings_ui::{ProfileAction, Profiles, progress_bar, settings_window};
use crate::view3d::{Scene, View3d};

/// How serious a message in the side panel is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Info,
    Warning,
    Error,
}

/// A line in the message list.
#[derive(Debug, Clone)]
struct Note {
    level: Level,
    text: String,
}

impl From<&Diagnostic> for Note {
    fn from(diagnostic: &Diagnostic) -> Self {
        Self {
            level: match diagnostic.severity {
                Severity::Warning => Level::Warning,
                Severity::Error => Level::Error,
            },
            text: diagnostic.message.clone(),
        }
    }
}

/// What the central area shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Plate,
    Preview,
}

/// A finished slice and what it was made from.
struct Sliced {
    result: SliceResult,
    settings: Settings,
    placements: Vec<Placement>,
    serial: u64,
}

/// What a drag in the plate view does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Drag {
    Model,
    Pan,
}

/// The application state.
pub struct App {
    plate: Plate,
    files: Vec<(PathBuf, SettingsFile)>,
    /// Settings from the files, before edits.
    base: Settings,
    common: CommonSettings,
    overrides: String,
    /// The settings to slice with, rebuilt every frame.
    current: Result<Settings, Diagnostic>,
    job: Option<SliceJob>,
    sliced: Option<Sliced>,
    /// Shown layer, counted from 0.
    layer: usize,
    tab: Tab,
    view: PlateView,
    drag: Option<Drag>,
    notes: Vec<Note>,
    /// 3D view instead of the 2D plate.
    three_d: bool,
    view3d: View3d,
    /// Shipped profiles to pick from.
    profiles: Profiles,
    /// Edits in the list of all settings.
    editor: SettingsEditor,
    /// Whether the list of all settings is open.
    all_settings: bool,
}

impl App {
    /// A window with nothing loaded; `paths` are opened as if dropped onto
    /// it (models and `.toml` settings files).
    pub fn new(paths: &[PathBuf]) -> Self {
        let base = Settings::default();
        let mut app = Self {
            plate: Plate::default(),
            files: Vec::new(),
            common: CommonSettings::from_settings(&base),
            current: Ok(base.clone()),
            base,
            overrides: String::new(),
            job: None,
            sliced: None,
            layer: 0,
            tab: Tab::Plate,
            view: PlateView::default(),
            drag: None,
            notes: Vec::new(),
            three_d: false,
            view3d: View3d::default(),
            profiles: Profiles::find(),
            editor: SettingsEditor::default(),
            all_settings: false,
        };
        app.open_paths(paths);
        app
    }

    fn note(&mut self, level: Level, text: impl Into<String>) {
        self.notes.push(Note {
            level,
            text: text.into(),
        });
    }

    fn bed(&self) -> [f64; 2] {
        let settings = self.current.as_ref().unwrap_or(&self.base);
        [settings.bed_size_x, settings.bed_size_y]
    }

    /// Opens models and settings files, told apart by their extension.
    fn open_paths(&mut self, paths: &[PathBuf]) {
        let (settings, models): (Vec<&PathBuf>, Vec<&PathBuf>) = paths.iter().partition(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"))
        });
        if !settings.is_empty() {
            let settings: Vec<PathBuf> = settings.into_iter().cloned().collect();
            self.add_settings_files(&settings);
        }
        for path in models {
            self.open_model(path);
        }
    }

    fn open_model(&mut self, path: &Path) {
        let name = file_name(path);
        let loaded = platform::read(path)
            .map_err(|e| format!("{}: {e}", path.display()))
            .and_then(|bytes| {
                load_model(&name, &bytes).map_err(|e| format!("{name}: {}", e.message))
            });
        match loaded {
            Ok((model, warnings)) => {
                self.note(
                    Level::Info,
                    format!("Opened {name} ({} triangles)", model.mesh.triangles.len()),
                );
                self.notes
                    .extend(warnings.as_slice().iter().map(Note::from));
                let settings = self.current.as_ref().unwrap_or(&self.base);
                self.plate.add(model, settings);
                self.sliced = None;
                self.job = None;
                self.tab = Tab::Plate;
            }
            Err(text) => self.note(Level::Error, text),
        }
    }

    /// Adds settings files after the ones already loaded. Nothing changes
    /// if one of them cannot be read or applied.
    fn add_settings_files(&mut self, paths: &[PathBuf]) {
        let mut all: Vec<PathBuf> = self.files.iter().map(|(path, _)| path.clone()).collect();
        all.extend_from_slice(paths);
        self.read_settings_files(&all);
    }

    /// Reads all settings files again, for changes made in an editor.
    /// Nothing changes if a file is gone or broken now.
    fn reload_settings_files(&mut self) {
        let all: Vec<PathBuf> = self.files.iter().map(|(path, _)| path.clone()).collect();
        self.read_settings_files(&all);
    }

    fn read_settings_files(&mut self, paths: &[PathBuf]) {
        let mut files = Vec::new();
        for path in paths {
            match platform::read_to_string(path) {
                Ok(text) => files.push((
                    path.clone(),
                    SettingsFile {
                        name: file_name(path),
                        text,
                    },
                )),
                Err(e) => {
                    self.note(Level::Error, format!("{}: {e}", path.display()));
                    return;
                }
            }
        }
        self.use_settings_files(files);
    }

    fn use_settings_files(&mut self, files: Vec<(PathBuf, SettingsFile)>) {
        let list: Vec<SettingsFile> = files.iter().map(|(_, file)| file.clone()).collect();
        match load_settings(&list) {
            Ok(settings) => {
                // A model in the middle of the old bed goes to the middle of
                // the new one.
                for item in &mut self.plate.items {
                    if item.placement.center == bed_center(&self.base) {
                        item.placement.center = bed_center(&settings);
                    }
                }
                self.common = CommonSettings::from_settings(&settings);
                self.base = settings;
                self.files = files;
                let names: Vec<&str> = list.iter().map(|f| f.name.as_str()).collect();
                if !names.is_empty() {
                    self.note(Level::Info, format!("Settings: {}", names.join(", ")));
                }
            }
            Err(error) => self.note(Level::Error, error.message),
        }
    }

    /// Why slicing is not possible now, if it is not.
    fn slice_blocker(&self) -> Option<String> {
        let settings = match &self.current {
            Ok(settings) => settings,
            Err(error) if !self.plate.items.is_empty() => return Some(error.message.clone()),
            Err(_) => &self.base,
        };
        self.plate.check(settings).err().map(|error| error.message)
    }

    fn start_slice(&mut self, ctx: &egui::Context) {
        let Ok(settings) = &self.current else {
            return;
        };
        let request = SliceRequest {
            objects: self.plate.meshes(),
            settings: settings.clone(),
        };
        let ctx = ctx.clone();
        self.job = Some(SliceJob::start(request, move || ctx.request_repaint()));
    }

    fn poll_job(&mut self) {
        let Some(outcome) = self.job.as_ref().and_then(SliceJob::try_result) else {
            return;
        };
        let elapsed = self.job.as_ref().map(SliceJob::elapsed).unwrap_or_default();
        self.job = None;
        match outcome {
            Ok(result) => {
                self.note(
                    Level::Info,
                    format!(
                        "Sliced {} layers in {:.1} s",
                        result.output.stats.layers,
                        elapsed.as_secs_f64()
                    ),
                );
                self.notes.extend(result.warnings.iter().map(Note::from));
                self.view3d.set_toolpaths(&result.toolpaths);
                if let Ok(settings) = &self.current {
                    self.sliced = Some(Sliced {
                        result,
                        settings: settings.clone(),
                        placements: self.plate.placements(),
                        serial: self.plate.serial,
                    });
                    self.layer = 0;
                    self.tab = Tab::Preview;
                }
            }
            Err(error) if error.code == "slicing.cancelled" => {
                self.note(Level::Info, "Slicing cancelled");
            }
            Err(error) => self.notes.push(Note::from(&error)),
        }
    }

    /// Whether the shown slice was made from the current model, placement
    /// and settings.
    fn slice_is_current(&self) -> bool {
        self.sliced.as_ref().is_some_and(|sliced| {
            sliced.serial == self.plate.serial
                && sliced.placements == self.plate.placements()
                && self.current.as_ref() == Ok(&sliced.settings)
        })
    }

    fn save(&mut self, gcode_3mf: bool) {
        let Some(sliced) = &self.sliced else {
            return;
        };
        let model_name = self.plate.items.first().map_or("model", |i| &i.model.name);
        let (label, extension) = if gcode_3mf {
            ("Bambu Lab project", "gcode.3mf")
        } else {
            ("G-code", "gcode")
        };
        let Some(path) = platform::save_path(&output_name(model_name, gcode_3mf), label, extension)
        else {
            return;
        };
        let path = PathBuf::from(with_output_extension(&path.to_string_lossy(), gcode_3mf));
        let name = path.to_string_lossy().into_owned();
        let written = encode_output(&sliced.result.output, &sliced.settings, &name)
            .map_err(|e| e.message)
            .and_then(|bytes| platform::write(&path, &bytes).map_err(|e| format!("{name}: {e}")));
        match written {
            Ok(()) => self.note(Level::Info, format!("Saved {name}")),
            Err(text) => self.note(Level::Error, text),
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = platform::opened_files(ctx);
        if !dropped.is_empty() {
            self.open_paths(&dropped);
        }
    }

    fn model_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Models");
        let bed = self.bed();
        ui.horizontal(|ui| {
            if ui.button("Add…").clicked() {
                let paths = platform::pick_files(ui.ctx(), "Models", &["stl", "3mf"], true);
                for path in paths {
                    self.open_model(&path);
                }
            }
            let arrange = egui::Button::new("Arrange");
            if ui
                .add_enabled(!self.plate.items.is_empty(), arrange)
                .on_hover_text("Place all models side by side")
                .clicked()
            {
                self.plate
                    .arrange(self.current.as_ref().unwrap_or(&self.base));
            }
            ui.weak("or drop files here");
        });
        let mut clicked = None;
        for (index, item) in self.plate.items.iter().enumerate() {
            let (s, size) = (item.placement.scale, item.model.size);
            let text = format!(
                "{}: {:.1} × {:.1} × {:.1} mm",
                item.model.name,
                size[0] * s,
                size[1] * s,
                size[2] * s
            );
            if ui
                .selectable_label(self.plate.selected == Some(index), text)
                .clicked()
            {
                clicked = Some(index);
            }
        }
        if clicked.is_some() {
            self.plate.selected = clicked;
        }
        let Some(item) = self.plate.selected_mut() else {
            return;
        };
        let placement = &mut item.placement;
        egui::Grid::new("placement").num_columns(2).show(ui, |ui| {
            ui.label("X");
            ui.add(
                egui::DragValue::new(&mut placement.center[0])
                    .speed(0.5)
                    .range(0.0..=bed[0])
                    .suffix(" mm"),
            );
            ui.end_row();
            ui.label("Y");
            ui.add(
                egui::DragValue::new(&mut placement.center[1])
                    .speed(0.5)
                    .range(0.0..=bed[1])
                    .suffix(" mm"),
            );
            ui.end_row();
            ui.label("Rotation");
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut placement.rotation_deg)
                        .speed(1.0)
                        .range(-360.0..=360.0)
                        .suffix("°"),
                );
                if ui.small_button("−90°").clicked() {
                    placement.rotation_deg = (placement.rotation_deg - 90.0) % 360.0;
                }
                if ui.small_button("+90°").clicked() {
                    placement.rotation_deg = (placement.rotation_deg + 90.0) % 360.0;
                }
            });
            ui.end_row();
            ui.label("Scale");
            let mut percent = placement.scale * 100.0;
            let (min, max) = SCALE_RANGE;
            if ui
                .add(
                    egui::DragValue::new(&mut percent)
                        .speed(1.0)
                        .range(min * 100.0..=max * 100.0)
                        .suffix(" %"),
                )
                .changed()
            {
                placement.scale = percent / 100.0;
            }
            ui.end_row();
        });
        let mut delete = false;
        ui.horizontal(|ui| {
            if ui.button("Center").clicked() {
                placement.center = [bed[0] / 2.0, bed[1] / 2.0];
            }
            if ui.button("Reset").clicked() {
                *placement = Placement {
                    center: [bed[0] / 2.0, bed[1] / 2.0],
                    rotation_deg: 0.0,
                    scale: 1.0,
                };
            }
            delete = ui.button("Delete").clicked();
        });
        if delete {
            self.plate.remove_selected();
        }
    }

    fn settings_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        let paths: Vec<PathBuf> = self.files.iter().map(|(path, _)| path.clone()).collect();
        match self.profiles.ui(ui, &paths) {
            Some(ProfileAction::Use { old, new }) => {
                self.read_settings_files(&replace_profiles(&paths, &old, &new));
            }
            Some(ProfileAction::Imported { path, notes }) => {
                self.note(Level::Info, format!("Imported {}", path.display()));
                for text in notes {
                    self.note(Level::Warning, text);
                }
                self.add_settings_files(&[path]);
            }
            Some(ProfileAction::Error(text)) => self.note(Level::Error, text),
            None => {}
        }
        let mut remove = None;
        for (index, (path, file)) in self.files.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui.small_button("Remove").clicked() {
                    remove = Some(index);
                }
                ui.label(&file.name)
                    .on_hover_text(path.display().to_string());
            });
        }
        if let Some(index) = remove {
            let mut files = self.files.clone();
            files.remove(index);
            self.use_settings_files(files);
        }
        ui.horizontal(|ui| {
            if ui
                .button("Add file…")
                .on_hover_text("Settings in TOML; later files override earlier ones")
                .clicked()
            {
                let paths = platform::pick_files(ui.ctx(), "Settings", &["toml"], true);
                if !paths.is_empty() {
                    self.add_settings_files(&paths);
                }
            }
            if !self.files.is_empty() && ui.button("Reload").clicked() {
                self.reload_settings_files();
            }
        });
        let base = &self.base;
        ui.weak(format!(
            "Bed {:.0} × {:.0} × {:.0} mm, nozzle {} mm",
            base.bed_size_x, base.bed_size_y, base.max_print_height, base.nozzle_diameter
        ));

        let from_files = CommonSettings::from_settings(base);
        let common = &mut self.common;
        egui::Grid::new("common").num_columns(3).show(ui, |ui| {
            ui.label("Layer height");
            ui.add(
                egui::DragValue::new(&mut common.layer_height)
                    .speed(0.01)
                    .range(0.02..=base.nozzle_diameter)
                    .max_decimals(3)
                    .suffix(" mm"),
            );
            changed_marker(ui, &mut common.layer_height, from_files.layer_height, |v| {
                format!("{v} mm")
            });
            ui.end_row();
            ui.label("Infill");
            ui.add(egui::Slider::new(&mut common.infill_density, 0.0..=100.0).suffix(" %"));
            changed_marker(
                ui,
                &mut common.infill_density,
                from_files.infill_density,
                |v| format!("{v} %"),
            );
            ui.end_row();
            ui.label("Walls");
            ui.add(egui::DragValue::new(&mut common.wall_loops).range(0..=20));
            changed_marker(ui, &mut common.wall_loops, from_files.wall_loops, |v| {
                v.to_string()
            });
            ui.end_row();
            ui.label("Supports");
            ui.checkbox(&mut common.supports, "");
            changed_marker(ui, &mut common.supports, from_files.supports, |v| {
                if v { "on" } else { "off" }.to_owned()
            });
            ui.end_row();
        });
        ui.label("More settings, one key = value per line:");
        ui.add(
            egui::TextEdit::multiline(&mut self.overrides)
                .hint_text("brim_width = 5\nnozzle_temp = 220")
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .font(egui::TextStyle::Monospace),
        );
        let edits = self.editor.edit_count();
        let label = if edits == 0 {
            "All settings…".to_owned()
        } else {
            format!("All settings… ({edits} edited)")
        };
        if ui.button(label).clicked() {
            self.all_settings = !self.all_settings;
        }
        if let Err(error) = &self.current {
            ui.colored_label(ui.visuals().error_fg_color, &error.message);
        }
    }

    fn slice_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Slice");
        if let Some(job) = &self.job {
            if job.is_cancelling() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Cancelling");
                });
            } else {
                let (step, fraction) = job.progress();
                progress_bar(ui, step, fraction, job.elapsed().as_secs_f64());
            }
            if ui
                .add_enabled(!job.is_cancelling(), egui::Button::new("Cancel"))
                .clicked()
            {
                job.cancel();
            }
        } else {
            let blocker = self.slice_blocker();
            let button = ui.add_enabled(
                blocker.is_none(),
                egui::Button::new(RichText::new("Slice").strong()).min_size(Vec2::new(120.0, 28.0)),
            );
            if button.clicked() {
                self.start_slice(ui.ctx());
            }
            if let Some(text) = blocker {
                ui.colored_label(ui.visuals().warn_fg_color, text);
            }
        }
        let current = self.slice_is_current();
        let Some(sliced) = &self.sliced else {
            return;
        };
        let stats = &sliced.result.output.stats;
        ui.label(format!(
            "{} layers, {}, {:.2} m / {:.1} g filament",
            stats.layers,
            format_duration(stats.print_time_s),
            stats.filament_length_mm / 1000.0,
            stats.filament_weight_g
        ));
        if !current {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "Model, placement or settings changed since this slice.",
            );
        }
        ui.horizontal(|ui| {
            if ui.button("Save G-code…").clicked() {
                self.save(false);
            }
            if ui
                .button("Save .gcode.3mf…")
                .on_hover_text("Project file for Bambu Lab printers (not yet tested on a printer)")
                .clicked()
            {
                self.save(true);
            }
        });
    }

    fn messages_section(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Messages");
            if !self.notes.is_empty() && ui.small_button("Clear").clicked() {
                self.notes.clear();
            }
        });
        let visuals = ui.visuals().clone();
        for note in self.notes.iter().rev() {
            let (color, prefix) = match note.level {
                Level::Info => (visuals.text_color(), ""),
                Level::Warning => (visuals.warn_fg_color, "Warning: "),
                Level::Error => (visuals.error_fg_color, "Error: "),
            };
            ui.add(
                egui::Label::new(RichText::new(format!("{prefix}{}", note.text)).color(color))
                    .wrap(),
            );
        }
    }

    /// The bed with the model outline or a sliced layer, with zoom and pan.
    fn plate(&mut self, ui: &mut egui::Ui) {
        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let area = response.rect;
        let bed = self.bed();
        let (area_min, area_size) = ([area.min.x, area.min.y], [area.width(), area.height()]);

        if response.hovered() {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta().y, i.zoom_delta()));
            let factor = (scroll / 200.0).exp() * zoom;
            if (factor - 1.0).abs() > 1e-4
                && let Some(cursor) = response.hover_pos()
            {
                self.view
                    .zoom_at(factor, [cursor.x, cursor.y], area_min, area_size, bed);
            }
        }
        if response.double_clicked() {
            self.view = PlateView::default();
        }
        let frame = self.view.frame(area_min, area_size, bed);
        let outlines: Vec<Vec<[f64; 2]>> = self
            .plate
            .items
            .iter()
            .map(|item| placed_outline(&item.model, &item.placement))
            .collect();
        let under_pointer = response
            .interact_pointer_pos()
            .and_then(|pos| self.plate.at(frame.to_bed([pos.x, pos.y])));
        if response.clicked() && self.tab == Tab::Plate {
            self.plate.selected = under_pointer;
        }
        if response.drag_started() {
            let primary = response.dragged_by(PointerButton::Primary);
            self.drag = Some(match under_pointer {
                Some(index) if primary && self.tab == Tab::Plate => {
                    self.plate.selected = Some(index);
                    Drag::Model
                }
                _ => Drag::Pan,
            });
        }
        if response.dragged() {
            let delta = response.drag_delta();
            match self.drag {
                Some(Drag::Model) => {
                    if let Some(item) = self.plate.selected_mut() {
                        let center = &mut item.placement.center;
                        center[0] =
                            (center[0] + f64::from(delta.x / frame.scale)).clamp(0.0, bed[0]);
                        center[1] =
                            (center[1] - f64::from(delta.y / frame.scale)).clamp(0.0, bed[1]);
                    }
                }
                Some(Drag::Pan) | None => {
                    self.view.pan[0] += delta.x;
                    self.view.pan[1] += delta.y;
                }
            }
        }
        if response.drag_stopped() {
            self.drag = None;
        }

        let to_screen = |p: [f64; 2]| {
            let [x, y] = frame.to_screen(p);
            Pos2::new(x, y)
        };
        let painter = painter.with_clip_rect(area);
        let visuals = ui.visuals();
        painter.rect_filled(area, 0.0, visuals.extreme_bg_color);
        let bed_rect = Rect::from_two_pos(to_screen([0.0, 0.0]), to_screen(bed));
        painter.rect_filled(bed_rect, 2.0, visuals.faint_bg_color);
        let grid = Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color);
        for step in 1..(bed[0] / 10.0).ceil() as usize {
            let x = step as f64 * 10.0;
            painter.line_segment([to_screen([x, 0.0]), to_screen([x, bed[1]])], grid);
        }
        for step in 1..(bed[1] / 10.0).ceil() as usize {
            let y = step as f64 * 10.0;
            painter.line_segment([to_screen([0.0, y]), to_screen([bed[0], y])], grid);
        }
        painter.rect_stroke(
            bed_rect,
            2.0,
            Stroke::new(1.5, visuals.widgets.inactive.fg_stroke.color),
            egui::StrokeKind::Outside,
        );

        match self.tab {
            Tab::Plate => self.paint_outlines(&painter, &outlines, &to_screen),
            Tab::Preview => self.paint_layer(&painter, frame.scale, &to_screen),
        }

        let hint = match self.tab {
            Tab::Plate => {
                "Click a model to select it, drag to move it · scroll to zoom · drag elsewhere to pan · double-click to fit"
            }
            Tab::Preview => {
                "Scroll to zoom · drag to pan · double-click to fit · arrow keys change the layer"
            }
        };
        painter.text(
            area.left_bottom() + Vec2::new(8.0, -8.0),
            Align2::LEFT_BOTTOM,
            hint,
            FontId::proportional(12.0),
            visuals.weak_text_color(),
        );
    }

    fn paint_outlines(
        &self,
        painter: &egui::Painter,
        outlines: &[Vec<[f64; 2]>],
        to_screen: &dyn Fn([f64; 2]) -> Pos2,
    ) {
        let Ok(settings) = &self.current else {
            return;
        };
        for (index, (item, outline)) in self.plate.items.iter().zip(outlines).enumerate() {
            if outline.len() < 3 {
                continue;
            }
            let fits = check_placement(&item.model, &item.placement, settings).is_ok();
            let [r, g, b] = if fits { [76, 141, 255] } else { [230, 60, 60] };
            let selected = self.plate.selected == Some(index);
            let (alpha, width) = if selected { (110, 2.5) } else { (50, 1.0) };
            let line = Color32::from_rgb(r, g, b);
            let fill = Color32::from_rgba_unmultiplied(r, g, b, alpha);
            let points: Vec<Pos2> = outline.iter().map(|&p| to_screen(p)).collect();
            painter.add(Shape::convex_polygon(
                points,
                fill,
                Stroke::new(width, line),
            ));
            if selected {
                painter.circle_filled(to_screen(item.placement.center), 3.0, line);
            }
        }
    }

    fn paint_layer(
        &self,
        painter: &egui::Painter,
        scale: f32,
        to_screen: &dyn Fn([f64; 2]) -> Pos2,
    ) {
        let Some(sliced) = &self.sliced else {
            return;
        };
        let Some(layer) = sliced.result.toolpaths.layers.get(self.layer) else {
            return;
        };
        let width = (sliced.settings.line_width as f32 * scale).max(1.0);
        for path in &layer.paths {
            let [r, g, b] = path_color(path.kind);
            let stroke = Stroke::new(width, Color32::from_rgb(r, g, b));
            let points: Vec<Pos2> = path.points.iter().map(|&p| to_screen(p)).collect();
            painter.add(if path.closed {
                Shape::closed_line(points, stroke)
            } else {
                Shape::line(points, stroke)
            });
        }
    }

    /// Layer slider and legend above the preview.
    fn preview_controls(&mut self, ui: &mut egui::Ui) {
        let Some(sliced) = &self.sliced else {
            ui.label("Slice the model to see the layers.");
            return;
        };
        let layers = &sliced.result.toolpaths.layers;
        if layers.is_empty() {
            return;
        }
        let last = layers.len() - 1;
        let (up, down) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
            )
        });
        if !ui.ctx().egui_wants_keyboard_input() {
            if up {
                self.layer = (self.layer + 1).min(last);
            }
            if down {
                self.layer = self.layer.saturating_sub(1);
            }
        }
        self.layer = self.layer.min(last);
        let mut number = self.layer + 1;
        ui.horizontal(|ui| {
            ui.spacing_mut().slider_width = (ui.available_width() - 260.0).max(100.0);
            ui.add(egui::Slider::new(&mut number, 1..=layers.len()).text("Layer"));
            let layer = &layers[number - 1];
            ui.label(format!("Z {:.2} mm", layer.z));
        });
        self.layer = number - 1;
        ui.horizontal_wrapped(|ui| {
            for kind in kinds_in(&layers[self.layer]) {
                let [r, g, b] = path_color(kind);
                let (swatch, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                ui.painter()
                    .rect_filled(swatch, 2.0, Color32::from_rgb(r, g, b));
                ui.label(kind_name(kind));
                ui.add_space(8.0);
            }
        });
    }
}

impl App {
    /// The 3D view: the model on the plate, or the toolpaths up to the
    /// chosen layer in the preview.
    fn show_3d(&mut self, ui: &mut egui::Ui) {
        let settings = self.current.as_ref().unwrap_or(&self.base);
        let models = self
            .plate
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let fits = check_placement(&item.model, &item.placement, settings).is_ok();
                let selected = self.plate.selected == Some(index);
                (&item.model, item.placement, selected, fits)
            })
            .collect();
        let layer = (self.tab == Tab::Preview && self.sliced.is_some()).then_some(self.layer);
        let scene = Scene {
            bed: [settings.bed_size_x, settings.bed_size_y],
            height: settings.max_print_height,
            serial: self.plate.serial,
            models,
            layer,
        };
        self.view3d.show(ui, &scene);
    }
}

/// A reset button next to a value that differs from the settings files.
fn changed_marker<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    value: &mut T,
    from_files: T,
    show: impl Fn(T) -> String,
) {
    if *value == from_files {
        ui.label("");
    } else if ui
        .small_button("reset")
        .on_hover_text(format!("Changed; back to {}", show(from_files)))
        .clicked()
    {
        *value = from_files;
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_job();
        self.handle_dropped_files(&ctx);
        self.current = self.editor.build(&self.base, &self.common, &self.overrides);

        egui::Panel::left("side")
            .resizable(true)
            .default_size(320.0)
            .min_size(260.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.model_section(ui);
                    ui.separator();
                    self.settings_section(ui);
                    ui.separator();
                    self.slice_section(ui);
                    ui.separator();
                    self.messages_section(ui);
                });
            });
        settings_window(
            &ctx,
            &mut self.all_settings,
            &mut self.editor,
            &self.base,
            &mut self.common,
            &self.overrides,
            self.current.as_ref().ok(),
        );
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Plate, "Plate");
                ui.selectable_value(&mut self.tab, Tab::Preview, "Preview");
                ui.separator();
                ui.selectable_value(&mut self.three_d, false, "2D");
                ui.selectable_value(&mut self.three_d, true, "3D");
            });
            if self.tab == Tab::Preview {
                self.preview_controls(ui);
            }
            if self.three_d {
                self.show_3d(ui);
            } else {
                self.plate(ui);
            }
        });
    }
}
