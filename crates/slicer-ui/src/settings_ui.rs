//! Drawing the profile pickers, the process tabs, the list of all settings
//! and the progress bar. The logic lives in `slicer_ui::profiles` and
//! `slicer_ui::settings_list`.

use std::collections::BTreeMap;
#[cfg(not(target_arch = "wasm32"))]
use std::fs;
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use std::path::PathBuf;

use eframe::egui::{self, RichText};
use slicer_fdm::Step;
use slicer_profiles::{SETTINGS_INFO, Settings, ValueKind};
use slicer_ui::filaments::{self, MAX_SLOTS};
#[cfg(not(target_arch = "wasm32"))]
use slicer_ui::profiles::import_orca_files;
use slicer_ui::profiles::{ProfileSet, find_profiles_dir, profile_name};
use slicer_ui::settings::CommonSettings;
use slicer_ui::settings_list::{
    PROCESS_TABS, Row, SettingsEditor, changed_overrides, rows_of, setting_label, setting_unit,
};

use crate::theme;

/// What the profile picker asks the window to do.
// The browser build has no Orca import, so two variants go unused there.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub enum ProfileAction {
    /// Use the profiles `new` instead of `old`, keeping other files.
    Use {
        /// Profile files in use until now.
        old: Vec<PathBuf>,
        /// Profile files to use from now on.
        new: Vec<PathBuf>,
    },
    /// An imported profile was saved at `path`; add it.
    Imported {
        /// The new settings file.
        path: PathBuf,
        /// What was left out.
        notes: Vec<String>,
    },
    /// Something went wrong.
    Error(String),
}

/// The profile folder and its profiles, and the built-in profiles chosen
/// in the setup assistant.
pub struct Profiles {
    set: Option<ProfileSet>,
    builtin: [Vec<PathBuf>; 3],
}

impl Profiles {
    /// Looks for the shipped profiles.
    pub fn find() -> Self {
        let exe = std::env::current_exe().ok();
        let cwd = std::env::current_dir().ok();
        Self {
            set: find_profiles_dir(exe.as_deref(), cwd.as_deref())
                .map(|dir| ProfileSet::scan(&dir)),
            builtin: Default::default(),
        }
    }

    /// Sets the built-in profiles listed before the files, by role.
    pub fn set_builtin(&mut self, builtin: [Vec<PathBuf>; 3]) {
        self.builtin = builtin;
    }

    /// Every profile to pick from: the built-in ones, then the files.
    pub fn listing(&self) -> ProfileSet {
        match &self.set {
            Some(set) => set.with_builtin(&self.builtin),
            None => ProfileSet {
                dir: PathBuf::new(),
                files: self.builtin.clone(),
            },
        }
    }

    /// Reads the profile folder again, after a profile was saved into it.
    pub fn rescan(&mut self) {
        if let Some(set) = &mut self.set {
            *set = ProfileSet::scan(&set.dir.clone());
        }
    }

    /// The drop-down for one role of `slicer_ui::profiles::ROLES` (printer, material,
    /// process), across the width of the panel less `reserve` pixels for
    /// what follows in the row. `files` are the settings files in use.
    pub fn combo(
        &mut self,
        ui: &mut egui::Ui,
        files: &[PathBuf],
        role: usize,
        reserve: f32,
    ) -> Option<ProfileAction> {
        let set = self.listing();
        if set.files[role].is_empty() && role < 2 {
            ui.weak("None yet: File > Setup assistant…");
            return None;
        }
        let mut action = None;
        let choice = set.choice_of(files);
        let shown = choice[role]
            .and_then(|index| set.files[role].get(index))
            .map_or_else(|| "(none)".to_owned(), |path| profile_name(path));
        egui::ComboBox::from_id_salt(("profile", role))
            .width(ui.available_width() - reserve)
            .selected_text(shown)
            .show_ui(ui, |ui| {
                let mut wanted = choice;
                ui.selectable_value(&mut wanted[role], None, "(none)");
                for (index, path) in set.files[role].iter().enumerate() {
                    ui.selectable_value(&mut wanted[role], Some(index), profile_name(path));
                }
                if wanted != choice {
                    action = Some(ProfileAction::Use {
                        old: set.chosen(&choice),
                        new: set.chosen(&wanted),
                    });
                }
            });
        action
    }

