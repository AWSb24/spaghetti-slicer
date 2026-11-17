//! Sparse infill patterns that are more than one or two hatchings.

use slicer_core::polygon::{self, HatchDirection, distance};
use slicer_core::{Point, Shapes};

use crate::PATH_RESOLUTION;

/// Samples of the gyroid per period along X and Y: steps of 15 degrees.
const GYROID_STEPS: i64 = 24;

/// Length of the gyroid's cross-section per area, times its period,
/// averaged over the height. Measured numerically on the surface
/// `sin x cos y + sin y cos z + sin z cos x = 0` (2.455; single layers lie
/// between 2.31 and 2.77).
const GYROID_LENGTH: f64 = 2.455;

/// Distance in millimetres within which a line end counts as lying on an
/// edge of the area it was cut from.
const ON_EDGE: f64 = 1e-6;

/// Joins hatch lines into zig-zags. The end of a line is connected to the
/// next line end along the boundary of `area`, the area the lines were cut
/// from, by a line along that boundary, when that stretch is at most
/// `max_walk` long and the line there has not been taken yet. Taking the
/// next end along the boundary means that no other line ends on the
/// connection, so it lies inside the area and crosses no line. Lines are
/// taken in the given order.
///
/// `fill` is the whole area infill goes into, `area` included. Where the
/// boundary of `area` runs through `fill` (along solid infill or a bridge,
/// found by looking `probe` to both sides of it), nothing is connected: a
/// connection there would overlap the ends of the lines on the other side.
pub(crate) fn connect(
    area: &Shapes,
    lines: &[[Point; 2]],
    max_walk: f64,
    fill: &Shapes,
    probe: f64,
) -> Vec<Vec<Point>> {
    // End 2i is where line i starts, end 2i + 1 where it ends.
    let ends: Vec<Point> = lines.iter().flatten().copied().collect();
    let contours: Vec<&Vec<Point>> = area.iter().flatten().collect();
    // Length along every contour from its first point to each point, and
    // back to the first one at the end.
    let along: Vec<Vec<f64>> = contours
        .iter()
        .map(|c| {
            let mut s = vec![0.0];
            for i in 0..c.len() {
                s.push(s[i] + distance(c[i], c[(i + 1) % c.len()]));
            }
            s
        })
        .collect();

    // Contour, edge and length along the contour of every end.
    let mut by_x: Vec<usize> = (0..ends.len()).collect();
    by_x.sort_by(|&a, &b| ends[a][0].total_cmp(&ends[b][0]).then(a.cmp(&b)));
    let mut place: Vec<Option<(usize, usize, f64)>> = vec![None; ends.len()];
    for (c, contour) in contours.iter().enumerate() {
        let n = contour.len();
        for e in 0..n {
            let (p, q) = (contour[e], contour[(e + 1) % n]);
            let (lo, hi) = (p[0].min(q[0]) - ON_EDGE, p[0].max(q[0]) + ON_EDGE);
            let length = along[c][e + 1] - along[c][e];
            let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
            let first = by_x.partition_point(|&k| ends[k][0] < lo);
            for &k in by_x[first..].iter().take_while(|&&k| ends[k][0] <= hi) {
                if place[k].is_some() {
                    continue;
                }
                let end = ends[k];
                let t = if length > 0.0 {
                    (((end[0] - p[0]) * dx + (end[1] - p[1]) * dy) / (length * length))
                        .clamp(0.0, 1.0)
                } else {
                    0.0
                };
                if distance(end, [p[0] + t * dx, p[1] + t * dy]) <= ON_EDGE {
                    place[k] = Some((c, e, along[c][e] + t * length));
                }
            }
        }
    }
    // The ends on every contour in order along it, and the rank of each.
    let mut on_contour: Vec<Vec<(f64, usize)>> = vec![Vec::new(); contours.len()];
    for (k, at) in place.iter().enumerate() {
        if let Some((c, _, s)) = at {
            on_contour[*c].push((*s, k));
        }
    }
    let mut rank = vec![0; ends.len()];
    for list in &mut on_contour {
        list.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        for (r, &(_, k)) in list.iter().enumerate() {
            rank[k] = r;
        }
    }

    // The next end along the contour from end `k`, forwards or backwards,
    // the length of the boundary up to it and the corners on the way.
    let walk = |k: usize, forward: bool| -> Option<(usize, f64, Vec<Point>)> {
        let (c, e, s) = place[k]?;
        let list = &on_contour[c];
        let m = list.len();
        if m < 2 {
            return None;
        }
        let r = rank[k];
        let (t, other) = list[if forward {
            (r + 1) % m
        } else {
            (r + m - 1) % m
        }];
        let total = along[c][along[c].len() - 1];
        let length = (if forward { t - s } else { s - t }).rem_euclid(total);
        if length > max_walk {
            return None;
        }
        // Corners in order, counting the distance edge by edge so that
        // it cannot wrap around.
        let (contour, n) = (contours[c], contours[c].len());
        let edge = |v: usize| along[c][v + 1] - along[c][v];
        let mut corners = Vec::new();
        let (mut v, mut d) = if forward {
            ((e + 1) % n, along[c][e + 1] - s)
        } else {
            (e, s - along[c][e])
        };
        for _ in 0..n {
            if d >= length - ON_EDGE {
                break;
            }
            if d > ON_EDGE {
                corners.push(contour[v]);
            }
            if forward {
                d += edge(v);
                v = (v + 1) % n;
            } else {
                v = (v + n - 1) % n;
                d += edge(v);
            }
        }
        let mut stretch = vec![ends[k]];
        stretch.extend(&corners);
        stretch.push(ends[other]);
        if stretch
            .windows(2)
            .any(|w| inside_fill(fill, w[0], w[1], probe))
        {
            return None;
        }
        Some((other, length, corners))
    };

    let mut used = vec![false; lines.len()];
    let mut chains = Vec::new();
    for i in 0..lines.len() {
        if used[i] {
            continue;
        }
        used[i] = true;
        let mut chain = lines[i].to_vec();
        let mut end = 2 * i + 1;
        loop {
            let next = [true, false]
                .into_iter()
                .filter_map(|forward| walk(end, forward))
                .filter(|(other, ..)| !used[other / 2])
                .min_by(|a, b| a.1.total_cmp(&b.1));
            let Some((other, _, corners)) = next else {
                break;
            };
            chain.extend(corners);
            let j = other / 2;
            used[j] = true;
            if other % 2 == 0 {
                chain.extend(lines[j]);
            } else {
                chain.extend([lines[j][1], lines[j][0]]);
            }
            end = other ^ 1;
        }
        chains.push(chain);
    }
    chains
}

