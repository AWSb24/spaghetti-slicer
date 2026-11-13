//! 2D polygon types and operations.
//!
//! Coordinates are `f64` millimetres. Boolean operations and offsetting are
//! delegated to the `i_overlay` crate, which computes on integers. All
//! results lie on a fixed grid of [`GRID`] millimetres, so the outcome of an
//! operation does not depend on where the geometry sits on the bed.
//!
//! This module is the only place that touches `i_overlay`.

use std::f64::consts::{FRAC_1_SQRT_2, FRAC_PI_4};

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::scale::FixedScaleFloatOverlay;
use i_overlay::float::single::SingleFloatOverlay;
use i_overlay::mesh::float::outline::offset::OutlineOffset;
use i_overlay::mesh::float::style::{LineJoin, OutlineStyle};

/// A point in the XY plane, in millimetres.
pub type Point = [f64; 2];
/// A closed loop of points; the last point connects back to the first.
pub type Contour = Vec<Point>;
/// One outer contour (counter-clockwise) followed by its holes (clockwise).
pub type Shape = Vec<Contour>;
/// A set of non-overlapping shapes.
pub type Shapes = Vec<Shape>;

/// Spacing of the grid that results of polygon operations are snapped to:
/// 10 nanometres.
pub const GRID: f64 = 1e-5;
/// Grid units per millimetre.
const SCALE: f64 = 1e5;
/// Largest distance between a rounded corner and its polygonal
/// approximation, in millimetres.
const ARC_TOLERANCE: f64 = 0.0025;

/// Removes floating-point noise by moving every point onto the grid.
/// Width in millimetres below which a detour of a contour counts as a spike.
const SPIKE_WIDTH: f64 = 1e-4;

fn snap(mut shapes: Shapes) -> Shapes {
    for point in shapes.iter_mut().flatten().flatten() {
        point[0] = (point[0] * SCALE).round() / SCALE;
        point[1] = (point[1] * SCALE).round() / SCALE;
    }
    shapes
}

/// Turns arbitrarily oriented, possibly nested loops into shapes using the
/// even-odd rule.
pub fn from_loops(loops: &[Contour]) -> Shapes {
    if loops.is_empty() {
        return Vec::new();
    }
    let (loops, none) = (loops.to_vec(), Vec::<Contour>::new());
    let (rule, fill) = (OverlayRule::Subject, FillRule::EvenOdd);
    snap(
        loops
            .overlay_with_fixed_scale(&none, rule, fill, SCALE)
            .unwrap_or_else(|_| loops.overlay(&none, rule, fill)),
    )
}

/// Unites loops whose direction is meaningful: counter-clockwise loops add
/// material, clockwise loops inside them cut holes. Unlike [`from_loops`],
/// overlapping counter-clockwise loops merge instead of cancelling out.
pub fn from_oriented_loops(loops: &[Contour]) -> Shapes {
    if loops.is_empty() {
        return Vec::new();
    }
    let (loops, none) = (loops.to_vec(), Vec::<Contour>::new());
    let (rule, fill) = (OverlayRule::Subject, FillRule::NonZero);
    snap(
        loops
            .overlay_with_fixed_scale(&none, rule, fill, SCALE)
            .unwrap_or_else(|_| loops.overlay(&none, rule, fill)),
    )
}

/// Runs a boolean operation on the fixed grid. Geometry too far from the
/// origin for the fixed grid (several metres) falls back to a grid chosen
/// from the bounding box.
fn boolean(a: &Shapes, b: &Shapes, rule: OverlayRule) -> Shapes {
    let fill = FillRule::NonZero;
    snap(
        a.overlay_with_fixed_scale(b, rule, fill, SCALE)
            .unwrap_or_else(|_| a.overlay(b, rule, fill)),
    )
}

