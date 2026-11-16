//! Vertex data for the 3D view, built once per model, bed or slice and
//! then handed to the GPU: the model as triangles with flat normals, the
//! bed, and every toolpath segment as a coloured line.

use std::ops::Range;

use slicer_core::Mesh;
use slicer_fdm::Toolpaths;

use crate::camera::Mat4;
use crate::plate::{Model, Placement, bounds_2d};
use crate::view::path_color;

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

/// Line segments to draw, two ends each, grouped by layer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Lines {
    /// Segment ends.
    pub positions: Vec<[f32; 3]>,
    /// RGBA colour at each end.
    pub colors: Vec<[u8; 4]>,
    /// Number of vertices up to the end of each layer.
    pub layer_ends: Vec<usize>,
}

impl Lines {
    fn push(&mut self, a: [f32; 3], b: [f32; 3], color: [u8; 4]) {
        self.positions.extend([a, b]);
        self.colors.extend([color, color]);
    }

    /// Vertices of the layers below `layer`.
    pub fn below(&self, layer: usize) -> Range<usize> {
        0..self.start_of(layer)
    }

    /// Vertices of `layer`; empty past the last layer.
    pub fn layer(&self, layer: usize) -> Range<usize> {
        let start = self.start_of(layer);
        start..self.layer_ends.get(layer).copied().unwrap_or(start)
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
/// colours, as one layer.
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
    lines.layer_ends.push(lines.positions.len());
    lines
}

/// Every extrusion of every layer as line segments in the middle of the
/// layer's height, coloured by path kind.
pub fn toolpath_lines(toolpaths: &Toolpaths) -> Lines {
    let mut lines = Lines::default();
    for layer in &toolpaths.layers {
        let z = (layer.z - layer.height / 2.0) as f32;
        for path in &layer.paths {
            let [r, g, b] = path_color(path.kind);
            let color = [r, g, b, 255];
            let at = |p: [f64; 2]| [p[0] as f32, p[1] as f32, z];
            for pair in path.points.windows(2) {
                lines.push(at(pair[0]), at(pair[1]), color);
            }
            if let (true, Some(&first), Some(&last)) = (
                path.closed && path.points.len() > 2,
                path.points.first(),
                path.points.last(),
            ) {
                lines.push(at(last), at(first), color);
            }
        }
        lines.layer_ends.push(lines.positions.len());
    }
    lines
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use slicer_fdm::{Layer, Path, PathKind};
    use slicer_profiles::Settings;

    use super::*;
    use crate::camera::transform_point;
    use crate::plate::placed_outline;

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
    fn bed_grid_and_plate() {
        let lines = bed_grid([25.0, 20.0], [1, 1, 1, 255], [2, 2, 2, 255]);
        // Lines at x = 10, 20 and y = 10, plus four border edges.
        assert_eq!(lines.positions.len(), 2 * (3 + 4));
        assert_eq!(lines.layer_ends, [14]);
        assert_eq!(lines.colors[0], [1, 1, 1, 255]);
        assert_eq!(lines.colors[13], [2, 2, 2, 255]);
        let plate = bed_plate([25.0, 20.0]);
        assert_eq!(plate.positions.len(), 6);
        assert!(plate.normals.iter().all(|n| *n == [0.0, 0.0, 1.0]));
    }

    #[test]
    fn toolpaths_become_segments_per_layer() {
        let square = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
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
                    ],
                },
            ],
        };
        let lines = toolpath_lines(&toolpaths);
        // Closed square: 4 segments; open: 3; a single point: none.
        assert_eq!(lines.layer_ends, [8, 8, 14]);
        assert_eq!(lines.positions.len(), lines.colors.len());
        assert_eq!(lines.positions[0], [0.0, 0.0, 0.1]);
        assert_eq!(lines.positions[7], [0.0, 0.0, 0.1]);
        assert!((lines.positions[8][2] - 0.5).abs() < 1e-6);
        let [r, g, b] = path_color(PathKind::OuterWall);
        assert_eq!(lines.colors[0], [r, g, b, 255]);
        assert_eq!(lines.below(0), 0..0);
        assert_eq!(lines.layer(0), 0..8);
        assert_eq!(lines.layer(1), 8..8);
        assert_eq!(lines.below(2), 0..8);
        assert_eq!(lines.layer(2), 8..14);
        // Past the top everything counts as below.
        assert_eq!(lines.below(7), 0..14);
        assert_eq!(lines.layer(7), 14..14);
    }

    #[test]
    fn sliced_cube_draws_every_layer() {
        let mesh = Mesh::cuboid(10.0, 10.0, 2.0);
        let toolpaths = slicer_fdm::generate(&mesh, &Settings::default()).unwrap();
        let lines = toolpath_lines(&toolpaths);
        assert_eq!(lines.layer_ends.len(), toolpaths.layers.len());
        assert!(lines.layer_ends.windows(2).all(|w| w[0] < w[1]));
        let top = lines.positions.iter().map(|p| p[2]).fold(0.0, f32::max);
        assert!(top < 2.0 && top > 1.7, "{top}");
    }
}
