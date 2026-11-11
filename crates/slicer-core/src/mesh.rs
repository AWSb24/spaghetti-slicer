//! Indexed triangle meshes.

use std::collections::HashMap;

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

    /// Enclosed volume in mm³. Only meaningful for closed meshes.
    pub fn volume(&self) -> f64 {
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
        (six_v / 6.0).abs()
    }

    /// Number of triangle edges without a matching neighbour. Zero for a
    /// closed, consistently oriented mesh.
    pub fn open_edge_count(&self) -> usize {
        let mut balance: HashMap<(u32, u32), i32> = HashMap::new();
        for t in &self.triangles {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                if a < b {
                    *balance.entry((a, b)).or_insert(0) += 1;
                } else {
                    *balance.entry((b, a)).or_insert(0) -= 1;
                }
            }
        }
        balance.values().map(|v| v.unsigned_abs() as usize).sum()
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