/// Area covered by `a` or `b`.
pub fn union(a: &Shapes, b: &Shapes) -> Shapes {
    match (a.is_empty(), b.is_empty()) {
        (true, _) => b.clone(),
        (_, true) => a.clone(),
        _ => boolean(a, b, OverlayRule::Union),
    }
}

/// Area covered by both `a` and `b`.
pub fn intersection(a: &Shapes, b: &Shapes) -> Shapes {
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    boolean(a, b, OverlayRule::Intersect)
}

/// Area covered by `a` but not by `b`.
pub fn difference(a: &Shapes, b: &Shapes) -> Shapes {
    if a.is_empty() || b.is_empty() {
        return a.clone();
    }
    boolean(a, b, OverlayRule::Difference)
}

/// Grows (`delta > 0`) or shrinks (`delta < 0`) shapes by `delta` millimetres
/// with rounded corners. Shapes that shrink to nothing disappear.
pub fn offset(shapes: &Shapes, delta: f64) -> Shapes {
    if shapes.is_empty() || delta == 0.0 {
        return shapes.clone();
    }
    // Angle per arc segment that keeps the chord within the tolerance.
    // Exactly 2 * acos(1 - t); sqrt(2t) is a slightly finer approximation and,
    // unlike acos, gives the same bits on every platform.
    let t = (ARC_TOLERANCE / delta.abs()).min(1.0);
    let step = 2.0 * (2.0 * t).sqrt();
    let style = OutlineStyle::new(delta).line_join(LineJoin::Round(step.min(FRAC_PI_4)));
    let grown = snap(
        shapes
            .outline_fixed_scale(&style, SCALE)
            .unwrap_or_else(|_| shapes.outline(&style)),
    );
    grown
        .into_iter()
        .filter_map(|shape| {
            let mut contours = shape.into_iter().map(without_spikes);
            let outer = contours.next().filter(|c| c.len() > 2)?;
            Some(
                std::iter::once(outer)
                    .chain(contours.filter(|c| c.len() > 2))
                    .collect(),
            )
        })
        .collect()
}

/// Drops points where a contour runs out and straight back on itself. At a
/// point where the input barely bends, the offset now and then leaves such a
/// spike of no width reaching back to the input; printed as a wall it would
/// be a line across the other walls.
fn without_spikes(mut contour: Contour) -> Contour {
    let mut i = 0;
    let mut unchanged = 0;
    while contour.len() > 2 && unchanged < contour.len() {
        let n = contour.len();
        let (a, b, c) = (
            contour[(i + n - 1) % n],
            contour[i % n],
            contour[(i + 1) % n],
        );
        let back = (b[0] - a[0]) * (c[0] - b[0]) + (b[1] - a[1]) * (c[1] - b[1]) < 0.0;
        let on_line = segment_distance(c, a, b).min(segment_distance(a, b, c)) < SPIKE_WIDTH;
        if (back && on_line) || distance(a, b) < SPIKE_WIDTH {
            contour.remove(i % n);
            i = (i % n + n - 1) % n;
            unchanged = 0;
        } else {
            i = (i + 1) % n;
            unchanged += 1;
        }
    }
    contour
}

/// Removes debris from shapes: points closer than `min_segment` to their
/// predecessor, holes smaller than `min_area` and shapes whose outer
/// contour is smaller than `min_area`.
///
/// A cutting plane that grazes a nearly horizontal face produces hundreds
/// of microscopic slivers. They cannot be printed and make offsetting
/// extremely slow.
pub fn clean(shapes: Shapes, min_area: f64, min_segment: f64) -> Shapes {
    let tidy = |contour: Contour| -> Option<Contour> {
        let mut kept: Contour = Vec::with_capacity(contour.len());
        for point in contour {
            if kept
                .last()
                .is_none_or(|&last| distance(last, point) >= min_segment)
            {
                kept.push(point);
            }
        }
        while kept.len() > 1 && distance(kept[0], kept[kept.len() - 1]) < min_segment {
            kept.pop();
        }
        (kept.len() >= 3 && signed_area(&kept).abs() >= min_area).then_some(kept)
    };
    shapes
        .into_iter()
        .filter_map(|shape| {
            let mut contours = shape.into_iter();
            let outer = tidy(contours.next()?)?;
            Some(
                std::iter::once(outer)
                    .chain(contours.filter_map(tidy))
                    .collect(),
            )
        })
        .collect()
}

