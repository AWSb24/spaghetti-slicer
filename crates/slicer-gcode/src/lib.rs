//! G-code generation with exchangeable flavors (Bambu, Marlin, Klipper).
//!
//! Marlin and Klipper are implemented and currently produce the same
//! commands; the Bambu flavor does not exist yet.

pub mod calibration;
mod estimate;
pub mod thumbnails;

use std::collections::BTreeMap;
use std::f64::consts::PI;
use std::fmt::{self, Write};

use serde::Serialize;
use slicer_core::Point;
use slicer_core::polygon::{convex_hull, distance};
use slicer_fdm::{Layer, NO_OBJECT, Path, PathKind, Progress, Step, Toolpaths};
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
    /// Estimated print time in seconds: the moves of the layers with
    /// acceleration, slowing down at corners and the limits of the axes
    /// (`times`), the start and end G-code (`time_start_end_s`), the tool
    /// change G-code (`time_toolchange_s`) and the waits for heaters
    /// (`time_heating_s`). The G-code header and the `prediction` of a
    /// `.gcode.3mf` give this figure. Not yet compared with a real print.
    pub print_time_s: f64,
    /// Of `print_time_s`, the seconds the moves, dwells (`G4`) and homing
    /// (`G28`, `homing_time`) of the start and end G-code take.
    pub time_start_end_s: f64,
    /// Of `print_time_s`, the seconds spent waiting for the nozzle and the
    /// bed to reach their temperatures (`M109`, `M190`, `M116`), in the
    /// start and end G-code, the tool change G-code and the layers; 0
    /// without `nozzle_heat_rate` and `bed_heat_rate`.
    pub time_heating_s: f64,
    /// Of `print_time_s`, the seconds the moves and dwells (`G4`) of the
    /// tool change G-code take, all tool changes together. 0, and left out
    /// of JSON, without them.
    #[serde(skip_serializing_if = "is_zero")]
    pub time_toolchange_s: f64,
    /// The same figures for every extruder the print uses, with more than
    /// one extruder (`extruder_count`); empty otherwise.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extruders: Vec<ExtruderStats>,
    /// Filament extruded into the prime tower, in cm³: the purges of the
    /// tool changes (`flush_volumes`), already part of the figures above.
    /// 0, and left out of JSON, without a tower.
    #[serde(skip_serializing_if = "is_zero")]
    pub flush_volume_cm3: f64,
    /// The same in grams.
    #[serde(skip_serializing_if = "is_zero")]
    pub flush_weight_g: f64,
    /// Estimated seconds per line type, in the order the types first
    /// appear; travel, retraction and Z moves are under `None`. They add up
    /// to `print_time_s` without `time_start_end_s`, `time_toolchange_s`
    /// and `time_heating_s`.
    /// Not written to the file or to JSON.
    #[serde(skip)]
    pub times: Vec<(Option<PathKind>, f64)>,
    /// Which extruders the layers use, as runs of layers with the same
    /// ones, for the layer list of a `.gcode.3mf`
    /// ([`slicer_formats::PrintInfo::layer_filaments`]). Not written to the
    /// file or to JSON.
    #[serde(skip)]
    pub layer_filaments: Vec<slicer_formats::LayerFilaments>,
}

/// Whether a figure is 0, to leave it out of JSON.
fn is_zero(value: &f64) -> bool {
    *value == 0.0
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

/// The filaments of a print for the description files of a `.gcode.3mf`
/// container ([`slicer_formats::PrintInfo::filaments`]): one per extruder
/// the print uses (with more than one extruder, from
/// [`Stats::extruders`]), otherwise the one of the first extruder with
/// all the filament. Each has its kind from `filament_type` and its colour
/// from `filament_colours`; without one, `PLA` and white `#FFFFFF`, as
/// the container wrote before these settings existed.
pub fn filaments(stats: &Stats, settings: &Settings) -> Vec<slicer_formats::Filament> {
    // Settings are checked when they are read.
    let colours = settings.filament_colours().unwrap_or_default();
    let types = settings.filament_types().unwrap_or_default();
    let filament = |extruder: u32, length_mm: f64, weight_g: f64| {
        let index = extruder as usize;
        slicer_formats::Filament {
            id: extruder + 1,
            kind: types.get(index).copied().unwrap_or("PLA").to_owned(),
            colour: colours
                .get(index)
                .map_or_else(|| "#FFFFFF".to_owned(), |&c| slicer_profiles::colour_hex(c)),
            used_m: length_mm / 1000.0,
            used_g: weight_g,
        }
    };
    if stats.extruders.is_empty() {
        return vec![filament(
            0,
            stats.filament_length_mm,
            stats.filament_weight_g,
        )];
    }
    stats
        .extruders
        .iter()
        .map(|e| filament(e.extruder, e.filament_length_mm, e.filament_weight_g))
        .collect()
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
        PathKind::SupportInterface => "Support interface",
        PathKind::OuterWall => "Outer wall",
        PathKind::InnerWall => "Inner wall",
        PathKind::GapFill => "Gap infill",
        PathKind::SolidInfill => "Solid infill",
        PathKind::TopSurface => "Top surface",
        PathKind::BottomSurface => "Bottom surface",
        PathKind::Bridge => "Bridge",
        PathKind::SparseInfill => "Sparse infill",
        PathKind::PrimeTower => "Prime tower",
        PathKind::Ironing => "Ironing",
        PathKind::OverhangWall | PathKind::OverhangWallFull => "Overhang wall",
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
    /// How far X, Y, Z and the filament move per millimetre of `length`,
    /// with their signs: for the limits per axis and Klipper's corners.
    axes: [f64; 4],
    /// Distance in millimetres.
    length: f64,
    /// Requested speed in mm/s.
    speed: f64,
    /// Extruder selected during the move (its slot in the writer).
    tool: usize,
    /// What is extruded; `None` for travel, retraction and Z moves.
    kind: Option<PathKind>,
}

/// How the printer takes corners, for the time estimate.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Corners {
    /// The velocity changes at once by at most this much (mm/s): `jerk`.
    Jerk(f64),
    /// The printer goes round a circle that touches both moves and passes
    /// this far (mm) from the corner, as fast as the acceleration allows:
    /// Marlin's junction deviation and Klipper's square corner velocity.
    Deviation(f64),
}

/// How the printer moves, as far as the time estimate needs it: the
/// acceleration, the limits per axis and the corners, from the settings
/// and the limit commands of the start G-code
/// ([`Machine::limit_command`], [`Machine::set_velocity_limit`]).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Machine {
    /// Acceleration of moves that extrude, in mm/s²: `acceleration`, at most
    /// `max_accel_extruding`.
    extruding: f64,
    /// Acceleration of all other moves: `acceleration`, at most
    /// `max_accel_travel`.
    travel: f64,
    /// Highest speed of the nozzle in mm/s (Klipper's `max_velocity`);
    /// infinite without a limit.
    velocity: f64,
    /// Highest speed of X, Y, Z and the filament in mm/s; infinite without
    /// a limit.
    max_speed: [f64; 4],
    /// Highest acceleration of X, Y, Z and the filament in mm/s².
    max_accel: [f64; 4],
    /// Klipper: one acceleration for all moves, the corners follow its
    /// square corner velocity, and the limits of the filament hold only for
    /// moves of the filament alone (`max_extrude_only_velocity` and
    /// `_accel`).
    klipper: bool,
    /// Klipper's square corner velocity in mm/s.
    square_corner: f64,
    corners: Corners,
    /// The share of the acceleration a move may use to reach its top speed
    /// without cruising: 1 - `minimum_cruise_ratio` for Klipper, else 1.
    reach: f64,
}

/// Klipper's corners with the square corner velocity `v` (mm/s) at the
/// acceleration `accel` (mm/s²). The square corner velocity is the speed of
/// a 90° corner; with v² = a * r ([`Machine::corner`]) and
/// r = d * sin(45°) / (1 - sin(45°)) = d / (sqrt(2) - 1), that is a
/// deviation d = v² * (sqrt(2) - 1) / a.
fn klipper_corners(v: f64, accel: f64) -> Corners {
    Corners::Deviation(v * v * (2.0_f64.sqrt() - 1.0) / accel)
}

impl Machine {
    /// The printer the settings describe.
    fn of(settings: &Settings) -> Self {
        let limit = |value: f64| if value > 0.0 { value } else { f64::INFINITY };
        let klipper = settings.flavor == Flavor::Klipper;
        let mut extruding = settings
            .acceleration
            .min(limit(settings.max_accel_extruding));
        let mut travel = settings.acceleration.min(limit(settings.max_accel_travel));
        let mut velocity = f64::INFINITY;
        let corners = if klipper {
            // Klipper has one acceleration and one speed limit of the
            // nozzle, whatever the direction.
            extruding = extruding.min(travel);
            travel = extruding;
            velocity = limit(settings.max_speed_x).min(limit(settings.max_speed_y));
            klipper_corners(settings.square_corner_velocity, extruding)
        } else if settings.junction_deviation > 0.0 {
            Corners::Deviation(settings.junction_deviation)
        } else {
            Corners::Jerk(settings.jerk)
        };
        Self {
            extruding,
            travel,
            velocity,
            klipper,
            square_corner: settings.square_corner_velocity,
            max_speed: [
                settings.max_speed_x,
                settings.max_speed_y,
                settings.max_speed_z,
                settings.max_speed_e,
            ]
            .map(limit),
            max_accel: [
                settings.max_accel_x,
                settings.max_accel_y,
                settings.max_accel_z,
                settings.max_accel_e,
            ]
            .map(limit),
            corners,
            reach: if klipper {
                1.0 - settings.minimum_cruise_ratio
            } else {
                1.0
            },
        }
    }

    /// Klipper's acceleration `accel` (mm/s²) for all moves; the corners
    /// follow it.
    fn set_klipper_accel(&mut self, accel: f64) {
        self.extruding = accel;
        self.travel = accel;
        self.corners = klipper_corners(self.square_corner, accel);
    }

    /// Applies `M<code>` with the numbers of its words (`value`) as the
    /// firmware would, as the Marlin documentation describes them: `M201`
    /// sets the highest acceleration and `M203` the highest speed of X, Y,
    /// Z and E; `M204` the acceleration of printing (`P`), travel (`T`) or
    /// both (`S`); `M205` the junction deviation (`J`) or the jerk (`X`,
    /// `Y`). Klipper knows only `M204`: `S`, or the smaller of `P` and `T`
    /// when both are given. Other codes and values of 0 or less change
    /// nothing.
    // ponytail: `M204 R` (retraction), the jerk of Z and E (`M205 Z`/`E`)
    // and the lowest feed rates (`M205 S`/`T`) are not modelled; `M205 X`
    // and `Y` set one jerk, the smaller, and only on a printer whose
    // corners go by jerk (`junction_deviation` = 0), as on Marlin with
    // classic jerk; `M205 J` switches to junction deviation.
    fn limit_command(&mut self, code: u32, value: &dyn Fn(char) -> Option<f64>) {
        let given = |letter: char| value(letter).filter(|&v| v > 0.0);
        let axes = ['X', 'Y', 'Z', 'E'];
        match code {
            201 | 203 if !self.klipper => {
                let limits = if code == 201 {
                    &mut self.max_accel
                } else {
                    &mut self.max_speed
                };
                for (limit, &axis) in limits.iter_mut().zip(&axes) {
                    if let Some(v) = given(axis) {
                        *limit = v;
                    }
                }
            }
            204 if self.klipper => {
                let both = given('P').zip(given('T')).map(|(p, t)| p.min(t));
                if let Some(accel) = given('S').or(both) {
                    self.set_klipper_accel(accel);
                }
            }
            204 => {
                if let Some(s) = given('S') {
                    self.extruding = s;
                    self.travel = s;
                }
                if let Some(p) = given('P') {
                    self.extruding = p;
                }
                if let Some(t) = given('T') {
                    self.travel = t;
                }
            }
            205 if !self.klipper => {
                if let Some(j) = given('J') {
                    self.corners = Corners::Deviation(j);
                } else if matches!(self.corners, Corners::Jerk(_)) {
                    let jerk = [given('X'), given('Y')].into_iter().flatten();
                    let jerk = jerk.fold(f64::INFINITY, f64::min);
                    if jerk.is_finite() {
                        self.corners = Corners::Jerk(jerk);
                    }
                }
            }
            _ => {}
        }
    }

    /// Applies Klipper's `SET_VELOCITY_LIMIT` with the numbers of its
    /// parameters (`value`), as the Klipper documentation describes them:
    /// `VELOCITY` and `ACCEL` of the nozzle, `SQUARE_CORNER_VELOCITY` and
    /// `MINIMUM_CRUISE_RATIO`. Does nothing on Marlin.
    fn set_velocity_limit(&mut self, value: &dyn Fn(&str) -> Option<f64>) {
        if !self.klipper {
            return;
        }
        if let Some(v) = value("VELOCITY").filter(|&v| v > 0.0) {
            self.velocity = v;
        }
        if let Some(v) = value("SQUARE_CORNER_VELOCITY").filter(|&v| v >= 0.0) {
            self.square_corner = v;
        }
        if let Some(r) = value("MINIMUM_CRUISE_RATIO").filter(|r| (0.0..1.0).contains(r)) {
            self.reach = 1.0 - r;
        }
        let accel = value("ACCEL").filter(|&a| a > 0.0);
        self.set_klipper_accel(accel.unwrap_or(self.extruding));
    }

    /// The speed (mm/s) and acceleration (mm/s²) of move `m`: as asked and
    /// as `acceleration` says, slowed so that no axis exceeds its limits.
    /// An axis that moves `share` mm per mm of the move goes `share` times
    /// as fast as the move and accelerates `share` times as hard.
    // ponytail: limits per axis, not per motor (CoreXY and delta printers
    // limit their motors); a retraction accelerates by `max_accel_e` like
    // the filament of a printing move, without Marlin's own `M204 R`; jerk
    // is one figure for the whole velocity, not one per axis.
    fn limits(&self, m: &Move) -> (f64, f64) {
        let alone = m.axes[..3].iter().all(|&a| a == 0.0);
        let mut speed = if alone {
            m.speed
        } else {
            m.speed.min(self.velocity)
        };
        let mut accel = if m.kind.is_some() {
            self.extruding
        } else {
            self.travel
        };
        for (axis, &share) in m.axes.iter().enumerate() {
            let share = share.abs();
            if share <= 0.0 || (axis == 3 && self.klipper && !alone) {
                continue;
            }
            speed = speed.min(self.max_speed[axis] / share);
            accel = accel.min(self.max_accel[axis] / share);
        }
        (speed, accel)
    }

    /// The highest speed (mm/s) where move `a` turns into move `b`, both
    /// accelerating at `accel` (mm/s²); infinite when the corner does not
    /// slow the printer.
    ///
    /// With a deviation d the printer is thought to go round the circle
    /// that touches both moves and passes d from the corner, at the speed
    /// whose centripetal acceleration v² / r is `accel`. With θ the angle
    /// between the two moves at the corner (180° for going straight on),
    /// the centre lies r / sin(θ/2) from the corner, so d = r / sin(θ/2) - r
    /// and r = d * sin(θ/2) / (1 - sin(θ/2)); sin(θ/2) = sqrt((1 + u·w) / 2)
    /// for the unit directions u of `a` and w of `b`.
    fn corner(&self, a: &Move, b: &Move, accel: f64) -> f64 {
        match self.corners {
            Corners::Jerk(jerk) => {
                let change = distance(a.direction, b.direction);
                if change > 1e-9 {
                    jerk / change
                } else {
                    f64::INFINITY
                }
            }
            Corners::Deviation(deviation) => {
                let (u, w) = (&a.axes, &b.axes);
                let lu = (u[0] * u[0] + u[1] * u[1] + u[2] * u[2]).sqrt();
                let lw = (w[0] * w[0] + w[1] * w[1] + w[2] * w[2]).sqrt();
                if lu <= 0.0 || lw <= 0.0 {
                    // ponytail: the head stands while the filament moves
                    // alone, so it comes to rest before and after.
                    return 0.0;
                }
                let dot = (u[0] * w[0] + u[1] * w[1] + u[2] * w[2]) / (lu * lw);
                let half = ((1.0 + dot) / 2.0).clamp(0.0, 1.0);
                if half >= 1.0 - 1e-12 {
                    return f64::INFINITY;
                }
                let sine = half.sqrt();
                (accel * deviation * sine / (1.0 - sine)).sqrt()
            }
        }
    }
}