    /// Choosing the profile folder and importing Orca profiles.
    // ponytail: the browser has no folders to choose and no files to save
    // next to the profiles; both stay on the desktop for now.
    #[cfg(target_arch = "wasm32")]
    pub fn tools(&mut self, _ui: &mut egui::Ui) -> Option<ProfileAction> {
        None
    }

    /// Choosing the profile folder and importing Orca profiles.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn tools(&mut self, ui: &mut egui::Ui) -> Option<ProfileAction> {
        let mut action = None;
        ui.horizontal(|ui| {
            let folder = self.set.as_ref().map(|set| set.dir.clone());
            if ui
                .small_button("Profile folder…")
                .on_hover_text(folder.as_ref().map_or_else(
                    || "Choose a folder with printers, materials and processes".to_owned(),
                    |dir| dir.display().to_string(),
                ))
                .clicked()
                && let Some(dir) = rfd::FileDialog::new().pick_folder()
            {
                self.set = Some(ProfileSet::scan(&dir));
            }
            if ui
                .small_button("Import Orca profile…")
                .on_hover_text("Convert profiles saved by Orca Slicer (JSON) into a settings file")
                .clicked()
            {
                action = self.import_orca(folder.as_deref());
            }
        });
        action
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn import_orca(&mut self, folder: Option<&Path>) -> Option<ProfileAction> {
        let paths = rfd::FileDialog::new()
            .add_filter("Orca Slicer profiles", &["json"])
            .pick_files()?;
        let mut files = Vec::new();
        for path in &paths {
            match fs::read_to_string(path) {
                Ok(text) => files.push((profile_name(path), text)),
                Err(e) => return Some(ProfileAction::Error(format!("{}: {e}", path.display()))),
            }
        }
        let imported = match import_orca_files(&files) {
            Ok(imported) => imported,
            Err(error) => return Some(ProfileAction::Error(error.message)),
        };
        let mut dialog = rfd::FileDialog::new()
            .set_file_name(&imported.name)
            .add_filter("Settings", &["toml"]);
        if let Some(dir) = folder {
            dialog = dialog.set_directory(dir);
        }
        let path = dialog.save_file()?;
        if let Err(e) = fs::write(&path, &imported.toml) {
            return Some(ProfileAction::Error(format!("{}: {e}", path.display())));
        }
        // A file saved into the profile folder shows up in the lists.
        if let Some(set) = &mut self.set {
            *set = ProfileSet::scan(&set.dir.clone());
        }
        Some(ProfileAction::Imported {
            path,
            notes: imported.notes,
        })
    }
}

/// Which profile a settings dialog edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileKind {
    /// The printer: bed, nozzle, firmware, retraction, custom G-code.
    Printer,
    /// The filament: temperatures, cooling, flow, density.
    Filament,
}

impl ProfileKind {
    /// Title of the dialog.
    fn title(self) -> &'static str {
        match self {
            ProfileKind::Printer => "Printer settings",
            ProfileKind::Filament => "Filament settings",
        }
    }

    /// The groups of settings (see `SettingInfo::group`) the dialog shows.
    fn groups(self) -> &'static [&'static str] {
        match self {
            ProfileKind::Printer => &["Printer", "Retraction", "Custom G-code"],
            ProfileKind::Filament => &["Material"],
        }
    }

    /// The keys of every setting the dialog shows, as a profile of this
    /// kind holds them.
    pub fn keys(self) -> Vec<&'static str> {
        SETTINGS_INFO
            .iter()
            .filter(|info| self.groups().contains(&info.group))
            .map(|info| info.key)
            .collect()
    }
}

