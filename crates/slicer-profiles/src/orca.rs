//! Import of profiles exported or saved by Orca Slicer (JSON), and of the
//! settings stored in project files of Orca Slicer and Bambu Studio, which
//! use the same names.
//!
//! The key names and value formats were taken from profiles and project
//! files a user saved, read as data files; no source code or shipped
//! profile of Orca Slicer was used. Only settings with a clear counterpart
//! here are taken, everything else is reported.

use serde_json::{Map, Value};

use crate::{Settings, SettingsError};

/// How the value of an Orca setting becomes one of ours.
#[derive(Clone, Copy)]
enum Kind {
    /// A plain number; relative values ("50%") have no counterpart.
    Number,
    /// A percentage, written with or without "%".
    Percent,
    /// "0" or "1".
    Flag,
    /// Corners of the bed as "XxY" texts.
    BedArea,
    /// An angle, folded into 0 to 180 degrees.
    Angle,
    /// Text values with a counterpart each; others are not supported.
    Choice(&'static [(&'static str, &'static str)]),
    /// The kind of brim: "no_brim" means a brim width of 0, the kinds that
    /// go around the outside keep the width, the others are not supported.
    BrimType,
    /// A square table as a list of numbers, row by row.
    Table,
    /// One text per filament, joined by commas.
    List,
    /// One colour per filament (`#RRGGBB`, or `#RRGGBBAA` with opacity),
    /// joined by commas.
    Colours,
}

/// Orca setting, our setting, and how to convert.
const MAPPING: &[(&str, &str, Kind)] = &[
    // Printer
    ("printable_area", "bed_size_x", Kind::BedArea),
    ("printable_height", "max_print_height", Kind::Number),
    ("nozzle_diameter", "nozzle_diameter", Kind::Number),
    ("retraction_length", "retraction_length", Kind::Number),
    ("retraction_speed", "retraction_speed", Kind::Number),
    ("z_hop", "z_hop", Kind::Number),
    // Material
    ("filament_diameter", "filament_diameter", Kind::Number),
    ("filament_density", "filament_density", Kind::Number),
    ("filament_flow_ratio", "flow_ratio", Kind::Number),
    ("nozzle_temperature", "nozzle_temp", Kind::Number),
    (
        "nozzle_temperature_initial_layer",
        "nozzle_temp_first_layer",
        Kind::Number,
    ),
    ("hot_plate_temp", "bed_temp", Kind::Number),
    (
        "hot_plate_temp_initial_layer",
        "bed_temp_first_layer",
        Kind::Number,
    ),
    ("fan_max_speed", "fan_speed", Kind::Number),
    (
        "support_material_interface_fan_speed",
        "support_interface_fan_speed",
        Kind::Number,
    ),
    (
        "close_fan_the_first_x_layers",
        "fan_off_layers",
        Kind::Number,
    ),
    (
        "filament_retraction_length",
        "retraction_length",
        Kind::Number,
    ),
    (
        "filament_retraction_speed",
        "retraction_speed",
        Kind::Number,
    ),
    ("filament_z_hop", "z_hop", Kind::Number),
    // Process
    ("layer_height", "layer_height", Kind::Number),
    (
        "initial_layer_print_height",
        "first_layer_height",
        Kind::Number,
    ),
    ("line_width", "line_width", Kind::Number),
    ("wall_loops", "wall_loops", Kind::Number),
    ("top_shell_layers", "top_layers", Kind::Number),
    ("bottom_shell_layers", "bottom_layers", Kind::Number),
    ("sparse_infill_density", "infill_density", Kind::Percent),
    (
        "sparse_infill_pattern",
        "infill_pattern",
        Kind::Choice(&[
            ("grid", "grid"),
            ("triangles", "triangles"),
            ("gyroid", "gyroid"),
            ("honeycomb", "honeycomb"),
            ("cubic", "cubic"),
            ("concentric", "concentric"),
            ("lightning", "lightning"),
        ]),
    ),
    (
        "top_surface_pattern",
        "top_surface_pattern",
        Kind::Choice(&[("monotonic", "monotonic"), ("concentric", "concentric")]),
    ),
    (
        "bottom_surface_pattern",
        "bottom_surface_pattern",
        Kind::Choice(&[("monotonic", "monotonic"), ("concentric", "concentric")]),
    ),
    ("infill_direction", "infill_angle", Kind::Angle),
    ("infill_wall_overlap", "infill_overlap", Kind::Percent),
    (
        "seam_position",
        "seam_position",
        Kind::Choice(&[("back", "rear"), ("nearest", "nearest")]),
    ),
    ("skirt_loops", "skirt_loops", Kind::Number),
    ("skirt_distance", "skirt_distance", Kind::Number),
    ("brim_width", "brim_width", Kind::Number),
    ("brim_type", "brim_width", Kind::BrimType),
    ("raft_layers", "raft_layers", Kind::Number),
    ("raft_expansion", "raft_expansion", Kind::Number),
    (
        "raft_contact_distance",
        "raft_contact_distance",
        Kind::Number,
    ),
    ("enable_support", "supports", Kind::Flag),
    // ponytail: Orca's 0 (no gap, for soluble supports) becomes one layer
    // here unless `support_soluble` is set, which has no Orca key of its
    // own (Orca marks the filament as soluble instead).
    (
        "support_top_z_distance",
        "support_top_z_distance",
        Kind::Number,
    ),
    (
        "support_bottom_z_distance",
        "support_bottom_z_distance",
        Kind::Number,
    ),
    (
        "support_object_xy_distance",
        "support_xy_distance",
        Kind::Number,
    ),
    // Tree supports, by the names in Orca's user documentation. Orca's
    // branch distance is the distance between neighbouring branches where
    // they start.
    // ponytail: the organic style has keys of its own for branch distance,
    // diameter and angle (`..._organic`); only the plain ones are taken.
    (
        "tree_support_branch_angle",
        "tree_branch_angle",
        Kind::Number,
    ),
    (
        "tree_support_branch_diameter",
        "tree_branch_diameter",
        Kind::Number,
    ),
    (
        "tree_support_tip_diameter",
        "tree_tip_diameter",
        Kind::Number,
    ),
    (
        "tree_support_branch_distance",
        "tree_contact_spacing",
        Kind::Number,
    ),
    // Filaments counted from 1, 0 for the default, as our extruders.
    ("support_filament", "support_extruder", Kind::Number),
    (
        "support_interface_filament",
        "support_interface_extruder",
        Kind::Number,
    ),
    ("outer_wall_speed", "outer_wall_speed", Kind::Number),
    ("inner_wall_speed", "inner_wall_speed", Kind::Number),
    ("sparse_infill_speed", "infill_speed", Kind::Number),
    ("bridge_speed", "bridge_speed", Kind::Number),
    ("support_speed", "support_speed", Kind::Number),
    (
        "support_interface_speed",
        "support_interface_speed",
        Kind::Number,
    ),
    ("initial_layer_speed", "first_layer_speed", Kind::Number),
    ("travel_speed", "travel_speed", Kind::Number),
    ("default_acceleration", "acceleration", Kind::Number),
    ("default_jerk", "jerk", Kind::Number),
    // Multiple filaments
    ("flush_volumes_matrix", "flush_volumes", Kind::Table),
    ("flush_multiplier", "flush_multiplier", Kind::Number),
    ("filament_colour", "filament_colours", Kind::Colours),
    ("filament_type", "filament_type", Kind::List),
];

/// Keys that describe the profile itself, not the print.
const METADATA: &[&str] = &[
    "name",
    "from",
    "inherits",
    "version",
    "setting_id",
    "instantiation",
    "print_settings_id",
    "printer_settings_id",
    "filament_settings_id",
    "compatible_printers",
    "compatible_printers_condition",
    "compatible_prints",
    "compatible_prints_condition",
    "filament_vendor",
    "filament_cost",
    "default_filament_colour",
    // Project files
    "different_settings_to_system",
    "inherits_group",
    "printer_model",
    "printer_variant",
    "print_compatible_printers",
    "upward_compatible_machine",
];

/// What was found in one Orca profile.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OrcaImport {
    /// Name of the profile.
    pub name: Option<String>,
    /// The Orca preset this profile builds on. Its values are not part of
    /// the file and therefore not imported.
    pub inherits: Option<String>,
    /// Our setting, its value as text, and the Orca setting it came from.
    pub taken: Vec<(String, String, String)>,
    /// Orca settings that were not taken, with the reason.
    pub skipped: Vec<(String, String)>,
}

