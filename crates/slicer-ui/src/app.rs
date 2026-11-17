//! The window, laid out the way users of common desktop slicers know it:
//! a top bar with the file menu, the Prepare and Preview tabs and the slice
//! and export buttons; a side panel with printer, filament, objects and the
//! process settings in tabs; the plate in 3D with a toolbar over it, and in
//! the preview a layer slider on the right and a legend of the line types.
//! Messages show at the bottom right of the plate.

use std::path::{Path, PathBuf};

use eframe::egui::{
    self, Align, Align2, Color32, FontId, Layout, Pos2, Rect, RichText, Sense, Stroke, Vec2,
};
use slicer_core::{Diagnostic, Severity};
use slicer_fdm::PathKind;
use slicer_profiles::Settings;
use slicer_ui::job::{
    SliceJob, SliceRequest, SliceResult, encode_output, format_duration, output_name,
    with_output_extension,
};
use slicer_ui::plate::{Placement, Plate, SCALE_RANGE, bed_center, check_placement, load_model};
use slicer_ui::profiles::replace_profiles;
use slicer_ui::settings::{CommonSettings, SettingsFile, load_settings};
use slicer_ui::settings_list::{PROCESS_TABS, SettingsEditor};
use slicer_ui::view::{kind_name, kinds_in, length_by_kind, path_color};

use crate::icons::{self, Icon};
use crate::platform;
use crate::settings_ui::{ProfileAction, Profiles, process_tabs, progress_bar, settings_window};
use crate::theme;
use crate::view3d::{Event, Scene, View3d};

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
    /// When it was first shown, in seconds of the window's clock.
    shown_at: Option<f64>,
}

impl From<&Diagnostic> for Note {
    fn from(diagnostic: &Diagnostic) -> Self {
        Self {
            level: match diagnostic.severity {
                Severity::Warning => Level::Warning,
                Severity::Error => Level::Error,
            },
            text: diagnostic.message.clone(),
            shown_at: None,
        }
    }
}

/// Seconds an information message stays on screen.
const INFO_SECONDS: f64 = 6.0;

/// What the central area shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Plate,
    Preview,
}

