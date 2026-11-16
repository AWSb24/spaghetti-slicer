//! Raft and supports, built directly as polygons in every layer instead of
//! as a mesh.
//!
//! Support points lie on a square grid of `support_spacing` under every
//! triangle that faces down more steeply than `support_overhang_angle`
//! allows, plus every vertex that is a local minimum of the mesh (no
//! neighbour lower). Each point gets a support straight down to the raft: a
//! cone from `support_tip_diameter` at the model to `support_pillar_diameter`
//! over `support_tip_length`, then a cylinder. Neighbouring pillars that are
//! tall enough are tied together by crossed diagonal braces at 45 degrees.
//! Own design along the lines of the usual resin supports.

// ponytail: a support whose way down is blocked by the model is dropped
// instead of standing on the model, so overhangs above other parts of the
// model stay unsupported; so do the ceilings of hollowed cavities. Small
// overhang islands between grid points are only caught when they contain a
// local minimum. Pillars are not checked against walls next to them except
// at four points around their edge.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use slicer_core::polygon::{
    convex_hull, distance, from_oriented_loops, offset, sin_cos_degrees, union,
};
use slicer_core::{Contour, Mesh, Point, Shapes};

use crate::{SlaSettings, disc, unit_circle};

/// Corners of the polygon standing in for the round cross-section of a
/// support.
const SEGMENTS: usize = 16;

/// A support from the raft up to `top`, where it touches the model.
struct Pillar {
    at: Point,
    top: f64,
}

/// A diagonal strut from `from` at height `bottom` to `to` at `top`.
struct Brace {
    from: Point,
    to: Point,
    bottom: f64,
    top: f64,
}

/// Raft, pillars and braces of a print.
pub(crate) struct Structures {
    raft: Shapes,
    pillars: Vec<Pillar>,
    braces: Vec<Brace>,
    unit: Vec<Point>,
    layer_height: f64,
    raft_top: f64,
    tip_radius: f64,
    tip_length: f64,
    pillar_radius: f64,
}

impl Structures {
    /// Finds the support points of `mesh` (already placed on the screen and
    /// raised) and lays out raft, pillars and braces. `slices` are the
    /// model's layers; `drains` the drain holes, which supports keep clear
    /// of.
    pub(crate) fn build(
        mesh: &Mesh,
        slices: &[Shapes],
        settings: &SlaSettings,
        drains: &[Point],
    ) -> Self {
        let mut structures = Structures {
            raft: Vec::new(),
            pillars: Vec::new(),
            braces: Vec::new(),
            unit: unit_circle(SEGMENTS),
            layer_height: settings.layer_height,
            raft_top: settings.raft_thickness,
            tip_radius: settings.support_tip_diameter / 2.0,
            tip_length: settings.support_tip_length,
            pillar_radius: settings.support_pillar_diameter / 2.0,
        };
        if settings.supports {
            let surface = Surface::new(mesh, settings.support_spacing);
            structures.pillars = structures.pillars(&surface, settings, drains);
            structures.braces = structures.braces(&surface, settings);
        }
        if settings.raft_thickness > 0.0 {
            // Under every pillar and under the model's lowest layer.
            let r = structures.pillar_radius;
            let mut points: Vec<Point> = structures
                .pillars
                .iter()
                .flat_map(|p| disc(&structures.unit, p.at, r))
                .collect();
            if let Some(lowest) = slices.iter().find(|s| !s.is_empty()) {
                points.extend(lowest.iter().flat_map(|shape| shape[0].iter().copied()));
            }
            let hull = convex_hull(&points);
            if hull.len() >= 3 {
                structures.raft = offset(&vec![vec![hull]], r);
            }
        }
        structures
    }

