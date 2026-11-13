//! Printer, material and process profiles with inheritance and validation.
//!
//! For now this is a single flat [`Settings`] struct loaded from TOML. The
//! split into printer, material and process profiles with inheritance comes
//! later.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// G-code dialect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Flavor {
    /// Marlin firmware.
    Marlin,
    /// Klipper firmware.
    Klipper,
}

/// Pattern of sparse infill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InfillPattern {
    /// Parallel lines, direction alternating by 90° every layer.
    Rectilinear,
    /// Lines in both directions on every layer.
    Grid,
}

/// All settings that influence slicing. Lengths in mm, speeds in mm/s,
/// temperatures in °C.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    // Printer
    /// Printable bed width (X).
    pub bed_size_x: f64,
    /// Printable bed depth (Y).
    pub bed_size_y: f64,
    /// Maximum print height (Z).
    pub max_print_height: f64,
    /// Nozzle diameter.
    pub nozzle_diameter: f64,
    /// G-code dialect.
    pub flavor: Flavor,

    // Material
    /// Filament diameter.
    pub filament_diameter: f64,
    /// Filament density in g/cm³, used for the weight estimate.
    pub filament_density: f64,
    /// Multiplier on the extruded amount.
    pub flow_ratio: f64,
    /// Nozzle temperature after the first layer.
    pub nozzle_temp: f64,
    /// Nozzle temperature on the first layer.
    pub nozzle_temp_first_layer: f64,
    /// Bed temperature after the first layer.
    pub bed_temp: f64,
    /// Bed temperature on the first layer.
    pub bed_temp_first_layer: f64,
    /// Part cooling fan speed in percent.
    pub fan_speed: f64,
    /// Number of initial layers printed with the fan off.
    pub fan_off_layers: u32,

    // Process
    /// Layer height.
    pub layer_height: f64,
    /// Height of the first layer.
    pub first_layer_height: f64,
    /// Width of one extruded line.
    pub line_width: f64,
    /// Number of wall loops.
    pub wall_loops: u32,
    /// Number of solid layers at the top.
    pub top_layers: u32,
    /// Number of solid layers at the bottom.
    pub bottom_layers: u32,
    /// Sparse infill density in percent.
    pub infill_density: f64,
    /// Sparse infill pattern.
    pub infill_pattern: InfillPattern,
    /// How far infill reaches into the innermost wall, in percent of the line width.
    pub infill_overlap: f64,
    /// Number of skirt loops around the first layer.
    pub skirt_loops: u32,
    /// Distance between skirt and model.
    pub skirt_distance: f64,
    /// Width of the brim attached to the first layer; 0 disables it.
    pub brim_width: f64,

    // Speeds
    /// Speed of the outermost wall.
    pub outer_wall_speed: f64,
    /// Speed of inner walls.
    pub inner_wall_speed: f64,
    /// Speed of solid and sparse infill.
    pub infill_speed: f64,
    /// Speed of all extrusion on the first layer.
    pub first_layer_speed: f64,
    /// Speed of moves without extrusion.
    pub travel_speed: f64,
    /// Acceleration of the printer in mm/s². Only used to estimate the
    /// print time; it is not written to the G-code.
    pub acceleration: f64,
    /// Largest speed change in mm/s the printer makes at once, for example
    /// at a corner. Only used to estimate the print time.
    pub jerk: f64,

    // Retraction
    /// Filament length pulled back before a travel move; 0 disables retraction.
    pub retraction_length: f64,
    /// Speed of retraction and unretraction.
    pub retraction_speed: f64,
    /// Travel moves shorter than this do not retract.
    pub retraction_min_travel: f64,
    /// Nozzle lift during retracted travel moves; 0 disables it.
    pub z_hop: f64,

    // Custom G-code
    /// G-code inserted before the print. `{setting_name}` placeholders are replaced.
    pub start_gcode: String,
    /// G-code inserted after the print. `{setting_name}` placeholders are replaced.
    pub end_gcode: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            bed_size_x: 220.0,
            bed_size_y: 220.0,
            max_print_height: 250.0,
            nozzle_diameter: 0.4,
            flavor: Flavor::Marlin,
            filament_diameter: 1.75,
            filament_density: 1.24,
            flow_ratio: 1.0,
            nozzle_temp: 210.0,
            nozzle_temp_first_layer: 215.0,
            bed_temp: 60.0,
            bed_temp_first_layer: 60.0,
            fan_speed: 100.0,
            fan_off_layers: 1,
            layer_height: 0.2,
            first_layer_height: 0.2,
            line_width: 0.45,
            wall_loops: 2,
            top_layers: 5,
            bottom_layers: 4,
            infill_density: 15.0,
            infill_pattern: InfillPattern::Grid,
            infill_overlap: 25.0,
            skirt_loops: 2,
            skirt_distance: 4.0,
            brim_width: 0.0,
            outer_wall_speed: 40.0,
            inner_wall_speed: 60.0,
            infill_speed: 80.0,
            first_layer_speed: 25.0,
            travel_speed: 150.0,
            acceleration: 1000.0,
            jerk: 8.0,
            retraction_length: 0.8,
            retraction_speed: 35.0,
            retraction_min_travel: 1.5,
            z_hop: 0.0,
            start_gcode: "M140 S{bed_temp_first_layer}\nM104 S{nozzle_temp_first_layer}\nG28\nM190 S{bed_temp_first_layer}\nM109 S{nozzle_temp_first_layer}"
                .to_owned(),
            end_gcode: "M104 S0\nM140 S0\nM107\nG91\nG1 Z10 F600\nG90\nM84".to_owned(),
        }
    }
}