/// The first entry of a per-extruder list, or the value itself. `None` for
/// values Orca uses for "not set".
fn text(value: &Value) -> Option<String> {
    let value = match value {
        Value::Array(items) => items.first()?,
        other => other,
    };
    let text = match value {
        Value::String(text) => text.trim().to_string(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        _ => return None,
    };
    (!text.is_empty() && text != "nil").then_some(text)
}

/// Converts one value; `Err` says why it has no counterpart.
fn convert(kind: Kind, value: &str) -> Result<String, String> {
    let number = |text: &str| {
        text.parse::<f64>()
            .map_err(|_| format!("'{value}' is not a number"))
    };
    match kind {
        Kind::Number if value.ends_with('%') => {
            Err(format!("relative value '{value}' has no counterpart"))
        }
        Kind::Number => Ok(value.to_string()),
        Kind::Percent => Ok(number(value.trim_end_matches('%'))?.to_string()),
        Kind::Flag => match value {
            "1" | "true" => Ok("true".into()),
            "0" | "false" => Ok("false".into()),
            _ => Err(format!("'{value}' is not 0 or 1")),
        },
        Kind::Angle => Ok(number(value)?.rem_euclid(180.0).to_string()),
        Kind::Choice(choices) => choices
            .iter()
            .find(|(orca, _)| *orca == value)
            .map(|(_, ours)| (*ours).to_string())
            .ok_or_else(|| format!("'{value}' is not available")),
        // Needs the whole list; see `bed_size`.
        Kind::BedArea => Err(format!("'{value}' is not a list of corners")),
        // Need the whole list; see `table` and `list`.
        Kind::Table => Err(format!("'{value}' is not a list of numbers")),
        Kind::List | Kind::Colours => Err(format!("'{value}' is not a list")),
        Kind::BrimType => match value {
            "no_brim" => Ok("0".into()),
            _ => Err(format!("'{value}' is not available")),
        },
    }
}

/// Width and depth of the bed from its corners ("0x0", "256x0", ...).
fn bed_size(value: &Value) -> Option<(f64, f64)> {
    let corners: Vec<(f64, f64)> = value
        .as_array()?
        .iter()
        .map(|corner| {
            let (x, y) = corner.as_str()?.split_once('x')?;
            Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
        })
        .collect::<Option<_>>()?;
    let span = |pick: fn(&(f64, f64)) -> f64| {
        let values = corners.iter().map(pick);
        values.clone().fold(f64::MIN, f64::max) - values.fold(f64::MAX, f64::min)
    };
    (corners.len() >= 3).then(|| (span(|c| c.0), span(|c| c.1)))
}

/// A list of n × n numbers (as texts or numbers) as rows separated by `;`,
/// such as `0,280;230,0`.
fn table(value: &Value) -> Option<String> {
    let numbers: Vec<String> = value
        .as_array()?
        .iter()
        .map(|item| match item {
            Value::String(text) => text.trim().parse::<f64>().ok(),
            Value::Number(number) => number.as_f64(),
            _ => None,
        })
        .map(|number| number.map(|n| n.to_string()))
        .collect::<Option<_>>()?;
    let side = (1..=numbers.len()).find(|n| n * n >= numbers.len())?;
    (side * side == numbers.len()).then(|| {
        let rows: Vec<String> = numbers.chunks(side).map(|row| row.join(",")).collect();
        rows.join(";")
    })
}

/// The texts of a list with one per filament (or a single text) joined by
/// commas; `None` if an entry is not a text. Colours lose their opacity.
fn list(value: &Value, colours: bool) -> Option<String> {
    let items = match value {
        Value::Array(items) => items.as_slice(),
        single => std::slice::from_ref(single),
    };
    let entries: Vec<&str> = items
        .iter()
        .map(|item| {
            let text = item.as_str()?.trim();
            // ponytail: the opacity of `#RRGGBBAA` is dropped; filaments
            // are opaque here.
            Some(match text.get(..7) {
                Some(rgb) if colours && text.len() == 9 && text.starts_with('#') => rgb,
                _ => text,
            })
        })
        .collect::<Option<_>>()?;
    Some(entries.join(","))
}

/// Reads one Orca profile (printer, material or process) and checks every
/// taken value against `settings`, which receives them. Values that our
/// settings reject are reported as skipped. The settings of a project file
/// (`Metadata/project_settings.config`) are read the same way; of lists
/// with one value per filament or extruder the first is taken.
pub fn import_orca(json: &str, settings: &mut Settings) -> Result<OrcaImport, SettingsError> {
    let profile: Value =
        serde_json::from_str(json).map_err(|e| SettingsError::Parse(e.to_string()))?;
    let Some(entries) = profile.as_object() else {
        return Err(SettingsError::Parse(
            "an Orca profile is a JSON object".into(),
        ));
    };
    Ok(import_entries(entries, settings))
}

/// Like [`import_orca`], for settings given as pairs of Orca name and
/// value text, as a project file stores those of a single object.
pub fn import_orca_values<'a>(
    values: impl IntoIterator<Item = (&'a str, &'a str)>,
    settings: &mut Settings,
) -> OrcaImport {
    let entries: Map<String, Value> = values
        .into_iter()
        .map(|(key, value)| (key.to_string(), Value::String(value.to_string())))
        .collect();
    import_entries(&entries, settings)
}

