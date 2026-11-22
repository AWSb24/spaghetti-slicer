//! Tree supports: branches that start at points under the overhangs, grow
//! down layer by layer, lean towards each other and merge into trunks that
//! stand on the bed, or on the model where they cannot get around it.
//!
//! Own design after the idea of Vanek, Galicia and Benes, "Clever Support:
//! Efficient Support Structure Generation for Digital Fabrication" (2014):
//! supports start at sampled points and join on their way down within a
//! bounded angle. Here every layer moves each branch a bounded step
//! (`tree_branch_angle`) towards a partner (branches pair up, closest pairs
//! first) or else its nearest neighbour, out of the way of the model of the
//! next few layers and out of the model itself, and merges branches that
//! touch. Branches start `tree_contact_spacing` apart as thin as
//! `tree_tip_diameter` and thicken gradually on the way down with the area
//! they carry, up to `tree_branch_diameter`. A branch that never merges
//! with another one widens below its tip like a slim cone instead of
//! standing as a needle.

use slicer_core::polygon::{self, distance, sin_cos_degrees};
use slicer_core::{Contour, Point, Shapes, parallel_map};
use slicer_profiles::Settings;

use crate::{MIN_SUPPORT_AREA, tidy_support};

/// Corners of the polygon that stands for the round cross-section of a
/// branch.
const CIRCLE_SEGMENTS: u32 = 32;

/// How far a branch leans towards its neighbour per layer
/// (`tree_branch_angle`) as a share of how far it may move at most; the
/// rest is room for getting out of the way of the model. A branch may move
/// at least as far as a layer may hang over the one below it.
const LEAN: f64 = 0.8;

/// Branches lean only towards neighbours closer than this, in multiples of
/// `tree_contact_spacing`; a branch without one leans towards a farther
/// neighbour only if it can get there on the upper half of its way down to
/// the bed (see [`branches`]), else it grows straight down.
const ATTRACTION: f64 = 4.0;

/// How much thicker a branch that never merges with another one gets per
/// millimetre below its tip, in millimetres of radius: from its tip down
/// to where it stands, it is a slim cone instead of a needle as thin as
/// its tip (a 1 mm wider diameter every 10 mm).
const LONE_TAPER: f64 = 0.05;

/// Smallest radius of a branch, and that of a tip without
/// `tree_tip_diameter`, in line widths: the support loop around it, half a
/// line width inside, still has room for a line.
const TIP_RADIUS: f64 = 1.0;

/// Two branches merge when their centres are closer than this share of
/// the sum of their radii: half-way overlapping.
const MERGE: f64 = 0.5;

/// How fast a branch thickens to the radius its load asks for after two
/// branches merged, per layer, as a share of the overhang a layer may have
/// without support of its own.
const GROWTH: f64 = 0.5;

/// How many layers below the current one a branch looks ahead to get out
/// of the way of the model early, instead of being pushed aside abruptly
/// on the layer where it meets the model. A power of two.
const LOOK_AHEAD: usize = 16;

/// Radius of the thickest trunk without `tree_branch_diameter`, in line
/// widths.
const MAX_RADIUS: f64 = 10.0;

/// Grid points this close to an overhang (in contact spacings) that are
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

