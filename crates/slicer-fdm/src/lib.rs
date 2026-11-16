//! FDM toolpath generation: perimeters, infill, supports and path planning.

mod adaptive;
mod infill;
pub mod preview;

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

use slicer_core::polygon::{self, HatchDirection, distance};
use slicer_core::slice::slice_mesh_reporting;
use slicer_core::{Contour, Diagnostic, Diagnostics, Mesh, Point, Shapes, parallel_map};
use slicer_profiles::{InfillPattern, SeamPosition, Settings, WallOrder};

/// Difference in Y below which two points count as equally far back when
/// placing the seam, in millimetres.
const SEAM_TOLERANCE: f64 = 0.001;

/// How sharp a corner must be to take the seam: one minus the cosine of the
/// angle by which the wall turns there (0.1 is about 26 degrees).
const MIN_SEAM_CORNER: f64 = 0.1;

/// How far along the wall on either side of a point its turn is measured
/// when looking for corners, in line widths. Long enough to take in the
/// rounded inward corners of the outer and the first inner wall.
const SEAM_CORNER_REACH: f64 = 2.0;

/// Corners this close in sharpness to the sharpest one count as equally
/// sharp.
const SEAM_CORNER_TOLERANCE: f64 = 0.05;

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

/// How far the lines of a bridge reach onto what carries them, in line
/// widths.
const BRIDGE_ANCHOR: f64 = 2.0;

/// Longest rim of a hole in the mesh that is closed before slicing.
const MAX_HOLE_EDGES: usize = 64;

/// Triangles whose corners lie this close to a line (mm) count as having
/// no area and are removed before slicing. A tenth of the 10 nm polygon
/// grid, so the surface moves by far less than the contours resolve.
const DEGENERATE_HEIGHT: f64 = 1e-6;

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
    /// Solid infill with nothing below it, spanning from side to side.
    Bridge,
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
    /// Extruder that prints the path, counted from 0.
    pub extruder: u32,
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
    /// The caller asked to stop.
    Cancelled,
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
            Self::Cancelled => write!(f, "slicing was cancelled"),
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
            Self::Cancelled => "slicing.cancelled",
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
    generate_cancellable(mesh, settings, diagnostics, &AtomicBool::new(false))
}

