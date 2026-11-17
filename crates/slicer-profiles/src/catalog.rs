//! Printers, materials and processes built into the program, for the setup
//! assistant and for pickers that have no profile folder (the browser).
//!
//! The printer data are build volumes, nozzle sizes, firmware, extruder
//! type and motion system as the makers publish them. Speeds, retraction
//! and temperatures are general starting values chosen for this project,
//! one set per kind of machine and material; nothing was taken from the
//! profiles of other slicers. None of these printers has run a print from
//! this program yet.

use crate::{Flavor, Settings};

/// How the filament reaches the nozzle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drive {
    /// Extruder on the print head: short retraction.
    Direct,
    /// Extruder on the frame, pushing through a tube: long retraction.
    Bowden,
}

/// How fast the machine can move, which sets the starting speeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    /// A classic moving-bed printer without vibration compensation.
    Classic,
    /// A moving-bed printer with input shaping.
    Fast,
    /// A CoreXY or similar machine with a fixed bed in X and Y.
    CoreXy,
}

/// A printer model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Printer {
    /// Maker, as shown in the assistant.
    pub vendor: &'static str,
    /// Model name without the maker.
    pub model: &'static str,
    /// Usable bed in X and Y, millimetres.
    pub bed: [f64; 2],
    /// Usable height, millimetres.
    pub height: f64,
    /// Nozzle sizes offered by the maker, millimetres; the first is the
    /// one fitted at delivery.
    pub nozzles: &'static [f64],
    /// Firmware.
    pub firmware: Flavor,
    /// Extruder type.
    pub drive: Drive,
    /// Motion system.
    pub motion: Motion,
    /// Whether the printer probes the bed with `G29` before printing.
    pub probe: bool,
}

use Drive::{Bowden, Direct};
use Flavor::{Klipper, Marlin};
use Motion::{Classic, CoreXy, Fast};

const N4: &[f64] = &[0.4];
const N4_6: &[f64] = &[0.4, 0.6];
const N4_6_8: &[f64] = &[0.4, 0.6, 0.8];
const N2_4_6_8: &[f64] = &[0.4, 0.2, 0.6, 0.8];
const N25_4_6_8: &[f64] = &[0.4, 0.25, 0.6, 0.8];

#[allow(clippy::too_many_arguments)]
const fn p(
    vendor: &'static str,
    model: &'static str,
    bed: [f64; 2],
    height: f64,
    nozzles: &'static [f64],
    firmware: Flavor,
    drive: Drive,
    motion: Motion,
    probe: bool,
) -> Printer {
    Printer {
        vendor,
        model,
        bed,
        height,
        nozzles,
        firmware,
        drive,
        motion,
        probe,
    }
}

