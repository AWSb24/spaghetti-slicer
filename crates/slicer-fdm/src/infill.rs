//! Sparse infill patterns that are more than one or two hatchings, and the
//! patterns of top and bottom surfaces.

use std::collections::BTreeSet;
use std::f64::consts::SQRT_2;

use slicer_core::polygon::{self, HatchDirection, distance};
use slicer_core::{Contour, Point, Shapes};
use slicer_profiles::{SeamPosition, SurfacePattern};

use crate::{CentreLine, PATH_RESOLUTION, PathKind, Work};

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

/// `area` turned by the angle with this sine and cosine about the origin.
fn turned(area: &Shapes, sin: f64, cos: f64) -> Shapes {
    area.iter()
        .map(|shape| {
            shape
                .iter()
                .map(|c| {
                    c.iter()
                        .map(|p| [p[0] * cos - p[1] * sin, p[0] * sin + p[1] * cos])
                        .collect()
                })
                .collect()
        })
        .collect()
}

/// Hexagons with as much line per area as straight lines `spacing` apart,
/// clipped to `area`; the same on every layer, so they stack into
/// hexagonal prisms. Zig-zag lines run along `angle` degrees; between two
/// of them, short lines across join the corners where they come closest.
/// Every edge of the hexagons is printed once: three edges meet in every
/// corner, so the zig-zags cannot take them all. Lines shorter than
/// `min_length` are dropped.
pub(crate) fn honeycomb(
    area: &Shapes,
    spacing: f64,
    angle: f64,
    min_length: f64,
) -> Vec<Vec<Point>> {
    // A tiling by hexagons of side s has 2 / (sqrt(3) s) of edges per area,
    // straight lines 1 / spacing. Half the width of a hexagon is then
    // `spacing`, and zig-zags lie 1.5 s apart.
    let side = 2.0 * spacing / 3.0_f64.sqrt();
    let rise = 1.5 * side;
    // Traced in a frame turned by `angle`, like the gyroid.
    let (sin, cos) = polygon::sin_cos_degrees(angle);
    let frame = turned(area, -sin, cos);
    let mut points = frame.iter().flatten().flatten();
    let Some(&first) = points.next() else {
        return Vec::new();
    };
    let (min, max) = points.fold((first, first), |(lo, hi), p| {
        (
            [lo[0].min(p[0]), lo[1].min(p[1])],
            [hi[0].max(p[0]), hi[1].max(p[1])],
        )
    });
    // Corners on a grid anchored at the origin, so that every layer is the
    // same wherever the area lies.
    let (i0, i1) = (
        (min[0] / spacing).floor() as i64 - 1,
        (max[0] / spacing).ceil() as i64 + 1,
    );
    let (j0, j1) = (
        (min[1] / rise).floor() as i64 - 1,
        (max[1] / rise).ceil() as i64 + 1,
    );
    let mut lines = Vec::new();
    for j in j0..=j1 {
        // Zig-zag j has its low corners on y = 1.5 s j and its high ones
        // half a side above, at every other multiple of `spacing`; from
        // each high corner a line of one side goes up to a low corner of
        // the next zig-zag.
        let low = j as f64 * rise;
        let high = |i: i64| (i - j).rem_euclid(2) == 0;
        lines.push(
            (i0..=i1)
                .map(|i| {
                    let y = if high(i) { low + 0.5 * side } else { low };
                    [i as f64 * spacing, y]
                })
                .collect(),
        );
        for i in (i0..=i1).filter(|&i| high(i)) {
            let x = i as f64 * spacing;
            lines.push(vec![[x, low + 0.5 * side], [x, low + rise]]);
        }
    }
    polygon::clip_polylines(&frame, &lines)
        .into_iter()
        .filter(|line| line.windows(2).map(|w| distance(w[0], w[1])).sum::<f64>() >= min_length)
        .map(|line| {
            line.iter()
                .map(|p| [p[0] * cos - p[1] * sin, p[0] * sin + p[1] * cos])
                .collect()
        })
        .collect()
}

/// Lines in three directions 60° apart, starting at `angle`, each set
/// `spacing` apart, at height `z`: the cross-section of cubes standing on a
/// corner. The faces of such cubes are tilted by acos(1 / sqrt(3)) from
/// the horizontal, so every set moves sideways by z / sqrt(2) as the
/// height goes up, across its lines and in its own direction; the cells
/// change from layer to layer between triangles and hexagons.
pub(crate) fn cubic(
    area: &Shapes,
    spacing: f64,
    z: f64,
    angle: f64,
    min_length: f64,
) -> Vec<Vec<[Point; 2]>> {
    // A shift by whole spacings brings the lines back; what is left keeps
    // the numbers small.
    let travel = z / SQRT_2;
    let shift = travel - spacing * (travel / spacing).floor();
    (0..3)
        .map(|k| {
            let turn = angle + 120.0 * f64::from(k);
            // Across the lines, 120° on from one set to the next, so that
            // the three faces of a cube meet at right angles.
            let (sin, cos) = polygon::sin_cos_degrees(turn + 90.0);
            let by = [cos * shift, sin * shift];
            let moved: Shapes = area
                .iter()
                .map(|shape| {
                    shape
                        .iter()
                        .map(|c| c.iter().map(|p| [p[0] - by[0], p[1] - by[1]]).collect())
                        .collect()
                })
                .collect();
            polygon::hatch(&moved, spacing, HatchDirection::degrees(turn), min_length)
                .into_iter()
                .map(|line| line.map(|p| [p[0] + by[0], p[1] + by[1]]))
                .collect()
        })
        .collect()
}

