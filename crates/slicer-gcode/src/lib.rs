//! G-code generation with exchangeable flavors (Bambu, Marlin, Klipper).
//!
//! Marlin and Klipper are implemented and currently produce the same
//! commands; the Bambu flavor does not exist yet.

pub mod calibration;

use std::collections::BTreeMap;
use std::f64::consts::PI;
use std::fmt::{self, Write};

use serde::Serialize;
use slicer_core::Point;
use slicer_core::polygon::distance;
use slicer_fdm::{Layer, PathKind, Progress, Step, Toolpaths};
use slicer_profiles::{ExtrusionMode, Flavor, Settings};

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
    /// The same figures for every extruder the print uses, with more than
    /// one extruder (`extruder_count`); empty otherwise.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extruders: Vec<ExtruderStats>,
    /// Estimated seconds per line type, in the order the types first
    /// appear; travel, retraction and Z moves are under `None`. They add up
    /// to `print_time_s`. Not written to the file or to JSON.
    #[serde(skip)]
    pub times: Vec<(Option<PathKind>, f64)>,
}

/// Summary of what one extruder prints.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExtruderStats {
    /// The extruder, counted from 0 as in `T<n>`.
    pub extruder: u32,
    /// Filament consumed, in millimetres of filament.
    pub filament_length_mm: f64,
    /// Filament consumed, in cm³.
    pub filament_volume_cm3: f64,
    /// Filament consumed, in grams.
    pub filament_weight_g: f64,
    /// Estimated time in seconds while this extruder is selected, travel
    /// moves included; tool changes themselves take no time here.
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
        /// Setting that contains the placeholder (`start_gcode`, `end_gcode`
        /// or `toolchange_gcode`).
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
        PathKind::Support => "Support",
        PathKind::OuterWall => "Outer wall",
        PathKind::InnerWall => "Inner wall",
        PathKind::GapFill => "Gap infill",
        PathKind::SolidInfill => "Solid infill",
        PathKind::Bridge => "Bridge",
        PathKind::SparseInfill => "Sparse infill",
        PathKind::PrimeTower => "Prime tower",
        PathKind::Ironing => "Ironing",
    }
}

/// Values for the placeholders of `toolchange_gcode`: the settings and the
/// extruders before and after the change.
fn toolchange_values(
    settings: &BTreeMap<String, String>,
    previous: u32,
    next: u32,
) -> BTreeMap<String, String> {
    let mut values = settings.clone();
    values.insert("previous_extruder".to_owned(), previous.to_string());
    values.insert("next_extruder".to_owned(), next.to_string());
    values
}

fn expand(
    template: &str,
    setting: &'static str,
    values: &BTreeMap<String, String>,
) -> Result<String, GcodeError> {
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
#[derive(Clone)]
struct Move {
    /// Unit vector in the XY plane; zero for moves along Z or of the
    /// filament only.
    direction: Point,
    /// Distance in millimetres.
    length: f64,
    /// Requested speed in mm/s.
    speed: f64,
    /// Extruder selected during the move (its slot in the writer).
    tool: usize,
    /// What is extruded; `None` for travel, retraction and Z moves.
    kind: Option<PathKind>,
}

/// Time in seconds the moves take when the printer starts and ends at rest,
/// changes its speed by at most `acceleration` (mm/s²) and may change its
/// velocity at once by at most `jerk` (mm/s) where two moves meet.
#[cfg(test)]
fn planned_time(moves: &[Move], acceleration: f64, jerk: f64) -> f64 {
    move_times(moves, acceleration, jerk).iter().sum()
}

/// Time in seconds every move takes, as for [`planned_time`].
///
/// The speeds where moves meet are limited three ways: by the requested
/// speeds, by the change of direction, and by what the neighbouring moves can
/// reach or shed over their length. Each move then accelerates, cruises and
/// brakes.
fn move_times(moves: &[Move], acceleration: f64, jerk: f64) -> Vec<f64> {
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
        .collect()
}

struct Writer<'a> {
    settings: &'a Settings,
    out: String,
    position: Option<Point>,
    z: f64,
    feed: f64,
    /// Slot of the selected extruder in `retracted` and `filament`: its
    /// number with several extruders, otherwise 0.
    tool: usize,
    /// Per extruder: whether its filament is pulled back.
    retracted: Vec<bool>,
    /// Per extruder: filament pushed through so far, in millimetres.
    filament: Vec<f64>,
    /// Filament extruded since the last `G92 E0`; only used for absolute
    /// extrusion.
    e: f64,
    /// Every movement so far, for the time estimate.
    moves: Vec<Move>,
    /// Height of the nozzle; unknown before the first Z move.
    nozzle_z: Option<f64>,
}

/// What the layers hand on to the next one besides the printer's state.
#[derive(Debug, Clone, Copy, Default)]
struct Carry {
    /// What the fan was last told, in percent.
    fan: f64,
    /// The extruder selected with `T<n>`, none before the first one.
    selected: Option<u32>,
    /// A tool change may have moved the nozzle away from the layer.
    restore_z: bool,
}

/// What every layer needs from the whole print.
struct Context<'a> {
    values: &'a BTreeMap<String, String>,
    filament_area: f64,
    several: bool,
    absolute: bool,
}

