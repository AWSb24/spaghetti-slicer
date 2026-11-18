//! Colours, names and lengths of the line types in the preview.

use slicer_fdm::{Layer, Path, PathKind, Toolpaths};
use slicer_profiles::Settings;

/// What the colours of the preview show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorBy {
    /// The kind of every line, in fixed colours.
    LineType,
    /// The speed it is printed at, from slow (blue) to fast (red).
    Speed,
    /// The height of its layer.
    LayerHeight,
    /// The width of the line.
    Width,
}

impl ColorBy {
    /// Every mode with its name for the legend.
    pub const ALL: [(ColorBy, &'static str); 4] = [
        (ColorBy::LineType, "Line type"),
        (ColorBy::Speed, "Speed"),
        (ColorBy::LayerHeight, "Layer height"),
        (ColorBy::Width, "Line width"),
    ];

    /// The unit of the values shown.
    pub fn unit(self) -> &'static str {
        match self {
            ColorBy::LineType => "",
            ColorBy::Speed => "mm/s",
            ColorBy::LayerHeight | ColorBy::Width => "mm",
        }
    }

    /// The value of a path on layer `index` for this mode; `None` for line
    /// types.
    pub fn value(
        self,
        path: &Path,
        index: usize,
        layer: &Layer,
        settings: &Settings,
    ) -> Option<f64> {
        match self {
            ColorBy::LineType => None,
            ColorBy::Speed => Some(slicer_gcode::path_speed(settings, path.kind, index)),
            ColorBy::LayerHeight => Some(layer.height),
            ColorBy::Width => Some(path.width.unwrap_or(settings.line_width)),
        }
    }

    /// Smallest and largest value over all paths, every path with the
    /// settings of its object ([`Toolpaths::settings_of`]); `None` for line
    /// types or without paths.
    pub fn range(self, toolpaths: &Toolpaths, settings: &Settings) -> Option<(f64, f64)> {
        let mut range: Option<(f64, f64)> = None;
        for (index, layer) in toolpaths.layers.iter().enumerate() {
            for path in &layer.paths {
                let own = toolpaths.settings_of(path, settings);
                if let Some(v) = self.value(path, index, layer, own) {
                    range = Some(range.map_or((v, v), |(lo, hi)| (lo.min(v), hi.max(v))));
                }
            }
        }
        range
    }

    /// The colour of a path in this mode, with values spread over `range`.
    pub fn color(
        self,
        path: &Path,
        index: usize,
        layer: &Layer,
        settings: &Settings,
        range: Option<(f64, f64)>,
    ) -> [u8; 3] {
        match (self.value(path, index, layer, settings), range) {
            (Some(v), Some((lo, hi))) if hi > lo => gradient((v - lo) / (hi - lo)),
            (Some(_), _) => gradient(0.5),
            (None, _) => path_color(path.kind),
        }
    }
}

