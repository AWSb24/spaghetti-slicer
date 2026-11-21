//! The list of all settings: search, values that differ from the profiles,
//! and edits with validation.
//!
//! Edits made in the list apply last, after the settings files, the values
//! of the side panel and the text overrides; the limits are checked once
//! everything is set, so values that depend on each other can change
//! together. The four settings the side panel shows are changed there
//! instead, so both always agree.

use std::collections::BTreeMap;

use slicer_core::{Diagnose, Diagnostic};
use slicer_profiles::{SETTINGS_INFO, SettingInfo, Settings, SettingsError};

use crate::settings::{CommonSettings, parse_overrides};

/// A tab of process settings: its name and its groups, each a name and
/// the keys in it.
pub type ProcessTab = (
    &'static str,
    &'static [(&'static str, &'static [&'static str])],
);

/// The process settings of the side panel in tabs, as slicers usually
/// group them: each tab has named groups with their keys in the order
/// shown. Everything else is in the list of all settings.
pub const PROCESS_TABS: [ProcessTab; 5] = [
    (
        "Quality",
        &[
            (
                "Layer height",
                &[
                    "layer_height",
                    "first_layer_height",
                    "adaptive_layers",
                    "min_layer_height",
                    "max_layer_height",
                ],
            ),
            ("Line width", &["line_width"]),
            ("Seam", &["seam_position"]),
            ("Walls", &["wall_order"]),
        ],
    ),
    (
        "Strength",
        &[
            ("Walls", &["wall_loops"]),
            (
                "Top/bottom shells",
                &[
                    "top_layers",
                    "bottom_layers",
                    "top_surface_pattern",
                    "bottom_surface_pattern",
                ],
            ),
            (
                "Sparse infill",
                &[
                    "infill_density",
                    "infill_pattern",
                    "infill_angle",
                    "infill_connect",
                    "infill_overlap",
                ],
            ),
        ],
    ),
    (
        "Speed",
        &[
            ("First layer speed", &["first_layer_speed"]),
            (
                "Other layers speed",
                &[
                    "outer_wall_speed",
                    "inner_wall_speed",
                    "infill_speed",
                    "bridge_speed",
                    "support_speed",
                    "support_interface_speed",
                ],
            ),
            ("Travel speed", &["travel_speed"]),
            ("Acceleration", &["acceleration", "jerk"]),
        ],
    ),
    (
        "Support",
        &[
            ("Support", &["supports", "support_style", "support_spacing"]),
            (
                "Advanced",
                &[
                    "support_interface_layers",
                    "support_bottom_interface_layers",
                    "support_interface_pattern",
                    "support_interface_spacing",
                    "support_interface_fan_speed",
                    "support_bridge_length",
                    "support_top_z_distance",
                    "support_bottom_z_distance",
                    "support_xy_distance",
                ],
            ),
        ],
    ),
    (
        "Others",
        &[
            ("Skirt", &["skirt_loops", "skirt_distance"]),
            ("Brim", &["brim_width"]),
            ("Arrange", &["arrange_outlines", "arrange_rotation"]),
            (
                "Prime tower",
                &["prime_tower_width", "prime_tower_x", "prime_tower_y"],
            ),
        ],
    ),
];

/// The unit of a setting's value for the field, e.g. "mm/s", from the
/// settings table ([`SettingInfo::unit`]); empty for counts, switches,
/// choices and unknown keys.
pub fn setting_unit(key: &str) -> &'static str {
    slicer_profiles::setting_info(key).map_or("", |info| info.unit)
}

