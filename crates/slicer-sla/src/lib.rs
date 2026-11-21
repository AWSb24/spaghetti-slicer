//! Resin (MSLA) pipeline: cuts a mesh into layers, renders every layer as a
//! grey-scale image for the printer's screen and packs the images with a
//! JSON manifest into a ZIP archive (described in `docs/sla-format.md`).
//!
//! All lengths are millimetres, times seconds.

// ponytail: the model is printed in the orientation it was loaded in (no
// auto-orientation). Vendor formats (.pwmx and similar) are not written,
// only the open archive.

mod hollow;
pub mod raster;
mod supports;

use std::fmt;
use std::io::{Cursor, Write};

use serde::{Deserialize, Serialize};
use serde_json::json;
use slicer_core::slice::slice_mesh_reporting;
use slicer_core::{Diagnostic, Diagnostics, Mesh, Shapes, parallel_map};

/// Settings of a resin print. The defaults describe the Anycubic Photon
/// Mono X: 3840 × 2400 pixels on a 192 × 120 mm screen, 245 mm build
/// height (manufacturer's product page). Exposure and lift values are
/// typical starting points for standard resin, not verified on the device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlaSettings {
    /// Screen width in pixels.
    pub resolution_x: u32,
    /// Screen height in pixels.
    pub resolution_y: u32,
    /// Width of the lit area of the screen in mm.
    pub display_width: f64,
    /// Height of the lit area of the screen in mm.
    pub display_height: f64,
    /// Highest printable model in mm.
    pub max_print_height: f64,
    /// Thickness of every layer in mm.
    pub layer_height: f64,
    /// Exposure of a normal layer in seconds.
    pub exposure_time: f64,
    /// Exposure of a bottom layer in seconds.
    pub bottom_exposure_time: f64,
    /// Number of layers at the bottom exposed for `bottom_exposure_time`.
    pub bottom_layers: u32,
    /// How far the build plate rises after each layer, in mm.
    pub lift_height: f64,
    /// Speed of the build plate going up, in mm/s.
    pub lift_speed: f64,
    /// Speed of the build plate going back down, in mm/s.
    pub retract_speed: f64,
    /// Samples per pixel along each axis for anti-aliasing; 1 turns it off.
    pub antialiasing: u32,
    /// Gap in mm between the top of the raft (or the build plate) and the
    /// lowest point of the model; supports bridge it. 0 puts the model down.
    pub elevation: f64,
    /// Thickness in mm of the raft printed under the model and its
    /// supports; 0 prints no raft.
    pub raft_thickness: f64,
    /// Whether to print supports from the raft up to overhangs.
    pub supports: bool,
    /// Downward-facing surfaces tilted further than this from the vertical
    /// (degrees) get supports; 0 supports every downward-facing surface.
    pub support_overhang_angle: f64,
    /// Distance in mm between the support points on an overhang.
    pub support_spacing: f64,
    /// Diameter in mm where a support touches the model.
    pub support_tip_diameter: f64,
    /// Length in mm of the cone between the tip and the pillar.
    pub support_tip_length: f64,
    /// Diameter of a support pillar in mm.
    pub support_pillar_diameter: f64,
    /// Neighbouring pillars whose straight part is at least this tall (mm)
    /// get diagonal cross braces; 0 never braces.
    pub support_brace_height: f64,
    /// Wall thickness in mm of a hollowed model; 0 prints it solid.
    pub hollow_wall: f64,
    /// Diameter in mm of the drain holes at the lowest points of every
    /// cavity; 0 leaves cavities closed.
    pub drain_hole_diameter: f64,
}

impl Default for SlaSettings {
    fn default() -> Self {
        Self {
            resolution_x: 3840,
            resolution_y: 2400,
            display_width: 192.0,
            display_height: 120.0,
            max_print_height: 245.0,
            layer_height: 0.05,
            exposure_time: 2.0,
            bottom_exposure_time: 40.0,
            bottom_layers: 6,
            lift_height: 6.0,
            lift_speed: 2.0,
            retract_speed: 3.0,
            antialiasing: 4,
            elevation: 0.0,
            raft_thickness: 0.0,
            supports: false,
            support_overhang_angle: 45.0,
            support_spacing: 3.0,
            support_tip_diameter: 0.4,
            support_tip_length: 2.0,
            support_pillar_diameter: 1.2,
            support_brace_height: 10.0,
            hollow_wall: 0.0,
            drain_hole_diameter: 2.0,
        }
    }
}

