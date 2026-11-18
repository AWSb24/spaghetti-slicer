//! Tree supports: branches that start at points under the overhangs, grow
//! down layer by layer, lean towards each other and merge into trunks that
//! stand on the bed, or on the model where they cannot get around it.
//!
//! Own design after the idea of Vanek, Galicia and Benes, "Clever Support:
//! Efficient Support Structure Generation for Digital Fabrication" (2014):
//! supports start at sampled points and join on their way down within a
//! bounded angle. Here every layer moves each branch a bounded step towards
//! its nearest neighbour, out of the way of the model of the next few
//! layers and out of the model itself, and merges branches that touch.
//! Branches start thin and thicken on the way down with their length and,
//! gradually, with the area they carry.

use slicer_core::polygon::{self, distance, sin_cos_degrees};
use slicer_core::{Contour, Point, Shapes, parallel_map};
use slicer_profiles::Settings;

use crate::{MIN_SUPPORT_AREA, tidy_support};

/// Corners of the polygon that stands for the round cross-section of a
/// branch.
const CIRCLE_SEGMENTS: u32 = 32;

/// How far a branch leans towards its neighbour per layer, as a share of
/// the overhang a layer may have without support of its own.
const LEAN: f64 = 0.8;

/// Branches lean only towards neighbours closer than this, in multiples of
/// the support spacing; farther ones grow straight down.
const ATTRACTION: f64 = 4.0;

/// Radius of a branch that carries one point, in line widths.
const TIP_RADIUS: f64 = 1.0;

/// How much the radius of a branch grows per millimetre that it reaches
/// down from its tip, so that long branches stand more firmly.
const TAPER: f64 = 0.01;

/// How fast a branch thickens to the radius its load asks for after two
/// branches merged, per layer, as a share of the overhang a layer may have
/// without support of its own.
const GROWTH: f64 = 0.5;

/// How many layers below the current one a branch looks ahead to get out
/// of the way of the model early, instead of being pushed aside abruptly
/// on the layer where it meets the model. A power of two.
const LOOK_AHEAD: usize = 16;

/// Radius of the thickest trunk, in line widths.
const MAX_RADIUS: f64 = 10.0;

/// Grid points this close to an overhang (in support spacings) that are
/// not inside it are moved onto its edge, so that narrow overhangs between
/// the grid points get contact points too. Just above the half diagonal of
/// a grid cell, so that every overhang is reached.
const SAMPLE_REACH: f64 = 0.71;

/// Extra distance by which a branch is pushed out of the space kept free
/// around the model, in millimetres, so that clipping does not nibble at it.
const ESCAPE_MARGIN: f64 = 0.01;

/// One branch on its way down.
#[derive(Clone)]
struct Node {
    /// Centre of the branch.
    at: Point,
    /// Area of the overhang it carries, in mm².
    load: f64,
    /// Radius of the branch.
    radius: f64,
    /// Length of the longest of the branches it carries, from its tip down
    /// to here, in millimetres.
    length: f64,
    /// Which branches of the layer above it continues.
    from: Vec<usize>,
}

/// Contours with their bounding boxes, for distance queries.
struct Outline {
    contours: Vec<(Contour, [f64; 4])>,
}

impl Outline {
    fn new(shapes: &Shapes) -> Self {
        let contours = shapes
            .iter()
            .flatten()
            .filter(|contour| contour.len() > 2)
            .map(|contour| {
                let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
                for p in contour {
                    b = [
                        b[0].min(p[0]),
                        b[1].min(p[1]),
                        b[2].max(p[0]),
                        b[3].max(p[1]),
                    ];
                }
                (contour.clone(), b)
            })
            .collect();
        Self { contours }
    }