/// Every printer the assistant offers, grouped by maker.
#[rustfmt::skip]
pub const PRINTERS: &[Printer] = &[
    p("Bambu Lab", "X1 Carbon", [256.0, 256.0], 256.0, N2_4_6_8, Marlin, Direct, CoreXy, true),
    p("Bambu Lab", "X1E", [256.0, 256.0], 256.0, N2_4_6_8, Marlin, Direct, CoreXy, true),
    p("Bambu Lab", "P1S", [256.0, 256.0], 256.0, N2_4_6_8, Marlin, Direct, CoreXy, true),
    p("Bambu Lab", "P1P", [256.0, 256.0], 256.0, N2_4_6_8, Marlin, Direct, CoreXy, true),
    p("Bambu Lab", "A1", [256.0, 256.0], 256.0, N2_4_6_8, Marlin, Direct, Fast, true),
    p("Bambu Lab", "A1 mini", [180.0, 180.0], 180.0, N2_4_6_8, Marlin, Direct, Fast, true),
    p("Bambu Lab", "H2D", [325.0, 320.0], 325.0, N2_4_6_8, Marlin, Direct, CoreXy, true),
    p("Prusa", "Core One", [250.0, 220.0], 270.0, N25_4_6_8, Marlin, Direct, CoreXy, true),
    p("Prusa", "MK4S", [250.0, 210.0], 220.0, N25_4_6_8, Marlin, Direct, Fast, true),
    p("Prusa", "MK4", [250.0, 210.0], 220.0, N25_4_6_8, Marlin, Direct, Fast, true),
    p("Prusa", "MK3S+", [250.0, 210.0], 210.0, N25_4_6_8, Marlin, Direct, Classic, true),
    p("Prusa", "MINI+", [180.0, 180.0], 180.0, N25_4_6_8, Marlin, Bowden, Classic, true),
    p("Prusa", "XL", [360.0, 360.0], 360.0, N25_4_6_8, Marlin, Direct, CoreXy, true),
    p("Creality", "K1", [220.0, 220.0], 250.0, N4_6_8, Klipper, Direct, CoreXy, false),
    p("Creality", "K1C", [220.0, 220.0], 250.0, N4_6_8, Klipper, Direct, CoreXy, false),
    p("Creality", "K1 Max", [300.0, 300.0], 300.0, N4_6_8, Klipper, Direct, CoreXy, false),
    p("Creality", "K2 Plus", [350.0, 350.0], 350.0, N4_6_8, Klipper, Direct, CoreXy, false),
    p("Creality", "Ender-3 V3", [220.0, 220.0], 250.0, N4_6_8, Klipper, Direct, Fast, false),
    p("Creality", "Ender-3 V3 KE", [220.0, 220.0], 240.0, N4_6_8, Klipper, Direct, Fast, false),
    p("Creality", "Ender-3 V3 SE", [220.0, 220.0], 250.0, N4, Marlin, Direct, Classic, true),
    p("Creality", "Ender-3 S1", [220.0, 220.0], 270.0, N4, Marlin, Direct, Classic, true),
    p("Creality", "Ender-3 V2", [220.0, 220.0], 250.0, N4, Marlin, Bowden, Classic, false),
    p("Creality", "Ender-3", [220.0, 220.0], 250.0, N4, Marlin, Bowden, Classic, false),
    p("Creality", "Ender-5 S1", [220.0, 220.0], 280.0, N4, Marlin, Direct, Classic, true),
    p("Creality", "CR-10", [300.0, 300.0], 400.0, N4, Marlin, Bowden, Classic, false),
    p("Anycubic", "Kobra S1", [250.0, 250.0], 250.0, N4_6_8, Klipper, Direct, CoreXy, false),
    p("Anycubic", "Kobra 3", [250.0, 250.0], 260.0, N4_6_8, Klipper, Direct, Fast, false),
    p("Anycubic", "Kobra 2", [220.0, 220.0], 250.0, N4, Marlin, Direct, Fast, true),
    p("Anycubic", "Kobra 2 Neo", [220.0, 220.0], 250.0, N4, Marlin, Direct, Classic, true),
    p("Anycubic", "Kobra 2 Pro", [220.0, 220.0], 250.0, N4, Marlin, Direct, Fast, true),
    p("Anycubic", "Kobra 2 Plus", [320.0, 320.0], 400.0, N4, Marlin, Direct, Fast, true),
    p("Anycubic", "Kobra 2 Max", [420.0, 420.0], 500.0, N4, Marlin, Direct, Fast, true),
    p("Elegoo", "Centauri Carbon", [256.0, 256.0], 256.0, N4, Klipper, Direct, CoreXy, false),
    p("Elegoo", "Neptune 4", [225.0, 225.0], 265.0, N4, Klipper, Direct, Fast, false),
    p("Elegoo", "Neptune 4 Pro", [225.0, 225.0], 265.0, N4, Klipper, Direct, Fast, false),
    p("Elegoo", "Neptune 4 Plus", [320.0, 320.0], 385.0, N4, Klipper, Direct, Fast, false),
    p("Elegoo", "Neptune 4 Max", [420.0, 420.0], 480.0, N4, Klipper, Direct, Fast, false),
    p("Elegoo", "Neptune 3 Pro", [225.0, 225.0], 280.0, N4, Marlin, Direct, Classic, true),
    p("Voron", "2.4 250", [250.0, 250.0], 250.0, N4_6_8, Klipper, Direct, CoreXy, false),
    p("Voron", "2.4 300", [300.0, 300.0], 300.0, N4_6_8, Klipper, Direct, CoreXy, false),
    p("Voron", "2.4 350", [350.0, 350.0], 350.0, N4_6_8, Klipper, Direct, CoreXy, false),
    p("Voron", "Trident 250", [250.0, 250.0], 250.0, N4_6_8, Klipper, Direct, CoreXy, false),
    p("Voron", "Trident 300", [300.0, 300.0], 250.0, N4_6_8, Klipper, Direct, CoreXy, false),
    p("Voron", "Trident 350", [350.0, 350.0], 250.0, N4_6_8, Klipper, Direct, CoreXy, false),
    p("Voron", "0.2", [120.0, 120.0], 120.0, N4_6, Klipper, Direct, CoreXy, false),
    p("Qidi", "Plus4", [305.0, 305.0], 280.0, N4_6_8, Klipper, Direct, CoreXy, false),
    p("Qidi", "Q1 Pro", [245.0, 245.0], 240.0, N4_6_8, Klipper, Direct, CoreXy, false),
    p("Qidi", "X-Plus 3", [280.0, 280.0], 270.0, N4_6_8, Klipper, Direct, CoreXy, false),
    p("Qidi", "X-Max 3", [325.0, 325.0], 315.0, N4_6_8, Klipper, Direct, CoreXy, false),
    p("Qidi", "X-Smart 3", [175.0, 180.0], 170.0, N4_6_8, Klipper, Direct, CoreXy, false),
    p("Sovol", "SV08", [350.0, 350.0], 345.0, N4_6_8, Klipper, Direct, CoreXy, false),
    p("Sovol", "SV07", [220.0, 220.0], 250.0, N4, Klipper, Direct, Fast, false),
    p("Sovol", "SV06 Plus", [300.0, 300.0], 340.0, N4, Marlin, Direct, Classic, true),
    p("Sovol", "SV06", [220.0, 220.0], 250.0, N4, Marlin, Direct, Classic, true),
    p("Flashforge", "Adventurer 5M", [220.0, 220.0], 220.0, N4_6_8, Klipper, Direct, CoreXy, false),
    p("Flashforge", "Adventurer 5M Pro", [220.0, 220.0], 220.0, N4_6_8, Klipper, Direct, CoreXy, false),
    p("Artillery", "Sidewinder X4 Plus", [300.0, 300.0], 400.0, N4, Klipper, Direct, Fast, false),
    p("Artillery", "Sidewinder X2", [300.0, 300.0], 400.0, N4, Marlin, Direct, Classic, true),
    p("Artillery", "Genius Pro", [220.0, 220.0], 250.0, N4, Marlin, Direct, Classic, true),
    p("RatRig", "V-Core 3.1 300", [300.0, 300.0], 300.0, N4_6_8, Klipper, Direct, CoreXy, false),
    p("RatRig", "V-Core 3.1 400", [400.0, 400.0], 400.0, N4_6_8, Klipper, Direct, CoreXy, false),
    p("RatRig", "V-Core 3.1 500", [500.0, 500.0], 500.0, N4_6_8, Klipper, Direct, CoreXy, false),
    p("Generic", "Marlin 220 × 220", [220.0, 220.0], 250.0, N4_6_8, Marlin, Direct, Classic, false),
    p("Generic", "Marlin 300 × 300", [300.0, 300.0], 300.0, N4_6_8, Marlin, Direct, Classic, false),
    p("Generic", "Klipper 250 × 250", [250.0, 250.0], 250.0, N4_6_8, Klipper, Direct, CoreXy, false),
    p("Generic", "Klipper 350 × 350", [350.0, 350.0], 350.0, N4_6_8, Klipper, Direct, CoreXy, false),
];

