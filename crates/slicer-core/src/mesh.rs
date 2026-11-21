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
    /// Extruder that prints each triangle, counted from 0. Empty while every
    /// triangle belongs to extruder 0, otherwise one entry per triangle.
    /// Read it through [`Mesh::extruder`], which tolerates both.
    pub extruders: Vec<u32>,
}

impl Mesh {
    /// Extruder that prints triangle `index`.
    pub fn extruder(&self, index: usize) -> u32 {
        self.extruders.get(index).copied().unwrap_or(0)
    }

    /// Gives the triangles from `first` to the last one to `extruder`.
    pub fn set_extruder(&mut self, first: usize, extruder: u32) {
        if extruder == 0 && self.extruders.is_empty() {
            return;
        }
        self.extruders.resize(self.triangles.len(), 0);
        if let Some(tail) = self.extruders.get_mut(first..) {
            tail.fill(extruder);
        }
    }

    /// The extruders the triangles ask for, ascending and without repeats.
    /// Empty for a mesh without triangles.
    pub fn used_extruders(&self) -> Vec<u32> {
        // Triangles beyond the end of `extruders` belong to extruder 0.
        let tail = (self.extruders.len() < self.triangles.len()).then_some(0);
        let used: std::collections::BTreeSet<u32> = self
            .extruders
            .iter()
            .take(self.triangles.len())
            .copied()
            .chain(tail)
            .collect();
        used.into_iter().collect()
    }

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
            extruders: Vec::new(),
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

    /// Appends all triangles of `other` with their extruders. Vertices are not
    /// merged, so the result consists of separate shells even where they
    /// touch.
    pub fn merge(&mut self, other: &Mesh) {
        let offset = self.vertices.len() as u32;
        let first = self.triangles.len();
        self.vertices.extend(&other.vertices);
        self.triangles
            .extend(other.triangles.iter().map(|t| t.map(|i| i + offset)));
        if !self.extruders.is_empty() || !other.extruders.is_empty() {
            self.extruders.resize(first, 0);
            self.extruders
                .extend((0..other.triangles.len()).map(|i| other.extruder(i)));
        }
    }

    /// Reverses the winding of every triangle, turning the mesh inside out.
    pub fn flip(&mut self) {
        for t in &mut self.triangles {
            t.swap(1, 2);
        }
    }

    /// Turns triangles around until neighbours agree on which side is
    /// outside, and returns how many were turned. In every connected piece
    /// of the surface the smaller part gives way to the larger one. Edges
    /// with more or fewer than two triangles do not connect, and neither do
    /// two copies of one triangle: at the rim of a hole they share an edge
    /// with nothing else, and turning one of them would make the pair look
    /// like two faces back to back.
    pub fn orient(&mut self) -> usize {
        let mut at: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
        for (i, t) in self.triangles.iter().enumerate() {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                at.entry((a.min(b), a.max(b))).or_default().push(i);
            }
        }
        // 0: not seen, 1: as the first triangle of its piece, 2: the other way.
        let mut side = vec![0u8; self.triangles.len()];
        let mut turned = 0;
        for start in 0..self.triangles.len() {
            if side[start] != 0 {
                continue;
            }
            side[start] = 1;
            let mut piece = vec![start];
            let mut next = 0;
            while next < piece.len() {
                let i = piece[next];
                next += 1;
                let t = self.triangles[i];
                for k in 0..3 {
                    let (a, b) = (t[k], t[(k + 1) % 3]);
                    let pair = &at[&(a.min(b), a.max(b))];
                    if pair.len() != 2 {
                        continue;
                    }
                    let j = if pair[0] == i { pair[1] } else { pair[0] };
                    if side[j] != 0 {
                        continue;
                    }
                    // Neighbours that agree run along their shared edge in
                    // opposite directions.
                    let u = self.triangles[j];
                    if t.iter().all(|corner| u.contains(corner)) {
                        continue;
                    }
                    let same = (0..3).any(|m| u[m] == a && u[(m + 1) % 3] == b);
                    side[j] = if same { 3 - side[i] } else { side[i] };
                    piece.push(j);
                }
            }
            let others = piece.iter().filter(|&&i| side[i] == 2).count();
            let wrong = if 2 * others > piece.len() { 1 } else { 2 };
            for &i in &piece {
                if side[i] == wrong {
                    self.triangles[i].swap(1, 2);
                    turned += 1;
                }
            }
        }
        turned
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

    /// Enclosed volume in mm³. Only meaningful for closed meshes. Shells
    /// that overlap count twice where they overlap, although the slicer
    /// prints that space once.
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