/// A setting's name for people: `infill_density` becomes "Infill density".
pub fn setting_label(key: &str) -> String {
    let text = key.replace('_', " ");
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The rows of `keys`, in that order; unknown keys are left out.
pub fn rows_of(current: &Settings, base: &Settings, keys: &[&str]) -> Vec<Row> {
    let all = all_rows(current, base);
    keys.iter()
        .filter_map(|key| all.iter().find(|row| row.info.key == *key).cloned())
        .collect()
}

/// Every setting, in the order of [`SETTINGS_INFO`].
fn all_rows(current: &Settings, base: &Settings) -> Vec<Row> {
    let values = current.values();
    let profile = base.values();
    let limits = current.limits();
    SETTINGS_INFO
        .iter()
        .map(|info| Row {
            info,
            value: values.get(info.key).cloned().unwrap_or_default(),
            profile_value: profile.get(info.key).cloned().unwrap_or_default(),
            range: limits
                .iter()
                .find(|(key, ..)| *key == info.key)
                .map(|&(_, _, min, max)| (min, max)),
        })
        .collect()
}

/// One line of the list.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// What the setting is.
    pub info: &'static SettingInfo,
    /// The value sliced with, as text.
    pub value: String,
    /// The value from the settings files (or the default), as text.
    pub profile_value: String,
    /// Smallest and largest allowed value of a number now; some depend on
    /// other settings.
    pub range: Option<(f64, f64)>,
}

impl Row {
    /// Whether the value differs from the settings files.
    pub fn changed(&self) -> bool {
        self.value != self.profile_value
    }
}

/// State of the settings list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SettingsEditor {
    /// Values set in the list, by key, as text.
    edits: BTreeMap<String, String>,
    /// Text typed into a field and not yet accepted, by key.
    pub drafts: BTreeMap<String, String>,
    /// Why the last attempt to set a key failed, by key.
    pub errors: BTreeMap<String, String>,
    /// Text to look for in names, groups and descriptions.
    pub search: String,
    /// Whether only values that differ from the settings files are listed.
    pub only_changed: bool,
}

/// Sets the field `key` of `common` from text. `None` if the side panel
/// does not show `key`.
fn set_common(
    common: &mut CommonSettings,
    key: &str,
    value: &str,
) -> Option<Result<(), Diagnostic>> {
    let value = value.trim();
    let invalid =
        || SettingsError::Parse(format!("cannot parse '{value}' for '{key}'")).diagnostic();
    let number = || value.parse::<f64>().map_err(|_| invalid());
    Some(match key {
        "layer_height" => number().map(|v| common.layer_height = v),
        "infill_density" => number().map(|v| common.infill_density = v),
        "wall_loops" => value
            .parse()
            .map(|v| common.wall_loops = v)
            .map_err(|_| invalid()),
        "supports" => value
            .parse()
            .map(|v| common.supports = v)
            .map_err(|_| invalid()),
        _ => return None,
    })
}

impl SettingsEditor {
    /// The settings to slice with: `base` (from the files) with the values
    /// of the side panel, the `overrides` text and the edits of the list.
    /// Replaces [`crate::settings::build_settings`] once the list is used.
    pub fn build(
        &self,
        base: &Settings,
        common: &CommonSettings,
        overrides: &str,
    ) -> Result<Settings, Diagnostic> {
        build(base, common, overrides, &self.edits)
    }

    /// Number of values set in the list.
    pub fn edit_count(&self) -> usize {
        self.edits.len()
    }

    /// Puts `edits` in place of the values set in the list so far, for
    /// example the values one object on the plate sets for itself. Drafts
    /// and errors stay.
    pub fn set_edits(&mut self, edits: BTreeMap<String, String>) {
        self.edits = edits;
    }

    /// The values set in the list, by key, as text.
    pub fn edits(&self) -> &BTreeMap<String, String> {
        &self.edits
    }

    /// Replaces the values set in the list, for example with those of an
    /// opened project; text typed and not accepted is dropped.
    pub fn replace_edits(&mut self, edits: BTreeMap<String, String>) {
        self.edits = edits;
        self.drafts.clear();
        self.errors.clear();
    }

    /// Every setting that matches the search, in the order of
    /// [`SETTINGS_INFO`]. `current` are the settings sliced with, `base`
    /// those from the settings files.
    pub fn rows(&self, current: &Settings, base: &Settings) -> Vec<Row> {
        let search = self.search.trim().to_lowercase();
        all_rows(current, base)
            .into_iter()
            .filter(|row| !self.only_changed || row.changed())
            .filter(|row| {
                search.is_empty()
                    || [row.info.key, row.info.group, row.info.description]
                        .iter()
                        .any(|text| text.to_lowercase().contains(&search))
            })
            .collect()
    }

