//! Lightning infill: trees of straight lines that carry the top surfaces
//! from below and nothing else.
//!
//! The layers are planned from the top down. Every layer has a set of
//! points that need support from it: the points of a square grid under the
//! surfaces of the layer above (where its sparse area ends), and the points
//! carried down from the layer above. Every point is joined by a straight
//! line to the nearest of the boundary of the sparse area (the innermost
//! wall or solid infill) and the lines drawn so far, points closer to the
//! boundary first, which forms trees standing on the boundary. Going down,
//! every point moves towards what it was joined to, by as much as an
//! overhang of 45 degrees allows; points that get there disappear. So the
//! branches move towards each other and the walls, merge and get shorter
//! further down, and the line ends of every layer stand on the lines of the
//! layer below.

use slicer_core::polygon::{self, HatchDirection, distance};
use slicer_core::{Point, Shapes};

use crate::point_inside;

/// How far a point that needs support moves towards what carries it from
/// one layer to the one below, per millimetre of layer height. 1 is an
/// overhang of 45 degrees.
// ponytail: a fixed overhang of 45 degrees; steeper branches would save
// more material but need a setting and a way to tell what the printer
// manages.
const OVERHANG: f64 = 1.0;

/// How much longer than the straight way to the boundary the way from a
/// point along the tree may be.
// ponytail: a fixed ratio; longer detours would merge more branches and
// save material, but they also reach further down.
const DETOUR: f64 = 1.5;

/// Lightning infill of every layer of a region, as line segments.
/// `sparse[i]` is the sparse area of layer `i`, `heights[i]` its height.
/// The points that need support lie `spacing` apart; lines shorter than
/// `min_length` are left out.
pub(crate) fn plan(
    sparse: &[Shapes],
    heights: &[f64],
    spacing: f64,
    min_length: f64,
) -> Vec<Vec<[Point; 2]>> {
    let mut layers = vec![Vec::new(); sparse.len()];
    if spacing <= 0.0 {
        return layers;
    }
    // Points that need support from the layer being planned.
    let mut points: Vec<Point> = Vec::new();
    for index in (0..sparse.len()).rev() {
        let area = &sparse[index];
        if area.is_empty() {
            points.clear();
            continue;
        }
        let edges = edges(area);
        // Over solid infill, walls or nothing they need no more support.
        points.retain(|&p| point_inside(area, p));
        // What has no sparse area above it carries a surface.
        let carrying = match sparse.get(index + 1) {
            Some(above) => polygon::difference(area, above),
            None => area.clone(),
        };
        // Points closer to the boundary than half the spacing rest on it.
        for p in grid(&carrying, spacing) {
            if nearest(&edges, p).0 >= 0.5 * spacing
                && points.iter().all(|&q| distance(p, q) >= 0.5 * spacing)
            {
                points.push(p);
            }
        }
        let (lines, targets) = tree(&points, &edges);
        layers[index] = lines
            .into_iter()
            .filter(|line| distance(line[0], line[1]) >= min_length)
            .collect();
        // Towards what carries them, by the overhang the layer allows.
        let step = OVERHANG * heights[index];
        let mut below: Vec<Point> = Vec::with_capacity(points.len());
        for (&p, &target) in points.iter().zip(&targets) {
            let d = distance(p, target);
            if d <= step {
                continue;
            }
            let t = step / d;
            let moved = [p[0] + t * (target[0] - p[0]), p[1] + t * (target[1] - p[1])];
            // Branches that meet go on as one.
            if below.iter().all(|&q| distance(moved, q) >= min_length) {
                below.push(moved);
            }
        }
        points = below;
    }
    layers
}

/// The edges of all contours of the shapes.
fn edges(shapes: &Shapes) -> Vec<[Point; 2]> {
    shapes
        .iter()
        .flatten()
        .flat_map(|contour| {
            let n = contour.len();
            (0..n).map(move |i| [contour[i], contour[(i + 1) % n]])
        })
        .collect()
}

/// The points of a square grid, `spacing` apart and fixed on the bed, that
/// lie inside the shapes.
// ponytail: an axis-parallel square grid; following `infill_angle` or a
// triangular grid would not change how much is carried.
fn grid(shapes: &Shapes, spacing: f64) -> Vec<Point> {
    // Hatch lines run at y = (k + 0.5) * spacing; the points on them at
    // x = (j + 0.5) * spacing.
    let mut points = Vec::new();
    for [a, b] in polygon::hatch(shapes, spacing, HatchDirection::Horizontal, 0.0) {
        let (lo, hi) = (a[0].min(b[0]), a[0].max(b[0]));
        let first = (lo / spacing - 0.5).ceil() as i64;
        let last = (hi / spacing - 0.5).floor() as i64;
        for j in first..=last {
            points.push([(j as f64 + 0.5) * spacing, a[1]]);
        }
    }
    points
}