/// A dialog with the printer or filament settings, grouped as in the
/// settings files, and a button to save them as a profile. `current` are
/// the settings sliced with. Returns true when the user asks to save.
#[allow(clippy::too_many_arguments)]
pub fn profile_window(
    ctx: &egui::Context,
    open: &mut Option<ProfileKind>,
    editor: &mut SettingsEditor,
    base: &Settings,
    common: &mut CommonSettings,
    overrides: &str,
    current: Option<&Settings>,
) -> bool {
    let Some(kind) = *open else {
        return false;
    };
    let mut still_open = true;
    let mut save = false;
    egui::Window::new(kind.title())
        .id(egui::Id::unique("profile_window"))
        .open(&mut still_open)
        .default_size([460.0, 560.0])
        .vscroll(true)
        .show(ctx, |ui| {
            let Some(current) = current else {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    "The settings are invalid; fix the message in the side panel.",
                );
                return;
            };
            for group in kind.groups() {
                let keys: Vec<&str> = SETTINGS_INFO
                    .iter()
                    .filter(|info| info.group == *group)
                    .map(|info| info.key)
                    .collect();
                ui.add_space(6.0);
                group_header(ui, group);
                for row in &rows_of(current, base, &keys) {
                    setting_row(ui, row, editor, base, common, overrides, true);
                }
            }
            ui.add_space(8.0);
            ui.separator();
            save = ui
                .button("Save as profile…")
                .on_hover_text(
                    "Write these settings to a profile file, for example in the profile folder",
                )
                .clicked();
        });
    if !still_open {
        *open = None;
    }
    save
}

/// The window with every setting. `current` are the settings sliced with,
/// if they are valid.
pub fn settings_window(
    ctx: &egui::Context,
    open: &mut bool,
    editor: &mut SettingsEditor,
    base: &Settings,
    common: &mut CommonSettings,
    overrides: &str,
    current: Option<&Settings>,
) {
    egui::Window::new("All settings")
        .open(open)
        .default_size([560.0, 640.0])
        .vscroll(false)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut editor.search)
                        .hint_text("Search names and descriptions")
                        .desired_width(240.0),
                );
                ui.checkbox(&mut editor.only_changed, "Only changed");
            });
            let Some(current) = current else {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    "The settings are invalid; fix the message in the side panel.",
                );
                return;
            };
            ui.weak("Changed values are bold; edits apply after the settings files.");
            let rows = editor.rows(current, base);
            egui::ScrollArea::vertical().show(ui, |ui| {
                let mut group = "";
                for row in &rows {
                    if row.info.group != group {
                        group = row.info.group;
                        ui.add_space(6.0);
                        ui.label(RichText::new(group).strong());
                        ui.separator();
                    }
                    setting_row(ui, row, editor, base, common, overrides, false);
                }
                if rows.is_empty() {
                    ui.weak("No setting matches.");
                }
            });
        });
}

/// The process settings in tabs ([`PROCESS_TABS`]); `tab` is the open one.
/// `current` are the settings sliced with, `base` those from the files.
pub fn process_tabs(
    ui: &mut egui::Ui,
    tab: &mut usize,
    editor: &mut SettingsEditor,
    base: &Settings,
    common: &mut CommonSettings,
    overrides: &str,
    current: &Settings,
) {
    // Text tabs; the open one is underlined in the accent colour.
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        for (index, (name, _)) in PROCESS_TABS.iter().enumerate() {
            let open = *tab == index;
            let color = if open { theme::TEXT } else { theme::TEXT_WEAK };
            let response = ui.add(
                egui::Label::new(RichText::new(*name).color(color)).sense(egui::Sense::click()),
            );
            if open {
                let r = response.rect;
                ui.painter().line_segment(
                    [
                        r.left_bottom() + egui::vec2(0.0, 2.0),
                        r.right_bottom() + egui::vec2(0.0, 2.0),
                    ],
                    egui::Stroke::new(2.0, theme::ACCENT),
                );
            }
            if response.clicked() {
                *tab = index;
            }
        }
    });
    ui.add_space(2.0);
    let (_, groups) = PROCESS_TABS[(*tab).min(PROCESS_TABS.len() - 1)];
    for (group, keys) in groups.iter() {
        ui.add_space(6.0);
        group_header(ui, group);
        for row in &rows_of(current, base, keys) {
            setting_row(ui, row, editor, base, common, overrides, true);
        }
    }
}