impl<'a> Writer<'a> {
    /// A writer in the same state, with nothing written or recorded yet,
    /// to try a layer out.
    fn fork(&self) -> Writer<'a> {
        Writer {
            settings: self.settings,
            out: String::new(),
            position: self.position,
            z: self.z,
            feed: self.feed,
            tool: self.tool,
            retracted: self.retracted.clone(),
            filament: self.filament.clone(),
            e: self.e,
            moves: Vec::new(),
            nozzle_z: self.nozzle_z,
        }
    }

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

    /// The ` E...` word for adding `amount` millimetres of filament.
    fn e_word(&mut self, amount: f64) -> String {
        if self.settings.extrusion_mode == ExtrusionMode::Absolute {
            self.e += amount;
            format!(" E{:.5}", self.e)
        } else {
            format!(" E{amount:.5}")
        }
    }

    fn retract(&mut self) {
        let length = self.settings.retraction_length;
        if length > 0.0 && !self.retracted[self.tool] {
            let f = self.feed_word(self.settings.retraction_speed);
            let e = self.e_word(-length);
            let _ = writeln!(self.out, "G1{e}{f}");
            self.retracted[self.tool] = true;
            self.filament[self.tool] -= length;
            self.record([0.0, 0.0], length, self.settings.retraction_speed, None);
        }
    }

    fn unretract(&mut self) {
        if self.retracted[self.tool] {
            let length = self.settings.retraction_length;
            let f = self.feed_word(self.settings.retraction_speed);
            let e = self.e_word(length);
            let _ = writeln!(self.out, "G1{e}{f}");
            self.retracted[self.tool] = false;
            self.filament[self.tool] += length;
            self.record([0.0, 0.0], length, self.settings.retraction_speed, None);
        }
    }

    /// Makes `tool` the selected extruder. An extruder used for the first
    /// time starts with its filament not pulled back; one used before
    /// stays retracted until it extrudes.
    fn select(&mut self, tool: usize) {
        if tool >= self.retracted.len() {
            self.retracted.resize(tool + 1, false);
            self.filament.resize(tool + 1, 0.0);
        }
        self.tool = tool;
    }

    fn record(&mut self, direction: Point, length: f64, speed: f64, kind: Option<PathKind>) {
        if length > 0.0 {
            self.moves.push(Move {
                direction,
                length,
                speed,
                tool: self.tool,
                kind,
            });
        }
    }

    fn move_z(&mut self, z: f64) {
        let f = self.feed_word(self.settings.travel_speed);
        let _ = writeln!(self.out, "G1 Z{z:.3}{f}");
        if let Some(from) = self.nozzle_z.replace(z) {
            self.record(
                [0.0, 0.0],
                (z - from).abs(),
                self.settings.travel_speed,
                None,
            );
        }
    }

    /// Moves without extruding. `inside` says that the way stays over the
    /// part, where a little oozing does no harm and retraction is skipped.
    fn travel(&mut self, to: Point, inside: bool) {
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
        let long =
            self.position.is_some() && !inside && length >= self.settings.retraction_min_travel;
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
            self.record(direction, length, self.settings.travel_speed, None);
        }
        if hop {
            self.move_z(self.z);
        }
        self.position = Some(to);
    }

    /// Like [`Writer::extrude`], and the nozzle rises to `z` on the way.
    fn extrude_rising(
        &mut self,
        to: Point,
        z: f64,
        filament_per_mm: f64,
        speed: f64,
        kind: PathKind,
    ) {
        let Some(from) = self.position else { return };
        let length = distance(from, to);
        if length < 1e-4 {
            return;
        }
        self.unretract();
        let e = length * filament_per_mm;
        let f = self.feed_word(speed);
        let word = self.e_word(e);
        let _ = writeln!(self.out, "G1 X{:.3} Y{:.3} Z{z:.3}{word}{f}", to[0], to[1]);
        self.filament[self.tool] += e;
        let direction = [(to[0] - from[0]) / length, (to[1] - from[1]) / length];
        self.record(direction, length, speed, Some(kind));
        self.position = Some(to);
        self.nozzle_z = Some(z);
    }

    fn extrude(&mut self, to: Point, filament_per_mm: f64, speed: f64, kind: PathKind) {
        let Some(from) = self.position else { return };
        let length = distance(from, to);
        if length < 1e-4 {
            return;
        }
        self.unretract();
        let e = length * filament_per_mm;
        let f = self.feed_word(speed);
        let word = self.e_word(e);
        let _ = writeln!(self.out, "G1 X{:.3} Y{:.3}{word}{f}", to[0], to[1]);
        self.filament[self.tool] += e;
        let direction = [(to[0] - from[0]) / length, (to[1] - from[1]) / length];
        self.record(direction, length, speed, Some(kind));
        self.position = Some(to);
    }
}

