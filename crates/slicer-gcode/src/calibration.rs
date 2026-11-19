//! Calibration prints: test models, and the changes to their G-code that
//! make each part of a model print with its own value.
//!
//! - Temperature tower: blocks stacked on each other, one per nozzle
//!   temperature. `M104` sets the temperature at the first layer of each
//!   block.
//! - Retraction test: two towers side by side, so that the nozzle travels
//!   between them on every layer, in bands of height with a retraction
//!   length each.
//! - Flow test: small flat blocks next to each other, each printed with its
//!   own share of the flow ratio.
//! - Pressure advance tower: a hollow tower whose pressure advance factor
//!   changes in bands of height (`M900 K` for Marlin,
//!   `SET_PRESSURE_ADVANCE` for Klipper at the first layer of each band).
//!
//! Every value is lettered on the model where it applies, in raised
//! digits of a small font of bars ([`text`]): on the front of each block
//! of the temperature tower, on a tab beside each retraction tower (odd
//! bands on the left one, even bands on the right one, counted from 1),
//! on top of each flow block near its front edge, and on the pressure
//! advance tower alternately on its front and its right side. The
//! lettering stays within the height of a tower, so the bands stay where
//! they are.
//!
//! The models are sliced like any others. [`Calibration::apply`] then
//! changes the written G-code: it puts the temperature command after the
//! layer change comments and scales the filament amounts of retractions, or
//! of the extrusions inside a flow block. The writer itself stays as it is,
//! so the G-code of every other print is unchanged.
//!
//! Own design.

use std::fmt;

use slicer_core::{Diagnose, Mesh, Point};
use slicer_profiles::{Flavor, Settings};

use crate::Output;

mod text;

use text::{Face, Style};

/// Most values one calibration print steps through.
pub const MAX_STEPS: usize = 20;

/// Width, depth and height of one block of the temperature tower, in mm.
const TOWER_BLOCK: [f64; 3] = [20.0, 10.0, 10.0];

/// Width, depth and height of one band of a retraction tower, in mm.
const RETRACTION_BAND: [f64; 3] = [8.0, 8.0, 5.0];

/// Distance between the two retraction towers, in mm: long enough for the
/// nozzle to ooze on the way.
const RETRACTION_GAP: f64 = 30.0;

/// Width, depth and height of one flow block, in mm.
const FLOW_BLOCK: [f64; 3] = [20.0, 20.0, 3.0];

/// Width, depth and height of one band of the pressure advance tower, in
/// mm. The tower is hollow, so that its walls print without pauses.
const PRESSURE_BAND: [f64; 3] = [30.0, 30.0, 4.0];

/// Depth of the groove that marks where a block of a tower ends, in mm. The
/// groove is as high as it is deep: a flat ledge below, a 45° slope above,
/// so it prints without supports.
const GROOVE: f64 = 1.0;

/// Lettering on the front of the temperature tower and on the tabs of the
/// retraction towers, in mm. The bars are at least two line widths wide,
/// and stand out little enough that their sloped undersides print.
const TOWER_TEXT: Style = Style {
    height: 6.0,
    width: 3.6,
    upright: 1.0,
    cross: 1.0,
    spacing: 1.2,
    raise: 0.6,
};

/// Lettering on the pressure advance tower, whose bands are only 4 mm
/// high: smaller, with thinner crossing bars (on an upright face they are
/// a few layers each, not lines side by side).
const PRESSURE_TEXT: Style = Style {
    height: 5.0,
    width: 3.4,
    upright: 1.0,
    cross: 0.8,
    spacing: 1.2,
    raise: 0.6,
};

/// Lettering on top of the flow blocks.
const FLOW_TEXT: Style = Style {
    height: 5.0,
    width: 3.4,
    upright: 1.0,
    cross: 1.0,
    spacing: 1.2,
    raise: 0.6,
};

/// Space between lettering and the bed, the top of a tower or the sides
/// of the face it is on, in mm.
const LABEL_MARGIN: f64 = 1.0;

/// Distance of the lettering on a flow block from its front edge, in mm.
const FLOW_TEXT_FRONT: f64 = 2.0;

/// Depth of the tab beside a retraction tower that carries the lettering,
/// in mm.
const TAB_DEPTH: f64 = 2.0;

/// How far a tab reaches into its tower, in mm: more than a groove is
/// deep.
const TAB_OVERLAP: f64 = 1.5;

/// The front face of a model at `centre`: read along X, out towards -Y.
fn front(centre: [f64; 3]) -> Face {
    Face {
        centre,
        across: [1.0, 0.0, 0.0],
        up: [0.0, 0.0, 1.0],
        out: [0.0, -1.0, 0.0],
    }
}

/// The kind of calibration print.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Temperature tower: one block per nozzle temperature.
    Temperature,
    /// Two towers with a retraction length per band of height.
    Retraction,
    /// Flat blocks, each with its own flow.
    Flow,
    /// A hollow tower with a pressure advance factor per band of height.
    PressureAdvance,
}

impl Kind {
    /// All kinds, in menu order.
    pub const ALL: [Kind; 4] = [
        Kind::Temperature,
        Kind::Retraction,
        Kind::Flow,
        Kind::PressureAdvance,
    ];

    /// Name on the command line.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Temperature => "temperature",
            Kind::Retraction => "retraction",
            Kind::Flow => "flow",
            Kind::PressureAdvance => "pressure-advance",
        }
    }

    /// The kind with the command line name `name`.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }

    /// Title for menus and messages.
    pub fn title(self) -> &'static str {
        match self {
            Kind::Temperature => "Temperature tower",
            Kind::Retraction => "Retraction test",
            Kind::Flow => "Flow rate test",
            Kind::PressureAdvance => "Pressure advance tower",
        }
    }

    /// Unit of the values: °C, mm, % of the flow ratio, or none for the
    /// pressure advance factor.
    pub fn unit(self) -> &'static str {
        match self {
            Kind::Temperature => "°C",
            Kind::Retraction => "mm",
            Kind::Flow => "%",
            Kind::PressureAdvance => "",
        }
    }

    /// Start, end and step that suit `settings`: temperatures 15 °C around
    /// the nozzle temperature in steps of 5, retraction lengths for a
    /// direct drive or a Bowden extruder (told apart by the retraction
    /// length in use), flow from -10 % to +10 % in steps of 5, pressure
    /// advance from 0 up to a value that suits a direct drive or a Bowden
    /// extruder (told apart the same way).
    pub fn default_range(self, settings: &Settings) -> [f64; 3] {
        match self {
            Kind::Temperature => {
                let middle = (settings.nozzle_temp / 5.0).round() * 5.0;
                [middle + 15.0, middle - 15.0, 5.0]
            }
            Kind::Retraction if settings.retraction_length <= 2.0 => [0.2, 2.0, 0.2],
            Kind::Retraction => [1.0, 6.0, 0.5],
            Kind::Flow => [-10.0, 10.0, 5.0],
            Kind::PressureAdvance if settings.retraction_length <= 2.0 => [0.0, 0.08, 0.005],
            Kind::PressureAdvance => [0.0, 0.8, 0.05],
        }
    }

    /// Smallest and largest value allowed.
    fn limits(self) -> (f64, f64) {
        match self {
            Kind::Temperature => (100.0, 450.0),
            Kind::Retraction => (0.0, 20.0),
            Kind::Flow => (-50.0, 100.0),
            Kind::PressureAdvance => (0.0, 2.0),
        }
    }
}

