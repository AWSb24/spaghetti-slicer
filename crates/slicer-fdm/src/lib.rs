//! FDM toolpath generation: perimeters, infill, supports and path planning.

pub mod preview;

use std::fmt;

use slicer_core::polygon::{self, HatchDirection, distance};
use slicer_core::slice::slice_mesh_reporting;
use slicer_core::{Contour, Diagnostic, Diagnostics, Mesh, Point, Shapes, parallel_map};
use slicer_profiles::{InfillPattern, SeamPosition, Settings, WallOrder};

/// Difference in Y below which two points count as equally far back when
/// placing the seam, in millimetres.
const SEAM_TOLERANCE: f64 = 0.001;

/// Largest distance a wall may move when points are dropped from it, in
/// millimetres. Far below what a printer can resolve, but it cuts the number
/// of moves on curves several times.
const PATH_RESOLUTION: f64 = 0.0125;

/// Smallest area in mm² hanging free over the layer below that is reported
/// as an overhang.
const MIN_OVERHANG_AREA: f64 = 1.0;

/// Smallest area in mm² that gets a support.
const MIN_SUPPORT_AREA: f64 = 0.25;

/// Largest distance the outline of a support may move when points are
/// dropped from it, in millimetres.
const SUPPORT_RESOLUTION: f64 = 0.05;

/// Distance between the lines of the dense support layers, in line widths.
const SUPPORT_INTERFACE_SPACING: f64 = 2.0;

/// Longest rim of a hole in the mesh that is closed before slicing.
const MAX_HOLE_EDGES: usize = 64;

/// What a path is for. Determines speed and the label in the G-code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    /// Loop around the model on the first layer, not touching it.
    Skirt,
    /// Loops attached to the first layer to improve adhesion.
    Brim,
    /// Lines that carry overhangs and are removed after printing.
    Support,
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
    /// Whether the move from the end of the previous path to the start of
    /// this one stays over the part, so that it needs no retraction.
    pub travel_inside: bool,
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

impl slicer_core::Diagnose for FdmError {
    fn code(&self) -> &'static str {
        match self {
            Self::EmptyModel => "slicing.empty_model",
            Self::TooLarge { .. } => "slicing.too_large",
            Self::NothingToPrint => "slicing.nothing_to_print",
        }
    }

    fn context(&self) -> Vec<(&'static str, String)> {
        match self {
            Self::TooLarge { model, volume } => {
                let text = |v: &[f64; 3]| format!("{:.3} x {:.3} x {:.3}", v[0], v[1], v[2]);
                vec![("model_size", text(model)), ("build_volume", text(volume))]
            }
            _ => Vec::new(),
        }
    }
}

/// Slices `mesh` and generates toolpaths. The model is centred on the bed
/// and placed on it.
pub fn generate(mesh: &Mesh, settings: &Settings) -> Result<Toolpaths, FdmError> {
    generate_reporting(mesh, settings, &mut Diagnostics::new())
}