/// The settings one object on the plate may set for itself
/// ([`slicer_fdm::OBJECT_SETTINGS`]), as rows like those of the process
/// tabs: values that differ from the plate's (`plate`) are bold and can be
/// reset. `overrides` are the object's values now, `editor` keeps what is
/// typed. Returns the object's new values after a change.
pub fn object_settings(
    ui: &mut egui::Ui,
    editor: &mut SettingsEditor,
    plate: &Settings,
    overrides: &BTreeMap<String, String>,
) -> Option<BTreeMap<String, String>> {
    // The editor lists the object's values on top of the plate's.
    editor.set_edits(overrides.clone());
    let mut common = CommonSettings::from_settings(plate);
    let before = match editor.build(plate, &common, "") {
        Ok(settings) => settings,
        Err(error) => {
            ui.colored_label(ui.visuals().error_fg_color, error.message);
            let clear = ui.button("Use the plate's settings").clicked();
            return clear.then(BTreeMap::new);
        }
    };
    ui.push_id("object_settings", |ui| {
        for row in &rows_of(&before, plate, slicer_fdm::OBJECT_SETTINGS) {
            setting_row(ui, row, editor, plate, &mut common, "", true);
        }
    });
    let after = editor.build(plate, &common, "").ok()?;
    (after != before).then(|| changed_overrides(overrides, plate, &before, &after))
}

/// The settings of a modifier or of a height range as rows of setting,
/// value and a button to remove it, with a list to add a setting.
///
/// `allowed` are the settings that may be set there, `values` those set
/// now, `defaults` the values the plate's settings have (used for a setting
/// that is added) and `drafts` the text being typed, by setting name.
/// Returns the new values after a change.
pub fn values_editor(
    ui: &mut egui::Ui,
    id: &str,
    allowed: &[&str],
    values: &[(String, String)],
    defaults: &BTreeMap<String, String>,
    drafts: &mut BTreeMap<String, String>,
    new_key: &mut String,
) -> Option<Vec<(String, String)>> {
    let mut result = None;
    let mut remove = None;
    for (index, (key, value)) in values.iter().enumerate() {
        let draft = drafts.entry(key.clone()).or_insert_with(|| value.clone());
        ui.horizontal(|ui| {
            ui.label(RichText::new(setting_label(key)).small())
                .on_hover_text(format!("{key} ({})", setting_unit(key)));
            if ui
                .add(egui::TextEdit::singleline(draft).desired_width(78.0))
                .changed()
                && draft != value
            {
                let mut next = values.to_vec();
                next[index].1 = draft.clone();
                result = Some(next);
            }
            if ui.small_button("×").on_hover_text("Remove").clicked() {
                remove = Some(index);
            }
        });
    }
    if let Some(index) = remove {
        let mut next = values.to_vec();
        let (key, _) = next.remove(index);
        drafts.remove(&key);
        return Some(next);
    }
    let missing: Vec<&str> = allowed
        .iter()
        .copied()
        .filter(|key| !values.iter().any(|(used, _)| used == key))
        .collect();
    if !missing.is_empty() {
        ui.horizontal(|ui| {
            let shown = if new_key.is_empty() {
                "Add setting".to_owned()
            } else {
                setting_label(new_key)
            };
            egui::ComboBox::from_id_salt((id, "new_setting"))
                .selected_text(RichText::new(shown).small())
                .width(150.0)
                .show_ui(ui, |ui| {
                    for key in &missing {
                        ui.selectable_value(new_key, (*key).to_owned(), setting_label(key));
                    }
                });
            if !new_key.is_empty() && ui.small_button("Add").clicked() {
                let key = std::mem::take(new_key);
                let value = defaults.get(&key).cloned().unwrap_or_default();
                drafts.insert(key.clone(), value.clone());
                let mut next = values.to_vec();
                next.push((key, value));
                result = Some(next);
            }
        });
    }
    result
}