    /// Removes triangles without area and returns how many were removed. A
    /// triangle counts as degenerate when one corner lies within `tolerance`
    /// (mm) of the line through the other two. The surface stays closed:
    /// an edge not longer than `tolerance` is collapsed into its
    /// lower-numbered end, which removes the triangles on both sides of it;
    /// a triangle with a corner on its longest edge is removed by splitting
    /// the neighbour across that edge at that corner (an edge flip). A
    /// degenerate triangle on an open or shared edge, or one whose flip
    /// would double an existing edge, is left in place.
    // ponytail: chains of short edges collapse into one vertex, which can
    // move it by more than `tolerance`; flips run for at most 8 passes.
    pub fn remove_degenerate(&mut self, tolerance: f64) -> usize {
        let tol2 = tolerance * tolerance;
        let pos = |v: &[[f64; 3]], i: u32| v[i as usize];

        // Short edges: merge their ends, then drop what lost its area.
        let mut root: Vec<u32> = (0..self.vertices.len() as u32).collect();
        let find = |root: &mut Vec<u32>, mut v: u32| {
            while root[v as usize] != v {
                let up = root[root[v as usize] as usize];
                root[v as usize] = up;
                v = up;
            }
            v
        };
        for t in &self.triangles {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                let d = sub(pos(&self.vertices, a), pos(&self.vertices, b));
                if dot(d, d) <= tol2 {
                    let (ra, rb) = (find(&mut root, a), find(&mut root, b));
                    root[ra.max(rb) as usize] = ra.min(rb);
                }
            }
        }
        let before = self.triangles.len();
        for t in &mut self.triangles {
            *t = t.map(|v| find(&mut root, v));
        }
        let keep = |t: &[u32; 3]| t[0] != t[1] && t[1] != t[2] && t[0] != t[2];
        if !self.extruders.is_empty() {
            // The extruders go with their triangles.
            self.extruders.resize(before, 0);
            let mut index = 0;
            let triangles = &self.triangles;
            self.extruders.retain(|_| {
                index += 1;
                keep(&triangles[index - 1])
            });
        }
        self.triangles.retain(keep);
        let mut removed = before - self.triangles.len();