/// Whether the segment from `a` to `b` has `fill` on both sides: points
/// `probe` off its middle to the left and right both lie in it.
fn inside_fill(fill: &Shapes, a: Point, b: Point, probe: f64) -> bool {
    let length = distance(a, b);
    if length <= ON_EDGE {
        return false;
    }
    let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    let off = [
        -(b[1] - a[1]) / length * probe,
        (b[0] - a[0]) / length * probe,
    ];
    crate::point_inside(fill, [mid[0] + off[0], mid[1] + off[1]])
        && crate::point_inside(fill, [mid[0] - off[0], mid[1] - off[1]])
}

/// Lines in three directions 60° apart, starting at `angle`, each set
/// `spacing` apart. All three meet in common points, so they form
/// triangles; the pattern is the same on every layer.
pub(crate) fn triangles(
    area: &Shapes,
    spacing: f64,
    angle: f64,
    min_length: f64,
) -> Vec<Vec<[Point; 2]>> {
    // Hatch lines lie half a spacing off the origin. Where two of the sets
    // cross, the middle one would lie a whole number of spacings off, so
    // its lines are shifted by half a spacing across their direction.
    let (sin, cos) = polygon::sin_cos_degrees(angle + 60.0);
    let shift = [-sin * spacing / 2.0, cos * spacing / 2.0];
    let moved: Shapes = area
        .iter()
        .map(|shape| {
            shape
                .iter()
                .map(|c| {
                    c.iter()
                        .map(|p| [p[0] - shift[0], p[1] - shift[1]])
                        .collect()
                })
                .collect()
        })
        .collect();
    let direction = |turn: f64| HatchDirection::degrees(angle + turn);
    let middle = polygon::hatch(&moved, spacing, direction(60.0), min_length)
        .into_iter()
        .map(|line| line.map(|p| [p[0] + shift[0], p[1] + shift[1]]))
        .collect();
    vec![
        polygon::hatch(area, spacing, direction(0.0), min_length),
        middle,
        polygon::hatch(area, spacing, direction(120.0), min_length),
    ]
}