/// A problem with settings.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingsError {
    /// The TOML text could not be parsed or contains unknown keys or wrong types.
    Parse(String),
    /// A key that does not exist.
    UnknownKey(String),
    /// A value outside its allowed range.
    OutOfRange {
        /// Name of the setting.
        key: &'static str,
        /// The rejected value.
        value: f64,
        /// Human-readable description of the allowed range.
        allowed: String,
    },
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(message) => write!(f, "invalid settings: {message}"),
            Self::UnknownKey(key) => write!(f, "unknown setting '{key}'"),
            Self::OutOfRange {
                key,
                value,
                allowed,
            } => {
                write!(
                    f,
                    "setting '{key}' = {value} is out of range (allowed: {allowed})"
                )
            }
        }
    }
}

impl std::error::Error for SettingsError {}

impl slicer_core::Diagnose for SettingsError {
    fn code(&self) -> &'static str {
        match self {
            Self::Parse(_) => "settings.parse",
            Self::UnknownKey(_) => "settings.unknown_key",
            Self::OutOfRange { .. } => "settings.out_of_range",
        }
    }

    fn context(&self) -> Vec<(&'static str, String)> {
        match self {
            Self::Parse(_) => Vec::new(),
            Self::UnknownKey(key) => vec![("key", key.clone())],
            Self::OutOfRange {
                key,
                value,
                allowed,
            } => vec![
                ("key", (*key).to_owned()),
                ("value", value.to_string()),
                ("allowed", allowed.clone()),
            ],
        }
    }
}

fn range(key: &'static str, value: f64, min: f64, max: f64) -> Result<(), SettingsError> {
    if value.is_finite() && value >= min && value <= max {
        Ok(())
    } else {
        Err(SettingsError::OutOfRange {
            key,
            value,
            allowed: format!("{min} to {max}"),
        })
    }
}

impl Settings {
    /// Parses settings from TOML. Missing keys take their default value,
    /// unknown keys are rejected.
    pub fn from_toml(text: &str) -> Result<Self, SettingsError> {
        let settings: Settings =
            toml::from_str(text).map_err(|e| SettingsError::Parse(e.message().to_owned()))?;
        settings.validate()?;
        Ok(settings)
    }

    /// Serializes all settings as TOML.
    pub fn to_toml(&self) -> String {
        toml::to_string(self).unwrap_or_default()
    }