/// Settings keys a printer profile sets.
pub const PRINTER_KEYS: &[&str] = &[
    "technology",
    "bed_size_x",
    "bed_size_y",
    "max_print_height",
    "nozzle_diameter",
    "line_width",
    "min_layer_height",
    "max_layer_height",
    "flavor",
    "outer_wall_speed",
    "inner_wall_speed",
    "infill_speed",
    "first_layer_speed",
    "travel_speed",
    "acceleration",
    "jerk",
    "retraction_length",
    "retraction_speed",
    "z_hop",
    "start_gcode",
    "end_gcode",
];

/// Settings keys a material profile sets.
pub const MATERIAL_KEYS: &[&str] = &[
    "filament_diameter",
    "filament_density",
    "nozzle_temp",
    "nozzle_temp_first_layer",
    "bed_temp",
    "bed_temp_first_layer",
    "fan_speed",
    "fan_off_layers",
    "bridge_fan_speed",
];

/// Settings keys a process profile sets.
pub const PROCESS_KEYS: &[&str] = &[
    "layer_height",
    "first_layer_height",
    "wall_loops",
    "top_layers",
    "bottom_layers",
    "infill_density",
];

/// `x` rounded to hundredths, so profiles show 0.45 and not 0.45000000000000007.
fn hundredths(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Lower case with dashes instead of spaces and other signs.
fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || c == '.' {
            out.push(c.to_ascii_lowercase());
        } else if c == '+' {
            if !out.ends_with('-') {
                out.push('-');
            }
            out.push_str("plus");
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}

impl Printer {
    /// Maker and model.
    pub fn full_name(&self) -> String {
        format!("{} {}", self.vendor, self.model)
    }

    /// The name of the profile for one nozzle, as the pickers show it.
    pub fn name(&self, nozzle: f64) -> String {
        format!("{} {nozzle} nozzle", self.full_name())
    }

    /// A stable identifier of the profile for one nozzle, for example
    /// `bambu-lab-x1-carbon-0.4`.
    pub fn id(&self, nozzle: f64) -> String {
        format!("{}-{nozzle}", slug(&self.full_name()))
    }

    /// The settings of this printer with `nozzle`, on top of the defaults.
    pub fn settings(&self, nozzle: f64) -> Settings {
        let (outer, inner, infill, first, travel, acceleration) = match self.motion {
            Motion::Classic => (40.0, 60.0, 80.0, 25.0, 150.0, 1000.0),
            Motion::Fast => (80.0, 120.0, 150.0, 30.0, 250.0, 3000.0),
            Motion::CoreXy => (100.0, 150.0, 200.0, 35.0, 300.0, 5000.0),
        };
        let (retraction_length, retraction_speed) = match self.drive {
            Drive::Direct => (0.8, 35.0),
            Drive::Bowden => (5.0, 40.0),
        };
        let mut start = String::from(
            "M140 S{bed_temp_first_layer}\nM104 S150\nG28\nM190 S{bed_temp_first_layer}\n",
        );
        if self.probe {
            start.push_str("G29\n");
        }
        start.push_str("M109 S{nozzle_temp_first_layer}");
        Settings {
            bed_size_x: self.bed[0],
            bed_size_y: self.bed[1],
            max_print_height: self.height,
            nozzle_diameter: nozzle,
            line_width: hundredths(nozzle * 1.125),
            min_layer_height: hundredths(nozzle * 0.2),
            max_layer_height: hundredths(nozzle * 0.75),
            flavor: self.firmware,
            outer_wall_speed: outer,
            inner_wall_speed: inner,
            infill_speed: infill,
            first_layer_speed: first,
            travel_speed: travel,
            acceleration,
            jerk: 9.0,
            retraction_length,
            retraction_speed,
            z_hop: if self.motion == Motion::CoreXy {
                0.4
            } else {
                0.2
            },
            start_gcode: start,
            ..Settings::default()
        }
    }

    /// The printer profile for one nozzle as a settings file.
    pub fn toml(&self, nozzle: f64) -> String {
        format!(
            "# {}: {} x {} x {} mm. Built-in profile, not yet run on this printer.\n{}",
            self.name(nozzle),
            self.bed[0],
            self.bed[1],
            self.height,
            self.settings(nozzle).to_toml_keys(PRINTER_KEYS)
        )
    }
}

/// The printer and nozzle of a profile identifier made by [`Printer::id`].
pub fn printer_by_id(id: &str) -> Option<(&'static Printer, f64)> {
    PRINTERS.iter().find_map(|printer| {
        printer
            .nozzles
            .iter()
            .find(|&&nozzle| printer.id(nozzle) == id)
            .map(|&nozzle| (printer, nozzle))
    })
}

/// The makers in the order of [`PRINTERS`].
pub fn vendors() -> Vec<&'static str> {
    let mut vendors: Vec<&'static str> = Vec::new();
    for printer in PRINTERS {
        if !vendors.contains(&printer.vendor) {
            vendors.push(printer.vendor);
        }
    }
    vendors
}