/// A small square in the colour of filament slot `extruder`, with its
/// number `T1`, `T2` and so on; hovering says kind and use. The response
/// is clickable, for opening the filament window.
pub fn slot_button(
    ui: &mut egui::Ui,
    settings: &Settings,
    extruder: usize,
    usage: usize,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(22.0), egui::Sense::click());
    let colour = filaments::slot_colour(settings, extruder);
    let fill = egui::Color32::from_rgb(colour[0], colour[1], colour[2]);
    ui.painter().rect_filled(rect, 3.0, fill);
    ui.painter().rect_stroke(
        rect,
        3.0,
        egui::Stroke::new(1.0, theme::LINE),
        egui::StrokeKind::Inside,
    );
    // Light colours get dark text.
    let lightness = (0.2126 * f32::from(colour[0])
        + 0.7152 * f32::from(colour[1])
        + 0.0722 * f32::from(colour[2]))
        / 255.0;
    let text = if lightness > 0.6 {
        egui::Color32::BLACK
    } else {
        egui::Color32::WHITE
    };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        format!("T{}", extruder + 1),
        egui::FontId::proportional(11.0),
        text,
    );
    let slot = filaments::slots(settings).get(extruder).cloned();
    let kind = slot.map_or_else(|| "PLA".to_owned(), |slot| slot.kind_label().to_owned());
    let used = match usage {
        0 => "unused".to_owned(),
        1 => "1 part".to_owned(),
        n => format!("{n} parts"),
    };
    response.on_hover_text(format!(
        "Filament {}: {kind}, {used}\nClick to change the filaments and the flush volumes",
        extruder + 1
    ))
}

/// The name of filament slot `extruder` (from 0) for menus: `T2 · PETG`.
pub fn slot_label(settings: &Settings, extruder: usize) -> String {
    let kind = filaments::slots(settings)
        .get(extruder)
        .map_or_else(|| "PLA".to_owned(), |slot| slot.kind_label().to_owned());
    format!("T{} · {kind}", extruder + 1)
}