    /// Points that need support and have a free way down to the raft.
    fn pillars(&self, surface: &Surface, settings: &SlaSettings, drains: &[Point]) -> Vec<Pillar> {
        let mesh = surface.mesh;
        let spacing = settings.support_spacing;
        let (steep, _) = sin_cos_degrees(settings.support_overhang_angle);
        // Keyed by grid cell: one point per cell, the lowest.
        let mut points: BTreeMap<(i64, i64), [f64; 3]> = BTreeMap::new();
        let mut lowest_neighbour = vec![f64::INFINITY; mesh.vertices.len()];
        for (t, tri) in mesh.triangles.iter().enumerate() {
            for k in 0..3 {
                let others =
                    [tri[(k + 1) % 3], tri[(k + 2) % 3]].map(|i| mesh.vertices[i as usize][2]);
                let slot = &mut lowest_neighbour[tri[k] as usize];
                *slot = slot.min(others[0]).min(others[1]);
            }
            let [a, b, c] = tri.map(|i| mesh.vertices[i as usize]);
            let (u, v) = (sub(b, a), sub(c, a));
            let normal = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            let length =
                (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
            // Facing down at more than the angle from the vertical.
            if -normal[2] <= steep * length {
                continue;
            }
            let (low, high) = bounds(&[a, b, c]);
            let cells =
                |lo: f64, hi: f64| (lo / spacing).ceil() as i64..=(hi / spacing).floor() as i64;
            for i in cells(low[0], high[0]) {
                for j in cells(low[1], high[1]) {
                    let (x, y) = (i as f64 * spacing, j as f64 * spacing);
                    if let Some(z) = surface.z_at(t, x, y) {
                        let point = points.entry((i, j)).or_insert([x, y, z]);
                        if z < point[2] {
                            *point = [x, y, z];
                        }
                    }
                }
            }
        }
        for (vertex, &lowest) in mesh.vertices.iter().zip(&lowest_neighbour) {
            if lowest.is_finite() && lowest >= vertex[2] {
                let key = (
                    (vertex[0] / spacing).round() as i64,
                    (vertex[1] / spacing).round() as i64,
                );
                points.entry(key).or_insert(*vertex);
            }
        }

        let r = self.pillar_radius;
        let drain_radius = settings.drain_hole_diameter / 2.0;
        points
            .into_values()
            .filter(|&[x, y, z]| {
                z - self.raft_top >= self.layer_height
                    && drains
                        .iter()
                        .all(|&d| distance(d, [x, y]) >= drain_radius + r)
                    && !surface.below(x, y, z - self.layer_height)
                    && [[r, 0.0], [-r, 0.0], [0.0, r], [0.0, -r]]
                        .iter()
                        .all(|d| !surface.below(x + d[0], y + d[1], z - self.tip_length))
            })
            .map(|[x, y, z]| Pillar { at: [x, y], top: z })
            .collect()
    }

    /// Crossed braces between every pillar and its nearest neighbour when
    /// both are tall enough and nothing of the model is in between.
    fn braces(&self, surface: &Surface, settings: &SlaSettings) -> Vec<Brace> {
        let mut braces = Vec::new();
        if settings.support_brace_height <= 0.0 {
            return braces;
        }
        let mut pairs = BTreeSet::new();
        for (i, a) in self.pillars.iter().enumerate() {
            let nearest = self
                .pillars
                .iter()
                .enumerate()
                .filter(|&(j, _)| j != i)
                .map(|(j, b)| (distance(a.at, b.at), j))
                .min_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
            if let Some((d, j)) = nearest
                && d > 0.0
                && d <= 2.0 * settings.support_spacing
            {
                pairs.insert((i.min(j), i.max(j)));
            }
        }
        for (i, j) in pairs {
            let (a, b) = (&self.pillars[i], &self.pillars[j]);
            let top = a.top.min(b.top) - self.tip_length;
            let rise = distance(a.at, b.at);
            let lerp = |t: f64| {
                [
                    a.at[0] + t * (b.at[0] - a.at[0]),
                    a.at[1] + t * (b.at[1] - a.at[1]),
                ]
            };
            if top - self.raft_top < settings.support_brace_height
                || [0.25, 0.5, 0.75].iter().any(|&t| {
                    let p = lerp(t);
                    surface.below(p[0], p[1], top)
                })
            {
                continue;
            }
            let mut z = self.raft_top;
            while z + rise <= top {
                for (from, to) in [(a.at, b.at), (b.at, a.at)] {
                    braces.push(Brace {
                        from,
                        to,
                        bottom: z,
                        top: z + rise,
                    });
                }
                z += rise;
            }
        }
        braces
    }

    /// The model's layer `index` with raft, supports and braces added.
    pub(crate) fn add_to(&self, slice: &Shapes, index: usize) -> Shapes {
        let z = (index as f64 + 0.5) * self.layer_height;
        if z < self.raft_top {
            return union(slice, &self.raft);
        }
        let mut discs: Vec<Contour> = Vec::new();
        for pillar in &self.pillars {
            let below_top = pillar.top - z;
            let radius = if below_top < -self.tip_radius {
                continue;
            } else if below_top <= 0.0 {
                // Reaches a little into the model so the tip holds.
                self.tip_radius
            } else if below_top < self.tip_length {
                self.tip_radius
                    + (self.pillar_radius - self.tip_radius) * below_top / self.tip_length
            } else {
                self.pillar_radius
            };
            discs.push(disc(&self.unit, pillar.at, radius));
        }
        for brace in &self.braces {
            if (brace.bottom..=brace.top).contains(&z) {
                let t = (z - brace.bottom) / (brace.top - brace.bottom);
                let at = [
                    brace.from[0] + t * (brace.to[0] - brace.from[0]),
                    brace.from[1] + t * (brace.to[1] - brace.from[1]),
                ];
                discs.push(disc(&self.unit, at, self.pillar_radius / 2.0));
            }
        }
        if discs.is_empty() {
            return slice.clone();
        }
        union(slice, &from_oriented_loops(&discs))
    }
}

/// The mesh's triangles sorted into square cells by their extent in XY, to
/// find the surfaces above or below a point quickly.
struct Surface<'a> {
    mesh: &'a Mesh,
    cell: f64,
    cells: HashMap<(i64, i64), Vec<usize>>,
}

