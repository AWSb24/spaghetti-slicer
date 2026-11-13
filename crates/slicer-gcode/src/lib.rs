//! G-code generation with exchangeable flavors (Bambu, Marlin, Klipper).
//!
//! Marlin and Klipper are implemented and currently produce the same
//! commands; the Bambu flavor does not exist yet.

use std::f64::consts::PI;
use std::fmt::{self, Write};

use serde::Serialize;
use slicer_core::Point;
use slicer_core::polygon::distance;
use slicer_fdm::{PathKind, Toolpaths};
use slicer_profiles::{Flavor, Settings};

/// Summary of a generated print.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stats {
    /// Number of layers.
    pub layers: usize,
    /// Filament consumed, in millimetres of filament.
    pub filament_length_mm: f64,
    /// Filament consumed, in cm³.
    pub filament_volume_cm3: f64,
    /// Filament consumed, in grams.
    pub filament_weight_g: f64,
    /// Estimated print time in seconds, with acceleration and slowing down
    /// at corners. Ignores heating and everything the start and end G-code
    /// do. Not yet compared with a real print.
    pub print_time_s: f64,
}

/// Generated G-code with its statistics.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    /// The complete G-code file.
    pub gcode: String,
    /// Statistics, also contained in the file header.
    pub stats: Stats,
}

/// Why G-code could not be generated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GcodeError {
    /// Custom G-code refers to a setting that does not exist.
    UnknownPlaceholder {
        /// Setting that contains the placeholder (`start_gcode` or `end_gcode`).
        setting: &'static str,
        /// The unknown name.
        name: String,
        /// One-based line within the custom G-code.
        line: usize,
    },
    /// A `{` in custom G-code is never closed.
    UnclosedPlaceholder {
        /// Setting that contains the placeholder.
        setting: &'static str,
        /// One-based line within the custom G-code.
        line: usize,
    },
}

impl fmt::Display for GcodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownPlaceholder {
                setting,
                name,
                line,
            } => {
                write!(
                    f,
                    "{setting}, line {line}: unknown placeholder '{{{name}}}'"
                )
            }
            Self::UnclosedPlaceholder { setting, line } => {
                write!(f, "{setting}, line {line}: '{{' without matching '}}'")
            }
        }
    }
}

impl std::error::Error for GcodeError {}

impl slicer_core::Diagnose for GcodeError {
    fn code(&self) -> &'static str {
        match self {
            Self::UnknownPlaceholder { .. } => "gcode.unknown_placeholder",
            Self::UnclosedPlaceholder { .. } => "gcode.unclosed_placeholder",
        }
    }

    fn context(&self) -> Vec<(&'static str, String)> {
        match self {
            Self::UnknownPlaceholder {
                setting,
                name,
                line,
            } => vec![
                ("setting", (*setting).to_owned()),
                ("name", name.clone()),
                ("line", line.to_string()),
            ],
            Self::UnclosedPlaceholder { setting, line } => vec![
                ("setting", (*setting).to_owned()),
                ("line", line.to_string()),
            ],
        }
    }
}

fn label(kind: PathKind) -> &'static str {
    match kind {
        PathKind::Skirt => "Skirt",
        PathKind::Brim => "Brim",
        PathKind::OuterWall => "Outer wall",
        PathKind::InnerWall => "Inner wall",
        PathKind::SolidInfill => "Solid infill",
        PathKind::SparseInfill => "Sparse infill",
    }
}

fn expand(
    template: &str,
    setting: &'static str,
    settings: &Settings,
) -> Result<String, GcodeError> {
    let values = settings.placeholder_values();
    let mut out = String::new();
    for (index, text) in template.lines().enumerate() {
        let line = index + 1;
        let mut rest = text;
        while let Some(open) = rest.find('{') {
            out.push_str(&rest[..open]);
            let after = &rest[open + 1..];
            let close = after
                .find('}')
                .ok_or(GcodeError::UnclosedPlaceholder { setting, line })?;
            let name = after[..close].trim();
            let value = values
                .get(name)
                .ok_or_else(|| GcodeError::UnknownPlaceholder {
                    setting,
                    name: name.to_owned(),
                    line,
                })?;
            out.push_str(value);
            rest = &after[close + 1..];
        }
        out.push_str(rest);
        out.push('\n');
    }
    Ok(out)
}

