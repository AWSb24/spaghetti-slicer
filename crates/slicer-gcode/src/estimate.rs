//! Time estimate of what the writer does not move itself: the start, end
//! and tool change G-code, and the waits for the heaters.
//!
//! The custom G-code is read after its placeholders are replaced. Moves
//! (`G0`, `G1`, `G2`, `G3`) go through the same motion model as the
//! layers ([`move_times`], with the limits of the axes and the corners of
//! [`Machine`]), with the feed rate their `F` words give (`travel_speed`
//! before the first one, in tool change G-code the feed rate of the
//! writer); `G4` waits `P` milliseconds or `S` seconds, `G28` takes
//! `homing_time`. The printer comes to rest before every `G4`, `G28` and
//! wait for a heater. Tool change G-code starts where the writer left the
//! nozzle.
//!
//! Heaters follow their targets at `nozzle_heat_rate` and `bed_heat_rate`
//! (°C per second, the same up and down) from `ambient_temp`, also while
//! the printer moves: `M104`/`M140` set a target without waiting, so the
//! heater may be warm already when `M109`/`M190`/`M116` wait for it. A
//! wait with `S` (and `M116`) waits only for heating, one with `R` also
//! for cooling, as the Marlin documentation describes `M109` and `M190`.
//! A rate of 0 leaves that heater out: it is at its target at once.
//!
//! In the layers only temperature commands the writer itself writes
//! count (the temperatures of the second layer and the tool change
//! G-code); they take effect after the moves before them.

use slicer_core::Point;
use slicer_profiles::Settings;

use crate::{Machine, Move, move_times};

/// What G-code [`Estimator::run`] reads, and where its time goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Part {
    /// Start or end G-code: moves, dwells and homing count in
    /// [`Estimator::motion`].
    StartEnd,
    /// Tool change G-code: moves, dwells and homing count in
    /// [`Estimator::toolchange`].
    ToolChange,
    /// Commands of the writer: only temperatures and tool selections
    /// count.
    Silent,
}

/// Pieces an arc (`G2`/`G3`) is divided into for its length and corners.
const ARC_PIECES: usize = 32;

/// One heater of the estimate.
#[derive(Debug, Clone, Copy)]
struct Heater {
    /// Its temperature now, in °C.
    now: f64,
    /// The temperature it is heading for, in °C.
    target: f64,
}

impl Heater {
    /// Lets `seconds` pass at `rate` °C per second (0: at once).
    fn pass(&mut self, seconds: f64, rate: f64) {
        if rate <= 0.0 {
            self.now = self.target;
        } else if self.now < self.target {
            self.now = (self.now + rate * seconds).min(self.target);
        } else {
            self.now = (self.now - rate * seconds).max(self.target);
        }
    }

    /// Seconds until it reaches its target at `rate`; cooling down counts
    /// only with `cooling`.
    fn wait(&self, rate: f64, cooling: bool) -> f64 {
        if rate <= 0.0 {
            return 0.0;
        }
        let gap = if cooling {
            (self.target - self.now).abs()
        } else {
            (self.target - self.now).max(0.0)
        };
        gap / rate
    }
}

/// The state of the printer as far as the estimate needs it.
pub(crate) struct Estimator<'a> {
    settings: &'a Settings,
    machine: Machine,
    /// What is being read.
    part: Part,
    /// The nozzle of every extruder that a command named so far.
    nozzles: Vec<Heater>,
    bed: Heater,
    /// The selected extruder.
    tool: usize,
    /// Position of the nozzle (X, Y, Z) in mm.
    position: [f64; 3],
    /// Whether X, Y and Z are relative (`G91`).
    relative: bool,
    /// Feed rate of moves without `F`, in mm/s.
    feed: f64,
    /// Moves since the printer last came to rest.
    run: Vec<Move>,
    /// Seconds of moves, dwells and homing of the start and end G-code.
    pub(crate) motion: f64,
    /// Seconds of moves, dwells and homing of the tool change G-code.
    pub(crate) toolchange: f64,
    /// Seconds spent waiting for heaters.
    pub(crate) heating: f64,
}

