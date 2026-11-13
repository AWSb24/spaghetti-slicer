//! Indexed triangle meshes.

use std::collections::HashMap;

use crate::diagnostics::{Diagnostic, Diagnostics};

/// An indexed triangle mesh. Triangles are expected to be wound
/// counter-clockwise when seen from outside the solid.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mesh {
    /// Vertex positions in millimetres.
    pub vertices: Vec<[f64; 3]>,
    /// Triangles as indices into `vertices`.
    pub triangles: Vec<[u32; 3]>,
}

impl Mesh {
    /// Builds a mesh from unconnected triangles, merging vertices with
    /// identical coordinates and dropping degenerate triangles.
    pub fn from_triangle_soup(triangles: &[[[f32; 3]; 3]]) -> Self {
        let mut index: HashMap<[u32; 3], u32> = HashMap::new();
        let mut mesh = Mesh::default();
        for tri in triangles {
            let mut ids = [0u32; 3];
            for (slot, v) in ids.iter_mut().zip(tri) {
                // `+ 0.0` turns -0.0 into 0.0 so both map to the same vertex.
                let key = [
                    (v[0] + 0.0).to_bits(),
                    (v[1] + 0.0).to_bits(),
                    (v[2] + 0.0).to_bits(),
                ];
                *slot = *index.entry(key).or_insert_with(|| {
                    mesh.vertices
                        .push([f64::from(v[0]), f64::from(v[1]), f64::from(v[2])]);
                    (mesh.vertices.len() - 1) as u32
                });
            }
            if ids[0] != ids[1] && ids[1] != ids[2] && ids[0] != ids[2] {
                mesh.triangles.push(ids);
            }
        }
        mesh
    }

    /// Returns the triangles as unconnected single-precision triangles.
    pub fn to_triangle_soup(&self) -> Vec<[[f32; 3]; 3]> {
        self.triangles
            .iter()
            .map(|t| t.map(|i| self.vertices[i as usize].map(|c| c as f32)))
            .collect()
    }

    /// An axis-aligned box with one corner at the origin.
    pub fn cuboid(x: f64, y: f64, z: f64) -> Self {
        Mesh {
            vertices: vec![
                [0.0, 0.0, 0.0],
                [x, 0.0, 0.0],
                [x, y, 0.0],
                [0.0, y, 0.0],
                [0.0, 0.0, z],
                [x, 0.0, z],
                [x, y, z],
                [0.0, y, z],
            ],
            triangles: vec![
                [0, 2, 1],
                [0, 3, 2],
                [4, 5, 6],
                [4, 6, 7],
                [0, 1, 5],
                [0, 5, 4],
                [1, 2, 6],
                [1, 6, 5],
                [2, 3, 7],
                [2, 7, 6],
                [3, 0, 4],
                [3, 4, 7],
            ],
        }
    }

    /// Minimum and maximum corner of the bounding box, or `None` if the mesh
    /// has no triangles.
    pub fn bounds(&self) -> Option<([f64; 3], [f64; 3])> {
        let mut used = self
            .triangles
            .iter()
            .flatten()
            .map(|&i| self.vertices[i as usize]);
        let first = used.next()?;
        Some(used.fold((first, first), |(mut lo, mut hi), v| {
            for a in 0..3 {
                lo[a] = lo[a].min(v[a]);
                hi[a] = hi[a].max(v[a]);
            }
            (lo, hi)
        }))
    }

    /// Moves every vertex by `delta`.
    pub fn translate(&mut self, delta: [f64; 3]) {
        for v in &mut self.vertices {
            for a in 0..3 {
                v[a] += delta[a];
            }
        }
    }

    /// Rotates every vertex around the Z axis by `angle` radians.
    pub fn rotate_z(&mut self, angle: f64) {
        let (sin, cos) = angle.sin_cos();
        for v in &mut self.vertices {
            (v[0], v[1]) = (v[0] * cos - v[1] * sin, v[0] * sin + v[1] * cos);
        }
    }

    /// Rotates every vertex around the X axis by `angle` radians.
    pub fn rotate_x(&mut self, angle: f64) {
        let (sin, cos) = angle.sin_cos();
        for v in &mut self.vertices {
            (v[1], v[2]) = (v[1] * cos - v[2] * sin, v[1] * sin + v[2] * cos);
        }
    }