/// Why a calibration print cannot be made from the values given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalibrationError(pub String);

impl fmt::Display for CalibrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CalibrationError {}

impl Diagnose for CalibrationError {
    fn code(&self) -> &'static str {
        "calibration.invalid_range"
    }
}

/// A value lettered on a test model.
#[derive(Debug, Clone, PartialEq)]
struct Label {
    /// Index of the model in [`Calibration::models`].
    model: usize,
    text: String,
    style: Style,
    face: Face,
}

/// A calibration print: its kind and the values it steps through, from the
/// bottom up (towers) or one per block (flow).
#[derive(Debug, Clone, PartialEq)]
pub struct Calibration {
    kind: Kind,
    values: Vec<f64>,
}

/// `value` without the noise of adding up decimal steps.
fn tidy(value: f64) -> f64 {
    (value * 1e6).round() / 1e6 + 0.0
}

impl Calibration {
    /// Steps from `start` to `end` (up or down) by `step`; `end` is the
    /// last value if the step divides the range. Fails if the step is not
    /// positive, a value lies outside what the kind allows, there would be
    /// more than [`MAX_STEPS`] values, or (retraction) all lengths are 0.
    pub fn new(kind: Kind, start: f64, end: f64, step: f64) -> Result<Self, CalibrationError> {
        if !(start.is_finite() && end.is_finite() && step.is_finite() && step > 0.0) {
            return Err(CalibrationError(
                "start, end and step must be numbers, and the step above 0".to_owned(),
            ));
        }
        let count = ((end - start).abs() / step + 1e-9).floor() + 1.0;
        if count > MAX_STEPS as f64 {
            return Err(CalibrationError(format!(
                "{} values from {start} to {end} in steps of {step}; at most {MAX_STEPS} fit",
                count
            )));
        }
        let direction = if end < start { -1.0 } else { 1.0 };
        let values: Vec<f64> = (0..count as usize)
            .map(|i| tidy(start + direction * step * i as f64))
            .collect();
        let (lo, hi) = kind.limits();
        if values.iter().any(|&v| v < lo || v > hi) {
            return Err(CalibrationError(format!(
                "{} values must lie between {lo} and {hi} {}",
                kind.title(),
                kind.unit()
            )));
        }
        if kind == Kind::Retraction && values.iter().all(|&v| v <= 0.0) {
            return Err(CalibrationError(
                "at least one retraction length must be above 0".to_owned(),
            ));
        }
        Ok(Self { kind, values })
    }

    /// The kind of print.
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// The values, from the bottom up (towers) or one per block (flow).
    pub fn values(&self) -> &[f64] {
        &self.values
    }

    /// A value with its unit, e.g. `215 °C`, `0.8 mm` or `-5 %`.
    pub fn value_text(&self, value: f64) -> String {
        match self.kind {
            Kind::Flow => format!("{value:+} %"),
            Kind::PressureAdvance => format!("K {value}"),
            kind => format!("{value} {}", kind.unit()),
        }
    }

    /// One line on what the print shows, for messages.
    pub fn summary(&self) -> String {
        let count = self.values.len();
        let (first, last) = match (self.values.first(), self.values.last()) {
            (Some(&first), Some(&last)) => (self.value_text(first), self.value_text(last)),
            _ => return self.kind.title().to_owned(),
        };
        match self.kind {
            Kind::Temperature => format!(
                "{count} blocks of {} mm, {first} at the bottom to {last} at the top",
                TOWER_BLOCK[2]
            ),
            Kind::Retraction => format!(
                "two towers in {count} bands of {} mm, retraction {first} at the bottom to {last} at the top",
                RETRACTION_BAND[2]
            ),
            Kind::Flow => format!(
                "{count} blocks, flow {first} to {last} of the flow ratio, left to right from the front row"
            ),
            Kind::PressureAdvance => format!(
                "a hollow tower in {count} bands of {} mm, {first} at the bottom to {last} at the top",
                PRESSURE_BAND[2]
            ),
        }
    }

    /// The test models with their names, standing on the bed (Z = 0)
    /// with the body of the first one at the origin; lettering and the
    /// tabs of the retraction towers may reach to negative X and Y. For
    /// the flow test, model `i` prints with value `i`.
    pub fn models(&self) -> Vec<(String, Mesh)> {
        let count = self.values.len();
        let mut models: Vec<(String, Mesh)> = match self.kind {
            Kind::Temperature => vec![(
                Kind::Temperature.title().to_owned(),
                stacked_blocks(count, TOWER_BLOCK),
            )],
            Kind::Retraction => (1..=2)
                .map(|i| {
                    (
                        format!("Retraction tower {i}"),
                        stacked_blocks(count, RETRACTION_BAND),
                    )
                })
                .collect(),
            Kind::Flow => self
                .values
                .iter()
                .map(|&value| {
                    let [x, y, z] = FLOW_BLOCK;
                    (
                        format!("Flow {}", self.value_text(value)),
                        Mesh::cuboid(x, y, z),
                    )
                })
                .collect(),
            Kind::PressureAdvance => {
                let [x, y, z] = PRESSURE_BAND;
                vec![(
                    Kind::PressureAdvance.title().to_owned(),
                    Mesh::cuboid(x, y, z * count as f64),
                )]
            }
        };
        if self.kind == Kind::Retraction {
            // A tab on the outer side of each tower that gets lettering.
            let [width, _, height] = RETRACTION_BAND;
            let tab = self.tab_width();
            for (i, (_, mesh)) in models.iter_mut().enumerate().take(count) {
                let mut piece = Mesh::cuboid(tab + TAB_OVERLAP, TAB_DEPTH, height * count as f64);
                let x = if i == 0 { -tab } else { width - TAB_OVERLAP };
                piece.translate([x, 0.0, 0.0]);
                mesh.merge(&piece);
            }
        }
        for label in self.labels() {
            if let Some((_, mesh)) = models.get_mut(label.model) {
                mesh.merge(&text::raised(&label.text, &label.style, &label.face));
            }
        }
        models
    }

    /// A value as lettered on the model: the number alone, the flow with
    /// its sign.
    fn label_text(&self, value: f64) -> String {
        match self.kind {
            Kind::Flow => format!("{value:+}"),
            _ => value.to_string(),
        }
    }

    /// Width of the tabs beside the retraction towers: the widest value
    /// and a margin on either side.
    fn tab_width(&self) -> f64 {
        let widest = (self.values.iter())
            .map(|&value| text::width(&self.label_text(value), &TOWER_TEXT))
            .fold(0.0, f64::max);
        widest + 2.0 * LABEL_MARGIN
    }