impl<'a> Estimator<'a> {
    /// A cold printer at rest at the origin.
    pub(crate) fn new(settings: &'a Settings) -> Self {
        let cold = Heater {
            now: settings.ambient_temp,
            target: settings.ambient_temp,
        };
        Self {
            settings,
            machine: Machine::of(settings),
            part: Part::Silent,
            nozzles: vec![cold],
            bed: cold,
            tool: 0,
            position: [0.0; 3],
            relative: false,
            feed: settings.travel_speed,
            run: Vec::new(),
            motion: 0.0,
            toolchange: 0.0,
            heating: 0.0,
        }
    }

    /// The feed rate of moves without `F` from now on, in mm/s.
    pub(crate) fn set_feed(&mut self, feed: f64) {
        self.feed = feed;
    }

    /// Lets `seconds` of moves, dwells or homing pass, counted for the
    /// part being read.
    fn spend(&mut self, seconds: f64) {
        match self.part {
            Part::StartEnd => self.motion += seconds,
            Part::ToolChange => self.toolchange += seconds,
            Part::Silent => {}
        }
        self.pass(seconds);
    }

    /// Lets `seconds` pass for the heaters.
    pub(crate) fn pass(&mut self, seconds: f64) {
        let rate = self.settings.nozzle_heat_rate;
        for nozzle in &mut self.nozzles {
            nozzle.pass(seconds, rate);
        }
        self.bed.pass(seconds, self.settings.bed_heat_rate);
    }

    /// Where the writer left the nozzle (`at`, if known, and `z`), with
    /// absolute positioning as the writer uses it.
    pub(crate) fn resume(&mut self, at: Option<Point>, z: f64) {
        if let Some([x, y]) = at {
            self.position = [x, y, z];
        } else {
            self.position[2] = z;
        }
        self.relative = false;
    }

    /// Reads G-code `text` of `part`. Its moves, dwells and homing take
    /// time ([`Estimator::motion`], [`Estimator::toolchange`]) unless it is
    /// [`Part::Silent`]; temperature commands and tool selections always
    /// count.
    // ponytail: the travel of the writer back from where the tool change
    // G-code left the nozzle is not timed (the writer does not know where
    // that is).
    pub(crate) fn run(&mut self, text: &str, part: Part) {
        self.part = part;
        let count = part != Part::Silent;
        for line in text.lines() {
            let code = line.split(';').next().unwrap_or_default().trim();
            let mut words = code.split_whitespace().filter_map(word);
            let Some((letter, number)) = words.next() else {
                continue;
            };
            let words: Vec<(char, f64)> = words.collect();
            let value = |l: char| words.iter().find(|(w, _)| *w == l).map(|&(_, v)| v);
            let is = |n: f64| number == n;
            match letter {
                'T' => self.select(number),
                'G' if is(0.0) || is(1.0) => {
                    if count {
                        self.line(&value);
                    }
                }
                'G' if is(2.0) || is(3.0) => {
                    if count {
                        self.arc(&value, is(3.0));
                    }
                }
                'G' if is(4.0) => {
                    if count {
                        let seconds =
                            value('P').map_or(0.0, |p| p / 1000.0) + value('S').unwrap_or(0.0);
                        self.rest();
                        self.spend(seconds.max(0.0));
                    }
                }
                'G' if is(28.0) => {
                    if count {
                        self.rest();
                        self.spend(self.settings.homing_time);
                    }
                    let all = !['X', 'Y', 'Z'].iter().any(|&a| value(a).is_some());
                    for (axis, name) in ['X', 'Y', 'Z'].iter().enumerate() {
                        if all || value(*name).is_some() {
                            self.position[axis] = 0.0;
                        }
                    }
                }
                'G' if is(90.0) => self.relative = false,
                'G' if is(91.0) => self.relative = true,
                'G' if is(92.0) => {
                    for (axis, name) in ['X', 'Y', 'Z'].iter().enumerate() {
                        if let Some(v) = value(*name) {
                            self.position[axis] = v;
                        }
                    }
                }
                'M' if is(104.0) || is(109.0) => {
                    let tool = value('T').map_or(self.tool, |t| t.max(0.0) as usize);
                    let target = value('S').or(value('R'));
                    let nozzle = self.nozzle(tool);
                    if let Some(target) = target {
                        nozzle.target = target;
                    }
                    if is(109.0) {
                        let cooling = value('R').is_some();
                        let rate = self.settings.nozzle_heat_rate;
                        let seconds = self.nozzle(tool).wait(rate, cooling);
                        self.wait(seconds);
                        let nozzle = self.nozzle(tool);
                        if cooling || nozzle.now < nozzle.target {
                            nozzle.now = nozzle.target;
                        }
                    }
                }
                'M' if is(140.0) || is(190.0) => {
                    if let Some(target) = value('S').or(value('R')) {
                        self.bed.target = target;
                    }
                    if is(190.0) {
                        let cooling = value('R').is_some();
                        let seconds = self.bed.wait(self.settings.bed_heat_rate, cooling);
                        self.wait(seconds);
                        if cooling || self.bed.now < self.bed.target {
                            self.bed.now = self.bed.target;
                        }
                    }
                }
                'M' if is(116.0) => {
                    let rate = self.settings.nozzle_heat_rate;
                    let seconds = self
                        .nozzles
                        .iter()
                        .map(|n| n.wait(rate, false))
                        .fold(self.bed.wait(self.settings.bed_heat_rate, false), f64::max);
                    self.wait(seconds);
                }
                _ => {}
            }
        }
        self.rest();
    }

