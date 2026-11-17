//! Vertex data for the 3D view, built once per model, bed or slice and
//! then handed to the GPU: the model as triangles with flat normals, the
//! bed with its grid, and every extrusion as a solid strand.

use std::ops::Range;

use slicer_core::Mesh;
use slicer_fdm::{Layer, Path, PathKind, Toolpaths};

use crate::camera::Mat4;
use crate::plate::{Model, Placement, bounds_2d};

/// Grid spacing on the bed in millimetres.
const GRID_STEP: f64 = 10.0;

/// Triangles to draw, three corners each, with the normal of the triangle
/// at every corner.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Triangles {
    /// Corner positions.
    pub positions: Vec<[f32; 3]>,
    /// Unit normal at each corner.
    pub normals: Vec<[f32; 3]>,
}

impl Triangles {
    /// Adds a triangle; one without area is left out.
    fn push(&mut self, corners: [[f32; 3]; 3]) {
        let [a, b, c] = corners;
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        // Also false for NaN.
        if length.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            return;
        }
        let n = [n[0] / length, n[1] / length, n[2] / length];
        self.positions.extend(corners);
        self.normals.extend([n; 3]);
    }
}

/// Line segments to draw, two ends each.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Lines {
    /// Segment ends.
    pub positions: Vec<[f32; 3]>,
    /// RGBA colour at each end.
    pub colors: Vec<[u8; 4]>,
}

impl Lines {
    fn push(&mut self, a: [f32; 3], b: [f32; 3], color: [u8; 4]) {
        self.positions.extend([a, b]);
        self.colors.extend([color, color]);
    }
}

/// Indices per strand segment: four sides of two triangles.
pub const INDICES_PER_SEGMENT: usize = 24;

/// Extrusions as solid strands, grouped by layer: every segment is a prism
/// with a diamond cross-section as wide as its line and as high as its
/// layer. The normals at the four corners point outwards, so the strand
/// looks round when lit.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Strands {
    /// Corner positions, eight per segment.
    pub positions: Vec<[f32; 3]>,
    /// Unit normal at each corner.
    pub normals: Vec<[f32; 3]>,
    /// RGBA colour at each corner.
    pub colors: Vec<[u8; 4]>,
    /// Triangles as corner indices.
    pub indices: Vec<u32>,
    /// Number of indices up to the end of each layer.
    pub layer_ends: Vec<usize>,
}

impl Strands {
    /// Indices of the layers below `layer`.
    pub fn below(&self, layer: usize) -> Range<usize> {
        0..self.start_of(layer)
    }

    /// Indices of `layer`; empty past the last layer.
    pub fn layer(&self, layer: usize) -> Range<usize> {
        let start = self.start_of(layer);
        start..self.layer_ends.get(layer).copied().unwrap_or(start)
    }

    /// Number of segments in every layer.
    pub fn segments_per_layer(&self) -> Vec<usize> {
        (0..self.layer_ends.len())
            .map(|layer| self.layer(layer).len() / INDICES_PER_SEGMENT)
            .collect()
    }

    fn start_of(&self, layer: usize) -> usize {
        match layer.checked_sub(1) {
            None => 0,
            Some(previous) => self
                .layer_ends
                .get(previous)
                .or(self.layer_ends.last())
                .copied()
                .unwrap_or(0),
        }
    }