    /// Where each value is lettered, in the coordinates of
    /// [`Calibration::models`].
    fn labels(&self) -> Vec<Label> {
        let count = self.values.len() as f64;
        let texts = (self.values.iter())
            .map(|&value| self.label_text(value))
            .enumerate();
        // Height of the middle of the lettering for band `band` of a tower
        // in bands of `pitch`: the middle of the band below its groove,
        // but with the whole lettering above the bed and below the top.
        let level = |band: usize, pitch: f64, groove: f64, style: &Style| {
            let half = style.height / 2.0 + LABEL_MARGIN;
            let middle = band as f64 * pitch + (pitch - groove) / 2.0;
            middle.max(half).min(count * pitch - half)
        };
        let room = |width: f64| width - 2.0 * LABEL_MARGIN;
        match self.kind {
            Kind::Temperature => {
                let [width, _, height] = TOWER_BLOCK;
                texts
                    .map(|(band, text)| Label {
                        model: 0,
                        style: TOWER_TEXT.fitting(&text, room(width)),
                        face: front([width / 2.0, 0.0, level(band, height, GROOVE, &TOWER_TEXT)]),
                        text,
                    })
                    .collect()
            }
            Kind::Retraction => {
                let [width, _, height] = RETRACTION_BAND;
                let tab = self.tab_width();
                texts
                    .map(|(band, text)| {
                        let x = if band % 2 == 0 {
                            -tab / 2.0
                        } else {
                            width + tab / 2.0
                        };
                        let z = level(band, height, GROOVE, &TOWER_TEXT);
                        Label {
                            model: band % 2,
                            style: TOWER_TEXT,
                            face: front([x, 0.0, z]),
                            text,
                        }
                    })
                    .collect()
            }
            Kind::Flow => {
                let [width, _, height] = FLOW_BLOCK;
                texts
                    .map(|(block, text)| {
                        let style = FLOW_TEXT.fitting(&text, room(width));
                        let y = FLOW_TEXT_FRONT + style.height / 2.0;
                        Label {
                            model: block,
                            style,
                            face: Face {
                                centre: [width / 2.0, y, height],
                                across: [1.0, 0.0, 0.0],
                                up: [0.0, 1.0, 0.0],
                                out: [0.0, 0.0, 1.0],
                            },
                            text,
                        }
                    })
                    .collect()
            }
            Kind::PressureAdvance => {
                let [width, depth, height] = PRESSURE_BAND;
                texts
                    .map(|(band, text)| {
                        let z = level(band, height, 0.0, &PRESSURE_TEXT);
                        let face = if band % 2 == 0 {
                            front([width / 2.0, 0.0, z])
                        } else {
                            Face {
                                centre: [width, depth / 2.0, z],
                                across: [0.0, 1.0, 0.0],
                                up: [0.0, 0.0, 1.0],
                                out: [1.0, 0.0, 0.0],
                            }
                        };
                        Label {
                            model: 0,
                            style: PRESSURE_TEXT.fitting(&text, room(width.min(depth))),
                            face,
                            text,
                        }
                    })
                    .collect()
            }
        }
    }

    /// The test models arranged side by side in the middle of the bed of
    /// `settings`, in bed coordinates. The flow blocks keep their order:
    /// left to right, front row first.
    pub fn placed_models(&self, settings: &Settings) -> Vec<(String, Mesh)> {
        let mut models = self.models();
        let size = |mesh: &Mesh| {
            mesh.bounds()
                .map_or([0.0, 0.0], |(lo, hi)| [hi[0] - lo[0], hi[1] - lo[1]])
        };
        let sizes: Vec<[f64; 2]> = models.iter().map(|(_, mesh)| size(mesh)).collect();
        let gap = match self.kind {
            Kind::Retraction => RETRACTION_GAP,
            _ => slicer_fdm::ARRANGE_GAP,
        };
        let corners = slicer_fdm::arrange(&sizes, settings, gap);
        for ((_, mesh), corner) in models.iter_mut().zip(corners) {
            if let Some((lo, _)) = mesh.bounds() {
                mesh.translate([corner[0] - lo[0], corner[1] - lo[1], -lo[2]]);
            }
        }
        models
    }

    /// `settings` as the print needs them: the temperature tower starts at
    /// its first temperature on every layer up to the first block change,
    /// the retraction test slices with its longest length, which
    /// [`Calibration::apply`] then shortens band by band, and the pressure
    /// advance tower is hollow (no infill, no top) and leaves pressure
    /// advance to [`Calibration::apply`].
    pub fn settings(&self, settings: &Settings) -> Settings {
        let mut settings = settings.clone();
        match self.kind {
            Kind::Temperature => {
                if let Some(&first) = self.values.first() {
                    settings.nozzle_temp = first;
                    settings.nozzle_temp_first_layer = first;
                }
            }
            Kind::Retraction => {
                settings.retraction_length = self.values.iter().copied().fold(0.0, f64::max);
            }
            Kind::Flow => {}
            Kind::PressureAdvance => {
                settings.infill_density = 0.0;
                settings.top_layers = 0;
                settings.pressure_advance = 0.0;
            }
        }
        settings
    }

    /// Turns the G-code of the sliced test models into the calibration
    /// G-code. `objects` are the test models as sliced, with their index in
    /// [`Calibration::models`]; `settings` are the ones the G-code was
    /// written with (see [`Calibration::settings`]).
    ///
    /// Towers are split into equal bands from the bed to the top of the
    /// highest object; a layer belongs to the band its middle lies in. The
    /// temperature tower gets `M104` at the first layer of every block after
    /// the first, the pressure advance tower its factor at the first layer
    /// of every band. The retraction test changes the length of every
    /// retraction (and the push back after it) to that of its band. The
    /// flow test scales every extrusion that starts and ends inside a flow
    /// block, and the filament figures of the statistics and the header
    /// with it (those of single extruders stay as they were).
    pub fn apply(&self, output: &mut Output, objects: &[(usize, &Mesh)], settings: &Settings) {
        let bounds: Vec<(usize, [f64; 3], [f64; 3])> = objects
            .iter()
            .filter_map(|&(index, mesh)| mesh.bounds().map(|(lo, hi)| (index, lo, hi)))
            .collect();
        let top = bounds.iter().map(|b| b.2[2]).fold(0.0, f64::max);
        // Walls lie at least half a line inside the outline, a brim at
        // least half a line outside.
        let margin = settings.line_width / 4.0;
        let regions = match self.kind {
            Kind::Flow => bounds
                .iter()
                .filter_map(|&(index, lo, hi)| {
                    let percent = self.values.get(index)?;
                    Some(Region {
                        lo: [lo[0] - margin, lo[1] - margin],
                        hi: [hi[0] + margin, hi[1] + margin],
                        factor: 1.0 + percent / 100.0,
                    })
                })
                .collect(),
            _ => Vec::new(),
        };
        let mut editor = Editor {
            calibration: self,
            retraction_length: settings.retraction_length,
            flavor: settings.flavor,
            top,
            regions,
            absolute_e: false,
            absolute_xy: true,
            source_e: 0.0,
            written_e: 0.0,
            position: None,
            in_layers: false,
            z: 0.0,
            band: None,
            retracted: None,
            added: 0.0,
        };
        let mut gcode = String::with_capacity(output.gcode.len() + 1024);
        for line in output.gcode.split_inclusive('\n') {
            editor.line(line, &mut gcode);
        }
        if editor.added != 0.0 {
            let old = output.stats.clone();
            let stats = &mut output.stats;
            let ratio = (old.filament_length_mm + editor.added) / old.filament_length_mm;
            stats.filament_length_mm += editor.added;
            stats.filament_volume_cm3 *= ratio;
            stats.filament_weight_g *= ratio;
            for (unit, before, after) in [
                ("mm", old.filament_length_mm, stats.filament_length_mm),
                ("cm3", old.filament_volume_cm3, stats.filament_volume_cm3),
                ("g", old.filament_weight_g, stats.filament_weight_g),
            ] {
                gcode = gcode.replacen(
                    &format!("; filament used [{unit}]: {before:.2}\n"),
                    &format!("; filament used [{unit}]: {after:.2}\n"),
                    1,
                );
            }
        }
        output.gcode = gcode;
    }

