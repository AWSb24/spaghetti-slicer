//! Cutting a mesh in two with a horizontal plane.

use std::collections::HashMap;

use crate::mesh::Mesh;

/// Vertices closer to the plane than this (mm) count as lying on it.
const ON_PLANE: f64 = 1e-7;

impl Mesh {
    /// Cuts the mesh at height `z` into the part below and the part above.
    /// Triangles crossing the plane are split; the cut face of each part is
    /// covered, so a closed solid gives two closed solids. A part with
    /// nothing in it has no triangles. Extruders carry over; the covers
    /// print with extruder 0.
    pub fn cut(&self, z: f64) -> [Mesh; 2] {
        let side = |v: [f64; 3]| {
            let d = v[2] - z;
            if d.abs() < ON_PLANE {
                0
            } else if d < 0.0 {
                -1
            } else {
                1
            }
        };
        // Where an edge crosses the plane, computed from the lower-numbered
        // end so that both triangles of the edge get the same point.
        let mut crossings: HashMap<(u32, u32), [f64; 3]> = HashMap::new();
        let mut crossing = |i: u32, j: u32| -> [f64; 3] {
            let key = (i.min(j), i.max(j));
            *crossings.entry(key).or_insert_with(|| {
                let (a, b) = (self.vertices[key.0 as usize], self.vertices[key.1 as usize]);
                let t = (z - a[2]) / (b[2] - a[2]);
                [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, z]
            })
        };
        let mut parts: [Vec<([[f64; 3]; 3], u32)>; 2] = [Vec::new(), Vec::new()];
        // Lines where the surface meets the plane; they outline the cut face.
        // Each runs the way the cover of the lower part runs along it:
        // against the wall below, so that outlines of material run
        // counter-clockwise and holes clockwise.
        let mut lines: Vec<[[f64; 2]; 2]> = Vec::new();
        for (index, t) in self.triangles.iter().enumerate() {
            let Some(p) = t
                .iter()
                .map(|&i| self.vertices.get(i as usize).copied())
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            let p = [p[0], p[1], p[2]];
            let s = p.map(side);
            let extruder = self.extruder(index);
            let below = s.iter().all(|&x| x <= 0);
            let above = s.iter().all(|&x| x >= 0);
            if below && above {
                // Lies in the plane: part of the cut face, which is rebuilt.
                continue;
            }
            if below || above {
                parts[usize::from(above)].push((p, extruder));
                // An edge in the plane outlines the cut face once: count it
                // with the triangle below.
                if below {
                    for k in 0..3 {
                        if s[k] == 0 && s[(k + 1) % 3] == 0 {
                            let (a, b) = (p[k], p[(k + 1) % 3]);
                            lines.push([[b[0], b[1]], [a[0], a[1]]]);
                        }
                    }
                }
                continue;
            }
            // Crosses the plane: walk round it, splitting where it crosses.
            let (mut lower, mut upper, mut on) = (Vec::new(), Vec::new(), Vec::new());
            for k in 0..3 {
                let (a, sa, sb) = (p[k], s[k], s[(k + 1) % 3]);
                match sa {
                    -1 => lower.push(a),
                    1 => upper.push(a),
                    _ => {
                        lower.push(a);
                        upper.push(a);
                        on.push(a);
                    }
                }
                if sa * sb == -1 {
                    let x = crossing(t[k], t[(k + 1) % 3]);
                    lower.push(x);
                    upper.push(x);
                    on.push(x);
                }
            }
            if let [a, b] = on[..] {
                let at = |q: [f64; 3]| lower.iter().position(|&v| v == q);
                let wall_runs_a_to_b = matches!(
                    (at(a), at(b)),
                    (Some(i), Some(j)) if (i + 1) % lower.len() == j
                );
                lines.push(if wall_runs_a_to_b {
                    [[b[0], b[1]], [a[0], a[1]]]
                } else {
                    [[a[0], a[1]], [b[0], b[1]]]
                });
            }
            for (polygon, part) in [(lower, 0), (upper, 1)] {
                for k in 1..polygon.len().saturating_sub(1) {
                    parts[part].push(([polygon[0], polygon[k], polygon[k + 1]], extruder));
                }
            }
        }
        let cover = cover(&lines);
        let [lower, upper] = parts;
        let mut lower_soup = lower;
        let mut upper_soup = upper;
        for [a, b, c] in cover {
            let at = |p: [f64; 2]| [p[0], p[1], z];
            lower_soup.push(([at(a), at(b), at(c)], 0));
            upper_soup.push(([at(a), at(c), at(b)], 0));
        }
        [lower_soup, upper_soup].map(|soup| {
            // Without the triangles the soup would drop, so that the
            // extruders stay in step.
            let soup: Vec<_> = soup
                .into_iter()
                .filter(|(t, _)| {
                    let [a, b, c] = t.map(|v| v.map(|c| c as f32));
                    a != b && b != c && a != c
                })
                .collect();
            let triangles: Vec<[[f32; 3]; 3]> = soup
                .iter()
                .map(|(t, _)| t.map(|v| v.map(|c| c as f32)))
                .collect();
            let mut mesh = Mesh::from_triangle_soup(&triangles);
            if soup.iter().any(|&(_, e)| e != 0) {
                mesh.extruders = soup.iter().map(|&(_, e)| e).collect();
            }
            mesh
        })
    }
}