/// The nearest point to `p` on any of the segments, and its distance.
// ponytail: tries every edge (number of points times number of edges per
// layer); a spatial index would help on very detailed outlines.
fn nearest(segments: &[[Point; 2]], p: Point) -> (f64, Point) {
    let mut best = (f64::INFINITY, p);
    for &[a, b] in segments {
        // No point of the segment can be closer than its bounding box.
        let dx = (a[0].min(b[0]) - p[0]).max(p[0] - a[0].max(b[0]));
        let dy = (a[1].min(b[1]) - p[1]).max(p[1] - a[1].max(b[1]));
        if dx >= best.0 || dy >= best.0 {
            continue;
        }
        let q = closest_on_segment(a, b, p);
        let d = distance(p, q);
        if d < best.0 {
            best = (d, q);
        }
    }
    best
}

/// The point of the segment from `a` to `b` closest to `p`.
fn closest_on_segment(a: Point, b: Point, p: Point) -> Point {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length2 = dx * dx + dy * dy;
    if length2 <= 0.0 {
        return a;
    }
    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / length2).clamp(0.0, 1.0);
    [a[0] + t * dx, a[1] + t * dy]
}

/// Joins every point to the nearest of the boundary (`edges`) and the
/// lines drawn before, points closer to the boundary first. Returns the
/// lines and, for every point, where its line ends.
///
/// A line is never longer than the distance from its point to the
/// boundary, so it stays inside the area. A point joins a line only if the
/// way from it along the tree to the boundary is at most [`DETOUR`] times
/// as long as the straight way; otherwise points along a wall would join
/// into one long line beside it, which takes long to shrink.
// ponytail: the tree is planned anew on every layer and tries every line
// drawn before for every point; keeping the branches from layer to layer
// would make them steadier.
fn tree(points: &[Point], edges: &[[Point; 2]]) -> (Vec<[Point; 2]>, Vec<Point>) {
    let walls: Vec<(f64, Point)> = points.iter().map(|&p| nearest(edges, p)).collect();
    let mut order: Vec<usize> = (0..points.len()).collect();
    order.sort_by(|&a, &b| walls[a].0.total_cmp(&walls[b].0).then(a.cmp(&b)));
    let mut lines: Vec<[Point; 2]> = Vec::with_capacity(points.len());
    // Length of the way along the tree from the end of every line to the
    // boundary.
    let mut ways: Vec<f64> = Vec::with_capacity(points.len());
    let mut targets = vec![[0.0, 0.0]; points.len()];
    for i in order {
        let p = points[i];
        let (straight, wall) = walls[i];
        let (mut best, mut target, mut way) = (straight, wall, 0.0);
        for (&[a, b], &rest) in lines.iter().zip(&ways) {
            // No point of the line can be closer than its bounding box.
            let dx = (a[0].min(b[0]) - p[0]).max(p[0] - a[0].max(b[0]));
            let dy = (a[1].min(b[1]) - p[1]).max(p[1] - a[1].max(b[1]));
            if dx >= best || dy >= best {
                continue;
            }
            let q = closest_on_segment(a, b, p);
            let d = distance(p, q);
            let along = distance(q, b) + rest;
            if d < best && d + along <= DETOUR * straight {
                (best, target, way) = (d, q, along);
            }
        }
        lines.push([p, target]);
        ways.push(way);
        targets[i] = target;
    }
    (lines, targets)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(size: f64) -> Shapes {
        vec![vec![vec![
            [0.0, 0.0],
            [size, 0.0],
            [size, size],
            [0.0, size],
        ]]]
    }

    #[test]
    fn grid_points_lie_inside_and_apart() {
        let points = grid(&square(10.0), 2.0);
        assert_eq!(points.len(), 25);
        assert!(points.contains(&[1.0, 1.0]) && points.contains(&[9.0, 9.0]));
    }

    #[test]
    fn branches_shrink_towards_the_walls() {
        // A sparse square of 20 mm under a roof on layer 99.
        let mut sparse = vec![square(20.0); 100];
        sparse.push(Vec::new());
        let heights = vec![0.2; 101];
        let layers = plan(&sparse, &heights, 2.0, 0.2);
        let length = |lines: &[[Point; 2]]| -> f64 {
            lines.iter().map(|line| distance(line[0], line[1])).sum()
        };
        // Every point of the grid is carried right under the roof.
        assert!(length(&layers[99]) > 90.0, "{}", length(&layers[99]));
        // Going down there is less and less, and 10 mm from the walls
        // (the middle) a 45 degree branch ends after 50 layers at most.
        for index in 1..100 {
            assert!(length(&layers[index - 1]) <= length(&layers[index]) + 1e-6);
        }
        assert!(layers[..30].iter().all(Vec::is_empty));
        // Every line stays inside the square.
        for line in layers.iter().flatten() {
            for p in line {
                assert!((-1e-9..=20.0 + 1e-9).contains(&p[0]), "{p:?}");
                assert!((-1e-9..=20.0 + 1e-9).contains(&p[1]), "{p:?}");
            }
        }
    }
}