/// A colour from blue (0) over green and yellow to red (1), the usual
/// scale for slow to fast or thin to thick.
pub fn gradient(t: f64) -> [u8; 3] {
    const STOPS: [[f64; 3]; 5] = [
        [43.0, 131.0, 186.0],
        [171.0, 221.0, 164.0],
        [255.0, 255.0, 191.0],
        [253.0, 174.0, 97.0],
        [215.0, 25.0, 28.0],
    ];
    let t = if t.is_finite() {
        t.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let at = t * (STOPS.len() - 1) as f64;
    let i = (at.floor() as usize).min(STOPS.len() - 2);
    let f = at - i as f64;
    let (a, b) = (STOPS[i], STOPS[i + 1]);
    [0, 1, 2].map(|c| (a[c] + (b[c] - a[c]) * f).round() as u8)
}

/// Colour of a path kind as RGB in the app, in the colours users of common
/// desktop slicers know (walls yellow and orange, sparse infill dark red,
/// solid infill purple, bridges blue). The SVG preview keeps its own.
pub fn path_color(kind: PathKind) -> [u8; 3] {
    match kind {
        PathKind::Skirt | PathKind::Brim => [0x00, 0x87, 0x6e],
        PathKind::Support => [0x5e, 0xd1, 0x94],
        PathKind::OuterWall => [0xff, 0x7d, 0x38],
        PathKind::InnerWall => [0xff, 0xe6, 0x4d],
        PathKind::GapFill => [0xff, 0xff, 0xff],
        PathKind::SolidInfill => [0x96, 0x54, 0xcc],
        PathKind::Bridge => [0x4d, 0x80, 0xba],
        PathKind::SparseInfill => [0xb0, 0x30, 0x29],
        PathKind::PrimeTower => [0xc1, 0xbe, 0x63],
        PathKind::Ironing => [0xff, 0x9e, 0xc7],
        PathKind::OverhangWall => [0x1d, 0x9b, 0xf0],
        PathKind::OverhangWallFull => [0x0b, 0x53, 0x94],
    }
}

/// Name of a path kind for the legend, e.g. "Outer wall".
pub fn kind_name(kind: PathKind) -> String {
    let mut name = String::new();
    for (index, c) in format!("{kind:?}").chars().enumerate() {
        if c.is_uppercase() && index > 0 {
            name.push(' ');
            name.extend(c.to_lowercase());
        } else {
            name.push(c);
        }
    }
    name
}

/// Extruded length in millimetres of every kind of path over all `layers`,
/// in the order the kinds first appear.
pub fn length_by_kind(layers: &[Layer]) -> Vec<(PathKind, f64)> {
    let mut lengths: Vec<(PathKind, f64)> = Vec::new();
    for path in layers.iter().flat_map(|layer| &layer.paths) {
        match lengths.iter_mut().find(|(kind, _)| *kind == path.kind) {
            Some((_, length)) => *length += path.length(),
            None => lengths.push((path.kind, path.length())),
        }
    }
    lengths
}

/// The kinds of path in a layer, in the order they are first printed.
pub fn kinds_in(layer: &Layer) -> Vec<PathKind> {
    let mut kinds = Vec::new();
    for path in &layer.paths {
        if !kinds.contains(&path.kind) {
            kinds.push(path.kind);
        }
    }
    kinds
}

/// Where the lines and layers of a G-code text start, to show the code of
/// the layer in the preview.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GcodeIndex {
    /// Byte offset of every line.
    lines: Vec<usize>,
    /// First line of every layer: the `;LAYER_CHANGE` comment.
    layers: Vec<usize>,
}

impl GcodeIndex {
    /// Indexes `gcode`.
    pub fn new(gcode: &str) -> Self {
        let mut lines = vec![0];
        let mut layers = Vec::new();
        for (offset, _) in gcode.match_indices('\n') {
            if gcode[lines[lines.len() - 1]..offset].starts_with(";LAYER_CHANGE") {
                layers.push(lines.len() - 1);
            }
            lines.push(offset + 1);
        }
        if lines.last() == Some(&gcode.len()) {
            lines.pop();
        }
        Self { lines, layers }
    }