/// A point in the cut plane.
type Point = [f64; 2];

/// Most corners the exact cover handles; larger cut faces get the strip
/// cover, whose ear clipping would take too long.
const MAX_CORNERS: usize = 20_000;

/// Triangles covering the area inside `lines` (closed outlines in any
/// order, material counter-clockwise and holes clockwise), counter-clockwise
/// seen from above. The corners are the ends of the lines, so the cover meets the
/// side walls edge to edge. Falls back to [`cover_strips`] for outlines
/// that do not close or cannot be triangulated.
fn cover(lines: &[[Point; 2]]) -> Vec<[Point; 3]> {
    exact_cover(lines).unwrap_or_else(|| cover_strips(lines))
}

fn exact_cover(lines: &[[Point; 2]]) -> Option<Vec<[Point; 3]>> {
    if lines.len() > MAX_CORNERS {
        return None;
    }
    let loops = chain(lines)?;
    let areas: Vec<f64> = loops.iter().map(|l| signed_area(l)).collect();
    let mut triangles = Vec::new();
    for (i, outer) in loops.iter().enumerate() {
        if areas[i] <= 0.0 {
            continue;
        }
        // The holes whose smallest surrounding outline this is. Bodies
        // that overlap each keep their own outlines and covers.
        let mut holes: Vec<&Vec<Point>> = loops
            .iter()
            .enumerate()
            .filter(|&(j, hole)| {
                areas[j] < 0.0
                    && (0..loops.len())
                        .filter(|&k| areas[k] > 0.0 && inside(&loops[k], hole[0]))
                        .min_by(|&a, &b| areas[a].total_cmp(&areas[b]))
                        == Some(i)
            })
            .map(|(_, hole)| hole)
            .collect();
        holes.sort_by(|a, b| max_x(b).total_cmp(&max_x(a)));
        let mut polygon = outer.clone();
        for h in 0..holes.len() {
            let rest: Vec<Vec<Point>> = holes[h + 1..].iter().map(|&l| l.clone()).collect();
            polygon = bridge(&polygon, holes[h], &rest)?;
        }
        triangles.extend(ear_clip(&polygon)?);
    }
    Some(triangles)
}