/// A filament type with general starting values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Material {
    /// Name as shown, for example `PETG`.
    pub name: &'static str,
    /// Density in g/cm³.
    pub density: f64,
    /// Nozzle temperature in °C, then for the first layer.
    pub nozzle: [f64; 2],
    /// Bed temperature in °C, for all layers.
    pub bed: f64,
    /// Part fan in percent.
    pub fan: f64,
    /// Layers printed without the fan.
    pub fan_off_layers: u32,
}

const fn m(
    name: &'static str,
    density: f64,
    nozzle: [f64; 2],
    bed: f64,
    fan: f64,
    fan_off_layers: u32,
) -> Material {
    Material {
        name,
        density,
        nozzle,
        bed,
        fan,
        fan_off_layers,
    }
}

/// Every material the assistant offers.
#[rustfmt::skip]
pub const MATERIALS: &[Material] = &[
    m("PLA", 1.24, [210.0, 215.0], 60.0, 100.0, 1),
    m("PLA-CF", 1.30, [220.0, 225.0], 60.0, 100.0, 1),
    m("PETG", 1.27, [240.0, 245.0], 80.0, 50.0, 3),
    m("PETG-CF", 1.30, [245.0, 250.0], 80.0, 40.0, 3),
    m("ABS", 1.04, [250.0, 255.0], 100.0, 20.0, 3),
    m("ASA", 1.07, [255.0, 260.0], 100.0, 20.0, 3),
    m("TPU 95A", 1.21, [225.0, 230.0], 40.0, 60.0, 2),
    m("PA", 1.14, [265.0, 270.0], 90.0, 0.0, 3),
    m("PC", 1.20, [270.0, 275.0], 110.0, 0.0, 3),
    m("HIPS", 1.04, [235.0, 240.0], 100.0, 20.0, 3),
    m("PVA", 1.23, [200.0, 205.0], 60.0, 100.0, 1),
];