    /// Overrides a single setting from its textual value, as given on the
    /// command line.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), SettingsError> {
        let mut table = self.table();
        let Some(current) = table.get(key) else {
            return Err(SettingsError::UnknownKey(key.to_owned()));
        };
        let parsed = if current.is_str() {
            toml::Value::String(value.to_owned())
        } else {
            format!("v = {value}")
                .parse::<toml::Table>()
                .ok()
                .and_then(|mut t| t.remove("v"))
                .ok_or_else(|| {
                    SettingsError::Parse(format!("cannot parse '{value}' for '{key}'"))
                })?
        };
        table.insert(key.to_owned(), parsed);
        let updated: Settings = table.try_into().map_err(|e: toml::de::Error| {
            SettingsError::Parse(format!("{key}: {}", e.message()))
        })?;
        updated.validate()?;
        *self = updated;
        Ok(())
    }

    fn table(&self) -> toml::Table {
        toml::Table::try_from(self).unwrap_or_default()
    }

    /// All settings as text, keyed by name. Used to fill `{name}`
    /// placeholders in custom G-code.
    pub fn placeholder_values(&self) -> BTreeMap<String, String> {
        self.table()
            .into_iter()
            .filter(|(key, _)| key != "start_gcode" && key != "end_gcode")
            .map(|(key, value)| {
                let text = match value {
                    toml::Value::String(s) => s,
                    toml::Value::Float(f) => f.to_string(),
                    other => other.to_string(),
                };
                (key, text)
            })
            .collect()
    }

    /// Checks every value against its allowed range and the settings against
    /// each other.
    pub fn validate(&self) -> Result<(), SettingsError> {
        range("bed_size_x", self.bed_size_x, 10.0, 2000.0)?;
        range("bed_size_y", self.bed_size_y, 10.0, 2000.0)?;
        range("max_print_height", self.max_print_height, 1.0, 2000.0)?;
        range("nozzle_diameter", self.nozzle_diameter, 0.1, 2.0)?;
        range("filament_diameter", self.filament_diameter, 1.0, 3.5)?;
        range("filament_density", self.filament_density, 0.1, 20.0)?;
        range("flow_ratio", self.flow_ratio, 0.5, 2.0)?;
        range("nozzle_temp", self.nozzle_temp, 0.0, 500.0)?;
        range(
            "nozzle_temp_first_layer",
            self.nozzle_temp_first_layer,
            0.0,
            500.0,
        )?;
        range("bed_temp", self.bed_temp, 0.0, 200.0)?;
        range(
            "bed_temp_first_layer",
            self.bed_temp_first_layer,
            0.0,
            200.0,
        )?;
        range("fan_speed", self.fan_speed, 0.0, 100.0)?;
        range(
            "layer_height",
            self.layer_height,
            0.02,
            self.nozzle_diameter,
        )?;
        range(
            "first_layer_height",
            self.first_layer_height,
            0.02,
            self.nozzle_diameter,
        )?;
        let thickest = self.layer_height.max(self.first_layer_height);
        range(
            "line_width",
            self.line_width,
            (0.6 * self.nozzle_diameter).max(1.05 * thickest),
            2.5 * self.nozzle_diameter,
        )?;
        range("wall_loops", f64::from(self.wall_loops), 1.0, 50.0)?;
        range("top_layers", f64::from(self.top_layers), 0.0, 1000.0)?;
        range("bottom_layers", f64::from(self.bottom_layers), 0.0, 1000.0)?;
        range("infill_density", self.infill_density, 0.0, 100.0)?;
        range("infill_overlap", self.infill_overlap, 0.0, 50.0)?;
        range("skirt_loops", f64::from(self.skirt_loops), 0.0, 20.0)?;
        range("skirt_distance", self.skirt_distance, 0.0, 50.0)?;
        range("brim_width", self.brim_width, 0.0, 50.0)?;
        range("outer_wall_speed", self.outer_wall_speed, 1.0, 1000.0)?;
        range("inner_wall_speed", self.inner_wall_speed, 1.0, 1000.0)?;
        range("infill_speed", self.infill_speed, 1.0, 1000.0)?;
        range("first_layer_speed", self.first_layer_speed, 1.0, 1000.0)?;
        range("travel_speed", self.travel_speed, 1.0, 2000.0)?;
        range("acceleration", self.acceleration, 10.0, 100_000.0)?;
        range("jerk", self.jerk, 0.0, 1000.0)?;
        range("retraction_length", self.retraction_length, 0.0, 20.0)?;
        range("retraction_speed", self.retraction_speed, 1.0, 200.0)?;
        range(
            "retraction_min_travel",
            self.retraction_min_travel,
            0.0,
            100.0,
        )?;
        range("z_hop", self.z_hop, 0.0, 10.0)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn defaults_are_valid_and_roundtrip() {
        let defaults = Settings::default();
        defaults.validate().unwrap();
        assert_eq!(Settings::from_toml(&defaults.to_toml()).unwrap(), defaults);
    }

    #[test]
    fn partial_file_keeps_defaults() {
        let s = Settings::from_toml("layer_height = 0.1\nflavor = \"klipper\"\n").unwrap();
        assert_eq!(s.layer_height, 0.1);
        assert_eq!(s.flavor, Flavor::Klipper);
        assert_eq!(s.wall_loops, 2);
    }

    #[test]
    fn unknown_key_is_rejected() {
        let error = Settings::from_toml("layer_hight = 0.1\n").unwrap_err();
        assert!(error.to_string().contains("layer_hight"), "{error}");
    }

    #[test]
    fn out_of_range_names_key_value_and_range() {
        let error = Settings::from_toml("layer_height = 0.9\n").unwrap_err();
        assert_eq!(
            error.to_string(),
            "setting 'layer_height' = 0.9 is out of range (allowed: 0.02 to 0.4)"
        );
    }

    #[test]
    fn set_overrides_single_values() {
        let mut s = Settings::default();
        s.set("infill_density", "100").unwrap();
        s.set("wall_loops", "3").unwrap();
        s.set("infill_pattern", "rectilinear").unwrap();
        s.set("start_gcode", "G28").unwrap();
        assert_eq!(s.infill_density, 100.0);
        assert_eq!(s.wall_loops, 3);
        assert_eq!(s.infill_pattern, InfillPattern::Rectilinear);
        assert_eq!(s.start_gcode, "G28");
        assert_eq!(
            s.set("nope", "1"),
            Err(SettingsError::UnknownKey("nope".into()))
        );
        assert!(s.set("wall_loops", "many").is_err());
        assert!(s.set("infill_density", "150").is_err());
        assert_eq!(s.infill_density, 100.0);
    }

    #[test]
    fn placeholders_use_plain_numbers() {
        let values = Settings::default().placeholder_values();
        assert_eq!(values["nozzle_temp"], "210");
        assert_eq!(values["layer_height"], "0.2");
        assert_eq!(values["flavor"], "marlin");
        assert!(!values.contains_key("start_gcode"));
    }
}
