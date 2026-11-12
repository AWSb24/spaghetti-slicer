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
    snap(
        shapes
            .outline_fixed_scale(&style, SCALE)
            .unwrap_or_else(|_| shapes.outline(&style)),
    )
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
        }
    };
    let backward = |a: f64, b: f64| -> Point {
        let (u, v) = match direction {
            HatchDirection::Diagonal45 => (a, b),
            HatchDirection::Diagonal135 => (b, a),
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
        for direction in [HatchDirection::Diagonal45, HatchDirection::Diagonal135] {
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