/// Like [`generate`], and collects warnings about the mesh and about
/// geometry that could not be printed faithfully.
pub fn generate_reporting(
    mesh: &Mesh,
    settings: &Settings,
    diagnostics: &mut Diagnostics,
) -> Result<Toolpaths, FdmError> {
    let mut mesh = mesh.clone();
    let closed = mesh.close_holes(MAX_HOLE_EDGES);
    if closed > 0 {
        diagnostics.push(
            Diagnostic::warning(
                "mesh.repaired",
                format!("{closed} hole(s) in the surface of the mesh were closed"),
            )
            .with("holes", closed),
        );
    }
    mesh.check(diagnostics);
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
    let slices = slice_mesh_reporting(&mesh, &cut_heights, diagnostics);
    if slices.iter().all(Vec::is_empty) {
        return Err(FdmError::NothingToPrint);
    }

    // The geometry of a layer depends only on the slices, so all cores work
    // on it. Putting the pieces in printing order depends on where the
    // previous layer ended and happens one layer after the other.
    // What every layer has beyond the layer below. With supports the limit
    // is half a line width (about 50 degrees from vertical at the default
    // layer height); without, only a full line width is worth a warning.
    let (reach, least) = if settings.supports {
        (0.5 * settings.line_width, 0.01)
    } else {
        (settings.line_width, MIN_OVERHANG_AREA)
    };
    let hanging: Vec<Shapes> = parallel_map(slices.len(), |index| {
        if index == 0 {
            return Vec::new();
        }
        let below = polygon::offset(&slices[index - 1], reach);
        polygon::clean(polygon::difference(&slices[index], &below), least, 0.0)
    });
    let supports = if settings.supports {
        support_areas(&slices, &hanging, settings)
    } else {
        Vec::new()
    };
    let geometry = parallel_map(slices.len(), |index| {
        layer_work(&slices, &supports, index, settings)
    });
    let mut position: Point = [0.0, 0.0];
    let mut thin_layers = Vec::new();
    let mut layers: Vec<Layer> = geometry
        .into_iter()
        .enumerate()
        .map(|(index, (work, mut thin))| {
            let mut paths = Vec::new();
            for piece in work {
                match piece {
                    Work::Loops(loops, kind, min_length) => {
                        let wall = matches!(kind, PathKind::OuterWall | PathKind::InnerWall);
                        let rear = wall && settings.seam_position == SeamPosition::Rear;
                        push_loops(loops, kind, min_length, rear, &mut position, &mut paths);
                    }
                    Work::Lines(lines, kind, join_distance) => {
                        push_lines(lines, kind, join_distance, &mut position, &mut paths);
                    }
                }
            }
            // A layer with material but without any outer wall prints nothing.
            if !slices[index].is_empty() && paths.iter().all(|p| p.kind != PathKind::OuterWall) {
                thin = true;
            }
            if thin {
                thin_layers.push(index);
            }
            Layer {
                z: tops[index],
                height: heights[index],
                paths,
            }
        })
        .collect();
    // Which travel moves stay over the part depends only on the finished
    // order of a layer, so all cores can work on it again.
    let inside = parallel_map(layers.len(), |index| {
        travels_inside(&layers[index], &slices[index])
    });
    for (layer, flags) in layers.iter_mut().zip(inside) {
        for (path, flag) in layer.paths.iter_mut().zip(flags) {
            path.travel_inside = flag;
        }
    }
    // Without supports, say where they would be needed.
    let hanging_layers: Vec<usize> = (0..hanging.len())
        .filter(|&index| !settings.supports && !hanging[index].is_empty())
        .collect();
    if let Some(first) = hanging_layers.first() {
        let largest = hanging
            .iter()
            .map(|shapes| polygon::area(shapes))
            .fold(0.0, f64::max);
        diagnostics.push(
            Diagnostic::warning(
                "slicing.unsupported_overhangs",
                format!(
                    "{} of {} layers have areas with nothing below them (overhangs or bridges, up to {largest:.1} mm² in one layer); supports are not generated, these areas may sag (first: layer {})",
                    hanging_layers.len(),
                    slices.len(),
                    first + 1
                ),
            )
            .with("layers", hanging_layers.len())
            .with("first_layer", first + 1),
        );
    }
    if let Some(first) = thin_layers.first() {
        diagnostics.push(
            Diagnostic::warning(
                "slicing.thin_regions",
                format!(
                    "{} of {} layers contain regions narrower than the line width ({} mm) that are not printed (first: layer {})",
                    thin_layers.len(),
                    slices.len(),
                    settings.line_width,
                    first + 1
                ),
            )
            .with("layers", thin_layers.len())
            .with("first_layer", first + 1),
        );
    }
    Ok(Toolpaths { layers })
}

