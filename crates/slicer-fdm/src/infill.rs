//! Sparse infill patterns that are more than one or two hatchings.

use slicer_core::polygon::{self, HatchDirection, distance};
use slicer_core::{Point, Shapes};

/// Samples of the gyroid per period along X and Y: steps of 15 degrees.
const GYROID_STEPS: i64 = 24;

/// Length of the gyroid's cross-section per area, times its period,
/// averaged over the height. Measured numerically on the surface
/// `sin x cos y + sin y cos z + sin z cos x = 0` (2.455; single layers lie
/// between 2.31 and 2.77).
const GYROID_LENGTH: f64 = 2.455;

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

/// Cross-section of a gyroid at height `z`, clipped to `area`, with as much
/// line per area as straight lines `spacing` apart. Polylines shorter than
/// `min_length` are dropped.
// ponytail: the curve is traced on a square grid (marching squares, 24
// samples per period) and the infill angle is ignored.
pub(crate) fn gyroid(area: &Shapes, spacing: f64, z: f64, min_length: f64) -> Vec<Vec<Point>> {
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
    let mut lines = polygon::clip_polylines(area, &segments);
    lines.retain(|line| line.windows(2).map(|w| distance(w[0], w[1])).sum::<f64>() >= min_length);
    lines
}