    /// Number of lines.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// Whether there are no lines.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Line `n` (from 0) of `gcode`, without its line break.
    pub fn line<'a>(&self, gcode: &'a str, n: usize) -> &'a str {
        let Some(&start) = self.lines.get(n) else {
            return "";
        };
        let end = self.lines.get(n + 1).map_or(gcode.len(), |&next| next - 1);
        gcode
            .get(start..end)
            .unwrap_or("")
            .trim_end_matches(['\r', '\n'])
    }

    /// The lines of layer `layer` (from 0), up to the next layer or the end
    /// of the print.
    pub fn layer(&self, layer: usize) -> std::ops::Range<usize> {
        let start = self.layers.get(layer).copied().unwrap_or(self.lines.len());
        let end = self
            .layers
            .get(layer + 1)
            .copied()
            .unwrap_or(self.lines.len());
        start..end
    }

    /// The line about `fraction` (0 to 1) of the way through layer `layer`.
    // ponytail: lines are spread evenly over the moves of the layer, so
    // the line found is near the move shown, not exactly it; exact would
    // need the writer to record the line of every move.
    pub fn line_at(&self, layer: usize, fraction: f64) -> usize {
        let range = self.layer(layer);
        let last = range.end.saturating_sub(1).max(range.start);
        range.start + ((last - range.start) as f64 * fraction.clamp(0.0, 1.0)).round() as usize
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn gcode_lines_and_layers_are_found() {
        let gcode =
            "; head\nG28\n;LAYER_CHANGE\n;Z:0.2\nG1 X1 E1\n;LAYER_CHANGE\r\nG1 X2 E1\nM84\n";
        let index = GcodeIndex::new(gcode);
        assert_eq!(index.len(), 8);
        assert_eq!(index.line(gcode, 0), "; head");
        assert_eq!(index.line(gcode, 5), ";LAYER_CHANGE");
        assert_eq!(index.line(gcode, 7), "M84");
        assert_eq!(index.line(gcode, 8), "");
        assert_eq!(index.layer(0), 2..5);
        assert_eq!(index.layer(1), 5..8);
        assert_eq!(index.layer(2), 8..8);
        assert_eq!(index.line_at(0, 0.0), 2);
        assert_eq!(index.line_at(0, 1.0), 4);
        assert_eq!(index.line_at(1, 0.5), 6);
        assert!(GcodeIndex::new("").is_empty() || GcodeIndex::new("").len() == 1);
    }

    #[test]
    fn colour_modes_spread_their_values() {
        let settings = Settings::default();
        let mesh = slicer_core::Mesh::cuboid(10.0, 10.0, 1.0);
        let toolpaths = slicer_fdm::generate(&mesh, &settings).unwrap();
        assert_eq!(ColorBy::LineType.range(&toolpaths, &settings), None);
        // Layer 0 prints at the first layer speed, the others faster.
        let (lo, hi) = ColorBy::Speed.range(&toolpaths, &settings).unwrap();
        assert_eq!(lo, settings.first_layer_speed);
        assert!(hi > lo);
        let (lo, hi) = ColorBy::Width.range(&toolpaths, &settings).unwrap();
        assert!(lo <= settings.line_width && hi >= settings.line_width);
        let layer = &toolpaths.layers[0];
        let path = &layer.paths[0];
        let range = ColorBy::Speed.range(&toolpaths, &settings);
        assert_eq!(
            ColorBy::Speed.color(path, 0, layer, &settings, range),
            gradient(0.0)
        );
        assert_eq!(
            ColorBy::LineType.color(path, 0, layer, &settings, None),
            path_color(path.kind)
        );
        assert_eq!(gradient(0.0), [43, 131, 186]);
        assert_eq!(gradient(1.0), [215, 25, 28]);
        assert_eq!(gradient(0.5), [255, 255, 191]);
        assert_eq!(gradient(f64::NAN), gradient(0.0));
        assert_eq!(gradient(7.0), gradient(1.0));
    }

    #[test]
    fn colours_and_names() {
        assert_eq!(path_color(PathKind::OuterWall), [0xff, 0x7d, 0x38]);
        assert_eq!(kind_name(PathKind::OuterWall), "Outer wall");
        assert_eq!(kind_name(PathKind::Skirt), "Skirt");
    }

    #[test]
    fn kinds_in_printing_order() {
        let mesh = slicer_core::Mesh::cuboid(10.0, 10.0, 1.0);
        let toolpaths = slicer_fdm::generate(&mesh, &slicer_profiles::Settings::default()).unwrap();
        let kinds = kinds_in(&toolpaths.layers[0]);
        assert_eq!(kinds[0], PathKind::Skirt);
        assert!(kinds.contains(&PathKind::OuterWall));
        let mut unique = kinds.clone();
        unique.dedup();
        assert_eq!(unique, kinds);

        let lengths = length_by_kind(&toolpaths.layers);
        assert_eq!(lengths[0].0, PathKind::Skirt);
        let total: f64 = lengths.iter().map(|(_, l)| l).sum();
        let expected: f64 = toolpaths
            .layers
            .iter()
            .flat_map(|layer| &layer.paths)
            .map(slicer_fdm::Path::length)
            .sum();
        assert!(total > 0.0 && (total - expected).abs() < 1e-9);
    }
}
