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

    /// Smallest and largest value over all paths; `None` for line types or
    /// without paths.
    pub fn range(self, toolpaths: &Toolpaths, settings: &Settings) -> Option<(f64, f64)> {
        let mut range: Option<(f64, f64)> = None;
        for (index, layer) in toolpaths.layers.iter().enumerate() {
            for path in &layer.paths {
                if let Some(v) = self.value(path, index, layer, settings) {
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

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