/// Cross-section of a gyroid at height `z`, turned by `angle` degrees and
/// clipped to `area`, with as much line per area as straight lines
/// `spacing` apart. The polylines are thinned out like the walls; those
/// shorter than `min_length` are dropped.
// ponytail: the curve is traced on a square grid (marching squares, 24
// samples per period).
pub(crate) fn gyroid(
    area: &Shapes,
    spacing: f64,
    z: f64,
    angle: f64,
    min_length: f64,
) -> Vec<Vec<Point>> {
    // The pattern is traced in a frame turned by `angle`: the area is turned
    // back into it and the lines are turned out again.
    let (sin, cos) = polygon::sin_cos_degrees(angle);
    let turn = |p: &Point, sin: f64| [p[0] * cos - p[1] * sin, p[0] * sin + p[1] * cos];
    let area: Shapes = area
        .iter()
        .map(|shape| {
            shape
                .iter()
                .map(|c| c.iter().map(|p| turn(p, -sin)).collect())
                .collect()
        })
        .collect();
    // A segment per cell is more than the curve needs. A coarser tolerance
    // shrinks the G-code further but makes sharper corners, at which a
    // jerk-limited printer slows down: 0.05 mm halves the size again but
    // adds a seventh to the estimated time of a gyroid cube.
    trace_gyroid(&area, spacing, z)
        .iter()
        .map(|line| polygon::simplify_open(line, PATH_RESOLUTION))
        .filter(|line| line.windows(2).map(|w| distance(w[0], w[1])).sum::<f64>() >= min_length)
        .map(|line| line.iter().map(|p| turn(p, sin)).collect())
        .collect()
}