impl Material {
    /// The name shown in the pickers, for example `Generic PETG`.
    pub fn display_name(&self) -> String {
        format!("Generic {}", self.name)
    }

    /// A stable identifier, for example `generic-petg`.
    pub fn id(&self) -> String {
        slug(&self.display_name())
    }

    /// The settings of this material, on top of the defaults.
    pub fn settings(&self) -> Settings {
        Settings {
            filament_diameter: 1.75,
            filament_density: self.density,
            nozzle_temp: self.nozzle[0],
            nozzle_temp_first_layer: self.nozzle[1],
            bed_temp: self.bed,
            bed_temp_first_layer: self.bed,
            fan_speed: self.fan,
            fan_off_layers: self.fan_off_layers,
            bridge_fan_speed: self.fan.max(50.0),
            ..Settings::default()
        }
    }

    /// The material profile as a settings file.
    pub fn toml(&self) -> String {
        format!(
            "# {}, 1.75 mm. General starting values; check the maker's data sheet.\n{}",
            self.display_name(),
            self.settings().to_toml_keys(MATERIAL_KEYS)
        )
    }
}

/// The material of an identifier made by [`Material::id`].
pub fn material_by_id(id: &str) -> Option<&'static Material> {
    MATERIALS.iter().find(|material| material.id() == id)
}

/// Layer heights of the processes, as fractions of the nozzle, with their
/// names.
const PROCESS_STEPS: [(f64, &str); 6] = [
    (0.2, "Extra fine"),
    (0.3, "Fine"),
    (0.4, "Optimal"),
    (0.5, "Standard"),
    (0.6, "Draft"),
    (0.7, "Extra draft"),
];

/// A process: layer height and the layer counts that go with it.
#[derive(Debug, Clone, PartialEq)]
pub struct Process {
    /// Layer height in millimetres.
    pub layer_height: f64,
    /// Name of the step, for example `Standard`.
    pub label: &'static str,
}

impl Process {
    /// The name shown in the pickers, for example `0.20mm Standard`.
    pub fn name(&self) -> String {
        format!("{:.2}mm {}", self.layer_height, self.label)
    }

