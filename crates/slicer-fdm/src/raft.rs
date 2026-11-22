//! A raft: a few layers of lines under the model and its supports
//! (`raft_layers`). The model starts `raft_contact_distance` above it, so
//! that the raft comes off after printing.
//!
//! The lowest layer is nearly solid and sticks to the bed, the layers above
//! it are as sparse as the supports and cross each other, and the top ones
//! (`support_interface_layers`, at least one) are solid, so that the first
//! layer of the model lies on a closed surface.

use slicer_core::polygon::{self, HatchDirection};
use slicer_core::{Point, Shapes};
use slicer_profiles::{SeamPosition, Settings};

use crate::{Layer, Path, PathKind, Work, adhesion_work, outer_contours, push_work};

/// How densely the lowest layer of a raft is filled, as a share of solid.
const BASE_DENSITY: f64 = 0.9;

/// How much higher every layer of the model goes on a raft: the height of
/// its layers and the gap above them; 0 without a raft.
pub(crate) fn lift(settings: &Settings) -> f64 {
    match settings.raft_layers {
        0 => 0.0,
        n => {
            settings.first_layer_height
                + f64::from(n - 1) * settings.layer_height
                + settings.raft_contact_distance
        }
    }
}

/// The layers of the raft under `first`, everything printed on the first
/// layer of the model (model, supports, prime tower), printed by
/// `extruder`, with the skirt of `settings` around the raft on its lowest
/// layer. `None` if `raft_layers` is 0 or there is nothing to stand on the
/// raft. The model goes [`lift`] higher.
// ponytail: the whole raft prints with one extruder, the one of skirt and
// brim, even where supports or their interface have extruders of their
// own; a soluble top for the raft is the upgrade.
pub(crate) fn build(first: &[&Shapes], settings: &Settings, extruder: u32) -> Option<Vec<Layer>> {
    let count = settings.raft_layers as usize;
    if count == 0 {
        return None;
    }
    let width = settings.line_width;
    let all = first
        .iter()
        .fold(Shapes::new(), |all, shapes| polygon::union(&all, shapes));
    // Around the outside only: holes of the first layer are covered.
    // Grown one by one and joined: grown together, shapes that come to
    // overlap would leave their overlap out.
    let footprint = outer_contours(&all)
        .into_iter()
        .fold(Shapes::new(), |raft, outline| {
            polygon::union(
                &raft,
                &polygon::offset(&vec![vec![outline]], settings.raft_expansion),
            )
        });
    if footprint.is_empty() {
        return None;
    }
    let inner = polygon::offset(&footprint, -0.5 * width);
    let solid = (settings.support_interface_layers as usize).clamp(1, count);
    let mut layers = Vec::with_capacity(count);
    let mut z = 0.0;
    let mut position: Point = [0.0, 0.0];
    for index in 0..count {
        let height = if index == 0 {
            settings.first_layer_height
        } else {
            settings.layer_height
        };
        z += height;
        let mut work = Vec::new();
        if index == 0 {
            let skirt_only = Settings {
                brim_width: 0.0,
                ..settings.clone()
            };
            adhesion_work(&footprint, None, &skirt_only, &mut work);
        }
        let top = index + solid >= count;
        let (spacing, kind) = if top {
            (width, PathKind::SupportInterface)
        } else if index == 0 {
            (width / BASE_DENSITY, PathKind::Support)
        } else {
            (settings.support_spacing.max(width), PathKind::Support)
        };
        // Every layer crosses the one below it.
        let direction = if index % 2 == 0 {
            HatchDirection::Horizontal
        } else {
            HatchDirection::Vertical
        };
        work.push(Work::Loops(
            outer_contours(&inner),
            kind,
            width,
            SeamPosition::Nearest,
        ));
        let lines = polygon::hatch(&inner, spacing, direction, 0.5 * width);
        work.push(Work::Lines(lines, kind, 0.0));
        let mut paths: Vec<Path> = Vec::new();
        for piece in work {
            push_work(piece, &mut position, &mut paths);
        }
        // From one line to the next the nozzle stays over the raft; the
        // first path of a layer is reached from the layer below.
        let mut from: Option<Point> = None;
        for path in &mut paths {
            path.extruder = extruder;
            path.travel_inside = match (from, path.points.first()) {
                (Some(from), Some(&to)) if path.kind != PathKind::Skirt => {
                    polygon::segment_inside(&footprint, from, to)
                }
                _ => false,
            };
            from = if path.closed {
                path.points.first().copied()
            } else {
                path.points.last().copied()
            };
        }
        layers.push(Layer {
            z,
            height,
            paths,
            spiral: false,
        });
    }
    Some(layers)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use slicer_core::Mesh;

    fn square(x: f64, y: f64, size: f64) -> Shapes {
        vec![vec![vec![
            [x, y],
            [x + size, y],
            [x + size, y + size],
            [x, y + size],
        ]]]
    }

    #[test]
    fn a_raft_joins_what_stands_close_together() {
        let settings = Settings {
            raft_layers: 2,
            skirt_loops: 0,
            ..Settings::default()
        };
        // 2 mm apart, each grown by 1.5 mm: one raft.
        let (a, b) = (square(0.0, 0.0, 10.0), square(12.0, 0.0, 10.0));
        let layers = build(&[&a, &b], &settings, 0).unwrap();
        let loops = layers[1].paths.iter().filter(|p| p.closed).count();
        assert_eq!(loops, 1);
        // 5 mm apart: two.
        let c = square(15.0, 0.0, 10.0);
        let layers = build(&[&a, &c], &settings, 0).unwrap();
        let loops = layers[1].paths.iter().filter(|p| p.closed).count();
        assert_eq!(loops, 2);
    }

    #[test]
    fn the_model_stands_on_the_raft() {
        let mesh = Mesh::cuboid(10.0, 10.0, 2.0);
        let plain = Settings {
            brim_width: 3.0,
            ..Settings::default()
        };
        // Nearly solid, sparse, and two solid layers.
        let rafted = Settings {
            raft_layers: 4,
            ..plain.clone()
        };
        let without = crate::generate(&mesh, &plain).unwrap();
        let with = crate::generate(&mesh, &rafted).unwrap();
        assert_eq!(with.layers.len(), without.layers.len() + 4);
        // 0.2 + 3 * 0.2 mm of raft and the gap of 0.1 mm.
        let lift = lift(&rafted);
        assert!((lift - 0.9).abs() < 1e-9);
        for (model, plain) in with.layers[4..].iter().zip(&without.layers) {
            assert!((model.z - plain.z - lift).abs() < 1e-9);
        }
        // The skirt goes around the raft, which replaces the brim.
        let kinds = |layer: &Layer| layer.paths.iter().map(|p| p.kind).collect::<Vec<_>>();
        assert!(kinds(&with.layers[0]).contains(&PathKind::Skirt));
        assert!(with.layers[4..].iter().all(|l| {
            !kinds(l).contains(&PathKind::Skirt) && !kinds(l).contains(&PathKind::Brim)
        }));
        assert!(kinds(&without.layers[0]).contains(&PathKind::Brim));
        // Sparse in the middle, solid on top, wider than the model.
        let length = |layer: &Layer| layer.paths.iter().map(Path::length).sum::<f64>();
        assert!(length(&with.layers[0]) > 2.0 * length(&with.layers[1]));
        assert!(length(&with.layers[2]) > 2.0 * length(&with.layers[1]));
        let kinds_of = |index: usize| kinds(&with.layers[index]);
        assert!(kinds_of(1).iter().all(|&k| k == PathKind::Support));
        assert!(kinds_of(3).iter().all(|&k| k == PathKind::SupportInterface));
        let reach = with.layers[3]
            .paths
            .iter()
            .flat_map(|p| p.points.iter())
            .map(|p| p[0])
            .fold(f64::INFINITY, f64::min);
        let model = with.layers[4]
            .paths
            .iter()
            .flat_map(|p| p.points.iter())
            .map(|p| p[0])
            .fold(f64::INFINITY, f64::min);
        assert!((model - reach - 1.5).abs() < 0.3, "{model} {reach}");
    }

    #[test]
    fn a_raft_must_fit_under_the_printer() {
        let mesh = Mesh::cuboid(10.0, 10.0, 10.0);
        let settings = Settings {
            raft_layers: 3,
            max_print_height: 10.5,
            ..Settings::default()
        };
        assert!(matches!(
            crate::generate(&mesh, &settings),
            Err(crate::FdmError::TooLarge { .. })
        ));
    }
}
