//! Mapping between the bed (millimetres, Y up) and the screen (pixels,
//! Y down), with zoom and pan, and the colours of the layer view.

use slicer_fdm::{Layer, PathKind};

/// Free space around the bed when the view is not zoomed, as a fraction of
/// the drawing area.
const MARGIN: f32 = 0.05;

/// How the plate is shown: zoom and pan chosen by the user.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlateView {
    /// Magnification; 1 fits the whole bed into the drawing area.
    pub zoom: f32,
    /// Shift of the bed on the screen, in pixels.
    pub pan: [f32; 2],
}

impl Default for PlateView {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan: [0.0, 0.0],
        }
    }
}

/// The mapping for one frame: where the bed's origin is on the screen and
/// how many pixels a millimetre takes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    /// Screen position of the bed's origin (front left corner).
    pub origin: [f32; 2],
    /// Pixels per millimetre.
    pub scale: f32,
}

impl Frame {
    /// Screen position of a point on the bed.
    pub fn to_screen(&self, p: [f64; 2]) -> [f32; 2] {
        [
            self.origin[0] + p[0] as f32 * self.scale,
            self.origin[1] - p[1] as f32 * self.scale,
        ]
    }

    /// Bed position of a point on the screen.
    pub fn to_bed(&self, s: [f32; 2]) -> [f64; 2] {
        [
            f64::from((s[0] - self.origin[0]) / self.scale),
            f64::from((self.origin[1] - s[1]) / self.scale),
        ]
    }
}

impl PlateView {
    /// Smallest and largest zoom.
    pub const ZOOM_RANGE: (f32, f32) = (0.2, 50.0);

    /// The mapping for a drawing area at `area_min` of size `area_size`
    /// (pixels) and a bed of `bed` millimetres.
    pub fn frame(&self, area_min: [f32; 2], area_size: [f32; 2], bed: [f64; 2]) -> Frame {
        let usable = [
            area_size[0] * (1.0 - 2.0 * MARGIN),
            area_size[1] * (1.0 - 2.0 * MARGIN),
        ];
        let fit = (usable[0] / bed[0] as f32).min(usable[1] / bed[1] as f32);
        let scale = (fit * self.zoom).max(f32::MIN_POSITIVE);
        let center = [
            area_min[0] + area_size[0] / 2.0 + self.pan[0],
            area_min[1] + area_size[1] / 2.0 + self.pan[1],
        ];
        Frame {
            origin: [
                center[0] - bed[0] as f32 * scale / 2.0,
                center[1] + bed[1] as f32 * scale / 2.0,
            ],
            scale,
        }
    }

    /// Zooms by `factor` and keeps the bed point under `cursor` in place.
    pub fn zoom_at(
        &mut self,
        factor: f32,
        cursor: [f32; 2],
        area_min: [f32; 2],
        area_size: [f32; 2],
        bed: [f64; 2],
    ) {
        let before = self.frame(area_min, area_size, bed).to_bed(cursor);
        let (min, max) = Self::ZOOM_RANGE;
        self.zoom = (self.zoom * factor).clamp(min, max);
        let after = self.frame(area_min, area_size, bed).to_screen(before);
        self.pan[0] += cursor[0] - after[0];
        self.pan[1] += cursor[1] - after[1];
    }
}

/// Colour of a path kind as RGB, the same as in the SVG preview.
pub fn path_color(kind: PathKind) -> [u8; 3] {
    parse_hex(slicer_fdm::preview::color(kind)).unwrap_or([255, 255, 255])
}

/// Parses `#rrggbb`.
pub fn parse_hex(text: &str) -> Option<[u8; 3]> {
    let digits = text.strip_prefix('#')?;
    if digits.len() != 6 {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(digits.get(i..i + 2)?, 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
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

    const AREA_MIN: [f32; 2] = [100.0, 50.0];
    const AREA_SIZE: [f32; 2] = [1000.0, 500.0];
    const BED: [f64; 2] = [200.0, 100.0];

    fn near(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
    }

    #[test]
    fn bed_fits_the_area_with_y_up() {
        let frame = PlateView::default().frame(AREA_MIN, AREA_SIZE, BED);
        // 450 px of height for 100 mm is the tighter fit.
        assert!((frame.scale - 4.5).abs() < 1e-4);
        // The bed centre is the area centre; the origin is bottom left.
        assert!(near(frame.to_screen([100.0, 50.0]), [600.0, 300.0]));
        assert!(near(frame.to_screen([0.0, 0.0]), [150.0, 525.0]));
        let back = frame.to_bed([600.0, 300.0]);
        assert!((back[0] - 100.0).abs() < 1e-3 && (back[1] - 50.0).abs() < 1e-3);
    }

    #[test]
    fn zoom_keeps_the_point_under_the_cursor() {
        let mut view = PlateView::default();
        let cursor = [200.0, 400.0];
        let before = view.frame(AREA_MIN, AREA_SIZE, BED).to_bed(cursor);
        view.zoom_at(3.0, cursor, AREA_MIN, AREA_SIZE, BED);
        assert_eq!(view.zoom, 3.0);
        let frame = view.frame(AREA_MIN, AREA_SIZE, BED);
        assert!(near(frame.to_screen(before), cursor));
        view.zoom_at(1000.0, cursor, AREA_MIN, AREA_SIZE, BED);
        assert_eq!(view.zoom, PlateView::ZOOM_RANGE.1);
    }

    #[test]
    fn colours_and_names() {
        assert_eq!(parse_hex("#e8590c"), Some([0xe8, 0x59, 0x0c]));
        assert_eq!(parse_hex("e8590c"), None);
        assert_eq!(parse_hex("#e8590"), None);
        assert_eq!(parse_hex("#zz590c"), None);
        assert_eq!(path_color(PathKind::OuterWall), [0xe8, 0x59, 0x0c]);
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