impl<'a> Surface<'a> {
    fn new(mesh: &'a Mesh, cell: f64) -> Self {
        let mut cells: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (t, tri) in mesh.triangles.iter().enumerate() {
            let (low, high) = bounds(&tri.map(|i| mesh.vertices[i as usize]));
            for i in (low[0] / cell).floor() as i64..=(high[0] / cell).floor() as i64 {
                for j in (low[1] / cell).floor() as i64..=(high[1] / cell).floor() as i64 {
                    cells.entry((i, j)).or_default().push(t);
                }
            }
        }
        Surface { mesh, cell, cells }
    }

    /// Height of triangle `t` above (x, y), if (x, y) lies in its shadow.
    fn z_at(&self, t: usize, x: f64, y: f64) -> Option<f64> {
        let [a, b, c] = self.mesh.triangles[t].map(|i| self.mesh.vertices[i as usize]);
        let (u, v) = (sub(b, a), sub(c, a));
        let d = u[0] * v[1] - u[1] * v[0];
        if d == 0.0 {
            return None;
        }
        let (px, py) = (x - a[0], y - a[1]);
        let s = (px * v[1] - py * v[0]) / d;
        let r = (u[0] * py - u[1] * px) / d;
        (s >= 0.0 && r >= 0.0 && s + r <= 1.0).then(|| a[2] + s * u[2] + r * v[2])
    }

    /// Whether some surface of the model lies under (x, y) lower than `z`.
    fn below(&self, x: f64, y: f64, z: f64) -> bool {
        let key = (
            (x / self.cell).floor() as i64,
            (y / self.cell).floor() as i64,
        );
        self.cells.get(&key).is_some_and(|triangles| {
            triangles
                .iter()
                .any(|&t| self.z_at(t, x, y).is_some_and(|height| height < z))
        })
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// Smallest and largest corner of the box around some points.
fn bounds(points: &[[f64; 3]]) -> ([f64; 3], [f64; 3]) {
    points.iter().fold(
        ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]),
        |(mut low, mut high), p| {
            for a in 0..3 {
                low[a] = low[a].min(p[a]);
                high[a] = high[a].max(p[a]);
            }
            (low, high)
        },
    )
}