/// One movement of the printer, as far as its duration is concerned.
struct Move {
    /// Unit vector in the XY plane; zero for moves along Z or of the
    /// filament only.
    direction: Point,
    /// Distance in millimetres.
    length: f64,
    /// Requested speed in mm/s.
    speed: f64,
}

/// Time in seconds the moves take when the printer starts and ends at rest,
/// changes its speed by at most `acceleration` (mm/s²) and may change its
/// velocity at once by at most `jerk` (mm/s) where two moves meet.
///
/// The speeds where moves meet are limited three ways: by the requested
/// speeds, by the change of direction, and by what the neighbouring moves can
/// reach or shed over their length. Each move then accelerates, cruises and
/// brakes.
fn planned_time(moves: &[Move], acceleration: f64, jerk: f64) -> f64 {
    let reachable = |from: f64, m: &Move| (from * from + 2.0 * acceleration * m.length).sqrt();
    // junction[i] is the speed at the start of move i; both ends are at rest.
    let mut junction = vec![0.0; moves.len() + 1];
    for i in 1..moves.len() {
        let (a, b) = (&moves[i - 1], &moves[i]);
        let change = distance(a.direction, b.direction);
        let corner = if change > 1e-9 {
            jerk / change
        } else {
            f64::INFINITY
        };
        junction[i] = a.speed.min(b.speed).min(corner);
    }
    for (i, m) in moves.iter().enumerate() {
        junction[i + 1] = junction[i + 1].min(reachable(junction[i], m));
    }
    for (i, m) in moves.iter().enumerate().rev() {
        junction[i] = junction[i].min(reachable(junction[i + 1], m));
    }
    moves
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let (start, end) = (junction[i], junction[i + 1]);
            // Highest speed when accelerating from `start` and braking to
            // `end` without cruising.
            let peak = ((2.0 * acceleration * m.length + start * start + end * end) / 2.0)
                .sqrt()
                .min(m.speed);
            let ramps = (2.0 * peak * peak - start * start - end * end) / (2.0 * acceleration);
            (2.0 * peak - start - end) / acceleration + (m.length - ramps).max(0.0) / peak
        })
        .sum()
}

struct Writer<'a> {
    settings: &'a Settings,
    out: String,
    position: Option<Point>,
    z: f64,
    feed: f64,
    retracted: bool,
    filament: f64,
    /// Every movement so far, for the time estimate.
    moves: Vec<Move>,
    /// Height of the nozzle; unknown before the first Z move.
    nozzle_z: Option<f64>,
}

impl Writer<'_> {
    /// Returns the ` F...` word if the feed rate (mm/s) differs from the
    /// active one.
    fn feed_word(&mut self, speed: f64) -> String {
        let feed = (speed * 60.0).round();
        if feed == self.feed {
            String::new()
        } else {
            self.feed = feed;
            format!(" F{feed}")
        }
    }

    fn retract(&mut self) {
        let length = self.settings.retraction_length;
        if length > 0.0 && !self.retracted {
            let f = self.feed_word(self.settings.retraction_speed);
            let _ = writeln!(self.out, "G1 E-{length:.5}{f}");
            self.retracted = true;
            self.filament -= length;
            self.record([0.0, 0.0], length, self.settings.retraction_speed);
        }
    }

    fn unretract(&mut self) {
        if self.retracted {
            let length = self.settings.retraction_length;
            let f = self.feed_word(self.settings.retraction_speed);
            let _ = writeln!(self.out, "G1 E{length:.5}{f}");
            self.retracted = false;
            self.filament += length;
            self.record([0.0, 0.0], length, self.settings.retraction_speed);
        }
    }

    fn record(&mut self, direction: Point, length: f64, speed: f64) {
        if length > 0.0 {
            self.moves.push(Move {
                direction,
                length,
                speed,
            });
        }
    }

    fn move_z(&mut self, z: f64) {
        let f = self.feed_word(self.settings.travel_speed);
        let _ = writeln!(self.out, "G1 Z{z:.3}{f}");
        if let Some(from) = self.nozzle_z.replace(z) {
            self.record([0.0, 0.0], (z - from).abs(), self.settings.travel_speed);
        }
    }

    fn travel(&mut self, to: Point) {
        let length = self
            .position
            .map_or(f64::INFINITY, |from| distance(from, to));
        if length < 1e-4 {
            return;
        }
        // The travel is recorded between the hop moves, where it happens.
        let direction = self
            .position
            .map(|from| [(to[0] - from[0]) / length, (to[1] - from[1]) / length]);
        let long = self.position.is_some() && length >= self.settings.retraction_min_travel;
        let hop = long && self.settings.z_hop > 0.0 && self.settings.retraction_length > 0.0;
        if long {
            self.retract();
        }
        if hop {
            self.move_z(self.z + self.settings.z_hop);
        }
        let f = self.feed_word(self.settings.travel_speed);
        let _ = writeln!(self.out, "G0 X{:.3} Y{:.3}{f}", to[0], to[1]);
        if let Some(direction) = direction {
            self.record(direction, length, self.settings.travel_speed);
        }
        if hop {
            self.move_z(self.z);
        }
        self.position = Some(to);
    }

    fn extrude(&mut self, to: Point, filament_per_mm: f64, speed: f64) {
        let Some(from) = self.position else { return };
        let length = distance(from, to);
        if length < 1e-4 {
            return;
        }
        self.unretract();
        let e = length * filament_per_mm;
        let f = self.feed_word(speed);
        let _ = writeln!(self.out, "G1 X{:.3} Y{:.3} E{e:.5}{f}", to[0], to[1]);
        self.filament += e;
        let direction = [(to[0] - from[0]) / length, (to[1] - from[1]) / length];
        self.record(direction, length, speed);
        self.position = Some(to);
    }
}