/// A finished slice and what it was made from.
struct Sliced {
    result: SliceResult,
    /// Extruded length per line type, for the legend.
    lengths: Vec<(PathKind, f64)>,
    settings: Settings,
    placements: Vec<Placement>,
    serial: u64,
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
    notes: Vec<Note>,
    view3d: View3d,
    /// Whether the theme has been applied.
    themed: bool,
    /// Shipped profiles to pick from.
    profiles: Profiles,
    /// Edits in the list of all settings.
    editor: SettingsEditor,
    /// Whether the list of all settings is open.
    all_settings: bool,
    /// The open tab of the process settings.
    process_tab: usize,
    /// Line types left out of the preview.
    hidden: Vec<PathKind>,
    /// Number of segments in every layer of the preview.
    segments: Vec<usize>,
    /// How many segments of the shown layer the preview draws; all if
    /// `None`.
    moves: Option<usize>,
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
            notes: Vec::new(),
            view3d: View3d::default(),
            themed: false,
            profiles: Profiles::find(),
            editor: SettingsEditor::default(),
            all_settings: false,
            process_tab: 0,
            hidden: Vec::new(),
            segments: Vec::new(),
            moves: None,
        };
        app.open_paths(paths);
        app
    }

    fn note(&mut self, level: Level, text: impl Into<String>) {
        self.notes.push(Note {
            level,
            text: text.into(),
            shown_at: None,
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
                // Models opened together with the files already go onto
                // the new bed.
                self.current = self.editor.build(&self.base, &self.common, &self.overrides);
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
                if let Ok(settings) = &self.current {
                    self.segments = self.view3d.set_toolpaths(
                        &result.toolpaths,
                        settings.line_width,
                        &self.hidden,
                    );
                    self.moves = None;
                    self.sliced = Some(Sliced {
                        lengths: length_by_kind(&result.toolpaths.layers),
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

    /// A section head of the side panel: icon and title on a lighter strip
    /// across the panel.
    fn section_head(ui: &mut egui::Ui, icon: Icon, title: &str) {
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::hover());
        let painter = ui.painter();
        // Reach the panel edges, past the panel's inner margin.
        painter.rect_filled(rect.expand2(Vec2::new(8.0, 0.0)), 0.0, theme::HEADER);
        let icon_rect = Rect::from_center_size(
            Pos2::new(rect.left() + 9.0, rect.center().y),
            Vec2::splat(16.0),
        );
        icons::paint(painter, icon_rect, icon, theme::ACCENT);
        painter.text(
            Pos2::new(rect.left() + 26.0, rect.center().y),
            Align2::LEFT_CENTER,
            title,
            FontId::proportional(14.0),
            theme::TEXT,
        );
        ui.add_space(4.0);
    }

    /// Printer, filament and process with their profiles, the process
    /// settings in tabs, and the settings files behind them.
    fn side_panel(&mut self, ui: &mut egui::Ui) {
        let paths: Vec<PathBuf> = self.files.iter().map(|(path, _)| path.clone()).collect();
        let mut action = None;

        Self::section_head(ui, Icon::Printer, "Printer");
        if let Some(chosen) = self.profiles.combo(ui, &paths, 0) {
            action = Some(chosen);
        }
        let base = &self.base;
        ui.label(
            RichText::new(format!(
                "Nozzle {} mm · Bed {:.0} × {:.0} × {:.0} mm",
                base.nozzle_diameter, base.bed_size_x, base.bed_size_y, base.max_print_height
            ))
            .small()
            .color(theme::TEXT_WEAK),
        );
        ui.add_space(6.0);

        Self::section_head(ui, Icon::Filament, "Filament");
        ui.horizontal(|ui| {
            let (mark, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
            ui.painter().rect_filled(mark, 2.0, theme::ACCENT);
            ui.painter().text(
                mark.center(),
                Align2::CENTER_CENTER,
                "1",
                FontId::proportional(12.0),
                Color32::WHITE,
            );
            if let Some(chosen) = self.profiles.combo(ui, &paths, 1) {
                action = Some(chosen);
            }
        });
        ui.add_space(6.0);

        Self::section_head(ui, Icon::Objects, "Objects");
        self.objects(ui);
        ui.add_space(6.0);

        Self::section_head(ui, Icon::Process, "Process");
        if let Some(chosen) = self.profiles.combo(ui, &paths, 2) {
            action = Some(chosen);
        }
        ui.add_space(2.0);
        let current = match &self.current {
            Ok(settings) => settings,
            Err(_) => &self.base,
        };
        process_tabs(
            ui,
            &mut self.process_tab,
            &mut self.editor,
            &self.base,
            &mut self.common,
            &self.overrides,
            current,
        );
        if self.process_tab == PROCESS_TABS.len() - 1 {
            ui.add_space(6.0);
            ui.label("More settings, one key = value per line:");
            ui.add(
                egui::TextEdit::multiline(&mut self.overrides)
                    .hint_text("brim_width = 5\nnozzle_temp = 220")
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );
        }
        if let Err(error) = &self.current {
            ui.colored_label(ui.visuals().error_fg_color, &error.message);
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let edits = self.editor.edit_count();
            let label = if edits == 0 {
                "All settings…".to_owned()
            } else {
                format!("All settings… ({edits} edited)")
            };
            if ui
                .button(label)
                .on_hover_text("Every setting with search, also printer and filament values")
                .clicked()
            {
                self.all_settings = !self.all_settings;
            }
        });

        let mut remove = None;
        egui::CollapsingHeader::new(format!("Settings files ({})", self.files.len()))
            .id_salt("settings_files")
            .show(ui, |ui| {
                for (index, (path, file)) in self.files.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.small_button("Remove").clicked() {
                            remove = Some(index);
                        }
                        ui.label(&file.name)
                            .on_hover_text(path.display().to_string());
                    });
                }
                ui.horizontal(|ui| {
                    if ui
                        .small_button("Add file…")
                        .on_hover_text("Settings in TOML; later files override earlier ones")
                        .clicked()
                    {
                        self.pick_settings_files(ui.ctx());
                    }
                    if !self.files.is_empty() && ui.small_button("Reload").clicked() {
                        self.reload_settings_files();
                    }
                });
                if let Some(chosen) = self.profiles.tools(ui) {
                    action = Some(chosen);
                }
            });
        if let Some(index) = remove {
            let mut files = self.files.clone();
            files.remove(index);
            self.use_settings_files(files);
        }
        match action {
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
    }

    fn pick_models(&mut self, ctx: &egui::Context) {
        for path in platform::pick_files(ctx, "Models", &["stl", "3mf"], true) {
            self.open_model(&path);
        }
    }

    fn pick_settings_files(&mut self, ctx: &egui::Context) {
        let paths = platform::pick_files(ctx, "Settings", &["toml"], true);
        if !paths.is_empty() {
            self.add_settings_files(&paths);
        }
    }

    /// The models on the plate and the placement of the selected one.
    fn objects(&mut self, ui: &mut egui::Ui) {
        if self.plate.items.is_empty() {
            ui.label(
                RichText::new("Drop STL or 3MF files here, or add them with + above the plate.")
                    .small()
                    .color(theme::TEXT_WEAK),
            );
            return;
        }
        let bed = self.bed();
        let mut clicked = None;
        for (index, item) in self.plate.items.iter().enumerate() {
            let (s, size) = (item.placement.scale, item.model.size);
            let text = format!(
                "{}   {:.1} × {:.1} × {:.1} mm",
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
        egui::Grid::new("placement")
            .num_columns(2)
            .spacing([10.0, 4.0])
            .show(ui, |ui| {
                for (label, axis) in [("X", 0), ("Y", 1)] {
                    ui.label(label);
                    ui.add(
                        egui::DragValue::new(&mut placement.center[axis])
                            .speed(0.5)
                            .range(0.0..=bed[axis])
                            .suffix(" mm"),
                    );
                    ui.end_row();
                }
                ui.label("Rotation");
                ui.add(
                    egui::DragValue::new(&mut placement.rotation_deg)
                        .speed(1.0)
                        .range(-360.0..=360.0)
                        .suffix("°"),
                );
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
    }

    /// The bar at the top: file menu, Prepare and Preview, and on the
    /// right slicing and exporting.
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.menu_button(RichText::new("  File  ").color(theme::TEXT), |ui| {
                if ui.button("Add models…").clicked() {
                    self.pick_models(ui.ctx());
                }
                if ui.button("Add settings file…").clicked() {
                    self.pick_settings_files(ui.ctx());
                }
                ui.separator();
                let sliced = self.sliced.is_some();
                if ui
                    .add_enabled(sliced, egui::Button::new("Export G-code…"))
                    .clicked()
                {
                    self.save(false);
                }
                if ui
                    .add_enabled(sliced, egui::Button::new("Export .gcode.3mf…"))
                    .clicked()
                {
                    self.save(true);
                }
            });
            ui.add_space(8.0);
            for (tab, icon, name) in [
                (Tab::Plate, Icon::Prepare, "Prepare"),
                (Tab::Preview, Icon::Preview, "Preview"),
            ] {
                if main_tab(ui, icon, name, self.tab == tab).clicked() {
                    self.tab = tab;
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                ui.add_space(8.0);
                self.slice_controls(ui);
            });
        });
    }

    /// Slice and export buttons, or the progress of a running slice. Laid
    /// out from right to left.
    fn slice_controls(&mut self, ui: &mut egui::Ui) {
        if let Some(job) = &self.job {
            if ui
                .add_enabled(!job.is_cancelling(), egui::Button::new("Cancel"))
                .clicked()
            {
                job.cancel();
            }
            if job.is_cancelling() {
                ui.label("Cancelling");
                ui.spinner();
            } else {
                let (step, fraction) = job.progress();
                progress_bar(ui, step, fraction, job.elapsed().as_secs_f64());
            }
            return;
        }
        let current = self.slice_is_current();
        let exported = pill(ui, "Export G-code", current, self.sliced.is_some());
        if exported.main.clicked() {
            self.save(false);
        }
        egui::Popup::menu(&exported.chevron).show(|ui| {
            if ui.button("G-code (.gcode)").clicked() {
                self.save(false);
            }
            if ui
                .button("Bambu Lab project (.gcode.3mf)")
                .on_hover_text("Not yet tested on a printer")
                .clicked()
            {
                self.save(true);
            }
        });
        let blocker = self.slice_blocker();
        let slice = pill(ui, "Slice plate", !current, blocker.is_none());
        if slice.main.clicked() || slice.chevron.clicked() {
            self.start_slice(ui.ctx());
        }
        if let Some(text) = blocker {
            ui.add(
                egui::Label::new(RichText::new(text).color(ui.visuals().warn_fg_color)).truncate(),
            );
        }
    }

    /// The toolbar over the plate: add, arrange, centre, turn, delete.
    fn toolbar(&mut self, ctx: &egui::Context, canvas: Rect) {
        let selected = self.plate.selected.is_some();
        let any = !self.plate.items.is_empty();
        egui::Area::new(egui::Id::unique("toolbar"))
            .pivot(Align2::CENTER_TOP)
            .fixed_pos(canvas.center_top() + Vec2::new(0.0, 8.0))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(theme::TOOLBAR)
                    .corner_radius(4)
                    .inner_margin(4)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            if icons::button(ui, Icon::Add, 34.0, "Add models", false).clicked() {
                                self.pick_models(ui.ctx());
                            }
                            let arrange = ui
                                .add_enabled_ui(any, |ui| {
                                    icons::button(ui, Icon::Arrange, 34.0, "Arrange all", false)
                                })
                                .inner;
                            if arrange.clicked() {
                                self.plate
                                    .arrange(self.current.as_ref().unwrap_or(&self.base));
                            }
                            ui.separator();
                            let bed = self.bed();
                            let tools = [
                                (Icon::Center, "Move to the middle of the bed"),
                                (Icon::TurnLeft, "Turn left by 90°"),
                                (Icon::TurnRight, "Turn right by 90°"),
                                (Icon::Delete, "Delete"),
                            ];
                            for (icon, tip) in tools {
                                let clicked = ui
                                    .add_enabled_ui(selected, |ui| {
                                        icons::button(ui, icon, 34.0, tip, false)
                                    })
                                    .inner
                                    .clicked();
                                if !clicked {
                                    continue;
                                }
                                if icon == Icon::Delete {
                                    self.plate.remove_selected();
                                } else if let Some(item) = self.plate.selected_mut() {
                                    let placement = &mut item.placement;
                                    match icon {
                                        Icon::Center => {
                                            placement.center = [bed[0] / 2.0, bed[1] / 2.0];
                                        }
                                        Icon::TurnLeft => {
                                            placement.rotation_deg =
                                                (placement.rotation_deg + 90.0) % 360.0;
                                        }
                                        _ => {
                                            placement.rotation_deg =
                                                (placement.rotation_deg - 90.0) % 360.0;
                                        }
                                    }
                                }
                            }
                        });
                    });
            });
    }

    /// The most recent messages at the bottom right of the plate, each with
    /// a coloured edge and a button to dismiss it.
    fn notifications(&mut self, ctx: &egui::Context, canvas: Rect, lift: f32) {
        // Information goes away by itself after a while; warnings and
        // errors stay until dismissed.
        let now = ctx.input(|i| i.time);
        for note in &mut self.notes {
            note.shown_at.get_or_insert(now);
        }
        self.notes.retain(|note| {
            note.level != Level::Info || note.shown_at.is_none_or(|at| now - at < INFO_SECONDS)
        });
        if self.notes.iter().any(|note| note.level == Level::Info) {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        if self.notes.is_empty() {
            return;
        }
        let mut dismiss = None;
        let mut clear = false;
        egui::Area::new(egui::Id::unique("notifications"))
            .pivot(Align2::RIGHT_BOTTOM)
            .fixed_pos(canvas.right_bottom() - Vec2::new(12.0, 12.0 + lift))
            .show(ctx, |ui| {
                ui.set_max_width(380.0);
                let shown = self.notes.len().min(4);
                let first = self.notes.len() - shown;
                for (index, note) in self.notes.iter().enumerate().skip(first) {
                    let edge = match note.level {
                        Level::Info => theme::ACCENT,
                        Level::Warning => ui.visuals().warn_fg_color,
                        Level::Error => ui.visuals().error_fg_color,
                    };
                    let frame = egui::Frame::new()
                        .fill(theme::POPUP)
                        .corner_radius(3)
                        .inner_margin(egui::Margin {
                            left: 12,
                            right: 6,
                            top: 6,
                            bottom: 6,
                        })
                        .show(ui, |ui| {
                            ui.set_width(360.0);
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&note.text).small().color(theme::TEXT),
                                    )
                                    .wrap(),
                                );
                                ui.with_layout(Layout::right_to_left(Align::TOP), |ui| {
                                    if ui.small_button("✕").clicked() {
                                        dismiss = Some(index);
                                    }
                                });
                            });
                        });
                    let r = frame.response.rect;
                    ui.painter().rect_filled(
                        Rect::from_min_size(r.min, Vec2::new(4.0, r.height())),
                        2.0,
                        edge,
                    );
                }
                if self.notes.len() > 1 && ui.small_button("Clear all").clicked() {
                    clear = true;
                }
            });
        if let Some(index) = dismiss {
            self.notes.remove(index);
        }
        if clear {
            self.notes.clear();
        }
    }

    /// The upright layer slider at the right of the preview; the arrow keys
    /// move it too.
    fn layer_slider(&mut self, ui: &mut egui::Ui) {
        let Some(sliced) = &self.sliced else {
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
        ui.vertical_centered(|ui| {
            ui.add_space(8.0);
            let label = format!("{}\n{:.2}", number, layers[self.layer].z);
            egui::Frame::new()
                .fill(theme::POPUP)
                .stroke(Stroke::new(1.0, theme::LINE))
                .corner_radius(3)
                .inner_margin(egui::Margin::symmetric(6, 2))
                .show(ui, |ui| {
                    ui.label(RichText::new(label).small().color(theme::ACCENT));
                })
                .response
                .on_hover_text("Layer and its height in mm");
            ui.spacing_mut().slider_width = (ui.available_height() - 24.0).max(80.0);
            ui.add(
                egui::Slider::new(&mut number, 1..=layers.len())
                    .vertical()
                    .show_value(false),
            )
            .on_hover_text("Layer; arrow keys up and down");
        });
        if number - 1 != self.layer {
            // The label above the slider belongs to the new layer, which
            // is shown whole.
            ui.ctx().request_repaint();
            self.moves = None;
        }
        self.layer = number - 1;
    }

    /// The slider along the bottom of the preview that draws the shown
    /// layer only up to a number of segments, in printing order.
    fn moves_slider(&mut self, ctx: &egui::Context, canvas: Rect) {
        let Some(&count) = self.segments.get(self.layer) else {
            return;
        };
        if count == 0 {
            return;
        }
        let mut shown = self.moves.unwrap_or(count).min(count);
        egui::Area::new(egui::Id::unique("moves"))
            .pivot(Align2::CENTER_BOTTOM)
            .fixed_pos(canvas.center_bottom() - Vec2::new(0.0, 10.0))
            .show(ctx, |ui| {
                ui.spacing_mut().slider_width = (canvas.width() * 0.6).max(120.0);
                ui.add(egui::Slider::new(&mut shown, 0..=count).show_value(true))
                    .on_hover_text("Segments of this layer shown, in printing order");
            });
        self.moves = (shown < count).then_some(shown);
    }

    /// The legend at the top right of the preview: every line type with
    /// its colour, its share of the extruded length and the length, and
    /// the totals. Types not in the shown layer are dimmed.
    fn legend(&mut self, ctx: &egui::Context, canvas: Rect) {
        let Some(sliced) = &self.sliced else {
            return;
        };
        let stats = &sliced.result.output.stats;
        let total: f64 = sliced.lengths.iter().map(|(_, length)| length).sum();
        let in_layer = sliced
            .result
            .toolpaths
            .layers
            .get(self.layer)
            .map(kinds_in)
            .unwrap_or_default();
        let current = self.slice_is_current();
        let mut toggled = None;
        egui::Area::new(egui::Id::unique("legend"))
            .pivot(Align2::RIGHT_TOP)
            .fixed_pos(canvas.right_top() + Vec2::new(-12.0, 12.0))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(theme::POPUP)
                    .corner_radius(4)
                    .inner_margin(10)
                    .show(ui, |ui| {
                        egui::Grid::new("legend_rows")
                            .num_columns(5)
                            .spacing([12.0, 3.0])
                            .show(ui, |ui| {
                                ui.label("");
                                ui.label(RichText::new("Line type").strong());
                                ui.label(RichText::new("%").strong());
                                ui.label(RichText::new("Length").strong());
                                ui.label("");
                                ui.end_row();
                                for &(kind, length) in &sliced.lengths {
                                    let [r, g, b] = path_color(kind);
                                    let (swatch, _) =
                                        ui.allocate_exact_size(Vec2::splat(11.0), Sense::hover());
                                    ui.painter().rect_filled(
                                        swatch,
                                        1.0,
                                        Color32::from_rgb(r, g, b),
                                    );
                                    let name = RichText::new(kind_name(kind));
                                    ui.label(if in_layer.contains(&kind) {
                                        name
                                    } else {
                                        name.color(theme::TEXT_WEAK)
                                    });
                                    let share = if total > 0.0 { length / total } else { 0.0 };
                                    ui.label(format!("{:.1}", share * 100.0));
                                    ui.label(format!("{:.2} m", length / 1000.0));
                                    let hidden = self.hidden.contains(&kind);
                                    let (icon, tip) = if hidden {
                                        (Icon::EyeClosed, "Show")
                                    } else {
                                        (Icon::Eye, "Hide")
                                    };
                                    if icons::button(ui, icon, 20.0, tip, false).clicked() {
                                        toggled = Some(kind);
                                    }
                                    ui.end_row();
                                }
                            });
                        ui.add_space(6.0);
                        ui.label(RichText::new("Total estimation").strong());
                        ui.separator();
                        egui::Grid::new("legend_totals")
                            .num_columns(2)
                            .spacing([12.0, 2.0])
                            .show(ui, |ui| {
                                ui.label("Filament:");
                                ui.label(format!(
                                    "{:.2} m   {:.2} g",
                                    stats.filament_length_mm / 1000.0,
                                    stats.filament_weight_g
                                ));
                                ui.end_row();
                                ui.label("Layers:");
                                ui.label(stats.layers.to_string());
                                ui.end_row();
                                ui.label("Total time:");
                                ui.label(format_duration(stats.print_time_s));
                                ui.end_row();
                            });
                        if !current {
                            ui.colored_label(
                                ui.visuals().warn_fg_color,
                                "Changed since this slice",
                            );
                        }
                    });
            });
        if let Some(kind) = toggled {
            if let Some(at) = self.hidden.iter().position(|&k| k == kind) {
                self.hidden.remove(at);
            } else {
                self.hidden.push(kind);
            }
            self.segments = self.view3d.set_toolpaths(
                &sliced.result.toolpaths,
                sliced.settings.line_width,
                &self.hidden,
            );
        }
    }

    /// The 3D view: the models on the plate, or the toolpaths up to the
    /// chosen layer in the preview.
    fn show_3d(&mut self, ui: &mut egui::Ui) {
        let settings = self.current.as_ref().unwrap_or(&self.base);
        let bed = [settings.bed_size_x, settings.bed_size_y];
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
            bed,
            height: settings.max_print_height,
            serial: self.plate.serial,
            models,
            layer,
            moves: self.moves,
        };
        match self.view3d.show(ui, &scene) {
            Event::None => {}
            Event::Select(index) => self.plate.selected = index,
            Event::Move(index, [dx, dy]) => {
                self.plate.selected = Some(index);
                if let Some(item) = self.plate.items.get_mut(index) {
                    let center = &mut item.placement.center;
                    center[0] = (center[0] + dx).clamp(0.0, bed[0]);
                    center[1] = (center[1] + dy).clamp(0.0, bed[1]);
                }
            }
        }
    }
}

