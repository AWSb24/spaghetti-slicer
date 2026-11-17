//! Drawing the profile pickers, the process tabs, the list of all settings
//! and the progress bar. The logic lives in `slicer_ui::profiles` and
//! `slicer_ui::settings_list`.

#[cfg(not(target_arch = "wasm32"))]
use std::fs;
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use std::path::PathBuf;

use eframe::egui::{self, RichText};
use slicer_fdm::Step;
use slicer_profiles::{Settings, ValueKind};
#[cfg(not(target_arch = "wasm32"))]
use slicer_ui::profiles::import_orca_files;
use slicer_ui::profiles::{ProfileSet, find_profiles_dir, profile_name};
use slicer_ui::settings::CommonSettings;
use slicer_ui::settings_list::{
    PROCESS_TABS, Row, SettingsEditor, rows_of, setting_label, setting_unit,
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

/// The profile folder and its profiles.
pub struct Profiles {
    set: Option<ProfileSet>,
}

impl Profiles {
    /// Looks for the shipped profiles.
    pub fn find() -> Self {
        let exe = std::env::current_exe().ok();
        let cwd = std::env::current_dir().ok();
        Self {
            set: find_profiles_dir(exe.as_deref(), cwd.as_deref())
                .map(|dir| ProfileSet::scan(&dir)),
        }
    }

    /// The drop-down for one role of `slicer_ui::profiles::ROLES` (printer, material,
    /// process), across the width of the panel. `files` are the settings
    /// files in use.
    pub fn combo(
        &mut self,
        ui: &mut egui::Ui,
        files: &[PathBuf],
        role: usize,
    ) -> Option<ProfileAction> {
        let Some(set) = &self.set else {
            if role == 0 {
                ui.weak("No profile folder found.");
            }
            return None;
        };
        let mut action = None;
        let choice = set.choice_of(files);
        let shown = choice[role]
            .and_then(|index| set.files[role].get(index))
            .map_or_else(|| "(none)".to_owned(), |path| profile_name(path));
        egui::ComboBox::from_id_salt(("profile", role))
            .width(ui.available_width())
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