/// Distance from `p` to the segment from `a` to `b`.
fn segment_distance(p: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length2 = dx * dx + dy * dy;
    if length2 == 0.0 {
        return distance(p, a);
    }
    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / length2).clamp(0.0, 1.0);
    distance(p, [a[0] + t * dx, a[1] + t * dy])
}

/// Thins out a closed loop: drops points as long as the loop moves by no
/// more than `tolerance` (Ramer-Douglas-Peucker). Corners stay where they
/// are; curves get fewer, longer segments.
pub fn simplify(contour: &[Point], tolerance: f64) -> Contour {
    let n = contour.len();
    if n < 4 {
        return contour.to_vec();
    }
    // Split the loop at point 0 and at the point farthest from it, then
    // treat both halves as open lines. `ring` repeats point 0 at the end.
    let far = (1..n)
        .max_by(|&a, &b| {
            distance(contour[0], contour[a]).total_cmp(&distance(contour[0], contour[b]))
        })
        .unwrap_or(1);
    let mut ring = contour.to_vec();
    ring.push(contour[0]);
    let mut keep = vec![false; n + 1];
    keep[0] = true;
    keep[far] = true;
    let mut pending = vec![(0, far), (far, n)];
    while let Some((lo, hi)) = pending.pop() {
        let mut worst = (lo, 0.0);
        for i in lo + 1..hi {
            let d = segment_distance(ring[i], ring[lo], ring[hi]);
            if d > worst.1 {
                worst = (i, d);
            }
        }
        if worst.1 > tolerance {
            keep[worst.0] = true;
            pending.push((lo, worst.0));
            pending.push((worst.0, hi));
        }
    }
    (0..n).filter(|&i| keep[i]).map(|i| contour[i]).collect()
}

/// Whether the straight line from `a` to `b` stays inside the shapes: it
/// starts inside and neither crosses nor touches any contour.
pub fn segment_inside(shapes: &[Shape], a: Point, b: Point) -> bool {
    let cross = |o: Point, p: Point, q: Point| {
        (p[0] - o[0]) * (q[1] - o[1]) - (p[1] - o[1]) * (q[0] - o[0])
    };
    let (min_x, max_x) = (a[0].min(b[0]), a[0].max(b[0]));
    let (min_y, max_y) = (a[1].min(b[1]), a[1].max(b[1]));
    let mut inside = false;
    for contour in shapes.iter().flatten() {
        for (i, &p) in contour.iter().enumerate() {
            let q = contour[(i + 1) % contour.len()];
            // Even-odd count of the edges a ray from `a` to the right crosses.
            if (p[1] > a[1]) != (q[1] > a[1])
                && a[0] < p[0] + (a[1] - p[1]) / (q[1] - p[1]) * (q[0] - p[0])
            {
                inside = !inside;
            }
            if p[0].max(q[0]) < min_x
                || p[0].min(q[0]) > max_x
                || p[1].max(q[1]) < min_y
                || p[1].min(q[1]) > max_y
            {
                continue;
            }
            // The edge and the line share a point unless one lies strictly
            // on one side of the other.
            let (d1, d2) = (cross(a, b, p), cross(a, b, q));
            let (d3, d4) = (cross(p, q, a), cross(p, q, b));
            let apart = (d1 > 0.0 && d2 > 0.0)
                || (d1 < 0.0 && d2 < 0.0)
                || (d3 > 0.0 && d4 > 0.0)
                || (d3 < 0.0 && d4 < 0.0);
            if !apart {
                return false;
            }
        }
    }
    inside
}