/// Writes one layer. Extrusion runs at `slow` times its speed (1 for full
/// speed), see [`slowdown`].
fn write_layer(
    w: &mut Writer,
    carry: &mut Carry,
    layer: &Layer,
    index: usize,
    context: &Context,
    slow: f64,
) -> Result<(), GcodeError> {
    let settings = w.settings;
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
    let layer_fan = if index >= settings.fan_off_layers as usize {
        settings.fan_speed
    } else {
        0.0
    };
    if layer_fan != carry.fan {
        carry.fan = layer_fan;
        let _ = writeln!(w.out, "M106 S{:.0}", carry.fan * 2.55);
    }
    if context.absolute {
        // Start every layer at zero so the numbers stay short and exact.
        w.out.push_str("G92 E0\n");
        w.e = 0.0;
    }
    // In vase mode the one wall rises from where the layer below ended;
    // a layer that has more to print is printed flat.
    let spiral = layer.spiral
        && matches!(layer.paths.as_slice(), [only] if only.closed && only.kind == PathKind::OuterWall);
    let rise_from = w.nozzle_z.filter(|_| spiral);
    match rise_from {
        Some(from) => w.z = from,
        None => {
            w.z = layer.z;
            w.move_z(layer.z);
        }
    }

    let filament_per_mm =
        settings.line_width * layer.height * settings.flow_ratio / context.filament_area;
    let mut current_kind = None;
    for path in &layer.paths {
        let Some(&first) = path.points.first() else {
            continue;
        };
        if context.several && carry.selected != Some(path.extruder) {
            let next = path.extruder;
            if let Some(previous) = carry.selected {
                w.retract();
                let values = toolchange_values(context.values, previous, next);
                let block = expand(&settings.toolchange_gcode, "toolchange_gcode", &values)?;
                w.out.push_str(&block);
                let _ = writeln!(w.out, "T{next}");
                if context.absolute {
                    w.out.push_str("G92 E0\n");
                    w.e = 0.0;
                }
                // Where the custom G-code left the nozzle is unknown.
                w.position = None;
                carry.restore_z = true;
            } else {
                let _ = writeln!(w.out, "T{next}");
            }
            carry.selected = Some(next);
            w.select(next as usize);
        }
        w.travel(first, path.travel_inside);
        if carry.restore_z {
            w.move_z(layer.z);
            carry.restore_z = false;
        }
        let bridge = path.kind == PathKind::Bridge;
        let wanted = if bridge {
            settings.bridge_fan_speed
        } else {
            layer_fan
        };
        if wanted != carry.fan {
            carry.fan = wanted;
            let _ = writeln!(w.out, "M106 S{:.0}", carry.fan * 2.55);
        }
        let filament_per_mm = if bridge {
            filament_per_mm * settings.bridge_flow
        } else {
            filament_per_mm
        };
        let filament_per_mm = if path.kind == PathKind::Ironing {
            filament_per_mm * settings.ironing_flow / 100.0
        } else {
            filament_per_mm
        };
        // Lines of their own width get material in proportion to it.
        let filament_per_mm = match path.width {
            Some(width) => filament_per_mm * width / settings.line_width,
            None => filament_per_mm,
        };
        if current_kind != Some(path.kind) {
            current_kind = Some(path.kind);
            let _ = writeln!(w.out, ";TYPE:{}", label(path.kind));
        }
        let speed = slowed(path_speed(settings, path.kind, index), slow, settings);
        if let Some(from) = rise_from {
            // Z climbs in proportion to the length gone round.
            let mut corners: Vec<Point> = path.points.clone();
            corners.push(first);
            let total: f64 = corners.windows(2).map(|p| distance(p[0], p[1])).sum();
            let mut done = 0.0;
            for pair in corners.windows(2) {
                done += distance(pair[0], pair[1]);
                let z = from + (layer.z - from) * done / total.max(1e-9);
                w.extrude_rising(pair[1], z, filament_per_mm, speed, path.kind);
            }
            w.z = layer.z;
            continue;
        }
        for &point in &path.points[1..] {
            w.extrude(point, filament_per_mm, speed, path.kind);
        }
        if path.closed {
            w.extrude(first, filament_per_mm, speed, path.kind);
        }
    }
    Ok(())
}

/// The factor by which extrusion on `layer` slows down so that the layer
/// takes at least `min_layer_time`; 1 if it takes that long anyway or the
/// setting is off. The layer is tried out on copies of the writer: the
/// time spent extruding is stretched to fill the gap, and as acceleration
/// makes slower moves gain less than their speed says, the stretch is
/// repeated a few times.
// ponytail: up to four trial runs of the layer; `min_print_speed` can keep
// a layer shorter than asked.
fn slowdown(
    w: &Writer,
    carry: Carry,
    layer: &Layer,
    index: usize,
    context: &Context,
) -> Result<f64, GcodeError> {
    let settings = w.settings;
    let goal = settings.min_layer_time;
    if goal <= 0.0 {
        return Ok(1.0);
    }
    let mut slow = 1.0;
    for _ in 0..4 {
        let mut trial = w.fork();
        let mut carry = carry;
        write_layer(&mut trial, &mut carry, layer, index, context, slow)?;
        let times = move_times(&trial.moves, settings.acceleration, settings.jerk);
        let total: f64 = times.iter().sum();
        let extruding: f64 = trial
            .moves
            .iter()
            .zip(&times)
            .filter(|(m, _)| m.kind.is_some())
            .map(|(_, t)| t)
            .sum();
        if total >= goal * 0.99 || extruding <= 0.0 {
            break;
        }
        slow *= extruding / (goal - (total - extruding));
    }
    Ok(slow)
}