    /// The nozzle of extruder `tool`, added cold if no command named it yet.
    fn nozzle(&mut self, tool: usize) -> &mut Heater {
        // ponytail: at most 64 nozzles; higher numbers share the last.
        let tool = tool.min(63);
        while self.nozzles.len() <= tool {
            let ambient = self.settings.ambient_temp;
            self.nozzles.push(Heater {
                now: ambient,
                target: ambient,
            });
        }
        &mut self.nozzles[tool]
    }

    /// `T<n>`: selects extruder `n`.
    fn select(&mut self, number: f64) {
        if number >= 0.0 {
            self.tool = number as usize;
            self.nozzle(self.tool);
        }
    }

    /// Comes to rest and waits `seconds` for a heater.
    fn wait(&mut self, seconds: f64) {
        self.rest();
        self.heating += seconds;
        self.pass(seconds);
    }

    /// Finishes the moves so far: the printer comes to rest.
    fn rest(&mut self) {
        if self.run.is_empty() {
            return;
        }
        let seconds: f64 = move_times(&self.run, &self.machine).iter().sum();
        self.run.clear();
        self.spend(seconds);
    }

    /// Takes the feed rate of an `F` word and returns where the move goes.
    fn target(&mut self, value: &dyn Fn(char) -> Option<f64>) -> [f64; 3] {
        if let Some(feed) = value('F').filter(|&f| f > 0.0) {
            self.feed = feed / 60.0;
        }
        let mut to = self.position;
        for (axis, name) in ['X', 'Y', 'Z'].iter().enumerate() {
            if let Some(v) = value(*name) {
                to[axis] = if self.relative { to[axis] + v } else { v };
            }
        }
        to
    }

    /// Adds a straight move from the position to `to`, or a move of the
    /// filament only of `e` mm.
    fn push(&mut self, to: [f64; 3], e: f64) {
        let [dx, dy, dz] = [0, 1, 2].map(|i| to[i] - self.position[i]);
        let flat = (dx * dx + dy * dy).sqrt();
        let length = (flat * flat + dz * dz).sqrt();
        let (length, direction, axes) = if length > 0.0 {
            let direction = if flat > 0.0 {
                [dx / flat, dy / flat]
            } else {
                [0.0, 0.0]
            };
            let axes = [dx / length, dy / length, dz / length, e / length];
            (length, direction, axes)
        } else {
            let alone = if e < 0.0 { -1.0 } else { 1.0 };
            (e.abs(), [0.0, 0.0], [0.0, 0.0, 0.0, alone])
        };
        self.position = to;
        if length > 0.0 {
            self.run.push(Move {
                direction,
                axes,
                length,
                speed: self.feed,
                tool: self.tool,
                kind: None,
            });
        }
    }

    /// `G0`/`G1`.
    fn line(&mut self, value: &dyn Fn(char) -> Option<f64>) {
        let to = self.target(value);
        // Relative or absolute, the filament moves this far at most; with
        // absolute extrusion the writer starts every layer at 0, and custom
        // G-code moving only the filament is a retraction or a purge.
        // ponytail: an absolute `E` counts as the distance from 0.
        let e = value('E').unwrap_or(0.0);
        self.push(to, e);
    }