/// Convex hull of a set of points as a counter-clockwise contour (Andrew's
/// monotone chain). Fewer than three distinct points give them back as is.
pub fn convex_hull(points: &[Point]) -> Contour {
    let mut sorted = points.to_vec();
    sorted.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    sorted.dedup();
    if sorted.len() < 3 {
        return sorted;
    }
    let cross = |o: Point, a: Point, b: Point| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let mut hull: Contour = Vec::new();
    // Lower chain left to right, then upper chain back.
    for pass in 0..2 {
        let start = hull.len();
        let chain: Box<dyn Iterator<Item = &Point>> = if pass == 0 {
            Box::new(sorted.iter())
        } else {
            Box::new(sorted.iter().rev())
        };
        for &point in chain {
            while hull.len() >= start + 2
                && cross(hull[hull.len() - 2], hull[hull.len() - 1], point) <= 0.0
            {
                hull.pop();
            }
            hull.push(point);
        }
        hull.pop();
    }
    hull
}

/// Signed area of a contour: positive when counter-clockwise.
pub fn signed_area(contour: &[Point]) -> f64 {
    let n = contour.len();
    (0..n)
        .map(|i| {
            let (p, q) = (contour[i], contour[(i + 1) % n]);
            p[0] * q[1] - q[0] * p[1]
        })
        .sum::<f64>()
        / 2.0
}

/// Total filled area of a set of shapes (holes subtracted).
pub fn area(shapes: &[Shape]) -> f64 {
    shapes
        .iter()
        .map(|shape| {
            let mut contours = shape.iter().map(|c| signed_area(c).abs());
            let outer = contours.next().unwrap_or(0.0);
            outer - contours.sum::<f64>()
        })
        .sum()
}

/// Euclidean distance between two points.
pub fn distance(a: Point, b: Point) -> f64 {
    // Not `hypot`: that calls the platform's maths library, whose last bit
    // may differ between operating systems. `sqrt` is exact everywhere.
    let (dx, dy) = (a[0] - b[0], a[1] - b[1]);
    (dx * dx + dy * dy).sqrt()
}

/// Direction of hatch lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HatchDirection {
    /// Lines running from lower left to upper right.
    Diagonal45,
    /// Lines running from lower right to upper left.
    Diagonal135,
    /// Lines along X.
    Horizontal,
    /// Lines along Y.
    Vertical,
}

