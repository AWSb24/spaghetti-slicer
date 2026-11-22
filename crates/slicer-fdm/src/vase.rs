//! Vase mode with several islands and with holes.
//!
//! Above the bottom layers vase mode prints the outline of every island of
//! a layer as one loop that rises from the height of the layer below to
//! the height of its own layer ([`crate::Layer::rises`]). With several
//! islands the G-code writer prints them one after the other, with a
//! travel between them; here every loop gets its start near the start of
//! the loop it continues on the layer below, so that each island grows as
//! its own spiral, and the islands are put in the order that starts the
//! layer where the layer below ended.
//!
//! Holes of the outline are left out on the spiral layers: the walls round
//! them are dropped ([`drop_holes`]), so a model that already has a wall
//! of its own prints with its outside only. The slices keep their holes,
//! so nothing above a hole counts as a bottom surface or an overhang.

use slicer_core::polygon::{self, distance, signed_area};
use slicer_core::{Contour, Point, Shapes};

use crate::{Path, PathKind};

/// Whether `paths` can rise as spiral loops: one or more paths, all of them
/// closed outer walls.
pub(crate) fn loops_only(paths: &[Path]) -> bool {
    !paths.is_empty()
        && paths
            .iter()
            .all(|p| p.closed && p.kind == PathKind::OuterWall)
}

/// Drops the outer walls round holes from the paths of a spiral layer and
/// says whether there were any. Outer walls run counter-clockwise round
/// the outline of an island and clockwise round a hole; lines along narrow
/// parts (which have a width of their own) run either way and stay. An
/// island inside a hole stays an island of its own.
// ponytail: a hole whose wall is narrower than two line widths has a line
// along its middle instead of two walls, which stays.
pub(crate) fn drop_holes(paths: &mut Vec<Path>) -> bool {
    let before = paths.len();
    paths.retain(|p| {
        !(p.closed
            && p.kind == PathKind::OuterWall
            && p.width.is_none()
            && signed_area(&p.points) < 0.0)
    });
    paths.len() < before
}

/// Where the last of `paths` ends: a closed one where it starts.
pub(crate) fn end_of(paths: &[Path]) -> Option<Point> {
    let last = paths.last()?;
    if last.closed {
        last.points.first().copied()
    } else {
        last.points.last().copied()
    }
}

/// Gives every loop of a spiral layer with several of them its start and
/// the layer its order, once its paths are known; `below` holds the
/// closed outer walls of the layer below, `entry` is where the layer below
/// ended. Returns where this layer ends, or `None` if it was left alone: a
/// layer with a single loop keeps the seam of the settings, and one with
/// anything but closed outer walls is printed flat anyway.
///
/// Every loop is matched with the loop of the layer below that it overlaps
/// most ([`below_of`]) and starts at its point nearest to where that one
/// started, so the spiral of every island goes on where it left off. A
/// loop with nothing below keeps the start of the seam setting. The loops
/// then follow each other from `entry`, always the one that starts nearest
/// to where the last one ended (each ends where it starts).
// ponytail: a single loop keeps the seam of the settings, so a model that
// merges again from several islands into one may start the merged loop
// away from where the islands ended.
pub(crate) fn follow(paths: &mut Vec<Path>, below: &[Contour], entry: Point) -> Option<Point> {
    if paths.len() < 2 || !loops_only(paths) {
        return None;
    }
    for path in paths.iter_mut() {
        if let Some(under) = below_of(&path.points, below)
            && let Some(&start) = under.first()
        {
            let nearest = nearest_point(&path.points, start);
            path.points.rotate_left(nearest);
        }
    }
    // Greedy order from where the layer below ended; ties go to the loop
    // that came first.
    let mut left = std::mem::take(paths);
    let mut at = entry;
    while !left.is_empty() {
        let starts: Vec<Point> = left.iter().map(|p| p.points[0]).collect();
        let next = nearest_point(&starts, at);
        let path = left.remove(next);
        at = path.points[0];
        paths.push(path);
    }
    Some(at)
}

/// The closed outer walls of `paths`, for [`follow`] on the next layer.
pub(crate) fn outer_loops(paths: &[Path]) -> Vec<Contour> {
    paths
        .iter()
        .filter(|p| p.closed && p.kind == PathKind::OuterWall && !p.points.is_empty())
        .map(|p| p.points.clone())
        .collect()
}

/// Index of the point of `points` nearest to `to`, the first of equally
/// near ones; 0 for no points.
fn nearest_point(points: &[Point], to: Point) -> usize {
    let mut best = (0, f64::INFINITY);
    for (i, &p) in points.iter().enumerate() {
        let d = distance(p, to);
        if d < best.1 {
            best = (i, d);
        }
    }
    best.0
}