/// The parts of a pill-shaped button: the label and the round chevron on
/// its left.
struct Pill {
    main: egui::Response,
    chevron: egui::Response,
}

/// A rounded button with a round chevron at its left end, as the slice and
/// export buttons; `primary` fills it with the accent colour.
fn pill(ui: &mut egui::Ui, label: &str, primary: bool, enabled: bool) -> Pill {
    let font = FontId::proportional(13.0);
    let text_width = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), theme::TEXT)
        .size()
        .x;
    let size = Vec2::new(text_width + 48.0, 26.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, main) = ui.allocate_exact_size(size, sense);
    let chevron_rect = Rect::from_min_size(rect.min, Vec2::splat(rect.height()));
    let chevron = ui.interact(chevron_rect, main.id.with("chevron"), sense);
    let (fill, text) = match (enabled, primary) {
        (false, _) => (theme::WIDGET, theme::TEXT_WEAK),
        (true, true) => (theme::ACCENT, Color32::WHITE),
        (true, false) => (theme::WIDGET, theme::TEXT),
    };
    let fill = if enabled && main.hovered() {
        fill.gamma_multiply(1.15)
    } else {
        fill
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 13.0, fill);
    painter.circle_filled(chevron_rect.center(), 10.0, theme::ACCENT_DARK);
    icons::paint(
        painter,
        chevron_rect.shrink(7.0),
        Icon::Chevron,
        Color32::WHITE,
    );
    painter.text(
        Pos2::new(chevron_rect.right() + 6.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        font,
        text,
    );
    Pill { main, chevron }
}