/// `speed` slowed by `slow`, but not below `min_print_speed` unless it was
/// slower already.
fn slowed(speed: f64, slow: f64, settings: &Settings) -> f64 {
    if slow >= 1.0 {
        speed
    } else {
        (speed * slow).max(settings.min_print_speed.min(speed))
    }
}

/// Converts toolpaths into a G-code file.
///
/// Extrusion is relative (`M83`) unless the settings ask for absolute
/// values (`M82`, reset with `G92 E0` on every layer). Every millimetre of path deposits a volume
/// of line width times layer height, so parallel lines one line width apart
/// fill a layer completely.
///
/// With more than one extruder (`extruder_count`) every path prints with its
/// own extruder. The first one is selected with `T<n>` before the first
/// move; at every later change the old extruder retracts, `toolchange_gcode`
/// follows, then `T<n>`, the travel to the next path and the move back to
/// the height of the layer (the custom G-code may have moved the nozzle).
pub fn write_gcode(toolpaths: &Toolpaths, settings: &Settings) -> Result<Output, GcodeError> {
    write_gcode_with_progress(toolpaths, settings, &Progress::new())
}

/// Speed in mm/s at which a path of `kind` is printed on layer `layer`
/// (counted from 0).
pub fn path_speed(settings: &Settings, kind: PathKind, layer: usize) -> f64 {
    if layer == 0 {
        return settings.first_layer_speed;
    }
    match kind {
        PathKind::OuterWall => settings.outer_wall_speed,
        PathKind::InnerWall | PathKind::GapFill => settings.inner_wall_speed,
        PathKind::SolidInfill
        | PathKind::SparseInfill
        | PathKind::Support
        | PathKind::PrimeTower => settings.infill_speed,
        PathKind::Bridge => settings.bridge_speed,
        PathKind::Ironing => settings.ironing_speed,
        PathKind::Skirt | PathKind::Brim => settings.first_layer_speed,
    }
}