/// Like [`generate_reporting`], and gives up with [`FdmError::Cancelled`]
/// soon after `stop` is set.
// ponytail: looked at between the steps and before every layer of the two
// longest ones. Cutting the mesh runs to its end; on a large model that can
// be a second or two.
pub fn generate_cancellable(
    mesh: &Mesh,
    settings: &Settings,
    diagnostics: &mut Diagnostics,
    stop: &AtomicBool,
) -> Result<Toolpaths, FdmError> {
    let stopped = || stop.load(Ordering::Relaxed);
    let mut mesh = mesh.clone();
    let turned = mesh.orient();
    if turned > 0 {
        diagnostics.push(
            Diagnostic::warning(
                "mesh.reoriented",
                format!(
                    "{turned} triangle(s) of the mesh faced the wrong way and were turned around"
                ),
            )
            .with("triangles", turned),
        );
    }
    let removed = mesh.remove_degenerate(DEGENERATE_HEIGHT);
    if removed > 0 {
        diagnostics.push(
            Diagnostic::warning(
                "mesh.degenerate_removed",
                format!("{removed} triangle(s) of the mesh without area were removed"),
            )
            .with("triangles", removed),
        );
    }
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
    // ponytail: one extruder prints everything, the lowest one the model
    // asks for. Separate regions per extruder and tool changes are the
    // upgrade.
    let used = mesh.used_extruders();
    let extruder = used.first().copied().unwrap_or(0);
    if used.len() > 1 {
        let list: Vec<String> = used.iter().map(|e| (e + 1).to_string()).collect();
        diagnostics.push(
            Diagnostic::warning(
                "slicing.multiple_extruders",
                format!(
                    "the objects of the model ask for extruders {}; only one extruder is supported, everything is printed with extruder {}",
                    list.join(", "),
                    extruder + 1
                ),
            )
            .with("extruders", list.join(" "))
            .with("extruder", extruder + 1),
        );
    }
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

    // Without adaptive layers layer i ends at first_layer_height + i *
    // layer_height. Every layer is cut at its mid-height.
    let (tops, heights) = if settings.adaptive_layers {
        adaptive::plan(&mesh, size[2], settings)
    } else {
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
        (tops, heights)
    };
    let cut_heights: Vec<f64> = tops
        .iter()
        .zip(&heights)
        .map(|(top, h)| top - h / 2.0)
        .collect();
    let slices = slice_mesh_reporting(&mesh, &cut_heights, diagnostics);
    if stopped() {
        return Err(FdmError::Cancelled);
    }
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
        // The same angle reaches less far between slices closer together.
        let reach = if settings.adaptive_layers {
            reach * (cut_heights[index] - cut_heights[index - 1]) / settings.layer_height
        } else {
            reach
        };
        let below = polygon::offset(&slices[index - 1], reach);
        polygon::clean(polygon::difference(&slices[index], &below), least, 0.0)
    });
    let supports = if settings.supports {
        // Short bridges carry themselves.
        let carried: Vec<Shapes> = parallel_map(slices.len(), |index| {
            hanging[index]
                .iter()
                .filter(|shape| !spans_freely(shape, &slices[index - 1], settings))
                .cloned()
                .collect()
        });
        support_areas(&slices, &carried, settings)
    } else {
        Vec::new()
    };
    let geometry = parallel_map(slices.len(), |index| {
        if stopped() {
            return (Vec::new(), false);
        }
        layer_work(&slices, &heights, &supports, index, settings)
    });
    if stopped() {
        return Err(FdmError::Cancelled);
    }
    let mut position: Point = [0.0, 0.0];
    let mut thin_layers = Vec::new();
    let mut layers: Vec<Layer> = geometry
        .into_iter()
        .enumerate()
        .map(|(index, (work, mut thin))| {
            let mut paths = Vec::new();
            let work = if stopped() { Vec::new() } else { work };
            for piece in work {
                match piece {
                    Work::Loops(loops, kind, min_length) => {
                        let wall = matches!(kind, PathKind::OuterWall | PathKind::InnerWall);
                        let seam = if wall {
                            settings.seam_position
                        } else {
                            SeamPosition::Nearest
                        };
                        push_loops(loops, kind, min_length, seam, &mut position, &mut paths);
                    }
                    Work::Lines(lines, kind, join_distance) => {
                        push_lines(lines, kind, join_distance, &mut position, &mut paths);
                    }
                    Work::Polylines(lines, kind, join_distance) => {
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
    if stopped() {
        return Err(FdmError::Cancelled);
    }
    // Which travel moves stay over the part depends only on the finished
    // order of a layer, so all cores can work on it again.
    let inside = parallel_map(layers.len(), |index| {
        travels_inside(&layers[index], &slices[index])
    });
    for (layer, flags) in layers.iter_mut().zip(inside) {
        for (path, flag) in layer.paths.iter_mut().zip(flags) {
            path.travel_inside = flag;
            path.extruder = extruder;
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
            PathKind::InnerWall | PathKind::SolidInfill | PathKind::Bridge | PathKind::SparseInfill
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
// ponytail: straight columns. No supports that lean or branch. The gaps are
// one layer whatever its thickness, so with adaptive layers they are as thin
// as the layers there.
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
    /// Open polylines, like `Lines`.
    Polylines(Vec<Vec<Point>>, PathKind, f64),
}

/// Generates the geometry of one layer in the order it is printed. The flag
/// tells whether a noticeable part of the layer is too narrow to be covered
/// by a wall.
fn layer_work(
    slices: &[Shapes],
    heights: &[f64],
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
    let (solid, sparse) = split_solid_sparse(slices, heights, index, &fill_area, settings);

    let turn = if index.is_multiple_of(2) { 0.0 } else { 90.0 };
    let direction = HatchDirection::degrees(settings.infill_angle + turn);
    let other = HatchDirection::degrees(settings.infill_angle + turn + 90.0);
    let min_length = 0.5 * width;

    let (solid, sparse) = if index > 0 {
        let (bridges, solid, sparse) =
            split_bridges(&slices[index - 1], &fill_area, solid, sparse, width);
        for (bridge, across) in bridges {
            let lines = polygon::hatch(&bridge, width, across, min_length);
            work.push(Work::Lines(lines, PathKind::Bridge, 2.0 * width));
        }
        (solid, sparse)
    } else {
        (solid, sparse)
    };
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
                InfillPattern::Triangles => {
                    let spacing = 3.0 * width / density;
                    let angle = settings.infill_angle;
                    for lines in infill::triangles(&sparse, spacing, angle, min_length) {
                        work.push(Work::Lines(lines, PathKind::SparseInfill, 0.0));
                    }
                }
                InfillPattern::Gyroid => {
                    // Top of the layer, as in `generate_cancellable`.
                    let z = settings.first_layer_height + index as f64 * settings.layer_height;
                    let lines = infill::gyroid(&sparse, width / density, z, min_length);
                    work.push(Work::Polylines(lines, PathKind::SparseInfill, 0.0));
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
    heights: &[f64],
    index: usize,
    fill_area: &Shapes,
    settings: &Settings,
) -> (Shapes, Shapes) {
    // Without enough layers below or above, the whole layer is a surface.
    let Some((bottom, top)) = shell_layers(heights, index, settings) else {
        return (fill_area.clone(), Vec::new());
    };
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

/// How many layers below and above layer `index` form its bottom and top
/// shell, or `None` when there are not enough of them and the layer is a
/// surface itself. With adaptive layers the shells keep the thickness they
/// have with constant layers, `bottom_layers` or `top_layers` times
/// `layer_height`, however many layers that takes.
fn shell_layers(heights: &[f64], index: usize, settings: &Settings) -> Option<(usize, usize)> {
    let bottom = settings.bottom_layers as usize;
    let top = settings.top_layers as usize;
    if !settings.adaptive_layers {
        let surface = (bottom > 0 && index < bottom) || (top > 0 && index + top >= heights.len());
        return (!surface).then_some((bottom, top));
    }
    let count = |layers: &mut dyn Iterator<Item = &f64>, layer_count: usize| {
        let wanted = layer_count as f64 * settings.layer_height - 1e-6;
        let (mut total, mut count) = (0.0, 0);
        for height in layers {
            if total >= wanted {
                break;
            }
            total += height;
            count += 1;
        }
        (total >= wanted).then_some(count)
    };
    let below = count(&mut heights[..index].iter().rev(), bottom)?;
    let above = count(&mut heights[index + 1..].iter(), top)?;
    Some((below, above))
}

/// Whether `point` lies inside the shapes (even-odd).
fn point_inside(shapes: &Shapes, point: Point) -> bool {
    let mut inside = false;
    for contour in shapes.iter().flatten() {
        for (i, &p) in contour.iter().enumerate() {
            let q = contour[(i + 1) % contour.len()];
            if (p[1] > point[1]) != (q[1] > point[1])
                && point[0] < p[0] + (point[1] - p[1]) / (q[1] - p[1]) * (q[0] - p[0])
            {
                inside = !inside;
            }
        }
    }
    inside
}

/// The directions the lines of a bridge can run in: every 15 degrees, the
/// four with exact arithmetic first, then along and across the longest edge
/// of the area, so a straight bridge at any angle gets lines straight across.
// ponytail: an area whose longest edge says nothing about how it is carried
// (a curved one, say) gets the best of the 15 degree steps.
fn bridge_directions(shape: &[Contour]) -> Vec<HatchDirection> {
    let mut directions: Vec<HatchDirection> = [
        0.0, 90.0, 45.0, 135.0, 15.0, 30.0, 60.0, 75.0, 105.0, 120.0, 150.0, 165.0,
    ]
    .into_iter()
    .map(HatchDirection::degrees)
    .collect();
    // Unit vector of the first longest edge: square root only, no angles.
    let mut longest = (0.0, [1.0, 0.0]);
    for contour in shape {
        for (i, &p) in contour.iter().enumerate() {
            let q = contour[(i + 1) % contour.len()];
            let length = distance(p, q);
            if length > longest.0 {
                longest = (length, [(q[0] - p[0]) / length, (q[1] - p[1]) / length]);
            }
        }
    }
    let [cos, sin] = longest.1;
    directions.push(HatchDirection::Angle(cos, sin));
    directions.push(HatchDirection::Angle(-sin, cos));
    directions
}

/// Whether an area that hangs free is a bridge short enough to print
/// without supports: in one direction nearly all lines across it go on over
/// the layer below at both ends, and none is longer than the setting allows.
fn spans_freely(shape: &[Contour], below: &Shapes, settings: &Settings) -> bool {
    let width = settings.line_width;
    let area = vec![shape.to_vec()];
    bridge_directions(shape).into_iter().any(|direction| {
        let (mut total, mut loose, mut longest) = (0.0, 0.0, 0.0_f64);
        for [a, b] in polygon::hatch(&area, width, direction, 0.0) {
            let length = distance(a, b);
            if length == 0.0 {
                continue;
            }
            // The area ends half a line before the layer below begins.
            let step = 2.0 * width / length;
            let beyond = |from: Point, to: Point| {
                [
                    to[0] + (to[0] - from[0]) * step,
                    to[1] + (to[1] - from[1]) * step,
                ]
            };
            total += length;
            longest = longest.max(length);
            if !(point_inside(below, beyond(a, b)) && point_inside(below, beyond(b, a))) {
                loose += length;
            }
        }
        total > 0.0 && loose <= 0.1 * total && longest <= settings.support_bridge_length
    })
}

/// Takes the bridges out of the solid and sparse areas of a layer: solid
/// areas at least a line wide with nothing below, grown a little onto what
/// carries them. Every bridge comes with the direction in which most of its
/// lines rest on the layer below at both ends (see `bridge_directions`).
fn split_bridges(
    below: &Shapes,
    fill_area: &Shapes,
    solid: Shapes,
    sparse: Shapes,
    width: f64,
) -> (Vec<(Shapes, HatchDirection)>, Shapes, Shapes) {
    if solid.is_empty() {
        return (Vec::new(), solid, sparse);
    }
    // Shrinking and regrowing drops the strips a sloped surface leaves on
    // every layer.
    let air = polygon::difference(&solid, below);
    let air = polygon::offset(&polygon::offset(&air, -0.5 * width), 0.5 * width);
    if air.is_empty() {
        return (Vec::new(), solid, sparse);
    }
    let area = polygon::intersection(&polygon::offset(&air, BRIDGE_ANCHOR * width), fill_area);
    let bridges = area
        .iter()
        .map(|shape| {
            let bridge = vec![shape.clone()];
            let mut best = (f64::INFINITY, HatchDirection::Diagonal45);
            for direction in bridge_directions(shape) {
                // Length of the lines with an end in the air.
                let loose: f64 = polygon::hatch(&bridge, width, direction, 0.0)
                    .iter()
                    .filter(|line| !line.iter().all(|&end| point_inside(below, end)))
                    .map(|line| distance(line[0], line[1]))
                    .sum();
                if loose < best.0 - 1e-6 {
                    best = (loose, direction);
                }
            }
            (bridge, best.1)
        })
        .collect();
    (
        bridges,
        polygon::difference(&solid, &area),
        polygon::difference(&sparse, &area),
    )
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

/// The rearmost of the points that `allowed` lets through (largest Y, then
/// smallest X). Points almost as far back as the rearmost one count as
/// equally far back; otherwise rounding would move the seam along a straight
/// rear edge from layer to layer.
fn rearmost(points: &[Point], allowed: impl Fn(usize) -> bool) -> usize {
    let back = (0..points.len())
        .filter(|&j| allowed(j))
        .map(|j| points[j][1])
        .fold(f64::MIN, f64::max);
    let mut rear = None;
    for (j, p) in points.iter().enumerate() {
        let better = rear.is_none_or(|r: usize| p[0] < points[r][0]);
        if allowed(j) && p[1] >= back - SEAM_TOLERANCE && better {
            rear = Some(j);
        }
    }
    rear.unwrap_or(0)
}

/// How sharply a closed loop turns around each of its points, measured over
/// `reach` millimetres of path on either side, so that a corner rounded off
/// by many short edges counts like a sharp one: one minus the cosine of the
/// angle between the chord from the point `reach` behind to this point and
/// the chord from this point to the one `reach` ahead. Positive where the
/// loop turns left, which is an outward corner (outer contours run
/// counter-clockwise, holes clockwise), negative where it turns right, an
/// inward corner.
// ponytail: chords, not the true total turn: a rounded corner measures a
// little blunter than a sharp one of the same angle, turns of more than
// 180 degrees within the reach are not told apart from smaller ones, and
// corners closer together than `reach` blur into one.
fn corners(points: &[Point], reach: f64) -> Vec<f64> {
    let n = points.len();
    let perimeter: f64 = (0..n)
        .map(|j| distance(points[j], points[(j + 1) % n]))
        .sum();
    let reach = reach.min(perimeter / 4.0);
    if reach <= 0.0 {
        return vec![0.0; n];
    }
    // The point `reach` along the loop from point `j`, ahead or behind.
    let along = |j: usize, ahead: bool| {
        let mut k = j;
        let mut left = reach;
        for _ in 0..n {
            let next = if ahead { (k + 1) % n } else { (k + n - 1) % n };
            let (p, q) = (points[k], points[next]);
            let d = distance(p, q);
            if d >= left {
                let t = left / d;
                return [p[0] + t * (q[0] - p[0]), p[1] + t * (q[1] - p[1])];
            }
            left -= d;
            k = next;
        }
        points[k]
    };
    (0..n)
        .map(|j| {
            let (p, q, r) = (along(j, false), points[j], along(j, true));
            let (a, b) = ([q[0] - p[0], q[1] - p[1]], [r[0] - q[0], r[1] - q[1]]);
            let lengths = distance(p, q) * distance(q, r);
            if lengths == 0.0 {
                return 0.0;
            }
            let sharpness = 1.0 - (a[0] * b[0] + a[1] * b[1]) / lengths;
            if a[0] * b[1] - a[1] * b[0] < 0.0 {
                -sharpness
            } else {
                sharpness
            }
        })
        .collect()
}

/// Appends closed loops, always continuing with the loop whose nearest
/// vertex is closest to the current position. Where a loop starts is up to
/// `seam`; loops shorter than the line `width` are dropped.
fn push_loops(
    mut loops: Vec<Contour>,
    kind: PathKind,
    width: f64,
    seam: SeamPosition,
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
        let start = match seam {
            SeamPosition::Rear => rearmost(&points, |_| true),
            SeamPosition::Corner => {
                // Inward corners hide the seam best, so they win over any
                // outward corner; among the sharpest of one kind, the rearmost.
                let turn = corners(&points, SEAM_CORNER_REACH * width);
                let inward = turn.iter().map(|t| -t).fold(0.0, f64::max);
                let (side, sharpest) = if inward >= MIN_SEAM_CORNER {
                    (-1.0, inward)
                } else {
                    (1.0, turn.iter().copied().fold(0.0, f64::max))
                };
                if sharpest < MIN_SEAM_CORNER {
                    rearmost(&points, |_| true)
                } else {
                    rearmost(&points, |j| {
                        side * turn[j] >= sharpest - SEAM_CORNER_TOLERANCE
                    })
                }
            }
            SeamPosition::Nearest => best.1,
        };
        points.rotate_left(start);
        let path = Path {
            kind,
            points,
            closed: true,
            travel_inside: false,
            extruder: 0,
        };
        if path.length() >= width {
            *position = path.points[0];
            paths.push(path);
        }
    }
}

/// Appends open lines (segments or polylines), always continuing with the
/// nearest end. Lines starting within `join_distance` of the previous end
/// are joined into one path.
fn push_lines<L: AsRef<[Point]>>(
    mut lines: Vec<L>,
    kind: PathKind,
    join_distance: f64,
    position: &mut Point,
    paths: &mut Vec<Path>,
) {
    lines.retain(|line| !line.as_ref().is_empty());
    let mut current: Option<Path> = None;
    while !lines.is_empty() {
        let mut best = (0, false, f64::INFINITY);
        for (i, line) in lines.iter().enumerate() {
            let line = line.as_ref();
            for (reversed, end) in [(false, line[0]), (true, line[line.len() - 1])] {
                let d = distance(*position, end);
                if d < best.2 {
                    best = (i, reversed, d);
                }
            }
        }
        // Keep the order of the remaining lines stable so ties resolve the
        // same way on every run.
        let mut line = lines.remove(best.0).as_ref().to_vec();
        if best.1 {
            line.reverse();
        }
        let end = line[line.len() - 1];
        match &mut current {
            Some(path) if best.2 <= join_distance => path.points.extend(line),
            _ => {
                paths.extend(current.take());
                current = Some(Path {
                    kind,
                    points: line,
                    closed: false,
                    travel_inside: false,
                    extruder: 0,
                });
            }
        }
        *position = end;
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

    #[test]
    fn bridges_run_from_support_to_support() {
        // Two pillars 20 mm apart carry a deck. Turned by a quarter or by
        // 30 or 20 degrees, the lines of the bridge must turn with it.
        for degrees in [0.0_f64, 90.0, 30.0, 20.0] {
            let mut mesh = Mesh::cuboid(5.0, 20.0, 5.0);
            let mut pillar = Mesh::cuboid(5.0, 20.0, 5.0);
            pillar.translate([25.0, 0.0, 0.0]);
            mesh.merge(&pillar);
            let mut deck = Mesh::cuboid(30.0, 20.0, 2.0);
            deck.translate([0.0, 0.0, 5.0]);
            mesh.merge(&deck);
            mesh.rotate_z(degrees.to_radians());
            let (sin, cos) = degrees.to_radians().sin_cos();
            let toolpaths = generate(&mesh, &Settings::default()).unwrap();
            // Supports go under a bridge only if it is longer than allowed.
            for (limit, expected) in [(10.0, true), (25.0, false)] {
                let mut settings = Settings::default();
                settings.supports = true;
                settings.support_bridge_length = limit;
                let supported = generate(&mesh, &settings)
                    .unwrap()
                    .layers
                    .iter()
                    .flat_map(|layer| &layer.paths)
                    .any(|path| path.kind == PathKind::Support);
                assert_eq!(supported, expected, "{degrees} degrees, limit {limit}");
            }
            // Layer 25 is the lowest layer of the deck.
            let mut along = 0.0;
            let mut long = 0.0;
            for path in &toolpaths.layers[25].paths {
                if path.kind != PathKind::Bridge {
                    continue;
                }
                for pair in path.points.windows(2) {
                    let length = distance(pair[0], pair[1]);
                    if length < 2.0 {
                        continue;
                    }
                    long += length;
                    let (dx, dy) = (pair[1][0] - pair[0][0], pair[1][1] - pair[0][1]);
                    if (dx * sin - dy * cos).abs() < 1e-6 * length {
                        along += length;
                        // From pillar to pillar, and a little onto both.
                        assert!(length > 20.5, "{degrees} degrees: {length}");
                    }
                }
            }
            assert!(long > 500.0, "{degrees} degrees: {long}");
            assert!(along > 0.95 * long, "{degrees} degrees: {along} of {long}");
        }
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
    fn infill_follows_the_chosen_angle() {
        let mut settings = Settings::default();
        settings.infill_pattern = InfillPattern::Rectilinear;
        settings.infill_angle = 30.0;
        let toolpaths = generate(&Mesh::cuboid(30.0, 30.0, 6.0), &settings).unwrap();
        // Layer 10 runs at 30 degrees, layer 11 across it.
        for (layer, along) in [(10, [0.75_f64.sqrt(), 0.5]), (11, [-0.5, 0.75_f64.sqrt()])] {
            let mut long = 0;
            for path in &toolpaths.layers[layer].paths {
                if path.kind != PathKind::SparseInfill {
                    continue;
                }
                for pair in path.points.windows(2) {
                    let (dx, dy) = (pair[1][0] - pair[0][0], pair[1][1] - pair[0][1]);
                    if distance(pair[0], pair[1]) > 5.0 {
                        long += 1;
                        assert!((dx * along[1] - dy * along[0]).abs() < 1e-6, "{dx} {dy}");
                    }
                }
            }
            assert!(long > 3, "{long}");
        }
    }

    const PATTERNS: [InfillPattern; 4] = [
        InfillPattern::Rectilinear,
        InfillPattern::Grid,
        InfillPattern::Triangles,
        InfillPattern::Gyroid,
    ];

    #[test]
    fn sparse_patterns_use_as_much_material_as_grid() {
        let mesh = Mesh::cuboid(30.0, 30.0, 30.0);
        let lengths: Vec<f64> = PATTERNS
            .iter()
            .map(|&pattern| {
                let mut settings = Settings::default();
                settings.infill_density = 20.0;
                settings.infill_pattern = pattern;
                let toolpaths = generate(&mesh, &settings).unwrap();
                toolpaths
                    .layers
                    .iter()
                    .map(|layer| extruded_length(layer, PathKind::SparseInfill))
                    .sum()
            })
            .collect();
        let grid = lengths[1];
        assert!(grid > 1000.0, "{grid}");
        for (pattern, length) in PATTERNS.iter().zip(&lengths) {
            let ratio = length / grid;
            assert!(
                (ratio - 1.0).abs() < 0.15,
                "{pattern:?}: {length} vs {grid}"
            );
        }
    }

    #[test]
    fn sparse_patterns_stay_inside_and_repeat() {
        let mesh = frame(30.0, 10.0, 4.0);
        for pattern in PATTERNS {
            let mut settings = Settings::default();
            settings.infill_pattern = pattern;
            let toolpaths = generate(&mesh, &settings).unwrap();
            assert_eq!(toolpaths, generate(&mesh, &settings).unwrap());
            // The sparse area: the frame (95 to 125 mm, hole 105 to 115 mm)
            // moved inwards by the walls; around the hole the corners are
            // round. Offsets approximate arcs by chords.
            let walls = f64::from(settings.wall_loops);
            let inset = (walls + 0.5 - settings.infill_overlap / 100.0) * settings.line_width;
            let tolerance = 0.005;
            let inside = |p: Point| {
                let outer = (p[0] - 110.0).abs().max((p[1] - 110.0).abs());
                let gap = |v: f64| ((v - 110.0).abs() - 5.0).max(0.0);
                outer <= 15.0 - inset + tolerance
                    && distance([gap(p[0]), gap(p[1])], [0.0, 0.0]) >= inset - tolerance
            };
            let sparse: Vec<&Path> = toolpaths
                .layers
                .iter()
                .flat_map(|layer| &layer.paths)
                .filter(|path| path.kind == PathKind::SparseInfill)
                .collect();
            assert!(sparse.len() > 20, "{pattern:?}");
            for path in sparse {
                for pair in path.points.windows(2) {
                    let mid = [
                        (pair[0][0] + pair[1][0]) / 2.0,
                        (pair[0][1] + pair[1][1]) / 2.0,
                    ];
                    for p in [pair[0], mid, pair[1]] {
                        assert!(inside(p), "{pattern:?} leaves the sparse area at {p:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn triangles_meet_in_points_and_gyroid_turns_with_height() {
        let mut settings = Settings::default();
        settings.skirt_loops = 0;
        settings.infill_pattern = InfillPattern::Triangles;
        let mesh = Mesh::cuboid(30.0, 30.0, 6.0);
        let toolpaths = generate(&mesh, &settings).unwrap();
        let sparse = |layer: &Layer| -> Vec<Vec<Point>> {
            layer
                .paths
                .iter()
                .filter(|p| p.kind == PathKind::SparseInfill)
                .map(|p| p.points.clone())
                .collect()
        };
        // Every layer alike.
        assert_eq!(sparse(&toolpaths.layers[10]), sparse(&toolpaths.layers[11]));
        // Lines of the first and the last direction cross on a line of the
        // middle one: the crossing of two lines through the centre region
        // lies on a third.
        let lines: Vec<[Point; 2]> = sparse(&toolpaths.layers[10])
            .iter()
            .flat_map(|points| points.windows(2).map(|w| [w[0], w[1]]))
            .filter(|s| distance(s[0], s[1]) > 5.0)
            .collect();
        let angle = |s: &[Point; 2]| {
            let deg = (s[1][1] - s[0][1]).atan2(s[1][0] - s[0][0]).to_degrees();
            deg.rem_euclid(180.0)
        };
        let family = |deg: f64| lines.iter().filter(move |s| (angle(s) - deg).abs() < 0.01);
        let crossing = |a: &[Point; 2], b: &[Point; 2]| -> Option<Point> {
            let (d1, d2) = (
                [a[1][0] - a[0][0], a[1][1] - a[0][1]],
                [b[1][0] - b[0][0], b[1][1] - b[0][1]],
            );
            let den = d1[0] * d2[1] - d1[1] * d2[0];
            let t = ((b[0][0] - a[0][0]) * d2[1] - (b[0][1] - a[0][1]) * d2[0]) / den;
            let u = ((b[0][0] - a[0][0]) * d1[1] - (b[0][1] - a[0][1]) * d1[0]) / den;
            ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u))
                .then(|| [a[0][0] + t * d1[0], a[0][1] + t * d1[1]])
        };
        let near_line = |p: Point, s: &[Point; 2]| {
            let d = [s[1][0] - s[0][0], s[1][1] - s[0][1]];
            ((p[0] - s[0][0]) * d[1] - (p[1] - s[0][1]) * d[0]).abs() / distance(s[0], s[1])
        };
        let mut checked = 0;
        for a in family(45.0) {
            for b in family(165.0) {
                if let Some(p) = crossing(a, b) {
                    let off = family(105.0)
                        .map(|m| near_line(p, m))
                        .fold(f64::MAX, f64::min);
                    assert!(off < 1e-6, "{p:?} is {off} mm off the middle lines");
                    checked += 1;
                }
            }
        }
        assert!(checked >= 4, "{checked}");

        settings.infill_pattern = InfillPattern::Gyroid;
        let toolpaths = generate(&mesh, &settings).unwrap();
        assert_ne!(sparse(&toolpaths.layers[10]), sparse(&toolpaths.layers[11]));
    }

    #[test]
    fn slicing_can_be_cancelled() {
        let mesh = Mesh::cuboid(20.0, 20.0, 20.0);
        let stop = AtomicBool::new(true);
        let result =
            generate_cancellable(&mesh, &Settings::default(), &mut Diagnostics::new(), &stop);
        assert_eq!(result, Err(FdmError::Cancelled));
    }

    #[test]
    fn seams_hide_in_corners() {
        // A house lying on its side: the rearmost point is the blunt ridge,
        // the sharpest corners are the two at the front.
        let house = [
            [0.0, 0.0],
            [20.0, 0.0],
            [20.0, 10.0],
            [10.0, 14.0],
            [0.0, 10.0],
        ];
        let mesh = slicer_core::primitives::extrude(&house, [10.0, 5.0], 4.0, 1, 0.0);
        let starts = |mesh: &Mesh, seam| {
            let mut settings = Settings::default();
            settings.seam_position = seam;
            generate(mesh, &settings)
                .unwrap()
                .layers
                .iter()
                .flat_map(|layer| layer.paths.clone())
                .filter(|path| path.kind == PathKind::OuterWall)
                .map(|path| path.points[0])
                .collect::<Vec<Point>>()
        };
        let rear = starts(&mesh, SeamPosition::Rear);
        let corner = starts(&mesh, SeamPosition::Corner);
        assert_eq!(corner.len(), 20);
        // The model is 20 x 14 mm, centred on the bed at (110, 110).
        assert!(rear.iter().all(|p| distance(*p, [110.0, 117.0]) < 0.5));
        assert!(
            corner.iter().all(|p| distance(*p, [100.0, 103.0]) < 0.5),
            "{corner:?}"
        );
        // Without corners the seam is at the rear.
        let round = slicer_core::primitives::cylinder(10.0, 4.0, 64);
        assert_eq!(
            starts(&round, SeamPosition::Corner),
            starts(&round, SeamPosition::Rear)
        );
    }

    #[test]
    fn seams_prefer_inward_corners() {
        // An L: five outward corners and one inward corner at (8, 8), which
        // the wall offsets round off.
        let l = [
            [0.0, 0.0],
            [20.0, 0.0],
            [20.0, 8.0],
            [8.0, 8.0],
            [8.0, 20.0],
            [0.0, 20.0],
        ];
        let mesh = slicer_core::primitives::extrude(&l, [4.0, 4.0], 4.0, 1, 0.0);
        let mut settings = Settings::default();
        settings.seam_position = SeamPosition::Corner;
        let toolpaths = generate(&mesh, &settings).unwrap();
        let walls = |kind| {
            toolpaths
                .layers
                .iter()
                .flat_map(|layer| &layer.paths)
                .filter(|path| path.kind == kind)
                .map(|path| path.points[0])
                .collect::<Vec<Point>>()
        };
        // The model is 20 x 20 mm, centred on the bed at (110, 110), so the
        // inward corner is at (108, 108).
        let outer = walls(PathKind::OuterWall);
        assert_eq!(outer.len(), 20);
        assert!(
            outer.iter().all(|p| distance(*p, [108.0, 108.0]) < 0.5),
            "{outer:?}"
        );
        assert!(outer.iter().all(|p| distance(*p, outer[0]) < 0.01));
        // The inner wall is rounded more widely there but still found.
        let inner = walls(PathKind::InnerWall);
        assert_eq!(inner.len(), 20);
        assert!(
            inner.iter().all(|p| distance(*p, [108.0, 108.0]) < 1.5),
            "{inner:?}"
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
    fn paths_carry_the_extruder_of_the_model() {
        let extruders = |mesh: &Mesh| {
            let mut diagnostics = Diagnostics::new();
            let toolpaths = generate_reporting(mesh, &Settings::default(), &mut diagnostics);
            let mut used: Vec<u32> = toolpaths
                .unwrap()
                .layers
                .iter()
                .flat_map(|layer| layer.paths.iter().map(|p| p.extruder))
                .collect();
            used.dedup();
            (used, diagnostics)
        };
        let cube = Mesh::cuboid(10.0, 10.0, 2.0);
        let (used, diagnostics) = extruders(&cube);
        assert_eq!(used, [0]);
        assert!(diagnostics.is_empty());

        // A model on extruder 3 alone prints with it, without a warning.
        let mut third = cube.clone();
        third.set_extruder(0, 2);
        let (used, diagnostics) = extruders(&third);
        assert_eq!(used, [2]);
        assert!(diagnostics.is_empty());

        // Two objects on extruders 2 and 3: the first one prints both.
        let mut other = cube.clone();
        other.translate([20.0, 0.0, 0.0]);
        other.set_extruder(0, 1);
        third.merge(&other);
        let (used, diagnostics) = extruders(&third);
        assert_eq!(used, [1]);
        assert_eq!(diagnostics.codes(), ["slicing.multiple_extruders"]);
        assert_eq!(diagnostics.as_slice()[0].context["extruders"], "2 3");
        assert_eq!(diagnostics.as_slice()[0].context["extruder"], "2");
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

    fn adaptive() -> Settings {
        let mut settings = Settings::default();
        settings.adaptive_layers = true;
        settings
    }

    #[test]
    fn adaptive_layers_follow_the_slope() {
        let mut settings = adaptive();
        settings.infill_density = 100.0;
        settings.infill_overlap = 0.0;
        settings.skirt_loops = 0;
        let mesh = slicer_core::primitives::sphere(15.0, 96);
        let layers = generate(&mesh, &settings).unwrap().layers;
        let (min, max) = (settings.min_layer_height, settings.max_layer_height);
        assert!((layers[0].height - settings.first_layer_height).abs() < 1e-9);
        for pair in layers.windows(2) {
            let h = pair[1].height;
            assert!(h > min - 1e-6 && h < max + 1e-6, "{h}");
            assert!((pair[1].z - pair[0].z - h).abs() < 1e-9);
        }
        // Thin at the top, thick at the equator.
        let at = |z: f64| {
            layers
                .iter()
                .find(|layer| layer.z >= z)
                .map(|layer| layer.height)
                .unwrap()
        };
        assert!((at(15.0) - max).abs() < 1e-6, "{}", at(15.0));
        assert!((at(29.8) - min).abs() < 0.005, "{}", at(29.8));
        assert!((at(0.6) - min).abs() < 0.03, "{}", at(0.6));
        // Far fewer layers than with the thinnest one throughout.
        assert!(layers.len() < 210, "{}", layers.len());
        // The model is no more than the thinnest layer taller than the print.
        let top = layers.last().unwrap().z;
        assert!(top <= 30.0 + 1e-6 && top > 30.0 - min, "{top}");
        // Extrusion follows the thickness of every layer.
        let volume: f64 = layers
            .iter()
            .map(|l| l.paths.iter().map(Path::length).sum::<f64>() * settings.line_width * l.height)
            .sum();
        let ratio = volume / mesh.volume();
        assert!((ratio - 1.0).abs() < 0.05, "ratio {ratio}");
    }

    #[test]
    fn adaptive_layers_end_on_flat_surfaces_and_keep_shell_thickness() {
        let mut base = Mesh::cuboid(30.0, 30.0, 5.0);
        let mut tower = Mesh::cuboid(10.0, 10.0, 10.0);
        tower.translate([10.0, 10.0, 5.0]);
        base.merge(&tower);
        let settings = adaptive();
        let layers = generate(&base, &settings).unwrap().layers;
        // Walls are vertical: thick layers, ending on the step and the top.
        assert!(layers[1..].iter().all(|l| l.height > 0.15), "{layers:?}");
        assert!(layers.iter().any(|l| (l.z - 5.0).abs() < 1e-9));
        assert!((layers.last().unwrap().z - 15.0).abs() < 1e-9);
        // The top shell is as thick as five layers of 0.2 mm, not five layers.
        let solid_on_top: f64 = layers
            .iter()
            .filter(|l| l.z > 10.0 && extruded_length(l, PathKind::SparseInfill) == 0.0)
            .map(|l| l.height)
            .sum();
        let wanted = f64::from(settings.top_layers) * settings.layer_height;
        assert!(
            solid_on_top > wanted - 1e-6 && solid_on_top < wanted + settings.max_layer_height,
            "{solid_on_top}"
        );
    }
}