    /// Whether `p` lies inside the shapes (even-odd), and the nearest point
    /// of their outline with its distance; `None` without any outline.
    fn nearest(&self, p: Point) -> (bool, Option<(Point, f64)>) {
        let mut inside = false;
        let mut best: Option<(Point, f64)> = None;
        for (contour, b) in &self.contours {
            let crossing = p[1] >= b[1] && p[1] <= b[3] && p[0] < b[2];
            let dx = (b[0] - p[0]).max(p[0] - b[2]).max(0.0);
            let dy = (b[1] - p[1]).max(p[1] - b[3]).max(0.0);
            let near = best.is_none_or(|(_, d)| (dx * dx + dy * dy).sqrt() < d);
            if !crossing && !near {
                continue;
            }
            for (i, &a) in contour.iter().enumerate() {
                let c = contour[(i + 1) % contour.len()];
                if crossing
                    && (a[1] > p[1]) != (c[1] > p[1])
                    && p[0] < a[0] + (p[1] - a[1]) / (c[1] - a[1]) * (c[0] - a[0])
                {
                    inside = !inside;
                }
                if near {
                    let q = closest_on_segment(p, a, c);
                    let d = distance(p, q);
                    if best.is_none_or(|(_, bd)| d < bd) {
                        best = Some((q, d));
                    }
                }
            }
        }
        (inside, best)
    }

    /// Where a circle of radius `r` around `p` gets out of the shapes when
    /// pushed away from their nearest edge, if it does in a few pushes.
    fn escape(&self, mut p: Point, r: f64) -> Option<Point> {
        for _ in 0..4 {
            let (inside, near) = self.nearest(p);
            let Some((q, d)) = near else {
                return Some(p);
            };
            if !inside && d >= r {
                return Some(p);
            }
            if d == 0.0 {
                return None;
            }
            let (ux, uy) = if inside {
                ((q[0] - p[0]) / d, (q[1] - p[1]) / d)
            } else {
                ((p[0] - q[0]) / d, (p[1] - q[1]) / d)
            };
            p = [
                q[0] + ux * (r + ESCAPE_MARGIN),
                q[1] + uy * (r + ESCAPE_MARGIN),
            ];
        }
        let (inside, near) = self.nearest(p);
        (!inside && near.is_none_or(|(_, d)| d >= r)).then_some(p)
    }

    /// Whether a circle of radius `r` around `p` lies entirely inside.
    fn covers(&self, p: Point, r: f64) -> bool {
        matches!(self.nearest(p), (true, Some((_, d))) if d >= r)
    }
}

/// The point of the segment from `a` to `b` closest to `p`.
fn closest_on_segment(p: Point, a: Point, b: Point) -> Point {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length2 = dx * dx + dy * dy;
    if length2 == 0.0 {
        return a;
    }
    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / length2).clamp(0.0, 1.0);
    [a[0] + t * dx, a[1] + t * dy]
}

/// Points on a grid `spacing` apart where branches touch an overhang, and
/// the area each of them carries. Grid points just outside a narrow
/// overhang move onto its edge unless a point is already close.
fn contact_points(hanging: &Shapes, spacing: f64) -> Vec<(Point, f64)> {
    let mut nodes = Vec::new();
    for shape in hanging {
        let shape = vec![shape.clone()];
        let area = polygon::area(&shape);
        if area < MIN_SUPPORT_AREA {
            continue;
        }
        let outline = Outline::new(&shape);
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for (_, c) in &outline.contours {
            b = [
                b[0].min(c[0]),
                b[1].min(c[1]),
                b[2].max(c[2]),
                b[3].max(c[3]),
            ];
        }
        let cell = |v: f64| (v / spacing).floor() as i64;
        let (mut inner, mut edge) = (Vec::new(), Vec::new());
        for gy in cell(b[1]) - 1..=cell(b[3]) + 1 {
            for gx in cell(b[0]) - 1..=cell(b[2]) + 1 {
                // Cell centres, so that grid points do not fall on the
                // straight edges of overhangs that start on round numbers.
                let p = [(gx as f64 + 0.5) * spacing, (gy as f64 + 0.5) * spacing];
                match outline.nearest(p) {
                    (true, _) => inner.push(p),
                    (false, Some((q, d))) if d <= SAMPLE_REACH * spacing => edge.push(q),
                    _ => {}
                }
            }
        }
        for q in edge {
            if inner
                .iter()
                .all(|&p| distance(p, q) >= SAMPLE_REACH * spacing)
            {
                inner.push(q);
            }
        }
        let load = area / inner.len().max(1) as f64;
        nodes.extend(inner.into_iter().map(|at| (at, load)));
    }
    nodes
}