/// Fills shapes with parallel line segments `spacing` millimetres apart.
///
/// Lines lie on a grid anchored at the origin, so the same direction lines up
/// across layers. Consecutive lines alternate direction. Segments shorter
/// than `min_length` are dropped.
pub fn hatch(
    shapes: &[Shape],
    spacing: f64,
    direction: HatchDirection,
    min_length: f64,
) -> Vec<[Point; 2]> {
    if spacing <= 0.0 {
        return Vec::new();
    }
    // Rotated frame (a, b): hatch lines have constant b and run along a.
    let forward = |p: Point| -> (f64, f64) {
        let u = (p[0] + p[1]) * FRAC_1_SQRT_2;
        let v = (p[1] - p[0]) * FRAC_1_SQRT_2;
        match direction {
            HatchDirection::Diagonal45 => (u, v),
            HatchDirection::Diagonal135 => (v, u),
            HatchDirection::Horizontal => (p[0], p[1]),
            HatchDirection::Vertical => (p[1], p[0]),
        }
    };
    let backward = |a: f64, b: f64| -> Point {
        let (u, v) = match direction {
            HatchDirection::Diagonal45 => (a, b),
            HatchDirection::Diagonal135 => (b, a),
            HatchDirection::Horizontal => return [a, b],
            HatchDirection::Vertical => return [b, a],
        };
        [(u - v) * FRAC_1_SQRT_2, (u + v) * FRAC_1_SQRT_2]
    };
    let line_b = |k: i64| (k as f64 + 0.5) * spacing;

    // Crossings of every edge with every hatch line, grouped by line index.
    let mut crossings: Vec<(i64, f64)> = Vec::new();
    for contour in shapes.iter().flatten() {
        let n = contour.len();
        for i in 0..n {
            let (a0, b0) = forward(contour[i]);
            let (a1, b1) = forward(contour[(i + 1) % n]);
            if b0 == b1 {
                continue;
            }
            let (lo, hi) = (b0.min(b1), b0.max(b1));
            let mut k = (lo / spacing - 0.5).ceil() as i64;
            // Half-open interval [lo, hi) so shared vertices count once.
            while line_b(k) < hi {
                let b = line_b(k);
                if b >= lo {
                    crossings.push((k, a0 + (b - b0) / (b1 - b0) * (a1 - a0)));
                }
                k += 1;
            }
        }
    }
    crossings.sort_by(|x, y| x.0.cmp(&y.0).then(x.1.total_cmp(&y.1)));

    let mut segments = Vec::new();
    let mut start = 0;
    while start < crossings.len() {
        let k = crossings[start].0;
        let end = start + crossings[start..].iter().take_while(|c| c.0 == k).count();
        let b = line_b(k);
        let mut line: Vec<[Point; 2]> = crossings[start..end]
            .as_chunks::<2>()
            .0
            .iter()
            .filter(|pair| pair[1].1 - pair[0].1 >= min_length)
            .map(|pair| [backward(pair[0].1, b), backward(pair[1].1, b)])
            .collect();
        if k.rem_euclid(2) == 1 {
            line.reverse();
            for segment in &mut line {
                segment.swap(0, 1);
            }
        }
        segments.extend(line);
        start = end;
    }
    segments
}

#[cfg(test)]
mod tests {
    #[test]
    fn simplify_keeps_corners_and_thins_curves() {
        // A square with extra points on its edges, some slightly off.
        let square = [
            [0.0, 0.0],
            [5.0, 0.001],
            [10.0, 0.0],
            [10.0, 4.0],
            [10.0, 10.0],
            [3.0, 10.0],
            [0.0, 10.0],
            [-0.002, 5.0],
        ];
        assert_eq!(
            super::simplify(&square, 0.01),
            [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]
        );
        // With a tighter tolerance the points that are off stay.
        assert_eq!(super::simplify(&square, 0.0005).len(), 6);

        // A circle of 720 points keeps its shape within the tolerance.
        let circle: Vec<super::Point> = (0..720)
            .map(|i| {
                let angle = f64::from(i) * std::f64::consts::TAU / 720.0;
                [10.0 * angle.cos(), 10.0 * angle.sin()]
            })
            .collect();
        let thinned = super::simplify(&circle, 0.0125);
        assert!(
            thinned.len() < 120 && thinned.len() > 40,
            "{}",
            thinned.len()
        );
        let lost = super::signed_area(&circle) - super::signed_area(&thinned);
        assert!((0.0..0.5).contains(&lost), "{lost}");
        assert_eq!(super::simplify(&circle[..3], 1.0), circle[..3]);
    }

    #[test]
    fn segment_inside_respects_outline_and_holes() {
        let outer = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let hole = vec![[4.0, 4.0], [4.0, 6.0], [6.0, 6.0], [6.0, 4.0]];
        let shapes = vec![vec![outer, hole]];
        let inside = |a, b| super::segment_inside(&shapes, a, b);
        assert!(inside([1.0, 1.0], [9.0, 1.0]));
        assert!(inside([1.0, 1.0], [1.0, 9.0]));
        // Across the hole, out of the shape, starting in the hole or outside.
        assert!(!inside([1.0, 5.0], [9.0, 5.0]));
        assert!(!inside([1.0, 1.0], [12.0, 1.0]));
        assert!(!inside([5.0, 5.0], [5.2, 5.2]));
        assert!(!inside([-1.0, 1.0], [-2.0, 1.0]));
        // Grazing a corner of the hole counts as leaving.
        assert!(!inside([2.0, 4.0], [5.0, 4.0]));
    }