        // Corners on the opposite edge: flip that edge.
        let key = |a: u32, b: u32| (a.min(b), a.max(b));
        let mut at: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
        for (i, t) in self.triangles.iter().enumerate() {
            for k in 0..3 {
                at.entry(key(t[k], t[(k + 1) % 3])).or_default().push(i);
            }
        }
        for _ in 0..8 {
            let mut flipped = false;
            for i in 0..self.triangles.len() {
                let t = self.triangles[i];
                let [a, b, c] = t.map(|v| pos(&self.vertices, v));
                let lengths = [sub(b, a), sub(c, b), sub(a, c)].map(|d| dot(d, d));
                let k = (0..3).fold(0, |m, k| if lengths[k] > lengths[m] { k } else { m });
                let area = cross(sub(b, a), sub(c, a));
                if dot(area, area) > tol2 * lengths[k] {
                    continue;
                }
                // t runs p → q → r with p → q its longest edge; the
                // neighbour runs q → p → s.
                let (p, q, r) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
                let users = &at[&key(p, q)];
                if users.len() != 2 {
                    continue;
                }
                let j = if users[0] == i { users[1] } else { users[0] };
                let u = self.triangles[j];
                let Some(m) = (0..3).find(|&m| u[m] == q && u[(m + 1) % 3] == p) else {
                    continue;
                };
                let s = u[(m + 2) % 3];
                if s == r || at.get(&key(r, s)).is_some_and(|e| !e.is_empty()) {
                    continue;
                }
                for (n, old) in [(i, t), (j, u)] {
                    for k in 0..3 {
                        if let Some(e) = at.get_mut(&key(old[k], old[(k + 1) % 3])) {
                            e.retain(|&x| x != n);
                        }
                    }
                }
                self.triangles[i] = [s, q, r];
                self.triangles[j] = [p, s, r];
                for n in [i, j] {
                    let t = self.triangles[n];
                    for k in 0..3 {
                        at.entry(key(t[k], t[(k + 1) % 3])).or_default().push(n);
                    }
                }
                removed += 1;
                flipped = true;
            }
            if !flipped {
                break;
            }
        }
        removed
    }

    /// Number of triangle edges without a matching neighbour. Zero for a
    /// closed, consistently oriented mesh.
    pub fn open_edge_count(&self) -> usize {
        self.edge_defects().0
    }

    /// Counts triangle edges without a matching neighbour, and edges that
    /// more than two triangles share.
    pub(crate) fn edge_defects(&self) -> (usize, usize) {
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

pub(crate) fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(crate) fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(crate) fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
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
    fn turned_triangles_are_turned_back() {
        let cube = Mesh::cuboid(2.0, 3.0, 4.0);
        let mut mesh = cube.clone();
        assert_eq!(mesh.orient(), 0);
        for i in [0, 5, 6] {
            mesh.triangles[i].swap(1, 2);
        }
        assert!(mesh.open_edge_count() > 0);
        assert_eq!(mesh.orient(), 3);
        assert_eq!(mesh, cube);
        // A mesh that is inside out as a whole agrees with itself.
        mesh.flip();
        assert_eq!(mesh.orient(), 0);
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
    fn concave_holes_are_filled_without_folds() {
        use crate::primitives::{extrude, star_contour};
        let l_shape = [
            [0.0, 0.0],
            [20.0, 0.0],
            [20.0, 8.0],
            [8.0, 8.0],
            [8.0, 20.0],
            [0.0, 20.0],
        ];
        for (contour, center) in [
            (star_contour(5, 10.0, 4.0), [0.0, 0.0]),
            (l_shape.to_vec(), [4.0, 4.0]),
        ] {
            let height = 3.0;
            let mut mesh = extrude(&contour, center, height, 1, 0.0);
            // Without its top cap, the solid has a hole shaped like the contour.
            let top = mesh
                .vertices
                .iter()
                .position(|v| *v == [center[0], center[1], height]);
            let Some(top) = top.map(|i| i as u32) else {
                panic!("no centre on the top cap");
            };
            mesh.triangles.retain(|t| !t.contains(&top));
            let kept = mesh.triangles.len();
            assert_eq!(mesh.close_holes(64), 1);
            assert_eq!(mesh.edge_defects(), (0, 0));
            let expected = crate::polygon::signed_area(&contour) * height;
            assert!((mesh.signed_volume() - expected).abs() < 1e-9 * expected);
            // A fan from a corner would also give the right volume, but with
            // some triangles upside down, covering the hole twice in places.
            for t in &mesh.triangles[kept..] {
                let [a, b, c] = t.map(|i| mesh.vertices[i as usize]);
                assert!(cross(sub(b, a), sub(c, a))[2] > 0.0, "{t:?} faces down");
            }
        }
    }

    #[test]
    fn degenerate_triangles_are_removed_without_opening_the_surface() {
        let cube = Mesh::cuboid(2.0, 3.0, 4.0);
        assert_eq!(cube.clone().remove_degenerate(1e-6), 0);

        // A corner on the edge between two bottom triangles and a side
        // triangle, joined by a triangle without area.
        let mut cap = cube.clone();
        cap.vertices.push([1.0, 0.0, 0.0]); // 8, middle of edge 0-1
        cap.triangles[0] = [0, 2, 8];
        cap.triangles.push([8, 2, 1]);
        cap.triangles.push([0, 8, 1]);
        assert_eq!(cap.edge_defects(), (0, 0));
        assert_eq!(cap.remove_degenerate(1e-6), 1);
        assert_eq!(cap.edge_defects(), (0, 0));
        // The side triangle is split in two where the flat one was.
        assert_eq!(cap.triangles.len(), 14);
        assert!((cap.signed_volume() - 24.0).abs() < 1e-12);
        assert!(cap.triangles.iter().all(|t| {
            let [a, b, c] = t.map(|i| cap.vertices[i as usize]);
            let n = cross(sub(b, a), sub(c, a));
            dot(n, n) > 1e-12
        }));
        assert_eq!(cap.remove_degenerate(1e-6), 0);

        // A top corner split in two a nanometre apart, stitched by two
        // needles: the short edge is collapsed and both needles go.
        let mut needle = cube.clone();
        needle.vertices.push([2.0 + 1e-9, 3.0, 4.0]); // 8, next to 6
        needle.triangles[3] = [4, 8, 7];
        needle.triangles.push([4, 6, 8]);
        needle.triangles.push([8, 6, 7]);
        assert_eq!(needle.edge_defects(), (0, 0));
        assert_eq!(needle.remove_degenerate(1e-6), 2);
        assert_eq!(needle.triangles, cube.triangles);
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
    fn extruders_follow_their_triangles() {
        let cube = Mesh::cuboid(1.0, 1.0, 1.0);
        assert_eq!(cube.used_extruders(), [0]);
        assert_eq!(Mesh::default().used_extruders(), Vec::<u32>::new());

        // A second object on extruder 2, with a hole that gets closed.
        let mut other = cube.clone();
        other.translate([5.0, 0.0, 0.0]);
        other.triangles.truncate(10);
        other.set_extruder(0, 2);
        let mut mesh = cube.clone();
        mesh.merge(&other);
        assert_eq!(mesh.used_extruders(), [0, 2]);
        assert_eq!(mesh.close_holes(64), 1);
        assert_eq!(mesh.extruders.len(), mesh.triangles.len());
        let counts = |e: u32| mesh.extruders.iter().filter(|&&x| x == e).count();
        assert_eq!((counts(0), counts(2)), (12, 12));

        // Extruder 0 alone allocates nothing.
        let mut plain = cube.clone();
        plain.set_extruder(0, 0);
        plain.merge(&cube);
        assert!(plain.extruders.is_empty());
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
