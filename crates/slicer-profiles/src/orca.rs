//! Import of profiles exported or saved by Orca Slicer (JSON).
//!
//! The key names and value formats were taken from profiles a user saved
//! in Orca Slicer, read as data files; no source code or shipped profile of
//! Orca Slicer was used. Only settings with a clear counterpart here are
//! taken, everything else is reported.

use serde_json::Value;

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
        ]),
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
    ("enable_support", "supports", Kind::Flag),
    ("outer_wall_speed", "outer_wall_speed", Kind::Number),
    ("inner_wall_speed", "inner_wall_speed", Kind::Number),
    ("sparse_infill_speed", "infill_speed", Kind::Number),
    ("bridge_speed", "bridge_speed", Kind::Number),
    ("initial_layer_speed", "first_layer_speed", Kind::Number),
    ("travel_speed", "travel_speed", Kind::Number),
    ("default_acceleration", "acceleration", Kind::Number),
    ("default_jerk", "jerk", Kind::Number),
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

/// Reads one Orca profile (printer, material or process) and checks every
/// taken value against `settings`, which receives them. Values that our
/// settings reject are reported as skipped.
pub fn import_orca(json: &str, settings: &mut Settings) -> Result<OrcaImport, SettingsError> {
    let profile: Value =
        serde_json::from_str(json).map_err(|e| SettingsError::Parse(e.to_string()))?;
    let Some(entries) = profile.as_object() else {
        return Err(SettingsError::Parse(
            "an Orca profile is a JSON object".into(),
        ));
    };
    let mut import = OrcaImport {
        name: profile.get("name").and_then(text),
        inherits: profile.get("inherits").and_then(text),
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
        } else {
            let Some(value) = text(value) else { continue };
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
    Ok(import)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::{InfillPattern, SeamPosition};

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
        let other = r#"{ "sparse_infill_pattern": "honeycomb" }"#;
        let other = import_orca(other, &mut settings).unwrap();
        assert_eq!(skipped(&other), ["sparse_infill_pattern"]);
        assert_eq!(settings.infill_pattern, InfillPattern::Gyroid);
    }

    #[test]
    fn broken_files_give_errors() {
        let mut settings = Settings::default();
        assert!(import_orca("not json", &mut settings).is_err());
        assert!(import_orca("[1, 2]", &mut settings).is_err());
        assert_eq!(settings, Settings::default());
    }
}
