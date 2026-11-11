//! FDM toolpath generation: perimeters, infill, supports and path planning.
//!
//! Supports are not implemented yet.

pub mod preview;

use std::fmt;

use slicer_core::polygon::{self, HatchDirection, distance};
use slicer_core::slice::slice_mesh;
use slicer_core::{Contour, Mesh, Point, Shapes};
use slicer_profiles::{InfillPattern, Settings};

/// What a path is for. Determines speed and the label in the G-code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    /// Loop around the model on the first layer, not touching it.
    Skirt,
    /// Loops attached to the first layer to improve adhesion.
    Brim,
    /// The visible outermost wall.
    OuterWall,
    /// Walls behind the outer wall.
    InnerWall,
    /// Dense fill of top and bottom surfaces.
    SolidInfill,
    /// Low-density fill of the interior.
    SparseInfill,
}

/// A continuous extrusion.
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    /// Purpose of the path.
    pub kind: PathKind,
    /// Points to visit, in order.
    pub points: Vec<Point>,
    /// Whether the path returns to its first point after the last one.
    pub closed: bool,
}

impl Path {
    /// Extruded length in millimetres.
    pub fn length(&self) -> f64 {
        let open: f64 = self.points.windows(2).map(|w| distance(w[0], w[1])).sum();
        match (self.closed, self.points.first(), self.points.last()) {
            (true, Some(&first), Some(&last)) => open + distance(last, first),
            _ => open,
        }
    }
}

/// All paths of one layer, in printing order.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    /// Height of the nozzle above the bed while printing this layer.
    pub z: f64,
    /// Thickness of this layer.
    pub height: f64,
    /// Paths in printing order.
    pub paths: Vec<Path>,
}

/// The complete print, layer by layer from the bed up.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Toolpaths {
    /// Layers from bottom to top.
    pub layers: Vec<Layer>,
}

/// Why toolpaths could not be generated.
#[derive(Debug, Clone, PartialEq)]
pub enum FdmError {
    /// The mesh has no triangles.
    EmptyModel,
    /// The model does not fit the build volume.
    TooLarge {
        /// Model size in X, Y, Z.
        model: [f64; 3],
        /// Build volume in X, Y, Z.
        volume: [f64; 3],
    },
    /// The model is lower than the first layer, or slicing produced no
    /// printable area.
    NothingToPrint,
}

impl fmt::Display for FdmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyModel => write!(f, "the model contains no geometry"),
            Self::TooLarge { model, volume } => write!(
                f,
                "model ({:.1} x {:.1} x {:.1} mm) does not fit the build volume ({:.1} x {:.1} x {:.1} mm)",
                model[0], model[1], model[2], volume[0], volume[1], volume[2]
            ),
            Self::NothingToPrint => write!(
                f,
                "nothing to print: the model is lower than the first layer or has no printable area"
            ),
        }
    }
}

impl std::error::Error for FdmError {}

/// Slices `mesh` and generates toolpaths. The model is centred on the bed
/// and placed on it.
pub fn generate(mesh: &Mesh, settings: &Settings) -> Result<Toolpaths, FdmError> {
    let (min, max) = mesh.bounds().ok_or(FdmError::EmptyModel)?;
    let size = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
    let volume = [
        settings.bed_size_x,
        settings.bed_size_y,
        settings.max_print_height,
    ];
    if (0..3).any(|a| size[a] > volume[a] + 1e-6) {
        return Err(FdmError::TooLarge {
            model: size,
            volume,
        });
    }
    let mut mesh = mesh.clone();
    mesh.translate([
        volume[0] / 2.0 - (min[0] + max[0]) / 2.0,
        volume[1] / 2.0 - (min[1] + max[1]) / 2.0,
        -min[2],
    ]);

    // Layer i ends at first_layer_height + i * layer_height and is cut at
    // its mid-height.
    let mut tops = Vec::new();
    loop {
        let top = settings.first_layer_height + tops.len() as f64 * settings.layer_height;
        if top > size[2] + 1e-6 {
            break;
        }
        tops.push(top);
    }
    let heights: Vec<f64> = (0..tops.len())
        .map(|i| {
            if i == 0 {
                settings.first_layer_height
            } else {
                settings.layer_height
            }
        })
        .collect();
    let cut_heights: Vec<f64> = tops
        .iter()
        .zip(&heights)
        .map(|(top, h)| top - h / 2.0)
        .collect();
    let slices = slice_mesh(&mesh, &cut_heights);
    if slices.iter().all(Vec::is_empty) {
        return Err(FdmError::NothingToPrint);
    }

    let mut position: Point = [0.0, 0.0];
    let layers = (0..slices.len())
        .map(|index| Layer {
            z: tops[index],
            height: heights[index],
            paths: layer_paths(&slices, index, settings, &mut position),
        })
        .collect();
    Ok(Toolpaths { layers })
}