/// Triangles whose corners lie this close to a line (mm) count as having
/// no area and are removed before slicing, as for filament printers.
const DEGENERATE_HEIGHT: f64 = 1e-6;

/// Allowed range of every setting and whether it must be a whole number.
/// `supports` is a switch: 0 or 1.
const LIMITS: [(&str, f64, f64, bool); 24] = [
    ("resolution_x", 1.0, 20000.0, true),
    ("resolution_y", 1.0, 20000.0, true),
    ("display_width", 1.0, 1000.0, false),
    ("display_height", 1.0, 1000.0, false),
    ("max_print_height", 1.0, 1000.0, false),
    ("layer_height", 0.005, 0.5, false),
    ("exposure_time", 0.01, 600.0, false),
    ("bottom_exposure_time", 0.01, 600.0, false),
    ("bottom_layers", 0.0, 1000.0, true),
    ("lift_height", 0.0, 100.0, false),
    ("lift_speed", 0.01, 100.0, false),
    ("retract_speed", 0.01, 100.0, false),
    ("antialiasing", 1.0, 16.0, true),
    ("elevation", 0.0, 100.0, false),
    ("raft_thickness", 0.0, 10.0, false),
    ("supports", 0.0, 1.0, true),
    ("support_overhang_angle", 0.0, 89.0, false),
    ("support_spacing", 0.5, 20.0, false),
    ("support_tip_diameter", 0.05, 5.0, false),
    ("support_tip_length", 0.1, 20.0, false),
    ("support_pillar_diameter", 0.2, 10.0, false),
    ("support_brace_height", 0.0, 1000.0, false),
    ("hollow_wall", 0.0, 20.0, false),
    ("drain_hole_diameter", 0.0, 20.0, false),
];

/// Settings that belong to a feature that is switched off. The manifest
/// leaves them out, so a print without raft, supports and hollowing gives
/// the same archive as before these features existed.
fn unused(settings: &SlaSettings, key: &str) -> bool {
    match key {
        "elevation" => settings.elevation == 0.0,
        "raft_thickness" => settings.raft_thickness == 0.0,
        "hollow_wall" | "drain_hole_diameter" => settings.hollow_wall == 0.0,
        _ => (key == "supports" || key.starts_with("support_")) && !settings.supports,
    }
}

impl SlaSettings {
    /// Width and height of one pixel in mm.
    pub fn pixel_size(&self) -> [f64; 2] {
        [
            self.display_width / f64::from(self.resolution_x),
            self.display_height / f64::from(self.resolution_y),
        ]
    }

    /// Sets one setting from text, e.g. `("exposure_time", "2.5")`.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), SlaError> {
        let &(key, min, max, whole) = LIMITS
            .iter()
            .find(|limit| limit.0 == key)
            .ok_or_else(|| SlaError::UnknownKey(key.to_owned()))?;
        let out_of_range = || SlaError::OutOfRange {
            key,
            value: value.to_owned(),
            allowed: allowed(min, max, whole),
        };
        let number: f64 = value.trim().parse().map_err(|_| out_of_range())?;
        if !(min..=max).contains(&number) || (whole && number.fract() != 0.0) {
            return Err(out_of_range());
        }
        let mut fields =
            serde_json::to_value(&*self).map_err(|e| SlaError::Output(e.to_string()))?;
        fields[key] = if fields[key].is_boolean() {
            json!(number != 0.0)
        } else if whole {
            json!(number as u32)
        } else {
            json!(number)
        };
        *self = serde_json::from_value(fields).map_err(|e| SlaError::Output(e.to_string()))?;
        Ok(())
    }

    /// Checks every setting against its allowed range.
    pub fn validate(&self) -> Result<(), SlaError> {
        let fields = serde_json::to_value(self).map_err(|e| SlaError::Output(e.to_string()))?;
        for &(key, min, max, whole) in &LIMITS {
            let number = fields[key]
                .as_f64()
                .or_else(|| fields[key].as_bool().map(f64::from))
                .unwrap_or(f64::NAN);
            if !(min..=max).contains(&number) {
                return Err(SlaError::OutOfRange {
                    key,
                    value: number.to_string(),
                    allowed: allowed(min, max, whole),
                });
            }
        }
        Ok(())
    }

    /// Exposure of the layer with the given index (0 = bottom) in seconds.
    pub fn exposure(&self, index: usize) -> f64 {
        if index < self.bottom_layers as usize {
            self.bottom_exposure_time
        } else {
            self.exposure_time
        }
    }
}