/// Loops following the outline of `area`, `spacing` apart. Sparse loops
/// start half a spacing inside the outline, as hatch lines do, so they
/// have as much line per area as straight lines `spacing` apart. `solid`
/// loops start on the outline, like the ends of solid infill lines, and
/// what they leave open between them and in the middle (up to a line
/// `width` wide) gets lines of its own width along its middle, so that the
/// surface has no holes.
// ponytail: where the area is narrower than a line, a loop runs out and
// back with its sides closer than a line and deposits up to twice the
// material.
pub(crate) fn concentric(
    area: &Shapes,
    spacing: f64,
    width: f64,
    solid: bool,
) -> (Vec<Contour>, Vec<CentreLine>) {
    let mut loops = Vec::new();
    let mut missed = Vec::new();
    let first = if solid { 0.0 } else { 0.5 * spacing };
    // Every loop is offset from the area itself: offsets of offsets gather
    // more and more points on their rounded corners.
    let inset = |k: usize| first + k as f64 * spacing;
    let mut shapes = polygon::offset(area, -first);
    let mut k = 0;
    while !shapes.is_empty() {
        loops.extend(
            shapes
                .iter()
                .flatten()
                .map(|c| polygon::simplify(c, PATH_RESOLUTION)),
        );
        let inner = polygon::offset(area, -inset(k + 1));
        if solid {
            // A loop covers half a width to either side: what lies further
            // inside and is not covered by the next loop is missed.
            let reach = 0.5 * width;
            let core = polygon::offset(area, -(inset(k) + reach));
            // Growing a shape is slow; most of the time an estimate shows
            // that nothing worth a line is missed, as for the walls.
            let estimate = polygon::area(&core)
                - crate::grown_area(&inner, reach)
                - crate::corner_loss(&core, reach);
            if estimate >= crate::MIN_THIN_WIDTH * width * width {
                missed.extend(polygon::difference(&core, &polygon::offset(&inner, reach)));
            }
        }
        shapes = inner;
        k += 1;
    }
    let gaps = if missed.is_empty() {
        Vec::new()
    } else {
        crate::centre_lines(missed, width).0
    };
    (loops, gaps)
}

/// Lines `spacing` apart along `angle` degrees, clipped to `area`, all
/// pointing the same way and in the order to print them: a line comes only
/// after every line beside it in the row before, and where possible right
/// after one of them, so that every line is laid against
/// the one before and the surface has no seams where lines were laid from
/// both sides. Lines shorter than `min_length` are dropped.
pub(crate) fn monotonic(
    area: &Shapes,
    spacing: f64,
    angle: f64,
    min_length: f64,
) -> Vec<[Point; 2]> {
    let (sin, cos) = polygon::sin_cos_degrees(angle);
    let along = |p: Point| p[0] * cos + p[1] * sin;
    // The number of the hatch line through `p`.
    let row = |p: Point| ((p[1] * cos - p[0] * sin) / spacing - 0.5).round() as i64;
    let mut lines: Vec<(i64, f64, f64, [Point; 2])> =
        polygon::hatch(area, spacing, HatchDirection::degrees(angle), min_length)
            .into_iter()
            .map(|[a, b]| {
                let line = if along(a) <= along(b) { [a, b] } else { [b, a] };
                (row(line[0]), along(line[0]), along(line[1]), line)
            })
            .collect();
    lines.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));

    // Every line waits for the lines of the row before that lie beside it.
    let n = lines.len();
    let mut waits = vec![0_usize; n];
    let mut frees: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut start = 0;
    while start < n {
        let row = lines[start].0;
        let end = start + lines[start..].partition_point(|l| l.0 == row);
        let next = end + lines[end..].partition_point(|l| l.0 == row + 1);
        for a in start..end {
            for b in end..next {
                if lines[a].1 < lines[b].2 && lines[b].1 < lines[a].2 {
                    waits[b] += 1;
                    frees[a].push(b);
                }
            }
        }
        start = end;
    }

    // Lines are numbered by row and then by position along it, so the
    // first one ready is the lowest.
    let mut ready: BTreeSet<usize> = (0..n).filter(|&i| waits[i] == 0).collect();
    let mut order = Vec::with_capacity(n);
    let mut last: Option<usize> = None;
    while let Some(pick) = last
        .and_then(|l| frees[l].iter().copied().find(|b| ready.contains(b)))
        .or_else(|| ready.first().copied())
    {
        ready.remove(&pick);
        for &b in &frees[pick] {
            waits[b] -= 1;
            if waits[b] == 0 {
                ready.insert(b);
            }
        }
        order.push(lines[pick].3);
        last = Some(pick);
    }
    order
}