    /// A value as written into G-code comments, without non-ASCII units.
    fn comment_text(&self, value: f64) -> String {
        match self.kind {
            Kind::Temperature => format!("nozzle temperature {value}"),
            Kind::Retraction => format!("retraction {value} mm"),
            Kind::Flow => format!("flow {value:+} %"),
            Kind::PressureAdvance => format!("pressure advance {value}"),
        }
    }
}

/// A flow block seen from above, and its flow factor.
struct Region {
    lo: Point,
    hi: Point,
    factor: f64,
}

impl Region {
    fn contains(&self, p: Point) -> bool {
        (0..2).all(|a| p[a] >= self.lo[a] && p[a] <= self.hi[a])
    }
}

/// Walks the G-code line by line and changes it for a calibration.
struct Editor<'a> {
    calibration: &'a Calibration,
    /// Length of the retractions the writer made.
    retraction_length: f64,
    /// Firmware, for the pressure advance command.
    flavor: Flavor,
    /// Top of the highest test model.
    top: f64,
    regions: Vec<Region>,
    /// Extrusion is absolute (`M82`).
    absolute_e: bool,
    /// Positions are absolute (`G90`).
    absolute_xy: bool,
    /// Last E value as read, and as written; absolute extrusion only.
    source_e: f64,
    written_e: f64,
    /// Where the nozzle is, if known.
    position: Option<Point>,
    /// Whether the first layer change has been seen; the start G-code
    /// before it stays untouched.
    in_layers: bool,
    /// Height of the current layer, from the `;Z:` comment.
    z: f64,
    /// Band (tower) of the current layer.
    band: Option<usize>,
    /// The filament pulled back by the last retraction as written, until
    /// it is pushed back.
    retracted: Option<f64>,
    /// Filament added by the flow changes, in mm.
    added: f64,
}

/// The words of a G-code command with their byte offsets.
fn words(code: &str) -> Vec<(usize, &str)> {
    let mut words = Vec::new();
    let mut start = None;
    for (i, c) in code.char_indices().chain([(code.len(), ' ')]) {
        match (c.is_whitespace(), start) {
            (true, Some(s)) => {
                words.push((s, &code[s..i]));
                start = None;
            }
            (false, None) => start = Some(i),
            _ => {}
        }
    }
    words
}

impl Editor<'_> {
    /// The band of a layer whose middle lies at `middle`.
    fn band_at(&self, middle: f64) -> usize {
        let count = self.calibration.values.len();
        if self.top <= 0.0 || count == 0 {
            return 0;
        }
        ((middle / self.top * count as f64).floor().max(0.0) as usize).min(count - 1)
    }

    /// Value of the current band.
    fn band_value(&self) -> f64 {
        let band = self.band.unwrap_or(0);
        self.calibration.values.get(band).copied().unwrap_or(0.0)
    }

    /// Copies `line` (with its line end) to `out`, changed as needed.
    fn line(&mut self, line: &str, out: &mut String) {
        let text = line.trim_end();
        if text.starts_with(";LAYER_CHANGE") && !self.in_layers {
            self.in_layers = true;
            let values: Vec<String> = (self.calibration.values.iter())
                .map(|&v| match self.calibration.kind {
                    Kind::Flow => format!("{v:+}"),
                    _ => v.to_string(),
                })
                .collect();
            let what = match self.calibration.kind {
                Kind::Temperature => "nozzle temperatures from the bottom up",
                Kind::Retraction => "retraction lengths in mm from the bottom up",
                Kind::Flow => "flow in % per block",
                Kind::PressureAdvance => "pressure advance from the bottom up",
            };
            out.push_str(&format!(
                "; calibration: {}, {what}: {}\n",
                self.calibration.kind.title().to_lowercase(),
                values.join(", ")
            ));
        }
        if let Some(z) = text.strip_prefix(";Z:") {
            self.z = z.trim().parse().unwrap_or(self.z);
        }
        if let Some(height) = text.strip_prefix(";HEIGHT:") {
            out.push_str(line);
            let height: f64 = height.trim().parse().unwrap_or(0.0);
            self.change_layer(self.z - height / 2.0, out);
            return;
        }
        let code = text.split(';').next().unwrap_or("");
        let words = words(code);
        let Some(&(_, command)) = words.first() else {
            out.push_str(line);
            return;
        };
        let value = |letter: char| {
            words.iter().find_map(|&(_, w)| {
                let rest = w.strip_prefix(letter)?;
                rest.parse::<f64>().ok()
            })
        };
        match command {
            "M82" => self.absolute_e = true,
            "M83" => self.absolute_e = false,
            "G90" => self.absolute_xy = true,
            "G91" => {
                self.absolute_xy = false;
                self.position = None;
            }
            "G92" => {
                if let Some(e) = value('E') {
                    self.source_e = e;
                    self.written_e = e;
                }
                if value('X').is_some() || value('Y').is_some() {
                    self.position = None;
                }
            }
            "G0" | "G1" => {
                let (x, y) = (value('X'), value('Y'));
                let from = self.position;
                let to = match (self.absolute_xy, from) {
                    (false, _) => None,
                    (true, Some(p)) => Some([x.unwrap_or(p[0]), y.unwrap_or(p[1])]),
                    (true, None) => x.zip(y).map(|(x, y)| [x, y]),
                };
                self.position = to;
                if let Some(e) = value('E') {
                    let moves = x.is_some() || y.is_some();
                    let lifts = value('Z').is_some();
                    let written = self.extrusion(e, moves || lifts, from.zip(to));
                    if (written - e).abs() > 1e-9
                        && let Some(&(at, word)) = words.iter().find(|(_, w)| w.starts_with('E'))
                    {
                        out.push_str(&line[..at]);
                        out.push_str(&format!("E{:.5}", written + 0.0));
                        out.push_str(&line[at + word.len()..]);
                        return;
                    }
                }
            }
            _ => {}
        }
        out.push_str(line);
    }

    /// Handles the start of a layer whose middle lies at `middle`.
    fn change_layer(&mut self, middle: f64, out: &mut String) {
        if self.calibration.kind == Kind::Flow {
            return;
        }
        let band = self.band_at(middle);
        if self.band == Some(band) {
            return;
        }
        let first = self.band.is_none();
        self.band = Some(band);
        let value = self.band_value();
        out.push_str(&format!(
            "; calibration: {}\n",
            self.calibration.comment_text(value)
        ));
        // The start G-code has heated the nozzle for the first band.
        match self.calibration.kind {
            Kind::Temperature if !first => out.push_str(&format!("M104 S{value}\n")),
            Kind::PressureAdvance => out.push_str(&match self.flavor {
                Flavor::Marlin => format!("M900 K{value}\n"),
                Flavor::Klipper => format!("SET_PRESSURE_ADVANCE ADVANCE={value}\n"),
            }),
            _ => {}
        }
    }

    /// The E value to write for a move with E value `e`. `moves` says
    /// whether the move goes anywhere; `path` holds its ends if known.
    fn extrusion(&mut self, e: f64, moves: bool, path: Option<(Point, Point)>) -> f64 {
        let amount = if self.absolute_e {
            e - self.source_e
        } else {
            e
        };
        let changed = if self.in_layers {
            self.changed_amount(amount, moves, path)
        } else {
            amount
        };
        if self.absolute_e {
            self.source_e = e;
            self.written_e += changed;
            self.written_e
        } else {
            changed
        }
    }

    /// The filament amount a move of `amount` gets in the calibration print.
    fn changed_amount(&mut self, amount: f64, moves: bool, path: Option<(Point, Point)>) -> f64 {
        match self.calibration.kind {
            Kind::Temperature | Kind::PressureAdvance => amount,
            Kind::Retraction => {
                let retraction = (amount.abs() - self.retraction_length).abs() < 1e-4;
                if moves || !retraction {
                    amount
                } else if amount < 0.0 {
                    let length = self.band_value();
                    self.retracted = Some(length);
                    -length
                } else {
                    // Pushed back as much as was pulled back, also when the
                    // band changed in between.
                    self.retracted.take().unwrap_or(amount)
                }
            }
            Kind::Flow => {
                let Some((from, to)) = path.filter(|_| moves && amount > 0.0) else {
                    return amount;
                };
                let Some(region) = (self.regions.iter())
                    .find(|region| region.contains(from) && region.contains(to))
                else {
                    return amount;
                };
                let changed = amount * region.factor;
                self.added += changed - amount;
                changed
            }
        }
    }
}