fn allowed(min: f64, max: f64, whole: bool) -> String {
    let kind = if whole { "whole number " } else { "" };
    format!("{kind}{min} to {max}")
}

/// Why a resin print could not be produced.
#[derive(Debug, Clone, PartialEq)]
pub enum SlaError {
    /// No setting has this name.
    UnknownKey(String),
    /// A setting is not a number in its allowed range.
    OutOfRange {
        /// Name of the setting.
        key: &'static str,
        /// The rejected value.
        value: String,
        /// Description of the allowed values.
        allowed: String,
    },
    /// The mesh has no triangles.
    EmptyModel,
    /// The model does not fit on the screen or under the maximum height.
    TooLarge {
        /// Model size in X, Y, Z.
        model: [f64; 3],
        /// Printable volume in X, Y, Z.
        volume: [f64; 3],
    },
    /// The model is thinner than one layer or has no area.
    NothingToPrint,
    /// Encoding an image or the archive failed.
    Output(String),
}

impl fmt::Display for SlaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownKey(key) => write!(f, "unknown resin setting '{key}'"),
            Self::OutOfRange {
                key,
                value,
                allowed,
            } => write!(
                f,
                "setting '{key}' = {value} is out of range (allowed: {allowed})"
            ),
            Self::EmptyModel => write!(f, "the model contains no geometry"),
            Self::TooLarge { model, volume } => write!(
                f,
                "model ({:.1} x {:.1} x {:.1} mm) does not fit the build volume ({:.1} x {:.1} x {:.1} mm)",
                model[0], model[1], model[2], volume[0], volume[1], volume[2]
            ),
            Self::NothingToPrint => write!(
                f,
                "nothing to print: the model is thinner than one layer or has no area"
            ),
            Self::Output(message) => write!(f, "could not write the resin print: {message}"),
        }
    }
}

impl std::error::Error for SlaError {}

impl slicer_core::Diagnose for SlaError {
    fn code(&self) -> &'static str {
        match self {
            Self::UnknownKey(_) => "settings.unknown_key",
            Self::OutOfRange { .. } => "settings.out_of_range",
            Self::EmptyModel => "slicing.empty_model",
            Self::TooLarge { .. } => "slicing.too_large",
            Self::NothingToPrint => "slicing.nothing_to_print",
            Self::Output(_) => "sla.output",
        }
    }

    fn context(&self) -> Vec<(&'static str, String)> {
        match self {
            Self::UnknownKey(key) => vec![("key", key.clone())],
            Self::OutOfRange {
                key,
                value,
                allowed,
            } => vec![
                ("key", (*key).to_owned()),
                ("value", value.clone()),
                ("allowed", allowed.clone()),
            ],
            _ => Vec::new(),
        }
    }
}

/// One exposed layer.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    /// Height of the top of the layer above the build plate in mm.
    pub z: f64,
    /// Exposure in seconds.
    pub exposure_time: f64,
    /// Pixels that are at least partly lit.
    pub lit_pixels: u64,
    /// Resin cured in this layer in mm³ (partly lit pixels count in part).
    pub resin_volume: f64,
    /// The layer image as PNG (see [`raster`]).
    pub png: Vec<u8>,
}