/// Takes the settings of `entries` into `settings`; see [`import_orca`].
fn import_entries(entries: &Map<String, Value>, settings: &mut Settings) -> OrcaImport {
    let mut import = OrcaImport {
        name: entries.get("name").and_then(text),
        inherits: entries.get("inherits").and_then(text),
        ..OrcaImport::default()
    };
    // Keys in a fixed order, so the report does not depend on the file.
    let mut keys: Vec<&String> = entries.keys().collect();
    keys.sort();
    for key in keys {
        let value = &entries[key];
        if METADATA.contains(&key.as_str()) {
            continue;
        }
        let Some((_, ours, kind)) = MAPPING.iter().find(|(orca, ..)| orca == key) else {
            if text(value).is_some() {
                import
                    .skipped
                    .push((key.clone(), "no matching setting".into()));
            }
            continue;
        };
        let converted = if let Kind::BedArea = kind {
            match bed_size(value) {
                Some((x, y)) => Ok(vec![
                    ("bed_size_x".to_string(), x.to_string()),
                    ("bed_size_y".to_string(), y.to_string()),
                ]),
                None => Err("not a list of corners".to_string()),
            }
        } else if let Kind::Table = kind {
            match table(value) {
                Some(text) => Ok(vec![((*ours).to_string(), text)]),
                None => Err("not a square table of numbers".to_string()),
            }
        } else if let Kind::List | Kind::Colours = kind {
            match list(value, matches!(kind, Kind::Colours)) {
                Some(text) => Ok(vec![((*ours).to_string(), text)]),
                None => Err("not a list of texts".to_string()),
            }
        } else {
            let Some(value) = text(value) else { continue };
            let no_brim = entries.get("brim_type").and_then(text).as_deref() == Some("no_brim");
            match kind {
                // The width of a brim that is not printed does not matter.
                Kind::Number if *ours == "brim_width" && no_brim => continue,
                // These kinds use the width as it is.
                Kind::BrimType if matches!(value.as_str(), "outer_only" | "auto_brim") => continue,
                _ => {}
            }
            convert(*kind, &value).map(|value| vec![((*ours).to_string(), value)])
        };
        let converted = converted.and_then(|pairs| {
            let mut trial = settings.clone();
            for (name, value) in &pairs {
                trial.set(name, value).map_err(|e| e.to_string())?;
            }
            *settings = trial;
            Ok(pairs)
        });
        match converted {
            Ok(pairs) => {
                for (name, value) in pairs {
                    import.taken.push((name, value, key.clone()));
                }
            }
            Err(reason) => import.skipped.push((key.clone(), reason)),
        }
    }
    import
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::{InfillPattern, SeamPosition, SurfacePattern};

    #[test]
    fn profiles_are_translated_and_the_rest_reported() {
        let mut settings = Settings::default();
        let machine = r#"{
            "name": "My printer", "from": "User", "inherits": "Some printer 0.4 nozzle",
            "printable_area": ["0x0", "256x0", "256x256", "0x256"],
            "printable_height": "250",
            "retraction_length": ["0.6"],
            "machine_start_gcode": "G28"
        }"#;
        let filament = r#"{
            "name": "My PLA", "inherits": "Generic PLA",
            "nozzle_temperature": ["225"],
            "hot_plate_temp_initial_layer": ["60"],
            "filament_flow_ratio": ["0.95"],
            "filament_retraction_length": ["nil"],
            "pressure_advance": ["0.02"]
        }"#;
        let process = r#"{
            "name": "Fine", "inherits": "0.16mm Optimal",
            "layer_height": "0.16",
            "wall_loops": "3",
            "sparse_infill_density": "10%",
            "sparse_infill_pattern": "gyroid",
            "infill_wall_overlap": "25%",
            "infill_direction": "225",
            "seam_position": "back",
            "enable_support": "1",
            "bridge_acceleration": "200%",
            "skirt_loops": "1.5"
        }"#;
        let machine = import_orca(machine, &mut settings).unwrap();
        let filament = import_orca(filament, &mut settings).unwrap();
        let process = import_orca(process, &mut settings).unwrap();

        assert_eq!(machine.inherits.as_deref(), Some("Some printer 0.4 nozzle"));
        assert_eq!((settings.bed_size_x, settings.bed_size_y), (256.0, 256.0));
        assert_eq!(settings.max_print_height, 250.0);
        // "nil" leaves the printer's value alone.
        assert_eq!(settings.retraction_length, 0.6);
        assert_eq!(settings.nozzle_temp, 225.0);
        assert_eq!(settings.bed_temp_first_layer, 60.0);
        assert_eq!(settings.flow_ratio, 0.95);
        assert_eq!(settings.layer_height, 0.16);
        assert_eq!(settings.wall_loops, 3);
        assert_eq!(settings.infill_density, 10.0);
        assert_eq!(settings.infill_pattern, InfillPattern::Gyroid);
        assert_eq!(settings.infill_overlap, 25.0);
        assert_eq!(settings.infill_angle, 45.0);
        assert_eq!(settings.seam_position, SeamPosition::Rear);
        assert!(settings.supports);

        let skipped = |import: &OrcaImport| -> Vec<String> {
            import.skipped.iter().map(|(key, _)| key.clone()).collect()
        };
        assert_eq!(skipped(&machine), ["machine_start_gcode"]);
        assert_eq!(skipped(&filament), ["pressure_advance"]);
        assert_eq!(skipped(&process), ["bridge_acceleration", "skirt_loops"]);
        assert_eq!(process.taken.len(), 8);

        // Patterns without a counterpart leave the setting alone.
        let other = r#"{ "sparse_infill_pattern": "crosshatch" }"#;
        let other = import_orca(other, &mut settings).unwrap();
        assert_eq!(skipped(&other), ["sparse_infill_pattern"]);
        assert_eq!(settings.infill_pattern, InfillPattern::Gyroid);
        let lightning = r#"{ "sparse_infill_pattern": "lightning" }"#;
        let lightning = import_orca(lightning, &mut settings).unwrap();
        assert!(lightning.skipped.is_empty());
        assert_eq!(settings.infill_pattern, InfillPattern::Lightning);
        let patterns = r#"{ "sparse_infill_pattern": "honeycomb",
            "top_surface_pattern": "monotonic", "bottom_surface_pattern": "concentric" }"#;
        let patterns = import_orca(patterns, &mut settings).unwrap();
        assert!(patterns.skipped.is_empty());
        assert_eq!(settings.infill_pattern, InfillPattern::Honeycomb);
        assert_eq!(settings.top_surface_pattern, SurfacePattern::Monotonic);
        assert_eq!(settings.bottom_surface_pattern, SurfacePattern::Concentric);

        // Speeds and fan of supports and their interface; -1 keeps the
        // fan of the layer in both.
        let supports = r#"{ "support_speed": "70", "support_interface_speed": "35",
            "support_material_interface_fan_speed": ["-1"] }"#;
        let supports = import_orca(supports, &mut settings).unwrap();
        assert!(supports.skipped.is_empty());
        assert_eq!(settings.support_speed, 70.0);
        assert_eq!(settings.support_interface_speed, 35.0);
        assert_eq!(settings.support_interface_fan_speed, -1.0);

        // The gaps around supports.
        let gaps = r#"{ "support_top_z_distance": "0.16", "support_bottom_z_distance": "0.2",
            "support_object_xy_distance": "0.5" }"#;
        let gaps = import_orca(gaps, &mut settings).unwrap();
        assert!(gaps.skipped.is_empty());
        assert_eq!(settings.support_top_z_distance, 0.16);
        assert_eq!(settings.support_bottom_z_distance, 0.2);
        assert_eq!(settings.support_xy_distance, 0.5);

        // Tree supports.
        let tree = r#"{ "tree_support_branch_angle": "40", "tree_support_branch_diameter": "5",
            "tree_support_tip_diameter": "0.8", "tree_support_branch_distance": "3" }"#;
        let tree = import_orca(tree, &mut settings).unwrap();
        assert!(tree.skipped.is_empty());
        assert_eq!(settings.tree_branch_angle, 40.0);
        assert_eq!(settings.tree_branch_diameter, 5.0);
        assert_eq!(settings.tree_tip_diameter, 0.8);
        assert_eq!(settings.tree_contact_spacing, 3.0);

        // The filaments of supports and their interface.
        let filaments = r#"{ "support_filament": "2", "support_interface_filament": "0" }"#;
        let filaments = import_orca(filaments, &mut settings).unwrap();
        assert!(filaments.skipped.is_empty());
        assert_eq!(settings.support_extruder, 2);
        assert_eq!(settings.support_interface_extruder, 0);
    }

    #[test]
    fn brim_kind_and_value_pairs() {
        // No brim: the width does not count.
        let mut settings = Settings::default();
        let project = r#"{ "brim_type": "no_brim", "brim_width": "5" }"#;
        let import = import_orca(project, &mut settings).unwrap();
        assert_eq!(settings.brim_width, 0.0);
        assert!(import.skipped.is_empty(), "{:?}", import.skipped);
        let outer = r#"{ "brim_type": "outer_only", "brim_width": "4" }"#;
        let import = import_orca(outer, &mut settings).unwrap();
        assert_eq!(settings.brim_width, 4.0);
        assert!(import.skipped.is_empty(), "{:?}", import.skipped);
        let ears = r#"{ "brim_type": "brim_ears" }"#;
        let import = import_orca(ears, &mut settings).unwrap();
        assert_eq!(import.skipped.len(), 1);
        assert_eq!(settings.brim_width, 4.0);

        // Pairs of text, as a project stores the settings of one object.
        let mut settings = Settings::default();
        let import = import_orca_values(
            [
                ("sparse_infill_density", "40%"),
                ("wall_loops", "4"),
                ("support_type", "tree(auto)"),
            ],
            &mut settings,
        );
        assert_eq!(settings.infill_density, 40.0);
        assert_eq!(settings.wall_loops, 4);
        assert_eq!(import.taken.len(), 2);
        assert_eq!(
            import.skipped,
            [(
                "support_type".to_string(),
                "no matching setting".to_string()
            )]
        );
    }

    #[test]
    fn flush_volumes_come_from_projects() {
        // As Bambu Studio stores them: row i is the change from filament i.
        let project = r#"{
            "flush_volumes_matrix": ["0", "353", "191", "298", "0", "210", "463", "487", "0"],
            "flush_multiplier": ["1.5"]
        }"#;
        let mut settings = Settings::default();
        let import = import_orca(project, &mut settings).unwrap();
        assert!(import.skipped.is_empty(), "{:?}", import.skipped);
        assert_eq!(settings.flush_volumes, "0,353,191;298,0,210;463,487,0");
        assert_eq!(settings.flush_multiplier, 1.5);
        // With as many extruders the table is used.
        settings.set("extruder_count", "3").unwrap();
        let matrix = settings.flush_matrix().unwrap();
        assert_eq!(matrix[2], [694.5, 730.5, 0.0]);

        // A table that does not fit the extruders is left out.
        let mut two = Settings::default();
        two.set("extruder_count", "2").unwrap();
        let import = import_orca(project, &mut two).unwrap();
        assert_eq!(import.skipped.len(), 1);
        assert!(
            import.skipped[0].1.contains("extruder_count"),
            "{:?}",
            import.skipped
        );
        assert_eq!(two.flush_volumes, "");
        let odd = r#"{ "flush_volumes_matrix": ["0", "1", "2"] }"#;
        let import = import_orca(odd, &mut two).unwrap();
        assert_eq!(import.skipped[0].1, "not a square table of numbers");
    }

    #[test]
    fn filament_colours_and_types_come_from_projects() {
        // As Bambu Studio stores them, one per filament, some with opacity.
        let project = r##"{
            "filament_colour": ["#000000", "#0000FF", "#FF0000FE", "#161616"],
            "filament_type": ["PLA", "PETG", "PLA", "PETG"],
            "default_filament_colour": ["#000000", "", "", ""]
        }"##;
        let mut settings = Settings::default();
        let import = import_orca(project, &mut settings).unwrap();
        assert!(import.skipped.is_empty(), "{:?}", import.skipped);
        assert_eq!(settings.filament_colours, "#000000,#0000FF,#FF0000,#161616");
        assert_eq!(settings.filament_type, "PLA,PETG,PLA,PETG");
        assert_eq!(settings.filament_colours().unwrap()[2], [0xFF, 0, 0]);
        // With as many extruders they are used.
        settings.set("extruder_count", "4").unwrap();

        // Fewer extruders than filaments: left out, as the flush table.
        let mut two = Settings::default();
        two.set("extruder_count", "2").unwrap();
        let import = import_orca(project, &mut two).unwrap();
        assert_eq!(import.skipped.len(), 2, "{:?}", import.skipped);
        assert_eq!(two.filament_colours, "");
        // A filament profile names one type and is taken as well.
        let filament = r#"{ "filament_type": ["PETG"], "filament_colour": ["oops"] }"#;
        let import = import_orca(filament, &mut two).unwrap();
        assert_eq!(two.filament_type, "PETG");
        assert_eq!(import.skipped.len(), 1);
        assert_eq!(import.skipped[0].0, "filament_colour");
    }

    #[test]
    fn broken_files_give_errors() {
        let mut settings = Settings::default();
        assert!(import_orca("not json", &mut settings).is_err());
        assert!(import_orca("[1, 2]", &mut settings).is_err());
        assert_eq!(settings, Settings::default());
    }
}