/// A main tab in the top bar: icon and name, filled with the dark accent
/// when open.
fn main_tab(ui: &mut egui::Ui, icon: Icon, name: &str, open: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(120.0, 36.0), Sense::click());
    let painter = ui.painter();
    if open {
        painter.rect_filled(rect, 0.0, theme::ACCENT_DARK);
    } else if response.hovered() {
        painter.rect_filled(rect, 0.0, theme::WIDGET_HOVER);
    }
    let icon_rect = Rect::from_center_size(
        Pos2::new(rect.left() + 28.0, rect.center().y),
        Vec2::splat(16.0),
    );
    icons::paint(painter, icon_rect, icon, theme::TEXT);
    painter.text(
        Pos2::new(rect.left() + 44.0, rect.center().y),
        Align2::LEFT_CENTER,
        name,
        FontId::proportional(14.0),
        if open { Color32::WHITE } else { theme::TEXT },
    );
    response
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
        if !self.themed {
            theme::apply(&ctx);
            self.themed = true;
        }
        self.poll_job();
        self.handle_dropped_files(&ctx);
        self.current = self.editor.build(&self.base, &self.common, &self.overrides);

        egui::Panel::top("top")
            .frame(egui::Frame::new().fill(theme::PANEL))
            .show(ui, |ui| self.top_bar(ui));
        egui::Panel::left("side")
            .resizable(true)
            .default_size(350.0)
            .min_size(300.0)
            .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(8))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.side_panel(ui));
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
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::CANVAS))
            .show(ui, |ui| {
                let preview = self.tab == Tab::Preview;
                if preview && self.sliced.is_some() {
                    egui::Panel::right("layers")
                        .resizable(false)
                        .exact_size(64.0)
                        .frame(egui::Frame::new().fill(theme::CANVAS))
                        .show(ui, |ui| self.layer_slider(ui));
                }
                let canvas = ui.available_rect_before_wrap();
                self.show_3d(ui);
                if preview {
                    if self.sliced.is_some() {
                        self.legend(&ctx, canvas);
                        self.moves_slider(&ctx, canvas);
                    } else {
                        egui::Area::new(egui::Id::unique("no_slice"))
                            .pivot(Align2::CENTER_TOP)
                            .fixed_pos(canvas.center_top() + Vec2::new(0.0, 16.0))
                            .show(&ctx, |ui| {
                                ui.label(
                                    RichText::new("Slice the plate to see the layers.")
                                        .color(theme::TEXT),
                                );
                            });
                    }
                } else {
                    self.toolbar(&ctx, canvas);
                }
                // Above the slider along the bottom of the preview.
                let lift = if preview && self.sliced.is_some() {
                    40.0
                } else {
                    0.0
                };
                self.notifications(&ctx, canvas, lift);
            });
    }
}