/// Totals of a resin print.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SlaStats {
    /// Number of layers.
    pub layers: usize,
    /// Cured resin in millilitres.
    pub resin_volume_ml: f64,
    /// Estimated print time in seconds: exposure plus lifting and lowering
    /// the plate after every layer.
    pub print_time_s: f64,
}

/// A sliced resin print.
#[derive(Debug, Clone, PartialEq)]
pub struct SlaPrint {
    /// The settings it was sliced with.
    pub settings: SlaSettings,
    /// Layers from the build plate up.
    pub layers: Vec<Layer>,
    /// Totals.
    pub stats: SlaStats,
}

/// Cuts `mesh` into layers and renders each one. The model is centred on
/// the screen and placed on the build plate.
pub fn slice(
    mesh: &Mesh,
    settings: &SlaSettings,
    diagnostics: &mut Diagnostics,
) -> Result<SlaPrint, SlaError> {
    settings.validate()?;
    let mut mesh = mesh.clone();
    // The same repair as for filament printers, see `slicer_core::repair`.
    mesh.repair(DEGENERATE_HEIGHT).report(diagnostics);
    let (min, max) = mesh.bounds().ok_or(SlaError::EmptyModel)?;
    // The model's lowest point sits this high above the build plate.
    let base = settings.raft_thickness + settings.elevation;
    let size = [max[0] - min[0], max[1] - min[1], max[2] - min[2] + base];
    let volume = [
        settings.display_width,
        settings.display_height,
        settings.max_print_height,
    ];
    if (0..3).any(|a| size[a] > volume[a] + 1e-6) {
        return Err(SlaError::TooLarge {
            model: size,
            volume,
        });
    }
    mesh.translate([
        volume[0] / 2.0 - (min[0] + max[0]) / 2.0,
        volume[1] / 2.0 - (min[1] + max[1]) / 2.0,
        base - min[2],
    ]);

    // Layer i ends at (i + 1) * layer_height and is cut at its mid-height.
    let height = settings.layer_height;
    let count = (size[2] / height + 1e-6).floor() as usize;
    let cuts: Vec<f64> = (0..count).map(|i| (i as f64 + 0.5) * height).collect();
    let mut slices: Vec<Shapes> = slice_mesh_reporting(&mesh, &cuts, diagnostics);
    if slices.iter().all(Vec::is_empty) {
        return Err(SlaError::NothingToPrint);
    }

    let drains = if settings.hollow_wall > 0.0 {
        hollow::hollow(&mut slices, settings, diagnostics)
    } else {
        Vec::new()
    };
    if settings.elevation > 0.0 && !settings.supports {
        diagnostics.push(Diagnostic::warning(
            "sla.floating_model",
            "the model is raised by `elevation` but has no supports to stand on".to_owned(),
        ));
    }
    let structures = (settings.supports || settings.raft_thickness > 0.0)
        .then(|| supports::Structures::build(&mesh, &slices, settings, &drains));

    let [pixel_x, pixel_y] = settings.pixel_size();
    let layers = parallel_map(slices.len(), |index| {
        let shapes = match &structures {
            Some(structures) => std::borrow::Cow::Owned(structures.add_to(&slices[index], index)),
            None => std::borrow::Cow::Borrowed(&slices[index]),
        };
        let pixels = raster::rasterize(&shapes, settings);
        let lit = pixels.iter().filter(|&&p| p > 0).count() as u64;
        let sum: u64 = pixels.iter().map(|&p| u64::from(p)).sum();
        let png = raster::encode_png(settings.resolution_x, settings.resolution_y, &pixels)
            .map_err(|e| SlaError::Output(e.to_string()))?;
        Ok(Layer {
            z: round((index + 1) as f64 * height, 1e6),
            exposure_time: settings.exposure(index),
            lit_pixels: lit,
            resin_volume: sum as f64 / 255.0 * pixel_x * pixel_y * height,
            png,
        })
    })
    .into_iter()
    .collect::<Result<Vec<_>, SlaError>>()?;

    let move_time =
        settings.lift_height / settings.lift_speed + settings.lift_height / settings.retract_speed;
    let stats = SlaStats {
        layers: layers.len(),
        resin_volume_ml: round(
            layers.iter().map(|l| l.resin_volume).sum::<f64>() / 1000.0,
            1e4,
        ),
        print_time_s: round(
            layers.iter().map(|l| l.exposure_time + move_time).sum(),
            10.0,
        ),
    };
    Ok(SlaPrint {
        settings: settings.clone(),
        layers,
        stats,
    })
}