    /// Appends all triangles of `other`. Vertices are not merged, so the
    /// result consists of separate shells even where they touch.
    pub fn merge(&mut self, other: &Mesh) {
        let offset = self.vertices.len() as u32;
        self.vertices.extend(&other.vertices);
        self.triangles
            .extend(other.triangles.iter().map(|t| t.map(|i| i + offset)));
    }

    /// Reverses the winding of every triangle, turning the mesh inside out.
    pub fn flip(&mut self) {
        for t in &mut self.triangles {
            t.swap(1, 2);
        }
    }

    /// Reports problems that make slicing results unreliable.
    pub fn check(&self, diagnostics: &mut Diagnostics) {
        let (open_edges, shared_edges) = self.edge_defects();
        if shared_edges > 0 {
            diagnostics.push(
                Diagnostic::warning(
                    "mesh.non_manifold",
                    format!(
                        "{shared_edges} edges of the mesh belong to more than two triangles (duplicate or touching surfaces); affected layers may be wrong"
                    ),
                )
                .with("edges", shared_edges),
            );
        }
        if open_edges == 0 && self.signed_volume() < 0.0 {
            diagnostics.push(Diagnostic::warning(
                "mesh.inside_out",
                "all triangles of the mesh face inward; it is sliced as if they faced outward",
            ));
        }
        if open_edges > 0 {
            diagnostics.push(
                Diagnostic::warning(
                    "mesh.not_watertight",
                    format!(
                        "mesh is not watertight ({open_edges} open or inconsistently oriented edges); affected layers may be incomplete"
                    ),
                )
                .with("open_edges", open_edges),
            );
        }
    }

    /// Enclosed volume in mm³. Only meaningful for closed meshes.
    pub fn volume(&self) -> f64 {
        self.signed_volume().abs()
    }