/// The machine limits of the settings as commands for the start of the
/// G-code (`emit_machine_limits`). Marlin: `M201` and `M203` with the
/// limits per axis that are set, `M204` with the accelerations of printing
/// and travel and `M205` with `jerk` or `junction_deviation`. Klipper:
/// `SET_VELOCITY_LIMIT` with the speed limit of the nozzle (the smaller of
/// `max_speed_x` and `max_speed_y`, if set), the acceleration, the
/// `square_corner_velocity` and the `minimum_cruise_ratio`.
// ponytail: Klipper's limits of Z and of the filament alone stay those of
// `printer.cfg`; `SET_VELOCITY_LIMIT` cannot change them.
fn machine_limits(settings: &Settings) -> String {
    let machine = Machine::of(settings);
    if machine.klipper {
        let mut line = String::from("SET_VELOCITY_LIMIT");
        if machine.velocity.is_finite() {
            let _ = write!(line, " VELOCITY={}", machine.velocity);
        }
        return format!(
            "{line} ACCEL={} SQUARE_CORNER_VELOCITY={} MINIMUM_CRUISE_RATIO={} ; machine limits [mm/s, mm/s²]\n",
            machine.extruding, settings.square_corner_velocity, settings.minimum_cruise_ratio
        );
    }
    let words = |values: [f64; 4]| {
        let mut words = String::new();
        for (letter, value) in ['X', 'Y', 'Z', 'E'].into_iter().zip(values) {
            if value > 0.0 {
                let _ = write!(words, " {letter}{value}");
            }
        }
        words
    };
    let mut out = String::new();
    let accel = words([
        settings.max_accel_x,
        settings.max_accel_y,
        settings.max_accel_z,
        settings.max_accel_e,
    ]);
    if !accel.is_empty() {
        let _ = writeln!(out, "M201{accel} ; max acceleration per axis [mm/s²]");
    }
    let speed = words([
        settings.max_speed_x,
        settings.max_speed_y,
        settings.max_speed_z,
        settings.max_speed_e,
    ]);
    if !speed.is_empty() {
        let _ = writeln!(out, "M203{speed} ; max feed rate per axis [mm/s]");
    }
    let _ = writeln!(
        out,
        "M204 P{} T{} ; acceleration of printing and travel [mm/s²]",
        machine.extruding, machine.travel
    );
    let _ = match machine.corners {
        Corners::Deviation(deviation) => {
            writeln!(out, "M205 J{deviation} ; junction deviation [mm]")
        }
        Corners::Jerk(jerk) => writeln!(out, "M205 X{jerk} Y{jerk} ; jerk [mm/s]"),
    };
    out
}

/// Time in seconds the moves take on `machine` when the printer starts and
/// ends at rest.
#[cfg(test)]
fn planned_time(moves: &[Move], machine: &Machine) -> f64 {
    move_times(moves, machine).iter().sum()
}