/// Corners of a regular polygon around the origin with circumradius 1,
/// counter-clockwise, from the platform-independent sine and cosine.
fn unit_circle(segments: usize) -> Vec<slicer_core::Point> {
    (0..segments)
        .map(|i| {
            let (sin, cos) =
                slicer_core::polygon::sin_cos_degrees(360.0 * i as f64 / segments as f64);
            [cos, sin]
        })
        .collect()
}

/// A disc as a polygon with the corners of `unit` scaled and moved.
fn disc(
    unit: &[slicer_core::Point],
    center: slicer_core::Point,
    radius: f64,
) -> slicer_core::Contour {
    unit.iter()
        .map(|p| [center[0] + radius * p[0], center[1] + radius * p[1]])
        .collect()
}

/// Rounds to a multiple of `1 / scale`.
fn round(value: f64, scale: f64) -> f64 {
    (value * scale).round() / scale
}

/// Name of a layer image inside the archive; `index` counts from 0.
pub fn layer_file(index: usize) -> String {
    format!("layers/{:05}.png", index + 1)
}

/// The JSON manifest describing the print (see `docs/sla-format.md`).
pub fn manifest(print: &SlaPrint) -> serde_json::Value {
    let settings = &print.settings;
    let layers: Vec<_> = print
        .layers
        .iter()
        .enumerate()
        .map(|(index, layer)| {
            json!({
                "file": layer_file(index),
                "z": layer.z,
                "exposure_time": layer.exposure_time,
                "lit_pixels": layer.lit_pixels,
                "resin_volume_mm3": round(layer.resin_volume, 1e3),
            })
        })
        .collect();
    let mut used_settings = json!(settings);
    if let Some(fields) = used_settings.as_object_mut() {
        fields.retain(|key, _| !unused(settings, key));
    }
    json!({
        "format": "spaghetti-sla",
        "format_version": 1,
        "generator": concat!("spaghetti-slicer ", env!("CARGO_PKG_VERSION")),
        "image": {
            "width": settings.resolution_x,
            "height": settings.resolution_y,
            "pixel_size": settings.pixel_size(),
            "encoding": "png-gray8",
            "orientation": "top-view",
        },
        "settings": used_settings,
        "stats": print.stats,
        "layers": layers,
    })
}

/// Packs the print into a ZIP archive: `manifest.json` followed by one PNG
/// per layer. The same print always gives the same bytes.
pub fn write_archive(print: &SlaPrint) -> Result<Vec<u8>, SlaError> {
    let error = |e: &dyn fmt::Display| SlaError::Output(e.to_string());
    let manifest = serde_json::to_vec_pretty(&manifest(print)).map_err(|e| error(&e))?;
    // A fixed date keeps the archive identical from run to run.
    let options = zip::write::SimpleFileOptions::default()
        .last_modified_time(zip::DateTime::default())
        .unix_permissions(0o644);
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file(
            "manifest.json",
            options.compression_method(zip::CompressionMethod::Deflated),
        )
        .map_err(|e| error(&e))?;
    writer.write_all(&manifest).map_err(|e| error(&e))?;
    // The images are compressed already.
    let stored = options.compression_method(zip::CompressionMethod::Stored);
    for (index, layer) in print.layers.iter().enumerate() {
        writer
            .start_file(layer_file(index), stored)
            .map_err(|e| error(&e))?;
        writer.write_all(&layer.png).map_err(|e| error(&e))?;
    }
    Ok(writer.finish().map_err(|e| error(&e))?.into_inner())
}

#[cfg(test)]
mod tests;