/// Where tree supports go on every layer: under every overhang
/// `interface` layers of its own shape (the dense interface), below that
/// the branches, all of it clear of `keep_out`.
pub(crate) fn areas(
    hanging: &[Shapes],
    keep_out: &[Shapes],
    cut_heights: &[f64],
    settings: &Settings,
) -> Vec<Shapes> {
    let branches = branches(hanging, keep_out, cut_heights, settings);
    let interface = settings.support_interface_layers as usize;
    let circle: Vec<(f64, f64)> = (0..CIRCLE_SEGMENTS)
        .map(|k| {
            let (sin, cos) = sin_cos_degrees(f64::from(k) * 360.0 / f64::from(CIRCLE_SEGMENTS));
            (cos, sin)
        })
        .collect();
    parallel_map(hanging.len(), |index| {
        let circles: Vec<Contour> = branches[index]
            .iter()
            .map(|node| {
                circle
                    .iter()
                    .map(|&(cos, sin)| {
                        [
                            node.at[0] + node.radius * cos,
                            node.at[1] + node.radius * sin,
                        ]
                    })
                    .collect()
            })
            .collect();
        let mut area = polygon::from_oriented_loops(&circles);
        for above in hanging.iter().skip(index + 2).take(interface) {
            area = polygon::union(&area, above);
        }
        if area.is_empty() {
            return Vec::new();
        }
        tidy_support(polygon::difference(&area, &keep_out[index]))
    })
}

/// The outlines of `shapes`, thinned out like the supports themselves.
fn simplified(shapes: &Shapes) -> Outline {
    let shapes: Shapes = shapes
        .iter()
        .map(|shape| {
            shape
                .iter()
                .map(|contour| polygon::simplify(contour, crate::SUPPORT_RESOLUTION))
                .collect()
        })
        .collect();
    Outline::new(&shapes)
}

/// The point at most `step` from `from` on the way to `to`.
fn towards(from: Point, to: Point, step: f64) -> Point {
    let d = distance(from, to);
    if d <= step {
        return to;
    }
    let t = step / d;
    [
        from[0] + (to[0] - from[0]) * t,
        from[1] + (to[1] - from[1]) * t,
    ]
}