/// Time in seconds every move takes on `machine`, starting and ending at
/// rest.
///
/// Every move is slowed to the limits of its axes ([`Machine::limits`]).
/// The speeds where moves meet are limited three ways: by the speeds of
/// the moves, by the corner ([`Machine::corner`]), and by what the
/// neighbouring moves can reach or shed over their length. Each move then
/// accelerates, cruises and brakes; with Klipper's `minimum_cruise_ratio`
/// a move reaches only the top speed it would with that share less of its
/// acceleration, which leaves at least that share of it for cruising.
fn move_times(moves: &[Move], machine: &Machine) -> Vec<f64> {
    let limits: Vec<(f64, f64)> = moves.iter().map(|m| machine.limits(m)).collect();
    let reachable =
        |from: f64, i: usize| (from * from + 2.0 * limits[i].1 * moves[i].length).sqrt();
    // junction[i] is the speed at the start of move i; both ends are at rest.
    let mut junction = vec![0.0; moves.len() + 1];
    for i in 1..moves.len() {
        let ((speed_a, accel_a), (speed_b, accel_b)) = (limits[i - 1], limits[i]);
        let corner = machine.corner(&moves[i - 1], &moves[i], accel_a.min(accel_b));
        junction[i] = speed_a.min(speed_b).min(corner);
    }
    for i in 0..moves.len() {
        junction[i + 1] = junction[i + 1].min(reachable(junction[i], i));
    }
    for i in (0..moves.len()).rev() {
        junction[i] = junction[i].min(reachable(junction[i + 1], i));
    }
    moves
        .iter()
        .zip(&limits)
        .enumerate()
        .map(|(i, (m, &(speed, accel)))| {
            let (start, end) = (junction[i], junction[i + 1]);
            // Highest speed when accelerating from `start` and braking to
            // `end` without cruising.
            let mut peak = ((2.0 * accel * m.length + start * start + end * end) / 2.0).sqrt();
            // ponytail: the lower top speed of short moves does not lower
            // the speeds where they meet their neighbours.
            if machine.reach < 1.0 {
                let reach = accel * machine.reach;
                let short = ((2.0 * reach * m.length + start * start + end * end) / 2.0).sqrt();
                peak = short.max(start).max(end);
            }
            let peak = peak.min(speed);
            let ramps = (2.0 * peak * peak - start * start - end * end) / (2.0 * accel);
            (2.0 * peak - start - end) / accel + (m.length - ramps).max(0.0) / peak
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
    /// How the next retraction, wipe and Z hop go: as the settings of the
    /// path printed last say.
    retraction: Retraction,
    /// Per extruder: how its filament was pulled back, if it is.
    retracted: Vec<Option<Retraction>>,
    /// Per extruder: filament pushed through so far, in millimetres.
    filament: Vec<f64>,
    /// Of that, what went into the prime tower, in millimetres.
    tower: f64,
    /// Filament extruded since the last `G92 E0`; only used for absolute
    /// extrusion.
    e: f64,
    /// Every movement so far, for the time estimate.
    moves: Vec<Move>,
    /// Height of the nozzle; unknown before the first Z move.
    nozzle_z: Option<f64>,
    /// Where a lifted travel comes down instead of `z`: the start of the
    /// next rising loop in vase mode.
    land: Option<f64>,
    /// The points of the line printed last, up to the nozzle, for wiping.
    trail: Vec<Point>,
    /// Temperature commands, tool selections and tool change G-code
    /// written between the moves, for the estimate.
    commands: Vec<Command>,
}

/// G-code the writer writes between its moves that the estimate reads.
struct Command {
    /// The number of moves before it.
    at: usize,
    text: String,
    /// For tool change G-code, whose moves and dwells take time: where the
    /// nozzle is (XY if known, and Z) and the feed rate in mm/s.
    toolchange: Option<(Option<Point>, f64, f64)>,
    /// For tool change G-code: where the writer travels after it, from
    /// wherever it left the nozzle, and the height it comes back to.
    back: Option<(Point, f64)>,
}

/// How the filament is pulled back before a travel: the retraction, wipe
/// and Z hop of one set of settings.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Retraction {
    /// Filament pulled back, in millimetres; 0 for none.
    length: f64,
    /// Speed of pulling back and pushing forward again, in mm/s.
    speed: f64,
    /// Lift of the nozzle during the travel, in millimetres.
    z_hop: f64,
    /// Way back along the line printed last while retracting, in
    /// millimetres.
    wipe: f64,
}

impl Retraction {
    /// The retraction of `settings`.
    fn of(settings: &Settings) -> Self {
        Self {
            length: settings.retraction_length,
            speed: settings.retraction_speed,
            z_hop: settings.z_hop,
            wipe: settings.wipe_distance,
        }
    }
}

/// How far above everything printed so far the nozzle rises before it
/// goes over to the next object of a sequential print, in millimetres.
const OBJECT_CLEARANCE: f64 = 2.0;

/// What the layers hand on to the next one besides the printer's state.
#[derive(Debug, Clone, Copy, Default)]
struct Carry {
    /// Height of the highest layer printed so far.
    top: f64,
    /// A later object of a sequential print is being printed, after the
    /// first layer of the first one.
    later: bool,
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
    /// The print, for the speeds of its objects and their height ranges
    /// ([`Toolpaths::settings_on`]).
    toolpaths: &'a Toolpaths,
    /// The printer as the start G-code leaves it, for the time estimate.
    machine: Machine,
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
            retraction: self.retraction,
            retracted: self.retracted.clone(),
            filament: self.filament.clone(),
            tower: self.tower,
            e: self.e,
            moves: Vec::new(),
            nozzle_z: self.nozzle_z,
            land: self.land,
            trail: self.trail.clone(),
            commands: Vec::new(),
        }
    }

    /// Writes `text`, G-code that may set temperatures or select a tool,
    /// and keeps it for the estimate of heating.
    fn command(&mut self, text: String) {
        if !text.is_empty() {
            self.out.push_str(&text);
            self.commands.push(Command {
                at: self.moves.len(),
                text,
                toolchange: None,
                back: None,
            });
        }
    }

    /// Writes `text`, the tool change G-code, and keeps it for the
    /// estimate, which also times its moves and dwells and the travel back
    /// to the print. Where it leaves the nozzle and at what feed rate is
    /// unknown afterwards.
    fn toolchange(&mut self, text: String) {
        let feed = if self.feed > 0.0 {
            self.feed / 60.0
        } else {
            self.settings.travel_speed
        };
        let z = self.nozzle_z.unwrap_or(self.z);
        self.out.push_str(&text);
        self.commands.push(Command {
            at: self.moves.len(),
            text,
            toolchange: Some((self.position, z, feed)),
            back: None,
        });
        self.position = None;
        self.feed = 0.0;
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

    /// Pulls the filament back as `retraction` says, unless it is already.
    fn retract(&mut self) {
        let r = self.retraction;
        if r.length > 0.0 && self.retracted[self.tool].is_none() {
            let f = self.feed_word(r.speed);
            let e = self.e_word(-r.length);
            let _ = writeln!(self.out, "G1{e}{f}");
            self.retracted[self.tool] = Some(r);
            self.filament[self.tool] -= r.length;
            self.record([0.0, 0.0, 0.0, -1.0], r.length, r.speed, None);
        }
    }

    /// Pushes the filament forward again by as much and as fast as it was
    /// pulled back, whatever the settings of the next path say.
    fn unretract(&mut self) {
        if let Some(r) = self.retracted[self.tool].take() {
            let f = self.feed_word(r.speed);
            let e = self.e_word(r.length);
            let _ = writeln!(self.out, "G1{e}{f}");
            self.filament[self.tool] += r.length;
            self.record([0.0, 0.0, 0.0, 1.0], r.length, r.speed, None);
        }
    }

    /// Makes `tool` the selected extruder. An extruder used for the first
    /// time starts with its filament not pulled back; one used before
    /// stays retracted until it extrudes.
    fn select(&mut self, tool: usize) {
        if tool >= self.retracted.len() {
            self.retracted.resize(tool + 1, None);
            self.filament.resize(tool + 1, 0.0);
        }
        self.tool = tool;
    }

    /// Records a move of `length` mm at `speed` for the time estimate; `axes`
    /// says how far X, Y, Z and the filament go per mm of it.
    fn record(&mut self, axes: [f64; 4], length: f64, speed: f64, kind: Option<PathKind>) {
        if length > 0.0 {
            self.moves.push(Move {
                direction: [axes[0], axes[1]],
                axes,
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
            let up = if z < from { -1.0 } else { 1.0 };
            self.record(
                [0.0, 0.0, up, 0.0],
                (z - from).abs(),
                self.settings.travel_speed,
                None,
            );
        }
    }

    /// Moves without extruding, past the corners `via` if the way stays
    /// over the part. `inside` says that it does, where a little oozing
    /// does no harm and retraction is skipped.
    fn travel(&mut self, via: &[Point], to: Point, inside: bool) {
        if inside {
            for &corner in via {
                self.go(corner);
            }
        }
        let length = self
            .position
            .map_or(f64::INFINITY, |from| distance(from, to));
        if length < 1e-4 {
            return;
        }
        let long =
            self.position.is_some() && !inside && length >= self.settings.retraction_min_travel;
        let hop = long && self.retraction.z_hop > 0.0 && self.retraction.length > 0.0;
        if long {
            self.wipe_and_retract();
        }
        // The travel is recorded between the hop moves, where it happens,
        // from where a wipe left the nozzle.
        let length = self.position.map_or(length, |from| distance(from, to));
        let direction = self
            .position
            .filter(|_| length > 0.0)
            .map(|from| [(to[0] - from[0]) / length, (to[1] - from[1]) / length]);
        if hop {
            self.move_z(self.z + self.retraction.z_hop);
        }
        let f = self.feed_word(self.settings.travel_speed);
        let _ = writeln!(self.out, "G0 X{:.3} Y{:.3}{f}", to[0], to[1]);
        self.trail.clear();
        if let Some([x, y]) = direction {
            self.record([x, y, 0.0, 0.0], length, self.settings.travel_speed, None);
        }
        // Back from wherever tool change G-code just before left the nozzle;
        // the estimate times it.
        let moves = self.moves.len();
        if self.position.is_none()
            && let Some(change) = self.commands.last_mut()
            && change.toolchange.is_some()
            && change.back.is_none()
            && change.at == moves
        {
            change.back = Some((to, self.z));
        }
        if hop {
            self.move_z(self.land.unwrap_or(self.z));
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
        self.count(e, kind);
        let rise = self.nozzle_z.map_or(0.0, |below| (z - below) / length);
        let axes = [
            (to[0] - from[0]) / length,
            (to[1] - from[1]) / length,
            rise,
            filament_per_mm,
        ];
        self.record(axes, length, speed, Some(kind));
        self.position = Some(to);
        self.nozzle_z = Some(z);
        self.extend_trail(from, to);
    }

    /// Counts `e` mm of filament extruded for a line of `kind`.
    fn count(&mut self, e: f64, kind: PathKind) {
        self.filament[self.tool] += e;
        if kind == PathKind::PrimeTower {
            self.tower += e;
        }
    }

    /// A straight move without extrusion, retraction or lift.
    fn go(&mut self, to: Point) {
        let Some(from) = self.position else { return };
        let length = distance(from, to);
        if length < 1e-4 {
            return;
        }
        let f = self.feed_word(self.settings.travel_speed);
        let _ = writeln!(self.out, "G0 X{:.3} Y{:.3}{f}", to[0], to[1]);
        self.trail.clear();
        let axes = [
            (to[0] - from[0]) / length,
            (to[1] - from[1]) / length,
            0.0,
            0.0,
        ];
        self.record(axes, length, self.settings.travel_speed, None);
        self.position = Some(to);
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
        self.count(e, kind);
        let axes = [
            (to[0] - from[0]) / length,
            (to[1] - from[1]) / length,
            0.0,
            filament_per_mm,
        ];
        self.record(axes, length, speed, Some(kind));
        self.position = Some(to);
        self.extend_trail(from, to);
    }

    /// Extrudes along `points` from where the nozzle is. With
    /// `arc_fitting`, runs of the points that lie on a circle become arcs,
    /// see [`fit_arc`]; the filament, the time estimate and the wipe trail
    /// stay those of the straight lines.
    fn extrude_all(&mut self, points: &[Point], filament_per_mm: f64, speed: f64, kind: PathKind) {
        let Some(start) = self.position.filter(|_| self.settings.arc_fitting) else {
            for &point in points {
                self.extrude(point, filament_per_mm, speed, kind);
            }
            return;
        };
        // The points the nozzle goes through; `extrude` skips shorter moves.
        let mut run = vec![start];
        for &point in points {
            if run
                .last()
                .is_some_and(|&last| distance(last, point) >= 1e-4)
            {
                run.push(point);
            }
        }
        let mut i = 0;
        while i + 1 < run.len() {
            // Grow the arc point by point for as long as it fits.
            // ponytail: greedy and rechecks the whole run at every step,
            // quadratic in the points of one arc.
            let mut found = None;
            let mut end = i + 3;
            while end < run.len() {
                match fit_arc(&run[i..=end], self.settings.arc_tolerance) {
                    Some(arc) => found = Some((end, arc)),
                    None => break,
                }
                end += 1;
            }
            let Some((end, (centre, clockwise))) = found else {
                self.extrude(run[i + 1], filament_per_mm, speed, kind);
                i += 1;
                continue;
            };
            self.unretract();
            let length: f64 = run[i..=end].windows(2).map(|p| distance(p[0], p[1])).sum();
            let e = length * filament_per_mm;
            let f = self.feed_word(speed);
            let word = self.e_word(e);
            let (from, to) = (run[i], run[end]);
            // ponytail: I and J are taken from the exact start, not from the
            // start as rounded in the G-code; the two differ by at most
            // 0.0005 mm.
            let _ = writeln!(
                self.out,
                "{} X{:.3} Y{:.3} I{:.3} J{:.3}{word}{f}",
                if clockwise { "G2" } else { "G3" },
                to[0],
                to[1],
                centre[0] - from[0],
                centre[1] - from[1]
            );
            self.count(e, kind);
            for pair in run[i..=end].windows(2) {
                let d = distance(pair[0], pair[1]);
                let axes = [
                    (pair[1][0] - pair[0][0]) / d,
                    (pair[1][1] - pair[0][1]) / d,
                    0.0,
                    filament_per_mm,
                ];
                self.record(axes, d, speed, Some(kind));
                self.extend_trail(pair[0], pair[1]);
            }
            self.position = Some(to);
            i = end;
        }
    }

    /// Adds the move from `from` to `to` to the line printed last.
    // ponytail: only the last 64 points are kept; a wipe never needs more
    // than a few millimetres of the line.
    fn extend_trail(&mut self, from: Point, to: Point) {
        if self.trail.last() != Some(&from) {
            self.trail.clear();
            self.trail.push(from);
        }
        self.trail.push(to);
        if self.trail.len() > 128 {
            self.trail.drain(..64);
        }
    }

    /// Retracts while going back along the line printed last for
    /// `wipe_distance`, the filament pulled back in proportion to the way;
    /// a plain retraction without that setting or without a line.
    fn wipe_and_retract(&mut self) {
        let length = self.retraction.length;
        let wipe = self.retraction.wipe;
        let Some(start) = self.position else {
            return self.retract();
        };
        let pulled = self.retracted[self.tool].is_some();
        if wipe <= 0.0 || length <= 0.0 || pulled || self.trail.len() < 2 {
            return self.retract();
        }
        let mut legs = Vec::new();
        let (mut at, mut left) = (start, wipe);
        for &p in self.trail.iter().rev().skip(1) {
            let d = distance(at, p);
            if d >= left {
                legs.push([
                    at[0] + (p[0] - at[0]) * left / d,
                    at[1] + (p[1] - at[1]) * left / d,
                ]);
                left = 0.0;
                break;
            }
            legs.push(p);
            left -= d;
            at = p;
        }
        let total = wipe - left;
        if total < 1e-4 {
            return self.retract();
        }
        let speed = self.settings.travel_speed;
        let mut from = start;
        for to in legs {
            let d = distance(from, to);
            if d < 1e-9 {
                continue;
            }
            let f = self.feed_word(speed);
            let e = self.e_word(-length * d / total);
            let _ = writeln!(self.out, "G1 X{:.3} Y{:.3}{e}{f}", to[0], to[1]);
            let axes = [
                (to[0] - from[0]) / d,
                (to[1] - from[1]) / d,
                0.0,
                -length / total,
            ];
            self.record(axes, d, speed, None);
            from = to;
        }
        self.retracted[self.tool] = Some(self.retraction);
        self.filament[self.tool] -= length;
        self.position = Some(from);
        self.trail.clear();
    }
}

/// The circle through the first, middle and last of `points` if all the
/// points and the middles of the lines between them lie within `tolerance`
/// of it, the lines turn one way and go round its centre in that direction
/// by less than a full turn; at least three lines and a radius above
/// `tolerance` and at most 1000 mm. Returns the centre and whether the arc
/// runs clockwise.
fn fit_arc(points: &[Point], tolerance: f64) -> Option<(Point, bool)> {
    if points.len() < 4 {
        return None;
    }
    let first = points[0];
    let sub = |p: Point, q: Point| [p[0] - q[0], p[1] - q[1]];
    let cross = |a: Point, b: Point| a[0] * b[1] - a[1] * b[0];
    let b = sub(points[points.len() / 2], first);
    let c = sub(points[points.len() - 1], first);
    let d = 2.0 * cross(b, c);
    let (bb, cc) = (b[0] * b[0] + b[1] * b[1], c[0] * c[0] + c[1] * c[1]);
    let u = [(c[1] * bb - b[1] * cc) / d, (b[0] * cc - c[0] * bb) / d];
    let radius = (u[0] * u[0] + u[1] * u[1]).sqrt();
    if !radius.is_finite() || radius > 1000.0 || radius <= tolerance {
        return None;
    }
    let centre = [first[0] + u[0], first[1] + u[1]];
    let side = d.signum();
    let off = |p: Point| (distance(centre, p) - radius).abs() > tolerance;
    // Angle round the centre from the first point in the direction of the
    // arc, as a number from 0 to 4 that grows with the angle (a quarter turn
    // per unit), so that no trigonometry is needed.
    let start = sub(first, centre);
    let turn = |p: Point| {
        let v = sub(p, centre);
        let (x, y) = (start[0] * v[0] + start[1] * v[1], side * cross(start, v));
        match (x >= 0.0, y >= 0.0) {
            (true, true) => y / (x + y),
            (false, true) => 1.0 - x / (y - x),
            (false, false) => 2.0 + y / (x + y),
            (true, false) => 3.0 + x / (x - y),
        }
    };
    let mut angle = 0.0;
    for (k, pair) in points.windows(2).enumerate() {
        let middle = [
            (pair[0][0] + pair[1][0]) / 2.0,
            (pair[0][1] + pair[1][1]) / 2.0,
        ];
        let next = turn(pair[1]);
        if off(pair[1]) || off(middle) || next <= angle {
            return None;
        }
        angle = next;
        if let Some(&after) = points.get(k + 2)
            && side * cross(sub(pair[1], pair[0]), sub(after, pair[1])) < 0.0
        {
            return None;
        }
    }
    Some((centre, side < 0.0))
}

/// Writes one layer. Extrusion runs at `slow` times its speed (1 for full
/// speed), see [`slowdown`]; the layer takes `time` seconds, from which
/// the fan of every path follows with the settings of its object, see
/// [`layer_fan`].
fn write_layer(
    w: &mut Writer,
    carry: &mut Carry,
    layer: &Layer,
    index: usize,
    context: &Context,
    (slow, time): (f64, f64),
) -> Result<(), GcodeError> {
    let settings = w.settings;
    // The fan is set at the start of the layer as the first path wants it
    // (bridges and overhangs aside).
    let first = layer
        .paths
        .iter()
        .find(|p| !p.points.is_empty())
        .map_or(settings, |p| {
            context.toolpaths.settings_on(p, layer, settings)
        });
    let start_fan = layer_fan(first, index, time);
    let _ = writeln!(
        w.out,
        ";LAYER_CHANGE\n;Z:{:.3}\n;HEIGHT:{:.3}",
        layer.z, layer.height
    );
    if index == 1 && !carry.later {
        let mut temperatures = String::new();
        if settings.nozzle_temp != settings.nozzle_temp_first_layer {
            let _ = writeln!(temperatures, "M104 S{}", settings.nozzle_temp);
        }
        if settings.bed_temp != settings.bed_temp_first_layer {
            let _ = writeln!(temperatures, "M140 S{}", settings.bed_temp);
        }
        w.command(temperatures);
    }
    if start_fan != carry.fan {
        carry.fan = start_fan;
        let _ = writeln!(w.out, "M106 S{:.0}", carry.fan * 2.55);
    }
    if context.absolute {
        // Start every layer at zero so the numbers stay short and exact.
        w.out.push_str("G92 E0\n");
        w.e = 0.0;
    }
    // In vase mode the outer wall of every island rises from where the
    // layer below ended; a layer that has more to print is printed flat.
    let rise_from = w.nozzle_z.filter(|_| layer.rises());
    // The first layer of the next object of a sequential print: up above
    // everything printed so far, across to where the layer starts, and down.
    if index == 0 && carry.top > 0.0 {
        carry.later = true;
        w.wipe_and_retract();
        let safe = (carry.top + OBJECT_CLEARANCE)
            .min(settings.max_print_height)
            .max(carry.top);
        w.move_z(safe);
        if let Some(&start) = layer.paths.iter().find_map(|p| p.points.first()) {
            w.travel(&[], start, true);
        }
    }
    carry.top = carry.top.max(layer.z);
    match rise_from {
        Some(from) => w.z = from,
        None => {
            w.z = layer.z;
            w.move_z(layer.z);
        }
    }

    let mut current_kind = None;
    // The object whose label is open.
    let mut open = None;
    for path in &layer.paths {
        let Some(&first) = path.points.first() else {
            continue;
        };
        if context.several && carry.selected != Some(path.extruder) {
            let next = path.extruder;
            if let Some(previous) = carry.selected {
                w.retract();
                let values = toolchange_values(context.values, previous, next);
                let mut block = expand(&settings.toolchange_gcode, "toolchange_gcode", &values)?;
                let _ = writeln!(block, "T{next}");
                w.toolchange(block);
                if context.absolute {
                    w.out.push_str("G92 E0\n");
                    w.e = 0.0;
                }
                carry.restore_z = true;
            } else {
                w.command(format!("T{next}\n"));
            }
            carry.selected = Some(next);
            w.select(next as usize);
        }
        let object = Some(path.object).filter(|&o| o != NO_OBJECT);
        if settings.label_objects && object != open {
            if let Some(o) = open {
                end_label(&mut w.out, settings, o);
            }
            if let Some(o) = object {
                start_label(&mut w.out, settings, o);
            }
            open = object;
        }
        // The loops of further islands start down where the first one
        // started; a lifted travel comes down there.
        w.land = rise_from;
        w.travel(&path.travel, first, path.travel_inside);
        w.land = None;
        if carry.restore_z {
            w.move_z(layer.z);
            carry.restore_z = false;
        }
        // ponytail: the nozzle comes straight down by a layer height onto
        // the start of the next island instead of ramping into it.
        if let Some(from) = rise_from
            && w.nozzle_z != Some(from)
        {
            w.move_z(from);
        }
        // Paths of an object print with the fan, flow, speeds and
        // retraction of the object and the height range of the layer, all
        // others with the plate's; the travel after a path retracts as its
        // object says.
        let own = context.toolpaths.settings_on(path, layer, settings);
        w.retraction = Retraction::of(own);
        let bridge = path.kind == PathKind::Bridge;
        let wanted = match path.kind {
            PathKind::Bridge => own.bridge_fan_speed,
            PathKind::OverhangWall | PathKind::OverhangWallFull => own.overhang_fan_speed,
            PathKind::SupportInterface if own.support_interface_fan_speed >= 0.0 => {
                own.support_interface_fan_speed
            }
            _ => layer_fan(own, index, time),
        };
        if wanted != carry.fan {
            carry.fan = wanted;
            let _ = writeln!(w.out, "M106 S{:.0}", carry.fan * 2.55);
        }
        let filament_per_mm =
            settings.line_width * layer.height * own.flow_ratio / context.filament_area;
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
        // `slow` stretches the speeds of all paths alike.
        let speed = slowed(speed_of(own, path, index), slow, settings);
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
        let mut points = path.points[1..].to_vec();
        if path.closed {
            points.push(first);
        }
        w.extrude_all(&points, filament_per_mm, speed, path.kind);
    }
    if let Some(o) = open {
        end_label(&mut w.out, settings, o);
    }
    Ok(())
}

/// The extruders `layer` uses, ascending: those of its paths with more
/// than one extruder (`several`), otherwise the first.
fn layer_extruders(layer: &Layer, several: bool) -> Vec<u32> {
    if !several {
        return vec![0];
    }
    let mut used: Vec<u32> = layer
        .paths
        .iter()
        .filter(|p| !p.points.is_empty())
        .map(|p| p.extruder)
        .collect();
    used.sort_unstable();
    used.dedup();
    used
}

/// Name of object `object` (counted from 0) in the labels of the G-code.
fn object_name(object: u32) -> String {
    format!("object_{}", u64::from(object) + 1)
}

/// Starts the paths of `object` for the firmware to leave out on request.
fn start_label(out: &mut String, settings: &Settings, object: u32) {
    let _ = match settings.flavor {
        Flavor::Klipper => writeln!(out, "EXCLUDE_OBJECT_START NAME={}", object_name(object)),
        Flavor::Marlin => writeln!(out, "M486 S{object}"),
    };
}

/// Ends the paths of `object` started with [`start_label`].
fn end_label(out: &mut String, settings: &Settings, object: u32) {
    let _ = match settings.flavor {
        Flavor::Klipper => writeln!(out, "EXCLUDE_OBJECT_END NAME={}", object_name(object)),
        Flavor::Marlin => writeln!(out, "M486 S-1"),
    };
}

/// Announces the objects of the print to the firmware, for
/// `label_objects`: for Klipper every object with the convex hull of its
/// paths on the first layer it is printed on and the middle of that hull's
/// bounding box, for Marlin how many objects there are.
fn define_objects(out: &mut String, toolpaths: &Toolpaths, settings: &Settings) {
    let mut first: BTreeMap<u32, Vec<Point>> = BTreeMap::new();
    for layer in &toolpaths.layers {
        let mut here: BTreeMap<u32, Vec<Point>> = BTreeMap::new();
        for path in layer.paths.iter().filter(|p| p.object != NO_OBJECT) {
            if !first.contains_key(&path.object) {
                here.entry(path.object)
                    .or_default()
                    .extend_from_slice(&path.points);
            }
        }
        first.extend(here);
    }
    match settings.flavor {
        Flavor::Marlin => {
            if let Some(&last) = first.keys().next_back() {
                let _ = writeln!(out, "M486 T{}", u64::from(last) + 1);
            }
        }
        Flavor::Klipper => {
            for (object, points) in first {
                let hull = convex_hull(&points);
                let (lo, hi) = hull.iter().fold(
                    ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]),
                    |(lo, hi), p| {
                        (
                            [lo[0].min(p[0]), lo[1].min(p[1])],
                            [hi[0].max(p[0]), hi[1].max(p[1])],
                        )
                    },
                );
                let polygon: Vec<String> = hull
                    .iter()
                    .map(|p| format!("[{:.3},{:.3}]", p[0], p[1]))
                    .collect();
                let _ = writeln!(
                    out,
                    "EXCLUDE_OBJECT_DEFINE NAME={} CENTER={:.3},{:.3} POLYGON=[{}]",
                    object_name(object),
                    (lo[0] + hi[0]) / 2.0,
                    (lo[1] + hi[1]) / 2.0,
                    polygon.join(",")
                );
            }
        }
    }
}

/// The factor by which extrusion on `layer` slows down so that the layer
/// takes at least `min_layer_time`, 1 if it takes that long anyway or the
/// setting is off, and the time in seconds the layer then takes (0 when
/// neither `min_layer_time` nor `fan_layer_time` needs it). The layer is
/// tried out on copies of the writer: the time spent extruding is
/// stretched to fill the gap, and as acceleration makes slower moves gain
/// less than their speed says, the stretch is repeated a few times.
// ponytail: up to five trial runs of the layer; `min_print_speed` can keep
// a layer shorter than asked.
fn slowdown(
    w: &Writer,
    carry: Carry,
    layer: &Layer,
    index: usize,
    context: &Context,
) -> Result<(f64, f64), GcodeError> {
    let settings = w.settings;
    let goal = settings.min_layer_time;
    let fan_by_time = std::iter::once(settings)
        .chain(context.toolpaths.own_settings())
        .any(|s| s.fan_layer_time > 0.0);
    if goal <= 0.0 && !fan_by_time {
        return Ok((1.0, 0.0));
    }
    let mut slow = 1.0;
    for tries in 1.. {
        let mut trial = w.fork();
        let mut carry = carry;
        // The fan takes no time; any layer time will do.
        write_layer(&mut trial, &mut carry, layer, index, context, (slow, 0.0))?;
        let times = move_times(&trial.moves, &context.machine);
        let total: f64 = times.iter().sum();
        let extruding: f64 = trial
            .moves
            .iter()
            .zip(&times)
            .filter(|(m, _)| m.kind.is_some())
            .map(|(_, t)| t)
            .sum();
        if total >= goal * 0.99 || extruding <= 0.0 || tries > 4 {
            return Ok((slow, total));
        }
        slow *= extruding / (goal - (total - extruding));
    }
    Ok((slow, 0.0))
}

/// Fan speed in percent of layer `index`, which takes `time` seconds: off
/// for the first `fan_off_layers`, then `fan_speed`, and for layers shorter
/// than `fan_layer_time` rising in proportion up to `fan_max_speed` at
/// `min_layer_time`.
fn layer_fan(settings: &Settings, index: usize, time: f64) -> f64 {
    let limit = settings.fan_layer_time;
    if index < settings.fan_off_layers as usize {
        0.0
    } else if limit <= 0.0 || time >= limit {
        settings.fan_speed
    } else {
        let shortest = settings.min_layer_time.min(limit);
        let t = ((limit - time) / (limit - shortest).max(1e-9)).clamp(0.0, 1.0);
        settings.fan_speed + (settings.fan_max_speed - settings.fan_speed) * t
    }
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
///
/// On the layers of vase mode that rise ([`Layer::rises`]) the outer wall
/// of every island climbs from the height where the layer below ended to
/// that of its own layer, in proportion to the length gone round, without
/// a move of Z alone. With several islands the nozzle travels to the next
/// one as between any two paths (retracting and lifting as the settings
/// say), comes down to that starting height again there and climbs along
/// that loop as well.
///
/// The paths of objects with settings of their own
/// ([`Toolpaths::object_settings`]) print with their speeds (except on the
/// first layer), fan (`M106` only where the value changes) and flow ratio,
/// and the travel after one of their paths retracts, wipes and lifts as
/// its object says; the filament then goes forward again by as much and
/// as fast as it was pulled back. On the layers of a height range of an
/// object with values of its own ([`Toolpaths::range_settings`]) those of
/// the range count instead ([`Toolpaths::settings_on`]). Everything else
/// prints with `settings`.
// ponytail: the slowdown for `min_layer_time`, the shortest travel that
// retracts, bridge and ironing flow and the speed of travel and wipe stay
// the plate's; `fan_layer_time` of an object goes by the time of the whole
// layer, not of the object's part of it.
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
        PathKind::OverhangWall => settings.overhang_speed,
        PathKind::OverhangWallFull if settings.overhang_speed_full > 0.0 => {
            settings.overhang_speed_full
        }
        PathKind::OverhangWallFull => settings.overhang_speed,
        PathKind::GapFill if settings.gap_infill_speed > 0.0 => settings.gap_infill_speed,
        PathKind::InnerWall | PathKind::GapFill => settings.inner_wall_speed,
        PathKind::TopSurface if settings.top_surface_speed > 0.0 => settings.top_surface_speed,
        PathKind::SupportInterface if settings.support_interface_speed > 0.0 => {
            settings.support_interface_speed
        }
        PathKind::Support | PathKind::SupportInterface if settings.support_speed > 0.0 => {
            settings.support_speed
        }
        PathKind::SolidInfill
        | PathKind::TopSurface
        | PathKind::BottomSurface
        | PathKind::SparseInfill
        | PathKind::Support
        | PathKind::SupportInterface
        | PathKind::PrimeTower => settings.infill_speed,
        PathKind::Bridge => settings.bridge_speed,
        PathKind::Ironing => settings.ironing_speed,
        PathKind::Skirt | PathKind::Brim => settings.first_layer_speed,
    }
}