    /// Sets `key` to `value` if the settings stay valid, otherwise keeps
    /// everything and records the error. `base`, `common` and `overrides`
    /// are the other sources of settings, see [`SettingsEditor::build`].
    pub fn edit(
        &mut self,
        base: &Settings,
        common: &mut CommonSettings,
        overrides: &str,
        key: &str,
        value: &str,
    ) -> Result<(), Diagnostic> {
        let outcome = self.try_edit(base, common, overrides, key, value);
        match &outcome {
            Ok(()) => {
                self.drafts.remove(key);
                self.errors.remove(key);
            }
            Err(error) => {
                self.errors.insert(key.to_owned(), error.message.clone());
            }
        }
        outcome
    }

    fn try_edit(
        &mut self,
        base: &Settings,
        common: &mut CommonSettings,
        overrides: &str,
        key: &str,
        value: &str,
    ) -> Result<(), Diagnostic> {
        let mut new_common = *common;
        let mut edits = self.edits.clone();
        match set_common(&mut new_common, key, value) {
            Some(result) => {
                result?;
                edits.remove(key);
            }
            None => {
                edits.insert(key.to_owned(), value.to_owned());
            }
        }
        let result = build(base, &new_common, overrides, &edits)?;
        // An edit that changes nothing is dropped.
        let without = {
            let mut others = edits.clone();
            others.remove(key);
            build(base, &new_common, overrides, &others).ok()
        };
        if without.as_ref() == Some(&result) {
            edits.remove(key);
        }
        *common = new_common;
        self.edits = edits;
        Ok(())
    }

    /// Takes `key` back to its value from the settings files.
    pub fn reset(&mut self, base: &Settings, common: &mut CommonSettings, key: &str) {
        let profile = base.values();
        if let Some(value) = profile.get(key) {
            let _ = set_common(common, key, value);
        }
        self.edits.remove(key);
        self.drafts.remove(key);
        self.errors.remove(key);
    }
}

/// The values an object on the plate sets for itself after its settings
/// went from `before` to `after` in the editor: `overrides` with every key
/// of [`slicer_fdm::OBJECT_SETTINGS`] that changed set to its new value, or
/// dropped where that is the value of the plate (`plate`). Keys that did
/// not change stay as they are, also if the plate has the same value now.
pub fn changed_overrides(
    overrides: &BTreeMap<String, String>,
    plate: &Settings,
    before: &Settings,
    after: &Settings,
) -> BTreeMap<String, String> {
    let (plate, before, after) = (plate.values(), before.values(), after.values());
    let mut changed = overrides.clone();
    for key in slicer_fdm::OBJECT_SETTINGS {
        let (Some(old), Some(new)) = (before.get(*key), after.get(*key)) else {
            continue;
        };
        if old == new {
            continue;
        }
        if plate.get(*key) == Some(new) {
            changed.remove(*key);
        } else {
            changed.insert((*key).to_owned(), new.clone());
        }
    }
    changed
}