    /// Adds the segment from `a` to `b` with its centre at height `z`;
    /// one without length is left out.
    fn push(
        &mut self,
        a: [f64; 2],
        b: [f64; 2],
        z: f64,
        [width, height]: [f64; 2],
        color: [u8; 4],
    ) {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let length = (dx * dx + dy * dy).sqrt();
        // Also false for NaN.
        if length.partial_cmp(&1e-9) != Some(std::cmp::Ordering::Greater) {
            return;
        }
        let side = [-dy / length, dx / length];
        let (w, h) = (width / 2.0, height / 2.0);
        let Ok(base) = u32::try_from(self.positions.len()) else {
            return;
        };
        for p in [a, b] {
            for (offset, normal) in [
                ([side[0] * w, side[1] * w, 0.0], [side[0], side[1], 0.0]),
                ([0.0, 0.0, h], [0.0, 0.0, 1.0]),
                ([-side[0] * w, -side[1] * w, 0.0], [-side[0], -side[1], 0.0]),
                ([0.0, 0.0, -h], [0.0, 0.0, -1.0]),
            ] {
                self.positions.push([
                    (p[0] + offset[0]) as f32,
                    (p[1] + offset[1]) as f32,
                    (z + offset[2]) as f32,
                ]);
                self.normals.push(normal.map(|v| v as f32));
                self.colors.push(color);
            }
        }
        for i in 0..4 {
            let j = (i + 1) % 4;
            self.indices.extend([
                base + i,
                base + j,
                base + 4 + j,
                base + i,
                base + 4 + j,
                base + 4 + i,
            ]);
        }
    }
}

/// The triangles of a mesh in its own coordinates. Triangles without area
/// are left out; their normal is undefined.
pub fn mesh_triangles(mesh: &Mesh) -> Triangles {
    let mut triangles = Triangles::default();
    let corner = |i: u32| {
        let v = mesh.vertices.get(i as usize).copied().unwrap_or_default();
        [v[0] as f32, v[1] as f32, v[2] as f32]
    };
    for t in &mesh.triangles {
        triangles.push([corner(t[0]), corner(t[1]), corner(t[2])]);
    }
    triangles
}

/// Maps the model's own coordinates to the bed for a placement: scaled,
/// turned around Z, and moved so that the middle of its outline is at the
/// placement's centre, as [`crate::plate::placed_outline`] shows it.
pub fn model_matrix(model: &Model, placement: &Placement) -> Mat4 {
    let (sin, cos) = placement.rotation_deg.to_radians().sin_cos();
    let s = placement.scale;
    let turn = |p: [f64; 2]| [(p[0] * cos - p[1] * sin) * s, (p[0] * sin + p[1] * cos) * s];
    let (lo, hi) = bounds_2d(model.hull.iter().map(|&p| turn(p))).unwrap_or_default();
    let shift = [
        placement.center[0] - (lo[0] + hi[0]) / 2.0,
        placement.center[1] - (lo[1] + hi[1]) / 2.0,
    ];
    [
        (cos * s) as f32,
        (sin * s) as f32,
        0.0,
        0.0,
        (-sin * s) as f32,
        (cos * s) as f32,
        0.0,
        0.0,
        0.0,
        0.0,
        s as f32,
        0.0,
        shift[0] as f32,
        shift[1] as f32,
        0.0,
        1.0,
    ]
}

/// The first model that a ray from `origin` in direction `dir` hits, and
/// how far along the ray: `models` are each model's matrix (see
/// [`model_matrix`]) and mesh.
pub fn pick(models: &[(Mat4, &Mesh)], origin: [f32; 3], dir: [f32; 3]) -> Option<(usize, f32)> {
    let mut best: Option<(usize, f32)> = None;
    for (index, (matrix, mesh)) in models.iter().enumerate() {
        let corners: Vec<[f32; 3]> = mesh
            .vertices
            .iter()
            .map(|v| {
                let p = [v[0] as f32, v[1] as f32, v[2] as f32];
                let row = |r: usize| {
                    matrix[r] * p[0] + matrix[4 + r] * p[1] + matrix[8 + r] * p[2] + matrix[12 + r]
                };
                [row(0), row(1), row(2)]
            })
            .collect();
        for t in &mesh.triangles {
            let [Some(&a), Some(&b), Some(&c)] = t.map(|i| corners.get(i as usize)) else {
                continue;
            };
            if let Some(distance) = ray_triangle(origin, dir, [a, b, c])
                && best.is_none_or(|(_, d)| distance < d)
            {
                best = Some((index, distance));
            }
        }
    }
    best
}