/// The window with the filament slots of the plate, their colours, kinds
/// and use, and the purge volumes between them.
///
/// The values are settings (`extruder_count`, `filament_colours`,
/// `filament_type`, `flush_volumes`, `flush_multiplier`, and
/// `flush_from_colours`), changed through `editor`, so they are checked
/// like every other setting and saved with the project. `usage` counts
/// the parts that print with each slot, `drafts` keeps what is typed into
/// the kind fields.
#[allow(clippy::too_many_arguments)]
pub fn filament_window(
    ctx: &egui::Context,
    open: &mut bool,
    editor: &mut SettingsEditor,
    base: &Settings,
    common: &mut CommonSettings,
    overrides: &str,
    current: Option<&Settings>,
    usage: &[usize],
    drafts: &mut BTreeMap<usize, String>,
) {
    if !*open {
        return;
    }
    let mut still_open = true;
    egui::Window::new("Filaments and flush volumes")
        .id(egui::Id::unique("filament_window"))
        .open(&mut still_open)
        .default_size([590.0, 620.0])
        .vscroll(true)
        .show(ctx, |ui| {
            let Some(current) = current else {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    "The settings are invalid; fix the message in the side panel.",
                );
                return;
            };
            let count = filaments::count(current);
            drafts.retain(|&slot, _| slot < count);

            ui.horizontal(|ui| {
                ui.label(RichText::new("Filament slots").strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(count < MAX_SLOTS as usize, egui::Button::new("+"))
                        .on_hover_text("Add a filament slot")
                        .clicked()
                    {
                        let plan = filaments::resize_plan(current, count as u32 + 1);
                        apply_plan(editor, base, common, overrides, plan);
                    }
                    ui.label(format!("{count} of {MAX_SLOTS}"));
                    if ui
                        .add_enabled(count > 1, egui::Button::new("−"))
                        .on_hover_text("Remove the last filament slot")
                        .clicked()
                    {
                        let plan = filaments::resize_plan(current, count as u32 - 1);
                        apply_plan(editor, base, common, overrides, plan);
                    }
                });
            });
            ui.label(
                RichText::new(
                    "One slot per extruder (extruder_count). Colours and kinds go into the \
                     G-code container; objects and parts print with the slot they name. \
                     Temperatures, flow and density come from the material profile and are \
                     the same for all slots.",
                )
                .small()
                .color(theme::TEXT_WEAK),
            );
            ui.add_space(4.0);

            egui::Grid::new("filament_slots")
                .num_columns(4)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    for (name, width) in [
                        ("Slot", 36.0),
                        ("Colour", 60.0),
                        ("Kind", 120.0),
                        ("Used by", 70.0),
                    ] {
                        ui.add_sized(
                            [width, 14.0],
                            egui::Label::new(RichText::new(name).small().color(theme::TEXT_WEAK)),
                        );
                    }
                    ui.end_row();
                    for index in 0..count {
                        slot_button(ui, current, index, usage.get(index).copied().unwrap_or(0));
                        let mut rgb = filaments::slot_colour(current, index);
                        if ui.color_edit_button_srgb(&mut rgb).changed() {
                            let text = filaments::set_slot_colour(current, index, rgb);
                            let _ = editor.edit(base, common, overrides, "filament_colours", &text);
                        }
                        let mut kind = drafts.get(&index).cloned().unwrap_or_else(|| {
                            filaments::slots(current).get(index).map_or_else(
                                || "PLA".to_owned(),
                                |slot| slot.kind_label().to_owned(),
                            )
                        });
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut kind)
                                .desired_width(112.0)
                                .hint_text("PLA"),
                        );
                        if response.changed() {
                            drafts.insert(index, kind.clone());
                        }
                        if response.lost_focus() && drafts.contains_key(&index) {
                            let text = filaments::set_slot_type(current, index, &kind);
                            if editor
                                .edit(base, common, overrides, "filament_type", &text)
                                .is_ok()
                            {
                                drafts.remove(&index);
                            }
                        }
                        let used = match usage.get(index).copied().unwrap_or(0) {
                            0 => "unused".to_owned(),
                            1 => "1 part".to_owned(),
                            n => format!("{n} parts"),
                        };
                        ui.label(RichText::new(used).small().color(theme::TEXT_WEAK));
                        ui.end_row();
                    }
                });
            for key in ["extruder_count", "filament_colours", "filament_type"] {
                if let Some(error) = editor.errors.get(key) {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
            }

            ui.add_space(8.0);
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(RichText::new("Flush volumes").strong());
                ui.label(
                    RichText::new("mm³ before the multiplier, from the row's slot to the column's")
                        .small()
                        .color(theme::TEXT_WEAK),
                );
            });
            if count < 2 {
                ui.label(
                    RichText::new(
                        "A tool change only purges with more than one filament slot; the \
                         \".gcode.3mf\" container still names one filament.",
                    )
                    .small()
                    .color(theme::TEXT_WEAK),
                );
            } else {
                let mut derive = current.flush_from_colours;
                if ui
                    .checkbox(&mut derive, "Derive from the filament colours")
                    .on_hover_text(
                        "While no volumes are given: a dark colour shows through a light one, \
                         so the change to a lighter colour purges more",
                    )
                    .changed()
                {
                    let _ = editor.edit(
                        base,
                        common,
                        overrides,
                        "flush_from_colours",
                        if derive { "true" } else { "false" },
                    );
                }
                ui.horizontal(|ui| {
                    ui.label("Multiplier");
                    let mut multiplier = current.flush_multiplier;
                    if ui
                        .add(
                            egui::DragValue::new(&mut multiplier)
                                .speed(0.1)
                                .range(0.0..=10.0),
                        )
                        .on_hover_text("Factor on every purge volume")
                        .changed()
                    {
                        let _ = editor.edit(
                            base,
                            common,
                            overrides,
                            "flush_multiplier",
                            &multiplier.to_string(),
                        );
                    }
                    if ui
                        .button("Clear")
                        .on_hover_text(
                            "No purge volumes; every tool change shares the solid tower layer",
                        )
                        .clicked()
                    {
                        let _ = editor.edit(base, common, overrides, "flush_volumes", "");
                    }
                });
                let mut table = filaments::flush_table(current);
                egui::Grid::new("flush_volumes")
                    .num_columns(count + 1)
                    .spacing([6.0, 4.0])
                    .show(ui, |ui| {
                        ui.label("");
                        for to in 0..count {
                            ui.label(
                                RichText::new(format!("T{}", to + 1))
                                    .small()
                                    .color(theme::TEXT_WEAK),
                            );
                        }
                        ui.end_row();
                        for from in 0..count {
                            ui.label(RichText::new(format!("T{}", from + 1)).small());
                            for to in 0..count {
                                if from == to {
                                    ui.weak("–");
                                    continue;
                                }
                                let mut value = table[from][to];
                                let response = ui.add_sized(
                                    [54.0, 18.0],
                                    egui::DragValue::new(&mut value)
                                        .speed(5.0)
                                        .range(0.0..=1000.0),
                                );
                                if response.changed() {
                                    // An explicit table replaces the one
                                    // derived from the colours.
                                    if current.flush_volumes.trim().is_empty()
                                        && current.flush_from_colours
                                    {
                                        let _ = editor.edit(
                                            base,
                                            common,
                                            overrides,
                                            "flush_from_colours",
                                            "false",
                                        );
                                    }
                                    table[from][to] = value;
                                    let text = filaments::flush_text(&table);
                                    let _ = editor.edit(
                                        base,
                                        common,
                                        overrides,
                                        "flush_volumes",
                                        &text,
                                    );
                                }
                            }
                            ui.end_row();
                        }
                    });
            }
            for key in ["flush_volumes", "flush_multiplier", "flush_from_colours"] {
                if let Some(error) = editor.errors.get(key) {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
            }
        });
    if !still_open {
        *open = false;
    }
}

