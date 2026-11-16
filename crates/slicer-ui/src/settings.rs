//! Building the settings for a slice from files and edits in the window.
//!
//! The order matches the command line tool: defaults, then the settings
//! files one after the other, then the values edited in the side panel,
//! then single `key = value` overrides.

use slicer_core::{Diagnose, Diagnostic};
use slicer_profiles::Settings;

/// A settings file (TOML) the user loaded.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsFile {
    /// Name shown in the list, usually the file name.
    pub name: String,
    /// Content of the file.
    pub text: String,
}

/// Applies settings files in order over the defaults; a later file
/// overrides an earlier one. The error names the file at fault.
pub fn load_settings(files: &[SettingsFile]) -> Result<Settings, Diagnostic> {
    let mut settings = Settings::default();
    for file in files {
        settings.apply_toml(&file.text).map_err(|e| {
            let mut diagnostic = e.diagnostic().with("file", &file.name);
            diagnostic.message = format!("{}: {}", file.name, diagnostic.message);
            diagnostic
        })?;
    }
    Ok(settings)
}

/// The settings the side panel edits directly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommonSettings {
    /// Layer height in millimetres.
    pub layer_height: f64,
    /// Infill density in percent.
    pub infill_density: f64,
    /// Number of walls.
    pub wall_loops: u32,
    /// Whether supports are generated.
    pub supports: bool,
}

impl CommonSettings {
    /// The values as they are in `settings`.
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            layer_height: settings.layer_height,
            infill_density: settings.infill_density,
            wall_loops: settings.wall_loops,
            supports: settings.supports,
        }
    }

    /// Writes the values into `settings`, without checking them.
    pub fn apply(&self, settings: &mut Settings) {
        settings.layer_height = self.layer_height;
        settings.infill_density = self.infill_density;
        settings.wall_loops = self.wall_loops;
        settings.supports = self.supports;
    }
}

/// Splits override text into `(key, value)` pairs: one `key = value` per
/// line; empty lines and lines starting with `#` are skipped.
pub fn parse_overrides(text: &str) -> Result<Vec<(String, String)>, Diagnostic> {
    text.lines()
        .enumerate()
        .map(|(index, line)| (index, line.trim()))
        .filter(|(_, line)| !line.is_empty() && !line.starts_with('#'))
        .map(|(index, line)| {
            let (key, value) = line.split_once('=').ok_or_else(|| {
                Diagnostic::error(
                    "ui.invalid_override",
                    format!("line {}: expected key = value, got '{line}'", index + 1),
                )
                .with("line", index + 1)
            })?;
            Ok((key.trim().to_owned(), value.trim().to_owned()))
        })
        .collect()
}

/// The settings to slice with: `base` (from the files) with the common
/// values and the overrides applied, checked against their limits.
pub fn build_settings(
    base: &Settings,
    common: &CommonSettings,
    overrides: &str,
) -> Result<Settings, Diagnostic> {
    let mut settings = base.clone();
    common.apply(&mut settings);
    settings.validate().map_err(|e| e.diagnostic())?;
    for (key, value) in parse_overrides(overrides)? {
        settings.set(&key, &value).map_err(|e| e.diagnostic())?;
    }
    Ok(settings)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn file(name: &str, text: &str) -> SettingsFile {
        SettingsFile {
            name: name.to_owned(),
            text: text.to_owned(),
        }
    }

    #[test]
    fn later_files_override_earlier_ones() {
        let settings = load_settings(&[
            file("printer.toml", "bed_size_x = 256.0\nlayer_height = 0.28\n"),
            file("process.toml", "layer_height = 0.12\n"),
        ])
        .unwrap();
        assert_eq!(settings.bed_size_x, 256.0);
        assert_eq!(settings.layer_height, 0.12);
        assert_eq!(load_settings(&[]).unwrap(), Settings::default());
    }

    #[test]
    fn errors_name_the_file() {
        let error = load_settings(&[file("bad.toml", "no_such_key = 1\n")]).unwrap_err();
        assert!(error.message.starts_with("bad.toml: "), "{}", error.message);
        assert_eq!(error.context["file"], "bad.toml");
    }

    #[test]
    fn common_values_and_overrides_apply_in_order() {
        let base = Settings::default();
        let mut common = CommonSettings::from_settings(&base);
        common.layer_height = 0.1;
        common.infill_density = 40.0;
        common.wall_loops = 4;
        common.supports = true;
        let settings = build_settings(
            &base,
            &common,
            "# a comment\n\n brim_width = 5 \ninfill_density=20\n",
        )
        .unwrap();
        assert_eq!(settings.layer_height, 0.1);
        assert_eq!(settings.wall_loops, 4);
        assert!(settings.supports);
        assert_eq!(settings.brim_width, 5.0);
        // The override comes last.
        assert_eq!(settings.infill_density, 20.0);
        assert_eq!(CommonSettings::from_settings(&settings).wall_loops, 4);
    }

    #[test]
    fn invalid_values_are_rejected() {
        let base = Settings::default();
        let mut common = CommonSettings::from_settings(&base);
        common.layer_height = 5.0;
        assert_eq!(
            build_settings(&base, &common, "").unwrap_err().code,
            "settings.out_of_range"
        );
        let common = CommonSettings::from_settings(&base);
        assert_eq!(
            build_settings(&base, &common, "layer_height")
                .unwrap_err()
                .code,
            "ui.invalid_override"
        );
        assert_eq!(
            build_settings(&base, &common, "colour = red")
                .unwrap_err()
                .code,
            "settings.unknown_key"
        );
    }
}