    /// Enclosed volume in mm³: positive when triangles face outward,
    /// negative when the mesh is inside out.
    pub fn signed_volume(&self) -> f64 {
        let six_v: f64 = self
            .triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| self.vertices[i as usize]);
                a[0] * (b[1] * c[2] - b[2] * c[1])
                    + a[1] * (b[2] * c[0] - b[0] * c[2])
                    + a[2] * (b[0] * c[1] - b[1] * c[0])
            })
            .sum();
        six_v / 6.0
    }

    /// Closes holes in the surface whose rim has at most `max_edges` edges
    /// and returns how many were closed. A hole is a closed chain of edges
    /// that belong to one triangle only.
    // ponytail: each hole is filled with a fan of triangles from one rim
    // vertex. That is exact for flat convex holes (a missing facet) and only
    // roughly right for bent or concave ones; a proper triangulation of the
    // rim would be the upgrade.
    pub fn close_holes(&mut self, max_edges: usize) -> usize {
        let mut uses: HashMap<(u32, u32), usize> = HashMap::new();
        for t in &self.triangles {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                *uses.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        // The rim of a hole runs against the direction of the triangles
        // next to it. Vertices where two rims meet are left alone.
        let mut next: HashMap<u32, Option<u32>> = HashMap::new();
        for t in &self.triangles {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                if uses[&(a.min(b), a.max(b))] == 1 {
                    next.entry(b).and_modify(|n| *n = None).or_insert(Some(a));
                }
            }
        }
        let mut starts: Vec<u32> = next.keys().copied().collect();
        starts.sort_unstable();
        let mut closed = 0;
        for start in starts {
            let mut rim = vec![start];
            let complete = loop {
                let Some(Some(to)) = next.get(&rim[rim.len() - 1]).copied() else {
                    break false;
                };
                if to == start {
                    break true;
                }
                if rim.len() >= max_edges || rim.contains(&to) {
                    break false;
                }
                rim.push(to);
            };
            if !complete || rim.len() < 3 {
                continue;
            }
            for vertex in &rim {
                next.remove(vertex);
            }
            for pair in rim[1..].windows(2) {
                self.triangles.push([rim[0], pair[0], pair[1]]);
            }
            closed += 1;
        }
        closed
    }

    /// Number of triangle edges without a matching neighbour. Zero for a
    /// closed, consistently oriented mesh.
    pub fn open_edge_count(&self) -> usize {
        self.edge_defects().0
    }

    /// Counts triangle edges without a matching neighbour, and edges that
    /// more than two triangles share.
    fn edge_defects(&self) -> (usize, usize) {
        // How often each edge is used in either direction.
        let mut uses: HashMap<(u32, u32), [usize; 2]> = HashMap::new();
        for t in &self.triangles {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                if a < b {
                    uses.entry((a, b)).or_default()[0] += 1;
                } else {
                    uses.entry((b, a)).or_default()[1] += 1;
                }
            }
        }
        let open = uses.values().map(|[f, b]| f.abs_diff(*b)).sum();
        let shared = uses.values().filter(|[f, b]| f + b > 2).count();
        (open, shared)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuboid_is_closed_and_has_expected_volume() {
        let m = Mesh::cuboid(20.0, 10.0, 5.0);
        assert_eq!(m.open_edge_count(), 0);
        assert!((m.volume() - 1000.0).abs() < 1e-9);
        assert_eq!(m.bounds(), Some(([0.0; 3], [20.0, 10.0, 5.0])));
    }

    #[test]
    fn holes_are_closed() {
        let cube = Mesh::cuboid(2.0, 3.0, 4.0);

        // One missing triangle and a whole missing face.
        for missing in [1, 2] {
            let mut open = cube.clone();
            open.triangles.truncate(12 - missing);
            assert!(open.open_edge_count() > 0);
            assert_eq!(open.close_holes(64), 1);
            assert_eq!(open.edge_defects(), (0, 0));
            assert!((open.signed_volume() - 24.0).abs() < 1e-9);
            assert_eq!(open.close_holes(64), 0);
        }

        // Two holes far apart are both closed; a closed mesh is untouched.
        let mut two = cube.clone();
        two.triangles.remove(11);
        two.triangles.remove(0);
        assert_eq!(two.close_holes(64), 2);
        assert_eq!(two.edge_defects(), (0, 0));
        assert_eq!(cube.clone().close_holes(64), 0);

        // A hole with a longer rim than allowed stays open.
        let mut wide = cube.clone();
        wide.triangles.truncate(10);
        assert_eq!(wide.close_holes(3), 0);
        assert_eq!(wide.triangles.len(), 10);
    }

    #[test]
    fn check_names_the_kind_of_defect() {
        let codes = |mesh: &Mesh| {
            let mut diagnostics = Diagnostics::new();
            mesh.check(&mut diagnostics);
            diagnostics.codes()
        };
        let cube = Mesh::cuboid(1.0, 2.0, 3.0);
        assert!(codes(&cube).is_empty());

        let mut inside_out = cube.clone();
        inside_out.flip();
        assert_eq!(codes(&inside_out), ["mesh.inside_out"]);

        let mut open = cube.clone();
        open.triangles.pop();
        assert_eq!(codes(&open), ["mesh.not_watertight"]);

        // A second copy of one triangle: its edges have three users, and
        // the surplus direction has no partner.
        let mut doubled = cube.clone();
        doubled.triangles.push(cube.triangles[0]);
        assert_eq!(doubled.edge_defects(), (3, 3));
        assert_eq!(
            codes(&doubled),
            ["mesh.non_manifold", "mesh.not_watertight"]
        );

        // Two cubes sharing an edge: closed, but four triangles meet there.
        let mut touching = cube.clone();
        let mut second = cube.clone();
        second.translate([1.0, 2.0, 0.0]);
        touching.merge(&second);
        let touching = Mesh::from_triangle_soup(&touching.to_triangle_soup());
        assert_eq!(touching.edge_defects(), (0, 1));
        assert_eq!(codes(&touching), ["mesh.non_manifold"]);
    }

    #[test]
    fn soup_roundtrip_merges_vertices() {
        let m = Mesh::cuboid(1.0, 2.0, 3.0);
        let again = Mesh::from_triangle_soup(&m.to_triangle_soup());
        assert_eq!(again.vertices.len(), 8);
        assert_eq!(again.triangles.len(), 12);
        assert_eq!(again.open_edge_count(), 0);
    }

    #[test]
    fn missing_face_is_reported_as_open() {
        let mut m = Mesh::cuboid(1.0, 1.0, 1.0);
        m.triangles.truncate(10);
        assert!(m.open_edge_count() > 0);
    }
}