    /// A stable identifier, for example `0.20mm-standard`.
    pub fn id(&self) -> String {
        slug(&self.name())
    }

    /// The settings of this process, on top of the defaults. The first
    /// layer is half as thick as the nozzle is wide, or the layer height
    /// if that is thicker; top and bottom are at least 0.8 and 0.6 mm thick.
    pub fn settings(&self, nozzle: f64) -> Settings {
        let h = self.layer_height;
        let layers = |thickness: f64| ((thickness / h - 1e-9).ceil() as u32).max(3);
        Settings {
            nozzle_diameter: nozzle,
            layer_height: h,
            first_layer_height: hundredths((nozzle / 2.0).max(h)),
            wall_loops: 2,
            top_layers: layers(0.8),
            bottom_layers: layers(0.6),
            infill_density: 15.0,
            ..Settings::default()
        }
    }

    /// The process as a settings file for `nozzle`.
    pub fn toml(&self, nozzle: f64) -> String {
        format!(
            "# {}: for a {nozzle} mm nozzle.\n{}",
            self.name(),
            self.settings(nozzle).to_toml_keys(PROCESS_KEYS)
        )
    }
}

/// The processes for a nozzle, from fine to coarse.
pub fn processes(nozzle: f64) -> Vec<Process> {
    PROCESS_STEPS
        .iter()
        .map(|&(share, label)| Process {
            layer_height: hundredths(nozzle * share),
            label,
        })
        .collect()
}

/// The process of an identifier made by [`Process::id`], with the nozzle
/// it was meant for: the smallest one whose processes contain it.
pub fn process_by_id(id: &str) -> Option<(Process, f64)> {
    [0.2, 0.25, 0.4, 0.6, 0.8].into_iter().find_map(|nozzle| {
        processes(nozzle)
            .into_iter()
            .find(|process| process.id() == id)
            .map(|process| (process, nozzle))
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn every_profile_is_valid_and_found_again() {
        let mut ids = Vec::new();
        for printer in PRINTERS {
            for &nozzle in printer.nozzles {
                let id = printer.id(nozzle);
                assert!(!ids.contains(&id), "{id} twice");
                assert_eq!(printer_by_id(&id), Some((printer, nozzle)));
                let mut settings = Settings::from_toml(&printer.toml(nozzle)).unwrap();
                assert_eq!(settings.nozzle_diameter, nozzle);
                for material in MATERIALS {
                    settings.apply_toml(&material.toml()).unwrap();
                    for process in processes(nozzle) {
                        let mut all = settings.clone();
                        all.apply_toml(&process.toml(nozzle)).unwrap();
                        assert_eq!(all.layer_height, process.layer_height);
                    }
                }
                ids.push(id);
            }
        }
        assert_eq!(
            printer_by_id("bambu-lab-x1-carbon-0.4").unwrap().0.bed,
            [256.0, 256.0]
        );
        assert_eq!(PRINTERS[11].id(0.4), "prusa-mini-plus-0.4");
        assert!(printer_by_id("bambu-lab-x1-carbon-0.5").is_none());
        for material in MATERIALS {
            assert_eq!(material_by_id(&material.id()), Some(material));
        }
        assert_eq!(MATERIALS[0].id(), "generic-pla");
        let standard = &processes(0.4)[3];
        assert_eq!(standard.name(), "0.20mm Standard");
        assert_eq!(
            process_by_id("0.20mm-standard"),
            Some((standard.clone(), 0.4))
        );
        let fine = processes(0.4)[1].settings(0.4);
        assert_eq!(
            (fine.layer_height, fine.top_layers, fine.bottom_layers),
            (0.12, 7, 5)
        );
        assert!(vendors().starts_with(&["Bambu Lab", "Prusa", "Creality"]));
    }

    #[test]
    fn probing_printers_level_the_bed() {
        let x1c = printer_by_id("bambu-lab-x1-carbon-0.4").unwrap().0;
        assert!(x1c.settings(0.4).start_gcode.contains("G29"));
        let k1 = printer_by_id("creality-k1-0.4").unwrap().0;
        assert!(!k1.settings(0.4).start_gcode.contains("G29"));
        assert_eq!(k1.settings(0.4).flavor, Flavor::Klipper);
    }
}