/// For every path of a layer: whether the move to its start stays inside
/// the outline of the layer. Only asked for infill and inner walls; before
/// an outer wall the nozzle always retracts, to keep the visible surface
/// clean.
fn travels_inside(layer: &Layer, slice: &Shapes) -> Vec<bool> {
    // The first path of a layer is reached from the layer below.
    let mut flags = vec![false];
    for pair in layer.paths.windows(2) {
        let (previous, path) = (&pair[0], &pair[1]);
        let hidden = matches!(
            path.kind,
            PathKind::InnerWall | PathKind::SolidInfill | PathKind::SparseInfill
        );
        let from = if previous.closed {
            previous.points.first()
        } else {
            previous.points.last()
        };
        flags.push(match (hidden, from, path.points.first()) {
            (true, Some(&from), Some(&to)) => polygon::segment_inside(slice, from, to),
            _ => false,
        });
    }
    flags
}

/// Where supports go on every layer, and which part of that is filled
/// densely: below everything that hangs free, down to the bed or to the
/// model, with one empty layer under the surface they carry and above the
/// model they stand on, and one line width of distance to its sides. The
/// dense part is what lies right under a carried surface.
// ponytail: straight columns. No supports that lean or branch, and bridges
// a printer could span are supported like any other overhang.
fn support_areas(
    slices: &[Shapes],
    hanging: &[Shapes],
    settings: &Settings,
) -> Vec<(Shapes, Shapes)> {
    // Where no support may be depends only on the model, so all cores work
    // on it; the supports themselves grow downwards layer by layer.
    let keep_out = parallel_map(slices.len(), |index| {
        let mut model = slices[index].clone();
        if index > 0 {
            model = polygon::union(&model, &slices[index - 1]);
        }
        polygon::offset(&model, settings.line_width)
    });
    let mut areas: Vec<Shapes> = vec![Vec::new(); slices.len()];
    for index in (0..slices.len().saturating_sub(2)).rev() {
        let above = polygon::union(&hanging[index + 2], &areas[index + 1]);
        if above.is_empty() {
            continue;
        }
        // Every layer adds the points of its own outline. Without thinning
        // them out a support under a round overhang ends up with tens of
        // thousands of points and takes seconds per model.
        areas[index] = polygon::clean(
            polygon::difference(&above, &keep_out[index]),
            MIN_SUPPORT_AREA,
            0.0,
        )
        .iter()
        .map(|shape| {
            shape
                .iter()
                .map(|contour| polygon::simplify(contour, SUPPORT_RESOLUTION))
                .filter(|contour| contour.len() > 2)
                .collect()
        })
        .collect();
    }
    let dense_layers = settings.support_interface_layers as usize;
    parallel_map(areas.len(), |index| {
        let area = areas[index].clone();
        let mut carried = Vec::new();
        if !area.is_empty() {
            for above in hanging.iter().skip(index + 2).take(dense_layers) {
                carried = polygon::union(&carried, above);
            }
        }
        let dense = polygon::intersection(&area, &carried);
        (area, dense)
    })
}

/// Geometry of one kind that still has to be put in printing order.
enum Work {
    /// Closed loops, their kind and the shortest loop worth printing.
    Loops(Vec<Contour>, PathKind, f64),
    /// Line segments, their kind and the distance up to which consecutive
    /// segments are joined into one path.
    Lines(Vec<[Point; 2]>, PathKind, f64),
}