/// Distance along the ray to the triangle, if the ray hits it from either
/// side (Möller and Trumbore 1997).
fn ray_triangle(origin: [f32; 3], dir: [f32; 3], [a, b, c]: [[f32; 3]; 3]) -> Option<f32> {
    let sub = |p: [f32; 3], q: [f32; 3]| [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
    let cross = |p: [f32; 3], q: [f32; 3]| {
        [
            p[1] * q[2] - p[2] * q[1],
            p[2] * q[0] - p[0] * q[2],
            p[0] * q[1] - p[1] * q[0],
        ]
    };
    let dot = |p: [f32; 3], q: [f32; 3]| p[0] * q[0] + p[1] * q[1] + p[2] * q[2];
    let (e1, e2) = (sub(b, a), sub(c, a));
    let p = cross(dir, e2);
    let det = dot(e1, p);
    if det.abs() < 1e-12 {
        return None;
    }
    let s = sub(origin, a);
    let u = dot(s, p) / det;
    let q = cross(s, e1);
    let v = dot(dir, q) / det;
    let t = dot(e2, q) / det;
    (u >= 0.0 && v >= 0.0 && u + v <= 1.0 && t > 0.0).then_some(t)
}

/// The bed surface, just below Z = 0 so that the grid and the first
/// layer stay visible on it.
pub fn bed_plate(bed: [f64; 2]) -> Triangles {
    let (x, y, z) = (bed[0] as f32, bed[1] as f32, -0.05);
    let mut triangles = Triangles::default();
    triangles.push([[0.0, 0.0, z], [x, 0.0, z], [x, y, z]]);
    triangles.push([[0.0, 0.0, z], [x, y, z], [0.0, y, z]]);
    triangles
}

/// A grid on the bed every 10 mm and its border, in `grid` and `border`
/// colours.
pub fn bed_grid(bed: [f64; 2], grid: [u8; 4], border: [u8; 4]) -> Lines {
    let mut lines = Lines::default();
    let (x, y) = (bed[0] as f32, bed[1] as f32);
    for (axis, size) in bed.into_iter().enumerate() {
        let steps = (size / GRID_STEP).ceil() as usize;
        for step in 1..steps {
            let at = (step as f64 * GRID_STEP) as f32;
            let (a, b) = if axis == 0 {
                ([at, 0.0, 0.0], [at, y, 0.0])
            } else {
                ([0.0, at, 0.0], [x, at, 0.0])
            };
            lines.push(a, b, grid);
        }
    }
    let corners = [[0.0, 0.0, 0.0], [x, 0.0, 0.0], [x, y, 0.0], [0.0, y, 0.0]];
    for i in 0..4 {
        lines.push(corners[i], corners[(i + 1) % 4], border);
    }
    lines
}

/// Every extrusion of every layer as a strand, leaving out the kinds in
/// `hidden`; `color` gives each path's colour from the path, the number of
/// its layer and the layer. Lines are `line_width` wide unless the path has
/// its own width, and as high as their layer.
pub fn toolpath_strands(
    toolpaths: &Toolpaths,
    line_width: f64,
    hidden: &[PathKind],
    color: impl Fn(&Path, usize, &Layer) -> [u8; 3],
) -> Strands {
    let mut strands = Strands::default();
    for (index, layer) in toolpaths.layers.iter().enumerate() {
        let z = layer.z - layer.height / 2.0;
        for path in layer.paths.iter().filter(|p| !hidden.contains(&p.kind)) {
            let [r, g, b] = color(path, index, layer);
            let size = [path.width.unwrap_or(line_width), layer.height];
            for pair in path.points.windows(2) {
                strands.push(pair[0], pair[1], z, size, [r, g, b, 255]);
            }
            if let (true, Some(&first), Some(&last)) = (
                path.closed && path.points.len() > 2,
                path.points.first(),
                path.points.last(),
            ) {
                strands.push(last, first, z, size, [r, g, b, 255]);
            }
        }
        strands.layer_ends.push(strands.indices.len());
    }
    strands
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use slicer_fdm::{Layer, Path, PathKind};
    use slicer_profiles::Settings;

    use super::*;
    use crate::camera::transform_point;
    use crate::plate::placed_outline;
    use crate::view::path_color;

    fn path(kind: PathKind, points: Vec<[f64; 2]>, closed: bool) -> Path {
        Path {
            kind,
            points,
            closed,
            travel_inside: false,
            extruder: 0,
            width: None,
        }
    }

    #[test]
    fn mesh_has_flat_normals_and_skips_slivers() {
        let mut mesh = Mesh::cuboid(10.0, 20.0, 5.0);
        mesh.triangles.push([0, 0, 1]);
        let triangles = mesh_triangles(&mesh);
        assert_eq!(triangles.positions.len(), 36);
        assert_eq!(triangles.normals.len(), 36);
        for (corners, normals) in triangles
            .positions
            .chunks(3)
            .zip(triangles.normals.chunks(3))
        {
            let n = normals[0];
            assert!(normals.iter().all(|&m| m == n));
            assert!((n[0] * n[0] + n[1] * n[1] + n[2] * n[2] - 1.0).abs() < 1e-6);
            // The cuboid's faces are axis-aligned and point outwards.
            let axis = (0..3).find(|&a| n[a].abs() > 0.99).unwrap();
            let centre = corners.iter().map(|c| c[axis]).sum::<f32>() / 3.0;
            let (lo, hi) = (
                mesh.bounds().unwrap().0[axis],
                mesh.bounds().unwrap().1[axis],
            );
            let mid = ((lo + hi) / 2.0) as f32;
            assert_eq!(n[axis] > 0.0, centre > mid);
        }
    }

    #[test]
    fn model_matrix_matches_the_outline() {
        let mut mesh = Mesh::cuboid(20.0, 10.0, 5.0);
        mesh.translate([3.0, 4.0, 0.0]);
        let model = Model::from_mesh("cube", mesh).unwrap();
        let placement = Placement {
            center: [70.0, 40.0],
            rotation_deg: 30.0,
            scale: 1.5,
        };
        let m = model_matrix(&model, &placement);
        let outline = placed_outline(&model, &placement);
        for (corner, expected) in model.hull.iter().zip(&outline) {
            let p = transform_point(&m, [corner[0] as f32, corner[1] as f32, 2.0]).unwrap();
            assert!((p[0] - expected[0] as f32).abs() < 1e-3);
            assert!((p[1] - expected[1] as f32).abs() < 1e-3);
            assert!((p[2] - 3.0).abs() < 1e-6);
        }
    }

    #[test]
    fn rays_pick_the_nearest_model() {
        let model = Model::from_mesh("cube", Mesh::cuboid(10.0, 10.0, 10.0)).unwrap();
        let place = |x: f64| Placement {
            center: [x, 50.0],
            rotation_deg: 0.0,
            scale: 1.0,
        };
        let (left, right) = (
            model_matrix(&model, &place(30.0)),
            model_matrix(&model, &place(70.0)),
        );
        let models = [(left, &model.mesh), (right, &model.mesh)];
        // Straight down onto the right cube: hits its top at Z = 10.
        let (index, distance) = pick(&models, [70.0, 50.0, 100.0], [0.0, 0.0, -1.0]).unwrap();
        assert_eq!(index, 1);
        assert!((distance - 90.0).abs() < 1e-3);
        // Along X through both: the left one is first.
        let (index, _) = pick(&models, [0.0, 50.0, 5.0], [1.0, 0.0, 0.0]).unwrap();
        assert_eq!(index, 0);
        // Between them nothing is hit.
        assert!(pick(&models, [50.0, 50.0, 100.0], [0.0, 0.0, -1.0]).is_none());
    }

    #[test]
    fn bed_grid_and_plate() {
        let lines = bed_grid([25.0, 20.0], [1, 1, 1, 255], [2, 2, 2, 255]);
        // Lines at x = 10, 20 and y = 10, plus four border edges.
        assert_eq!(lines.positions.len(), 2 * (3 + 4));
        assert_eq!(lines.colors[0], [1, 1, 1, 255]);
        assert_eq!(lines.colors[13], [2, 2, 2, 255]);
        let plate = bed_plate([25.0, 20.0]);
        assert_eq!(plate.positions.len(), 6);
        assert!(plate.normals.iter().all(|n| *n == [0.0, 0.0, 1.0]));
    }

    #[test]
    fn toolpaths_become_strands_per_layer() {
        let square = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let mut thin = path(PathKind::GapFill, vec![[0.0, 0.0], [2.0, 0.0]], false);
        thin.width = Some(0.2);
        let toolpaths = Toolpaths {
            layers: vec![
                Layer {
                    z: 0.2,
                    height: 0.2,
                    paths: vec![path(PathKind::OuterWall, square.clone(), true)],
                },
                Layer {
                    z: 0.4,
                    height: 0.2,
                    paths: vec![],
                },
                Layer {
                    z: 0.6,
                    height: 0.2,
                    paths: vec![
                        path(PathKind::SparseInfill, square, false),
                        path(PathKind::Bridge, vec![[5.0, 5.0]], false),
                        thin,
                    ],
                },
            ],
        };
        let by_kind = |path: &Path, _: usize, _: &Layer| path_color(path.kind);
        let strands = toolpath_strands(&toolpaths, 0.4, &[], by_kind);
        let n = INDICES_PER_SEGMENT;
        // Closed square: 4 segments; open: 3; a single point: none; thin: 1.
        assert_eq!(strands.layer_ends, [4 * n, 4 * n, 8 * n]);
        assert_eq!(strands.segments_per_layer(), [4, 0, 4]);
        assert_eq!(strands.positions.len(), 8 * 8);
        assert_eq!(strands.normals.len(), strands.positions.len());
        assert_eq!(strands.colors.len(), strands.positions.len());
        // The first segment runs along +X at the middle of layer 0: its
        // corners lie 0.2 to the side and 0.1 above and below.
        assert_eq!(strands.positions[0], [0.0, 0.2, 0.1]);
        assert_eq!(strands.positions[1], [0.0, 0.0, 0.2]);
        assert_eq!(strands.positions[2], [0.0, -0.2, 0.1]);
        assert!((strands.positions[3][2]).abs() < 1e-6);
        assert_eq!(strands.normals[0], [0.0, 1.0, 0.0]);
        // The thin line keeps its own width.
        let last = strands.positions.len() - 8;
        assert!((strands.positions[last][1] - 0.1).abs() < 1e-6);
        let [r, g, b] = path_color(PathKind::OuterWall);
        assert_eq!(strands.colors[0], [r, g, b, 255]);
        assert!(
            strands
                .indices
                .iter()
                .all(|&i| (i as usize) < strands.positions.len())
        );
        assert_eq!(strands.below(0), 0..0);
        assert_eq!(strands.layer(0), 0..4 * n);
        assert_eq!(strands.layer(1), 4 * n..4 * n);
        assert_eq!(strands.below(2), 0..4 * n);
        // Past the top everything counts as below.
        assert_eq!(strands.below(7), 0..8 * n);
        assert_eq!(strands.layer(7), 8 * n..8 * n);
        // Hidden kinds are left out.
        let walls_only = toolpath_strands(
            &toolpaths,
            0.4,
            &[PathKind::SparseInfill, PathKind::GapFill],
            by_kind,
        );
        assert_eq!(walls_only.segments_per_layer(), [4, 0, 0]);
    }

    #[test]
    fn sliced_cube_draws_every_layer() {
        let mesh = Mesh::cuboid(10.0, 10.0, 2.0);
        let toolpaths = slicer_fdm::generate(&mesh, &Settings::default()).unwrap();
        let strands = toolpath_strands(&toolpaths, 0.45, &[], |path, _, _| path_color(path.kind));
        assert_eq!(strands.layer_ends.len(), toolpaths.layers.len());
        assert!(strands.layer_ends.windows(2).all(|w| w[0] < w[1]));
        // The top of the strands is the top of the last layer.
        let top = strands.positions.iter().map(|p| p[2]).fold(0.0, f32::max);
        assert!((top - 2.0).abs() < 1e-4, "{top}");
    }
}