    /// `G2` (clockwise) or `G3` (`counter`-clockwise), around the centre
    /// `I`/`J` relative to the start or with radius `R`, divided into
    /// [`ARC_PIECES`] straight pieces; Z rises evenly.
    fn arc(&mut self, value: &dyn Fn(char) -> Option<f64>, counter: bool) {
        let from = self.position;
        let to = self.target(value);
        let centre = match (value('I'), value('J'), value('R')) {
            (None, None, Some(radius)) => {
                let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
                let chord = (dx * dx + dy * dy).sqrt();
                let half = chord / 2.0;
                if chord <= 0.0 || radius.abs() < half {
                    None
                } else {
                    let h = (radius * radius - half * half).sqrt();
                    // Left of the chord for a short counter-clockwise arc.
                    let side = if counter == (radius > 0.0) { h } else { -h };
                    Some([
                        from[0] + dx / 2.0 - dy / chord * side,
                        from[1] + dy / 2.0 + dx / chord * side,
                    ])
                }
            }
            (i, j, _) if i.is_some() || j.is_some() => {
                Some([from[0] + i.unwrap_or(0.0), from[1] + j.unwrap_or(0.0)])
            }
            _ => None,
        };
        let Some(centre) = centre else {
            self.push(to, 0.0);
            return;
        };
        let a = [from[0] - centre[0], from[1] - centre[1]];
        let b = [to[0] - centre[0], to[1] - centre[1]];
        let radius = (a[0] * a[0] + a[1] * a[1]).sqrt();
        if radius <= 0.0 {
            self.push(to, 0.0);
            return;
        }
        let mut points = vec![a, b];
        while points.len() <= ARC_PIECES {
            let mut finer = Vec::with_capacity(points.len() * 2);
            for pair in points.windows(2) {
                finer.push(pair[0]);
                finer.push(arc_middle(pair[0], pair[1], radius, counter));
            }
            finer.push(b);
            points = finer;
        }
        let pieces = points.len() - 1;
        for (index, p) in points.iter().enumerate().skip(1) {
            let z = from[2] + (to[2] - from[2]) * index as f64 / pieces as f64;
            let at = if index == pieces {
                to
            } else {
                [centre[0] + p[0], centre[1] + p[1], z]
            };
            self.push(at, 0.0);
        }
    }
}

/// The middle of the arc from `a` to `b` (relative to the centre, `a`
/// `radius` from it) going `counter`-clockwise or clockwise; the opposite
/// point of `a` when `a` and `b` are the same (a full circle).
fn arc_middle(a: Point, b: Point, radius: f64, counter: bool) -> Point {
    let cross = a[0] * b[1] - a[1] * b[0];
    let m = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    let length = (m[0] * m[0] + m[1] * m[1]).sqrt();
    if length <= radius * 1e-9 {
        // Half a circle: a quarter turn from `a`.
        return if counter {
            [-a[1], a[0]]
        } else {
            [a[1], -a[0]]
        };
    }
    let middle = [m[0] * radius / length, m[1] * radius / length];
    let short = if counter { cross > 0.0 } else { cross < 0.0 };
    if short {
        middle
    } else {
        [-middle[0], -middle[1]]
    }
}

