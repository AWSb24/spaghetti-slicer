//! Travel moves that go round holes and notches of a layer instead of
//! crossing them over open air (`Settings::avoid_crossing_walls`).
//!
//! The way is found by following the straight line and, wherever it leaves
//! the region, the edge of the region from where it leaves to where it
//! comes back in, whichever way round is shorter.

use slicer_core::Point;
use slicer_core::polygon::{self, Contour, Shapes, distance};

/// A route longer than this many times the straight distance (plus
/// [`DETOUR_ALLOWANCE`]) is not worth it; the travel goes straight and
/// retracts.
const DETOUR_FACTOR: f64 = 3.0;

/// Millimetres a route may be longer than [`DETOUR_FACTOR`] allows, so
/// short travels round a corner are not dropped.
const DETOUR_ALLOWANCE: f64 = 10.0;

/// The corners of a travel from `from` to `to` that stays inside `region`:
/// empty if the straight line does, `None` if there is no such travel
/// (the points lie in different islands or outside) or it would be a long
/// detour.
// ponytail: a line that leaves through one edge and comes back through
// another one (an island inside a hole) gets no route; such travels go
// straight.
pub(crate) fn route(region: &Shapes, from: Point, to: Point) -> Option<Vec<Point>> {
    if polygon::segment_inside(region, from, to) {
        return Some(Vec::new());
    }
    let shape = region
        .iter()
        .find(|shape| inside(shape, from) && inside(shape, to))?;
    // Where the line crosses the edges: along the line, which contour,
    // and where on it (edge index plus the fraction along that edge).
    let mut hits: Vec<(f64, usize, f64)> = Vec::new();
    for (c, contour) in shape.iter().enumerate() {
        let n = contour.len();
        for i in 0..n {
            if let Some((t, u)) = crossing(from, to, contour[i], contour[(i + 1) % n]) {
                hits.push((t, c, i as f64 + u));
            }
        }
    }
    hits.sort_by(|a, b| a.0.total_cmp(&b.0));
    if !hits.len().is_multiple_of(2) {
        return None;
    }
    let at = |t: f64| {
        [
            from[0] + (to[0] - from[0]) * t,
            from[1] + (to[1] - from[1]) * t,
        ]
    };
    let mut corners = Vec::new();
    for [(t0, c0, s0), (t1, c1, s1)] in hits.as_chunks::<2>().0.iter().copied() {
        if c0 != c1 {
            return None;
        }
        let (out, back) = (at(t0), at(t1));
        let contour = &shape[c0];
        let forward = walk(contour, s0, s1, true);
        let backward = walk(contour, s0, s1, false);
        let length = |way: &[Point]| {
            let mut points = vec![out];
            points.extend_from_slice(way);
            points.push(back);
            points.windows(2).map(|w| distance(w[0], w[1])).sum::<f64>()
        };
        let way = if length(&forward) <= length(&backward) {
            forward
        } else {
            backward
        };
        corners.push(out);
        corners.extend(way);
        corners.push(back);
    }
    let mut all = vec![from];
    all.extend_from_slice(&corners);
    all.push(to);
    let length: f64 = all.windows(2).map(|w| distance(w[0], w[1])).sum();
    (length <= DETOUR_FACTOR * distance(from, to) + DETOUR_ALLOWANCE).then_some(corners)
}

/// The corners of `contour` passed going from position `start` to
/// position `end` (edge index plus fraction along the edge), forwards or
/// backwards.
fn walk(contour: &Contour, start: f64, end: f64, forwards: bool) -> Vec<Point> {
    let n = contour.len();
    let (first, last) = (start.floor() as usize % n, end.floor() as usize % n);
    let mut corners = Vec::new();
    let mut edge = first;
    for step in 0..=n {
        let arrived =
            edge == last && (step > 0 || if forwards { end >= start } else { end <= start });
        if arrived {
            break;
        }
        if forwards {
            edge = (edge + 1) % n;
            corners.push(contour[edge]);
        } else {
            corners.push(contour[edge]);
            edge = (edge + n - 1) % n;
        }
    }
    corners
}

/// Where the segments `p`–`q` and `a`–`b` cross: the fraction along each,
/// if they cross properly.
fn crossing(p: Point, q: Point, a: Point, b: Point) -> Option<(f64, f64)> {
    let cross = |u: Point, v: Point| u[0] * v[1] - u[1] * v[0];
    let (r, s) = ([q[0] - p[0], q[1] - p[1]], [b[0] - a[0], b[1] - a[1]]);
    let d = cross(r, s);
    if d.abs() < 1e-12 {
        return None;
    }
    let ap = [a[0] - p[0], a[1] - p[1]];
    let (t, u) = (cross(ap, s) / d, cross(ap, r) / d);
    (t > 0.0 && t < 1.0 && (0.0..1.0).contains(&u)).then_some((t, u))
}

/// Whether `point` lies inside the shape (even-odd over its contours).
fn inside(shape: &[Contour], point: Point) -> bool {
    let mut inside = false;
    for contour in shape {
        for (i, &p) in contour.iter().enumerate() {
            let q = contour[(i + 1) % contour.len()];
            if (p[1] > point[1]) != (q[1] > point[1])
                && point[0] < p[0] + (point[1] - p[1]) / (q[1] - p[1]) * (q[0] - p[0])
            {
                inside = !inside;
            }
        }
    }
    inside
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn square(x0: f64, y0: f64, x1: f64, y1: f64) -> Contour {
        vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
    }

    fn length(from: Point, corners: &[Point], to: Point) -> f64 {
        let mut all = vec![from];
        all.extend_from_slice(corners);
        all.push(to);
        all.windows(2).map(|w| distance(w[0], w[1])).sum()
    }

    #[test]
    fn travels_go_round_a_hole() {
        // A 30 mm square with a 10 mm hole in the middle.
        let mut hole = square(10.0, 10.0, 20.0, 20.0);
        hole.reverse();
        let region = vec![vec![square(0.0, 0.0, 30.0, 30.0), hole]];
        // Across the hole: round its nearer side, two corners.
        let (from, to) = ([5.0, 12.0], [25.0, 12.0]);
        let corners = route(&region, from, to).unwrap();
        assert_eq!(
            corners,
            [[10.0, 12.0], [10.0, 10.0], [20.0, 10.0], [20.0, 12.0]]
        );
        assert!((length(from, &corners, to) - 24.0).abs() < 1e-9);
        // Beside the hole: straight.
        assert_eq!(route(&region, [5.0, 5.0], [25.0, 5.0]), Some(Vec::new()));
        // Into the hole or outside: no route.
        assert_eq!(route(&region, [5.0, 5.0], [15.0, 15.0]), None);
        assert_eq!(route(&region, [5.0, 5.0], [35.0, 5.0]), None);
    }

    #[test]
    fn travels_go_round_a_notch_and_skip_long_detours() {
        // A U: two arms joined at the bottom.
        let u = vec![vec![vec![
            [0.0, 0.0],
            [30.0, 0.0],
            [30.0, 60.0],
            [20.0, 60.0],
            [20.0, 5.0],
            [10.0, 5.0],
            [10.0, 60.0],
            [0.0, 60.0],
        ]]];
        let corners = route(&u, [5.0, 10.0], [25.0, 10.0]).unwrap();
        assert_eq!(
            corners,
            [[10.0, 10.0], [10.0, 5.0], [20.0, 5.0], [20.0, 10.0]]
        );
        // From the tops of the arms the way round is too long.
        assert_eq!(route(&u, [5.0, 59.0], [25.0, 59.0]), None);
    }
}
