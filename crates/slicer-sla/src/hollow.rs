//! Hollowing: the model keeps a wall of `hollow_wall` millimetres all round,
//! sides, top and bottom; everything further inside becomes a cavity. Drain
//! holes are drilled straight down from the lowest points of every cavity.
//!
//! A point of a layer belongs to the cavity when a ball of radius `wall`
//! around it lies inside the model (erosion by a ball, textbook
//! morphology). Layer `i ± k` lies `k · layer_height` away, where the ball
//! is a disc of radius `sqrt(wall² − (k · layer_height)²)`; so the cavity is
//! the layer shrunk by `wall`, intersected with the layers above and below
//! shrunk by those radii.

use slicer_core::polygon::{difference, intersection, offset};
use slicer_core::{Diagnostic, Diagnostics, Point, Shape, Shapes, parallel_map};

use crate::{SlaSettings, disc, unit_circle};

/// Most layers above (and as many below) a layer that are compared with it.
// ponytail: with a wall thicker than this many layers only that many
// evenly spaced layers are compared, so the inner surface of a slope may
// come out up to a fraction of a layer thinner between them. Comparing
// every layer is exact and slower.
const SAMPLED_LAYERS: usize = 8;

/// Hollows `slices` (the model's layers, from the build plate up) in place
/// and drills the drain holes. Returns the centres of the drain holes.
pub(crate) fn hollow(
    slices: &mut [Shapes],
    settings: &SlaSettings,
    diagnostics: &mut Diagnostics,
) -> Vec<Point> {
    let wall = settings.hollow_wall;
    let height = settings.layer_height;
    // At least one layer, so a wall thinner than a layer still closes the
    // cavity at top and bottom.
    let reach = ((wall / height + 1e-9).floor() as usize).max(1);
    let distances: Vec<usize> = if reach <= SAMPLED_LAYERS {
        (1..=reach).collect()
    } else {
        (1..=SAMPLED_LAYERS)
            .map(|m| (m * reach + SAMPLED_LAYERS / 2) / SAMPLED_LAYERS)
            .collect()
    };
    let radii: Vec<f64> = std::iter::once(wall)
        .chain(distances.iter().map(|&k| {
            let dz = k as f64 * height;
            (wall * wall - dz * dz).max(0.0).sqrt()
        }))
        .collect();
    let count = slices.len();
    let solid = slices.to_vec();
    let shrunk: Vec<Vec<Shapes>> = parallel_map(count, |j| {
        radii.iter().map(|&r| offset(&solid[j], -r)).collect()
    });
    let cavities: Vec<Shapes> = parallel_map(count, |i| {
        let mut cavity = shrunk[i][0].clone();
        for (m, &k) in distances.iter().enumerate() {
            if cavity.is_empty() {
                break;
            }
            if i < k || i + k >= count {
                return Vec::new();
            }
            cavity = intersection(&cavity, &shrunk[i - k][m + 1]);
            cavity = intersection(&cavity, &shrunk[i + k][m + 1]);
        }
        cavity
    });
    let hollowed = parallel_map(count, |i| difference(&solid[i], &cavities[i]));
    for (slice, hollowed) in slices.iter_mut().zip(hollowed) {
        *slice = hollowed;
    }

    let mut drains = Vec::new();
    if cavities.iter().all(Vec::is_empty) {
        return drains;
    }
    if settings.drain_hole_diameter == 0.0 {
        diagnostics.push(Diagnostic::warning(
            "sla.sealed_cavity",
            "the hollowed model has no drain holes; uncured resin stays trapped inside".to_owned(),
        ));
        return drains;
    }
    let unit = unit_circle(32);
    let first_layer = solid.iter().position(|s| !s.is_empty()).unwrap_or(0);
    let mut on_plate = false;
    for i in 0..count {
        for shape in &cavities[i] {
            // A cavity region that overlaps the layer below continues a
            // cavity that already has its hole.
            if i > 0 && !intersection(&vec![shape.clone()], &cavities[i - 1]).is_empty() {
                continue;
            }
            let at = inside_point(shape);
            let hole = vec![vec![disc(&unit, at, settings.drain_hole_diameter / 2.0)]];
            let mut j = i;
            while j > 0 && !intersection(&solid[j - 1], &hole).is_empty() {
                j -= 1;
                slices[j] = difference(&slices[j], &hole);
            }
            on_plate |= j == first_layer && settings.elevation == 0.0;
            drains.push(at);
        }
    }
    if on_plate {
        diagnostics.push(Diagnostic::warning(
            "sla.drain_on_plate",
            "a drain hole opens at the bottom of the model, where the build plate or raft closes it; raise the model with `elevation`".to_owned(),
        ));
    }
    drains
}

/// A point inside a shape: the middle of the widest stretch of the
/// horizontal line half way up the outer contour that lies inside.
fn inside_point(shape: &Shape) -> Point {
    let outer = &shape[0];
    let (low, high) = outer
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), p| {
            (low.min(p[1]), high.max(p[1]))
        });
    let y = (low + high) / 2.0;
    let mut crossings: Vec<f64> = Vec::new();
    for contour in shape {
        for (k, &p) in contour.iter().enumerate() {
            let q = contour[(k + 1) % contour.len()];
            if (p[1] > y) != (q[1] > y) {
                crossings.push(p[0] + (y - p[1]) / (q[1] - p[1]) * (q[0] - p[0]));
            }
        }
    }
    crossings.sort_by(f64::total_cmp);
    crossings
        .as_chunks::<2>()
        .0
        .iter()
        .max_by(|a, b| (a[1] - a[0]).total_cmp(&(b[1] - b[0])))
        .map_or(outer[0], |[left, right]| [(left + right) / 2.0, y])
}