/// The lines joined end to start into closed outlines, or `None` if one
/// does not close.
fn chain(lines: &[[Point; 2]]) -> Option<Vec<Vec<Point>>> {
    let key = |p: Point| ((p[0] + 0.0).to_bits(), (p[1] + 0.0).to_bits());
    let mut starts: HashMap<(u64, u64), Vec<usize>> = HashMap::new();
    for (i, [a, _]) in lines.iter().enumerate() {
        starts.entry(key(*a)).or_default().push(i);
    }
    let mut used = vec![false; lines.len()];
    let mut loops = Vec::new();
    for start in 0..lines.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let [first, mut at] = lines[start];
        let mut outline = vec![first];
        while key(at) != key(first) {
            outline.push(at);
            let next = starts.get(&key(at))?.iter().copied().find(|&j| !used[j])?;
            used[next] = true;
            at = lines[next][1];
        }
        if outline.len() >= 3 {
            loops.push(outline);
        }
    }
    Some(loops)
}

fn signed_area(polygon: &[Point]) -> f64 {
    polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1))
        .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
        .sum::<f64>()
        / 2.0
}

fn max_x(polygon: &[Point]) -> f64 {
    polygon
        .iter()
        .map(|p| p[0])
        .fold(f64::NEG_INFINITY, f64::max)
}

/// Whether `p` lies inside `polygon` (even-odd rule).
fn inside(polygon: &[Point], p: Point) -> bool {
    let mut inside = false;
    for (a, b) in polygon.iter().zip(polygon.iter().cycle().skip(1)) {
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < a[0] + (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1])
        {
            inside = !inside;
        }
    }
    inside
}

/// Positive if `c` lies left of the line from `a` to `b`.
fn turn(a: Point, b: Point, c: Point) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// Whether the segments `p`–`q` and `r`–`s` touch anywhere other than at a
/// shared end.
fn segments_meet(p: Point, q: Point, r: Point, s: Point) -> bool {
    if p == r || p == s || q == r || q == s {
        // Sharing an end, they meet elsewhere only when overlapping.
        let (shared, a, b) = if p == r || p == s {
            (p, q, if p == r { s } else { r })
        } else {
            (q, p, if q == r { s } else { r })
        };
        return turn(shared, a, b) == 0.0
            && (a[0] - shared[0]) * (b[0] - shared[0]) + (a[1] - shared[1]) * (b[1] - shared[1])
                > 0.0;
    }
    let (d1, d2) = (turn(r, s, p), turn(r, s, q));
    let (d3, d4) = (turn(p, q, r), turn(p, q, s));
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }
    let on = |a: Point, b: Point, c: Point| {
        turn(a, b, c) == 0.0
            && c[0] >= a[0].min(b[0])
            && c[0] <= a[0].max(b[0])
            && c[1] >= a[1].min(b[1])
            && c[1] <= a[1].max(b[1])
    };
    on(r, s, p) || on(r, s, q) || on(p, q, r) || on(p, q, s)
}

/// `polygon` (counter-clockwise) with the clockwise `hole` joined in along
/// a cut from the hole's rightmost corner to the nearest corner of the
/// polygon it can see; `others` are the holes still to come, which the cut
/// must not cross either.
// ponytail: tries the corners by distance and checks each against every
// edge, O(n²) per hole.
fn bridge(polygon: &[Point], hole: &[Point], others: &[Vec<Point>]) -> Option<Vec<Point>> {
    let m = (0..hole.len()).max_by(|&a, &b| hole[a][0].total_cmp(&hole[b][0]))?;
    let from = hole[m];
    let mut order: Vec<usize> = (0..polygon.len()).collect();
    let distance = |p: Point| (p[0] - from[0]).hypot(p[1] - from[1]);
    order.sort_by(|&a, &b| distance(polygon[a]).total_cmp(&distance(polygon[b])));
    let edges = |l: &[Point]| -> Vec<(Point, Point)> {
        l.iter()
            .copied()
            .zip(l.iter().copied().cycle().skip(1))
            .collect()
    };
    let mut all = edges(polygon);
    all.extend(edges(hole));
    for other in others {
        all.extend(edges(other));
    }
    let to = order.into_iter().find(|&i| {
        let target = polygon[i];
        all.iter()
            .all(|&(a, b)| !segments_meet(from, target, a, b) || a == target && b == target)
    })?;
    let mut joined: Vec<Point> = polygon[..=to].to_vec();
    joined.extend(hole[m..].iter().copied());
    joined.extend(hole[..=m].iter().copied());
    joined.push(polygon[to]);
    joined.extend(polygon[to + 1..].iter().copied());
    Some(joined)
}