/// A word of G-code such as `X12.5`: its letter in upper case and number.
fn word(text: &str) -> Option<(char, f64)> {
    let mut chars = text.chars();
    let letter = chars.next()?.to_ascii_uppercase();
    if !letter.is_ascii_alphabetic() {
        return None;
    }
    let number = chars.as_str().parse::<f64>().ok()?;
    Some((letter, number))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings {
            acceleration: 1.0e9,
            ..Settings::default()
        }
    }

    fn time(text: &str, settings: &Settings) -> (f64, f64) {
        let mut estimate = Estimator::new(settings);
        estimate.run(text, Part::StartEnd);
        (estimate.motion, estimate.heating)
    }

    #[test]
    fn moves_dwells_and_homing_take_their_time() {
        let settings = settings();
        // 60 mm at 10 mm/s, nearly without acceleration.
        let (motion, _) = time("G1 X60 F600", &settings);
        assert!((motion - 6.0).abs() < 1e-3, "{motion}");
        let (motion, _) = time("G4 P2000\nG4 S1\nG28\n", &settings);
        assert!(
            (motion - 3.0 - settings.homing_time).abs() < 1e-9,
            "{motion}"
        );
        // Relative moves add up; a move of the filament only takes its length.
        let (motion, _) = time("G91\nG1 Z10 F600\nG1 Z10\nG1 E5 F300", &settings);
        assert!((motion - 3.0).abs() < 1e-3, "{motion}");
        // Comments, unknown commands and lower case.
        let (motion, _) = time("; G1 X100\nM117 hello\ng1 x30 f600 ; go", &settings);
        assert!((motion - 3.0).abs() < 1e-3, "{motion}");
    }

    #[test]
    fn arcs_take_the_length_of_the_arc() {
        let settings = settings();
        let pi = std::f64::consts::PI;
        // Half a circle of radius 10 counter-clockwise from (10, 0) to (-10, 0).
        let (motion, _) = time("G1 X10 Y0 F6000\nG3 X-10 Y0 I-10 J0 F600", &settings);
        let half = pi * 10.0 / 10.0;
        assert!((motion - 0.1 - half).abs() < 0.01, "{motion}");
        // A full circle.
        let (motion, _) = time("G1 X10 Y0 F6000\nG2 X10 Y0 I-10 J0 F600", &settings);
        assert!((motion - 0.1 - 2.0 * half).abs() < 0.03, "{motion}");
        // A quarter by radius, the short way and the long way.
        let (short, _) = time("G1 X10 F6000\nG3 X0 Y10 R10 F600", &settings);
        let (long, _) = time("G1 X10 F6000\nG3 X0 Y10 R-10 F600", &settings);
        assert!((short - 0.1 - half / 2.0).abs() < 0.01, "{short}");
        assert!((long - 0.1 - 3.0 * half / 2.0).abs() < 0.02, "{long}");
    }

    #[test]
    fn heating_counts_only_when_waited_for() {
        let rates = Settings {
            nozzle_heat_rate: 2.0,
            bed_heat_rate: 1.0,
            ..settings()
        };
        // From 25 to 60 °C at 1 °C/s.
        let (_, heating) = time("M190 S60", &rates);
        assert!((heating - 35.0).abs() < 1e-9, "{heating}");
        // Setting without waiting takes no time.
        assert_eq!(time("M104 S215\nM140 S60", &rates).1, 0.0);
        // The bed heats while the printer homes; the nozzle while the bed
        // is waited for, then the rest.
        let (motion, heating) = time("M140 S60\nM104 S215\nG28\nM190 S60\nM109 S215", &rates);
        assert_eq!(motion, rates.homing_time);
        let bed = 35.0 - rates.homing_time;
        let nozzle = (215.0 - 25.0) / 2.0 - rates.homing_time - bed;
        assert!((heating - bed - nozzle).abs() < 1e-9, "{heating}");
        // `S` does not wait for cooling, `R` does.
        let (_, heating) = time("M109 S200\nM109 S180", &rates);
        assert!((heating - 87.5).abs() < 1e-9, "{heating}");
        let (_, heating) = time("M109 S200\nM109 R180", &rates);
        assert!((heating - 97.5).abs() < 1e-9, "{heating}");
        // Every extruder has its own nozzle.
        let (_, heating) = time("M109 S205\nT1\nM109 S205\nM109 T0 S205", &rates);
        assert!((heating - 180.0).abs() < 1e-9, "{heating}");
        // `M116` waits for all.
        let (_, heating) = time("M104 S125\nM140 S45\nM116", &rates);
        assert!((heating - 50.0).abs() < 1e-9, "{heating}");
        // Without rates heating is left out.
        assert_eq!(time("M190 S60\nM109 S215", &settings()).1, 0.0);
    }

    #[test]
    fn without_count_only_heaters_and_tools_count() {
        let rates = Settings {
            nozzle_heat_rate: 2.0,
            ..settings()
        };
        let mut estimate = Estimator::new(&rates);
        estimate.run("G1 X100 F600\nG4 S5\nG28\nT1\nM109 S45", Part::Silent);
        assert_eq!(estimate.motion, 0.0);
        assert_eq!(estimate.tool, 1);
        assert!((estimate.heating - 10.0).abs() < 1e-9);
        // Time passing heats too.
        estimate.run("M104 T0 S45", Part::Silent);
        estimate.pass(4.0);
        estimate.run("M109 T0 S45", Part::Silent);
        assert!(
            (estimate.heating - 16.0).abs() < 1e-9,
            "{}",
            estimate.heating
        );
    }
}