fn build(
    base: &Settings,
    common: &CommonSettings,
    overrides: &str,
    edits: &BTreeMap<String, String>,
) -> Result<Settings, Diagnostic> {
    let mut settings = base.clone();
    common.apply(&mut settings);
    let overrides = parse_overrides(overrides)?;
    // A later value of the same key wins.
    let values = overrides
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .chain(
            edits
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        );
    settings.set_all(values).map_err(|e| e.diagnostic())?;
    Ok(settings)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// Files, side panel, overrides and list as the window combines them.
    fn current(base: &Settings, common: &CommonSettings, editor: &SettingsEditor) -> Settings {
        editor.build(base, common, "").unwrap()
    }

    #[test]
    fn the_list_covers_every_setting_with_value_and_range() {
        let base = Settings::default();
        let editor = SettingsEditor::default();
        let rows = editor.rows(&base, &base);
        assert_eq!(rows.len(), base.values().len());
        assert!(rows.iter().all(|row| !row.changed()));
        let layer = rows.iter().find(|r| r.info.key == "layer_height").unwrap();
        assert_eq!(layer.value, "0.2");
        assert_eq!(layer.range, Some((0.02, 0.4)));
        assert!(
            rows.iter()
                .find(|r| r.info.key == "flavor")
                .unwrap()
                .range
                .is_none()
        );

        let mut search = SettingsEditor {
            search: "BRIDGE".to_owned(),
            ..SettingsEditor::default()
        };
        let keys: Vec<&str> = search
            .rows(&base, &base)
            .iter()
            .map(|r| r.info.key)
            .collect();
        assert!(keys.contains(&"bridge_speed") && keys.contains(&"support_bridge_length"));
        assert!(!keys.contains(&"nozzle_temp"));
        // Descriptions are searched too.
        search.search = "g/cm".to_owned();
        assert_eq!(search.rows(&base, &base)[0].info.key, "filament_density");
    }

    #[test]
    fn process_tabs_name_known_settings_once() {
        let base = Settings::default();
        let mut seen = Vec::new();
        for (_, groups) in PROCESS_TABS {
            for (_, keys) in groups.iter() {
                let rows = rows_of(&base, &base, keys);
                let shown: Vec<&str> = rows.iter().map(|r| r.info.key).collect();
                assert_eq!(shown, *keys, "every key is a setting");
                for key in keys.iter() {
                    assert!(!seen.contains(key), "{key} is in two places");
                    seen.push(key);
                }
            }
        }
        assert!(rows_of(&base, &base, &["nope"]).is_empty());
        assert_eq!(setting_label("infill_density"), "Infill density");
        assert_eq!(setting_label("jerk"), "Jerk");
        assert_eq!(setting_label(""), "");
        for (key, unit) in [
            ("layer_height", "mm"),
            ("outer_wall_speed", "mm/s"),
            ("acceleration", "mm/s²"),
            ("infill_density", "%"),
            ("infill_angle", "°"),
            ("nozzle_temp", "°C"),
            ("nozzle_temp_first_layer", "°C"),
            ("prime_tower_x", "mm"),
            ("retraction_min_travel", "mm"),
            ("fan_speed", "%"),
            ("overhang_fan_speed", "%"),
            ("fan_max_speed", "%"),
            ("support_speed", "mm/s"),
            ("support_interface_speed", "mm/s"),
            ("support_interface_fan_speed", "%"),
            ("support_interface_spacing", "mm"),
            ("min_layer_time", "s"),
            ("fan_layer_time", "s"),
            ("ironing_flow", "%"),
            ("arc_tolerance", "mm"),
            ("elephant_foot_compensation", "mm"),
            ("wall_loops", ""),
            ("supports", ""),
        ] {
            assert_eq!(setting_unit(key), unit, "{key}");
        }
    }

    #[test]
    fn without_edits_the_settings_are_built_as_before() {
        let base = Settings::default();
        let mut common = CommonSettings::from_settings(&base);
        common.wall_loops = 5;
        let editor = SettingsEditor::default();
        for overrides in [
            "brim_width = 5\nbrim_width = 6",
            "",
            "nope = 1",
            "x",
            "z_hop = 99",
        ] {
            assert_eq!(
                editor.build(&base, &common, overrides).map_err(|e| e.code),
                crate::settings::build_settings(&base, &common, overrides).map_err(|e| e.code),
                "{overrides}"
            );
        }
    }

    #[test]
    fn edits_are_validated_and_marked() {
        let base = Settings::default();
        let mut common = CommonSettings::from_settings(&base);
        let mut editor = SettingsEditor::default();

        editor
            .edit(&base, &mut common, "", "brim_width", "5")
            .unwrap();
        editor
            .edit(&base, &mut common, "", "flavor", "klipper")
            .unwrap();
        editor
            .edit(&base, &mut common, "", "end_gcode", "M84\nM104 S0")
            .unwrap();
        let settings = current(&base, &common, &editor);
        assert_eq!(settings.brim_width, 5.0);
        assert_eq!(settings.end_gcode, "M84\nM104 S0");
        assert_eq!(editor.edit_count(), 3);

        // Out of range, wrong type, unknown choice: nothing changes.
        editor.drafts.insert("brim_width".into(), "500".into());
        let error = editor
            .edit(&base, &mut common, "", "brim_width", "500")
            .unwrap_err();
        assert_eq!(error.code, "settings.out_of_range");
        assert!(editor.errors["brim_width"].contains("0 to 50"));
        assert_eq!(editor.drafts["brim_width"], "500");
        assert!(
            editor
                .edit(&base, &mut common, "", "wall_order", "random")
                .is_err()
        );
        assert!(
            editor
                .edit(&base, &mut common, "", "fan_speed", "loud")
                .is_err()
        );
        assert_eq!(current(&base, &common, &editor), settings);
        // A good value clears the error and the draft.
        editor
            .edit(&base, &mut common, "", "brim_width", "8")
            .unwrap();
        assert!(!editor.errors.contains_key("brim_width") && editor.drafts.is_empty());

        // Settings of the side panel are changed there.
        editor
            .edit(&base, &mut common, "", "wall_loops", "4")
            .unwrap();
        assert_eq!(common.wall_loops, 4);
        assert!(
            editor
                .edit(&base, &mut common, "", "layer_height", "0.9")
                .is_err()
        );
        assert_eq!(common.layer_height, 0.2);

        let settings = current(&base, &common, &editor);
        editor.only_changed = true;
        let changed: Vec<&str> = editor
            .rows(&settings, &base)
            .iter()
            .map(|r| r.info.key)
            .collect();
        assert_eq!(changed, ["flavor", "wall_loops", "brim_width", "end_gcode"]);

        // Back to the profile values.
        for key in changed {
            editor.reset(&base, &mut common, key);
        }
        assert_eq!(current(&base, &common, &editor), base);
        assert_eq!(editor.edit_count(), 0);
    }

    #[test]
    fn values_that_depend_on_each_other_change_together() {
        let base = Settings::default();
        let mut common = CommonSettings::from_settings(&base);
        let mut editor = SettingsEditor::default();
        // Too narrow a line for the new nozzle: rejected.
        assert!(
            editor
                .edit(&base, &mut common, "", "nozzle_diameter", "0.8")
                .is_err()
        );
        editor
            .edit(&base, &mut common, "", "line_width", "0.9")
            .unwrap();
        editor
            .edit(&base, &mut common, "", "nozzle_diameter", "0.8")
            .unwrap();
        // The side panel's layer height is set before the list's nozzle
        // and is still checked against it.
        editor
            .edit(&base, &mut common, "", "layer_height", "0.6")
            .unwrap();
        let settings = current(&base, &common, &editor);
        assert_eq!(
            (settings.nozzle_diameter, settings.layer_height),
            (0.8, 0.6)
        );
        // Setting a value back to what it was drops the edit.
        editor
            .edit(&base, &mut common, "", "line_width", "0.9")
            .unwrap();
        editor
            .edit(&base, &mut common, "", "brim_width", "0")
            .unwrap();
        assert_eq!(editor.edit_count(), 2);
    }

    #[test]
    fn objects_keep_the_values_they_set_for_themselves() {
        let plate = Settings::default();
        let mut editor = SettingsEditor::default();
        // What the object section does in a frame: the object's values on
        // top of the plate's, one edit, and the values that result.
        let mut step = |overrides: &BTreeMap<String, String>, key: &str, value: Option<&str>| {
            editor.set_edits(overrides.clone());
            let mut common = CommonSettings::from_settings(&plate);
            let before = editor.build(&plate, &common, "").unwrap();
            match value {
                Some(value) => editor.edit(&plate, &mut common, "", key, value).unwrap(),
                None => editor.reset(&plate, &mut common, key),
            }
            let after = editor.build(&plate, &common, "").unwrap();
            changed_overrides(overrides, &plate, &before, &after)
        };
        // One of the side panel's values and one of the list's.
        let own = step(&BTreeMap::new(), "wall_loops", Some("4"));
        let own = step(&own, "infill_pattern", Some("gyroid"));
        let expected = BTreeMap::from([
            ("infill_pattern".to_owned(), "gyroid".to_owned()),
            ("wall_loops".to_owned(), "4".to_owned()),
        ]);
        assert_eq!(own, expected);
        // Reset, or set to the plate's value, the key is dropped.
        let own = step(&own, "wall_loops", None);
        let own = step(&own, "infill_pattern", Some("grid"));
        assert!(own.is_empty(), "{own:?}");
        // A value that equals the plate's now stays while not edited.
        let kept = BTreeMap::from([("infill_density".to_owned(), "15".to_owned())]);
        assert_eq!(step(&kept, "top_layers", Some("6")).len(), 2);
    }
}