/// A tower of `count` blocks of `size` (width, depth, height) stacked on
/// each other, with a groove under every block boundary, one corner at the
/// origin. Built as rings of four corners from the bottom up, joined by
/// side faces, with a cap at each end.
fn stacked_blocks(count: usize, size: [f64; 3]) -> Mesh {
    let [width, depth, height] = size;
    // (height, inset) of every ring.
    let mut rings = vec![(0.0, 0.0)];
    for block in 0..count {
        let top = (block + 1) as f64 * height;
        if block + 1 < count {
            rings.extend([(top - GROOVE, 0.0), (top - GROOVE, GROOVE), (top, 0.0)]);
        } else {
            rings.push((top, 0.0));
        }
    }
    let mut mesh = Mesh::default();
    for &(z, inset) in &rings {
        mesh.vertices.extend([
            [inset, inset, z],
            [width - inset, inset, z],
            [width - inset, depth - inset, z],
            [inset, depth - inset, z],
        ]);
    }
    for ring in 0..rings.len() as u32 - 1 {
        let (a, b) = (4 * ring, 4 * ring + 4);
        for j in 0..4 {
            let k = (j + 1) % 4;
            mesh.triangles
                .extend([[a + j, a + k, b + k], [a + j, b + k, b + j]]);
        }
    }
    let last = 4 * (rings.len() as u32 - 1);
    mesh.triangles.extend([
        [0, 2, 1],
        [0, 3, 2],
        [last, last + 1, last + 2],
        [last, last + 2, last + 3],
    ]);
    mesh
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::field_reassign_with_default)]

    use std::sync::atomic::AtomicBool;

    use super::*;
    use slicer_core::Diagnostics;
    use slicer_profiles::ExtrusionMode;

    /// Value of the word starting with `letter` in a G-code line.
    fn word(line: &str, letter: char) -> Option<f64> {
        line.split(';')
            .next()?
            .split_whitespace()
            .skip(1)
            .find_map(|w| w.strip_prefix(letter).and_then(|v| v.parse().ok()))
    }

    /// Slices the test models of `calibration` as placed on the bed and
    /// applies the calibration; returns the plain and the changed G-code.
    fn print(calibration: &Calibration, base: &Settings) -> (Output, Output) {
        let settings = calibration.settings(base);
        let meshes: Vec<Mesh> = (calibration.placed_models(&settings).into_iter())
            .map(|(_, mesh)| mesh)
            .collect();
        let toolpaths = slicer_fdm::generate_plate(
            &meshes,
            slicer_fdm::Placement::AsGiven,
            &settings,
            &mut Diagnostics::new(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let plain = crate::write_gcode(&toolpaths, &settings).unwrap();
        let mut changed = plain.clone();
        let objects: Vec<(usize, &Mesh)> = meshes.iter().enumerate().collect();
        calibration.apply(&mut changed, &objects, &settings);
        (plain, changed)
    }

    #[test]
    fn values_step_up_or_down_and_are_checked() {
        let t = Calibration::new(Kind::Temperature, 230.0, 200.0, 10.0).unwrap();
        assert_eq!(t.values(), [230.0, 220.0, 210.0, 200.0]);
        let r = Calibration::new(Kind::Retraction, 0.2, 1.0, 0.2).unwrap();
        assert_eq!(r.values(), [0.2, 0.4, 0.6, 0.8, 1.0]);
        // The step need not divide the range.
        let f = Calibration::new(Kind::Flow, -10.0, 10.0, 8.0).unwrap();
        assert_eq!(f.values(), [-10.0, -2.0, 6.0]);
        assert_eq!(f.value_text(6.0), "+6 %");
        assert_eq!(t.value_text(230.0), "230 °C");

        for (kind, start, end, step) in [
            (Kind::Temperature, 200.0, 230.0, 0.0),
            (Kind::Temperature, 200.0, 230.0, f64::NAN),
            (Kind::Temperature, 50.0, 230.0, 5.0),
            (Kind::Temperature, 100.0, 300.0, 1.0),
            (Kind::Retraction, 0.0, 0.0, 0.1),
            (Kind::Flow, -60.0, 0.0, 10.0),
        ] {
            let error = Calibration::new(kind, start, end, step).unwrap_err();
            assert_eq!(error.diagnostic().code, "calibration.invalid_range");
        }
        assert_eq!(Kind::from_name("flow"), Some(Kind::Flow));
        assert_eq!(Kind::from_name("pressure"), None);
        assert_eq!(
            Kind::from_name("pressure-advance"),
            Some(Kind::PressureAdvance)
        );
        let settings = Settings::default();
        for kind in Kind::ALL {
            let [start, end, step] = kind.default_range(&settings);
            assert!(Calibration::new(kind, start, end, step).is_ok());
        }
    }

    #[test]
    fn towers_are_closed_with_a_groove_between_blocks() {
        let mesh = stacked_blocks(3, TOWER_BLOCK);
        assert_eq!(mesh.open_edge_count(), 0);
        let (lo, hi) = mesh.bounds().unwrap();
        assert_eq!((lo, hi), ([0.0; 3], [20.0, 10.0, 30.0]));
        // Two grooves: the box loses a sloped ring (prismatoid) under each
        // of the two upper blocks.
        let [w, d, _] = TOWER_BLOCK;
        let g = GROOVE;
        let sloped = g / 6.0 * (w * d + (w - 2.0 * g) * (d - 2.0 * g) + 4.0 * (w - g) * (d - g));
        let expected = w * d * 30.0 - 2.0 * (w * d * g - sloped);
        assert!((mesh.volume() - expected).abs() < 1e-9, "{}", mesh.volume());
        assert!(mesh.signed_volume() > 0.0);
        let single = stacked_blocks(1, RETRACTION_BAND);
        assert!((single.volume() - 8.0 * 8.0 * 5.0).abs() < 1e-9);
    }

    #[test]
    fn models_and_placement() {
        let settings = Settings::default();
        let tower = Calibration::new(Kind::Temperature, 220.0, 200.0, 5.0).unwrap();
        let placed = tower.placed_models(&settings);
        assert_eq!(placed.len(), 1);
        let (lo, hi) = placed[0].1.bounds().unwrap();
        assert_eq!(hi[2], 50.0);
        assert!(((lo[0] + hi[0]) / 2.0 - 110.0).abs() < 1e-9);

        let retraction = Calibration::new(Kind::Retraction, 0.5, 1.5, 0.5).unwrap();
        let placed = retraction.placed_models(&settings);
        assert_eq!(placed.len(), 2);
        // The tabs are on the outer sides: the gap between the towers
        // stays as it was.
        let (left, right) = (placed[0].1.bounds().unwrap(), placed[1].1.bounds().unwrap());
        assert!(
            (right.0[0] - left.1[0] - RETRACTION_GAP).abs() < 1e-9,
            "{left:?} {right:?}"
        );
        let tab = retraction.tab_width();
        assert!((left.1[0] - left.0[0] - 8.0 - tab).abs() < 1e-9);
        assert!((right.1[0] - right.0[0] - 8.0 - tab).abs() < 1e-9);

        let flow = Calibration::new(Kind::Flow, -10.0, 10.0, 5.0).unwrap();
        let placed = flow.placed_models(&settings);
        assert_eq!(placed[0].0, "Flow -10 %");
        assert_eq!(placed[4].0, "Flow +10 %");
        // Left to right in the order of the values.
        let xs: Vec<f64> = placed
            .iter()
            .map(|(_, m)| m.bounds().unwrap().0[0])
            .collect();
        assert!(xs.windows(2).all(|w| w[0] < w[1]), "{xs:?}");
    }

    /// Whether `p` lies inside `shapes` (even-odd over all contours).
    fn inside(shapes: &slicer_core::polygon::Shapes, p: Point) -> bool {
        let mut crossings = 0;
        for contour in shapes.iter().flatten() {
            for (i, a) in contour.iter().enumerate() {
                let b = contour[(i + 1) % contour.len()];
                if (a[1] > p[1]) != (b[1] > p[1])
                    && p[0] < a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0])
                {
                    crossings += 1;
                }
            }
        }
        crossings % 2 == 1
    }

    /// Reads the lettering of `label` off the slices of `mesh`: for every
    /// digit, which of the seven places hold a bar, probed in front of the
    /// face; other characters by the middle of their bars. Returns the
    /// text it reads.
    fn read_label(mesh: &Mesh, label: &Label) -> String {
        let style = &label.style;
        let width = text::width(&label.text, style);
        let filled = |along: f64, up: f64| {
            let p = label
                .face
                .point([along, up, style.raise / 2.0], width, style.height);
            let shapes = slicer_core::slice::slice_mesh(mesh, &[p[2]]);
            inside(&shapes[0], [p[0], p[1]])
        };
        let middle = |[l, b, r, t]: text::Bar| ((l + r) / 2.0, (b + t) / 2.0);
        let mut read = String::new();
        for (c, left) in text::layout(&label.text, style) {
            // Only the places within the character's own width; a narrow
            // one is followed by the next character.
            let (bars, own) = text::glyph(c, style).unwrap();
            let places = text::places(style);
            let bits = (0..7).fold(0u8, |bits, k| {
                let (along, up) = middle(places[k]);
                let bar = along <= own && filled(left + along, up);
                bits | (u8::from(bar) << k)
            });
            if let Some(digit) = text::DIGITS.iter().position(|&d| d == bits) {
                read.push(char::from_digit(digit as u32, 10).unwrap());
                continue;
            }
            // Not a digit of seven places: the narrow 1, the point, the
            // signs.
            if bars
                .iter()
                .all(|&bar| filled(left + middle(bar).0, middle(bar).1))
            {
                read.push(c);
            } else {
                read.push('?');
            }
        }
        read
    }

    #[test]
    fn every_value_is_lettered_on_its_block_or_band() {
        let cases = [
            Calibration::new(Kind::Temperature, 225.0, 190.0, 5.0).unwrap(),
            Calibration::new(Kind::Temperature, 200.0, 207.5, 2.5).unwrap(),
            Calibration::new(Kind::Retraction, 0.2, 2.0, 0.2).unwrap(),
            Calibration::new(Kind::Flow, -10.0, 10.0, 5.0).unwrap(),
            Calibration::new(Kind::PressureAdvance, 0.0, 0.045, 0.005).unwrap(),
        ];
        for calibration in cases {
            let models = calibration.models();
            let labels = calibration.labels();
            assert_eq!(labels.len(), calibration.values().len());
            for (i, (label, &value)) in labels.iter().zip(calibration.values()).enumerate() {
                let expected = calibration.label_text(value);
                assert_eq!(label.text, expected);
                let read = read_label(&models[label.model].1, label);
                assert_eq!(read, expected, "{:?} value {i}", calibration.kind());
                let [_, _, z] = label.face.centre;
                let half = label.style.height / 2.0;
                match calibration.kind() {
                    // On the front, in the block it belongs to, clear of
                    // its groove.
                    Kind::Temperature => {
                        let block = i as f64 * TOWER_BLOCK[2];
                        assert!(z - half >= block && z + half <= block + 9.0, "{z}");
                        // The narrow style where the text needs it.
                        let wide = text::width(&expected, &TOWER_TEXT) > 18.0;
                        assert_eq!(label.style != TOWER_TEXT, wide, "{expected}");
                    }
                    // Alternately on the left and the right tab, with the
                    // middle of the band inside the lettering.
                    Kind::Retraction | Kind::PressureAdvance => {
                        let pitch = match calibration.kind() {
                            Kind::Retraction => RETRACTION_BAND[2],
                            _ => PRESSURE_BAND[2],
                        };
                        let middle = (i as f64 + 0.5) * pitch;
                        assert!((z - middle).abs() < half, "{z} {middle}");
                        let side = (label.face.out, label.model);
                        if calibration.kind() == Kind::Retraction {
                            assert_eq!(label.model, i % 2);
                        } else if i % 2 == 0 {
                            assert_eq!(side, ([0.0, -1.0, 0.0], 0));
                        } else {
                            assert_eq!(side, ([1.0, 0.0, 0.0], 0));
                        }
                    }
                    // On top of its own block.
                    Kind::Flow => {
                        assert_eq!(label.model, i);
                        assert_eq!(z, FLOW_BLOCK[2]);
                    }
                }
            }
            // The lettering keeps within the height of a tower, so the
            // bands stay where they are.
            let top = |(_, mesh): &(String, Mesh)| mesh.bounds().unwrap().1[2];
            let count = calibration.values().len() as f64;
            let expected_top = match calibration.kind() {
                Kind::Temperature => count * TOWER_BLOCK[2],
                Kind::Retraction => count * RETRACTION_BAND[2],
                Kind::Flow => FLOW_BLOCK[2] + FLOW_TEXT.raise,
                Kind::PressureAdvance => count * PRESSURE_BAND[2],
            };
            assert!(models.iter().all(|m| top(m) == expected_top));
            // Lettering and tabs only add closed shells.
            for (_, mesh) in &models {
                assert_eq!(mesh.open_edge_count(), 0);
                let mut diagnostics = Diagnostics::new();
                mesh.check(&mut diagnostics);
                assert!(diagnostics.is_empty(), "{diagnostics:?}");
            }
        }
    }

    #[test]
    fn lettering_prints_on_the_tower() {
        // The sliced front of the temperature tower: the outer wall bulges
        // out where the lettering stands, by about the raise.
        let calibration = Calibration::new(Kind::Temperature, 215.0, 210.0, 5.0).unwrap();
        let settings = calibration.settings(&Settings::default());
        let models = calibration.models();
        let toolpaths = slicer_fdm::generate(&models[0].1, &settings).unwrap();
        let front = |z: f64| {
            let layer = (toolpaths.layers.iter())
                .find(|l| (l.z - z).abs() < 0.11)
                .unwrap();
            (layer.paths.iter())
                .filter(|p| p.kind == slicer_fdm::PathKind::OuterWall)
                .flat_map(|p| p.points.iter().map(|q| q[1]))
                .fold(f64::INFINITY, f64::min)
        };
        let (lo, _) = models[0].1.bounds().unwrap();
        // At 6 mm the upright bars of "215" stand out at full depth; at
        // 8.5 mm, above the lettering and below the groove, the face is
        // plain.
        let bulge = front(8.5) - front(6.0);
        assert!((bulge - TOWER_TEXT.raise).abs() < 0.1, "{bulge}");
        assert!(lo[1] == -TOWER_TEXT.raise);
    }

    #[test]
    fn temperature_tower_sets_the_temperature_per_block() {
        let calibration = Calibration::new(Kind::Temperature, 225.0, 215.0, 5.0).unwrap();
        let (plain, changed) = print(&calibration, &Settings::default());
        assert!(plain.gcode.contains("M109 S225\n"));
        assert!(!plain.gcode.contains("M104 S215"));
        // 3 blocks of 10 mm with 0.2 mm layers: blocks change at layers 50
        // and 100 (counted from 0).
        let layers: Vec<&str> = changed.gcode.split(";LAYER_CHANGE").collect();
        assert_eq!(layers.len(), 151);
        let with = |text: &str| -> Vec<usize> {
            (1..layers.len())
                .filter(|&i| layers[i].contains(text))
                .map(|i| i - 1)
                .collect()
        };
        assert_eq!(with("\nM104 S220\n"), [50]);
        assert_eq!(with("\nM104 S215\n"), [100]);
        assert!(layers[0].contains(
            "; calibration: temperature tower, nozzle temperatures from the bottom up: 225, 220, 215\n"
        ));
        // Nothing else changed.
        let removed: String = changed
            .gcode
            .lines()
            .filter(|l| !l.starts_with("; calibration") && *l != "M104 S220" && *l != "M104 S215")
            .map(|l| format!("{l}\n"))
            .collect();
        assert_eq!(removed, plain.gcode);
        assert_eq!(changed.stats, plain.stats);
    }

    #[test]
    fn pressure_advance_tower_sets_the_factor_per_band() {
        for (flavor, command) in [
            (Flavor::Marlin, "M900 K"),
            (Flavor::Klipper, "SET_PRESSURE_ADVANCE ADVANCE="),
        ] {
            let calibration = Calibration::new(Kind::PressureAdvance, 0.0, 0.04, 0.02).unwrap();
            let mut base = Settings::default();
            base.flavor = flavor;
            // The tower's own values replace the one of the settings.
            base.pressure_advance = 0.5;
            let (plain, changed) = print(&calibration, &base);
            assert!(!plain.gcode.contains(command));
            // 3 bands of 4 mm with 0.2 mm layers: bands start at layers 0,
            // 20 and 40.
            let layers: Vec<&str> = changed.gcode.split(";LAYER_CHANGE").collect();
            assert_eq!(layers.len(), 61);
            let with = |text: String| -> Vec<usize> {
                (1..layers.len())
                    .filter(|&i| layers[i].contains(&text))
                    .map(|i| i - 1)
                    .collect()
            };
            assert_eq!(
                with(format!(
                    "
{command}0
"
                )),
                [0]
            );
            assert_eq!(
                with(format!(
                    "
{command}0.02
"
                )),
                [20]
            );
            assert_eq!(
                with(format!(
                    "
{command}0.04
"
                )),
                [40]
            );
            // Hollow: walls and the bottom only.
            assert!(!changed.gcode.contains(";TYPE:Sparse infill"));
            let removed: String = changed
                .gcode
                .lines()
                .filter(|l| !l.starts_with("; calibration") && !l.starts_with(command))
                .map(|l| {
                    format!(
                        "{l}
"
                    )
                })
                .collect();
            assert_eq!(removed, plain.gcode);
            assert_eq!(changed.stats, plain.stats);
        }
    }

    /// Retraction lengths as written, per band (retractions only).
    fn retractions(gcode: &str, absolute: bool) -> Vec<(String, f64)> {
        let mut band = String::new();
        let mut last = 0.0;
        let mut found = Vec::new();
        for line in gcode.lines() {
            if let Some(text) = line.strip_prefix("; calibration: retraction ") {
                band = text.to_owned();
            }
            if line.starts_with("G92 E0") {
                last = 0.0;
            }
            let Some(e) = word(line, 'E') else { continue };
            let amount = if absolute { e - last } else { e };
            last = e;
            if word(line, 'X').is_none() && amount < 0.0 {
                found.push((band.clone(), -amount));
            }
        }
        found
    }

    #[test]
    fn retraction_test_changes_the_length_per_band() {
        for mode in [ExtrusionMode::Relative, ExtrusionMode::Absolute] {
            let absolute = mode == ExtrusionMode::Absolute;
            let mut settings = Settings::default();
            settings.extrusion_mode = mode;
            let calibration = Calibration::new(Kind::Retraction, 0.4, 1.2, 0.4).unwrap();
            let (plain, changed) = print(&calibration, &settings);
            // Sliced with the longest length.
            let before = retractions(&plain.gcode, absolute);
            assert!(before.iter().all(|(_, length)| (length - 1.2).abs() < 1e-4));
            let after = retractions(&changed.gcode, absolute);
            assert_eq!(after.len(), before.len());
            for (band, expected) in [("0.4 mm", 0.4), ("0.8 mm", 0.8), ("1.2 mm", 1.2)] {
                let lengths: Vec<f64> = (after.iter())
                    .filter(|(b, _)| b == band)
                    .map(|(_, l)| *l)
                    .collect();
                // Every layer travels between the towers.
                assert!(lengths.len() >= 20, "{band}: {}", lengths.len());
                assert!(
                    lengths.iter().all(|l| (l - expected).abs() < 1e-4),
                    "{band}"
                );
            }
            // Filament pushed through in total stays the same.
            let pushed = |gcode: &str| {
                let mut last = 0.0;
                let mut total = 0.0;
                for line in gcode.lines() {
                    if line.starts_with("G92 E0") {
                        last = 0.0;
                    }
                    if let Some(e) = word(line, 'E') {
                        total += if absolute { e - last } else { e };
                        last = e;
                    }
                }
                total
            };
            assert!((pushed(&plain.gcode) - pushed(&changed.gcode)).abs() < 1e-3);
            assert_eq!(changed.stats, plain.stats);
        }
    }

    #[test]
    fn flow_test_scales_each_block() {
        for mode in [ExtrusionMode::Relative, ExtrusionMode::Absolute] {
            let absolute = mode == ExtrusionMode::Absolute;
            let mut settings = Settings::default();
            settings.extrusion_mode = mode;
            settings.brim_width = 3.0;
            let calibration = Calibration::new(Kind::Flow, -10.0, 10.0, 10.0).unwrap();
            let (plain, changed) = print(&calibration, &settings);
            let meshes: Vec<Mesh> = (calibration.placed_models(&settings).into_iter())
                .map(|(_, mesh)| mesh)
                .collect();
            // Filament extruded per block (both ends inside), and outside.
            let per_block = |gcode: &str| {
                let mut sums = [0.0; 4];
                let mut position: Option<Point> = None;
                let mut last = 0.0;
                for line in gcode.lines() {
                    if line.starts_with("G92 E0") {
                        last = 0.0;
                    }
                    let (x, y) = (word(line, 'X'), word(line, 'Y'));
                    let to = x.zip(y).map(|(x, y)| [x, y]);
                    if let Some(e) = word(line, 'E') {
                        let amount = if absolute { e - last } else { e };
                        last = e;
                        if let (Some(from), Some(to)) = (position, to)
                            && amount > 0.0
                        {
                            let inside = |p: Point, mesh: &Mesh| {
                                let (lo, hi) = mesh.bounds().unwrap();
                                (0..2).all(|a| p[a] >= lo[a] - 0.1 && p[a] <= hi[a] + 0.1)
                            };
                            let block = (meshes.iter())
                                .position(|m| inside(from, m) && inside(to, m))
                                .unwrap_or(3);
                            sums[block] += amount;
                        }
                    }
                    if to.is_some() {
                        position = to;
                    }
                }
                sums
            };
            let before = per_block(&plain.gcode);
            let after = per_block(&changed.gcode);
            for (block, factor) in [0.9, 1.0, 1.1].into_iter().enumerate() {
                assert!(before[block] > 100.0);
                let ratio = after[block] / before[block];
                assert!((ratio - factor).abs() < 1e-4, "block {block}: {ratio}");
            }
            // Skirt and brim keep their flow.
            assert!(before[3] > 10.0);
            assert!((after[3] - before[3]).abs() < 1e-3);
            // The statistics follow; the -10 % and +10 % blocks cancel out.
            let expected = plain.stats.filament_length_mm - 0.1 * before[0] + 0.1 * before[2];
            assert!((changed.stats.filament_length_mm - expected).abs() < 1e-3);
            assert!(changed.gcode.contains(&format!(
                "; filament used [mm]: {:.2}\n",
                changed.stats.filament_length_mm
            )));
        }
    }

    #[test]
    fn start_gcode_and_other_lines_stay_untouched() {
        let calibration = Calibration::new(Kind::Retraction, 0.8, 0.8, 1.0).unwrap();
        let gcode = "M83\nG1 E-0.80000 F2100\n;LAYER_CHANGE\n;Z:0.2\n;HEIGHT:0.2\n\
            G1 E-0.80000 F2100 ; pull\nG0 X10 Y10\nG1 E0.80000\nG1 X20 Y10 E0.80000\n";
        let mut output = Output {
            gcode: gcode.to_owned(),
            stats: crate::Stats {
                layers: 1,
                filament_length_mm: 1.0,
                filament_volume_cm3: 0.0,
                filament_weight_g: 0.0,
                print_time_s: 0.0,
                extruders: Vec::new(),
                flush_volume_cm3: 0.0,
                flush_weight_g: 0.0,
                times: Vec::new(),
            },
        };
        let mut settings = Settings::default();
        settings.retraction_length = 1.6;
        // Tall enough that the first layer lies in the first band.
        let mesh = Mesh::cuboid(10.0, 10.0, 10.0);
        calibration.apply(&mut output, &[(0, &mesh)], &settings);
        // Only lines with the writer's retraction length change; none here.
        assert_eq!(
            output.gcode,
            "M83\nG1 E-0.80000 F2100\n\
             ; calibration: retraction test, retraction lengths in mm from the bottom up: 0.8\n\
             ;LAYER_CHANGE\n;Z:0.2\n;HEIGHT:0.2\n; calibration: retraction 0.8 mm\n\
             G1 E-0.80000 F2100 ; pull\nG0 X10 Y10\nG1 E0.80000\nG1 X20 Y10 E0.80000\n"
        );
        settings.retraction_length = 0.8;
        let calibration = Calibration::new(Kind::Retraction, 0.0, 0.5, 0.5).unwrap();
        let mut output2 = output.clone();
        output2.gcode = gcode.to_owned();
        calibration.apply(&mut output2, &[(0, &mesh)], &settings);
        // The first band retracts by 0; the start G-code keeps its 0.8, the
        // comment stays where it was.
        assert!(output2.gcode.starts_with("M83\nG1 E-0.80000 F2100\n"));
        assert!(output2.gcode.contains("\nG1 E0.00000 F2100 ; pull\n"));
        assert!(
            output2
                .gcode
                .contains("\nG0 X10 Y10\nG1 E0.00000\nG1 X20 Y10 E0.80000\n")
        );
    }
}