/// Where tree supports go on every layer: under every overhang, starting
/// on the layers of `starts` (what hangs free the gap above them),
/// `interface` layers of its own shape (the dense interface), below that
/// the branches, all of it clear of `keep_out`. The branches also keep out
/// of `blockers` (support blockers, on every layer) as they keep out of the
/// model, and are cut off where they cannot.
pub(crate) fn areas(
    starts: &[Shapes],
    keep_out: &[Shapes],
    blockers: Option<&[Shapes]>,
    cut_heights: &[f64],
    settings: &Settings,
) -> Vec<Shapes> {
    let avoid: Vec<Shapes>;
    let clear = match blockers {
        Some(blockers) => {
            avoid = parallel_map(starts.len(), |index| match blockers.get(index) {
                Some(blocker) if !blocker.is_empty() => polygon::union(&keep_out[index], blocker),
                _ => keep_out[index].clone(),
            });
            avoid.as_slice()
        }
        None => keep_out,
    };
    let branches = branches(starts, clear, cut_heights, settings);
    let interface = settings.support_interface_layers as usize;
    let circle: Vec<(f64, f64)> = (0..CIRCLE_SEGMENTS)
        .map(|k| {
            let (sin, cos) = sin_cos_degrees(f64::from(k) * 360.0 / f64::from(CIRCLE_SEGMENTS));
            (cos, sin)
        })
        .collect();
    parallel_map(starts.len(), |index| {
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
        if let Some(blocker) = blockers
            .and_then(|blockers| blockers.get(index))
            .filter(|blocker| !blocker.is_empty() && !area.is_empty())
        {
            area = polygon::difference(&area, blocker);
        }
        for above in starts.iter().skip(index).take(interface) {
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

/// Radius of the tip of a branch: half of `tree_tip_diameter`, at least
/// [`TIP_RADIUS`] line widths, which is also what 0 means.
pub(crate) fn tip_radius(settings: &Settings) -> f64 {
    (0.5 * settings.tree_tip_diameter).max(TIP_RADIUS * settings.line_width)
}

/// The branch nearest to branch `k`, at most `limit` away.
fn nearest(nodes: &[Node], k: usize, limit: f64) -> Option<usize> {
    let mut neighbour: Option<(usize, f64)> = None;
    for (m, other) in nodes.iter().enumerate() {
        let d = distance(nodes[k].at, other.at);
        if m != k && d <= limit && neighbour.is_none_or(|(_, best)| d < best) {
            neighbour = Some((m, d));
        }
    }
    neighbour.map(|(m, _)| m)
}

/// For every branch the nearest other one at most `limit` away.
fn nearest_neighbours(nodes: &[Node], limit: f64) -> Vec<Option<usize>> {
    (0..nodes.len()).map(|k| nearest(nodes, k, limit)).collect()
}

/// For every branch the one it leans towards: branches pair up, closest
/// pairs first, so that two of them always lean towards each other and
/// meet, instead of a row of evenly spaced branches all following their
/// neighbour in the same direction. A branch left without a partner leans
/// towards its nearest neighbour, at most `limit` away.
// ponytail: greedy pairing, not the best matching, compared all against
// all on every layer; candidates are the pairs at most twice as far apart
// as the nearer one's nearest neighbour.
fn partners(nodes: &[Node], limit: f64) -> Vec<Option<usize>> {
    let nearest = nearest_neighbours(nodes, limit);
    // Candidate pairs: every branch with the others at most twice as far
    // as its nearest one.
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (k, node) in nodes.iter().enumerate() {
        let Some(n) = nearest[k] else { continue };
        let reach = (2.0 * distance(node.at, nodes[n].at)).min(limit);
        for (m, other) in nodes.iter().enumerate().skip(k + 1) {
            let d = distance(node.at, other.at);
            if d <= reach {
                pairs.push((d, k, m));
            }
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut partner: Vec<Option<usize>> = vec![None; nodes.len()];
    for (_, k, m) in pairs {
        if partner[k].is_none() && partner[m].is_none() {
            partner[k] = Some(m);
            partner[m] = Some(k);
        }
    }
    partner
        .into_iter()
        .zip(nearest)
        .map(|(paired, near)| paired.or(near))
        .collect()
}

/// The branches on every layer, grown from the top down. Every branch
/// lists the branches of the layer above that it continues.
// ponytail: a branch that cannot get around the model, even looking
// ahead, goes on straight down, clipped, until the model covers it, and
// then stands on the model. Looking ahead only sees the next
// `LOOK_AHEAD` layers, so a branch can still lean into a dead end farther
// down.
fn branches(
    starts: &[Shapes],
    keep_out: &[Shapes],
    cut_heights: &[f64],
    settings: &Settings,
) -> Vec<Vec<Node>> {
    let count = starts.len();
    let width = settings.line_width;
    let spacing = settings.support_spacing;
    let interface = settings.support_interface_layers as usize;
    let contact = settings.tree_contact_spacing;
    let tip = tip_radius(settings);
    let thickest = match settings.tree_branch_diameter {
        0.0 => MAX_RADIUS * width,
        diameter => 0.5 * diameter,
    }
    .max(tip);
    // How far a branch leans per millimetre of height.
    let (sin, cos) = sin_cos_degrees(settings.tree_branch_angle);
    let slope = sin / cos;
    // The thickness a branch grows to, by the area it carries.
    let target = |load: f64| (tip * (load / (spacing * spacing)).max(1.0).sqrt()).min(thickest);

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

        // Grow the branches of the layer above one layer down. A branch
        // leans at `tree_branch_angle` and may move as far as a layer may
        // hang over the one below it, or a quarter more than it leans.
        if index + 1 < count && !nodes.is_empty() {
            let thickness = cut_heights[index + 1] - cut_heights[index];
            let reach = 0.5 * width * thickness / settings.layer_height;
            let step = slope * thickness;
            let reach = reach.max(step / LEAN);
            let mut grown = Vec::with_capacity(nodes.len());
            let partner = partners(&nodes, ATTRACTION * contact);
            // A branch without a neighbour close by leans towards a farther
            // one that it reaches on the upper half of its way down to the
            // bed, instead of running down alone.
            let far = 0.5 * slope * cut_heights[index];
            for (k, node) in nodes.iter().enumerate() {
                let r = node
                    .radius
                    .max(target(node.load).min(node.radius + GROWTH * reach));
                let toward = partner[k].or_else(|| nearest(&nodes, k, far));
                let neighbour = toward.map(|m| (nodes[m].at, distance(node.at, nodes[m].at)));
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
                    from: vec![k],
                });
            }
            nodes = grown;
        }

        // New branches start right under the dense interface.
        if let Some(above) = starts.get(index + interface) {
            for (at, load) in contact_points(above, contact) {
                // As thin as a tip; it thickens on the way down.
                let radius = tip;
                if let Some(at) = outline
                    .escape(at, radius)
                    .filter(|&p| distance(at, p) <= 0.5 * contact)
                {
                    nodes.push(Node {
                        at,
                        load,
                        radius,
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
                if distance(a.at, b.at) < MERGE * (a.radius + b.radius) {
                    let load = a.load + b.load;
                    let at = [
                        (a.at[0] * a.load + b.at[0] * b.load) / load,
                        (a.at[1] * a.load + b.at[1] * b.load) / load,
                    ];
                    let b = nodes.remove(j);
                    let a = &mut nodes[i];
                    // Between the two, the merged branch may reach into the
                    // model; it moves out of it, or else stays where the
                    // thicker one was.
                    let thicker = if b.radius > a.radius { b.at } else { a.at };
                    a.load = load;
                    a.radius = a.radius.max(b.radius);
                    a.at = outline
                        .escape(at, a.radius + ESCAPE_MARGIN)
                        .unwrap_or(thicker);
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
    taper_lone(&mut layers, cut_heights, tip, thickest);
    layers
}

/// Thickens the branches that run from their tip down to where they stand
/// without ever merging with another one: a branch that carries little
/// stays as thin as its tip, a needle that wobbles and is printed as a tiny
/// ring on every layer. Such a branch becomes a slim cone instead,
/// [`LONE_TAPER`] wider per millimetre below its tip (and at most
/// `thickest`); branches that merge thicken with their load anyway.
// ponytail: thickened after the branches have been placed, so a thicker
// branch is only clipped where it reaches into the model, not moved out of
// its way.
fn taper_lone(layers: &mut [Vec<Node>], cut_heights: &[f64], tip: f64, thickest: f64) {
    let count = layers.len();
    // For every branch the height of its tip, if it continues a single tip
    // that has not taken up any other branch (so still carries the same).
    let mut tips: Vec<Vec<Option<f64>>> = vec![Vec::new(); count];
    for index in (0..count).rev() {
        tips[index] = layers[index]
            .iter()
            .map(|node| match node.from.as_slice() {
                [] => Some(cut_heights[index]),
                &[k] if layers[index + 1][k].load == node.load => tips[index + 1][k],
                _ => None,
            })
            .collect();
    }
    // From the bottom up: a branch stays alone if it ends here or the
    // branch continuing it does.
    let mut alone: Vec<Vec<bool>> = vec![Vec::new(); count];
    for index in 0..count {
        let mut below = vec![None; layers[index].len()];
        if index > 0 {
            for (m, node) in layers[index - 1].iter().enumerate() {
                for &k in &node.from {
                    below[k] = Some(m);
                }
            }
        }
        alone[index] = (0..layers[index].len())
            .map(|k| tips[index][k].is_some() && below[k].is_none_or(|m| alone[index - 1][m]))
            .collect();
    }
    for index in 0..count {
        for (k, node) in layers[index].iter_mut().enumerate() {
            if let (true, Some(top)) = (alone[index][k], tips[index][k]) {
                let tapered = tip + LONE_TAPER * (top - cut_heights[index]);
                node.radius = node.radius.max(tapered.min(thickest));
            }
        }
    }
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
        grow_with(
            model,
            &Settings {
                // Many tips to merge: as close as before `tree_contact_spacing`.
                tree_contact_spacing: 2.5,
                ..Settings::default()
            },
        )
    }

    /// Like [`grow`] with the given settings.
    fn grow_with(
        model: impl Fn(usize) -> Shapes,
        settings: &Settings,
    ) -> (Vec<Vec<Node>>, Vec<Shapes>, Settings) {
        grow_under(&rect(0.0, 0.0, 30.0, 10.0), 50, model, settings)
    }

    /// Branches under `plate`, two layers thick from layer `top` up, on
    /// layers of 0.2 mm, like [`grow_with`].
    fn grow_under(
        plate: &Shapes,
        top: usize,
        model: impl Fn(usize) -> Shapes,
        settings: &Settings,
    ) -> (Vec<Vec<Node>>, Vec<Shapes>, Settings) {
        let settings = settings.clone();
        let count = top + 10;
        let slices: Vec<Shapes> = (0..count)
            .map(|i| {
                if (top..top + 2).contains(&i) {
                    plate.clone()
                } else {
                    model(i)
                }
            })
            .collect();
        // The supports start one empty layer below the plate.
        let starts: Vec<Shapes> = (0..count)
            .map(|i| match i {
                _ if i + 2 == top => polygon::difference(plate, &slices[top - 1]),
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
        let branches = branches(&starts, &keep_out, &heights, &settings);
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
        // Thin tips, thicker trunks.
        let thickest = |index: usize| {
            branches[index]
                .iter()
                .map(|n| n.radius)
                .fold(f64::MIN, f64::max)
        };
        assert!(thickest(46) < thickest(0));
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
                // Branches keep clear of the simplified outline, which may
                // lie up to its resolution inside the exact one.
                assert!(
                    near.is_none_or(|(_, d)| d >= node.radius - crate::SUPPORT_RESOLUTION),
                    "layer {index}"
                );
            }
        }
    }

    #[test]
    fn branch_angle_limits_the_lean() {
        // Nothing in the way below the plate: branches move only to lean
        // towards each other, at most `tree_branch_angle` from the
        // vertical on every layer of 0.2 mm.
        let free = |_| Vec::new();
        let most = |angle: f64| {
            let settings = Settings {
                tree_branch_angle: angle,
                ..Settings::default()
            };
            let (branches, _, _) = grow_with(free, &settings);
            let mut most: f64 = 0.0;
            for index in 0..46 {
                // Branches that merged on this layer moved to the middle.
                for node in branches[index].iter().filter(|n| n.from.len() == 1) {
                    let above = &branches[index + 1][node.from[0]];
                    most = most.max(distance(node.at, above.at));
                }
            }
            most
        };
        let tan = |angle: f64| {
            let (sin, cos) = sin_cos_degrees(angle);
            sin / cos
        };
        let (steep, flat) = (most(20.0), most(60.0));
        assert!(steep <= 0.2 * tan(20.0) + 1e-9, "{steep}");
        assert!(flat <= 0.2 * tan(60.0) + 1e-9, "{flat}");
        assert!(flat > 0.2 * tan(20.0) + 1e-6, "{flat}");
        // Straight down without any lean.
        assert!(most(0.0) < 1e-9);
    }

    #[test]
    fn tip_and_branch_diameters_set_the_thickness() {
        let free = |_| Vec::new();
        let radii = |settings: &Settings| {
            let (branches, _, _) = grow_with(free, settings);
            // The tips start on layer 46, right under the two interface
            // layers; layer 0 holds the trunks on the bed.
            let tips: Vec<f64> = branches[46].iter().map(|n| n.radius).collect();
            let thickest = branches
                .iter()
                .flatten()
                .map(|n| n.radius)
                .fold(0.0, f64::max);
            (tips, thickest)
        };
        // By default two line widths across.
        let (tips, thickest) = radii(&Settings::default());
        assert!(!tips.is_empty());
        assert!(tips.iter().all(|&r| (r - 0.45).abs() < 1e-9), "{tips:?}");
        assert!(thickest > 0.45);
        // Thicker tips, and trunks no thicker than 1.5 mm across.
        let settings = Settings {
            tree_tip_diameter: 1.2,
            tree_branch_diameter: 1.5,
            ..Settings::default()
        };
        let (tips, thickest) = radii(&settings);
        assert!(tips.iter().all(|&r| (r - 0.6).abs() < 1e-9), "{tips:?}");
        assert!(thickest <= 0.75 + 1e-9 && thickest > 0.6, "{thickest}");
        // A tip is never thinner than two line widths, which leaves room
        // for its loop.
        let settings = Settings {
            tree_tip_diameter: 0.2,
            ..Settings::default()
        };
        let (tips, _) = radii(&settings);
        assert!(tips.iter().all(|&r| (r - 0.45).abs() < 1e-9), "{tips:?}");
    }

    #[test]
    fn a_lone_branch_tapers_down_to_the_bed() {
        // A 2 mm square on layer 50 with nothing around it: a single tip
        // that carries little, so it would stay as thin as the tip all the
        // way down to the bed.
        let square = rect(1.5, 1.5, 3.5, 3.5);
        let settings = Settings::default();
        let (branches, _, _) = grow_under(&square, 50, |_| Vec::new(), &settings);
        let tip = tip_radius(&settings);
        assert_eq!(branches[46].len(), 1);
        assert!((branches[46][0].radius - tip).abs() < 1e-9);
        // It gets thicker below its tip and stands on the bed as a cone:
        // 1 mm wider across for every 10 mm below the tip.
        assert_eq!(branches[0].len(), 1);
        let height = 0.2 * 46.0;
        let base = branches[0][0].radius;
        assert!((base - (tip + LONE_TAPER * height)).abs() < 1e-9, "{base}");
        for index in 0..46 {
            assert!(branches[index][0].radius >= branches[index + 1][0].radius);
        }
    }

    #[test]
    fn lone_branches_lean_towards_far_neighbours() {
        // Two 2 mm squares 25 mm apart on layer 300 (60 mm up), farther
        // apart than branches lean towards each other by default (four
        // contact spacings, 20 mm). As they have the height to meet on the
        // way down, they lean towards each other and stand on the bed as
        // one instead of two needles.
        let mut squares = rect(1.5, 1.5, 3.5, 3.5);
        squares.extend(rect(26.5, 1.5, 28.5, 3.5));
        let settings = Settings::default();
        let (branches, _, _) = grow_under(&squares, 300, |_| Vec::new(), &settings);
        assert_eq!(branches[296].len(), 2);
        assert_eq!(branches[0].len(), 1);
        // They meet on the upper half of the way down.
        assert_eq!(branches[150].len(), 1);
        // Too low to meet, they grow straight down.
        let (low, _, _) = grow_under(&squares, 50, |_| Vec::new(), &settings);
        assert_eq!(low[0].len(), 2);
        assert!((low[0][1].at[0] - low[46][1].at[0]).abs() < 1e-9);
    }

    #[test]
    fn wider_contact_spacing_gives_fewer_tips() {
        let free = |_| Vec::new();
        let tips = |spacing: f64| {
            let settings = Settings {
                tree_contact_spacing: spacing,
                ..Settings::default()
            };
            let (branches, _, _) = grow_with(free, &settings);
            branches[46].len()
        };
        let (close, wide) = (tips(2.5), tips(5.0));
        assert!(wide > 0 && 2 * wide <= close, "{wide} {close}");
    }
}