fn layer_paths(
    slices: &[Shapes],
    index: usize,
    settings: &Settings,
    position: &mut Point,
) -> Vec<Path> {
    let slice = &slices[index];
    let width = settings.line_width;
    let walls = settings.wall_loops as usize;
    let mut paths = Vec::new();

    if index == 0 {
        adhesion_paths(slice, settings, position, &mut paths);
    }

    // Walls: centre lines half a width inside the contour, then one width
    // further in for every additional loop. Printed from the inside out.
    let mut wall_loops: Vec<Vec<Contour>> = (0..walls)
        .map(|k| contours(&polygon::offset(slice, -(0.5 + k as f64) * width)))
        .collect();
    while let Some(loops) = wall_loops.pop() {
        let kind = if wall_loops.is_empty() {
            PathKind::OuterWall
        } else {
            PathKind::InnerWall
        };
        push_loops(loops, kind, width, position, &mut paths);
    }

    // Infill centre lines stay half a width away from the inner edge of the
    // innermost wall, so line ends and the links between lines do not
    // deposit material twice. The overlap setting pushes them back out.
    let inset = (walls as f64 + 0.5 - settings.infill_overlap / 100.0) * width;
    let fill_area = polygon::offset(slice, -inset);
    if fill_area.is_empty() {
        return paths;
    }
    let (solid, sparse) = split_solid_sparse(slices, index, &fill_area, settings);

    let direction = if index.is_multiple_of(2) {
        HatchDirection::Diagonal45
    } else {
        HatchDirection::Diagonal135
    };
    let other = if index.is_multiple_of(2) {
        HatchDirection::Diagonal135
    } else {
        HatchDirection::Diagonal45
    };
    let min_length = 0.5 * width;

    let solid_lines = polygon::hatch(&solid, width, direction, min_length);
    push_lines(
        solid_lines,
        PathKind::SolidInfill,
        2.0 * width,
        position,
        &mut paths,
    );

    let density = settings.infill_density / 100.0;
    if density > 0.0 {
        if density >= 1.0 {
            let lines = polygon::hatch(&sparse, width, direction, min_length);
            push_lines(
                lines,
                PathKind::SolidInfill,
                2.0 * width,
                position,
                &mut paths,
            );
        } else {
            match settings.infill_pattern {
                InfillPattern::Rectilinear => {
                    let lines = polygon::hatch(&sparse, width / density, direction, min_length);
                    push_lines(lines, PathKind::SparseInfill, 0.0, position, &mut paths);
                }
                InfillPattern::Grid => {
                    // Two directions share the material, so each is half as dense.
                    for d in [direction, other] {
                        let lines = polygon::hatch(&sparse, 2.0 * width / density, d, min_length);
                        push_lines(lines, PathKind::SparseInfill, 0.0, position, &mut paths);
                    }
                }
            }
        }
    }
    paths
}

/// Splits the fill area of a layer into the part that needs solid infill
/// (top and bottom surfaces) and the part that gets sparse infill.
fn split_solid_sparse(
    slices: &[Shapes],
    index: usize,
    fill_area: &Shapes,
    settings: &Settings,
) -> (Shapes, Shapes) {
    let bottom = settings.bottom_layers as usize;
    let top = settings.top_layers as usize;
    // Without enough layers below or above, the whole layer is a surface.
    if (bottom > 0 && index < bottom) || (top > 0 && index + top >= slices.len()) {
        return (fill_area.clone(), Vec::new());
    }
    // Interior is what is covered by every layer within reach above and below.
    let mut interior = fill_area.clone();
    for neighbour in (index - bottom..index).chain(index + 1..=index + top) {
        interior = polygon::intersection(&interior, &slices[neighbour]);
        if interior.is_empty() {
            break;
        }
    }
    // Shrink and regrow to drop slivers too thin to fill, then keep them solid.
    let width = settings.line_width;
    let interior = polygon::intersection(
        &polygon::offset(&polygon::offset(&interior, -width), width),
        fill_area,
    );
    (polygon::difference(fill_area, &interior), interior)
}