/// Converts toolpaths into a G-code file.
///
/// Extrusion is relative (`M83`). Every millimetre of path deposits a volume
/// of line width times layer height, so parallel lines one line width apart
/// fill a layer completely.
pub fn write_gcode(toolpaths: &Toolpaths, settings: &Settings) -> Result<Output, GcodeError> {
    let start = expand(&settings.start_gcode, "start_gcode", settings)?;
    let end = expand(&settings.end_gcode, "end_gcode", settings)?;
    let filament_area = PI * (settings.filament_diameter / 2.0).powi(2);

    let mut w = Writer {
        settings,
        out: String::new(),
        position: None,
        z: 0.0,
        feed: 0.0,
        retracted: false,
        filament: 0.0,
        moves: Vec::new(),
        nozzle_z: None,
    };
    w.out.push_str(&start);
    w.out.push_str("G21\nG90\nM83\n");

    for (index, layer) in toolpaths.layers.iter().enumerate() {
        let _ = writeln!(
            w.out,
            ";LAYER_CHANGE\n;Z:{:.3}\n;HEIGHT:{:.3}",
            layer.z, layer.height
        );
        if index == 1 {
            if settings.nozzle_temp != settings.nozzle_temp_first_layer {
                let _ = writeln!(w.out, "M104 S{}", settings.nozzle_temp);
            }
            if settings.bed_temp != settings.bed_temp_first_layer {
                let _ = writeln!(w.out, "M140 S{}", settings.bed_temp);
            }
        }
        if index == settings.fan_off_layers as usize && settings.fan_speed > 0.0 {
            let _ = writeln!(w.out, "M106 S{:.0}", settings.fan_speed * 2.55);
        }
        w.z = layer.z;
        w.move_z(layer.z);

        let filament_per_mm =
            settings.line_width * layer.height * settings.flow_ratio / filament_area;
        let mut current_kind = None;
        for path in &layer.paths {
            let Some(&first) = path.points.first() else {
                continue;
            };
            w.travel(first);
            if current_kind != Some(path.kind) {
                current_kind = Some(path.kind);
                let _ = writeln!(w.out, ";TYPE:{}", label(path.kind));
            }
            let speed = if index == 0 {
                settings.first_layer_speed
            } else {
                match path.kind {
                    PathKind::OuterWall => settings.outer_wall_speed,
                    PathKind::InnerWall => settings.inner_wall_speed,
                    PathKind::SolidInfill | PathKind::SparseInfill => settings.infill_speed,
                    PathKind::Skirt | PathKind::Brim => settings.first_layer_speed,
                }
            };
            for &point in &path.points[1..] {
                w.extrude(point, filament_per_mm, speed);
            }
            if path.closed {
                w.extrude(first, filament_per_mm, speed);
            }
        }
    }
    w.retract();
    w.out.push_str(&end);

    // The final retraction is not consumed filament.
    let used = w.filament
        + if w.retracted {
            settings.retraction_length
        } else {
            0.0
        };
    let volume_cm3 = used * filament_area / 1000.0;
    let stats = Stats {
        layers: toolpaths.layers.len(),
        filament_length_mm: used,
        filament_volume_cm3: volume_cm3,
        filament_weight_g: volume_cm3 * settings.filament_density,
        print_time_s: planned_time(&w.moves, settings.acceleration, settings.jerk),
    };

    let flavor = match settings.flavor {
        Flavor::Marlin => "marlin",
        Flavor::Klipper => "klipper",
    };
    let mut gcode = String::new();
    let _ = writeln!(
        gcode,
        "; generated by spaghetti-slicer {}",
        env!("CARGO_PKG_VERSION")
    );
    let _ = writeln!(gcode, "; flavor: {flavor}");
    let _ = writeln!(gcode, "; layers: {}", stats.layers);
    let _ = writeln!(
        gcode,
        "; filament used [mm]: {:.2}",
        stats.filament_length_mm
    );
    let _ = writeln!(
        gcode,
        "; filament used [cm3]: {:.2}",
        stats.filament_volume_cm3
    );
    let _ = writeln!(gcode, "; filament used [g]: {:.2}", stats.filament_weight_g);
    let _ = writeln!(
        gcode,
        "; estimated printing time [s]: {:.0} (with acceleration, without heating)",
        stats.print_time_s
    );
    gcode.push_str(&w.out);
    gcode.push_str("; settings\n");
    for (key, value) in settings.placeholder_values() {
        let _ = writeln!(gcode, "; {key} = {value}");
    }
    Ok(Output { gcode, stats })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::field_reassign_with_default)]
    use super::*;
    use slicer_core::Mesh;

    fn slice(mesh: &Mesh, settings: &Settings) -> Output {
        write_gcode(&slicer_fdm::generate(mesh, settings).unwrap(), settings).unwrap()
    }

    /// Value of the word starting with `letter` in a G-code line.
    fn word(line: &str, letter: char) -> Option<f64> {
        line.split(';')
            .next()?
            .split_whitespace()
            .skip(1)
            .find_map(|w| w.strip_prefix(letter).and_then(|v| v.parse().ok()))
    }

    #[test]
    fn cube_gcode_is_well_formed() {
        let settings = Settings::default();
        let output = slice(&Mesh::cuboid(20.0, 20.0, 20.0), &settings);
        let mut e_sum = 0.0;
        let mut last_z = 0.0;
        for line in output.gcode.lines() {
            if line.starts_with("G0 ") || line.starts_with("G1 ") {
                assert!(!line.contains("NaN") && !line.contains("inf"), "{line}");
                for (axis, max) in [('X', settings.bed_size_x), ('Y', settings.bed_size_y)] {
                    if let Some(v) = word(line, axis) {
                        assert!((0.0..=max).contains(&v), "{line}");
                    }
                }
                if let Some(z) = word(line, 'Z') {
                    // The end G-code lifts the nozzle with a relative move.
                    assert!(z >= last_z || z == 10.0, "{line}");
                    last_z = z;
                }
                e_sum += word(line, 'E').unwrap_or(0.0);
            }
        }
        assert_eq!(output.stats.layers, 100);
        // Net extrusion in the file is the filament used minus the final retraction.
        let expected = output.stats.filament_length_mm - settings.retraction_length;
        assert!((e_sum - expected).abs() < 0.05, "{e_sum} vs {expected}");
        assert_eq!(output.gcode.matches(";LAYER_CHANGE").count(), 100);
        assert!(output.gcode.contains("M109 S215\n"));
        assert!(output.gcode.contains("M104 S210\n"));
        assert!(output.gcode.contains("M106 S255\n"));
        assert!(output.gcode.contains(";TYPE:Outer wall\n"));
        assert!(output.gcode.starts_with("; generated by spaghetti-slicer"));
    }

    #[test]
    fn filament_matches_model_volume_for_solid_part() {
        let mut settings = Settings::default();
        settings.infill_density = 100.0;
        settings.skirt_loops = 0;
        settings.infill_overlap = 0.0;
        let output = slice(&Mesh::cuboid(20.0, 20.0, 10.0), &settings);
        let ratio = output.stats.filament_volume_cm3 / 4.0;
        assert!((ratio - 1.0).abs() < 0.03, "{ratio}");
        assert!((output.stats.filament_weight_g - 4.0 * 1.24 * ratio).abs() < 1e-9);
    }

    #[test]
    fn time_estimate_follows_acceleration_and_corners() {
        let go = |direction, length, speed| Move {
            direction,
            length,
            speed,
        };
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        // 100 mm at 50 mm/s with 1000 mm/s²: 0.05 s and 1.25 mm for each
        // ramp, 97.5 mm of cruising.
        let straight = planned_time(&[go([1.0, 0.0], 100.0, 50.0)], 1000.0, 8.0);
        assert!(close(straight, 0.05 + 0.05 + 97.5 / 50.0), "{straight}");
        // Too short to reach the speed: accelerate half way, then brake.
        let short = planned_time(&[go([1.0, 0.0], 1.0, 50.0)], 1000.0, 8.0);
        assert!(close(short, 2.0 * (0.5_f64 / 500.0).sqrt()), "{short}");
        // Cutting a straight line in two changes nothing.
        let halves = [go([1.0, 0.0], 50.0, 50.0), go([1.0, 0.0], 50.0, 50.0)];
        assert!(close(planned_time(&halves, 1000.0, 8.0), straight));
        // A right angle is taken at jerk / sqrt(2), which costs time; a
        // full stop costs more.
        let corner = [go([1.0, 0.0], 50.0, 50.0), go([0.0, 1.0], 50.0, 50.0)];
        let stop = [go([1.0, 0.0], 50.0, 50.0), go([-1.0, 0.0], 50.0, 50.0)];
        let (corner, stop) = (
            planned_time(&corner, 1000.0, 8.0),
            planned_time(&stop, 1000.0, 8.0),
        );
        let slow = 8.0 / 2.0_f64.sqrt();
        let expected = straight + 2.0 * (50.0 - slow) / 1000.0
            - 2.0 * (50.0 * 50.0 - slow * slow) / 2000.0 / 50.0;
        assert!(close(corner, expected), "{corner} {expected}");
        assert!(straight < corner && corner < stop);
        assert!(close(planned_time(&[], 1000.0, 8.0), 0.0));

        // The estimate of a real slice is longer than distance over speed.
        let mut settings = Settings::default();
        let fast = slice(&Mesh::cuboid(20.0, 20.0, 2.0), &settings)
            .stats
            .print_time_s;
        settings.acceleration = 100.0;
        let slow = slice(&Mesh::cuboid(20.0, 20.0, 2.0), &settings)
            .stats
            .print_time_s;
        assert!(fast.is_finite() && slow > fast, "{fast} {slow}");
    }

    #[test]
    fn output_is_reproducible() {
        let settings = Settings::default();
        let mesh = Mesh::cuboid(12.0, 17.0, 3.0);
        assert_eq!(slice(&mesh, &settings).gcode, slice(&mesh, &settings).gcode);
    }

    #[test]
    fn retraction_can_be_disabled_and_z_hop_lifts() {
        let mesh = Mesh::cuboid(10.0, 10.0, 1.0);
        let mut settings = Settings::default();
        settings.retraction_length = 0.0;
        assert!(!slice(&mesh, &settings).gcode.contains("E-"));

        let mut settings = Settings::default();
        settings.z_hop = 0.4;
        let gcode = slice(&mesh, &settings).gcode;
        assert!(gcode.contains("G1 Z0.600"), "first layer hop missing");
    }

    #[test]
    fn placeholders() {
        let mut settings = Settings::default();
        settings.start_gcode = "M104 S{ nozzle_temp }\n; {flavor}".into();
        let toolpaths = slicer_fdm::generate(&Mesh::cuboid(5.0, 5.0, 1.0), &settings).unwrap();
        let gcode = write_gcode(&toolpaths, &settings).unwrap().gcode;
        assert!(gcode.contains("M104 S210\n; marlin\n"));

        settings.start_gcode = "G28\nM104 S{nozle_temp}".into();
        assert_eq!(
            write_gcode(&toolpaths, &settings).unwrap_err().to_string(),
            "start_gcode, line 2: unknown placeholder '{nozle_temp}'"
        );
        settings.start_gcode = "G28".into();
        settings.end_gcode = "M104 S{nozzle_temp".into();
        assert_eq!(
            write_gcode(&toolpaths, &settings).unwrap_err(),
            GcodeError::UnclosedPlaceholder {
                setting: "end_gcode",
                line: 1
            }
        );
    }
}