/// Cross-section of a gyroid at height `z`, clipped to `area`, as traced:
/// one segment per sampling cell.
fn trace_gyroid(area: &Shapes, spacing: f64, z: f64) -> Vec<Vec<Point>> {
    let mut points = area.iter().flatten().flatten();
    let Some(&first) = points.next() else {
        return Vec::new();
    };
    let (min, max) = points.fold((first, first), |(lo, hi), p| {
        (
            [lo[0].min(p[0]), lo[1].min(p[1])],
            [hi[0].max(p[0]), hi[1].max(p[1])],
        )
    });
    let period = GYROID_LENGTH * spacing;
    let step = period / GYROID_STEPS as f64;
    // Sines and cosines of one period, so that the samples repeat exactly.
    let wave: Vec<(f64, f64)> = (0..GYROID_STEPS)
        .map(|k| polygon::sin_cos_degrees(k as f64 * 360.0 / GYROID_STEPS as f64))
        .collect();
    let wave = |k: i64| wave[k.rem_euclid(GYROID_STEPS) as usize];
    let (sin_z, cos_z) = polygon::sin_cos_degrees(z / period * 360.0);

    // Samples on a grid anchored at the origin, so the pattern does not
    // depend on where the area lies.
    let (i0, j0) = (
        (min[0] / step).floor() as i64,
        (min[1] / step).floor() as i64,
    );
    let (i1, j1) = ((max[0] / step).ceil() as i64, (max[1] / step).ceil() as i64);
    let (nx, ny) = ((i1 - i0 + 1) as usize, (j1 - j0 + 1) as usize);
    let mut values = Vec::with_capacity(nx * ny);
    for j in j0..=j1 {
        let (sin_y, cos_y) = wave(j);
        for i in i0..=i1 {
            let (sin_x, cos_x) = wave(i);
            values.push(sin_x * cos_y + sin_y * cos_z + sin_z * cos_x);
        }
    }
    let value = |(a, b): (usize, usize)| values[b * nx + a];

    // Marching squares. Each cell gets segments with the positive side on
    // their left, so that the segments of one curve point the same way and
    // clipping joins them into polylines.
    let mut segments = Vec::new();
    for b in 0..ny - 1 {
        for a in 0..nx - 1 {
            let corners = [(a, b), (a + 1, b), (a + 1, b + 1), (a, b + 1)];
            // Crossings counter-clockwise around the cell, and whether the
            // curve leaves the positive side there.
            let mut crossings: Vec<(Point, bool)> = Vec::new();
            for e in 0..4 {
                let (p, q) = (corners[e], corners[(e + 1) % 4]);
                let (fp, fq) = (value(p), value(q));
                if (fp > 0.0) == (fq > 0.0) {
                    continue;
                }
                // Computed from the lower corner, so both cells sharing the
                // edge get the same bits.
                let (lo, hi, f_lo, f_hi) = if e < 2 {
                    (p, q, fp, fq)
                } else {
                    (q, p, fq, fp)
                };
                let t = f_lo / (f_lo - f_hi);
                let x = ((i0 + lo.0 as i64) as f64 + t * (hi.0 - lo.0) as f64) * step;
                let y = ((j0 + lo.1 as i64) as f64 + t * (hi.1 - lo.1) as f64) * step;
                crossings.push(([x, y], fp > 0.0));
            }
            let n = crossings.len();
            // Where the curve crosses itself, the centre decides which
            // corners are connected.
            let centre_positive = corners.iter().map(|&c| value(c)).sum::<f64>() > 0.0;
            for k in 0..n {
                let (from, leaves) = crossings[k];
                let to = if n == 4 && !centre_positive {
                    crossings[(k + 3) % 4].0
                } else {
                    crossings[(k + 1) % n].0
                };
                if leaves && from != to {
                    segments.push(vec![from, to]);
                }
            }
        }
    }
    polygon::clip_polylines(area, &segments)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn length(lines: &[Vec<Point>]) -> f64 {
        lines
            .iter()
            .flat_map(|line| line.windows(2))
            .map(|w| distance(w[0], w[1]))
            .sum()
    }

    fn square(lo: f64, hi: f64) -> Shapes {
        vec![vec![vec![[lo, lo], [hi, lo], [hi, hi], [lo, hi]]]]
    }

    #[test]
    fn gyroid_lines_are_thinned_out() {
        let area = square(0.0, 30.0);
        for z in [0.3, 1.1, 2.5] {
            let traced = trace_gyroid(&area, 3.0, z);
            let thinned = gyroid(&area, 3.0, z, 0.0, 0.0);
            let points = |lines: &[Vec<Point>]| lines.iter().map(Vec::len).sum::<usize>();
            // At least a quarter of the points goes.
            assert!(
                4 * points(&thinned) < 3 * points(&traced),
                "{} of {} points left at z = {z}",
                points(&thinned),
                points(&traced)
            );
            let ratio = length(&thinned) / length(&traced);
            assert!((ratio - 1.0).abs() < 0.01, "{ratio} at z = {z}");
        }
    }

    #[test]
    fn gyroid_turns_with_the_infill_angle() {
        // Turning by 90 degrees is exact: (x, y) becomes (-y, x).
        let area = square(5.0, 35.0);
        let turned: Shapes = vec![vec![area[0][0].iter().map(|p| [p[1], -p[0]]).collect()]];
        let sorted = |mut lines: Vec<Vec<Point>>| {
            lines.sort_by(|a, b| {
                a[0][0]
                    .total_cmp(&b[0][0])
                    .then(a[0][1].total_cmp(&b[0][1]))
            });
            lines
        };
        let straight = gyroid(&area, 3.0, 1.0, 0.0, 0.0);
        let at_90 = gyroid(&area, 3.0, 1.0, 90.0, 0.0);
        let expected: Vec<Vec<Point>> = gyroid(&turned, 3.0, 1.0, 0.0, 0.0)
            .into_iter()
            .map(|line| line.iter().map(|p| [-p[1], p[0]]).collect())
            .collect();
        assert_ne!(sorted(at_90.clone()), sorted(straight));
        assert_eq!(sorted(at_90), sorted(expected));
    }

    #[test]
    fn connections_follow_the_boundary_to_the_next_line() {
        // A square with a square hole; lines across it at 45 degrees.
        let mut area = square(0.0, 20.0);
        area[0].push(vec![[8.0, 8.0], [8.0, 12.0], [12.0, 12.0], [12.0, 8.0]]);
        let spacing = 2.0;
        let lines = polygon::hatch(&area, spacing, HatchDirection::Diagonal45, 0.0);
        let chains = connect(&area, &lines, 2.0 * spacing, &area, 0.1);
        assert!(
            chains.len() < lines.len() / 4,
            "{} of {}",
            chains.len(),
            lines.len()
        );
        let on_boundary = |p: Point| {
            area[0].iter().any(|c| {
                (0..c.len()).any(|i| {
                    let (a, b) = (c[i], c[(i + 1) % c.len()]);
                    let cross = (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
                    let inside = (p[0] - a[0]) * (b[0] - a[0]) + (p[1] - a[1]) * (b[1] - a[1]);
                    cross.abs() < 1e-9 && inside >= -1e-9 && inside <= distance(a, b).powi(2) + 1e-9
                })
            })
        };
        let (mut joined, mut whole) = (0, 0);
        for chain in &chains {
            for w in chain.windows(2) {
                if lines.contains(&[w[0], w[1]]) || lines.contains(&[w[1], w[0]]) {
                    whole += 1;
                    continue;
                }
                // Not a line, so a connection: on the boundary and short.
                let mid = [(w[0][0] + w[1][0]) / 2.0, (w[0][1] + w[1][1]) / 2.0];
                assert!(
                    on_boundary(w[0]) && on_boundary(mid) && on_boundary(w[1]),
                    "{w:?}"
                );
                assert!(distance(w[0], w[1]) <= 2.0 * spacing + 1e-9, "{w:?}");
                joined += 1;
            }
        }
        assert!(joined >= lines.len() - chains.len(), "{joined}");
        // Every line is printed once.
        assert_eq!(whole, lines.len());
    }

    #[test]
    fn no_connections_along_other_infill() {
        // Sparse infill on the left of a fill area whose right third is
        // solid; horizontal lines end on the wall at x = 0 and on the solid
        // infill at x = 20.
        let area = square(0.0, 20.0);
        let fill = vec![vec![vec![
            [0.0, 0.0],
            [30.0, 0.0],
            [30.0, 20.0],
            [0.0, 20.0],
        ]]];
        let spacing = 2.0;
        let lines = polygon::hatch(&area, spacing, HatchDirection::Horizontal, 0.0);
        let chains = connect(&area, &lines, 2.0 * spacing, &fill, 0.1);
        let mut joined = 0;
        for chain in &chains {
            for w in chain.windows(2) {
                if lines.contains(&[w[0], w[1]]) || lines.contains(&[w[1], w[0]]) {
                    continue;
                }
                assert!(w[0][0] < 1e-9 && w[1][0] < 1e-9, "{w:?}");
                joined += 1;
            }
        }
        // Lines join in pairs at the wall only.
        assert!(joined >= lines.len() / 2 - 1, "{joined} of {}", lines.len());
        assert!(chains.len() <= lines.len() / 2 + 1, "{}", chains.len());
    }
}