    #[test]
    fn offset_leaves_no_spikes() {
        // The outlines of a cylinder keep a point in the middle of one
        // side. Without the clean-up, 5 of these 200 offsets have a spike
        // from the offset circle out to the cylinder.
        let mesh = crate::primitives::cylinder(10.0, 20.0, 64);
        let heights: Vec<f64> = (0..100).map(|i| 0.1 + 0.2 * f64::from(i)).collect();
        for slice in crate::slice::slice_mesh(&mesh, &heights) {
            for delta in [-0.225, -0.675] {
                let grown = offset(&slice, delta);
                assert_eq!(grown.len(), 1);
                let radii: Vec<f64> = grown[0][0]
                    .iter()
                    .map(|&point| distance(point, [0.0, 0.0]))
                    .collect();
                let least = radii.iter().copied().fold(f64::MAX, f64::min);
                let most = radii.iter().copied().fold(0.0, f64::max);
                assert!(most - least < 0.02, "{delta}: {least} to {most}");
            }
        }
        // A spike on its own is removed, a sharp corner is not.
        let spiked = vec![
            [0.0, 0.0],
            [4.0, 0.0],
            [4.0, 3.0],
            [4.0, 0.0],
            [4.0, -4.0],
            [0.0, -4.0],
        ];
        assert_eq!(super::without_spikes(spiked).len(), 4);
        let sharp = vec![[0.0, 0.0], [4.0, 0.0], [0.0, 0.1]];
        assert_eq!(super::without_spikes(sharp).len(), 3);
    }