/// The branches on every layer, grown from the top down. Every branch
/// lists the branches of the layer above that it continues.
// ponytail: a branch that cannot get around the model, even looking
// ahead, goes on straight down, clipped, until the model covers it, and
// then stands on the model. Looking ahead only sees the next
// `LOOK_AHEAD` layers, so a branch can still lean into a dead end farther
// down.
fn branches(
    hanging: &[Shapes],
    keep_out: &[Shapes],
    cut_heights: &[f64],
    settings: &Settings,
) -> Vec<Vec<Node>> {
    let count = hanging.len();
    let width = settings.line_width;
    let spacing = settings.support_spacing;
    let interface = settings.support_interface_layers as usize;
    // The thickness a branch grows to: by the area it carries and by its
    // length below the tip.
    let target = |load: f64, length: f64| {
        (TIP_RADIUS * width * (load / (spacing * spacing)).max(1.0).sqrt() + TAPER * length)
            .min(MAX_RADIUS * width)
    };

    // Where the model is on every layer, and on it and the layers right
    // below it together (doubling the number of layers in every round).
    let outlines = parallel_map(count, |index| simplified(&keep_out[index]));
    let mut below = keep_out.to_vec();
    let mut span = 1;
    while span < LOOK_AHEAD {
        below = parallel_map(count, |index| match index.checked_sub(span) {
            Some(lower) => polygon::union(&below[index], &below[lower]),
            None => below[index].clone(),
        });
        span *= 2;
    }
    let ahead = parallel_map(count, |index| simplified(&below[index]));
    drop(below);

    let mut layers: Vec<Vec<Node>> = (0..count).map(|_| Vec::new()).collect();
    let mut nodes: Vec<Node> = Vec::new();
    for index in (0..count).rev() {
        let outline = &outlines[index];

        // Grow the branches of the layer above one layer down. A branch may
        // move as far as a layer may hang over the one below it.
        if index + 1 < count && !nodes.is_empty() {
            let thickness = cut_heights[index + 1] - cut_heights[index];
            let reach = 0.5 * width * thickness / settings.layer_height;
            let step = LEAN * reach;
            let mut grown = Vec::with_capacity(nodes.len());
            for (k, node) in nodes.iter().enumerate() {
                let length = node.length + thickness;
                let r = node
                    .radius
                    .max(target(node.load, length).min(node.radius + GROWTH * reach));
                let mut neighbour: Option<(Point, f64)> = None;
                for (m, other) in nodes.iter().enumerate() {
                    let d = distance(node.at, other.at);
                    if m != k
                        && d <= ATTRACTION * spacing
                        && neighbour.is_none_or(|(_, best)| d < best)
                    {
                        neighbour = Some((other.at, d));
                    }
                }
                let attracted = match neighbour {
                    Some((other, d)) if d > 0.0 => towards(node.at, other, step.min(0.5 * d)),
                    _ => node.at,
                };
                // Looking ahead: where the model of the next layers is in
                // the way, the branch starts moving out of it now, if it
                // can get there within those layers.
                let wanted = ahead[index]
                    .escape(attracted, r)
                    .filter(|&goal| distance(node.at, goal) <= LOOK_AHEAD as f64 * reach)
                    .map_or(attracted, |goal| towards(node.at, goal, reach));
                let placed = [wanted, attracted, node.at].into_iter().find_map(|start| {
                    outline
                        .escape(start, r)
                        .filter(|&p| distance(node.at, p) <= reach)
                });
                // Stuck: clipped by the model, on its way out of it as far
                // as a layer allows, until the model covers it.
                let at = match placed {
                    Some(at) => at,
                    None if !outline.covers(node.at, r) => outline
                        .escape(node.at, r)
                        .map_or(node.at, |goal| towards(node.at, goal, reach)),
                    None => continue,
                };
                grown.push(Node {
                    at,
                    load: node.load,
                    radius: r,
                    length,
                    from: vec![k],
                });
            }
            nodes = grown;
        }

        // New branches start right under the dense interface.
        if let Some(above) = hanging.get(index + 2 + interface) {
            for (at, load) in contact_points(above, spacing) {
                let radius = target(load, 0.0);
                if let Some(at) = outline
                    .escape(at, radius)
                    .filter(|&p| distance(at, p) <= 0.5 * spacing)
                {
                    nodes.push(Node {
                        at,
                        load,
                        radius,
                        length: 0.0,
                        from: Vec::new(),
                    });
                }
            }
        }

        // Branches that touch become one, as thick as the thicker one; it
        // grows further on the way down.
        let mut i = 0;
        while i < nodes.len() {
            let mut j = i + 1;
            while j < nodes.len() {
                let (a, b) = (&nodes[i], &nodes[j]);
                if distance(a.at, b.at) < 0.5 * (a.radius + b.radius) {
                    let load = a.load + b.load;
                    let at = [
                        (a.at[0] * a.load + b.at[0] * b.load) / load,
                        (a.at[1] * a.load + b.at[1] * b.load) / load,
                    ];
                    let b = nodes.remove(j);
                    let a = &mut nodes[i];
                    a.at = at;
                    a.load = load;
                    a.radius = a.radius.max(b.radius);
                    a.length = a.length.max(b.length);
                    a.from.extend(b.from);
                    j = i + 1;
                } else {
                    j += 1;
                }
            }
            i += 1;
        }
        layers[index].clone_from(&nodes);
    }
    layers
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    /// A rectangle from `(x0, y0)` to `(x1, y1)`.
    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Shapes {
        vec![vec![vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]]]
    }

    /// Branches under a 30 × 10 mm plate on layer 50 that is carried by the
    /// model `model` on the layers below it, kept clear of like the
    /// supports in `support_areas`.
    fn grow(model: impl Fn(usize) -> Shapes) -> (Vec<Vec<Node>>, Vec<Shapes>, Settings) {
        let settings = Settings::default();
        let count = 60;
        let plate = rect(0.0, 0.0, 30.0, 10.0);
        let slices: Vec<Shapes> = (0..count)
            .map(|i| {
                if (50..52).contains(&i) {
                    plate.clone()
                } else {
                    model(i)
                }
            })
            .collect();
        let hanging: Vec<Shapes> = (0..count)
            .map(|i| match i {
                50 => polygon::difference(&plate, &slices[49]),
                _ => Vec::new(),
            })
            .collect();
        let keep_out: Vec<Shapes> = (0..count)
            .map(|i| {
                let below = &slices[i.saturating_sub(1)];
                polygon::offset(&polygon::union(&slices[i], below), settings.line_width)
            })
            .collect();
        let heights: Vec<f64> = (0..count).map(|i| 0.2 * (i + 1) as f64).collect();
        let branches = branches(&hanging, &keep_out, &heights, &settings);
        (branches, keep_out, settings)
    }

    #[test]
    fn branches_thicken_from_the_tip_down() {
        // The T: a column under the middle of the plate.
        let (branches, keep_out, _) = grow(|_| rect(10.0, 0.0, 20.0, 10.0));
        assert!(branches[46].len() >= 16, "{}", branches[46].len());
        assert!(!branches[0].is_empty());
        for index in 0..46 {
            for node in &branches[index] {
                // Never thinner than any branch it continues.
                for &k in &node.from {
                    let above = &branches[index + 1][k];
                    assert!(node.radius >= above.radius, "layer {index}");
                }
                // Clear of the model.
                let outline = Outline::new(&keep_out[index]);
                let (inside, near) = outline.nearest(node.at);
                assert!(!inside, "layer {index}: {:?}", node.at);
                assert!(
                    near.is_none_or(|(_, d)| d >= node.radius - 1e-9),
                    "layer {index}"
                );
            }
        }
        // Thin tips, thicker trunks; growing by length too.
        let thinnest = |index: usize| {
            branches[index]
                .iter()
                .map(|n| n.radius)
                .fold(f64::MAX, f64::min)
        };
        assert!(thinnest(46) < thinnest(0));
    }

    #[test]
    fn branches_get_out_of_the_way_early() {
        // A narrow block 4 mm wide under the middle of the plate, up to
        // layer 30. Looking ahead, the branches above it lean aside before
        // they reach it, and all of them get down to the bed.
        let (branches, keep_out, _) = grow(|i| {
            if i <= 30 {
                rect(13.0, 0.0, 17.0, 10.0)
            } else {
                Vec::new()
            }
        });
        let load = |index: usize| branches[index].iter().map(|n| n.load).sum::<f64>();
        assert!(
            (load(0) - load(46)).abs() < 1e-6,
            "{} {}",
            load(0),
            load(46)
        );
        for index in 0..46 {
            let outline = Outline::new(&keep_out[index]);
            for node in &branches[index] {
                let (inside, near) = outline.nearest(node.at);
                assert!(!inside, "layer {index}: {:?}", node.at);
                assert!(
                    near.is_none_or(|(_, d)| d >= node.radius - 1e-9),
                    "layer {index}"
                );
            }
        }
    }
}