/// Speed in mm/s at which `path` is printed on layer `layer` (counted
/// from 0): that of its kind ([`path_speed`]), for a small wall loop no
/// faster than `small_perimeter_speed`.
pub fn speed_of(settings: &Settings, path: &Path, layer: usize) -> f64 {
    let speed = path_speed(settings, path.kind, layer);
    let wall = matches!(path.kind, PathKind::OuterWall | PathKind::InnerWall);
    let small = settings.small_perimeter_speed > 0.0
        && wall
        && path.closed
        && path.length() <= 2.0 * PI * settings.small_perimeter_threshold;
    if small {
        speed.min(settings.small_perimeter_speed)
    } else {
        speed
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
        retraction: Retraction::of(settings),
        retracted: vec![None],
        filament: vec![0.0],
        tower: 0.0,
        e: 0.0,
        moves: Vec::new(),
        nozzle_z: None,
        land: None,
        trail: Vec::new(),
        commands: Vec::new(),
    };
    if settings.emit_machine_limits {
        w.out.push_str(&machine_limits(settings));
    }
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

    if settings.label_objects {
        define_objects(&mut w.out, toolpaths, settings);
    }

    let context = Context {
        values: &values,
        filament_area,
        several,
        absolute,
        toolpaths,
        machine: estimate::machine_after(&start, settings),
    };
    let mut carry = Carry::default();
    // Layers are counted within their object: a layer no higher than the
    // one before starts the next object of a sequential print, which gets
    // the speed and fan of a first layer again.
    let mut index = 0;
    let mut below = f64::NEG_INFINITY;
    for layer in &toolpaths.layers {
        if layer.z <= below + 1e-9 {
            index = 0;
        }
        below = layer.z;
        let (slow, time) = slowdown(&w, carry, layer, index, &context)?;
        write_layer(&mut w, &mut carry, layer, index, &context, (slow, time))?;
        progress.advance();
        index += 1;
    }
    w.retract();
    w.out.push_str(&end);

    // The final retraction of every extruder is not consumed filament.
    let used: Vec<f64> = w
        .filament
        .iter()
        .zip(&w.retracted)
        .map(|(&filament, retracted)| filament + retracted.map_or(0.0, |r| r.length))
        .collect();
    let times = move_times(&w.moves, &context.machine);
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
    // The start and end G-code, and the heaters through the whole print.
    let mut estimate = estimate::Estimator::new(settings);
    estimate.run(&start, estimate::Part::StartEnd);
    let mut before = 0;
    for command in &w.commands {
        estimate.pass(times[before..command.at].iter().sum());
        before = command.at;
        match command.toolchange {
            Some((at, z, feed)) => {
                estimate.resume(at, z);
                estimate.set_feed(feed);
                estimate.run(&command.text, estimate::Part::ToolChange);
                if let Some((to, z)) = command.back {
                    estimate.travel_back(to, z);
                }
            }
            None => estimate.run(&command.text, estimate::Part::Silent),
        }
    }
    estimate.pass(times[before..].iter().sum());
    estimate.resume(w.position, w.z);
    estimate.run(&end, estimate::Part::StartEnd);
    let layer_filaments = slicer_formats::layer_filament_ranges(
        &toolpaths
            .layers
            .iter()
            .map(|layer| layer_extruders(layer, several))
            .collect::<Vec<_>>(),
    );
    let total: f64 = used.iter().sum();
    let moving: f64 = times.iter().sum();
    let stats = Stats {
        layers: toolpaths.layers.len(),
        filament_length_mm: total,
        filament_volume_cm3: volume(total),
        filament_weight_g: volume(total) * settings.filament_density,
        print_time_s: moving + estimate.motion + estimate.toolchange + estimate.heating,
        time_start_end_s: estimate.motion,
        time_heating_s: estimate.heating,
        time_toolchange_s: estimate.toolchange,
        extruders,
        flush_volume_cm3: volume(w.tower),
        flush_weight_g: volume(w.tower) * settings.filament_density,
        times: by_kind,
        layer_filaments,
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
    let toolchanges = if stats.time_toolchange_s > 0.0 {
        format!(", tool changes {:.0} s", stats.time_toolchange_s)
    } else {
        String::new()
    };
    let _ = writeln!(
        gcode,
        "; estimated printing time [s]: {:.0} (with acceleration; of that start and end G-code {:.0} s{toolchanges}, heating {:.0} s)",
        stats.print_time_s, stats.time_start_end_s, stats.time_heating_s
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

    #[test]
    fn small_wall_loops_print_slower() {
        let square = |side: f64, kind: PathKind, closed: bool| Path {
            kind,
            points: vec![[0.0, 0.0], [side, 0.0], [side, side], [0.0, side]],
            closed,
            travel_inside: false,
            extruder: 0,
            object: NO_OBJECT,
            width: None,
            travel: Vec::new(),
        };
        let mut settings = Settings::default();
        let small = square(5.0, PathKind::OuterWall, true);
        // Off by default.
        assert_eq!(speed_of(&settings, &small, 3), settings.outer_wall_speed);
        settings.small_perimeter_speed = 15.0;
        // 20 mm round, within a circle of 5 mm radius (31.4 mm).
        assert_eq!(speed_of(&settings, &small, 3), 15.0);
        let inner = square(5.0, PathKind::InnerWall, true);
        assert_eq!(speed_of(&settings, &inner, 3), 15.0);
        // 40 mm round is not small; open lines and infill are no loops.
        let large = square(10.0, PathKind::OuterWall, true);
        assert_eq!(speed_of(&settings, &large, 3), settings.outer_wall_speed);
        let open = square(5.0, PathKind::OuterWall, false);
        assert_eq!(speed_of(&settings, &open, 3), settings.outer_wall_speed);
        let infill = square(5.0, PathKind::SolidInfill, true);
        assert_eq!(speed_of(&settings, &infill, 3), settings.infill_speed);
        // Never faster than the wall's own speed.
        settings.small_perimeter_speed = 500.0;
        assert_eq!(speed_of(&settings, &small, 3), settings.outer_wall_speed);
    }

    #[test]
    fn top_surfaces_and_gap_infill_have_speeds_of_their_own() {
        let mut settings = Settings::default();
        // Unset, they keep the speeds of solid infill and inner walls.
        let top = |s: &Settings| path_speed(s, PathKind::TopSurface, 3);
        let gap = |s: &Settings| path_speed(s, PathKind::GapFill, 3);
        assert_eq!(top(&settings), settings.infill_speed);
        assert_eq!(gap(&settings), settings.inner_wall_speed);
        settings.top_surface_speed = 35.0;
        settings.gap_infill_speed = 25.0;
        assert_eq!(top(&settings), 35.0);
        assert_eq!(gap(&settings), 25.0);
        // The first layer keeps its own speed.
        let first = path_speed(&settings, PathKind::TopSurface, 0);
        assert_eq!(first, settings.first_layer_speed);
        assert_eq!(label(PathKind::TopSurface), "Top surface");
    }

    fn slice(mesh: &Mesh, settings: &Settings) -> Output {
        write_gcode(&slicer_fdm::generate(mesh, settings).unwrap(), settings).unwrap()
    }

    #[test]
    fn times_per_line_type_add_up() {
        let output = slice(&Mesh::cuboid(20.0, 20.0, 4.0), &Settings::default());
        let stats = &output.stats;
        let sum: f64 = stats.times.iter().map(|(_, t)| t).sum();
        let layers = stats.print_time_s - stats.time_start_end_s - stats.time_heating_s;
        assert!((sum - layers).abs() < 1e-6, "{sum}");
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

    /// One loop of vase mode in the G-code: where the nozzle was before its
    /// first rising move, how high it was then, and the height of every
    /// rising move.
    struct Rising {
        start: Point,
        before: f64,
        heights: Vec<f64>,
    }

    /// The rising loops of every layer of `gcode` and the number of moves
    /// of Z alone on it.
    fn rising_loops(gcode: &str) -> Vec<(Vec<Rising>, usize)> {
        let mut layers: Vec<(Vec<Rising>, usize)> = Vec::new();
        let (mut at, mut z, mut rising) = ([0.0, 0.0], 0.0, false);
        for line in gcode.lines() {
            if line.starts_with(";LAYER_CHANGE") {
                layers.push((Vec::new(), 0));
                rising = false;
                continue;
            }
            if !(line.starts_with("G0") || line.starts_with("G1")) {
                continue;
            }
            let (x, y, to) = (word(line, 'X'), word(line, 'Y'), word(line, 'Z'));
            let climbs = x.is_some() && to.is_some() && word(line, 'E').is_some();
            if let Some(layer) = layers.last_mut() {
                if climbs && !rising {
                    layer.0.push(Rising {
                        start: at,
                        before: z,
                        heights: Vec::new(),
                    });
                }
                if let (true, Some(to), Some(last)) = (climbs, to, layer.0.last_mut()) {
                    last.heights.push(to);
                }
                if x.is_none() && to.is_some() {
                    layer.1 += 1;
                }
            }
            rising = climbs;
            at = [x.unwrap_or(at[0]), y.unwrap_or(at[1])];
            z = to.unwrap_or(z);
        }
        layers
    }

    /// Checks that every rising loop of the layers of `toolpaths` from
    /// `first` on climbs steadily from the height of the layer below to its
    /// own, and returns the starts of the loops of every such layer.
    fn check_rising(gcode: &str, toolpaths: &Toolpaths, first: usize) -> Vec<Vec<Point>> {
        let layers = rising_loops(gcode);
        assert_eq!(layers.len(), toolpaths.layers.len());
        let mut starts = Vec::new();
        for (index, (loops, _)) in layers.iter().enumerate().skip(first) {
            let (z, below) = (toolpaths.layers[index].z, toolpaths.layers[index - 1].z);
            assert_eq!(loops.len(), toolpaths.layers[index].paths.len(), "{index}");
            for l in loops {
                assert!(
                    (l.before - below).abs() < 1e-6,
                    "{index}: from {}",
                    l.before
                );
                assert!(l.heights[0] > below && l.heights[0] < z, "{index}");
                assert!(l.heights.windows(2).all(|w| w[1] >= w[0]), "{index}");
                let top = l.heights[l.heights.len() - 1];
                assert!((top - z).abs() < 1e-6, "{index}: to {top}");
            }
            starts.push(loops.iter().map(|l| l.start).collect());
        }
        starts
    }

    /// Two cylinders 16 mm across, 6 mm high, side by side.
    fn two_cylinders() -> [Mesh; 2] {
        [80.0, 110.0].map(|x| {
            let mut mesh = slicer_core::primitives::cylinder(8.0, 6.0, 64);
            mesh.translate([x, 100.0, 0.0]);
            mesh
        })
    }

    #[test]
    fn vase_mode_spirals_every_island() {
        let settings = Settings {
            spiral_vase: true,
            ..Settings::default()
        };
        let toolpaths = plate(&two_cylinders(), &[], &settings);
        let bottom = settings.bottom_layers as usize;
        for layer in &toolpaths.layers[bottom..] {
            assert!(layer.rises());
            assert_eq!(layer.paths.len(), 2);
        }
        let gcode = write_gcode(&toolpaths, &settings).unwrap().gcode;
        let starts = check_rising(&gcode, &toolpaths, bottom);
        // Every island goes on where it started on the layer below, and the
        // layer starts with the island the layer below ended with.
        let mut last: [Option<Point>; 2] = [None, None];
        let mut end: Option<Point> = None;
        for (layer, loops) in starts.iter().enumerate() {
            assert_eq!(loops.len(), 2);
            if let Some(end) = end {
                assert!(distance(end, loops[0]) < 0.5, "{layer}: {end:?} {loops:?}");
            }
            assert!(distance(loops[0], loops[1]) > 20.0, "a travel between them");
            for &start in loops {
                let island = usize::from(start[0] > 100.0);
                if let Some(before) = last[island] {
                    assert!(
                        distance(before, start) < 0.5,
                        "{layer}: {before:?} {start:?}"
                    );
                }
                last[island] = Some(start);
            }
            assert!(last.iter().all(Option::is_some));
            end = Some(loops[1]);
        }
        // Between them the nozzle retracts, travels and goes down again:
        // one move of Z alone a layer.
        let body =
            |gcode: &str| gcode[..gcode.rfind(settings.end_gcode.as_str()).unwrap()].to_string();
        let layers = rising_loops(&body(&gcode));
        assert!(layers[bottom..].iter().all(|(_, z_moves)| *z_moves == 1));
        let retractions: Vec<usize> = body(&gcode)
            .split(";LAYER_CHANGE")
            .skip(bottom + 1)
            .map(|layer| layer.lines().filter(|l| l.starts_with("G1 E-")).count())
            .collect();
        // The first spiral layer also comes from the infill of the bottom,
        // the last retracts at the end.
        let inner = &retractions[1..retractions.len() - 1];
        assert!(inner.iter().all(|&count| count == 1), "{retractions:?}");
        // With a lift the travel comes down where the next loop starts.
        let lifted = Settings {
            z_hop: 0.4,
            ..settings.clone()
        };
        let toolpaths = plate(&two_cylinders(), &[], &lifted);
        let gcode = write_gcode(&toolpaths, &lifted).unwrap().gcode;
        check_rising(&gcode, &toolpaths, bottom);
        let layers = rising_loops(&body(&gcode));
        let moves: Vec<usize> = layers.iter().map(|(_, z_moves)| *z_moves).collect();
        assert!(moves[bottom + 1..].iter().all(|&n| n == 2), "{moves:?}");
    }

    #[test]
    fn vase_mode_follows_a_model_into_two_branches() {
        // A block 3 mm high with two legs on it, 10 mm apart.
        let mut mesh = Mesh::cuboid(30.0, 10.0, 3.0);
        for x in [0.0, 20.0] {
            let mut leg = Mesh::cuboid(10.0, 10.0, 3.0);
            leg.translate([x, 0.0, 3.0]);
            mesh.merge(&leg);
        }
        let settings = Settings {
            spiral_vase: true,
            ..Settings::default()
        };
        let toolpaths = slicer_fdm::generate(&mesh, &settings).unwrap();
        let bottom = settings.bottom_layers as usize;
        let counts: Vec<usize> = toolpaths.layers[bottom..]
            .iter()
            .map(|layer| {
                assert!(layer.rises());
                layer.paths.len()
            })
            .collect();
        let split = counts.iter().position(|&n| n == 2).unwrap();
        assert!(split > 0);
        assert!(counts[..split].iter().all(|&n| n == 1));
        assert!(counts[split..].iter().all(|&n| n == 2));
        let gcode = write_gcode(&toolpaths, &settings).unwrap().gcode;
        let starts = check_rising(&gcode, &toolpaths, bottom);
        // The first layer of the legs goes on where the block ended, and
        // every leg where it started on the layer below.
        let end = starts[split - 1][0];
        assert!(distance(end, starts[split][0]) < 0.5, "{end:?}");
        let right = (toolpaths.layers[bottom].paths[0].points.iter())
            .map(|p| p[0])
            .fold(0.0, f64::max);
        let mut last: [Option<Point>; 2] = [None, None];
        for loops in &starts[split..] {
            for &start in loops {
                let leg = usize::from(start[0] > right - 15.0);
                if let Some(before) = last[leg] {
                    assert!(distance(before, start) < 0.5, "{before:?} {start:?}");
                }
                last[leg] = Some(start);
            }
        }
        assert!(last.iter().all(Option::is_some));
    }

    #[test]
    fn vase_islands_slow_down_to_the_minimum_layer_time() {
        let settings = Settings {
            spiral_vase: true,
            bottom_layers: 1,
            ..Settings::default()
        };
        let slow = Settings {
            min_layer_time: 6.0,
            ..settings.clone()
        };
        let layers_time = |settings: &Settings| {
            let toolpaths = plate(&two_cylinders(), &[], settings);
            let stats = write_gcode(&toolpaths, settings).unwrap().stats;
            (
                stats.print_time_s - stats.time_start_end_s,
                stats.layers as f64,
            )
        };
        let (fast, layers) = layers_time(&settings);
        let (time, _) = layers_time(&slow);
        // Both islands and the travel between them make up the layer time.
        assert!(fast < 0.85 * 6.0 * layers, "{fast}");
        assert!(time > 0.85 * 6.0 * layers, "{time}");
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
    fn travels_go_round_the_hole_of_a_tube() {
        let mut tube = Mesh::cuboid(30.0, 30.0, 2.0);
        let mut hole = Mesh::cuboid(10.0, 10.0, 2.0);
        hole.translate([10.0, 10.0, 0.0]);
        hole.flip();
        tube.merge(&hole);
        // Travels that pass over the hole, more than a line width inside it,
        // and retractions; the tube sits in the middle of the bed.
        let over_the_hole = |gcode: &str| {
            let (mut crossing, mut retractions) = (0, 0);
            let mut at: Option<Point> = None;
            let lo = 110.0 - 15.0 + 10.0 + 0.5;
            let hi = 110.0 - 15.0 + 20.0 - 0.5;
            for line in gcode.lines() {
                let (Some(x), Some(y)) = (word(line, 'X'), word(line, 'Y')) else {
                    if line.starts_with("G1 E-") {
                        retractions += 1;
                    }
                    continue;
                };
                if let (true, Some(from)) = (line.starts_with("G0"), at) {
                    let over = (1..20).any(|k| {
                        let t = f64::from(k) / 20.0;
                        let p = [from[0] + (x - from[0]) * t, from[1] + (y - from[1]) * t];
                        p[0] > lo && p[0] < hi && p[1] > lo && p[1] < hi
                    });
                    crossing += usize::from(over);
                }
                at = Some([x, y]);
            }
            (crossing, retractions)
        };
        let plain = Settings::default();
        let combing = Settings {
            avoid_crossing_walls: true,
            ..Settings::default()
        };
        let (crossed, retracted) = over_the_hole(&slice(&tube, &plain).gcode);
        assert!(crossed > 0, "the test needs travels over the hole");
        let output = slice(&tube, &combing);
        let (crossed_now, retracted_now) = over_the_hole(&output.gcode);
        assert_eq!(crossed_now, 0);
        assert!(retracted_now < retracted, "{retracted_now} {retracted}");
        // Same material.
        assert!(
            (output.stats.filament_length_mm - slice(&tube, &plain).stats.filament_length_mm).abs()
                < 1e-6
        );
    }

    #[test]
    fn wiping_retracts_along_the_last_line() {
        let mesh = Mesh::cuboid(20.0, 20.0, 1.0);
        let plain = Settings::default();
        let wiping = Settings {
            wipe_distance: 2.0,
            ..plain.clone()
        };
        let (a, b) = (slice(&mesh, &plain), slice(&mesh, &wiping));
        assert!((a.stats.filament_length_mm - b.stats.filament_length_mm).abs() < 1e-6);
        // Every retraction moves the nozzle now, pulling back the whole
        // length on its way.
        let retractions = |gcode: &str| {
            let mut found = Vec::new();
            let mut current: Option<(bool, f64)> = None;
            for line in gcode.lines() {
                match (word(line, 'E'), word(line, 'X')) {
                    (Some(e), x) if e < 0.0 => {
                        let entry = current.get_or_insert((x.is_some(), 0.0));
                        entry.1 += e;
                    }
                    _ => found.extend(current.take()),
                }
            }
            found.extend(current);
            found
        };
        let without = retractions(&a.gcode);
        let with = retractions(&b.gcode);
        assert!(!without.is_empty() && without.iter().all(|&(moving, _)| !moving));
        assert!(with.iter().filter(|&&(moving, _)| moving).count() + 1 >= with.len());
        for (_, pulled) in with {
            assert!((pulled + plain.retraction_length).abs() < 1e-4, "{pulled}");
        }
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
        let go = |direction: Point, length, speed| Move {
            direction,
            axes: [direction[0], direction[1], 0.0, 0.0],
            length,
            speed,
            tool: 0,
            kind: None,
        };
        let machine = Machine::of(&Settings::default());
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        // 100 mm at 50 mm/s with 1000 mm/s²: 0.05 s and 1.25 mm for each
        // ramp, 97.5 mm of cruising.
        let straight = planned_time(&[go([1.0, 0.0], 100.0, 50.0)], &machine);
        assert!(close(straight, 0.05 + 0.05 + 97.5 / 50.0), "{straight}");
        // Too short to reach the speed: accelerate half way, then brake.
        let short = planned_time(&[go([1.0, 0.0], 1.0, 50.0)], &machine);
        assert!(close(short, 2.0 * (0.5_f64 / 500.0).sqrt()), "{short}");
        // Cutting a straight line in two changes nothing.
        let halves = [go([1.0, 0.0], 50.0, 50.0), go([1.0, 0.0], 50.0, 50.0)];
        assert!(close(planned_time(&halves, &machine), straight));
        // A right angle is taken at jerk / sqrt(2), which costs time; a
        // full stop costs more.
        let corner = [go([1.0, 0.0], 50.0, 50.0), go([0.0, 1.0], 50.0, 50.0)];
        let stop = [go([1.0, 0.0], 50.0, 50.0), go([-1.0, 0.0], 50.0, 50.0)];
        let (corner, stop) = (
            planned_time(&corner, &machine),
            planned_time(&stop, &machine),
        );
        let slow = 8.0 / 2.0_f64.sqrt();
        let expected = straight + 2.0 * (50.0 - slow) / 1000.0
            - 2.0 * (50.0 * 50.0 - slow * slow) / 2000.0 / 50.0;
        assert!(close(corner, expected), "{corner} {expected}");
        assert!(straight < corner && corner < stop);
        assert!(close(planned_time(&[], &machine), 0.0));

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

    fn shift(axes: [f64; 4], length: f64, speed: f64, kind: Option<PathKind>) -> Move {
        Move {
            direction: [axes[0], axes[1]],
            axes,
            length,
            speed,
            tool: 0,
            kind,
        }
    }

    #[test]
    fn axis_limits_slow_moves_down() {
        let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
        // Nearly without acceleration a move takes its length over its speed.
        let mut settings = Settings {
            acceleration: 1.0e9,
            ..Settings::default()
        };
        let up = [shift([0.0, 0.0, 1.0, 0.0], 10.0, 50.0, None)];
        assert!(close(planned_time(&up, &Machine::of(&settings)), 0.2));
        // A Z move limited by `max_speed_z` takes length / limit.
        settings.max_speed_z = 5.0;
        let machine = Machine::of(&settings);
        assert!(close(planned_time(&up, &machine), 2.0));
        // X and Y moves are not affected by the limit of Z.
        let across = [shift([0.6, 0.8, 0.0, 0.0], 10.0, 50.0, None)];
        assert!(close(planned_time(&across, &machine), 0.2));
        // The most limiting axis counts: 0.6 of the move along X at 10 mm/s
        // allows 16.7 mm/s.
        settings.max_speed_x = 10.0;
        settings.max_speed_y = 100.0;
        let machine = Machine::of(&settings);
        assert!(close(planned_time(&across, &machine), 10.0 / (10.0 / 0.6)));
        // A retraction is a move of the filament alone.
        settings.max_speed_e = 25.0;
        let retract = [shift([0.0, 0.0, 0.0, -1.0], 1.0, 35.0, None)];
        assert!(close(planned_time(&retract, &Machine::of(&settings)), 0.04));

        // Acceleration: 1 mm from rest to rest takes 2 * sqrt(1 / a).
        let settings = Settings::default();
        let along_x = [shift([1.0, 0.0, 0.0, 0.0], 1.0, 1000.0, None)];
        let along_y = [shift([0.0, 1.0, 0.0, 0.0], 1.0, 1000.0, None)];
        let ramp = |a: f64| 2.0 * (1.0 / a).sqrt();
        assert!(close(
            planned_time(&along_x, &Machine::of(&settings)),
            ramp(1000.0)
        ));
        let limited = Settings {
            max_accel_x: 100.0,
            ..settings.clone()
        };
        let machine = Machine::of(&limited);
        assert!(close(planned_time(&along_x, &machine), ramp(100.0)));
        assert!(close(planned_time(&along_y, &machine), ramp(1000.0)));
        // Half of the move along X accelerates twice as hard as X may.
        let diagonal = [shift([0.5, 0.75_f64.sqrt(), 0.0, 0.0], 1.0, 1000.0, None)];
        assert!(close(planned_time(&diagonal, &machine), ramp(200.0)));
        // Printing and travel have accelerations of their own.
        let split = Settings {
            max_accel_extruding: 250.0,
            max_accel_travel: 400.0,
            ..settings.clone()
        };
        let machine = Machine::of(&split);
        let print = [shift(
            [1.0, 0.0, 0.0, 0.05],
            1.0,
            1000.0,
            Some(PathKind::InnerWall),
        )];
        assert!(close(planned_time(&print, &machine), ramp(250.0)));
        assert!(close(planned_time(&along_x, &machine), ramp(400.0)));
    }

    #[test]
    fn klipper_takes_corners_at_the_square_corner_velocity() {
        let klipper = Settings {
            flavor: Flavor::Klipper,
            acceleration: 1000.0,
            square_corner_velocity: 5.0,
            ..Settings::default()
        };
        let machine = Machine::of(&klipper);
        let go = |x: f64, y: f64| shift([x, y, 0.0, 0.0], 10.0, 100.0, None);
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        // A 90° corner at the acceleration: the square corner velocity.
        let square = machine.corner(&go(1.0, 0.0), &go(0.0, 1.0), 1000.0);
        assert!(close(square, 5.0), "{square}");
        // Also at another acceleration of the printer.
        let fast = Settings {
            acceleration: 4000.0,
            ..klipper.clone()
        };
        let square = Machine::of(&fast).corner(&go(1.0, 0.0), &go(0.0, -1.0), 4000.0);
        assert!(close(square, 5.0), "{square}");
        // Wider corners are faster, sharper ones slower; straight on does not
        // slow down, turning back stops.
        let half = 0.5_f64.sqrt();
        let wide = machine.corner(&go(1.0, 0.0), &go(half, half), 1000.0);
        let sharp = machine.corner(&go(1.0, 0.0), &go(-half, half), 1000.0);
        assert!(wide > 5.0 && sharp < 5.0, "{wide} {sharp}");
        assert_eq!(
            machine.corner(&go(1.0, 0.0), &go(1.0, 0.0), 1000.0),
            f64::INFINITY
        );
        assert_eq!(machine.corner(&go(1.0, 0.0), &go(-1.0, 0.0), 1000.0), 0.0);
        // 45° off straight on: sin(67.5°) in r = d * s / (1 - s).
        let d = 25.0 * (2.0_f64.sqrt() - 1.0) / 1000.0;
        let s = ((1.0 + half) / 2.0).sqrt();
        assert!(close(wide, (1000.0 * d * s / (1.0 - s)).sqrt()), "{wide}");
        // Marlin with junction deviation: v² = a * d / (sqrt(2) - 1) at 90°.
        let marlin = Settings {
            junction_deviation: 0.02,
            ..Settings::default()
        };
        let square = Machine::of(&marlin).corner(&go(1.0, 0.0), &go(0.0, 1.0), 1000.0);
        let expected = (1000.0 * 0.02 / (2.0_f64.sqrt() - 1.0)).sqrt();
        assert!(close(square, expected), "{square} {expected}");
        // A corner with the square corner velocity is slower than one with
        // a jerk of 8 mm/s (taken at 8 / sqrt(2) = 5.7 mm/s).
        let corner = [go(1.0, 0.0), go(0.0, 1.0)];
        let jerk = planned_time(&corner, &Machine::of(&Settings::default()));
        let no_cruise = Settings {
            minimum_cruise_ratio: 0.0,
            ..klipper.clone()
        };
        assert!(planned_time(&corner, &Machine::of(&no_cruise)) > jerk);
    }

    #[test]
    fn klipper_keeps_a_share_of_short_moves_for_cruising() {
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        let klipper = Settings {
            flavor: Flavor::Klipper,
            acceleration: 1000.0,
            minimum_cruise_ratio: 0.5,
            ..Settings::default()
        };
        // A standalone 1.5 mm move cruises for 0.75 mm, as Klipper's
        // documentation describes the ratio of 0.5: its top speed is what
        // it reaches at half the acceleration.
        let short = [shift([1.0, 0.0, 0.0, 0.0], 1.5, 100.0, None)];
        let peak = (500.0_f64 * 1.5).sqrt();
        let expected = 2.0 * peak / 1000.0 + 0.75 / peak;
        let time = planned_time(&short, &Machine::of(&klipper));
        assert!(close(time, expected), "{time} {expected}");
        // Without the ratio it accelerates and brakes at once.
        let off = Settings {
            minimum_cruise_ratio: 0.0,
            ..klipper.clone()
        };
        let time = planned_time(&short, &Machine::of(&off));
        assert!(close(time, 2.0 * (1.5_f64 / 1000.0).sqrt()), "{time}");
        // A long move reaches its speed anyway.
        let long = [shift([1.0, 0.0, 0.0, 0.0], 100.0, 50.0, None)];
        let a = planned_time(&long, &Machine::of(&klipper));
        let b = planned_time(&long, &Machine::of(&off));
        assert!(close(a, b), "{a} {b}");
    }

    #[test]
    fn tool_change_gcode_moves_and_dwells_take_time() {
        let mut settings = Settings::default();
        settings.extruder_count = 2;
        let plain = slice(&two_cubes(), &settings);
        let changes =
            plain.gcode.matches("\nT1\n").count() + plain.gcode.matches("\nT0\n").count() - 1;
        assert!(changes > 0);
        // Without tool change G-code only the travel to the next path counts.
        let back = plain.stats.time_toolchange_s;
        assert!(back > 0.0 && back < changes as f64, "{back}");
        // Each change: 5 mm up and down at 10 mm/s from rest to rest with
        // 1000 mm/s² (0.01 s for each ramp of 0.05 mm, 4.9 mm cruising) and
        // a dwell of half a second.
        settings.toolchange_gcode = "G91\nG1 Z5 F600\nG4 P500\nG1 Z-5\nG90".into();
        let output = slice(&two_cubes(), &settings);
        let stats = &output.stats;
        let each = 2.0 * (0.02 + 4.9 / 10.0) + 0.5;
        assert!(
            (stats.time_toolchange_s - back - changes as f64 * each).abs() < 1e-6,
            "{} {changes}",
            stats.time_toolchange_s
        );
        let layers: f64 = stats.times.iter().map(|(_, t)| t).sum();
        let total =
            layers + stats.time_start_end_s + stats.time_toolchange_s + stats.time_heating_s;
        assert!((stats.print_time_s - total).abs() < 1e-6);
        assert!(
            (stats.print_time_s - plain.stats.print_time_s - stats.time_toolchange_s + back).abs()
                < 1e-6
        );
        assert!(output.gcode.contains(&format!(
            "tool changes {:.0} s, heating",
            stats.time_toolchange_s
        )));
    }

    #[test]
    fn the_way_back_from_tool_change_gcode_takes_time() {
        let mut settings = Settings::default();
        settings.extruder_count = 2;
        // The feed rate of the travel back is written even where it is the
        // one the writer used last: the tool change G-code changed it.
        settings.retraction_speed = settings.travel_speed;
        settings.toolchange_gcode = "G1 X0 Y0 F6000".into();
        let output = slice(&two_cubes(), &settings);
        let lines: Vec<&str> = output.gcode.lines().collect();
        let mut changes = 0;
        for (at, line) in lines.iter().enumerate() {
            if *line == "G1 X0 Y0 F6000" {
                changes += 1;
                let next = lines[at + 2];
                assert!(next.starts_with("G0 X") && next.contains(" F"), "{next}");
            }
        }
        assert!(changes > 0);
        // The cubes lie at least 140 mm from the corner of the bed: every
        // change goes there at 100 mm/s and back at `travel_speed`.
        let least = 140.0 / 100.0 + 140.0 / settings.travel_speed;
        let time = output.stats.time_toolchange_s;
        assert!(time > changes as f64 * least, "{time} {changes}");
        assert!(time < changes as f64 * 2.0 * least, "{time} {changes}");
    }

    #[test]
    fn limits_in_the_start_gcode_hold_for_the_print() {
        let mesh = Mesh::cuboid(20.0, 20.0, 4.0);
        let layers = |settings: &Settings| -> f64 {
            let stats = slice(&mesh, settings).stats;
            stats.times.iter().map(|(_, t)| t).sum()
        };
        let with_start = |base: &Settings, gcode: &str| Settings {
            start_gcode: format!("{}\n{gcode}", base.start_gcode),
            ..base.clone()
        };
        let marlin = Settings::default();
        let plain = layers(&marlin);
        // Marlin: the start G-code sets the same limits as the settings.
        let start = with_start(
            &marlin,
            "M201 X400 Y500 E3000 ; per axis\nM203 X40 Y50 Z3\nM204 P300 T600\nM205 J0.03",
        );
        let same = Settings {
            max_accel_x: 400.0,
            max_accel_y: 500.0,
            max_accel_e: 3000.0,
            max_speed_x: 40.0,
            max_speed_y: 50.0,
            max_speed_z: 3.0,
            max_accel_extruding: 300.0,
            max_accel_travel: 600.0,
            junction_deviation: 0.03,
            ..marlin.clone()
        };
        let limited = layers(&start);
        assert!(limited > plain * 1.1, "{limited} {plain}");
        assert!((limited - layers(&same)).abs() < 1e-9);
        // `M204 S` sets both accelerations, `M205 X`/`Y` the jerk.
        let start = with_start(&marlin, "M204 S300\nM205 X4 Y5");
        let same = Settings {
            acceleration: 300.0,
            jerk: 4.0,
            ..marlin.clone()
        };
        assert!((layers(&start) - layers(&same)).abs() < 1e-9);
        // Klipper's command does nothing on Marlin.
        let start = with_start(&marlin, "SET_VELOCITY_LIMIT VELOCITY=5 ACCEL=50");
        assert_eq!(layers(&start), plain);

        // Klipper: `SET_VELOCITY_LIMIT` and `M204`, not Marlin's limits.
        let klipper = Settings {
            flavor: Flavor::Klipper,
            ..Settings::default()
        };
        let plain = layers(&klipper);
        let start = with_start(
            &klipper,
            "set_velocity_limit VELOCITY=40 ACCEL=300 SQUARE_CORNER_VELOCITY=2 MINIMUM_CRUISE_RATIO=0.2",
        );
        let same = Settings {
            max_speed_x: 40.0,
            max_speed_y: 40.0,
            acceleration: 300.0,
            square_corner_velocity: 2.0,
            minimum_cruise_ratio: 0.2,
            ..klipper.clone()
        };
        let limited = layers(&start);
        assert!(limited > plain * 1.1, "{limited} {plain}");
        assert!((limited - layers(&same)).abs() < 1e-9);
        let start = with_start(&klipper, "M204 P500 T300");
        let same = Settings {
            acceleration: 300.0,
            ..klipper.clone()
        };
        assert!((layers(&start) - layers(&same)).abs() < 1e-9);
        let start = with_start(&klipper, "M203 X5 Y5\nM201 X10\nM204 P5\nM205 J1");
        assert_eq!(layers(&start), plain);
    }

    #[test]
    fn without_limits_the_estimate_stays_the_same() {
        let mesh = Mesh::cuboid(20.0, 20.0, 4.0);
        let plain = slice(&mesh, &Settings::default());
        // Klipper's settings do not touch a Marlin printer, and limits far
        // above the speeds and accelerations used change nothing.
        let settings = Settings {
            square_corner_velocity: 20.0,
            minimum_cruise_ratio: 0.9,
            max_speed_x: 2000.0,
            max_speed_y: 2000.0,
            max_speed_z: 2000.0,
            max_speed_e: 2000.0,
            max_accel_x: 100_000.0,
            max_accel_y: 100_000.0,
            max_accel_z: 100_000.0,
            max_accel_e: 100_000.0,
            max_accel_extruding: 100_000.0,
            max_accel_travel: 100_000.0,
            ..Settings::default()
        };
        let same = slice(&mesh, &settings);
        assert_eq!(same.stats, plain.stats);
        assert_eq!(
            Machine::of(&Settings::default()).corners,
            Corners::Jerk(8.0)
        );
        // A limit that holds back the layer changes makes it longer.
        let slow_z = Settings {
            max_speed_z: 1.0,
            ..Settings::default()
        };
        let slow = slice(&mesh, &slow_z).stats;
        assert!(slow.print_time_s > plain.stats.print_time_s + 1.0);
    }

    #[test]
    fn machine_limits_are_written_on_request() {
        let mesh = Mesh::cuboid(10.0, 10.0, 1.0);
        let mut settings = Settings::default();
        let plain = slice(&mesh, &settings);
        assert!(!plain.gcode.contains("M201") && !plain.gcode.contains("M204"));
        settings.emit_machine_limits = true;
        settings.max_speed_z = 5.0;
        settings.max_speed_e = 25.0;
        settings.max_accel_x = 500.0;
        settings.max_accel_extruding = 800.0;
        let gcode = slice(&mesh, &settings).gcode;
        let start = gcode.find("M140").unwrap();
        for line in [
            "M201 X500 ",
            "M203 Z5 E25 ",
            "M204 P800 T1000 ",
            "M205 X8 Y8 ",
        ] {
            let at = gcode.find(line).unwrap_or(usize::MAX);
            assert!(at < start, "{line}");
        }
        settings.junction_deviation = 0.02;
        assert!(slice(&mesh, &settings).gcode.contains("\nM205 J0.02 "));
        // Klipper gets what `SET_VELOCITY_LIMIT` can set: one speed limit
        // and one acceleration of the nozzle and its corners.
        settings.flavor = Flavor::Klipper;
        settings.max_speed_x = 300.0;
        settings.max_speed_y = 250.0;
        settings.max_accel_travel = 900.0;
        let gcode = slice(&mesh, &settings).gcode;
        assert!(!gcode.contains("M201") && !gcode.contains("M204"));
        let line = "SET_VELOCITY_LIMIT VELOCITY=250 ACCEL=800 SQUARE_CORNER_VELOCITY=5 \
                    MINIMUM_CRUISE_RATIO=0.5 ;";
        let at = gcode.find(line).unwrap_or(usize::MAX);
        assert!(at < gcode.find("M140").unwrap(), "{gcode}");
        // The estimate assumes the same printer, set or read.
        let start = Settings {
            start_gcode: format!("{line}\n{}", settings.start_gcode),
            emit_machine_limits: false,
            max_speed_x: 0.0,
            max_speed_y: 0.0,
            max_accel_travel: 0.0,
            max_accel_extruding: 0.0,
            ..settings.clone()
        };
        let set = slice(&mesh, &settings).stats;
        let read = slice(&mesh, &start).stats;
        assert!((set.print_time_s - read.print_time_s).abs() < 1e-6);
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
    #[test]
    fn start_and_end_gcode_take_their_time() {
        let mut settings = Settings::default();
        settings.start_gcode = String::new();
        settings.end_gcode = String::new();
        let mesh = Mesh::cuboid(10.0, 10.0, 2.0);
        let bare = slice(&mesh, &settings).stats;
        assert_eq!(bare.time_start_end_s, 0.0);
        // 100 mm at 10 mm/s, speeding up and slowing down at 1000 mm/s²,
        // and two seconds of waiting.
        settings.start_gcode = "G1 X100 F600\nG4 P2000".into();
        let with = slice(&mesh, &settings).stats;
        let expected = 10.01 + 2.0;
        assert!((with.time_start_end_s - expected).abs() < 1e-9);
        assert!((with.print_time_s - bare.print_time_s - expected).abs() < 1e-9);
        assert_eq!(with.times, bare.times);
        // The end G-code starts where the print ended: up 10 mm at 10 mm/s.
        settings.start_gcode = String::new();
        settings.end_gcode = "G91\nG1 Z10 F600\nG28 X".into();
        let end = slice(&mesh, &settings).stats;
        let expected = 1.01 + settings.homing_time;
        assert!((end.time_start_end_s - expected).abs() < 1e-9);
        // The default start and end G-code home once and lift the nozzle.
        let defaults = slice(&mesh, &Settings::default()).stats;
        assert!((defaults.time_start_end_s - 11.01).abs() < 1e-9);
        assert_eq!(defaults.time_heating_s, 0.0);
        let output = slice(&mesh, &Settings::default());
        assert!(output.gcode.contains(&format!(
            "\n; estimated printing time [s]: {:.0} (with acceleration; of that start and end G-code 11 s, heating 0 s)\n",
            output.stats.print_time_s
        )));
    }

    #[test]
    fn heating_counts_when_it_is_waited_for() {
        let mesh = Mesh::cuboid(10.0, 10.0, 2.0);
        let mut settings = Settings::default();
        settings.start_gcode = "M190 S60".into();
        // Without heat rates heating is left out.
        assert_eq!(slice(&mesh, &settings).stats.time_heating_s, 0.0);
        // From 25 to 60 °C at 1 °C/s.
        settings.bed_heat_rate = 1.0;
        let stats = slice(&mesh, &settings).stats;
        assert!((stats.time_heating_s - 35.0).abs() < 1e-9);
        let layers: f64 = stats.times.iter().map(|(_, t)| t).sum();
        let total = layers + stats.time_start_end_s + stats.time_heating_s;
        assert!((stats.print_time_s - total).abs() < 1e-9);
        // The second layer sets the nozzle cooler without waiting.
        settings.nozzle_heat_rate = 1.0;
        settings.start_gcode = "M109 S{nozzle_temp_first_layer}".into();
        let output = slice(&mesh, &settings);
        assert!(output.gcode.contains("\nM104 S210\n"));
        assert!((output.stats.time_heating_s - 190.0).abs() < 1e-9);

        // A tool change waits for the next nozzle to heat: the first time
        // from cold, later it is still warm.
        let mut settings = Settings::default();
        settings.extruder_count = 2;
        settings.nozzle_heat_rate = 1.0;
        settings.start_gcode = "M109 S200".into();
        settings.toolchange_gcode = "M109 T{next_extruder} S200".into();
        let stats = slice(&two_cubes(), &settings).stats;
        assert!(
            (stats.time_heating_s - 350.0).abs() < 1e-9,
            "{}",
            stats.time_heating_s
        );
        // Without waiting it takes no time.
        settings.toolchange_gcode = "M104 T{next_extruder} S200".into();
        let stats = slice(&two_cubes(), &settings).stats;
        assert!((stats.time_heating_s - 175.0).abs() < 1e-9);
    }

    #[test]
    fn layers_list_the_extruders_they_use() {
        let run = |filaments: &[u32], first, last| slicer_formats::LayerFilaments {
            filaments: filaments.to_vec(),
            first,
            last,
        };
        // One extruder: all layers with the first, as before.
        let one = slice(&two_cubes(), &Settings::default()).stats;
        assert_eq!(one.layer_filaments, [run(&[0], 0, 24)]);
        // The second cube is lower: above it only the first extruder prints.
        let mut settings = Settings::default();
        settings.extruder_count = 2;
        settings.skirt_loops = 0;
        settings.prime_tower_width = 0.0;
        let mut mesh = Mesh::cuboid(10.0, 10.0, 5.0);
        let mut other = Mesh::cuboid(10.0, 10.0, 3.0);
        other.translate([20.0, 0.0, 0.0]);
        other.set_extruder(0, 1);
        mesh.merge(&other);
        let stats = slice(&mesh, &settings).stats;
        assert_eq!(
            stats.layer_filaments,
            [run(&[0, 1], 0, 14), run(&[0], 15, 24)]
        );
    }

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
        let stats = &output.stats;
        let layers = stats.print_time_s
            - stats.time_start_end_s
            - stats.time_heating_s
            - stats.time_toolchange_s;
        assert!((time - layers).abs() < 1e-6);
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
    fn tool_changes_purge_their_flush_volumes_into_the_tower() {
        let mut settings = Settings::default();
        settings.extruder_count = 2;
        settings.prime_tower_width = 40.0;
        settings.prime_tower_x = 160.0;
        settings.prime_tower_y = 160.0;
        let solid = slice(&two_cubes(), &settings);
        settings.set("flush_volumes", "0,50;200,0").unwrap();
        let output = slice(&two_cubes(), &settings);
        settings.prime_tower_width = 0.0;
        let plain = slice(&two_cubes(), &settings);
        // Layer by layer: from extruder 0 to 1 on even layers, back on odd
        // ones, the tower gets the volume of that change.
        let area = PI * (settings.filament_diameter / 2.0).powi(2);
        let layers: Vec<&str> = output.gcode.split(";LAYER_CHANGE").skip(1).collect();
        assert_eq!(layers.len(), 25);
        for (index, layer) in layers.iter().enumerate() {
            let volume: f64 = (extrusions(layer).iter())
                .filter(|(kind, ..)| kind == "Prime tower")
                .map(|(_, _, e)| e * area)
                .sum();
            let wanted = if index % 2 == 0 { 50.0 } else { 200.0 };
            assert!(
                (volume / wanted - 1.0).abs() < 0.05,
                "layer {index}: {volume}"
            );
        }
        // The statistics count the purges with their extruders and on
        // their own: 13 changes of 50 mm³, 12 of 200 mm³.
        let stats = &output.stats;
        let expected = (13.0 * 50.0 + 12.0 * 200.0) / 1000.0;
        assert!(
            (stats.flush_volume_cm3 / expected - 1.0).abs() < 0.05,
            "{stats:?}"
        );
        let weight = stats.flush_volume_cm3 * settings.filament_density;
        assert!((stats.flush_weight_g - weight).abs() < 1e-9);
        let cubes = solid.stats.filament_volume_cm3 - solid.stats.flush_volume_cm3;
        let total = stats.filament_volume_cm3 - stats.flush_volume_cm3;
        assert!((total - cubes).abs() < 1e-6, "{total} {cubes}");
        assert!((total - plain.stats.filament_volume_cm3).abs() < 1e-6);
        let volumes = |output: &Output| -> Vec<f64> {
            (output.stats.extruders.iter())
                .map(|e| e.filament_volume_cm3)
                .collect()
        };
        let (with, without) = (volumes(&output), volumes(&plain));
        // Extruder 1 purges 13 × 50 mm³, extruder 0 12 × 200 mm³.
        let (one, zero) = (with[1] - without[1], with[0] - without[0]);
        assert!((one / 0.65 - 1.0).abs() < 0.08, "{one}");
        assert!((zero / 2.4 - 1.0).abs() < 0.08, "{zero}");
        assert!(stats.print_time_s > plain.stats.print_time_s);

        // Without a tower nothing is purged, and JSON leaves it out.
        assert!(is_zero(&plain.stats.flush_volume_cm3));
        assert!(is_zero(&plain.stats.flush_weight_g));
    }

    #[test]
    fn purges_come_from_the_colours_only_with_the_switch() {
        let mut settings = Settings::default();
        settings.extruder_count = 2;
        settings.prime_tower_width = 40.0;
        settings.prime_tower_x = 160.0;
        settings.prime_tower_y = 160.0;
        let before = slice(&two_cubes(), &settings);
        // Colours alone change nothing but the settings list at the end.
        settings.set("filament_colours", "#101010,#F0F0F0").unwrap();
        let coloured = slice(&two_cubes(), &settings);
        let body = |output: &Output| output.gcode.split("; settings\n").next().map(str::to_owned);
        assert_eq!(body(&coloured), body(&before));
        assert_eq!(coloured.stats, before.stats);
        // With the switch, the changes purge what the colours ask for: dark
        // to light more than back.
        settings.set("flush_from_colours", "true").unwrap();
        let derived = slice(&two_cubes(), &settings);
        let matrix = settings.flush_matrix().unwrap();
        let mut table = settings.clone();
        table.flush_from_colours = false;
        table
            .set(
                "flush_volumes",
                &format!("0,{};{},0", matrix[0][1], matrix[1][0]),
            )
            .unwrap();
        assert_eq!(derived.stats, slice(&two_cubes(), &table).stats);
        assert!(matrix[0][1] > matrix[1][0]);
        assert!(derived.stats.flush_volume_cm3 > 0.0);
    }

    #[test]
    fn filaments_carry_their_colours_and_use() {
        let mut settings = Settings::default();
        settings.extruder_count = 2;
        settings.prime_tower_width = 0.0;
        settings
            .set_all([
                ("filament_colours", "#FF8000,#202020"),
                ("filament_type", "PLA,PETG"),
            ])
            .unwrap();
        let output = slice(&two_cubes(), &settings);
        let list = filaments(&output.stats, &settings);
        assert_eq!(list.len(), 2);
        assert_eq!(
            (list[0].id, list[0].kind.as_str(), list[0].colour.as_str()),
            (1, "PLA", "#FF8000")
        );
        assert_eq!(
            (list[1].id, list[1].kind.as_str(), list[1].colour.as_str()),
            (2, "PETG", "#202020")
        );
        let e = &output.stats.extruders[1];
        assert_eq!(list[1].used_m, e.filament_length_mm / 1000.0);
        assert_eq!(list[1].used_g, e.filament_weight_g);
        // One extruder without settings: the white PLA of before.
        let plain = slice(&two_cubes(), &Settings::default());
        let list = filaments(&plain.stats, &Settings::default());
        assert_eq!(list.len(), 1);
        assert_eq!(
            (list[0].id, list[0].kind.as_str(), list[0].colour.as_str()),
            (1, "PLA", "#FFFFFF")
        );
        assert_eq!(list[0].used_m, plain.stats.filament_length_mm / 1000.0);
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

    #[test]
    fn short_layers_get_more_fan() {
        let fans = |output: &Output| -> Vec<f64> {
            output
                .gcode
                .lines()
                .filter(|l| l.starts_with("M106"))
                .filter_map(|l| word(l, 'S'))
                .collect()
        };
        let mesh = Mesh::cuboid(10.0, 10.0, 2.0);
        let mut settings = Settings::default();
        settings.fan_speed = 40.0;
        assert_eq!(fans(&slice(&mesh, &settings)), [102.0]);
        // Layers of a few seconds get nearly the full fan.
        settings.fan_layer_time = 60.0;
        let more = fans(&slice(&mesh, &settings));
        assert!(more.iter().all(|&s| s > 102.0 && s < 255.0), "{more:?}");
        // Layers slowed down to the minimum layer time get all of it.
        settings.min_layer_time = 20.0;
        settings.fan_layer_time = 30.0;
        assert_eq!(fans(&slice(&mesh, &settings)), [255.0]);
    }

    #[test]
    fn overhanging_walls_print_slowly_with_their_own_fan() {
        let mut mesh = Mesh::cuboid(10.0, 10.0, 4.0);
        let mut slab = Mesh::cuboid(20.0, 10.0, 2.0);
        slab.translate([-5.0, 0.0, 4.0]);
        mesh.merge(&slab);
        let mut settings = Settings::default();
        settings.fan_speed = 40.0;
        settings.bridge_fan_speed = 40.0;
        settings.overhang_speed = 15.0;
        settings.overhang_fan_speed = 80.0;
        let overhang_feeds = |settings: &Settings, feed: f64| {
            let output = slice(&mesh, settings);
            let mut overhang = false;
            let mut moves = 0;
            for line in output.gcode.lines() {
                if let Some(kind) = line.strip_prefix(";TYPE:") {
                    overhang = kind == "Overhang wall";
                } else if overhang && line.starts_with("G1 X") && word(line, 'E').is_some() {
                    moves += 1;
                    if let Some(f) = word(line, 'F') {
                        assert_eq!(f, feed, "{line}");
                    }
                }
            }
            assert!(moves > 4, "{moves}");
            output
        };
        let output = overhang_feeds(&settings, 900.0);
        let fans: Vec<&str> = output
            .gcode
            .lines()
            .filter(|l| l.starts_with("M106"))
            .collect();
        assert!(fans.contains(&"M106 S204"), "{fans:?}");
        // The slab hangs free entirely: with its own speed.
        settings.overhang_speed_full = 5.0;
        overhang_feeds(&settings, 300.0);
    }

    /// The G-code without the settings dump at its end.
    fn moves_only(gcode: &str) -> &str {
        &gcode[..gcode.rfind("; settings").unwrap()]
    }

    #[test]
    fn arcs_replace_the_lines_round_a_cylinder() {
        let mesh = slicer_core::primitives::cylinder(10.0, 2.0, 128);
        let plain = Settings::default();
        let arcs = Settings {
            arc_fitting: true,
            ..plain.clone()
        };
        let (a, b) = (slice(&mesh, &plain), slice(&mesh, &arcs));
        let count = |gcode: &str, prefix: &str| {
            moves_only(gcode)
                .lines()
                .filter(|l| l.starts_with(prefix))
                .count()
        };
        assert_eq!(count(&a.gcode, "G2 ") + count(&a.gcode, "G3 "), 0);
        assert!(count(&b.gcode, "G2 ") + count(&b.gcode, "G3 ") > 0);
        assert!(
            count(&b.gcode, "G") < count(&a.gcode, "G"),
            "{} {}",
            count(&b.gcode, "G"),
            count(&a.gcode, "G")
        );
        assert!((a.stats.filament_length_mm - b.stats.filament_length_mm).abs() < 1e-6);
        assert!((a.stats.print_time_s - b.stats.print_time_s).abs() < 1e-6);

        // Every other line stays as it was; every arc stands for three or
        // more of the straight lines, ends where they end, and their points
        // lie on its circle.
        let tolerance = arcs.arc_tolerance + 0.002;
        let mut straight = moves_only(&a.gcode).lines();
        let mut at = [0.0, 0.0];
        let mut checked = 0;
        for line in moves_only(&b.gcode).lines() {
            if !(line.starts_with("G2 ") || line.starts_with("G3 ")) {
                assert_eq!(Some(line), straight.next());
                if let (Some(x), Some(y)) = (word(line, 'X'), word(line, 'Y')) {
                    at = [x, y];
                }
                continue;
            }
            // Round the cylinder counter-clockwise, as the walls go.
            assert!(line.starts_with("G3 "), "{line}");
            let centre = [
                at[0] + word(line, 'I').unwrap(),
                at[1] + word(line, 'J').unwrap(),
            ];
            let radius = distance(at, centre);
            let end = [word(line, 'X').unwrap(), word(line, 'Y').unwrap()];
            let mut replaced = 0;
            loop {
                let original = straight.next().unwrap();
                assert!(original.starts_with("G1 X"), "{line} for {original}");
                let p = [word(original, 'X').unwrap(), word(original, 'Y').unwrap()];
                let off = (distance(p, centre) - radius).abs();
                assert!(off <= tolerance, "{line} for {original}: {off}");
                replaced += 1;
                if p == end {
                    break;
                }
            }
            assert!(replaced >= 3, "{line}");
            at = end;
            checked += 1;
        }
        assert_eq!(straight.next(), None);
        assert!(checked > 0);
    }

    #[test]
    fn straight_walls_get_no_arcs() {
        let mesh = Mesh::cuboid(20.0, 20.0, 3.0);
        // The skirt has round corners.
        let plain = Settings {
            skirt_loops: 0,
            ..Settings::default()
        };
        let arcs = Settings {
            arc_fitting: true,
            ..plain.clone()
        };
        let (a, b) = (slice(&mesh, &plain), slice(&mesh, &arcs));
        assert_eq!(moves_only(&a.gcode), moves_only(&b.gcode));
    }

    /// The G-code of the cubes 10 x 10 mm and `heights` high, one beside
    /// the other `gap` mm apart, centred on the bed together.
    fn cubes(heights: &[f64], gap: f64, settings: &Settings) -> Output {
        let objects: Vec<Mesh> = (heights.iter().enumerate())
            .map(|(k, &h)| {
                let mut mesh = Mesh::cuboid(10.0, 10.0, h);
                mesh.translate([k as f64 * (10.0 + gap), 0.0, 0.0]);
                mesh
            })
            .collect();
        let toolpaths = slicer_fdm::generate_plate(
            &objects,
            slicer_fdm::Placement::Centre,
            settings,
            &mut slicer_core::Diagnostics::new(),
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
        write_gcode(&toolpaths, settings).unwrap()
    }

    /// The lines of `gcode` without comments and object labels.
    fn without_labels(gcode: &str) -> Vec<&str> {
        gcode
            .lines()
            .filter(|l| !l.starts_with(';') && !l.starts_with("EXCLUDE_OBJECT"))
            .filter(|l| !l.starts_with("M486"))
            .collect()
    }

    #[test]
    fn objects_are_labelled_for_the_firmware() {
        let mut settings = Settings::default();
        let plain = cubes(&[2.0, 2.0], 10.0, &settings).gcode;
        assert!(!plain.contains("EXCLUDE_OBJECT") && !plain.contains("M486"));
        settings.label_objects = true;
        settings.flavor = Flavor::Klipper;
        let klipper = cubes(&[2.0, 2.0], 10.0, &settings).gcode;
        // The moves stay the same.
        assert_eq!(without_labels(&klipper), without_labels(&plain));
        let defined: Vec<&str> = (klipper.lines())
            .filter(|l| l.starts_with("EXCLUDE_OBJECT_DEFINE"))
            .collect();
        assert_eq!(defined.len(), 2);
        // The cubes are centred at X 100 and 120 (both 10 mm wide, 10 mm
        // apart, centred on the 220 mm bed together); the outline of the
        // first layer is that of the outer wall.
        assert_eq!(
            defined[0],
            "EXCLUDE_OBJECT_DEFINE NAME=object_1 CENTER=100.000,110.000 POLYGON=[[95.225,105.225],[104.775,105.225],[104.775,114.775],[95.225,114.775]]"
        );
        assert!(defined[1].contains("NAME=object_2 CENTER=120.000,110.000"));
        // On every layer both objects are started and ended in turn.
        let layers: Vec<&str> = klipper.split(";LAYER_CHANGE").skip(1).collect();
        assert_eq!(layers.len(), 10);
        for layer in &layers {
            let labels: Vec<&str> = (layer.lines())
                .filter(|l| l.starts_with("EXCLUDE_OBJECT_"))
                .collect();
            assert!(
                labels.len() >= 4 && labels.len().is_multiple_of(2),
                "{labels:?}"
            );
            for pair in labels.chunks(2) {
                let name = pair[0].strip_prefix("EXCLUDE_OBJECT_START ").unwrap();
                assert_eq!(pair[1], format!("EXCLUDE_OBJECT_END {name}"));
            }
            for name in ["NAME=object_1", "NAME=object_2"] {
                assert!(labels.contains(&format!("EXCLUDE_OBJECT_START {name}").as_str()));
            }
        }

        settings.flavor = Flavor::Marlin;
        let marlin = cubes(&[2.0, 2.0], 10.0, &settings).gcode;
        assert_eq!(without_labels(&marlin), without_labels(&plain));
        assert_eq!(marlin.matches("\nM486 T").count(), 1);
        assert!(marlin.contains("\nM486 T2\n"));
        for layer in marlin.split(";LAYER_CHANGE").skip(1) {
            let labels: Vec<&str> = layer.lines().filter(|l| l.starts_with("M486")).collect();
            for pair in labels.chunks(2) {
                assert!(pair[0] == "M486 S0" || pair[0] == "M486 S1", "{pair:?}");
                assert_eq!(pair[1], "M486 S-1");
            }
        }
    }

    #[test]
    fn sequential_print_finishes_one_object_before_the_next() {
        let mut settings = Settings {
            sequential_print: true,
            label_objects: true,
            flavor: Flavor::Klipper,
            ..Settings::default()
        };
        // The taller cube is given first and printed last.
        let output = cubes(&[1.0, 0.6], 40.0, &settings);
        let gcode = &output.gcode;
        let heights: Vec<&str> = (gcode.lines())
            .filter_map(|l| l.strip_prefix(";Z:"))
            .collect();
        assert_eq!(
            heights,
            [
                "0.200", "0.400", "0.600", "0.200", "0.400", "0.600", "0.800", "1.000"
            ]
        );
        assert_eq!(output.stats.layers, 8);
        // Between the objects: up 2 mm above the first one, across, down.
        let layers: Vec<&str> = gcode.split(";LAYER_CHANGE").collect();
        let start: Vec<&str> = layers[4].lines().filter(|l| l.starts_with('G')).collect();
        assert!(start[0].starts_with("G1 E-0.8"), "{start:?}");
        assert!(start[1].starts_with("G1 Z2.600"), "{start:?}");
        assert!(start[2].starts_with("G0 X"), "{start:?}");
        assert!(start[3].starts_with("G1 Z0.200"), "{start:?}");
        // All of the second cube first, then all of the first.
        let objects = |layers: &[&str]| -> Vec<String> {
            let mut names: Vec<String> = (layers.iter().flat_map(|l| l.lines()))
                .filter_map(|l| l.strip_prefix("EXCLUDE_OBJECT_START "))
                .map(str::to_owned)
                .collect();
            names.dedup();
            names
        };
        assert_eq!(objects(&layers[1..4]), ["NAME=object_2"]);
        assert_eq!(objects(&layers[4..]), ["NAME=object_1"]);
        // The first layer of both is slow and without fan.
        assert!(layers[4].contains(" F1500\n"), "{}", layers[4]);
        assert!(layers[4].contains("\nM106 S0\n"), "{}", layers[4]);

        // Filament as when printed together (skirt aside); the time adds the
        // way between.
        settings.skirt_loops = 0;
        let one_by_one = cubes(&[1.0, 0.6], 40.0, &settings).stats;
        settings.sequential_print = false;
        let together = cubes(&[1.0, 0.6], 40.0, &settings).stats;
        let ratio = one_by_one.filament_length_mm / together.filament_length_mm;
        assert!((ratio - 1.0).abs() < 0.02, "{ratio}");
        assert!(one_by_one.print_time_s > 0.0);
        let sum: f64 = one_by_one.times.iter().map(|(_, t)| t).sum();
        let layers = one_by_one.print_time_s - one_by_one.time_start_end_s;
        assert!((sum - layers).abs() < 1e-6);
    }

    /// A house lying on its side, its smallest corner at `x`, 50 mm: the
    /// rearmost point is the blunt ridge, the sharpest corners are the two
    /// at the front.
    fn house(x: f64) -> Mesh {
        let outline = [
            [0.0, 0.0],
            [20.0, 0.0],
            [20.0, 10.0],
            [10.0, 14.0],
            [0.0, 10.0],
        ];
        let mut mesh = slicer_core::primitives::extrude(&outline, [10.0, 5.0], 4.0, 1, 0.0);
        mesh.translate([x, 50.0, 0.0]);
        mesh
    }

    /// Toolpaths of `objects` where they are, object `i` with `own[i]`.
    fn plate(objects: &[Mesh], own: &[Option<Settings>], settings: &Settings) -> Toolpaths {
        slicer_fdm::generate_plate_with_object_settings(
            objects,
            own,
            slicer_fdm::Placement::AsGiven,
            settings,
            &mut slicer_core::Diagnostics::new(),
            &std::sync::atomic::AtomicBool::new(false),
            &Progress::new(),
        )
        .unwrap()
    }

    /// The feed rates in mm/min of the extruding moves of line type `kind`
    /// above the first layer, left of X = 100 (the first house) and right
    /// of it (the second).
    fn feeds(gcode: &str, kind: &str, first_layer: f64) -> (Vec<f64>, Vec<f64>) {
        let (mut left, mut right) = (Vec::new(), Vec::new());
        let (mut z, mut x, mut feed, mut this) = (0.0, 0.0, 0.0, false);
        for line in gcode.lines() {
            if let Some(height) = line.strip_prefix(";Z:") {
                z = height.parse().unwrap();
            }
            if let Some(name) = line.strip_prefix(";TYPE:") {
                this = name == kind;
            }
            if let Some(f) = word(line, 'F') {
                feed = f;
            }
            if let Some(at) = word(line, 'X') {
                x = at;
            }
            // Moves in X or Y; not the filament pushed back after a retraction.
            let moving = word(line, 'X').is_some() || word(line, 'Y').is_some();
            let extruding = moving && word(line, 'E').is_some_and(|e| e > 0.0);
            if this && extruding && z > first_layer + 1e-6 {
                if x < 100.0 { &mut left } else { &mut right }.push(feed);
            }
        }
        (left, right)
    }

    #[test]
    fn objects_print_at_their_own_speeds_and_seams() {
        let objects = [house(20.0), house(140.0)];
        let own = Settings {
            outer_wall_speed: 15.0,
            seam_position: slicer_profiles::SeamPosition::Corner,
            ..Settings::default()
        };
        for sequential_print in [false, true] {
            let settings = Settings {
                min_layer_time: 0.0,
                fan_layer_time: 0.0,
                sequential_print,
                ..Settings::default()
            };
            let toolpaths = plate(&objects, &[None, Some(own.clone())], &settings);
            // The first house starts its walls at the ridge, the second in a
            // front corner.
            let walls = toolpaths.layers.iter().flat_map(|l| &l.paths);
            let mut seams = 0;
            for path in walls.filter(|p| p.kind == PathKind::OuterWall) {
                let [x, y] = path.points[0];
                if x < 100.0 {
                    assert!(y > 63.0, "{:?}", path.points[0]);
                } else {
                    assert!(y < 51.0, "{:?}", path.points[0]);
                }
                seams += 1;
            }
            assert_eq!(seams, 40);
            let gcode = write_gcode(&toolpaths, &settings).unwrap().gcode;
            let first = settings.first_layer_height;
            let (left, right) = feeds(&gcode, "Outer wall", first);
            assert!(left.len() > 10 && right.len() > 10);
            assert!(left.iter().all(|&f| f == settings.outer_wall_speed * 60.0));
            assert!(right.iter().all(|&f| f == own.outer_wall_speed * 60.0));
            // Its other lines keep the plate's speeds, as does its first
            // layer.
            let (left, right) = feeds(&gcode, "Inner wall", first);
            assert!(!right.is_empty() && left == right);
            assert!(right.iter().all(|&f| f == settings.inner_wall_speed * 60.0));
            let (left, right) = feeds(&gcode, "Outer wall", 0.0);
            assert!(left.contains(&(settings.first_layer_speed * 60.0)));
            assert!(right.contains(&(settings.first_layer_speed * 60.0)));
        }

        // The minimum layer time slows both down alike.
        let settings = Settings {
            min_layer_time: 60.0,
            min_print_speed: 1.0,
            ..Settings::default()
        };
        let toolpaths = plate(&objects, &[None, Some(own.clone())], &settings);
        let gcode = write_gcode(&toolpaths, &settings).unwrap().gcode;
        let (left, right) = feeds(&gcode, "Outer wall", settings.first_layer_height);
        let (left, right) = (left[0], right[0]);
        assert!(left < settings.outer_wall_speed * 60.0, "{left}");
        let ratio = right / left;
        let expected = own.outer_wall_speed / settings.outer_wall_speed;
        assert!((ratio - expected).abs() < 0.01, "{ratio}");
    }

    #[test]
    fn objects_without_own_speeds_print_as_before() {
        let objects = [house(20.0), house(140.0)];
        let settings = Settings::default();
        let plain = plate(&objects, &[], &settings);
        assert!(plain.object_settings.is_empty());
        let plain = write_gcode(&plain, &settings).unwrap().gcode;
        // Own values equal to the plate's, or differing only in keys an
        // object cannot set, are none.
        let elsewhere = Settings {
            first_layer_speed: 5.0,
            travel_speed: 99.0,
            ..settings.clone()
        };
        let own = [Some(settings.clone()), Some(elsewhere)];
        let toolpaths = plate(&objects, &own, &settings);
        assert!(toolpaths.object_settings.is_empty());
        assert_eq!(write_gcode(&toolpaths, &settings).unwrap().gcode, plain);
        // Own speeds change the speeds and nothing else.
        let fast = Settings {
            infill_speed: 120.0,
            bridge_speed: 50.0,
            ..settings.clone()
        };
        let toolpaths = plate(&objects, &[None, Some(fast)], &settings);
        assert_eq!(toolpaths.object_settings.len(), 2);
        let fast = write_gcode(&toolpaths, &settings).unwrap().gcode;
        assert_ne!(fast, plain);
        let moves = |gcode: &str| -> Vec<String> {
            gcode
                .lines()
                .filter(|l| !l.starts_with(';'))
                .map(|l| {
                    let words = l.split_whitespace().filter(|w| !w.starts_with('F'));
                    words.collect::<Vec<_>>().join(" ")
                })
                .filter(|l| l != "G1")
                .collect()
        };
        assert_eq!(moves(&fast), moves(&plain));
    }

    /// The extruding moves of `gcode` (not the filament pushed back after
    /// a retraction): line type, X they end at and filament.
    fn extrusions(gcode: &str) -> Vec<(String, f64, f64)> {
        let (mut kind, mut x) = ("", 0.0);
        let mut moves = Vec::new();
        for line in gcode.lines() {
            if let Some(name) = line.strip_prefix(";TYPE:") {
                kind = name;
            }
            if let Some(at) = word(line, 'X') {
                x = at;
            }
            let moving = word(line, 'X').is_some() || word(line, 'Y').is_some();
            if let Some(e) = word(line, 'E').filter(|&e| moving && e > 0.0) {
                moves.push((kind.to_owned(), x, e));
            }
        }
        moves
    }

    /// Toolpaths of `objects` where they are, object `i` with the height
    /// ranges `ranges[i]`.
    fn plate_with_ranges(
        objects: &[Mesh],
        ranges: &[Vec<slicer_fdm::HeightRange>],
        settings: &Settings,
    ) -> Toolpaths {
        let own = slicer_fdm::OwnSettings {
            ranges,
            ..slicer_fdm::OwnSettings::default()
        };
        slicer_fdm::generate_plate_with_own_settings(
            objects,
            own,
            slicer_fdm::Placement::AsGiven,
            settings,
            &mut slicer_core::Diagnostics::new(),
            &std::sync::atomic::AtomicBool::new(false),
            &Progress::new(),
        )
        .unwrap()
    }

    /// For every layer above the first, whether its middle lies at least
    /// 10 mm up, and the feed rates of its outer and inner walls with the
    /// fan its other lines print with.
    fn walls_and_fan(gcode: &str, first_layer: f64) -> Vec<(bool, Vec<f64>, Vec<f64>, String)> {
        let mut layers: Vec<(bool, Vec<f64>, Vec<f64>, String)> = Vec::new();
        let (mut z, mut kind, mut feed, mut fan) = (0.0, "", 0.0, String::new());
        for line in gcode.lines() {
            if let Some(height) = line.strip_prefix(";Z:") {
                z = height.parse().unwrap();
            }
            if let Some(height) = line.strip_prefix(";HEIGHT:") {
                let height: f64 = height.parse().unwrap();
                if z > first_layer + 1e-6 {
                    layers.push((
                        z - height / 2.0 >= 10.0,
                        Vec::new(),
                        Vec::new(),
                        String::new(),
                    ));
                }
            }
            if line.starts_with("M106") {
                fan = line.to_owned();
            }
            if let Some(name) = line.strip_prefix(";TYPE:") {
                kind = name;
            }
            if let Some(f) = word(line, 'F') {
                feed = f;
            }
            let moving = word(line, 'X').is_some() || word(line, 'Y').is_some();
            let extruding = moving && word(line, 'E').is_some_and(|e| e > 0.0);
            let Some(layer) = layers
                .last_mut()
                .filter(|_| extruding && z > first_layer + 1e-6)
            else {
                continue;
            };
            match kind {
                "Outer wall" => layer.1.push(feed),
                "Inner wall" => layer.2.push(feed),
                _ => {}
            }
            // Bridges and overhangs have fans of their own.
            if ["Bridge", "Overhang wall"].contains(&kind) {
                continue;
            }
            if layer.3.is_empty() {
                layer.3.clone_from(&fan);
            }
            assert_eq!(layer.3, fan, "{line} at {z}");
        }
        layers
    }

    #[test]
    fn height_ranges_print_at_their_own_speeds_and_fan() {
        let mut cube = Mesh::cuboid(20.0, 20.0, 20.0);
        cube.translate([90.0, 90.0, 0.0]);
        let objects = [cube];
        let settings = Settings {
            min_layer_time: 0.0,
            fan_layer_time: 0.0,
            ..Settings::default()
        };
        let mut range = slicer_fdm::HeightRange::new(10.0, 20.0);
        slicer_fdm::apply_range_toml(
            &mut range,
            &settings,
            "outer_wall_speed = 15.0\nfan_speed = 30.0",
        )
        .unwrap();
        let ranges = [vec![range]];
        let plain = write_gcode(&plate(&objects, &[], &settings), &settings)
            .unwrap()
            .gcode;
        // An object without ranges prints as before.
        let without = plate_with_ranges(&objects, &[vec![]], &settings);
        assert_eq!(write_gcode(&without, &settings).unwrap().gcode, plain);

        let toolpaths = plate_with_ranges(&objects, &ranges, &settings);
        let gcode = write_gcode(&toolpaths, &settings).unwrap().gcode;
        let quiet = format!("M106 S{:.0}", 30.0 * 2.55);
        let full = format!("M106 S{:.0}", settings.fan_speed * 2.55);
        let layers = walls_and_fan(&gcode, settings.first_layer_height);
        let inside = layers.iter().filter(|l| l.0).count();
        assert!(inside > 40 && layers.len() - inside > 40, "{inside}");
        for (inside, outer, inner, fan) in &layers {
            let (speed, wanted_fan) = if *inside {
                (15.0, &quiet)
            } else {
                (settings.outer_wall_speed, &full)
            };
            assert!(!outer.is_empty() && outer.iter().all(|&f| f == speed * 60.0));
            assert!(inner.iter().all(|&f| f == settings.inner_wall_speed * 60.0));
            assert_eq!(fan, wanted_fan);
        }
        // The lines are the same as without the range.
        let moves = |gcode: &str| -> Vec<String> {
            gcode
                .lines()
                .filter(|l| !l.starts_with(';') && !l.starts_with("M106"))
                .map(|l| {
                    let words = l.split_whitespace().filter(|w| !w.starts_with('F'));
                    words.collect::<Vec<_>>().join(" ")
                })
                .filter(|l| l != "G1")
                .collect()
        };
        assert_eq!(moves(&gcode), moves(&plain));

        // The minimum layer time slows the walls of the range down like
        // all others: outer and inner walls keep the ratio of their speeds.
        let slow = Settings {
            min_layer_time: 60.0,
            min_print_speed: 1.0,
            ..settings.clone()
        };
        let toolpaths = plate_with_ranges(&objects, &ranges, &slow);
        let gcode = write_gcode(&toolpaths, &slow).unwrap().gcode;
        for (inside, outer, inner, _) in walls_and_fan(&gcode, slow.first_layer_height) {
            let speed = if inside { 15.0 } else { slow.outer_wall_speed };
            let ratio = outer[0] / inner[0];
            let expected = speed / slow.inner_wall_speed;
            assert!((ratio - expected).abs() < 0.01, "{ratio} {expected}");
            assert!(inner[0] < slow.inner_wall_speed * 60.0, "{}", inner[0]);
        }
    }

    #[test]
    fn objects_have_their_own_fan_flow_and_retraction() {
        let objects = [house(20.0), house(140.0)];
        let settings = Settings {
            min_layer_time: 0.0,
            fan_layer_time: 0.0,
            ..Settings::default()
        };
        let own = Settings {
            fan_speed: 30.0,
            flow_ratio: 0.9,
            retraction_length: 2.0,
            z_hop: 0.4,
            ..settings.clone()
        };
        let plain = write_gcode(&plate(&objects, &[], &settings), &settings).unwrap();
        let toolpaths = plate(&objects, &[None, Some(own.clone())], &settings);
        let output = write_gcode(&toolpaths, &settings).unwrap();

        // Walking through the G-code: the fan, the line type, the layer,
        // and whether the line printed last belongs to the second house.
        let quiet = format!("M106 S{:.0}", own.fan_speed * 2.55);
        let full = format!("M106 S{:.0}", settings.fan_speed * 2.55);
        let (mut fan, mut kind, mut x, mut z) = (String::new(), "", 0.0, 0.0);
        let (mut second, mut pulled) = (false, 0.0);
        let (mut hops, mut retractions, mut fans) = (0, [0, 0], [0, 0]);
        for line in output.gcode.lines() {
            if line.starts_with("M106") {
                assert_ne!(line, fan, "the fan is set twice");
                fan = line.to_owned();
            }
            if let Some(name) = line.strip_prefix(";TYPE:") {
                kind = name;
            }
            if let Some(height) = line.strip_prefix(";Z:") {
                z = height.parse().unwrap();
            }
            if let Some(at) = word(line, 'X') {
                x = at;
            }
            let moving = word(line, 'X').is_some() || word(line, 'Y').is_some();
            let e = word(line, 'E');
            if moving && e.is_some_and(|e| e > 0.0) {
                second = x >= 100.0 && kind != "Skirt";
                let plain_fan = !["Skirt", "Bridge", "Overhang wall"].contains(&kind);
                if z > settings.first_layer_height + 1e-6 && plain_fan {
                    assert_eq!(fan, if second { &*quiet } else { &*full }, "{line}");
                    fans[usize::from(second)] += 1;
                }
            }
            // Every retraction is that of the house printed last, and the
            // filament goes forward again by as much.
            if let Some(e) = e.filter(|_| !moving) {
                if e < 0.0 {
                    let length = (if second { &own } else { &settings }).retraction_length;
                    assert!((e + length).abs() < 1e-9, "{line}");
                    pulled = length;
                    retractions[usize::from(second)] += 1;
                } else {
                    assert!((e - pulled).abs() < 1e-9, "{line}");
                }
            }
            // Only the second house lifts the nozzle (the end G-code goes
            // up 10 mm).
            let lift = word(line, 'Z').map_or(0.0, |h| h - z);
            if line.starts_with("G1 Z") && lift > 1e-6 && lift < 1.0 {
                assert!(second, "{line}");
                assert!((lift - own.z_hop).abs() < 1e-6, "{line}");
                hops += 1;
            }
        }
        assert!(fans[0] > 100 && fans[1] > 100, "{fans:?}");
        assert!(retractions[0] > 5 && retractions[1] > 5, "{retractions:?}");
        assert!(hops > 5, "{hops}");

        // The lines are the same, the second house's with 0.9 times the
        // filament.
        let (before, after) = (extrusions(&plain.gcode), extrusions(&output.gcode));
        assert_eq!(before.len(), after.len());
        let mut less = 0.0;
        for ((kind, x, e), (kind_now, x_now, e_now)) in before.iter().zip(&after) {
            assert_eq!((kind, x), (kind_now, x_now));
            if *x >= 100.0 && kind != "Skirt" {
                assert!((e_now - 0.9 * e).abs() < 2e-5, "{e} {e_now}");
                less += 0.1 * e;
            } else {
                assert_eq!(e, e_now);
            }
        }
        assert!(less > 10.0, "{less}");
        // The statistics count the filament as extruded.
        let saved = plain.stats.filament_length_mm - output.stats.filament_length_mm;
        assert!((saved - less).abs() < 0.05, "{saved} {less}");
        let ratio = output.stats.filament_weight_g / output.stats.filament_length_mm;
        assert!(
            (ratio - plain.stats.filament_weight_g / plain.stats.filament_length_mm).abs() < 1e-9
        );
    }

    #[test]
    fn objects_with_their_own_fan_by_layer_time() {
        // The second house turns its fan on later and by layer time.
        let objects = [house(20.0), house(140.0)];
        let settings = Settings {
            min_layer_time: 0.0,
            fan_layer_time: 0.0,
            fan_speed: 40.0,
            ..Settings::default()
        };
        let own = Settings {
            fan_off_layers: 3,
            fan_layer_time: 600.0,
            fan_max_speed: 90.0,
            ..settings.clone()
        };
        let toolpaths = plate(&objects, &[None, Some(own)], &settings);
        let gcode = write_gcode(&toolpaths, &settings).unwrap().gcode;
        let (mut fan, mut layer, mut kind, mut x) = (0.0, 0, "", 0.0);
        let mut seen = Vec::new();
        for line in gcode.lines() {
            if let Some(s) = line.strip_prefix("M106 S") {
                fan = s.parse().unwrap();
            }
            if line.starts_with(";LAYER_CHANGE") {
                layer += 1;
            }
            if let Some(name) = line.strip_prefix(";TYPE:") {
                kind = name;
            }
            if let Some(at) = word(line, 'X') {
                x = at;
            }
            let moving = word(line, 'X').is_some() || word(line, 'Y').is_some();
            let extruding = moving && word(line, 'E').is_some_and(|e| e > 0.0);
            if extruding && !["Skirt", "Bridge", "Overhang wall"].contains(&kind) {
                seen.push((layer, x >= 100.0, fan));
            }
        }
        for &(layer, second, fan) in &seen {
            if (second && layer <= 3) || (!second && layer <= 1) {
                assert_eq!(fan, 0.0, "layer {layer}");
            } else if second {
                // Every layer takes far less than 600 s: close to the most.
                assert!(
                    fan > 40.0 * 2.55 && fan <= 90.0 * 2.55,
                    "layer {layer}: {fan}"
                );
            } else {
                assert_eq!(fan, (40.0f64 * 2.55).round(), "layer {layer}");
            }
        }
        assert!(seen.iter().any(|s| s.1 && s.0 > 3));
    }

    #[test]
    fn support_speed_changes_only_the_feed_of_supports() {
        let mut mesh = Mesh::cuboid(10.0, 10.0, 12.0);
        mesh.translate([105.0, 105.0, 0.0]);
        let mut top = Mesh::cuboid(30.0, 10.0, 2.0);
        top.translate([95.0, 105.0, 10.0]);
        mesh.merge(&top);
        let settings = Settings {
            supports: true,
            min_layer_time: 0.0,
            ..Settings::default()
        };
        let slow = Settings {
            support_speed: 33.0,
            ..settings.clone()
        };
        // Every move: line type, layer above the first, feed rate in effect
        // and the line without it.
        let moves = |gcode: &str| -> Vec<(String, bool, f64, String)> {
            let (mut kind, mut above, mut feed) = ("", false, 0.0);
            let mut moves = Vec::new();
            for line in gcode.lines() {
                if let Some(name) = line.strip_prefix(";TYPE:") {
                    kind = name;
                }
                if let Some(z) = line.strip_prefix(";Z:") {
                    above = z.parse::<f64>().unwrap() > settings.first_layer_height + 1e-6;
                }
                if let Some(f) = word(line, 'F') {
                    feed = f;
                }
                if line.starts_with("G0") || line.starts_with("G1") {
                    let words = line.split_whitespace().filter(|w| !w.starts_with('F'));
                    let text = words.collect::<Vec<_>>().join(" ");
                    moves.push((kind.to_owned(), above, feed, text));
                }
            }
            moves
        };
        let before = moves(&slice(&mesh, &settings).gcode);
        let after = moves(&slice(&mesh, &slow).gcode);
        assert_eq!(before.len(), after.len());
        let mut supports = 0;
        for (b, a) in before.iter().zip(&after) {
            assert_eq!((&b.0, b.1, &b.3), (&a.0, a.1, &a.3));
            let extruding = a.3.starts_with("G1 X") && word(&a.3, 'E').is_some_and(|e| e > 0.0);
            // The interface prints at the speed of supports too.
            if a.0.starts_with("Support") && a.1 && extruding {
                assert_eq!(b.2, settings.infill_speed * 60.0);
                assert_eq!(a.2, slow.support_speed * 60.0);
                supports += 1;
            } else {
                assert_eq!(a.2, b.2, "{}", a.3);
            }
        }
        assert!(supports > 100, "{supports}");
    }

    #[test]
    fn support_interfaces_get_their_own_speed_and_fan() {
        let mut mesh = Mesh::cuboid(10.0, 10.0, 12.0);
        mesh.translate([105.0, 105.0, 0.0]);
        let mut top = Mesh::cuboid(30.0, 10.0, 2.0);
        top.translate([95.0, 105.0, 10.0]);
        mesh.merge(&top);
        let settings = Settings {
            supports: true,
            min_layer_time: 0.0,
            fan_speed: 40.0,
            ..Settings::default()
        };
        let own = Settings {
            support_interface_speed: 15.0,
            support_interface_fan_speed: 100.0,
            ..settings.clone()
        };
        // Every move: line type, layer above the first, feed rate and fan
        // in effect and the line without the feed rate.
        let moves = |gcode: &str| -> Vec<(String, bool, f64, f64, String)> {
            let (mut kind, mut above, mut feed, mut fan) = ("", false, 0.0, 0.0);
            let mut moves = Vec::new();
            for line in gcode.lines() {
                if let Some(name) = line.strip_prefix(";TYPE:") {
                    kind = name;
                }
                if let Some(z) = line.strip_prefix(";Z:") {
                    above = z.parse::<f64>().unwrap() > settings.first_layer_height + 1e-6;
                }
                if line.starts_with("M106") {
                    fan = word(line, 'S').unwrap();
                }
                if let Some(f) = word(line, 'F') {
                    feed = f;
                }
                if line.starts_with("G0") || line.starts_with("G1") {
                    let words = line.split_whitespace().filter(|w| !w.starts_with('F'));
                    let text = words.collect::<Vec<_>>().join(" ");
                    moves.push((kind.to_owned(), above, feed, fan, text));
                }
            }
            moves
        };
        let before = moves(&slice(&mesh, &settings).gcode);
        let after = moves(&slice(&mesh, &own).gcode);
        assert_eq!(before.len(), after.len());
        let (mut interface, mut support) = (0, 0);
        for (b, a) in before.iter().zip(&after) {
            assert_eq!((&b.0, b.1, &b.4), (&a.0, a.1, &a.4));
            let extruding = a.4.starts_with("G1 X") && word(&a.4, 'E').is_some_and(|e| e > 0.0);
            if !(a.1 && extruding) {
                continue;
            }
            if a.0 == "Support interface" {
                assert_eq!(b.2, settings.infill_speed * 60.0);
                assert_eq!(a.2, own.support_interface_speed * 60.0);
                assert_eq!((b.3, a.3), (102.0, 255.0));
                interface += 1;
            } else {
                assert_eq!((a.2, a.3), (b.2, b.3), "{}", a.4);
                support += usize::from(a.0 == "Support");
            }
        }
        assert!(interface > 20 && support > 100, "{interface} {support}");
    }
}