/// Applies the settings of a [`filaments::resize_plan`] in order; the
/// first one that is not valid stays in `editor.errors`.
fn apply_plan(
    editor: &mut SettingsEditor,
    base: &Settings,
    common: &mut CommonSettings,
    overrides: &str,
    plan: Vec<(&'static str, String)>,
) {
    for (key, value) in plan {
        if editor.edit(base, common, overrides, key, &value).is_err() {
            break;
        }
    }
}

/// The title of a group of settings: a small mark, the name in bold and a
/// line to the right edge.
fn group_header(ui: &mut egui::Ui, name: &str) {
    ui.horizontal(|ui| {
        let (mark, _) = ui.allocate_exact_size(egui::Vec2::splat(12.0), egui::Sense::hover());
        ui.painter().rect_stroke(
            mark.shrink(1.0),
            2.0,
            egui::Stroke::new(1.2, theme::ACCENT),
            egui::StrokeKind::Inside,
        );
        let label = ui.label(RichText::new(name).strong().color(theme::TEXT));
        let y = label.rect.center().y;
        let right = ui.max_rect().right();
        ui.painter().line_segment(
            [
                egui::pos2(label.rect.right() + 6.0, y),
                egui::pos2(right, y),
            ],
            egui::Stroke::new(1.0, theme::LINE),
        );
    });
}

/// One setting with its field. `friendly` shows the name as words and
/// the allowed range only on hover, for the narrow side panel; otherwise
/// the key and the range are shown.
fn setting_row(
    ui: &mut egui::Ui,
    row: &Row,
    editor: &mut SettingsEditor,
    base: &Settings,
    common: &mut CommonSettings,
    overrides: &str,
    friendly: bool,
) {
    let key = row.info.key;
    // A failed edit is shown below the row from `editor.errors`.
    let edit = |editor: &mut SettingsEditor, common: &mut CommonSettings, value: &str| {
        let _ = editor.edit(base, common, overrides, key, value);
    };
    ui.horizontal(|ui| {
        let (name, width) = if friendly {
            ui.add_space(14.0);
            (RichText::new(setting_label(key)), 150.0)
        } else {
            (RichText::new(key).monospace(), 210.0)
        };
        let name = if row.changed() { name.strong() } else { name };
        let hover = match row.range {
            Some((min, max)) if friendly => {
                format!("{}\n\nAllowed: {min} – {max}", row.info.description)
            }
            _ => row.info.description.to_owned(),
        };
        // Left-aligned in a column of fixed width.
        ui.allocate_ui_with_layout(
            egui::Vec2::new(width, 18.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_width(width);
                ui.add(egui::Label::new(name).truncate())
                    .on_hover_text(hover);
            },
        );
        match row.info.kind {
            ValueKind::Flag => {
                let mut on = row.value == "true";
                if ui.checkbox(&mut on, "").changed() {
                    edit(editor, common, if on { "true" } else { "false" });
                }
            }
            ValueKind::Choice => {
                egui::ComboBox::from_id_salt(key)
                    .selected_text(row.value.as_str())
                    .show_ui(ui, |ui| {
                        for choice in row.info.choices {
                            if ui.selectable_label(row.value == *choice, *choice).clicked() {
                                edit(editor, common, choice);
                            }
                        }
                    });
            }
            ValueKind::Number | ValueKind::Count | ValueKind::Text => {
                let multiline = row.info.kind == ValueKind::Text;
                let mut text = editor
                    .drafts
                    .get(key)
                    .cloned()
                    .unwrap_or_else(|| row.value.clone());
                let field = if multiline {
                    egui::TextEdit::multiline(&mut text)
                        .desired_rows(2)
                        .font(egui::TextStyle::Monospace)
                } else {
                    egui::TextEdit::singleline(&mut text)
                };
                let unit = setting_unit(key);
                let response = if friendly && !multiline {
                    // Value and unit in one box.
                    egui::Frame::new()
                        .fill(theme::PANEL)
                        .stroke(egui::Stroke::new(1.0, theme::LINE))
                        .corner_radius(3)
                        .inner_margin(egui::Margin::symmetric(4, 1))
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            let response = ui.add(
                                field
                                    .frame(egui::Frame::NONE)
                                    .desired_width(if unit.is_empty() { 82.0 } else { 52.0 }),
                            );
                            if !unit.is_empty() {
                                ui.add_sized(
                                    [28.0, 16.0],
                                    egui::Label::new(
                                        RichText::new(unit).small().color(theme::TEXT_WEAK),
                                    ),
                                );
                            }
                            response
                        })
                        .inner
                } else {
                    ui.add(field.desired_width(if multiline { 220.0 } else { 90.0 }))
                };
                if response.changed() {
                    editor.drafts.insert(key.to_owned(), text.clone());
                }
                if response.lost_focus() && editor.drafts.contains_key(key) {
                    edit(editor, common, &text);
                }
            }
        }
        if let Some((min, max)) = row.range.filter(|_| !friendly) {
            ui.weak(format!("{min} – {max}"));
        }
        if row.changed()
            && ui
                .small_button("reset")
                .on_hover_text(format!("Back to {}", row.profile_value))
                .clicked()
        {
            editor.reset(base, common, key);
        }
    });
    if let Some(error) = editor.errors.get(key) {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
}

/// A progress bar with the step of a running slice.
pub fn progress_bar(ui: &mut egui::Ui, step: Step, fraction: f64, seconds: f64) {
    ui.add(
        egui::ProgressBar::new(fraction as f32)
            .desired_width(260.0)
            .text(format!(
                "{} … {:.0} % · {seconds:.0} s",
                step.label(),
                fraction * 100.0
            ))
            .animate(true),
    );
}