/// Splits the solid infill of a layer into the surface that gets `pattern`
/// and is printed as `kind`, and the rest, which keeps straight lines: the
/// part with nothing `above` it, or all of it without anything above (the
/// last layer, and the first one for its bottom surface). Returns the rest
/// and the work for the surface. Strips of surface narrower than a line,
/// along sloped sides, stay with the rest. The lines of the monotonic and
/// rectilinear patterns run along `angle` degrees like those of the rest.
/// A rectilinear surface of solid infill is not split off at all.
pub(crate) fn surface(
    solid: Shapes,
    above: Option<&Shapes>,
    pattern: SurfacePattern,
    angle: f64,
    width: f64,
    kind: PathKind,
) -> (Shapes, Vec<Work>) {
    let plain = pattern == SurfacePattern::Rectilinear && kind == PathKind::SolidInfill;
    if plain || solid.is_empty() {
        return (solid, Vec::new());
    }
    let (surface, rest) = match above {
        None => (solid, Vec::new()),
        Some(above) => {
            let top = polygon::difference(&solid, above);
            let top = polygon::offset(&polygon::offset(&top, -0.5 * width), 0.5 * width);
            // Loops run along the edge of the surface and cover half a
            // line beyond it, where the line ends of the rest reach too.
            // Joined straight lines run along it as well, where they link
            // one line to the next; so do those of the rest.
            let reach = if pattern != SurfacePattern::Monotonic {
                polygon::offset(&top, width)
            } else {
                top.clone()
            };
            (top, polygon::difference(&solid, &reach))
        }
    };
    let work = match pattern {
        SurfacePattern::Rectilinear => {
            let direction = HatchDirection::degrees(angle);
            let lines = polygon::hatch(&surface, width, direction, 0.5 * width);
            vec![Work::Lines(lines, kind, 2.0 * width)]
        }
        SurfacePattern::Concentric => {
            let (loops, gaps) = concentric(&surface, width, width, true);
            vec![
                Work::Loops(loops, kind, width, SeamPosition::Nearest),
                Work::Centred(gaps, kind),
            ]
        }
        SurfacePattern::Monotonic => vec![Work::Ordered(
            monotonic(&surface, width, angle, 0.5 * width),
            kind,
        )],
    };
    (rest, work)
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

    #[test]
    fn honeycomb_has_as_much_line_as_straight_lines() {
        let area = square(0.0, 60.0);
        let lines = honeycomb(&area, 3.0, 30.0, 0.0);
        assert_eq!(lines, honeycomb(&area, 3.0, 30.0, 0.0));
        let ratio = length(&lines) / (60.0 * 60.0 / 3.0);
        assert!((ratio - 1.0).abs() < 0.03, "{ratio}");
        for p in lines.iter().flatten() {
            assert!(p.iter().all(|v| (-1e-4..=60.0 + 1e-4).contains(v)), "{p:?}");
        }
    }

    #[test]
    fn concentric_loops_are_a_spacing_apart() {
        let (loops, gaps) = concentric(&square(0.0, 20.0), 2.0, 0.45, false);
        assert!(gaps.is_empty());
        // Squares 18, 14, 10, 6 and 2 mm wide.
        assert_eq!(loops.len(), 5);
        let total: f64 = loops.iter().map(|c| crate::line_length(c, true)).sum();
        assert!((total - 200.0).abs() < 1e-6, "{total}");
    }

    #[test]
    fn monotonic_lines_wait_for_the_lines_beside_them() {
        // A square with a notch from below: two legs, one top.
        let area = vec![vec![vec![
            [0.0, 0.0],
            [8.0, 0.0],
            [8.0, 12.0],
            [12.0, 12.0],
            [12.0, 0.0],
            [20.0, 0.0],
            [20.0, 20.0],
            [0.0, 20.0],
        ]]];
        let lines = monotonic(&area, 1.0, 0.0, 0.0);
        assert_eq!(lines.len(), 2 * 12 + 8);
        // Left to right; the left leg, then the right one, then the top.
        assert!(lines.iter().all(|[a, b]| a[0] < b[0] && a[1] == b[1]));
        let left = (0..12).map(|k| (0.0, k as f64 + 0.5));
        let right = (0..12).map(|k| (12.0, k as f64 + 0.5));
        let top = (12..20).map(|k| (0.0, k as f64 + 0.5));
        let expected: Vec<(f64, f64)> = left.chain(right).chain(top).collect();
        let starts: Vec<(f64, f64)> = lines.iter().map(|[a, _]| (a[0], a[1])).collect();
        assert_eq!(starts, expected);
    }
}