    #[test]
    fn convex_hull_drops_inner_and_collinear_points() {
        let points = [
            [4.0, 4.0],
            [0.0, 0.0],
            [2.0, 0.0],
            [1.0, 1.0],
            [4.0, 0.0],
            [0.0, 4.0],
            [0.0, 0.0],
            [3.0, 2.0],
        ];
        assert_eq!(
            super::convex_hull(&points),
            [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]]
        );
        assert_eq!(super::convex_hull(&points[..1]), [[4.0, 4.0]]);
    }

    use super::*;

    fn square(x: f64, y: f64, size: f64) -> Contour {
        vec![[x, y], [x + size, y], [x + size, y + size], [x, y + size]]
    }

    #[test]
    fn nested_loops_become_a_shape_with_a_hole() {
        // Deliberately inconsistent orientation: even-odd must sort it out.
        let shapes = from_loops(&[square(0.0, 0.0, 20.0), square(5.0, 5.0, 10.0)]);
        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].len(), 2);
        assert!((area(&shapes) - 300.0).abs() < 1e-3);
        assert!(signed_area(&shapes[0][0]) > 0.0);
        assert!(signed_area(&shapes[0][1]) < 0.0);
    }

    #[test]
    fn offset_shrinks_grows_and_collapses() {
        let shapes = from_loops(&[square(0.0, 0.0, 20.0)]);
        assert!((area(&offset(&shapes, -1.0)) - 324.0).abs() < 1e-2);
        // Growing rounds the corners: 400 + 4*20*1 + pi*1^2, slightly less
        // because the arcs are approximated by chords.
        let grown = area(&offset(&shapes, 1.0));
        assert!(grown > 482.9 && grown < 483.2, "{grown}");
        assert!(offset(&shapes, -10.5).is_empty());
    }

    #[test]
    fn offset_of_a_hole_moves_the_other_way() {
        let shapes = from_loops(&[square(0.0, 0.0, 20.0), square(5.0, 5.0, 10.0)]);
        let shrunk = offset(&shapes, -1.0);
        // Outer becomes 18x18, the hole grows to roughly 12x12 with round corners.
        let a = area(&shrunk);
        assert!(a > 324.0 - 144.0 && a < 324.0 - 143.0, "{a}");
    }

    #[test]
    fn booleans() {
        let a = from_loops(&[square(0.0, 0.0, 10.0)]);
        let b = from_loops(&[square(5.0, 0.0, 10.0)]);
        assert!((area(&union(&a, &b)) - 150.0).abs() < 1e-3);
        assert!((area(&intersection(&a, &b)) - 50.0).abs() < 1e-3);
        assert!((area(&difference(&a, &b)) - 50.0).abs() < 1e-3);
        assert!(intersection(&a, &Vec::new()).is_empty());
        assert_eq!(difference(&a, &Vec::new()), a);
    }

    #[test]
    fn results_lie_on_the_grid_regardless_of_surroundings() {
        // The same square, offset next to a far-away second shape, must give
        // bit-identical coordinates.
        let run = |far: f64| -> Contour {
            let shapes = from_loops(&[square(100.0, 100.0, 20.0), square(130.0 + far, 100.0, 1.0)]);
            let mut big: Contour = offset(&shapes, -0.123457)
                .into_iter()
                .flatten()
                .filter(|c| signed_area(c).abs() > 10.0)
                .flatten()
                .collect();
            big.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
            big
        };
        let near = run(0.0);
        assert_eq!(near.len(), 4);
        assert_eq!(near, run(50.0));
        assert_eq!(near, run(1000.0));
        assert_eq!(near[0], [100.12346, 100.12346]);
    }

    #[test]
    fn geometry_beyond_the_fixed_grid_still_works() {
        let shapes = from_loops(&[square(5.0e6, 5.0e6, 20.0)]);
        assert!((area(&shapes) - 400.0).abs() < 1.0);
        assert!((area(&offset(&shapes, -1.0)) - 324.0).abs() < 1.0);
        assert!((area(&intersection(&shapes, &shapes)) - 400.0).abs() < 1.0);
    }

    #[test]
    fn clean_drops_slivers_and_duplicate_points() {
        let big = vec![
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 0.0002], // closer than the minimum segment
            [10.0, 10.0],
            [0.0, 10.0],
            [0.0, 0.0003], // closes onto the first point
        ];
        let small_hole = vec![[5.0, 5.0], [5.0, 5.01], [5.01, 5.01]];
        let real_hole = vec![[2.0, 2.0], [2.0, 4.0], [4.0, 4.0], [4.0, 2.0]];
        let sliver = vec![[20.0, 0.0], [20.02, 0.0], [20.02, 0.02]];
        let cleaned = clean(
            vec![vec![big, small_hole, real_hole.clone()], vec![sliver]],
            1e-3,
            1e-3,
        );
        assert_eq!(cleaned.len(), 1);
        assert_eq!(cleaned[0].len(), 2);
        assert_eq!(cleaned[0][0].len(), 4);
        assert_eq!(cleaned[0][1], real_hole);
    }

    #[test]
    fn hatch_covers_the_area() {
        let shapes = from_loops(&[square(0.0, 0.0, 20.0), square(5.0, 5.0, 10.0)]);
        for direction in [
            HatchDirection::Diagonal45,
            HatchDirection::Diagonal135,
            HatchDirection::Horizontal,
            HatchDirection::Vertical,
        ] {
            let segments = hatch(&shapes, 0.5, direction, 0.0);
            let length: f64 = segments.iter().map(|s| distance(s[0], s[1])).sum();
            // Total length times spacing approximates the area.
            assert!((length * 0.5 - 300.0).abs() < 3.0, "{length}");
            for s in &segments {
                for p in s {
                    assert!(
                        p[0] > -1e-9 && p[0] < 20.0 + 1e-9 && p[1] > -1e-9 && p[1] < 20.0 + 1e-9
                    );
                }
            }
        }
    }
}