/// Triangulates a counter-clockwise polygon, which may run along both sides
/// of a cut, by cutting off ears. Corners in a straight line are dropped
/// when nothing else is left. `None` if it gets stuck.
// ponytail: checks every corner for every ear candidate, O(n²) to O(n³);
// fine below `MAX_CORNERS`.
fn ear_clip(polygon: &[Point]) -> Option<Vec<[Point; 3]>> {
    let mut left: Vec<Point> = polygon.to_vec();
    let mut triangles = Vec::with_capacity(left.len());
    let mut start = 0;
    while left.len() > 3 {
        let n = left.len();
        let corners = |k: usize| (left[(k + n - 1) % n], left[k], left[(k + 1) % n]);
        let ear = (0..n).map(|i| (start + i) % n).find(|&k| {
            let (a, b, c) = corners(k);
            turn(a, b, c) > 0.0
                && left.iter().all(|&p| {
                    p == a
                        || p == b
                        || p == c
                        || turn(a, b, p) < 0.0
                        || turn(b, c, p) < 0.0
                        || turn(c, a, p) < 0.0
                })
        });
        match ear {
            Some(k) => {
                let (a, b, c) = corners(k);
                triangles.push([a, b, c]);
                left.remove(k);
                start = k.saturating_sub(1);
            }
            None => {
                // A corner on a straight line between its neighbours.
                let k = (0..n).find(|&k| {
                    let (a, b, c) = corners(k);
                    turn(a, b, c) == 0.0
                })?;
                left.remove(k);
            }
        }
    }
    if turn(left[0], left[1], left[2]) > 0.0 {
        triangles.push([left[0], left[1], left[2]]);
    }
    Some(triangles)
}