/// Skirt and brim on the first layer.
fn adhesion_paths(
    slice: &Shapes,
    settings: &Settings,
    position: &mut Point,
    paths: &mut Vec<Path>,
) {
    let width = settings.line_width;
    // Ignore holes: adhesion aids go around the outside only.
    let outlines: Shapes = slice
        .iter()
        .filter_map(|s| s.first().map(|o| vec![o.clone()]))
        .collect();
    let brim_loops = (settings.brim_width / width).round() as usize;

    let skirt_start = brim_loops as f64 * width + settings.skirt_distance;
    for k in (0..settings.skirt_loops as usize).rev() {
        let loops = outer_contours(&polygon::offset(
            &outlines,
            skirt_start + (0.5 + k as f64) * width,
        ));
        push_loops(loops, PathKind::Skirt, width, position, paths);
    }
    for k in (0..brim_loops).rev() {
        let loops = outer_contours(&polygon::offset(&outlines, (0.5 + k as f64) * width));
        push_loops(loops, PathKind::Brim, width, position, paths);
    }
}

fn contours(shapes: &Shapes) -> Vec<Contour> {
    shapes.iter().flatten().cloned().collect()
}

fn outer_contours(shapes: &Shapes) -> Vec<Contour> {
    shapes.iter().filter_map(|s| s.first().cloned()).collect()
}

/// Appends closed loops, always continuing with the loop whose nearest
/// vertex is closest to the current position and starting it there.
fn push_loops(
    mut loops: Vec<Contour>,
    kind: PathKind,
    min_length: f64,
    position: &mut Point,
    paths: &mut Vec<Path>,
) {
    loops.retain(|l| l.len() >= 3);
    while !loops.is_empty() {
        let mut best = (0, 0, f64::INFINITY);
        for (i, contour) in loops.iter().enumerate() {
            for (j, &p) in contour.iter().enumerate() {
                let d = distance(*position, p);
                if d < best.2 {
                    best = (i, j, d);
                }
            }
        }
        let mut points = loops.swap_remove(best.0);
        points.rotate_left(best.1);
        let path = Path {
            kind,
            points,
            closed: true,
        };
        if path.length() >= min_length {
            *position = path.points[0];
            paths.push(path);
        }
    }
}

