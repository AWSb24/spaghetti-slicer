//! Colours, names and lengths of the line types in the preview.

use slicer_fdm::{Layer, PathKind};

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