/// The loop of `below` that `outline` continues: of those whose bounding
/// boxes overlap its own the one it overlaps most (the only one without
/// cutting them), otherwise, and among loops it does not overlap at all,
/// the one whose centroid is nearest; the first of equal ones. `None` if
/// there is no loop below.
// ponytail: the overlap is only measured when the bounding boxes of
// several loops below overlap that of the loop, so islands far apart cost
// no polygon operations.
fn below_of<'a>(outline: &[Point], below: &'a [Contour]) -> Option<&'a Contour> {
    let own = bounds(outline);
    let touching: Vec<&Contour> = below
        .iter()
        .filter(|under| boxes_overlap(own, bounds(under)))
        .collect();
    if let [only] = touching.as_slice() {
        return Some(only);
    }
    if touching.len() > 1 {
        let shape: Shapes = vec![vec![outline.to_vec()]];
        let mut best: Option<(&Contour, f64)> = None;
        for under in &touching {
            let common = polygon::intersection(&shape, &vec![vec![under.to_vec()]]);
            let area = polygon::area(&common);
            if area > best.map_or(0.0, |(_, a)| a) {
                best = Some((under, area));
            }
        }
        if let Some((under, _)) = best {
            return Some(under);
        }
    }
    let centre = centroid(outline);
    let candidates: Vec<&Contour> = if touching.is_empty() {
        below.iter().collect()
    } else {
        touching
    };
    let mut best: Option<(&Contour, f64)> = None;
    for under in candidates {
        let d = distance(centroid(under), centre);
        if best.is_none_or(|(_, b)| d < b) {
            best = Some((under, d));
        }
    }
    best.map(|(under, _)| under)
}

/// Smallest and largest X and Y of `points`.
fn bounds(points: &[Point]) -> [Point; 2] {
    points.iter().fold(
        [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]],
        |[lo, hi], p| {
            [
                [lo[0].min(p[0]), lo[1].min(p[1])],
                [hi[0].max(p[0]), hi[1].max(p[1])],
            ]
        },
    )
}

/// Whether two bounding boxes from [`bounds`] overlap or touch.
fn boxes_overlap(a: [Point; 2], b: [Point; 2]) -> bool {
    a[0][0] <= b[1][0] && b[0][0] <= a[1][0] && a[0][1] <= b[1][1] && b[0][1] <= a[1][1]
}

/// The centroid of the area a closed loop encloses; the mean of its points
/// if it encloses none.
fn centroid(points: &[Point]) -> Point {
    let n = points.len();
    let (mut area, mut x, mut y) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let (p, q) = (points[i], points[(i + 1) % n]);
        let cross = p[0] * q[1] - q[0] * p[1];
        area += cross;
        x += (p[0] + q[0]) * cross;
        y += (p[1] + q[1]) * cross;
    }
    if area.abs() > 1e-12 {
        return [x / (3.0 * area), y / (3.0 * area)];
    }
    let count = n.max(1) as f64;
    let (sx, sy) = points
        .iter()
        .fold((0.0, 0.0), |(sx, sy), p| (sx + p[0], sy + p[1]));
    [sx / count, sy / count]
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::NO_OBJECT;

    fn square(x: f64, y: f64, size: f64) -> Contour {
        vec![[x, y], [x + size, y], [x + size, y + size], [x, y + size]]
    }

    fn wall(points: Contour) -> Path {
        Path {
            kind: PathKind::OuterWall,
            points,
            closed: true,
            travel_inside: false,
            extruder: 0,
            object: NO_OBJECT,
            width: None,
            travel: Vec::new(),
        }
    }

    #[test]
    fn centroid_of_a_square_is_its_middle() {
        let c = centroid(&square(2.0, 4.0, 2.0));
        assert!(distance(c, [3.0, 5.0]) < 1e-12, "{c:?}");
    }

    #[test]
    fn loops_continue_the_loop_they_overlap_most() {
        // Two islands below; the wide one above the left overlaps it most,
        // although the centroid of the right one is nearer.
        let left = square(0.0, 0.0, 10.0);
        let right = square(10.5, 0.0, 2.0);
        let wide = vec![[1.0, 1.0], [11.0, 1.0], [11.0, 9.0], [1.0, 9.0]];
        let below = [left.clone(), right.clone()];
        assert_eq!(below_of(&wide, &below), Some(&left));
        // Far from both: the nearest centroid.
        assert_eq!(below_of(&square(30.0, 0.0, 2.0), &below), Some(&right));
        assert_eq!(below_of(&wide, &[]), None);
    }

    #[test]
    fn several_loops_start_near_their_starts_below() {
        let a = square(0.0, 0.0, 10.0);
        let b = square(20.0, 0.0, 10.0);
        // Below, `a` started at its top right corner and `b` at its bottom
        // left one, and the layer below ended at `b`.
        let below = [vec![a[2], a[3], a[0], a[1]], vec![b[0], b[1], b[2], b[3]]];
        let mut paths = vec![wall(a.clone()), wall(b.clone())];
        let end = follow(&mut paths, &below, b[0]);
        assert_eq!(paths[0].points[0], b[0], "starts with the loop it ended on");
        assert_eq!(paths[1].points[0], a[2]);
        assert_eq!(end, Some(a[2]));
        // One loop keeps its seam, as does a layer with more than walls.
        let mut one = vec![wall(a.clone())];
        assert_eq!(follow(&mut one, &below, b[0]), None);
        assert_eq!(one[0].points[0], a[0]);
        let end = b[0];
        let mut open = vec![wall(a), wall(b)];
        open[1].closed = false;
        assert_eq!(follow(&mut open, &below, end), None);
    }

    #[test]
    fn walls_round_holes_are_dropped() {
        let mut hole = square(2.0, 2.0, 6.0);
        hole.reverse();
        let mut narrow = hole.clone();
        let mut paths = vec![wall(square(0.0, 0.0, 10.0)), wall(hole), {
            narrow.truncate(3);
            let mut line = wall(narrow);
            line.width = Some(0.3);
            line
        }];
        assert!(drop_holes(&mut paths));
        assert_eq!(paths.len(), 2, "the outline and the narrow line stay");
        assert!(paths[1].width.is_some());
        assert_eq!(end_of(&paths), Some(paths[1].points[0]));
        assert!(!drop_holes(&mut paths));
    }
}