/// Appends line segments, always continuing with the nearest end. Segments
/// starting within `join_distance` of the previous end are joined into one
/// path.
fn push_lines(
    mut lines: Vec<[Point; 2]>,
    kind: PathKind,
    join_distance: f64,
    position: &mut Point,
    paths: &mut Vec<Path>,
) {
    let mut current: Option<Path> = None;
    while !lines.is_empty() {
        let mut best = (0, false, f64::INFINITY);
        for (i, line) in lines.iter().enumerate() {
            for (reversed, end) in [(false, line[0]), (true, line[1])] {
                let d = distance(*position, end);
                if d < best.2 {
                    best = (i, reversed, d);
                }
            }
        }
        // Keep the order of the remaining lines stable so ties resolve the
        // same way on every run.
        let mut line = lines.remove(best.0);
        if best.1 {
            line.swap(0, 1);
        }
        match &mut current {
            Some(path) if best.2 <= join_distance => path.points.extend(line),
            _ => {
                paths.extend(current.take());
                current = Some(Path {
                    kind,
                    points: line.to_vec(),
                    closed: false,
                });
            }
        }
        *position = line[1];
    }
    paths.extend(current);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::field_reassign_with_default)]
    use super::*;

    fn extruded_length(layer: &Layer, kind: PathKind) -> f64 {
        layer
            .paths
            .iter()
            .filter(|p| p.kind == kind)
            .map(Path::length)
            .sum()
    }

    #[test]
    fn cube_has_expected_layers_and_structure() {
        let settings = Settings::default();
        let toolpaths = generate(&Mesh::cuboid(20.0, 20.0, 20.0), &settings).unwrap();
        assert_eq!(toolpaths.layers.len(), 100);
        assert!((toolpaths.layers[0].z - 0.2).abs() < 1e-9);
        assert!((toolpaths.layers[99].z - 20.0).abs() < 1e-6);

        let kinds = |i: usize| -> Vec<PathKind> {
            let mut kinds: Vec<PathKind> =
                toolpaths.layers[i].paths.iter().map(|p| p.kind).collect();
            kinds.dedup();
            kinds
        };
        use PathKind::*;
        assert_eq!(kinds(0), [Skirt, InnerWall, OuterWall, SolidInfill]);
        assert_eq!(kinds(3), [InnerWall, OuterWall, SolidInfill]);
        assert_eq!(kinds(4), [InnerWall, OuterWall, SparseInfill]);
        assert_eq!(kinds(94), [InnerWall, OuterWall, SparseInfill]);
        assert_eq!(kinds(95), [InnerWall, OuterWall, SolidInfill]);

        // Outer wall centre line of a 20 mm square, half a line width inside.
        let outer = extruded_length(&toolpaths.layers[50], OuterWall);
        assert!(
            (outer - 4.0 * (20.0 - settings.line_width)).abs() < 1e-3,
            "{outer}"
        );
    }

    #[test]
    fn model_is_centred_and_stays_on_the_bed() {
        let settings = Settings::default();
        let mut mesh = Mesh::cuboid(20.0, 30.0, 2.0);
        mesh.translate([-500.0, 300.0, 40.0]);
        let toolpaths = generate(&mesh, &settings).unwrap();
        assert_eq!(toolpaths.layers.len(), 10);
        let points: Vec<Point> = toolpaths
            .layers
            .iter()
            .flat_map(|l| &l.paths)
            .filter(|p| p.kind == PathKind::OuterWall)
            .flat_map(|p| p.points.clone())
            .collect();
        let min_x = points.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min);
        let max_y = points
            .iter()
            .map(|p| p[1])
            .fold(f64::NEG_INFINITY, f64::max);
        assert!((min_x - (100.0 + 0.225)).abs() < 1e-3, "{min_x}");
        assert!((max_y - (125.0 - 0.225)).abs() < 1e-3, "{max_y}");
    }

    #[test]
    fn solid_cube_extrudes_its_own_volume() {
        let mut settings = Settings::default();
        settings.infill_density = 100.0;
        settings.skirt_loops = 0;
        settings.infill_overlap = 0.0;
        let toolpaths = generate(&Mesh::cuboid(20.0, 20.0, 10.0), &settings).unwrap();
        // Nominal volume: every millimetre of path deposits width x height.
        let volume: f64 = toolpaths
            .layers
            .iter()
            .map(|l| l.paths.iter().map(Path::length).sum::<f64>() * settings.line_width * l.height)
            .sum();
        let ratio = volume / 4000.0;
        assert!(
            (ratio - 1.0).abs() < 0.03,
            "extruded {volume} mm3, ratio {ratio}"
        );
    }

    #[test]
    fn steps_get_solid_infill_where_exposed() {
        // A wide base with a narrow tower: the base top around the tower
        // must be solid, the part under the tower stays sparse.
        let mut base = Mesh::cuboid(30.0, 30.0, 5.0);
        let mut tower = Mesh::cuboid(10.0, 10.0, 10.0);
        tower.translate([10.0, 10.0, 5.0]);
        let offset = base.vertices.len() as u32;
        base.vertices.extend(tower.vertices);
        base.triangles
            .extend(tower.triangles.iter().map(|t| t.map(|i| i + offset)));

        let settings = Settings::default();
        let toolpaths = generate(&base, &settings).unwrap();
        // Layer 24 is the last base layer (z = 5.0), layer 15 is deep inside.
        let top_of_base = &toolpaths.layers[24];
        let solid = extruded_length(top_of_base, PathKind::SolidInfill);
        let sparse = extruded_length(top_of_base, PathKind::SparseInfill);
        assert!(solid > 1000.0, "{solid}");
        assert!(sparse > 1.0, "{sparse}");
        assert_eq!(
            extruded_length(&toolpaths.layers[12], PathKind::SolidInfill),
            0.0
        );
    }

    /// A square plate with a square through-hole. Winding is not
    /// consistent, which the slicer must tolerate.
    fn frame(outer: f64, hole: f64, height: f64) -> Mesh {
        let inset = (outer - hole) / 2.0;
        let ring = |lo: f64, hi: f64, z: f64| [[lo, lo, z], [hi, lo, z], [hi, hi, z], [lo, hi, z]];
        let mut mesh = Mesh::default();
        for z in [0.0, height] {
            mesh.vertices.extend(ring(0.0, outer, z));
            mesh.vertices.extend(ring(inset, outer - inset, z));
        }
        for k in 0..4u32 {
            let n = (k + 1) % 4;
            for base in [0, 4] {
                // Side walls: outer (base 0) and hole (base 4).
                mesh.triangles.push([base + k, base + n, base + n + 8]);
                mesh.triangles.push([base + k, base + n + 8, base + k + 8]);
            }
            for level in [0, 8] {
                // Bottom and top rings between outer and hole contour.
                mesh.triangles.push([level + k, level + n, level + n + 4]);
                mesh.triangles
                    .push([level + k, level + n + 4, level + k + 4]);
            }
        }
        mesh
    }

    #[test]
    fn holes_get_their_own_walls() {
        let settings = Settings::default();
        let toolpaths = generate(&frame(30.0, 10.0, 4.0), &settings).unwrap();
        assert_eq!(toolpaths.layers.len(), 20);
        let layer = &toolpaths.layers[10];
        let outer: Vec<&Path> = layer
            .paths
            .iter()
            .filter(|p| p.kind == PathKind::OuterWall)
            .collect();
        assert_eq!(outer.len(), 2);
        // Outside: 30 mm square moved in by half a line width. Hole: 10 mm
        // square moved out by half a line width, with rounded corners.
        let w = settings.line_width;
        let expected = 4.0 * (30.0 - w) + 4.0 * 10.0 + std::f64::consts::PI * w;
        let length = extruded_length(layer, PathKind::OuterWall);
        assert!((length - expected).abs() < 0.2, "{length} vs {expected}");
        // No infill line may cross the hole.
        for path in layer
            .paths
            .iter()
            .filter(|p| p.kind == PathKind::SparseInfill)
        {
            for pair in path.points.windows(2) {
                let mid = [
                    (pair[0][0] + pair[1][0]) / 2.0,
                    (pair[0][1] + pair[1][1]) / 2.0,
                ];
                let in_hole = (mid[0] - 110.0).abs() < 5.0 && (mid[1] - 110.0).abs() < 5.0;
                assert!(!in_hole, "infill crosses the hole at {mid:?}");
            }
        }
    }

    #[test]
    fn brim_and_skirt() {
        let mut settings = Settings::default();
        settings.brim_width = 2.25;
        settings.skirt_loops = 1;
        let toolpaths = generate(&Mesh::cuboid(10.0, 10.0, 1.0), &settings).unwrap();
        let first = &toolpaths.layers[0];
        assert_eq!(
            first
                .paths
                .iter()
                .filter(|p| p.kind == PathKind::Brim)
                .count(),
            5
        );
        assert_eq!(
            first
                .paths
                .iter()
                .filter(|p| p.kind == PathKind::Skirt)
                .count(),
            1
        );
        assert!(
            toolpaths.layers[1]
                .paths
                .iter()
                .all(|p| p.kind != PathKind::Brim)
        );
    }

    #[test]
    fn errors() {
        let settings = Settings::default();
        assert_eq!(
            generate(&Mesh::default(), &settings),
            Err(FdmError::EmptyModel)
        );
        assert!(matches!(
            generate(&Mesh::cuboid(300.0, 10.0, 10.0), &settings),
            Err(FdmError::TooLarge { .. })
        ));
        assert_eq!(
            generate(&Mesh::cuboid(10.0, 10.0, 0.1), &settings),
            Err(FdmError::NothingToPrint)
        );
    }

    #[test]
    fn output_is_deterministic() {
        let settings = Settings::default();
        let mesh = Mesh::cuboid(15.0, 25.0, 3.0);
        assert_eq!(
            generate(&mesh, &settings).unwrap(),
            generate(&mesh, &settings).unwrap()
        );
    }
}