/// Triangles covering the area inside `lines`, counter-clockwise seen from
/// above, by cutting it into vertical strips at every end point; in a
/// strip the lines crossing it, sorted from front to back, bound the inside
/// in pairs (even-odd rule), and each pair gives a trapezoid.
// ponytail: the corners on strip borders are not corners of the side
// walls, so the part is not closed edge to edge; only used when the exact
// cover fails.
fn cover_strips(lines: &[[Point; 2]]) -> Vec<[Point; 3]> {
    let mut xs: Vec<f64> = lines.iter().flatten().map(|p| p[0]).collect();
    xs.sort_by(f64::total_cmp);
    xs.dedup();
    let mut triangles = Vec::new();
    for strip in xs.windows(2) {
        let (x0, x1) = (strip[0], strip[1]);
        if x1 - x0 < 1e-9 {
            continue;
        }
        let xm = (x0 + x1) / 2.0;
        let mut crossing: Vec<(f64, f64, f64)> = lines
            .iter()
            .filter(|[a, b]| a[0].min(b[0]) <= x0 && a[0].max(b[0]) >= x1)
            .map(|[a, b]| {
                let y = |x: f64| a[1] + (b[1] - a[1]) * (x - a[0]) / (b[0] - a[0]);
                (y(xm), y(x0), y(x1))
            })
            .collect();
        crossing.sort_by(|a, b| a.0.total_cmp(&b.0));
        for [front, back] in crossing.as_chunks::<2>().0 {
            let (front, back) = (*front, *back);
            let quad = [[x0, front.1], [x1, front.2], [x1, back.2], [x0, back.1]];
            for t in [[quad[0], quad[1], quad[2]], [quad[0], quad[2], quad[3]]] {
                let area = (t[1][0] - t[0][0]) * (t[2][1] - t[0][1])
                    - (t[1][1] - t[0][1]) * (t[2][0] - t[0][0]);
                if area > 1e-12 {
                    triangles.push(t);
                }
            }
        }
    }
    triangles
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn a_cube_cut_in_two_closed_parts() {
        let cube = Mesh::cuboid(10.0, 20.0, 10.0);
        let [lower, upper] = cube.cut(4.0);
        assert!(
            close(lower.signed_volume(), 800.0),
            "{}",
            lower.signed_volume()
        );
        assert!(
            close(upper.signed_volume(), 1200.0),
            "{}",
            upper.signed_volume()
        );
        // Closed edge to edge, all facing out.
        for part in [&lower, &upper] {
            assert_eq!(part.open_edge_count(), 0);
            assert_eq!(part.clone().orient(), 0);
        }
        let (lo, hi) = lower.bounds().unwrap_or_default();
        assert!(close(lo[2], 0.0) && close(hi[2], 4.0));
        let (lo, hi) = upper.bounds().unwrap_or_default();
        assert!(close(lo[2], 4.0) && close(hi[2], 10.0));
        // Above or below everything, one part is empty.
        let [lower, upper] = cube.cut(12.0);
        assert_eq!(lower.triangles.len(), cube.triangles.len());
        assert!(upper.triangles.is_empty());
        // Through the bottom face: nothing below.
        let [lower, upper] = cube.cut(0.0);
        assert!(lower.triangles.is_empty());
        assert!(close(upper.signed_volume(), 2000.0));
    }

    #[test]
    fn overlapping_bodies_get_a_cover_each() {
        let mut two = Mesh::cuboid(20.0, 10.0, 10.0);
        let mut other = Mesh::cuboid(10.0, 20.0, 10.0);
        other.translate([5.0, -5.0, 0.0]);
        two.merge(&other);
        for part in two.cut(6.0) {
            assert_eq!(part.open_edge_count(), 0);
            assert_eq!(part.clone().orient(), 0);
        }
    }

    #[test]
    fn a_tube_keeps_its_hole() {
        // A 20 mm cube with a 10 mm square hole: cube minus the inner box
        // turned inside out.
        let mut tube = Mesh::cuboid(20.0, 20.0, 10.0);
        let mut hole = Mesh::cuboid(10.0, 10.0, 10.0);
        hole.translate([5.0, 5.0, 0.0]);
        hole.flip();
        tube.merge(&hole);
        let [lower, upper] = tube.cut(3.0);
        assert!(
            close(lower.signed_volume(), 300.0 * 3.0),
            "{}",
            lower.signed_volume()
        );
        assert!(
            close(upper.signed_volume(), 300.0 * 7.0),
            "{}",
            upper.signed_volume()
        );
        // Closed edge to edge, all facing out.
        for part in [&lower, &upper] {
            assert_eq!(part.open_edge_count(), 0);
            assert_eq!(part.clone().orient(), 0);
        }
        // The cover leaves the hole open.
        let covers = cover(&[
            [[0.0, 0.0], [4.0, 0.0]],
            [[4.0, 0.0], [4.0, 4.0]],
            [[4.0, 4.0], [0.0, 4.0]],
            [[0.0, 4.0], [0.0, 0.0]],
            // The hole runs clockwise.
            [[1.0, 1.0], [1.0, 3.0]],
            [[1.0, 3.0], [3.0, 3.0]],
            [[3.0, 3.0], [3.0, 1.0]],
            [[3.0, 1.0], [1.0, 1.0]],
        ]);
        let area: f64 = covers
            .iter()
            .map(|t| {
                ((t[1][0] - t[0][0]) * (t[2][1] - t[0][1])
                    - (t[1][1] - t[0][1]) * (t[2][0] - t[0][0]))
                    / 2.0
            })
            .sum();
        assert!(close(area, 12.0), "{area}");
    }
}
