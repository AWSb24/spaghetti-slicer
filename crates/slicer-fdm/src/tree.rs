//! Tree supports: branches that start at points under the overhangs, grow
//! down layer by layer, lean towards each other and merge into trunks that
//! stand on the bed, or on the model where they cannot get around it.
//!
//! Own design after the idea of Vanek, Galicia and Benes, "Clever Support:
//! Efficient Support Structure Generation for Digital Fabrication" (2014):
//! supports start at sampled points and join on their way down within a
//! bounded angle. Here every layer moves each branch a bounded step towards
//! its nearest neighbour, pushes it out of the model and merges branches
//! that touch.

use slicer_core::polygon::{self, distance, sin_cos_degrees};
use slicer_core::{Contour, Point, Shapes};
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
struct Node {
    /// Centre of the branch.
    at: Point,
    /// Area of the overhang it carries, in mm².
    load: f64,
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
fn contact_points(hanging: &Shapes, spacing: f64) -> Vec<Node> {
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
        nodes.extend(inner.into_iter().map(|at| Node { at, load }));
    }
    nodes
}

/// Where tree supports go on every layer: under every overhang
/// `interface` layers of its own shape (the dense interface), below that
/// the branches, all of it clear of `keep_out`.
// ponytail: branches grow only by the area they carry, not by their height;
// they avoid the model one layer at a time without looking ahead, so a
// branch can lean into a dead end and then stands on the model. A branch
// that cannot get around the model goes on straight down, clipped, until
// the model covers it completely.
pub(crate) fn areas(
    hanging: &[Shapes],
    keep_out: &[Shapes],
    cut_heights: &[f64],
    settings: &Settings,
) -> Vec<Shapes> {
    let count = hanging.len();
    let width = settings.line_width;
    let spacing = settings.support_spacing;
    let interface = settings.support_interface_layers as usize;
    let radius = |load: f64| {
        (TIP_RADIUS * width * (load / (spacing * spacing)).max(1.0).sqrt()).min(MAX_RADIUS * width)
    };
    let circle: Vec<(f64, f64)> = (0..CIRCLE_SEGMENTS)
        .map(|k| {
            let (sin, cos) = sin_cos_degrees(f64::from(k) * 360.0 / f64::from(CIRCLE_SEGMENTS));
            (cos, sin)
        })
        .collect();

    let mut areas = vec![Vec::new(); count];
    let mut nodes: Vec<Node> = Vec::new();
    for index in (0..count).rev() {
        let outline = Outline::new(
            keep_out[index]
                .iter()
                .map(|shape| {
                    shape
                        .iter()
                        .map(|contour| polygon::simplify(contour, crate::SUPPORT_RESOLUTION))
                        .collect()
                })
                .collect::<Shapes>()
                .as_ref(),
        );

        // Grow the branches of the layer above one layer down. A branch may
        // move as far as a layer may hang over the one below it.
        if index + 1 < count && !nodes.is_empty() {
            let reach =
                0.5 * width * (cut_heights[index + 1] - cut_heights[index]) / settings.layer_height;
            let step = LEAN * reach;
            let before: Vec<Point> = nodes.iter().map(|n| n.at).collect();
            let mut grown = Vec::with_capacity(nodes.len());
            for (k, node) in nodes.iter().enumerate() {
                let r = radius(node.load);
                let mut neighbour: Option<(Point, f64)> = None;
                for (m, &other) in before.iter().enumerate() {
                    let d = distance(node.at, other);
                    if m != k
                        && d <= ATTRACTION * spacing
                        && neighbour.is_none_or(|(_, best)| d < best)
                    {
                        neighbour = Some((other, d));
                    }
                }
                let wanted = match neighbour {
                    Some((other, d)) if d > 0.0 => {
                        let t = step.min(0.5 * d) / d;
                        [
                            node.at[0] + (other[0] - node.at[0]) * t,
                            node.at[1] + (other[1] - node.at[1]) * t,
                        ]
                    }
                    _ => node.at,
                };
                let placed = [wanted, node.at].into_iter().find_map(|start| {
                    outline
                        .escape(start, r)
                        .filter(|&p| distance(node.at, p) <= reach)
                });
                match placed {
                    Some(at) => grown.push(Node {
                        at,
                        load: node.load,
                    }),
                    // Stuck: straight down, clipped by the model, until the
                    // model covers it.
                    None if !outline.covers(node.at, r) => grown.push(Node {
                        at: node.at,
                        load: node.load,
                    }),
                    None => {}
                }
            }
            nodes = grown;
        }

        // New branches start right under the dense interface.
        if let Some(above) = hanging.get(index + 2 + interface) {
            for node in contact_points(above, spacing) {
                let r = radius(node.load);
                if let Some(at) = outline
                    .escape(node.at, r)
                    .filter(|&p| distance(node.at, p) <= 0.5 * spacing)
                {
                    nodes.push(Node {
                        at,
                        load: node.load,
                    });
                }
            }
        }

        // Branches that touch become one.
        let mut i = 0;
        while i < nodes.len() {
            let mut j = i + 1;
            while j < nodes.len() {
                let (a, b) = (&nodes[i], &nodes[j]);
                if distance(a.at, b.at) < 0.5 * (radius(a.load) + radius(b.load)) {
                    let load = a.load + b.load;
                    let at = [
                        (a.at[0] * a.load + b.at[0] * b.load) / load,
                        (a.at[1] * a.load + b.at[1] * b.load) / load,
                    ];
                    nodes[i] = Node { at, load };
                    nodes.remove(j);
                    j = i + 1;
                } else {
                    j += 1;
                }
            }
            i += 1;
        }

        let circles: Vec<Contour> = nodes
            .iter()
            .map(|node| {
                let r = radius(node.load);
                circle
                    .iter()
                    .map(|&(cos, sin)| [node.at[0] + r * cos, node.at[1] + r * sin])
                    .collect()
            })
            .collect();
        let mut area = polygon::from_oriented_loops(&circles);
        for above in hanging.iter().skip(index + 2).take(interface) {
            area = polygon::union(&area, above);
        }
        if !area.is_empty() {
            areas[index] = tidy_support(polygon::difference(&area, &keep_out[index]));
        }
    }
    areas
}