/// Like [`write_gcode`], and reports how far it has got in `progress`,
/// which is finished at the end.
pub fn write_gcode_with_progress(
    toolpaths: &Toolpaths,
    settings: &Settings,
    progress: &Progress,
) -> Result<Output, GcodeError> {
    // One part per layer and one for the time estimate.
    progress.start(Step::Gcode, toolpaths.layers.len() + 1);
    let values = settings.placeholder_values();
    let start = expand(&settings.start_gcode, "start_gcode", &values)?;
    let end = expand(&settings.end_gcode, "end_gcode", &values)?;
    let several = settings.extruder_count > 1;
    if several {
        // Mistakes in the tool change G-code show up even without a change.
        let check = toolchange_values(&values, 0, 1);
        expand(&settings.toolchange_gcode, "toolchange_gcode", &check)?;
    }
    let filament_area = PI * (settings.filament_diameter / 2.0).powi(2);

    let mut w = Writer {
        settings,
        out: String::new(),
        position: None,
        z: 0.0,
        feed: 0.0,
        tool: 0,
        retracted: vec![false],
        filament: vec![0.0],
        e: 0.0,
        moves: Vec::new(),
        nozzle_z: None,
    };
    w.out.push_str(&start);
    let absolute = settings.extrusion_mode == ExtrusionMode::Absolute;
    w.out.push_str(if absolute {
        "G21\nG90\nM82\n"
    } else {
        "G21\nG90\nM83\n"
    });

    if settings.pressure_advance > 0.0 {
        let _ = match settings.flavor {
            Flavor::Marlin => writeln!(w.out, "M900 K{}", settings.pressure_advance),
            Flavor::Klipper => writeln!(
                w.out,
                "SET_PRESSURE_ADVANCE ADVANCE={}",
                settings.pressure_advance
            ),
        };
    }

    let context = Context {
        values: &values,
        filament_area,
        several,
        absolute,
    };
    let mut carry = Carry::default();
    for (index, layer) in toolpaths.layers.iter().enumerate() {
        let slow = slowdown(&w, carry, layer, index, &context)?;
        write_layer(&mut w, &mut carry, layer, index, &context, slow)?;
        progress.advance();
    }
    w.retract();
    w.out.push_str(&end);

    // The final retraction of every extruder is not consumed filament.
    let used: Vec<f64> = w
        .filament
        .iter()
        .zip(&w.retracted)
        .map(|(&filament, &retracted)| {
            filament
                + if retracted {
                    settings.retraction_length
                } else {
                    0.0
                }
        })
        .collect();
    let times = move_times(&w.moves, settings.acceleration, settings.jerk);
    let volume = |length: f64| length * filament_area / 1000.0;
    let extruders = if several {
        (0..used.len())
            .filter(|&tool| w.moves.iter().any(|m| m.tool == tool))
            .map(|tool| ExtruderStats {
                extruder: tool as u32,
                filament_length_mm: used[tool],
                filament_volume_cm3: volume(used[tool]),
                filament_weight_g: volume(used[tool]) * settings.filament_density,
                print_time_s: (w.moves.iter().zip(&times))
                    .filter(|(m, _)| m.tool == tool)
                    .map(|(_, t)| t)
                    .sum(),
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut by_kind: Vec<(Option<PathKind>, f64)> = Vec::new();
    for (m, &time) in w.moves.iter().zip(&times) {
        match by_kind.iter_mut().find(|(kind, _)| *kind == m.kind) {
            Some((_, sum)) => *sum += time,
            None => by_kind.push((m.kind, time)),
        }
    }
    let total: f64 = used.iter().sum();
    let stats = Stats {
        layers: toolpaths.layers.len(),
        filament_length_mm: total,
        filament_volume_cm3: volume(total),
        filament_weight_g: volume(total) * settings.filament_density,
        print_time_s: times.iter().sum(),
        extruders,
        times: by_kind,
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
    for e in &stats.extruders {
        let _ = writeln!(
            gcode,
            "; extruder T{}: filament {:.2} mm, {:.2} g, {:.0} s",
            e.extruder, e.filament_length_mm, e.filament_weight_g, e.print_time_s
        );
    }
    gcode.push_str(&w.out);
    gcode.push_str("; settings\n");
    for (key, value) in settings.placeholder_values() {
        let _ = writeln!(gcode, "; {key} = {value}");
    }
    progress.finish();
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

    #[test]
    fn times_per_line_type_add_up() {
        let output = slice(&Mesh::cuboid(20.0, 20.0, 4.0), &Settings::default());
        let stats = &output.stats;
        let sum: f64 = stats.times.iter().map(|(_, t)| t).sum();
        assert!((sum - stats.print_time_s).abs() < 1e-6, "{sum}");
        let kinds: Vec<Option<PathKind>> = stats.times.iter().map(|(k, _)| *k).collect();
        for kind in [
            None,
            Some(PathKind::Skirt),
            Some(PathKind::OuterWall),
            Some(PathKind::SparseInfill),
        ] {
            assert!(kinds.contains(&kind), "{kind:?} in {kinds:?}");
        }
        assert!(stats.times.iter().all(|(_, t)| *t > 0.0));
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
    fn progress_grows_to_the_end_and_leaves_the_output_alone() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let mesh = Mesh::cuboid(20.0, 20.0, 30.0);
        let mut settings = Settings::default();
        settings.layer_height = 0.1;
        let progress = Progress::new();
        let finished = AtomicBool::new(false);
        let (output, samples) = std::thread::scope(|scope| {
            let watcher = scope.spawn(|| {
                let mut samples = vec![progress.fraction()];
                while !finished.load(Ordering::Relaxed) {
                    samples.push(progress.fraction());
                    std::thread::yield_now();
                }
                samples.push(progress.fraction());
                samples
            });
            let stop = AtomicBool::new(false);
            let mut warnings = slicer_core::Diagnostics::new();
            let toolpaths = slicer_fdm::generate_with_progress(
                &mesh,
                &settings,
                &mut warnings,
                &stop,
                &progress,
            )
            .unwrap();
            assert_eq!(progress.step(), Step::Ordering);
            assert_eq!(progress.fraction(), 0.91);
            let output = write_gcode_with_progress(&toolpaths, &settings, &progress).unwrap();
            finished.store(true, Ordering::Relaxed);
            (output, watcher.join().unwrap())
        });
        assert!(samples.windows(2).all(|w| w[0] <= w[1]), "{samples:?}");
        assert_eq!(samples.last(), Some(&1.0));
        assert!(samples.iter().any(|&s| s > 0.0 && s < 1.0));
        assert_eq!(progress.step(), Step::Done);
        assert_eq!(output, slice(&mesh, &settings));
    }

    #[test]
    fn pressure_advance_is_set_for_the_firmware() {
        let mesh = Mesh::cuboid(10.0, 10.0, 2.0);
        let set = Settings {
            pressure_advance: 0.04,
            ..Settings::default()
        };
        assert!(slice(&mesh, &set).gcode.contains(
            "
M83
M900 K0.04
"
        ));
        let klipper = Settings {
            flavor: Flavor::Klipper,
            ..set
        };
        let gcode = slice(&mesh, &klipper).gcode;
        assert!(gcode.contains(
            "
SET_PRESSURE_ADVANCE ADVANCE=0.04
"
        ));
        let off = slice(&mesh, &Settings::default()).gcode;
        assert!(!off.contains("M900") && !off.contains("PRESSURE_ADVANCE"));
    }

    #[test]
    fn small_layers_slow_down_to_the_minimum_layer_time() {
        let mesh = Mesh::cuboid(15.0, 15.0, 3.0);
        let fast = slice(&mesh, &Settings::default());
        let set = Settings {
            min_layer_time: 25.0,
            ..Settings::default()
        };
        let slow = slice(&mesh, &set);
        let layers = slow.stats.layers as f64;
        let per_layer = fast.stats.print_time_s / layers;
        assert!(per_layer < 25.0 * 0.6, "{per_layer}");
        // Close to 25 seconds a layer; acceleration makes it inexact.
        let time = slow.stats.print_time_s;
        assert!(
            time > layers * 25.0 * 0.85 && time < layers * 25.0 * 1.2,
            "{time}"
        );
        // The same material, and extrusion never slower than the minimum.
        assert!((slow.stats.filament_length_mm - fast.stats.filament_length_mm).abs() < 1e-6);
        let mut feed = 0.0;
        for line in slow.gcode.lines() {
            if let Some(f) = word(line, 'F') {
                feed = f;
            }
            if line.starts_with("G1 X") && word(line, 'E').is_some_and(|e| e > 0.0) {
                assert!(feed >= set.min_print_speed * 60.0 - 1e-9, "{line}");
            }
        }
        // A floor of 30 mm/s keeps the layers shorter.
        let floor = Settings {
            min_print_speed: 35.0,
            ..set
        };
        assert!(slice(&mesh, &floor).stats.print_time_s < time);
    }

    #[test]
    fn vase_mode_rises_in_one_line() {
        let mesh = Mesh::cuboid(20.0, 20.0, 5.0);
        let settings = Settings {
            spiral_vase: true,
            ..Settings::default()
        };
        let toolpaths = slicer_fdm::generate(&mesh, &settings).unwrap();
        let bottom = settings.bottom_layers as usize;
        for layer in &toolpaths.layers[bottom..] {
            assert!(layer.spiral);
            assert_eq!(layer.paths.len(), 1, "one wall, nothing inside");
        }
        assert!(!toolpaths.layers[bottom - 1].spiral);
        let gcode = write_gcode(&toolpaths, &settings).unwrap().gcode;
        // From the first spiral layer on, no move of Z alone, and Z never
        // goes down while extruding up to the top.
        let start = gcode
            .split(";LAYER_CHANGE")
            .take(bottom + 1)
            .map(|part| part.len() + ";LAYER_CHANGE".len())
            .sum::<usize>()
            - ";LAYER_CHANGE".len();
        let spiral = &gcode[start..gcode.rfind(settings.end_gcode.as_str()).unwrap()];
        let mut z = 0.0;
        let mut rising = 0;
        for line in spiral.lines() {
            if line.starts_with("G1 Z") {
                panic!("separate Z move in vase mode: {line}");
            }
            if let (true, Some(next)) = (line.starts_with("G1 X"), word(line, 'Z')) {
                assert!(next >= z - 1e-9, "{line}");
                z = next;
                rising += 1;
            }
        }
        // Four sides a layer, each climbing on its way.
        assert_eq!(rising, 4 * (toolpaths.layers.len() - bottom));
        assert!((z - 5.0).abs() < 1e-6, "{z}");
    }

    #[test]
    fn top_surfaces_are_ironed_with_little_material() {
        // A step: the lower block's top shows beside the upper one.
        let mut mesh = Mesh::cuboid(30.0, 20.0, 2.0);
        let mut upper = Mesh::cuboid(10.0, 20.0, 2.0);
        upper.translate([0.0, 0.0, 2.0]);
        mesh.merge(&upper);
        let settings = Settings {
            ironing: true,
            ..Settings::default()
        };
        let toolpaths = slicer_fdm::generate(&mesh, &settings).unwrap();
        let ironed: Vec<usize> = (0..toolpaths.layers.len())
            .filter(|&i| {
                toolpaths.layers[i]
                    .paths
                    .iter()
                    .any(|p| p.kind == PathKind::Ironing)
            })
            .collect();
        // The top of the step and the top of the tower, nothing else.
        assert_eq!(ironed.len(), 2, "{ironed:?}");
        assert_eq!(*ironed.last().unwrap(), toolpaths.layers.len() - 1);
        let gcode = write_gcode(&toolpaths, &settings).unwrap().gcode;
        let filament_area = PI * (settings.filament_diameter / 2.0).powi(2);
        let full = settings.line_width * settings.layer_height / filament_area;
        let mut ironing = false;
        let (mut checked, mut feed) = (0, 0.0);
        for line in gcode.lines() {
            if let Some(kind) = line.strip_prefix(";TYPE:") {
                ironing = kind == "Ironing";
            }
            if let Some(f) = word(line, 'F') {
                feed = f;
            }
            let (Some(_), Some(e)) = (word(line, 'X'), word(line, 'E')) else {
                continue;
            };
            if ironing && line.starts_with("G1") && e > 0.0 {
                assert_eq!(feed, settings.ironing_speed * 60.0, "{line}");
                checked += 1;
            }
        }
        assert!(checked > 10);
        let without = slicer_fdm::generate(
            &mesh,
            &Settings {
                ironing: false,
                ..settings.clone()
            },
        )
        .unwrap();
        let with_gcode = write_gcode(&toolpaths, &settings).unwrap();
        let without_gcode = write_gcode(&without, &settings).unwrap();
        // A tenth of the material of a normal line, per millimetre.
        let extra = with_gcode.stats.filament_length_mm - without_gcode.stats.filament_length_mm;
        let ironed_length: f64 = ironed
            .iter()
            .flat_map(|&i| &toolpaths.layers[i].paths)
            .filter(|p| p.kind == PathKind::Ironing)
            .map(slicer_fdm::Path::length)
            .sum();
        assert!((extra - ironed_length * full * 0.1).abs() < 1e-3, "{extra}");
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
    fn bridges_get_their_own_fan_and_flow() {
        // A deck on two pillars.
        let mut mesh = Mesh::cuboid(5.0, 20.0, 5.0);
        let mut pillar = Mesh::cuboid(5.0, 20.0, 5.0);
        pillar.translate([25.0, 0.0, 0.0]);
        mesh.merge(&pillar);
        let mut deck = Mesh::cuboid(30.0, 20.0, 2.0);
        deck.translate([0.0, 0.0, 5.0]);
        mesh.merge(&deck);

        let mut settings = Settings::default();
        let toolpaths = slicer_fdm::generate(&mesh, &settings).unwrap();
        let plain = write_gcode(&toolpaths, &settings).unwrap();
        assert!(plain.gcode.contains(";TYPE:Bridge\n"));
        assert_eq!(plain.gcode.matches("M106").count(), 1);

        settings.fan_speed = 40.0;
        settings.bridge_fan_speed = 100.0;
        settings.bridge_flow = 0.8;
        let output = write_gcode(&toolpaths, &settings).unwrap();
        // Up for the bridge, back down after it.
        let up = output.gcode.find("M106 S255\n").unwrap();
        let bridge = output.gcode.find(";TYPE:Bridge\n").unwrap();
        let down = up + output.gcode[up..].find("M106 S102\n").unwrap();
        assert!(up < bridge && bridge < down);
        assert_eq!(output.gcode.matches("M106").count(), 3);
        assert!(output.stats.filament_length_mm < plain.stats.filament_length_mm - 1.0);
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
            tool: 0,
            kind: None,
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
    fn lines_of_their_own_width_extrude_in_proportion() {
        let mut settings = Settings::default();
        settings.retraction_length = 0.0;
        let mut toolpaths =
            slicer_fdm::generate(&Mesh::cuboid(10.0, 10.0, 1.0), &settings).unwrap();
        let full = write_gcode(&toolpaths, &settings).unwrap();
        for path in toolpaths.layers.iter_mut().flat_map(|l| &mut l.paths) {
            path.width = Some(settings.line_width / 4.0);
        }
        let quarter = write_gcode(&toolpaths, &settings).unwrap();
        let ratio = quarter.stats.filament_length_mm / full.stats.filament_length_mm;
        assert!((ratio - 0.25).abs() < 1e-3, "{ratio}");
    }

    #[test]
    fn thin_fin_extrudes_its_volume() {
        // 0.3 mm thick, narrower than a line: one line of its own width.
        let mut settings = Settings::default();
        settings.skirt_loops = 0;
        let output = slice(&Mesh::cuboid(20.0, 0.3, 5.0), &settings);
        let ratio = output.stats.filament_volume_cm3 * 1000.0 / (20.0 * 0.3 * 5.0);
        assert!((ratio - 1.0).abs() < 0.25, "{ratio}");
        assert!(output.gcode.contains(";TYPE:Outer wall\n"));
    }

    #[test]
    fn output_is_reproducible() {
        let settings = Settings::default();
        let mesh = Mesh::cuboid(12.0, 17.0, 3.0);
        assert_eq!(slice(&mesh, &settings).gcode, slice(&mesh, &settings).gcode);
    }

    #[test]
    fn absolute_extrusion_adds_up_to_the_same_filament() {
        let mesh = Mesh::cuboid(10.0, 10.0, 1.0);
        let relative = slice(&mesh, &Settings::default());
        let mut settings = Settings::default();
        settings.extrusion_mode = ExtrusionMode::Absolute;
        let absolute = slice(&mesh, &settings);
        assert!(relative.gcode.contains("\nM83\n") && !relative.gcode.contains("G92 E0"));
        assert!(absolute.gcode.contains("\nM82\n"));
        assert_eq!(
            absolute.gcode.matches("G92 E0").count(),
            absolute.stats.layers
        );
        assert_eq!(
            absolute.stats.filament_length_mm,
            relative.stats.filament_length_mm
        );

        // Replay both files: the filament pushed through must be the same.
        let pushed = |gcode: &str, absolute: bool| {
            let (mut total, mut last) = (0.0, 0.0);
            for line in gcode.lines() {
                if line == "G92 E0" {
                    last = 0.0;
                } else if let Some(e) = word(line, 'E').filter(|_| line.starts_with("G1")) {
                    total += if absolute { e - last } else { e };
                    last = e;
                }
            }
            total
        };
        let (a, r) = (
            pushed(&absolute.gcode, true),
            pushed(&relative.gcode, false),
        );
        assert!((a - r).abs() < 1e-3, "{a} {r}");
        // Within a layer the value never jumps back by more than a retraction.
        let mut last = 0.0;
        for line in absolute.gcode.lines() {
            if line == "G92 E0" {
                last = 0.0;
            } else if let Some(e) = word(line, 'E').filter(|_| line.starts_with("G1")) {
                assert!(e >= last - settings.retraction_length - 1e-9, "{line}");
                last = e;
            }
        }
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

    /// Two 10 x 10 x 5 mm cubes 10 mm apart, the second one on extruder 2.
    fn two_cubes() -> Mesh {
        let mut mesh = Mesh::cuboid(10.0, 10.0, 5.0);
        let mut other = Mesh::cuboid(10.0, 10.0, 5.0);
        other.translate([20.0, 0.0, 0.0]);
        other.set_extruder(0, 1);
        mesh.merge(&other);
        mesh
    }

    #[test]
    fn two_extruders_change_tools_once_per_layer() {
        let mut settings = Settings::default();
        settings.extruder_count = 2;
        settings.toolchange_gcode = "; change {previous_extruder} to {next_extruder}".into();
        // Solid cubes without skirt and tower, so the filament of every
        // extruder is the volume of its cube.
        settings.infill_density = 100.0;
        settings.infill_overlap = 0.0;
        settings.skirt_loops = 0;
        settings.prime_tower_width = 0.0;
        let output = slice(&two_cubes(), &settings);
        let lines: Vec<&str> = output.gcode.lines().collect();
        let changes: Vec<(usize, &str)> = (lines.iter().enumerate())
            .filter(|(_, l)| l.starts_with('T'))
            .map(|(i, l)| (i, *l))
            .collect();
        // T0 to start with, then one change on each of the 25 layers.
        assert_eq!(changes.len(), 26);
        for (k, &(at, tool)) in changes.iter().enumerate() {
            assert_eq!(tool, if k % 2 == 0 { "T0" } else { "T1" });
            if k > 0 {
                let previous = if k % 2 == 0 { 1 } else { 0 };
                let block = format!("; change {previous} to {}", 1 - previous);
                assert_eq!(lines[at - 1], block);
                // Retracted before, travel to the next path after, then
                // back to the layer.
                assert!(lines[at - 2].starts_with("G1 E-0.8"), "{}", lines[at - 2]);
                assert!(lines[at + 1].starts_with("G0 X"), "{}", lines[at + 1]);
                assert!(lines[at + 2].starts_with("G1 Z"), "{}", lines[at + 2]);
            }
        }
        // Every layer holds exactly one change; the first one also selects
        // the first extruder.
        let layers: Vec<&str> = output.gcode.split(";LAYER_CHANGE").skip(1).collect();
        assert_eq!(layers.len(), 25);
        assert_eq!(layers[0].matches("\nT").count(), 2);
        assert!(layers[1..].iter().all(|l| l.matches("\nT").count() == 1));

        // Filament and time per extruder.
        let [first, second] = output.stats.extruders.as_slice() else {
            panic!("{:?}", output.stats.extruders)
        };
        assert_eq!((first.extruder, second.extruder), (0, 1));
        for e in [first, second] {
            let ratio = e.filament_volume_cm3 / 0.5;
            assert!((ratio - 1.0).abs() < 0.03, "T{}: {ratio}", e.extruder);
        }
        let sum = first.filament_length_mm + second.filament_length_mm;
        assert!((sum - output.stats.filament_length_mm).abs() < 1e-6);
        let time = first.print_time_s + second.print_time_s;
        assert!((time - output.stats.print_time_s).abs() < 1e-6);
        assert!(output.gcode.contains("\n; extruder T1: filament "));

        // The prime tower adds its volume on every layer, shared by both.
        settings.prime_tower_width = 15.0;
        let tower = slice(&two_cubes(), &settings);
        let added = tower.stats.filament_volume_cm3 - output.stats.filament_volume_cm3;
        let expected = 15.0 * 15.0 * 5.0 / 1000.0;
        assert!((added / expected - 1.0).abs() < 0.1, "{added} {expected}");
        assert!(tower.gcode.contains(";TYPE:Prime tower\n"));
        for e in &tower.stats.extruders {
            assert!(e.filament_volume_cm3 > 0.5 + 0.3 * expected, "{e:?}");
        }
    }

    #[test]
    fn one_extruder_ignores_the_extruders_of_the_model() {
        let settings = Settings::default();
        let mut plain = two_cubes();
        plain.extruders.clear();
        let output = slice(&two_cubes(), &settings);
        assert_eq!(output.gcode, slice(&plain, &settings).gcode);
        assert!(!output.gcode.lines().any(|l| l.starts_with('T')));
        assert!(output.stats.extruders.is_empty());
    }

    #[test]
    fn tool_changes_restart_absolute_extrusion_and_check_placeholders() {
        let mut settings = Settings::default();
        settings.extruder_count = 2;
        settings.extrusion_mode = ExtrusionMode::Absolute;
        let toolpaths = slicer_fdm::generate(&two_cubes(), &settings).unwrap();
        let gcode = write_gcode(&toolpaths, &settings).unwrap().gcode;
        assert!(gcode.contains("\nT1\nG92 E0\n"));
        assert!(gcode.contains("\nT0\nG92 E0\n"));

        // Mistakes show up even where no tool changes.
        settings.toolchange_gcode = "M117 {next_extruder}\nM117 {next_extrudr}".into();
        let one = slicer_fdm::generate(&Mesh::cuboid(5.0, 5.0, 1.0), &settings).unwrap();
        assert_eq!(
            write_gcode(&one, &settings).unwrap_err().to_string(),
            "toolchange_gcode, line 2: unknown placeholder '{next_extrudr}'"
        );
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