/// Generates the geometry of one layer in the order it is printed. The flag
/// tells whether a noticeable part of the layer is too narrow to be covered
/// by a wall.
fn layer_work(
    slices: &[Shapes],
    supports: &[(Shapes, Shapes)],
    index: usize,
    settings: &Settings,
) -> (Vec<Work>, bool) {
    let slice = &slices[index];
    let width = settings.line_width;
    let walls = settings.wall_loops as usize;
    let mut work = Vec::new();

    let support = supports.get(index).filter(|(area, _)| !area.is_empty());
    if index == 0 {
        adhesion_work(slice, support.map(|(area, _)| area), settings, &mut work);
    }
    if let Some((area, dense)) = support {
        // A loop around every support holds its lines together and makes
        // sure that a support too narrow for a line is printed at all.
        let inner = polygon::offset(area, -0.5 * width);
        let loops = contours(&inner)
            .iter()
            .map(|contour| polygon::simplify(contour, PATH_RESOLUTION))
            .collect();
        work.push(Work::Loops(loops, PathKind::Support, width));
        // The same direction on every layer, so the lines stack up to walls.
        let lines = polygon::hatch(
            &polygon::difference(&inner, dense),
            settings.support_spacing,
            HatchDirection::Diagonal45,
            0.5 * width,
        );
        work.push(Work::Lines(lines, PathKind::Support, 0.0));
        // Close lines across the walls below carry the surface.
        let lines = polygon::hatch(
            &polygon::intersection(&inner, dense),
            SUPPORT_INTERFACE_SPACING * width,
            HatchDirection::Diagonal135,
            0.5 * width,
        );
        work.push(Work::Lines(lines, PathKind::Support, 0.0));
    }

    // Walls: centre lines half a width inside the contour, then one width
    // further in for every additional loop. Printed from the inside out.
    let wall_shapes: Vec<Shapes> = (0..walls)
        .map(|k| polygon::offset(slice, -(0.5 + k as f64) * width))
        .collect();

    // What the outer wall covers is its centre line grown back by half a
    // width. Anything of the slice beyond that is lost. Rounded corners
    // always lose a little, so only report a noticeable share.
    let total = polygon::area(slice);
    let covered = polygon::area(&polygon::offset(&wall_shapes[0], 0.5 * width));
    let thin = total - covered > (0.02 * total).max(width * width);

    // Wall 0 is the outer one.
    let mut order: Vec<usize> = (0..walls).collect();
    if settings.wall_order == WallOrder::InnerOuter {
        order.reverse();
    }
    for k in order {
        let kind = if k == 0 {
            PathKind::OuterWall
        } else {
            PathKind::InnerWall
        };
        let loops = contours(&wall_shapes[k])
            .iter()
            .map(|contour| polygon::simplify(contour, PATH_RESOLUTION))
            .collect();
        work.push(Work::Loops(loops, kind, width));
    }

    // Infill centre lines stay half a width away from the inner edge of the
    // innermost wall, so line ends and the links between lines do not
    // deposit material twice. The overlap setting pushes them back out.
    let inset = (walls as f64 + 0.5 - settings.infill_overlap / 100.0) * width;
    let fill_area = polygon::offset(slice, -inset);
    if fill_area.is_empty() {
        return (work, thin);
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
    work.push(Work::Lines(solid_lines, PathKind::SolidInfill, 2.0 * width));

    let density = settings.infill_density / 100.0;
    if density > 0.0 {
        if density >= 1.0 {
            let lines = polygon::hatch(&sparse, width, direction, min_length);
            work.push(Work::Lines(lines, PathKind::SolidInfill, 2.0 * width));
        } else {
            match settings.infill_pattern {
                InfillPattern::Rectilinear => {
                    let lines = polygon::hatch(&sparse, width / density, direction, min_length);
                    work.push(Work::Lines(lines, PathKind::SparseInfill, 0.0));
                }
                InfillPattern::Grid => {
                    // Two directions share the material, so each is half as dense.
                    for d in [direction, other] {
                        let lines = polygon::hatch(&sparse, 2.0 * width / density, d, min_length);
                        work.push(Work::Lines(lines, PathKind::SparseInfill, 0.0));
                    }
                }
            }
        }
    }
    (work, thin)
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
    // Regrowing rounds the corners of the interior. The bits it leaves in
    // the corners of the fill area are no surface; they stay sparse.
    let solid = polygon::clean(
        polygon::difference(fill_area, &interior),
        width * width,
        0.0,
    );
    if solid.is_empty() {
        return (solid, fill_area.clone());
    }
    let sparse = polygon::difference(fill_area, &solid);
    (solid, sparse)
}

/// Skirt and brim on the first layer.
fn adhesion_work(
    slice: &Shapes,
    support: Option<&Shapes>,
    settings: &Settings,
    work: &mut Vec<Work>,
) {
    let width = settings.line_width;
    // Ignore holes: adhesion aids go around the outside only.
    let outlines: Shapes = slice
        .iter()
        .filter_map(|s| s.first().map(|o| vec![o.clone()]))
        .collect();
    let brim_loops = (settings.brim_width / width).round() as usize;

    let skirt_start = brim_loops as f64 * width + settings.skirt_distance;
    // The skirt goes around the convex hull of everything on the first
    // layer, supports included. Following a detailed outline at a distance
    // of millimetres is very slow to compute (14 s for an outline of 5000
    // points).
    let points: Vec<Point> = outlines
        .iter()
        .chain(support.into_iter().flatten())
        .flatten()
        .flatten()
        .copied()
        .collect();
    let hull: Shapes = vec![vec![polygon::convex_hull(&points)]];
    for k in (0..settings.skirt_loops as usize).rev() {
        let loops = outer_contours(&polygon::offset(
            &hull,
            skirt_start + (0.5 + k as f64) * width,
        ));
        work.push(Work::Loops(loops, PathKind::Skirt, width));
    }
    for k in (0..brim_loops).rev() {
        let loops = outer_contours(&polygon::offset(&outlines, (0.5 + k as f64) * width));
        work.push(Work::Loops(loops, PathKind::Brim, width));
    }
}

fn contours(shapes: &Shapes) -> Vec<Contour> {
    shapes.iter().flatten().cloned().collect()
}

fn outer_contours(shapes: &Shapes) -> Vec<Contour> {
    shapes.iter().filter_map(|s| s.first().cloned()).collect()
}

/// Appends closed loops, always continuing with the loop whose nearest
/// vertex is closest to the current position. A loop starts at that vertex
/// or, with `rear_start`, at its rearmost vertex (largest Y, then smallest
/// X), so the starts of all layers line up.
fn push_loops(
    mut loops: Vec<Contour>,
    kind: PathKind,
    min_length: f64,
    rear_start: bool,
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
        let start = if rear_start {
            // Points almost as far back as the rearmost one count as equally
            // far back; the leftmost of them is taken. Otherwise rounding
            // would move the seam along a straight rear edge from layer to
            // layer.
            let back = points.iter().map(|p| p[1]).fold(f64::MIN, f64::max);
            let mut rear = None;
            for (j, p) in points.iter().enumerate() {
                let better = rear.is_none_or(|r: usize| p[0] < points[r][0]);
                if p[1] >= back - SEAM_TOLERANCE && better {
                    rear = Some(j);
                }
            }
            rear.unwrap_or(0)
        } else {
            best.1
        };
        points.rotate_left(start);
        let path = Path {
            kind,
            points,
            closed: true,
            travel_inside: false,
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
                    travel_inside: false,
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
    fn seams_line_up_at_the_rear() {
        // A cylinder off the bed centre, so "nearest" would wander.
        let mesh = slicer_core::primitives::cylinder(10.0, 4.0, 64);
        let starts = |seam| {
            let mut settings = Settings::default();
            settings.seam_position = seam;
            let toolpaths = generate(&mesh, &settings).unwrap();
            toolpaths
                .layers
                .iter()
                .flat_map(|layer| &layer.paths)
                .filter(|path| path.kind == PathKind::OuterWall)
                .map(|path| path.points[0])
                .collect::<Vec<Point>>()
        };
        let rear = starts(SeamPosition::Rear);
        assert_eq!(rear.len(), 20);
        // Every outer wall starts at the same point, the rearmost one.
        assert!(
            rear.iter().all(|p| distance(*p, rear[1]) < 0.05),
            "{rear:?}"
        );
        assert!(
            (rear[0][1] - (110.0 + 10.0 - 0.225)).abs() < 0.01,
            "{:?}",
            rear[0]
        );
        let nearest = starts(SeamPosition::Nearest);
        assert!(nearest.iter().any(|p| distance(*p, nearest[0]) > 1.0));

        // On a straight rear edge the seam is at its left end on every layer.
        let mut settings = Settings::default();
        settings.skirt_loops = 0;
        let mut cube = Mesh::cuboid(20.0, 20.0, 4.0);
        cube.rotate_z(0.3);
        cube.rotate_z(-0.3);
        let toolpaths = generate(&cube, &settings).unwrap();
        let starts: Vec<Point> = toolpaths
            .layers
            .iter()
            .flat_map(|layer| &layer.paths)
            .filter(|path| path.kind == PathKind::OuterWall)
            .map(|path| path.points[0])
            .collect();
        assert!(
            starts.iter().all(|p| distance(*p, starts[0]) < 0.01),
            "{starts:?}"
        );
        assert!(
            starts[0][0] < 105.0 && starts[0][1] > 115.0,
            "{:?}",
            starts[0]
        );
    }

    #[test]
    fn travel_inside_the_part_is_marked() {
        let mut settings = Settings::default();
        settings.skirt_loops = 0;
        let toolpaths = generate(&Mesh::cuboid(30.0, 30.0, 6.0), &settings).unwrap();
        // A layer in the middle: sparse infill inside two walls.
        let paths = &toolpaths.layers[15].paths;
        let sparse: Vec<&Path> = paths
            .iter()
            .filter(|p| p.kind == PathKind::SparseInfill)
            .collect();
        assert!(sparse.len() > 10);
        assert!(sparse.iter().all(|p| p.travel_inside));
        // The first path of a layer and every outer wall are reached with a
        // retraction.
        assert!(!paths[0].travel_inside);
        assert!(
            paths
                .iter()
                .filter(|p| p.kind == PathKind::OuterWall)
                .all(|p| !p.travel_inside)
        );

        // Between two separate parts the nozzle leaves the material.
        let mut two = Mesh::cuboid(10.0, 10.0, 2.0);
        let mut other = Mesh::cuboid(10.0, 10.0, 2.0);
        other.translate([30.0, 0.0, 0.0]);
        two.merge(&other);
        let toolpaths = generate(&two, &settings).unwrap();
        let paths = &toolpaths.layers[5].paths;
        let leaving = paths
            .windows(2)
            .filter(|w| distance(w[0].points[0], w[1].points[0]) > 20.0)
            .count();
        assert!(leaving >= 1);
        assert!(
            paths
                .windows(2)
                .filter(|w| distance(w[0].points[0], w[1].points[0]) > 20.0)
                .all(|w| !w[1].travel_inside)
        );
    }

    #[test]
    fn supports_stand_under_overhangs() {
        // A column with a wide plate on top, like the letter T.
        let mut t = Mesh::cuboid(10.0, 10.0, 10.0);
        t.translate([10.0, 0.0, 0.0]);
        let mut plate = Mesh::cuboid(30.0, 10.0, 2.0);
        plate.translate([0.0, 0.0, 10.0]);
        t.merge(&plate);

        let support_layers = |settings: &Settings| -> (Vec<usize>, Diagnostics, Toolpaths) {
            let mut warnings = Diagnostics::new();
            let toolpaths = generate_reporting(&t, settings, &mut warnings).unwrap();
            let layers = (0..toolpaths.layers.len())
                .filter(|&i| {
                    toolpaths.layers[i]
                        .paths
                        .iter()
                        .any(|p| p.kind == PathKind::Support)
                })
                .collect();
            (layers, warnings, toolpaths)
        };
        let mut settings = Settings::default();
        let (none, warnings, _) = support_layers(&settings);
        assert!(none.is_empty());
        assert_eq!(warnings.codes(), ["slicing.unsupported_overhangs"]);

        settings.supports = true;
        let (layers, warnings, toolpaths) = support_layers(&settings);
        assert!(warnings.is_empty(), "{:?}", warnings.codes());
        // The plate starts at 10 mm, layer 50. Supports fill layers 0 to 48
        // and leave layer 49 empty.
        assert_eq!(layers, (0..49).collect::<Vec<usize>>());
        // They stand left and right of the column (which is centred on the
        // bed at x 105 to 115), a line width away from it, under the plate.
        for layer in &toolpaths.layers[..49] {
            for path in layer.paths.iter().filter(|p| p.kind == PathKind::Support) {
                for p in &path.points {
                    assert!((94.99..=125.01).contains(&p[0]), "{p:?}");
                    assert!(p[0] <= 105.0 - 0.4 || p[0] >= 115.0 + 0.4, "{p:?}");
                    assert!((104.99..=115.01).contains(&p[1]), "{p:?}");
                }
            }
        }
        let both_sides = |layer: &Layer| {
            let xs: Vec<f64> = layer
                .paths
                .iter()
                .filter(|p| p.kind == PathKind::Support)
                .flat_map(|p| p.points.iter().map(|q| q[0]))
                .collect();
            xs.iter().any(|&x| x < 105.0) && xs.iter().any(|&x| x > 115.0)
        };
        assert!(toolpaths.layers[..49].iter().all(both_sides));

        // The two layers right under the plate are filled densely.
        let length = |layer: &Layer| -> f64 {
            layer
                .paths
                .iter()
                .filter(|p| p.kind == PathKind::Support)
                .flat_map(|p| p.points.windows(2))
                .map(|w| distance(w[0], w[1]))
                .sum()
        };
        let sparse = length(&toolpaths.layers[40]);
        assert!((length(&toolpaths.layers[46]) - sparse).abs() < 1.0);
        assert!(length(&toolpaths.layers[47]) > 2.0 * sparse);
        assert!(length(&toolpaths.layers[48]) > 2.0 * sparse);

        // The skirt goes around the supports, not only around the column.
        let skirt_left = toolpaths.layers[0]
            .paths
            .iter()
            .filter(|p| p.kind == PathKind::Skirt)
            .flat_map(|p| p.points.iter().map(|q| q[0]))
            .fold(f64::MAX, f64::min);
        assert!(skirt_left < 95.0, "{skirt_left}");

        // With a base under the column the supports stand on the model and
        // leave one layer empty above it too: the base ends at 2 mm (layers
        // 0 to 9), the plate starts at 12 mm (layer 60).
        let mut h = Mesh::cuboid(30.0, 10.0, 2.0);
        let mut column = Mesh::cuboid(10.0, 10.0, 10.0);
        column.translate([10.0, 0.0, 2.0]);
        h.merge(&column);
        plate.translate([0.0, 0.0, 2.0]);
        h.merge(&plate);
        let toolpaths = generate(&h, &settings).unwrap();
        let layers: Vec<usize> = (0..toolpaths.layers.len())
            .filter(|&i| {
                toolpaths.layers[i]
                    .paths
                    .iter()
                    .any(|p| p.kind == PathKind::Support)
            })
            .collect();
        assert_eq!(layers, (11..59).collect::<Vec<usize>>());
    }

    #[test]
    fn wall_order_can_be_chosen() {
        let first_wall = |order| {
            let mut settings = Settings::default();
            settings.wall_order = order;
            settings.skirt_loops = 0;
            let toolpaths = generate(&Mesh::cuboid(20.0, 20.0, 1.0), &settings).unwrap();
            let kinds: Vec<PathKind> = toolpaths.layers[0].paths.iter().map(|p| p.kind).collect();
            assert!(kinds.contains(&PathKind::OuterWall) && kinds.contains(&PathKind::InnerWall));
            kinds[0]
        };
        assert_eq!(first_wall(WallOrder::InnerOuter), PathKind::InnerWall);
        assert_eq!(first_wall(WallOrder::OuterInner), PathKind::OuterWall);
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
