//! FDM toolpath generation: perimeters, infill, supports and path planning.

mod adaptive;
mod comb;
mod infill;
mod objects;
mod plate;
pub mod preview;
mod sequential;
mod tree;

pub use objects::{OBJECT_SETTINGS, apply_object_toml, object_settings};
pub use plate::{ARRANGE_GAP, Placement, arrange, arrange_objects};
pub mod progress;

pub use progress::{Progress, Step};

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

use slicer_core::polygon::{self, HatchDirection, distance};
use slicer_core::slice::{slice_mesh, slice_mesh_reporting};
use slicer_core::{Contour, Diagnostic, Diagnostics, Mesh, Point, Shapes, parallel_map};
use slicer_profiles::{InfillPattern, SeamPosition, Settings, SupportStyle, WallOrder};

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

/// Narrowest region that a wall misses and that still gets a line along its
/// middle, in line widths. Narrower regions stay empty.
const MIN_THIN_WIDTH: f64 = 0.5;

/// Widest line along the middle of a narrow region, in line widths. Regions
/// up to twice a line width wide get a single line instead of a wall loop
/// that would fold back onto itself.
// ponytail: one line up to twice the line width (0.9 mm at the defaults),
// not two narrower lines side by side. Whether a nozzle lays down a line that
// wide cleanly is unverified; split regions wider than about 1.5 widths into
// two lines if prints show it does not.
const MAX_CENTRED_WIDTH: f64 = 2.0;

/// Shortest part of a wall loop, in line widths, whose two sides lie closer
/// than a line width and that is printed as a line along the middle of the
/// region instead. The tips of corners sharper than about 40 degrees count.
const MIN_FOLD_LENGTH: f64 = 2.0;

/// How much closer than a line width (mm) the two sides of a wall loop may
/// come before the loop counts as folded. Keeps regions exactly two widths
/// wide at two lines and rounding noise along the walls out of the folds.
const FOLD_MARGIN: f64 = 0.01;

/// Upper limit of the area, per contour and per millimetre of growth, by
/// which the polygonal arcs of an offset fall short of circular ones, in
/// mm² / mm. Twice the 4π/3 times 0.0025 mm (the arc tolerance of the
/// polygon operations) that a full turn of arc loses.
const ARC_SHORTFALL: f64 = 0.02;

/// Walls are told apart into overhanging and supported parts by the
/// middles of pieces at most this long, see [`split_overhangs`].
const OVERHANG_STEP: f64 = 0.5;

/// Overhangs along a wall shorter than this many millimetres print as
/// ordinary wall, and supported stretches this short between two
/// overhangs as overhang, so speed and fan do not switch back and forth.
const MIN_OVERHANG_RUN: f64 = 1.0;

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
    /// Lines in gaps between walls, or between the walls and the infill,
    /// too narrow for another wall.
    GapFill,
    /// Dense fill of top and bottom surfaces.
    SolidInfill,
    /// Solid infill with nothing below it, spanning from side to side.
    Bridge,
    /// Low-density fill of the interior.
    SparseInfill,
    /// Tower beside the model that an extruder primes into after a tool
    /// change.
    PrimeTower,
    /// A pass with very little material over a top surface to smooth it.
    Ironing,
    /// Part of a wall where more than half the line hangs over the edge of
    /// the layer below (see `Settings::overhang_speed`).
    OverhangWall,
    /// Part of a wall that hangs over the edge of the layer below entirely
    /// (see `Settings::overhang_speed_full`).
    OverhangWallFull,
}

/// [`Path::object`] of a path that belongs to no single object.
pub const NO_OBJECT: u32 = u32::MAX;

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
    /// Object on the plate the path belongs to, counted from 0 in the order
    /// the objects were given; [`NO_OBJECT`] for skirt, brim, supports and
    /// the prime tower. On a plate of several objects printed together it
    /// is only worked out with `Settings::label_objects` and is
    /// [`NO_OBJECT`] for every path otherwise.
    pub object: u32,
    /// Width of the extruded line in millimetres where it differs from the
    /// line width setting: lines along the middle of regions too narrow for
    /// a wall loop (up to two line widths wide).
    pub width: Option<f64>,
    /// Corners the travel to this path goes round, between the end of the
    /// previous path and the start of this one; empty for a straight move
    /// (see `Settings::avoid_crossing_walls`).
    pub travel: Vec<Point>,
}

impl Path {
    /// Extruded length in millimetres.
    pub fn length(&self) -> f64 {
        line_length(&self.points, self.closed)
    }
}

/// Length of a line through `points`, back to the first one if `closed`.
fn line_length(points: &[Point], closed: bool) -> f64 {
    let open: f64 = points.windows(2).map(|w| distance(w[0], w[1])).sum();
    match (closed, points.first(), points.last()) {
        (true, Some(&first), Some(&last)) => open + distance(last, first),
        _ => open,
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
    /// Vase mode: the layer's one outer wall rises from the height of the
    /// layer below to `z` as it goes round (see `Settings::spiral_vase`).
    pub spiral: bool,
}

/// The complete print, layer by layer from the bed up.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Toolpaths {
    /// Layers from bottom to top. With `Settings::sequential_print` the
    /// layers of every object follow those of the object printed before;
    /// a layer no higher than the one before it starts the next object.
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
    /// An object placed as given reaches over the edge of the bed.
    OutsideBed {
        /// Index of the object in the list, counted from 0.
        object: usize,
        /// Smallest X and Y, then largest X and Y of the object.
        footprint: [f64; 4],
        /// Bed size in X and Y.
        bed: [f64; 2],
    },
    /// With `Settings::sequential_print`, two objects stand closer together
    /// than `extruder_clearance_radius`, so the print head would hit the one
    /// printed first.
    ObjectsTooClose {
        /// Indices of the two objects in the list, counted from 0.
        objects: [usize; 2],
        /// The clearance asked for, in millimetres.
        clearance: f64,
    },
    /// With `Settings::sequential_print`, an object that is not the last
    /// one printed is taller than `gantry_height`, so the gantry would hit
    /// it while the next one prints.
    ObjectTooTall {
        /// Index of the object in the list, counted from 0.
        object: usize,
        /// Height of the object in millimetres.
        height: f64,
        /// The gantry height, in millimetres.
        gantry: f64,
    },
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
            Self::OutsideBed {
                object,
                footprint: [x0, y0, x1, y1],
                bed,
            } => write!(
                f,
                "object {} reaches over the edge of the bed: it spans X {x0:.1} to {x1:.1} and Y {y0:.1} to {y1:.1} mm, the bed {:.1} x {:.1} mm",
                object + 1,
                bed[0],
                bed[1]
            ),
            Self::ObjectsTooClose {
                objects: [a, b],
                clearance,
            } => write!(
                f,
                "objects {} and {} stand closer together than extruder_clearance_radius ({clearance:.1} mm); move them apart to print them one after the other",
                a + 1,
                b + 1
            ),
            Self::ObjectTooTall {
                object,
                height,
                gantry,
            } => write!(
                f,
                "object {} ({height:.1} mm) is taller than gantry_height ({gantry:.1} mm), and only the tallest object can be printed last; print the objects together or lower them",
                object + 1
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
            Self::Cancelled => "slicing.cancelled",
            Self::OutsideBed { .. } => "slicing.outside_bed",
            Self::ObjectsTooClose { .. } => "slicing.objects_too_close",
            Self::ObjectTooTall { .. } => "slicing.object_too_tall",
        }
    }

    fn context(&self) -> Vec<(&'static str, String)> {
        match self {
            Self::TooLarge { model, volume } => {
                let text = |v: &[f64; 3]| format!("{:.3} x {:.3} x {:.3}", v[0], v[1], v[2]);
                vec![("model_size", text(model)), ("build_volume", text(volume))]
            }
            Self::OutsideBed {
                object,
                footprint: f,
                bed,
            } => vec![
                ("object", (object + 1).to_string()),
                (
                    "footprint",
                    format!("{:.3} {:.3} {:.3} {:.3}", f[0], f[1], f[2], f[3]),
                ),
                ("bed_size", format!("{:.3} x {:.3}", bed[0], bed[1])),
            ],
            Self::ObjectsTooClose {
                objects: [a, b],
                clearance,
            } => vec![
                ("objects", format!("{} {}", a + 1, b + 1)),
                ("clearance", format!("{clearance:.3}")),
            ],
            Self::ObjectTooTall {
                object,
                height,
                gantry,
            } => vec![
                ("object", (object + 1).to_string()),
                ("height", format!("{height:.3}")),
                ("gantry_height", format!("{gantry:.3}")),
            ],
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
    generate_with_progress(mesh, settings, diagnostics, stop, &Progress::new())
}

/// Like [`generate_cancellable`], and reports how far it has got in
/// `progress`, up to the end of [`Step::Ordering`]; writing the G-code is
/// the last step.
pub fn generate_with_progress(
    mesh: &Mesh,
    settings: &Settings,
    diagnostics: &mut Diagnostics,
    stop: &AtomicBool,
    progress: &Progress,
) -> Result<Toolpaths, FdmError> {
    let objects = std::slice::from_ref(mesh);
    generate_plate_with_progress(
        objects,
        Placement::Centre,
        settings,
        diagnostics,
        stop,
        progress,
    )
}

/// Slices several objects on one build plate and generates toolpaths, like
/// [`generate_with_progress`] does for one. `placement` says where the
/// objects go; [`arrange`] finds positions for [`Placement::AsGiven`].
/// Objects whose outlines overlap seen from above are reported and printed
/// as one.
// ponytail: printed together, the objects share the layer heights of the
// whole plate (`sequential_print` prints one after the other). Cutting the
// mesh counts as one
// part of the progress; reporting from inside `slice_mesh_reporting` is
// the upgrade there.
pub fn generate_plate(
    objects: &[Mesh],
    placement: Placement,
    settings: &Settings,
    diagnostics: &mut Diagnostics,
    stop: &AtomicBool,
) -> Result<Toolpaths, FdmError> {
    let progress = Progress::new();
    generate_plate_with_progress(objects, placement, settings, diagnostics, stop, &progress)
}

/// Like [`generate_plate`], and reports how far it has got in `progress`.
pub fn generate_plate_with_progress(
    objects: &[Mesh],
    placement: Placement,
    settings: &Settings,
    diagnostics: &mut Diagnostics,
    stop: &AtomicBool,
    progress: &Progress,
) -> Result<Toolpaths, FdmError> {
    let own = [];
    generate_plate_with_object_settings(
        objects,
        &own,
        placement,
        settings,
        diagnostics,
        stop,
        progress,
    )
}

/// Like [`generate_plate_with_progress`], and object `i` prints with its
/// own settings `own[i]` where given: its walls, top and bottom shells,
/// infill and whether it gets supports follow the keys in
/// [`OBJECT_SETTINGS`] (see [`object_settings`]). All other settings of
/// `own[i]` are ignored; layer heights, skirt, brim, the shape of supports,
/// speeds and travel are those of `settings` for the whole plate. Objects
/// without own settings, or with values equal to the plate's, print
/// exactly as with [`generate_plate_with_progress`].
///
/// With `Settings::sequential_print` and more than one object, every object
/// is generated on its own and its layers follow those of the object before
/// (see [`Toolpaths`]).
pub fn generate_plate_with_object_settings(
    objects: &[Mesh],
    own: &[Option<Settings>],
    placement: Placement,
    settings: &Settings,
    diagnostics: &mut Diagnostics,
    stop: &AtomicBool,
    progress: &Progress,
) -> Result<Toolpaths, FdmError> {
    if settings.sequential_print && objects.len() > 1 {
        return sequential::generate(
            objects,
            own,
            placement,
            settings,
            diagnostics,
            stop,
            progress,
        );
    }
    let stopped = || stop.load(Ordering::Relaxed);
    // Vase mode prints one wall and nothing inside or on top, the same for
    // every object.
    let vase;
    let (settings, own) = if settings.spiral_vase {
        vase = Settings {
            wall_loops: 1,
            top_layers: 0,
            infill_density: 0.0,
            supports: false,
            ..settings.clone()
        };
        (&vase, &[][..])
    } else {
        (settings, own)
    };
    // Objects with equal settings form a group. With a single group its
    // settings are those of the whole plate.
    let (groups, membership) = objects::groups(objects.len(), own, settings);
    let settings = match groups.as_slice() {
        [only] => only,
        _ => settings,
    };
    let any_supports = groups.iter().any(|g| g.supports);
    // Turning, removing degenerate triangles, closing holes, checking.
    progress.start(Step::Preparing, 4);
    plate::check(objects, placement, settings, diagnostics)?;
    let mut mesh = plate::merge(objects, placement);
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
    progress.advance();
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
    progress.advance();
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
    progress.advance();
    mesh.check(diagnostics);
    progress.advance();
    let count = settings.extruder_count;
    let mut used = mesh.used_extruders();
    // Objects asking for an extruder the printer does not have print with
    // the first one.
    let missing: Vec<String> = used
        .iter()
        .filter(|&&e| count > 1 && e >= count)
        .map(|e| (e + 1).to_string())
        .collect();
    if !missing.is_empty() {
        diagnostics.push(
            Diagnostic::warning(
                "slicing.missing_extruder",
                format!(
                    "objects of the model ask for extruder(s) {}, but the printer has {count} (extruder_count); they are printed with extruder 1",
                    missing.join(", ")
                ),
            )
            .with("extruders", missing.join(" "))
            .with("extruder_count", count),
        );
        for e in &mut mesh.extruders {
            if *e >= count {
                *e = 0;
            }
        }
        used = mesh.used_extruders();
    }
    // With one extruder it prints everything, the lowest one the model asks
    // for. Otherwise this one prints skirt, brim and supports.
    let extruder = used.first().copied().unwrap_or(0);
    if used.len() > 1 && count == 1 {
        let list: Vec<String> = used.iter().map(|e| (e + 1).to_string()).collect();
        diagnostics.push(
            Diagnostic::warning(
                "slicing.multiple_extruders",
                format!(
                    "the objects of the model ask for extruders {}; the printer has one extruder (extruder_count), everything is printed with extruder {}",
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
    let shift = match placement {
        Placement::Centre => Some([
            volume[0] / 2.0 - (min[0] + max[0]) / 2.0,
            volume[1] / 2.0 - (min[1] + max[1]) / 2.0,
            -min[2],
        ]),
        // Every object is on the bed already.
        Placement::AsGiven => None,
    };
    if let Some(shift) = shift {
        mesh.translate(shift);
    }

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
    progress.start(Step::Cutting, 1);
    let slices = slice_mesh_reporting(&mesh, &cut_heights, diagnostics);
    progress.advance();
    if stopped() {
        return Err(FdmError::Cancelled);
    }
    if slices.iter().all(Vec::is_empty) {
        return Err(FdmError::NothingToPrint);
    }
    // With several groups of objects every group gets its own region of
    // every layer, which it prints with its settings.
    let object_regions = if groups.len() > 1 {
        objects::regions(
            objects,
            placement,
            &membership,
            groups.len(),
            shift,
            &cut_heights,
        )
    } else {
        Vec::new()
    };
    // Where only some groups have supports, those only go under them, and
    // only the others are warned about.
    let mixed = !object_regions.is_empty() && groups.iter().any(|g| !g.supports) && any_supports;
    let (supported, unsupported) = if mixed {
        (
            Some(objects::with_supports(&object_regions, &groups, true)),
            Some(objects::with_supports(&object_regions, &groups, false)),
        )
    } else {
        (None, None)
    };

    // The geometry of a layer depends only on the slices, so all cores work
    // on it. Putting the pieces in printing order depends on where the
    // previous layer ended and happens one layer after the other.
    // What every layer has beyond the layer below. With supports the limit
    // is half a line width (about 50 degrees from vertical at the default
    // layer height); without, only a full line width is worth a warning.
    progress.start(Step::Geometry, slices.len());
    let (reach, least) = if any_supports {
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
    let supports = if any_supports {
        // Short bridges carry themselves.
        let carried: Vec<Shapes> = parallel_map(slices.len(), |index| {
            let carried: Shapes = hanging[index]
                .iter()
                .filter(|shape| !spans_freely(shape, &slices[index - 1], settings))
                .cloned()
                .collect();
            match &supported {
                Some(supported) => polygon::intersection(&carried, &supported[index]),
                None => carried,
            }
        });
        support_areas(&slices, &carried, &cut_heights, settings)
    } else {
        Vec::new()
    };
    // With several extruders every one of them gets its own region: the
    // part of the layer that its objects cover. Where objects of different
    // extruders overlap, the higher extruder takes the overlap, so that a
    // part placed into another one (an inlay, a label) keeps its colour.
    let extruder_regions: Vec<(u32, Vec<Shapes>)> = if count > 1 && used.len() > 1 {
        let own: Vec<Vec<Shapes>> = used
            .iter()
            .map(|&e| slice_mesh(&with_extruder(&mesh, e), &cut_heights))
            .collect();
        (0..own.len())
            .map(|i| {
                let region = parallel_map(slices.len(), |index| {
                    own[i + 1..]
                        .iter()
                        .fold(own[i][index].clone(), |region, later| {
                            polygon::difference(&region, &later[index])
                        })
                });
                (used[i], region)
            })
            .collect()
    } else {
        Vec::new()
    };
    // Every region with its extruder and settings; empty if one extruder
    // prints the whole model with the plate's settings.
    let regions = objects::split(
        extruder_regions,
        object_regions,
        &groups,
        extruder,
        settings,
    );
    if stopped() {
        return Err(FdmError::Cancelled);
    }
    // What every extruder prints on a layer, in ascending order of the
    // extruders; skirt, brim and supports go first with the lowest one.
    let geometry = parallel_map(slices.len(), |index| {
        if stopped() {
            return (Vec::new(), false);
        }
        progress.advance();
        let mut groups = vec![(extruder, shared_work(&slices, &supports, index, settings))];
        if regions.is_empty() {
            let (work, thin) = region_work(&slices, &slices, &heights, index, settings);
            groups[0].1.extend(work);
            return (groups, thin);
        }
        let mut thin = false;
        for (e, region_settings, region) in &regions {
            if region[index].is_empty() {
                continue;
            }
            let (work, region_thin) =
                region_work(region, &slices, &heights, index, region_settings);
            thin |= region_thin;
            match groups.iter_mut().find(|(g, _)| g == e) {
                Some(group) => group.1.extend(work),
                None => groups.push((*e, work)),
            }
        }
        groups.retain(|(_, work)| !work.is_empty());
        (groups, thin)
    });
    if stopped() {
        return Err(FdmError::Cancelled);
    }
    // The extruder that finished a layer starts the next one; the others
    // follow in ascending order.
    let mut tool = extruder;
    let mut orders: Vec<Vec<u32>> = Vec::new();
    for (groups, _) in &geometry {
        let mut order: Vec<u32> = groups.iter().map(|(e, _)| *e).collect();
        if let Some(at) = order.iter().position(|&e| e == tool) {
            order[..=at].rotate_right(1);
        }
        tool = order.last().copied().unwrap_or(tool);
        orders.push(order);
    }
    let tower = PrimeTower::plan(&orders, settings);
    if let Some(tower) = &tower
        && slices[..tower.layers]
            .iter()
            .any(|slice| !polygon::intersection(slice, &tower.footprint).is_empty())
    {
        diagnostics.push(
            Diagnostic::warning(
                "slicing.prime_tower_overlap",
                format!(
                    "the prime tower at X {} Y {} overlaps the model; move it with prime_tower_x and prime_tower_y",
                    settings.prime_tower_x, settings.prime_tower_y
                ),
            )
            .with("x", settings.prime_tower_x)
            .with("y", settings.prime_tower_y),
        );
    }
    // Every layer is counted twice: once in order, once for the travels.
    progress.start(Step::Ordering, 2 * slices.len());
    let mut position: Point = [0.0, 0.0];
    let mut thin_layers = Vec::new();
    let mut layers: Vec<Layer> = geometry
        .into_iter()
        .zip(orders)
        .enumerate()
        .map(|(index, ((mut groups, mut thin), order))| {
            let mut paths = Vec::new();
            if stopped() {
                groups.clear();
            }
            for e in order {
                let Some(at) = groups.iter().position(|(g, _)| *g == e) else {
                    continue;
                };
                let (_, work) = groups.swap_remove(at);
                let start = paths.len();
                if let Some(tower) = &tower {
                    tower.push(index, e, &mut position, &mut paths);
                }
                for piece in work {
                    push_work(piece, settings, &mut position, &mut paths);
                }
                for path in &mut paths[start..] {
                    path.extruder = e;
                }
            }
            // A layer with material but without any outer wall prints nothing.
            if !slices[index].is_empty() && paths.iter().all(|p| p.kind != PathKind::OuterWall) {
                thin = true;
            }
            if thin {
                thin_layers.push(index);
            }
            progress.advance();
            Layer {
                z: tops[index],
                height: heights[index],
                spiral: settings.spiral_vase && index >= settings.bottom_layers as usize,
                paths,
            }
        })
        .collect();
    if stopped() {
        return Err(FdmError::Cancelled);
    }
    assign_objects(
        &mut layers,
        objects,
        placement,
        shift,
        &cut_heights,
        settings,
    );
    if settings.overhang_speed > 0.0 {
        let free = match settings.overhang_speed_full {
            speed if speed > 0.0 => 0.5 * settings.line_width,
            _ => 0.0,
        };
        let split = parallel_map(layers.len(), |index| match index {
            0 => None,
            _ if layers[index].spiral => None,
            _ => split_overhangs(
                &layers[index].paths,
                &slices[index],
                &slices[index - 1],
                free,
            ),
        });
        for (layer, paths) in layers.iter_mut().zip(split) {
            if let Some(paths) = paths {
                layer.paths = paths;
            }
        }
    }
    // Which travel moves stay over the part depends only on the finished
    // order of a layer, so all cores can work on it again.
    let inside = parallel_map(layers.len(), |index| {
        let flags = travels_inside(&layers[index], &slices[index]);
        let routes = if settings.avoid_crossing_walls {
            combed(&layers[index], &slices[index], &flags, settings)
        } else {
            Vec::new()
        };
        progress.advance();
        (flags, routes)
    });
    for (layer, (flags, routes)) in layers.iter_mut().zip(inside) {
        for (index, (path, flag)) in layer.paths.iter_mut().zip(flags).enumerate() {
            path.travel_inside = flag;
            if let Some(Some(route)) = routes.get(index) {
                path.travel.clone_from(route);
                path.travel_inside = true;
            }
        }
    }
    // Without supports, say where they would be needed.
    // ponytail: where only some objects have supports, what hangs free is
    // measured with the reach used for supports, half a line width, so
    // the others are warned about slightly shallower overhangs than
    // without any supports.
    let hanging = match &unsupported {
        Some(unsupported) => parallel_map(hanging.len(), |index| {
            let free = polygon::intersection(&hanging[index], &unsupported[index]);
            polygon::clean(free, MIN_OVERHANG_AREA, 0.0)
        }),
        None if any_supports => Vec::new(),
        None => hanging,
    };
    let hanging_layers: Vec<usize> = (0..hanging.len())
        .filter(|&index| !hanging[index].is_empty())
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

/// Sets [`Path::object`] of every path that belongs to an object: with one
/// object to it, with several and `label_objects` to the object whose part
/// of the layer the path starts in (see [`objects::regions`]), trying the
/// other points of the path if the first lies in none.
// ponytail: objects whose outlines overlap are printed as one, and a path
// running from one into the other belongs to the first one it starts in.
fn assign_objects(
    layers: &mut [Layer],
    objects: &[Mesh],
    placement: Placement,
    shift: Option<[f64; 3]>,
    cut_heights: &[f64],
    settings: &Settings,
) {
    let own = |path: &Path| {
        !matches!(
            path.kind,
            PathKind::Skirt | PathKind::Brim | PathKind::Support | PathKind::PrimeTower
        )
    };
    if objects.len() == 1 {
        for path in layers.iter_mut().flat_map(|l| &mut l.paths) {
            if own(path) {
                path.object = 0;
            }
        }
        return;
    }
    if !settings.label_objects {
        return;
    }
    let membership: Vec<usize> = (0..objects.len()).collect();
    let regions = objects::regions(
        objects,
        placement,
        &membership,
        objects.len(),
        shift,
        cut_heights,
    );
    let found = parallel_map(layers.len(), |index| {
        let object_of = |path: &Path| {
            path.points
                .iter()
                .find_map(|&p| regions.iter().position(|r| point_inside(&r[index], p)))
                .map_or(NO_OBJECT, |o| o as u32)
        };
        layers[index]
            .paths
            .iter()
            .map(|path| {
                if own(path) {
                    object_of(path)
                } else {
                    NO_OBJECT
                }
            })
            .collect::<Vec<u32>>()
    });
    for (layer, found) in layers.iter_mut().zip(found) {
        for (path, object) in layer.paths.iter_mut().zip(found) {
            path.object = object;
        }
    }
}

/// The triangles of `mesh` that `extruder` prints, with all vertices.
fn with_extruder(mesh: &Mesh, extruder: u32) -> Mesh {
    Mesh {
        vertices: mesh.vertices.clone(),
        triangles: (0..mesh.triangles.len())
            .filter(|&i| mesh.extruder(i) == extruder)
            .map(|i| mesh.triangles[i])
            .collect(),
        extruders: Vec::new(),
    }
}

/// Puts one piece of geometry in printing order and appends it to `paths`.
fn push_work(piece: Work, settings: &Settings, position: &mut Point, paths: &mut Vec<Path>) {
    match piece {
        Work::Loops(loops, kind, min_length) => {
            let wall = matches!(kind, PathKind::OuterWall | PathKind::InnerWall);
            let seam = if wall {
                settings.seam_position
            } else {
                SeamPosition::Nearest
            };
            push_loops(loops, kind, min_length, seam, position, paths);
        }
        Work::Lines(lines, kind, join_distance) => {
            push_lines(lines, kind, join_distance, position, paths);
        }
        Work::Polylines(lines, kind, join_distance) => {
            push_lines(lines, kind, join_distance, position, paths);
        }
        Work::Centred(lines, kind) => push_centred(lines, kind, position, paths),
        Work::Ordered(lines, kind) => {
            for line in lines {
                // Starting where the line starts makes it the nearest end.
                *position = line[0];
                push_lines(vec![line], kind, 0.0, position, paths);
            }
        }
    }
}

/// A square tower beside the model that every tool change primes the new
/// extruder into, so that it extrudes evenly again before it reaches the
/// model. It stands on every layer up to the last tool change; on a layer
/// with tool changes the extruders changed to share its lines, on a layer
/// without the extruder that starts the layer prints all of it.
// ponytail: solid on every layer whatever the extruders need to purge, and
// the skirt does not go around it. Purge volumes per pair of filaments and a
// sparse tower are the upgrade.
struct PrimeTower {
    /// The square on the bed.
    footprint: Shapes,
    /// For every layer of the tower, the extruders that print a share of
    /// it, in printing order. The first one also prints the outline.
    shares: Vec<Vec<u32>>,
    /// Number of layers the tower reaches up.
    layers: usize,
    /// Line width.
    width: f64,
}

impl PrimeTower {
    /// The tower for the extruders printing every layer in the given order,
    /// or `None` if there is no tool change or no tower.
    fn plan(orders: &[Vec<u32>], settings: &Settings) -> Option<Self> {
        if settings.extruder_count < 2 || settings.prime_tower_width <= 0.0 {
            return None;
        }
        let mut tool = orders.iter().flatten().next().copied()?;
        let mut shares = Vec::new();
        let mut layers = 0;
        for (index, order) in orders.iter().enumerate() {
            let changed: Vec<u32> = order
                .iter()
                .copied()
                .filter(|&e| std::mem::replace(&mut tool, e) != e)
                .collect();
            if changed.is_empty() {
                shares.push(order.first().copied().into_iter().collect());
            } else {
                layers = index + 1;
                shares.push(changed);
            }
        }
        if layers == 0 {
            return None;
        }
        shares.truncate(layers);
        let half = settings.prime_tower_width / 2.0;
        let (x, y) = (settings.prime_tower_x, settings.prime_tower_y);
        let square = vec![
            [x - half, y - half],
            [x + half, y - half],
            [x + half, y + half],
            [x - half, y + half],
        ];
        Some(Self {
            footprint: vec![vec![square]],
            shares,
            layers,
            width: settings.line_width,
        })
    }

    /// Appends the share of the tower that `extruder` prints on layer
    /// `index`, if any: every n-th line of a solid fill when n extruders
    /// share the layer.
    fn push(&self, index: usize, extruder: u32, position: &mut Point, paths: &mut Vec<Path>) {
        let Some(shares) = self.shares.get(index) else {
            return;
        };
        let Some(share) = shares.iter().position(|&e| e == extruder) else {
            return;
        };
        let width = self.width;
        if share == 0 {
            let outline = contours(&polygon::offset(&self.footprint, -0.5 * width));
            let kind = PathKind::PrimeTower;
            push_loops(outline, kind, width, SeamPosition::Nearest, position, paths);
        }
        let direction = if index.is_multiple_of(2) {
            HatchDirection::Diagonal45
        } else {
            HatchDirection::Diagonal135
        };
        let fill = polygon::offset(&self.footprint, -width);
        let lines: Vec<[Point; 2]> = polygon::hatch(&fill, width, direction, 0.5 * width)
            .into_iter()
            .enumerate()
            .filter(|(i, _)| i % shares.len() == share)
            .map(|(_, line)| line)
            .collect();
        // Neighbouring lines of a share are joined along the edge.
        let join = 2.0 * shares.len() as f64 * width;
        push_lines(lines, PathKind::PrimeTower, join, position, paths);
    }
}

/// For every path of a layer: whether the move to its start stays inside
/// the outline of the layer. Only asked for infill and inner walls; before
/// an outer wall the nozzle always retracts, to keep the visible surface
/// clean.
/// For every path of a layer whose travel would leave the part, the way
/// round inside it (see [`comb::route`]), if there is one; `None` where the
/// travel stays straight. The way keeps a quarter line width inside the
/// outline, over the outer wall at most.
fn combed(
    layer: &Layer,
    slice: &Shapes,
    inside: &[bool],
    settings: &Settings,
) -> Vec<Option<Vec<Point>>> {
    let region = polygon::offset(slice, -0.25 * settings.line_width);
    let mut routes = vec![None];
    for (index, pair) in layer.paths.windows(2).enumerate() {
        let (previous, path) = (&pair[0], &pair[1]);
        let from = if previous.closed {
            previous.points.first()
        } else {
            previous.points.last()
        };
        let route = match (from, path.points.first()) {
            (Some(&from), Some(&to))
                if !inside[index + 1] && previous.extruder == path.extruder =>
            {
                comb::route(&region, from, to)
            }
            _ => None,
        };
        routes.push(route);
    }
    routes
}

/// The paths of a layer with the parts of walls whose line middle lies
/// outside `below`, the layer below, turned into overhang walls; `None` if
/// nothing changes. Where a wall changes between overhanging and supported
/// it is cut into open pieces that follow on one another, starting where
/// the wall started. Short changes are evened out, see
/// [`MIN_OVERHANG_RUN`]. With a `free` distance above 0, the parts whose
/// line middle lies farther than that outside `below` become overhang
/// walls that hang free entirely.
fn split_overhangs(paths: &[Path], slice: &Shapes, below: &Shapes, free: f64) -> Option<Vec<Path>> {
    let hanging = polygon::difference(slice, below);
    if hanging.is_empty() {
        return None;
    }
    let free = (free > 0.0).then(|| polygon::difference(slice, &polygon::offset(below, free)));
    let mut out = Vec::with_capacity(paths.len());
    let mut changed = false;
    for path in paths {
        let wall = matches!(path.kind, PathKind::OuterWall | PathKind::InnerWall);
        if !wall || path.points.len() < 2 {
            out.push(path.clone());
            continue;
        }
        let mut corners = path.points.clone();
        if path.closed {
            corners.push(path.points[0]);
        }
        // Every piece: where it ends, whether that is a corner of the wall,
        // and whether it hangs.
        let mut pieces: Vec<(Point, bool, bool)> = Vec::new();
        let mut lengths = Vec::new();
        let mut middles = Vec::new();
        for pair in corners.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let at = |f: f64| [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f];
            let n = (distance(a, b) / OVERHANG_STEP).ceil().max(1.0) as usize;
            lengths.extend(std::iter::repeat_n(distance(a, b) / n as f64, n));
            for k in 1..=n {
                let middle = at((k as f64 - 0.5) / n as f64);
                middles.push(middle);
                pieces.push((
                    at(k as f64 / n as f64),
                    k == n,
                    point_inside(&hanging, middle),
                ));
            }
        }
        let mut flags: Vec<bool> = pieces.iter().map(|p| p.2).collect();
        even_out(&mut flags, &lengths, false);
        even_out(&mut flags, &lengths, true);
        // Of what hangs, what hangs free entirely: short stretches of it
        // hang only partly, short stretches that hang partly next to it
        // hang free.
        let mut whole = vec![false; flags.len()];
        if let Some(free) = &free {
            whole = middles.iter().map(|&m| point_inside(free, m)).collect();
            for (w, &hangs) in whole.iter_mut().zip(&flags) {
                *w &= hangs;
            }
            even_out(&mut whole, &lengths, true);
            for (w, &hangs) in whole.iter_mut().zip(&flags) {
                *w |= !hangs;
            }
            even_out(&mut whole, &lengths, false);
            for (w, &hangs) in whole.iter_mut().zip(&flags) {
                *w &= hangs;
            }
        }
        let kinds: Vec<PathKind> = flags
            .iter()
            .zip(&whole)
            .map(|(&hangs, &whole)| match (hangs, whole) {
                (_, true) => PathKind::OverhangWallFull,
                (true, false) => PathKind::OverhangWall,
                (false, false) => path.kind,
            })
            .collect();
        if kinds.iter().all(|&k| k == path.kind) {
            out.push(path.clone());
            continue;
        }
        changed = true;
        if kinds.iter().all(|&k| k == kinds[0]) {
            out.push(Path {
                kind: kinds[0],
                ..path.clone()
            });
            continue;
        }
        let piece = |points: Vec<Point>, kind: PathKind| Path {
            kind,
            points,
            closed: false,
            travel_inside: false,
            extruder: path.extruder,
            object: path.object,
            width: path.width,
            travel: Vec::new(),
        };
        let mut run = vec![corners[0]];
        let mut kind = kinds[0];
        let mut last = corners[0];
        for (&(end, corner, _), &piece_kind) in pieces.iter().zip(&kinds) {
            if piece_kind != kind {
                if run.last() != Some(&last) {
                    run.push(last);
                }
                out.push(piece(std::mem::replace(&mut run, vec![last]), kind));
                kind = piece_kind;
            }
            if corner {
                run.push(end);
            }
            last = end;
        }
        out.push(piece(run, kind));
    }
    changed.then_some(out)
}

/// Flips runs of `flags` that are shorter than [`MIN_OVERHANG_RUN`] (piece
/// `i` is `lengths[i]` long): runs of `false` between two runs of `true` if
/// `hangs` is false, otherwise runs of `true` anywhere.
// ponytail: the ends of a closed wall are evened out as if it were open, so
// an overhang across the seam counts as two shorter ones.
fn even_out(flags: &mut [bool], lengths: &[f64], hangs: bool) {
    let mut start = 0;
    while start < flags.len() {
        let end = (start..flags.len())
            .find(|&i| flags[i] != flags[start])
            .unwrap_or(flags.len());
        let short = lengths[start..end].iter().sum::<f64>() < MIN_OVERHANG_RUN;
        let between = start > 0 && end < flags.len();
        if flags[start] == hangs && short && (hangs || between) {
            flags[start..end].fill(!hangs);
        }
        start = end;
    }
}

fn travels_inside(layer: &Layer, slice: &Shapes) -> Vec<bool> {
    // The first path of a layer is reached from the layer below.
    let mut flags = vec![false];
    for pair in layer.paths.windows(2) {
        let (previous, path) = (&pair[0], &pair[1]);
        let hidden = matches!(
            path.kind,
            PathKind::InnerWall
                | PathKind::GapFill
                | PathKind::SolidInfill
                | PathKind::Bridge
                | PathKind::SparseInfill
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
/// dense part is what lies right under a carried surface. Tree supports
/// (`support_style = tree`) replace the columns below the dense part by
/// branches.
// ponytail: the gaps are one layer whatever its thickness, so with adaptive
// layers they are as thin as the layers there.
fn support_areas(
    slices: &[Shapes],
    hanging: &[Shapes],
    cut_heights: &[f64],
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
    let areas = match settings.support_style {
        SupportStyle::Normal => column_areas(hanging, &keep_out),
        SupportStyle::Tree => tree::areas(hanging, &keep_out, cut_heights, settings),
    };
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

/// Straight columns under everything that hangs free, down to the bed or to
/// the model, clear of `keep_out`.
fn column_areas(hanging: &[Shapes], keep_out: &[Shapes]) -> Vec<Shapes> {
    let mut areas: Vec<Shapes> = vec![Vec::new(); hanging.len()];
    for index in (0..hanging.len().saturating_sub(2)).rev() {
        let above = polygon::union(&hanging[index + 2], &areas[index + 1]);
        if above.is_empty() {
            continue;
        }
        areas[index] = tidy_support(polygon::difference(&above, &keep_out[index]));
    }
    areas
}

/// Drops support pieces too small to print and thins out the outlines.
/// Every layer adds the points of its own outline. Without thinning them out
/// a support under a round overhang ends up with tens of thousands of points
/// and takes seconds per model.
fn tidy_support(area: Shapes) -> Shapes {
    polygon::clean(area, MIN_SUPPORT_AREA, 0.0)
        .iter()
        .map(|shape| {
            shape
                .iter()
                .map(|contour| polygon::simplify(contour, SUPPORT_RESOLUTION))
                .filter(|contour| contour.len() > 2)
                .collect()
        })
        .collect()
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
    /// Lines along the middle of narrow regions, and their kind.
    Centred(Vec<CentreLine>, PathKind),
    /// Line segments printed in the given order and direction, each a path
    /// of its own, and their kind.
    Ordered(Vec<[Point; 2]>, PathKind),
}

/// A line along the middle of a region too narrow for a wall: its points,
/// whether it returns to its first point, and the width it is extruded
/// with.
type CentreLine = (Vec<Point>, bool, f64);

/// Generates what a layer prints for the whole model, whatever extruder its
/// parts take: skirt and brim on the first layer, and supports.
fn shared_work(
    slices: &[Shapes],
    supports: &[(Shapes, Shapes)],
    index: usize,
    settings: &Settings,
) -> Vec<Work> {
    let width = settings.line_width;
    let mut work = Vec::new();
    let support = supports.get(index).filter(|(area, _)| !area.is_empty());
    if index == 0 {
        adhesion_work(
            &slices[0],
            support.map(|(area, _)| area),
            settings,
            &mut work,
        );
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
    work
}

/// Generates the walls and infill of one layer of a region, in the order
/// they are printed. `slices` is the region on every layer, `model` the
/// whole model, which carries the region where it bridges. The flag tells
/// whether a noticeable part of the layer is too narrow to be covered by a
/// wall.
fn region_work(
    slices: &[Shapes],
    model: &[Shapes],
    heights: &[f64],
    index: usize,
    settings: &Settings,
) -> (Vec<Work>, bool) {
    // The first layer squashes out wider than the model; it is printed
    // that much smaller.
    let shrunk;
    let slice = if index == 0 && settings.elephant_foot_compensation > 0.0 {
        shrunk = polygon::offset(&slices[0], -settings.elephant_foot_compensation);
        &shrunk
    } else {
        &slices[index]
    };
    let width = settings.line_width;
    let walls = settings.wall_loops as usize;
    let mut work = Vec::new();

    // Walls: centre lines half a width inside the contour, then one width
    // further in for every additional loop. Printed from the inside out.
    let mut wall_shapes: Vec<Shapes> = (0..walls)
        .map(|k| polygon::offset(slice, -(0.5 + k as f64) * width))
        .collect();
    // Where the area a wall has to cover (the slice, or what lies inside the
    // previous wall) is between one and two widths wide, the wall's centre
    // area is narrower than a width: its loop runs out and back with its
    // sides closer than a width and deposits up to twice the material. Those
    // parts are what shrinking the centre area by half a width and growing it
    // back does not reach; long ones are taken out of the wall, so that the
    // missed regions below get a line of their own width instead.
    for centre in &mut wall_shapes {
        if centre.is_empty() {
            break;
        }
        let shrunk = polygon::offset(centre, -(0.5 * width - FOLD_MARGIN));
        let folds: Shapes = polygon::difference(centre, &polygon::offset(&shrunk, 0.5 * width))
            .into_iter()
            .filter(|shape| line_length(&shape[0], true) >= 2.0 * MIN_FOLD_LENGTH * width)
            .collect();
        if !folds.is_empty() {
            *centre = polygon::difference(centre, &polygon::offset(&folds, FOLD_MARGIN));
        }
    }

    // Infill centre lines stay half a width away from the inner edge of the
    // innermost wall, so line ends and the links between lines do not
    // deposit material twice. The overlap setting pushes them back out.
    let overlap = settings.infill_overlap / 100.0;
    let inset = (walls as f64 + 0.5 - overlap) * width;
    let fill_area = polygon::offset(slice, -inset);

    // Wall k covers its centre line grown by half a width, but nothing of
    // the area k + 1 widths inside the contour; the infill covers the fill
    // area grown back to the innermost wall. So what the area k widths
    // inside (the slice itself for k = 0) has beyond the grown wall k, or
    // beyond the grown fill area for k = walls, is covered by nothing:
    // regions too narrow for the outer wall (thin walls), gaps between
    // walls and gaps between the walls and the infill. Rounded corners
    // always lose a little; only regions wide and long enough get a line.
    let reaches = wall_shapes
        .iter()
        .map(|centre| (centre, 0.5 * width))
        .chain(std::iter::once((
            &fill_area,
            (0.5 - overlap).max(0.0) * width,
        )));
    let mut thin_lines = Vec::new();
    let mut gap_lines = Vec::new();
    let total = polygon::area(slice);
    let mut covered = total;
    for (k, (centre, reach)) in reaches.enumerate() {
        let eroded;
        let inside = if k == 0 {
            slice
        } else {
            eroded = polygon::offset(slice, -(k as f64) * width);
            &eroded
        };
        // Further in there is nothing either.
        if inside.is_empty() {
            break;
        }
        let inside_area = polygon::area(inside);
        // Growing a shape is slow; most of the time an estimate shows that
        // nothing worth a line is missed.
        let least = MIN_THIN_WIDTH * width * width;
        let estimate = inside_area - grown_area(centre, reach) - corner_loss(inside, reach);
        let reached = if estimate < least {
            None
        } else {
            Some(polygon::offset(centre, reach))
        };
        let missed_area = reached
            .as_ref()
            .map_or(estimate, |r| inside_area - polygon::area(r));
        if k == 0 {
            covered = total - missed_area.max(0.0);
        }
        let Some(reached) = reached.filter(|_| missed_area >= least) else {
            continue;
        };
        let (lines, regions) = centre_lines(polygon::difference(inside, &reached), width);
        if k == 0 {
            covered += polygon::area(&regions);
            thin_lines = lines;
        } else {
            gap_lines.extend(lines);
        }
    }
    // What the outer wall and the lines in narrow regions do not cover is
    // lost. Report only a noticeable share.
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
        let mut loops: Vec<Contour> = contours(&wall_shapes[k])
            .iter()
            .map(|contour| polygon::simplify(contour, PATH_RESOLUTION))
            .collect();
        // The first layer stays smooth, so that it sticks.
        if k == 0 && settings.fuzzy_skin && index > 0 {
            loops = loops
                .iter()
                .enumerate()
                .map(|(number, contour)| fuzz(contour, index, number, settings))
                .collect();
        }
        work.push(Work::Loops(loops, kind, width));
        if k == 0 && !thin_lines.is_empty() {
            work.push(Work::Centred(std::mem::take(&mut thin_lines), kind));
        }
    }
    if !gap_lines.is_empty() {
        work.push(Work::Centred(gap_lines, PathKind::GapFill));
    }

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
            split_bridges(&model[index - 1], &fill_area, solid, sparse, width);
        for (bridge, across) in bridges {
            let lines = polygon::hatch(&bridge, width, across, min_length);
            work.push(Work::Lines(lines, PathKind::Bridge, 2.0 * width));
        }
        (solid, sparse)
    } else {
        (solid, sparse)
    };
    // The bottom surface on the bed and the top surfaces may have a pattern
    // of their own.
    let (pattern, above) = match index {
        0 => (settings.bottom_surface_pattern, None),
        _ => (settings.top_surface_pattern, model.get(index + 1)),
    };
    let angle = settings.infill_angle + turn;
    let (solid, surface) = infill::surface(solid, above, pattern, angle, width);
    let solid_lines = polygon::hatch(&solid, width, direction, min_length);
    // Joined by straight links; following the boundary instead (as sparse
    // rectilinear infill does) saves a few travels but adds many points.
    work.push(Work::Lines(solid_lines, PathKind::SolidInfill, 2.0 * width));
    work.extend(surface);

    let density = settings.infill_density / 100.0;
    if density > 0.0 {
        if density >= 1.0 {
            let lines = polygon::hatch(&sparse, width, direction, min_length);
            work.push(Work::Lines(lines, PathKind::SolidInfill, 2.0 * width));
        } else {
            match settings.infill_pattern {
                InfillPattern::Rectilinear => {
                    // Line ends up to two spacings apart along the boundary
                    // are connected; the other patterns would cross lines.
                    let spacing = width / density;
                    let lines = polygon::hatch(&sparse, spacing, direction, min_length);
                    if settings.infill_connect {
                        let lines = infill::connect(
                            &sparse,
                            &lines,
                            2.0 * spacing,
                            &fill_area,
                            width / 4.0,
                        );
                        work.push(Work::Polylines(lines, PathKind::SparseInfill, 0.0));
                    } else {
                        work.push(Work::Lines(lines, PathKind::SparseInfill, 0.0));
                    }
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
                    let angle = settings.infill_angle;
                    let lines = infill::gyroid(&sparse, width / density, z, angle, min_length);
                    work.push(Work::Polylines(lines, PathKind::SparseInfill, 0.0));
                }
                InfillPattern::Honeycomb => {
                    let angle = settings.infill_angle;
                    let lines = infill::honeycomb(&sparse, width / density, angle, min_length);
                    work.push(Work::Polylines(lines, PathKind::SparseInfill, 0.0));
                }
                InfillPattern::Cubic => {
                    // Three directions share the material, like triangles.
                    let z = settings.first_layer_height + index as f64 * settings.layer_height;
                    let spacing = 3.0 * width / density;
                    let angle = settings.infill_angle;
                    for lines in infill::cubic(&sparse, spacing, z, angle, min_length) {
                        work.push(Work::Lines(lines, PathKind::SparseInfill, 0.0));
                    }
                }
                InfillPattern::Concentric => {
                    let (loops, _) = infill::concentric(&sparse, width / density, width, false);
                    work.push(Work::Loops(loops, PathKind::SparseInfill, width));
                }
            }
        }
    }
    if settings.ironing {
        // The part of the fill area that nothing covers: the top surface.
        // The lines cross those of the solid infill below them.
        let top = match model.get(index + 1) {
            Some(above) => polygon::difference(&fill_area, above),
            None => fill_area.clone(),
        };
        // Only the topmost surface: the parts of the model that end on
        // this layer, with nothing of them on the layer above.
        let top = match (settings.ironing_type, model.get(index + 1)) {
            (slicer_profiles::IroningType::Top, Some(above)) => {
                let ending: Shapes = model[index]
                    .iter()
                    .filter(|&part| polygon::intersection(&vec![part.clone()], above).is_empty())
                    .cloned()
                    .collect();
                polygon::intersection(&top, &ending)
            }
            _ => top,
        };
        let top = polygon::clean(top, width * width, 0.0);
        if !top.is_empty() {
            let lines = polygon::hatch(&top, settings.ironing_spacing, other, min_length);
            work.push(Work::Lines(lines, PathKind::Ironing, 2.0 * width));
        }
    }
    (work, thin)
}

/// `contour` with a point every `fuzzy_skin_point_distance` along it, each
/// moved sideways by a random amount of up to half the
/// `fuzzy_skin_thickness` either way. The amounts come from the layer, the
/// number of the contour and of the point, so every run is the same.
fn fuzz(contour: &[Point], layer: usize, number: usize, settings: &Settings) -> Contour {
    let step = settings.fuzzy_skin_point_distance;
    let half = settings.fuzzy_skin_thickness / 2.0;
    let mut points = Vec::new();
    let mut count: u64 = 0;
    for (i, &a) in contour.iter().enumerate() {
        let b = contour[(i + 1) % contour.len()];
        let length = distance(a, b);
        if length < 1e-9 {
            continue;
        }
        // Right of the direction of travel: away from the material.
        let normal = [(b[1] - a[1]) / length, -(b[0] - a[0]) / length];
        let parts = (length / step).ceil().max(1.0) as usize;
        for k in 0..parts {
            let t = k as f64 / parts as f64;
            let shift = (2.0 * noise(layer as u64, number as u64, count) - 1.0) * half;
            count += 1;
            points.push([
                a[0] + (b[0] - a[0]) * t + normal[0] * shift,
                a[1] + (b[1] - a[1]) * t + normal[1] * shift,
            ]);
        }
    }
    points
}

/// A number from 0 to 1 that looks random but depends only on the three
/// inputs (SplitMix64 on their mix).
fn noise(a: u64, b: u64, c: u64) -> f64 {
    let mut x = a
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        .wrapping_add(b.wrapping_mul(0xc2b2_ae3d_27d4_eb4f))
        .wrapping_add(c.wrapping_mul(0x1656_67b1_9e37_79f9));
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^= x >> 31;
    (x >> 11) as f64 / (1u64 << 53) as f64
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
pub(crate) fn point_inside(shapes: &Shapes, point: Point) -> bool {
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

/// About the area of `shapes` grown by `reach` (Steiner's formula: area,
/// plus contour length times the reach, plus a disc of that radius per
/// shape and minus one per hole), less what the polygonal arcs of a real
/// offset may fall short of round ones.
// ponytail: exact only where the grown shape neither merges with itself nor
// fills a sharp inward corner; there it comes out too large, and a missed
// region of up to about reach² per such place goes unnoticed.
fn grown_area(shapes: &Shapes, reach: f64) -> f64 {
    let mut grown = polygon::area(shapes);
    for shape in shapes {
        for (i, contour) in shape.iter().enumerate() {
            let disc = std::f64::consts::PI * reach * reach;
            let disc = if i == 0 { disc } else { -disc };
            grown += reach * line_length(contour, true) + disc - ARC_SHORTFALL * reach;
        }
    }
    grown
}

/// At least two thirds of the area that rounding the outward corners of
/// `shapes` to a radius of `reach` cuts off, counting only corners where
/// the outline turns by up to 90 degrees. No line fits in what those lose.
fn corner_loss(shapes: &Shapes, reach: f64) -> f64 {
    let mut loss = 0.0;
    for contour in shapes.iter().flatten() {
        let n = contour.len();
        for j in 0..n {
            let (p, q, r) = (contour[(j + n - 1) % n], contour[j], contour[(j + 1) % n]);
            let (a, b) = ([q[0] - p[0], q[1] - p[1]], [r[0] - q[0], r[1] - q[1]]);
            let (cross, dot) = (a[0] * b[1] - a[1] * b[0], a[0] * b[0] + a[1] * b[1]);
            let lengths = distance(p, q) * distance(q, r);
            // Material lies to the left: a left turn is an outward corner.
            if cross <= 0.0 || dot < 0.0 || lengths == 0.0 {
                continue;
            }
            // Between corner and arc: reach² (tan x - x) for a turn by 2x.
            // As x < (2 sin x + tan x) / 3, two thirds of tan x - sin x are
            // less, and need no trigonometry.
            let tan = cross / (lengths + dot);
            let sin = ((1.0 - dot / lengths) / 2.0).max(0.0).sqrt();
            loss += reach * reach * (tan - sin) * 2.0 / 3.0;
        }
    }
    loss
}

/// Lines along the middle of the regions in `missed` that are at least
/// [`MIN_THIN_WIDTH`] line widths wide and give a line at least one line
/// width long, and the regions that got one. A region's lines are as wide
/// as its area divided by their length (at most [`MAX_CENTRED_WIDTH`] line
/// widths), so they
/// deposit its volume.
// ponytail: an approximate centre line, not a medial axis. A region is
// shrunk by half the minimum width; one without holes is cut at its two
// points farthest apart and its two sides are averaged, one with holes gets
// a loop halfway between its outer contour and its holes. A branching
// region (T, Y) gets a line along one side of the shrunk region instead,
// into the branch and back; a grid of thin walls gets only its outer loop;
// a region wider than a line in places still gets a single line.
fn centre_lines(missed: Shapes, width: f64) -> (Vec<CentreLine>, Shapes) {
    let least = MIN_THIN_WIDTH * width;
    let mut lines = Vec::new();
    let mut regions = Vec::new();
    for region in polygon::clean(missed, least * width, 0.0) {
        let region = vec![region];
        let mut found: Vec<(Vec<Point>, bool)> = Vec::new();
        for core in polygon::offset(&region, -0.5 * least) {
            if core.len() > 1 {
                // In a thin band, area over the length of all contours is
                // half its width.
                let contour_length: f64 = core.iter().map(|c| line_length(c, true)).sum();
                let half = polygon::area(std::slice::from_ref(&core)) / contour_length;
                let outline = vec![vec![core[0].clone()]];
                for shape in polygon::offset(&outline, -half) {
                    found.push((polygon::simplify(&shape[0], PATH_RESOLUTION), true));
                }
            } else {
                let outline = polygon::simplify(&core[0], PATH_RESOLUTION);
                found.push((midline(&outline, &region), false));
            }
        }
        found.retain(|(points, closed)| line_length(points, *closed) >= width);
        let length: f64 = found
            .iter()
            .map(|(p, closed)| line_length(p, *closed))
            .sum();
        if length > 0.0 {
            let line_width = (polygon::area(&region) / length).min(MAX_CENTRED_WIDTH * width);
            lines.extend(found.into_iter().map(|(p, closed)| (p, closed, line_width)));
            regions.extend(region);
        }
    }
    (lines, regions)
}

/// Middle line of a strip given by its outline: the outline is cut at its
/// two points farthest apart (found by two sweeps), and its two sides are
/// averaged at equal shares of their length. Where the average leaves
/// `region`, the strip branches, and the longer side is used instead.
fn midline(outline: &[Point], region: &Shapes) -> Vec<Point> {
    let n = outline.len();
    let farthest = |from: Point| {
        (0..n)
            .max_by(|&i, &j| distance(from, outline[i]).total_cmp(&distance(from, outline[j])))
            .unwrap_or(0)
    };
    let (Some(&start), true) = (outline.first(), n > 2) else {
        return outline.to_vec();
    };
    let a = farthest(start);
    let b = farthest(outline[a]);
    let steps = (b + n - a) % n;
    let one: Vec<Point> = (0..=steps).map(|i| outline[(a + i) % n]).collect();
    let other: Vec<Point> = (0..=n - steps).map(|i| outline[(a + n - i) % n]).collect();
    // Position along a side, from 0 at its start to 1 at its end.
    let shares = |side: &[Point]| {
        let mut run = vec![0.0];
        for pair in side.windows(2) {
            run.push(run[run.len() - 1] + distance(pair[0], pair[1]));
        }
        let total = run[run.len() - 1];
        if total > 0.0 {
            run.iter_mut().for_each(|s| *s /= total);
        }
        run
    };
    let (s1, s2) = (shares(&one), shares(&other));
    let at = |side: &[Point], s: &[f64], t: f64| {
        let i = s.partition_point(|&x| x < t).clamp(1, side.len() - 1);
        let span = s[i] - s[i - 1];
        let f = if span > 0.0 {
            ((t - s[i - 1]) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (p, q) = (side[i - 1], side[i]);
        [p[0] + f * (q[0] - p[0]), p[1] + f * (q[1] - p[1])]
    };
    let mut ts: Vec<f64> = s1.iter().chain(&s2).copied().collect();
    ts.sort_by(f64::total_cmp);
    ts.dedup_by(|x, y| *x - *y < 1e-9);
    let middle: Vec<Point> = ts
        .iter()
        .map(|&t| {
            let (p, q) = (at(&one, &s1, t), at(&other, &s2, t));
            [(p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0]
        })
        .collect();
    if middle
        .windows(2)
        .all(|w| polygon::segment_inside(region, w[0], w[1]))
    {
        middle
    } else if line_length(&one, false) >= line_length(&other, false) {
        one
    } else {
        other
    }
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
            object: NO_OBJECT,
            width: None,
            travel: Vec::new(),
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
                    object: NO_OBJECT,
                    width: None,
                    travel: Vec::new(),
                });
            }
        }
        *position = end;
    }
    paths.extend(current);
}

/// Appends lines along the middle of narrow regions, always continuing with
/// the nearest end of an open line or the nearest point of a loop.
fn push_centred(
    mut lines: Vec<CentreLine>,
    kind: PathKind,
    position: &mut Point,
    paths: &mut Vec<Path>,
) {
    while !lines.is_empty() {
        let mut best = (0, 0, f64::INFINITY);
        for (i, (points, closed, _)) in lines.iter().enumerate() {
            for (j, &p) in points.iter().enumerate() {
                let end = j == 0 || j + 1 == points.len();
                let d = distance(*position, p);
                if (*closed || end) && d < best.2 {
                    best = (i, j, d);
                }
            }
        }
        let (mut points, closed, width) = lines.remove(best.0);
        if closed {
            points.rotate_left(best.1);
        } else if best.1 > 0 {
            points.reverse();
        }
        *position = if closed {
            points[0]
        } else {
            points[points.len() - 1]
        };
        paths.push(Path {
            kind,
            points,
            closed,
            travel_inside: false,
            extruder: 0,
            object: NO_OBJECT,
            width: Some(width),
            travel: Vec::new(),
        });
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::field_reassign_with_default)]
    use super::*;
    use slicer_profiles::SurfacePattern;

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
    fn objects_placed_as_given_keep_their_positions() {
        let settings = Settings {
            skirt_loops: 0,
            ..Settings::default()
        };
        // One floats above the bed and is put down; the other is taller.
        let mut low = Mesh::cuboid(10.0, 10.0, 2.0);
        low.translate([20.0, 30.0, 7.0]);
        let mut tall = Mesh::cuboid(10.0, 20.0, 4.0);
        tall.translate([150.0, 100.0, 0.0]);
        let stop = AtomicBool::new(false);
        let mut diagnostics = Diagnostics::new();
        let objects = [low, tall];
        let toolpaths = generate_plate(
            &objects,
            Placement::AsGiven,
            &settings,
            &mut diagnostics,
            &stop,
        )
        .unwrap();
        assert!(diagnostics.is_empty(), "{:?}", diagnostics.codes());
        assert_eq!(toolpaths.layers.len(), 20);
        let centres = |layer: &Layer| -> Vec<Point> {
            layer
                .paths
                .iter()
                .filter(|p| p.kind == PathKind::OuterWall)
                .map(|p| {
                    let (lo, hi) =
                        p.points
                            .iter()
                            .fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), q| {
                                (
                                    [lo[0].min(q[0]), lo[1].min(q[1])],
                                    [hi[0].max(q[0]), hi[1].max(q[1])],
                                )
                            });
                    [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0]
                })
                .collect()
        };
        let mut first = centres(&toolpaths.layers[0]);
        first.sort_by(|a, b| a[0].total_cmp(&b[0]));
        assert_eq!(first.len(), 2);
        assert!(distance(first[0], [25.0, 35.0]) < 1e-6, "{first:?}");
        assert!(distance(first[1], [155.0, 110.0]) < 1e-6, "{first:?}");
        assert_eq!(centres(&toolpaths.layers[15]).len(), 1);

        // Centred, the two keep their distance and the group moves.
        let centred = generate_plate(
            &objects,
            Placement::Centre,
            &settings,
            &mut diagnostics,
            &stop,
        )
        .unwrap();
        assert_eq!(centres(&centred.layers[0]).len(), 1, "the low one floats");

        let mut off = objects.clone();
        off[1].translate([70.0, 0.0, 0.0]);
        let error = generate_plate(&off, Placement::AsGiven, &settings, &mut diagnostics, &stop)
            .unwrap_err();
        assert_eq!(slicer_core::Diagnose::code(&error), "slicing.outside_bed");
    }

    #[test]
    fn overlapping_shells_extrude_their_union_once() {
        // The gear of the reference models: a hub cylinder runs through the
        // gear. The mesh volume counts the overlap twice; the print must not.
        let mut gear = slicer_core::primitives::extrude(
            &slicer_core::primitives::gear_contour(16, 18.0, 3.0),
            [0.0, 0.0],
            6.0,
            1,
            0.0,
        );
        let hub = slicer_core::primitives::cylinder(8.0, 10.0, 48);
        let overlap = hub.volume() * 6.0 / 10.0;
        gear.merge(&hub);
        let mut settings = Settings::default();
        settings.infill_density = 100.0;
        settings.skirt_loops = 0;
        let toolpaths = generate(&gear, &settings).unwrap();
        let volume: f64 = toolpaths
            .layers
            .iter()
            .flat_map(|l| l.paths.iter().map(move |p| (l.height, p)))
            .map(|(h, p)| p.length() * p.width.unwrap_or(settings.line_width) * h)
            .sum();
        let union = gear.volume() - overlap;
        assert!((volume / union - 1.0).abs() < 0.03, "{volume} of {union}");
        assert!(
            volume / gear.volume() < 0.9,
            "the overlap was printed twice"
        );
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
                    // Longer than the connections along the boundary, which
                    // are at most two spacings (6 mm).
                    if distance(pair[0], pair[1]) > 6.5 {
                        long += 1;
                        assert!((dx * along[1] - dy * along[0]).abs() < 1e-6, "{dx} {dy}");
                    }
                }
            }
            assert!(long > 3, "{long}");
        }
    }

    #[test]
    fn rectilinear_lines_are_connected_along_the_boundary() {
        let mut settings = Settings::default();
        settings.infill_pattern = InfillPattern::Rectilinear;
        let toolpaths = generate(&Mesh::cuboid(30.0, 30.0, 6.0), &settings).unwrap();
        // The sparse area is a square around the centre of the bed.
        let walls = f64::from(settings.wall_loops);
        let inset = (walls + 0.5 - settings.infill_overlap / 100.0) * settings.line_width;
        let (centre, half) = (
            [settings.bed_size_x / 2.0, settings.bed_size_y / 2.0],
            15.0 - inset,
        );
        let edge = |p: Point, axis: usize| ((p[axis] - centre[axis]).abs() - half).abs() < 1e-6;
        let (mut paths, mut lines, mut connections) = (0, 0, 0);
        // Between the solid layers at the bottom and the top.
        for layer in &toolpaths.layers[8..20] {
            for path in layer
                .paths
                .iter()
                .filter(|p| p.kind == PathKind::SparseInfill)
            {
                paths += 1;
                for w in path.points.windows(2) {
                    let mid = [(w[0][0] + w[1][0]) / 2.0, (w[0][1] + w[1][1]) / 2.0];
                    for p in [w[0], mid, w[1]] {
                        let off = (p[0] - centre[0]).abs().max((p[1] - centre[1]).abs());
                        assert!(off <= half + 1e-6, "{p:?} is outside the sparse area");
                    }
                    // A line runs from one side to another; a connection
                    // along one side.
                    if (0..2).any(|axis| edge(w[0], axis) && edge(w[1], axis) && edge(mid, axis)) {
                        connections += 1;
                    } else {
                        lines += 1;
                    }
                }
            }
        }
        // Without connections every line is a path of its own, and the
        // nozzle travels to each.
        assert!(lines > 100, "{lines}");
        assert!(paths <= 12 * 2, "{paths} paths for {lines} lines");
        assert!(connections >= lines - paths, "{connections}");

        // Switched off, every line is a path of its own.
        settings.infill_connect = false;
        let toolpaths = generate(&Mesh::cuboid(30.0, 30.0, 6.0), &settings).unwrap();
        let sparse: Vec<_> = toolpaths.layers[8..20]
            .iter()
            .flat_map(|layer| &layer.paths)
            .filter(|p| p.kind == PathKind::SparseInfill)
            .collect();
        assert!(sparse.len() > 100, "{}", sparse.len());
        assert!(sparse.iter().all(|p| p.points.len() == 2));
    }

    const PATTERNS: [InfillPattern; 7] = [
        InfillPattern::Rectilinear,
        InfillPattern::Grid,
        InfillPattern::Triangles,
        InfillPattern::Gyroid,
        InfillPattern::Honeycomb,
        InfillPattern::Cubic,
        InfillPattern::Concentric,
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
            // Rectilinear lines are as long as the others; the connections
            // along the boundary come on top (about half the boundary of
            // every layer, a sixth more on this cube).
            let ratio = length / grid;
            let ratio = if *pattern == InfillPattern::Rectilinear {
                ratio - 0.15
            } else {
                ratio
            };
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
    fn sparse_infill_grows_with_the_density() {
        let mesh = Mesh::cuboid(30.0, 30.0, 10.0);
        let length = |pattern, density| -> f64 {
            let mut settings = Settings::default();
            settings.infill_pattern = pattern;
            settings.infill_density = density;
            let toolpaths = generate(&mesh, &settings).unwrap();
            toolpaths
                .layers
                .iter()
                .map(|layer| extruded_length(layer, PathKind::SparseInfill))
                .sum()
        };
        for pattern in PATTERNS {
            let (thin, dense) = (length(pattern, 10.0), length(pattern, 30.0));
            let ratio = dense / thin;
            assert!(
                (ratio - 3.0).abs() < 0.45,
                "{pattern:?}: {thin} and {dense}"
            );
        }
    }

    #[test]
    fn cubic_moves_with_height_and_honeycomb_repeats() {
        let mut settings = Settings::default();
        settings.skirt_loops = 0;
        let mesh = Mesh::cuboid(30.0, 30.0, 6.0);
        let sparse = |toolpaths: &Toolpaths, index: usize| -> Vec<Vec<Point>> {
            toolpaths.layers[index]
                .paths
                .iter()
                .filter(|p| p.kind == PathKind::SparseInfill)
                .map(|p| p.points.clone())
                .collect()
        };
        settings.infill_pattern = InfillPattern::Honeycomb;
        let toolpaths = generate(&mesh, &settings).unwrap();
        assert_eq!(sparse(&toolpaths, 10), sparse(&toolpaths, 11));
        // The edges of the hexagons are one side long, those cut by the
        // boundary shorter.
        let spacing = settings.line_width / (settings.infill_density / 100.0);
        let side = 2.0 * spacing / 3.0_f64.sqrt();
        let edges: Vec<f64> = sparse(&toolpaths, 10)
            .iter()
            .flat_map(|line| line.windows(2).map(|w| distance(w[0], w[1])))
            .collect();
        assert!(edges.iter().all(|&e| e < side + 1e-3), "{edges:?}");
        let whole = edges.iter().filter(|&&e| (e - side).abs() < 1e-3).count();
        assert!(whole > edges.len() / 2, "{whole} of {}", edges.len());

        settings.infill_pattern = InfillPattern::Cubic;
        let toolpaths = generate(&mesh, &settings).unwrap();
        assert_ne!(sparse(&toolpaths, 10), sparse(&toolpaths, 11));
        // Three directions 60 degrees apart on every layer.
        let mut directions: Vec<i64> = sparse(&toolpaths, 10)
            .iter()
            .flat_map(|line| line.windows(2).map(|w| [w[0], w[1]]).collect::<Vec<_>>())
            .filter(|s| distance(s[0], s[1]) > 2.0)
            .map(|s| {
                let deg = (s[1][1] - s[0][1]).atan2(s[1][0] - s[0][0]).to_degrees();
                deg.rem_euclid(180.0).round() as i64 % 180
            })
            .collect();
        directions.sort_unstable();
        directions.dedup();
        assert_eq!(directions, [45, 105, 165]);
    }

    /// The solid infill of the top layer of a frame with the given top
    /// surface pattern, the settings, and a test whether a point lies in
    /// the fill area.
    fn top_surface(pattern: SurfacePattern) -> (Vec<Path>, Settings, impl Fn(Point) -> bool) {
        let mut settings = Settings::default();
        settings.top_surface_pattern = pattern;
        let toolpaths = generate(&frame(30.0, 10.0, 4.0), &settings).unwrap();
        let top: Vec<Path> = toolpaths.layers[19]
            .paths
            .iter()
            .filter(|p| p.kind == PathKind::SolidInfill)
            .cloned()
            .collect();
        let walls = f64::from(settings.wall_loops);
        let inset = (walls + 0.5 - settings.infill_overlap / 100.0) * settings.line_width;
        let inside = move |p: Point| {
            let outer = (p[0] - 110.0).abs().max((p[1] - 110.0).abs());
            let gap = |v: f64| ((v - 110.0).abs() - 5.0).max(0.0);
            outer <= 15.0 - inset + 0.005
                && distance([gap(p[0]), gap(p[1])], [0.0, 0.0]) >= inset - 0.005
        };
        (top, settings, inside)
    }

    #[test]
    fn monotonic_surfaces_lay_every_line_beside_the_last() {
        let (top, settings, inside) = top_surface(SurfacePattern::Monotonic);
        let width = settings.line_width;
        assert!(top.len() > 30, "{}", top.len());
        // Layer 19 runs across the infill angle, at 135 degrees.
        let (sin, cos) = (0.5_f64.sqrt(), -(0.5_f64.sqrt()));
        let along = |p: Point| p[0] * cos + p[1] * sin;
        let row = |p: Point| ((p[1] * cos - p[0] * sin) / width - 0.5).round() as i64;
        let lines: Vec<(i64, f64, f64)> = top
            .iter()
            .map(|path| {
                assert_eq!(path.points.len(), 2);
                let [a, b] = [path.points[0], path.points[1]];
                assert!(inside(a) && inside(b), "{a:?} {b:?}");
                // All in the same direction.
                assert!(along(b) > along(a), "{a:?} {b:?}");
                assert_eq!(row(a), row(b));
                (row(a), along(a), along(b))
            })
            .collect();
        let beside =
            |a: &(i64, f64, f64), b: &(i64, f64, f64)| b.0 == a.0 + 1 && a.1 < b.2 && b.1 < a.2;
        // No line comes before one beside it in the row before.
        for (i, later) in lines.iter().enumerate() {
            for earlier in &lines[..i] {
                assert!(!beside(later, earlier), "{earlier:?} before {later:?}");
            }
        }
        // Nearly always one row on from the line before; the two sides of
        // the hole are filled one after the other.
        let jumps = lines.windows(2).filter(|w| !beside(&w[0], &w[1])).count();
        assert!(jumps <= 4, "{jumps} jumps among {} lines", lines.len());
        // The same lines as rectilinear, only in another order; there the
        // links between lines (not along them) come on top.
        let (rectilinear, ..) = top_surface(SurfacePattern::Rectilinear);
        let total = |paths: &[Path]| -> f64 {
            paths
                .iter()
                .flat_map(|p| p.points.windows(2))
                .filter(|w| ((w[1][0] - w[0][0]) * sin - (w[1][1] - w[0][1]) * cos).abs() < 1e-6)
                .map(|w| distance(w[0], w[1]))
                .sum()
        };
        let ratio = total(&top) / total(&rectilinear);
        assert!((ratio - 1.0).abs() < 1e-9, "{ratio}");
    }

    #[test]
    fn concentric_surfaces_are_closed_loops_inside() {
        let (top, _, inside) = top_surface(SurfacePattern::Concentric);
        let loops = top.iter().filter(|p| p.closed).count();
        // Around the outside and around the hole, about nine times each.
        assert!(loops >= 14, "{loops}");
        for path in &top {
            for p in &path.points {
                assert!(inside(*p), "{p:?}");
            }
        }
        let (rectilinear, ..) = top_surface(SurfacePattern::Rectilinear);
        let total = |paths: &[Path]| paths.iter().map(Path::length).sum::<f64>();
        let ratio = total(&top) / total(&rectilinear);
        assert!((ratio - 1.0).abs() < 0.1, "{ratio}");
    }

    #[test]
    fn surface_patterns_leave_other_solid_infill_alone() {
        // A step: the top of the lower part around the tower gets the
        // pattern; under the tower the lines stay.
        let mut settings = Settings::default();
        settings.skirt_loops = 0;
        let mut mesh = Mesh::cuboid(30.0, 30.0, 4.0);
        let mut tower = Mesh::cuboid(10.0, 10.0, 4.0);
        tower.translate([10.0, 10.0, 4.0]);
        mesh.merge(&tower);
        let plain = generate(&mesh, &settings).unwrap();
        settings.top_surface_pattern = SurfacePattern::Concentric;
        settings.bottom_surface_pattern = SurfacePattern::Monotonic;
        let patterned = generate(&mesh, &settings).unwrap();
        let closed = |toolpaths: &Toolpaths, index: usize| {
            toolpaths.layers[index]
                .paths
                .iter()
                .filter(|p| p.kind == PathKind::SolidInfill && p.closed)
                .count()
        };
        // Layer 19 is the top of the step.
        assert!(closed(&patterned, 19) > 5);
        assert_eq!(closed(&plain, 19), 0);
        assert!(closed(&patterned, 39) > 5);
        // Solid infill below the top surfaces stays as it was.
        assert_eq!(plain.layers[17], patterned.layers[17]);
        assert_eq!(plain.layers[30], patterned.layers[30]);
        assert_ne!(plain.layers[0], patterned.layers[0]);
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

    /// Two 10 mm cubes, the second `gap` mm to the right of the first and
    /// on extruder 2.
    fn two_cubes(gap: f64, height: f64) -> Mesh {
        let mut mesh = Mesh::cuboid(10.0, 10.0, height);
        let mut other = Mesh::cuboid(10.0, 10.0, height);
        other.translate([10.0 + gap, 0.0, 0.0]);
        other.set_extruder(0, 1);
        mesh.merge(&other);
        mesh
    }

    #[test]
    fn every_extruder_prints_its_own_objects() {
        let mut settings = Settings::default();
        settings.extruder_count = 2;
        let mut diagnostics = Diagnostics::new();
        let toolpaths = generate_reporting(&two_cubes(10.0, 2.0), &settings, &mut diagnostics);
        let toolpaths = toolpaths.unwrap();
        assert!(diagnostics.is_empty(), "{:?}", diagnostics.codes());
        // The model is centred: the cubes span X 95..105 and 115..125.
        let mut tool = 0;
        for (index, layer) in toolpaths.layers.iter().enumerate() {
            for path in &layer.paths {
                let model = !matches!(path.kind, PathKind::Skirt | PathKind::PrimeTower);
                for p in path.points.iter().filter(|_| model) {
                    let expected = u32::from(p[0] > 110.0);
                    assert_eq!(path.extruder, expected, "layer {index}: {p:?}");
                }
            }
            // The extruder that ended a layer starts the next one, so every
            // layer changes tools once.
            let mut order: Vec<u32> = layer.paths.iter().map(|p| p.extruder).collect();
            order.dedup();
            assert_eq!(order, [tool, 1 - tool], "layer {index}");
            tool = 1 - tool;
            // The extruder changed to primes into the tower first.
            let first_of_new = layer.paths.iter().find(|p| p.extruder == order[1]).unwrap();
            assert_eq!(first_of_new.kind, PathKind::PrimeTower, "layer {index}");
            // The tower stands at its place and fills its square.
            let tower: Vec<&Path> = layer
                .paths
                .iter()
                .filter(|p| p.kind == PathKind::PrimeTower)
                .collect();
            for p in tower.iter().flat_map(|path| &path.points) {
                assert!(
                    (p[0] - 190.0).abs() < 7.5 && (p[1] - 190.0).abs() < 7.5,
                    "{p:?}"
                );
            }
            let length: f64 = tower.iter().map(|path| path.length()).sum();
            let area = length * settings.line_width;
            assert!((area / 225.0 - 1.0).abs() < 0.1, "layer {index}: {area}");
        }

        // Without a tower nothing but the cubes and the skirt is printed.
        settings.prime_tower_width = 0.0;
        let plain = generate(&two_cubes(10.0, 2.0), &settings).unwrap();
        assert!(
            plain
                .layers
                .iter()
                .flat_map(|l| &l.paths)
                .all(|p| p.kind != PathKind::PrimeTower)
        );
        // Over the top of the lower cube nothing changes tools any more,
        // and the tower ends with the last tool change.
        let mut taller = Mesh::cuboid(10.0, 10.0, 4.0);
        let mut lower = Mesh::cuboid(10.0, 10.0, 2.0);
        lower.translate([20.0, 0.0, 0.0]);
        lower.set_extruder(0, 1);
        taller.merge(&lower);
        settings.prime_tower_width = 15.0;
        let toolpaths = generate(&taller, &settings).unwrap();
        let towers: Vec<bool> = (toolpaths.layers.iter())
            .map(|l| l.paths.iter().any(|p| p.kind == PathKind::PrimeTower))
            .collect();
        // Layer 9, the last one of the lower cube, ends with extruder 1, so
        // layer 10 starts without a change.
        assert_eq!(towers.iter().filter(|&&t| t).count(), 10, "{towers:?}");
        assert!(towers[..10].iter().all(|&t| t));
    }

    #[test]
    fn the_higher_extruder_takes_where_objects_overlap() {
        // The second cube reaches 4 mm into the first one.
        let mut settings = Settings::default();
        settings.extruder_count = 2;
        settings.prime_tower_width = 0.0;
        let toolpaths = generate(&two_cubes(-4.0, 2.0), &settings).unwrap();
        // The cubes span X 102..112 and 108..118; extruder 2 takes 108..118.
        let w = settings.line_width;
        for path in &toolpaths.layers[5].paths {
            for p in &path.points {
                match path.extruder {
                    0 => assert!(p[0] < 108.0 - 0.5 * w + 1e-6, "{p:?}"),
                    _ => assert!(p[0] > 108.0 + 0.5 * w - 1e-6, "{p:?}"),
                }
            }
        }
        // Both regions together are filled like one solid: as much path as
        // the same block on one extruder, give or take the walls between.
        let length = |toolpaths: &Toolpaths| -> f64 {
            toolpaths.layers[5].paths.iter().map(Path::length).sum()
        };
        let mut one = two_cubes(-4.0, 2.0);
        one.extruders.clear();
        let single = generate(&one, &settings).unwrap();
        let ratio = length(&toolpaths) / length(&single);
        assert!(ratio > 1.0 && ratio < 1.3, "{ratio}");
    }

    #[test]
    fn objects_on_missing_extruders_print_with_the_first() {
        let mut settings = Settings::default();
        settings.extruder_count = 2;
        let mut mesh = two_cubes(10.0, 1.0);
        let mut third = Mesh::cuboid(10.0, 10.0, 1.0);
        third.translate([0.0, 20.0, 0.0]);
        third.set_extruder(0, 4);
        mesh.merge(&third);
        let mut diagnostics = Diagnostics::new();
        let toolpaths = generate_reporting(&mesh, &settings, &mut diagnostics).unwrap();
        assert_eq!(diagnostics.codes(), ["slicing.missing_extruder"]);
        assert_eq!(diagnostics.as_slice()[0].context["extruders"], "5");
        let mut used: Vec<u32> = (toolpaths.layers.iter().flat_map(|l| &l.paths))
            .map(|p| p.extruder)
            .collect();
        used.sort_unstable();
        used.dedup();
        assert_eq!(used, [0, 1]);
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
    fn tree_supports_branch_and_merge() {
        // The T of `supports_stand_under_overhangs`.
        let mut t = Mesh::cuboid(10.0, 10.0, 10.0);
        t.translate([10.0, 0.0, 0.0]);
        let mut plate = Mesh::cuboid(30.0, 10.0, 2.0);
        plate.translate([0.0, 0.0, 10.0]);
        t.merge(&plate);
        fn support(layer: &Layer) -> Vec<&Path> {
            layer
                .paths
                .iter()
                .filter(|p| p.kind == PathKind::Support)
                .collect()
        }
        let length = |toolpaths: &Toolpaths| -> f64 {
            toolpaths
                .layers
                .iter()
                .flat_map(|layer| support(layer))
                .map(Path::length)
                .sum()
        };
        let mut settings = Settings::default();
        settings.supports = true;
        let normal = length(&generate(&t, &settings).unwrap());
        settings.support_style = SupportStyle::Tree;
        let mut warnings = Diagnostics::new();
        let toolpaths = generate_reporting(&t, &settings, &mut warnings).unwrap();
        assert!(warnings.is_empty(), "{:?}", warnings.codes());
        let tree = length(&toolpaths);
        // Every layer under the plate (layer 50) but the empty one right
        // below it has support, and it stands on the bed.
        let layers: Vec<usize> = (0..toolpaths.layers.len())
            .filter(|&i| !support(&toolpaths.layers[i]).is_empty())
            .collect();
        assert_eq!(layers, (0..49).collect::<Vec<usize>>());
        assert!(
            tree < 0.5 * normal,
            "tree {tree:.0} mm, normal {normal:.0} mm"
        );
        // No line comes closer to the column (x 105 to 115, y 105 to 115)
        // than the gap of one line width, measured from its middle.
        for layer in &toolpaths.layers[..49] {
            for p in support(layer).iter().flat_map(|path| &path.points) {
                let dx = (105.0 - p[0]).max(p[0] - 115.0).max(0.0);
                let dy = (105.0 - p[1]).max(p[1] - 115.0).max(0.0);
                let gap = (dx * dx + dy * dy).sqrt();
                assert!(gap >= 0.45 - 1e-3, "{p:?} is {gap} mm from the column");
            }
        }
        // The two dense layers under the plate; below them the branches
        // start as many thin tips and merge into fewer trunks.
        let loops = |index: usize| {
            support(&toolpaths.layers[index])
                .iter()
                .filter(|p| p.closed)
                .count()
        };
        assert!(loops(46) >= 16, "{}", loops(46));
        assert!(loops(0) <= 4 && loops(0) < loops(46) / 4, "{}", loops(0));

        // With a base under the column the branches stand on it, one empty
        // layer above it (layers 11 to 58 as with straight columns).
        let mut h = Mesh::cuboid(30.0, 10.0, 2.0);
        let mut column = Mesh::cuboid(10.0, 10.0, 10.0);
        column.translate([10.0, 0.0, 2.0]);
        h.merge(&column);
        plate.translate([0.0, 0.0, 2.0]);
        h.merge(&plate);
        let toolpaths = generate(&h, &settings).unwrap();
        let layers: Vec<usize> = (0..toolpaths.layers.len())
            .filter(|&i| !support(&toolpaths.layers[i]).is_empty())
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

    /// Material a layer deposits per millimetre of height, in mm².
    fn deposited_area(layer: &Layer, settings: &Settings) -> f64 {
        layer
            .paths
            .iter()
            .map(|p| p.length() * p.width.unwrap_or(settings.line_width))
            .sum()
    }

    #[test]
    fn thin_fin_gets_a_line_on_every_layer() {
        // A fin 0.3 mm thick on a plate 1 mm thick.
        let mut mesh = Mesh::cuboid(20.0, 20.0, 1.0);
        let mut fin = Mesh::cuboid(20.0, 0.3, 5.0);
        fin.translate([0.0, 10.0, 1.0]);
        mesh.merge(&fin);
        let settings = Settings::default();
        let mut diagnostics = Diagnostics::new();
        let toolpaths = generate_reporting(&mesh, &settings, &mut diagnostics).unwrap();
        assert!(diagnostics.is_empty(), "{:?}", diagnostics.codes());
        let fin_layers: Vec<&Layer> = toolpaths.layers.iter().filter(|l| l.z > 1.01).collect();
        assert_eq!(fin_layers.len(), 25);
        for layer in fin_layers {
            let [line] = layer.paths.as_slice() else {
                panic!("z {}: {} paths", layer.z, layer.paths.len());
            };
            assert_eq!(line.kind, PathKind::OuterWall);
            assert!(!line.closed);
            assert!(line.length() > 19.0, "{}", line.length());
            // Along the middle of the fin, which spans Y 110 to 110.3 on
            // the bed. The ends may bend off by a few hundredths.
            let mid = line.points[line.points.len() / 2][1];
            assert!((mid - 110.15).abs() < 0.01, "{mid}");
            assert!(line.points.iter().all(|p| (p[1] - 110.15).abs() < 0.05));
            let width = line.width.unwrap();
            assert!((0.25..0.35).contains(&width), "{width}");
            let ratio = deposited_area(layer, &settings) / (20.0 * 0.3);
            assert!((ratio - 1.0).abs() < 0.25, "z {}: {ratio}", layer.z);
        }
    }

    #[test]
    fn thin_ring_gets_a_closed_line() {
        let ring = slicer_core::primitives::tube(10.0, 9.7, 2.0, 96);
        let settings = Settings::default();
        let toolpaths = generate(&ring, &settings).unwrap();
        let expected = std::f64::consts::PI * (10.0 * 10.0 - 9.7 * 9.7);
        for layer in &toolpaths.layers[1..] {
            let lines: Vec<&Path> = layer
                .paths
                .iter()
                .filter(|p| p.kind == PathKind::OuterWall)
                .collect();
            let [line] = lines.as_slice() else {
                panic!("z {}: {} walls", layer.z, lines.len());
            };
            assert!(line.closed && line.width.is_some());
            let ratio = deposited_area(layer, &settings) / expected;
            assert!((ratio - 1.0).abs() < 0.1, "z {}: {ratio}", layer.z);
        }
    }

    #[test]
    fn fuzzy_skin_roughens_the_outer_wall_only() {
        let mesh = Mesh::cuboid(20.0, 20.0, 1.0);
        let plain = Settings {
            skirt_loops: 0,
            ..Settings::default()
        };
        let fuzzy = Settings {
            fuzzy_skin: true,
            ..plain.clone()
        };
        let (a, b) = (
            generate(&mesh, &plain).unwrap(),
            generate(&mesh, &fuzzy).unwrap(),
        );
        assert_eq!(a.layers[0], b.layers[0], "the first layer stays smooth");
        let outer = |t: &Toolpaths, layer: usize| -> Vec<Point> {
            t.layers[layer]
                .paths
                .iter()
                .filter(|p| p.kind == PathKind::OuterWall)
                .flat_map(|p| p.points.clone())
                .collect()
        };
        let (smooth, rough) = (outer(&a, 2), outer(&b, 2));
        // A point about every 0.8 mm round the 19.55 mm square.
        assert!(rough.len() > 90 && smooth.len() < 10, "{}", rough.len());
        // Within 0.15 mm of the smooth square, both ways.
        let (lo, hi) = (110.0 - 10.0 + 0.225, 110.0 + 10.0 - 0.225);
        let edge = |p: &Point| {
            [p[0] - lo, hi - p[0], p[1] - lo, hi - p[1]]
                .into_iter()
                .map(f64::abs)
                .fold(f64::INFINITY, f64::min)
        };
        assert!(rough.iter().all(|p| edge(p) <= 0.15 + 1e-9));
        assert!(rough.iter().any(|p| edge(p) > 0.05));
        // The same every time, and inner walls untouched.
        assert_eq!(generate(&mesh, &fuzzy).unwrap(), b);
        let inner = |t: &Toolpaths| {
            t.layers[2]
                .paths
                .iter()
                .filter(|p| p.kind == PathKind::InnerWall)
                .map(|p| p.points.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(inner(&a), inner(&b));
    }

    #[test]
    fn elephant_foot_compensation_shrinks_the_first_layer() {
        let mesh = Mesh::cuboid(20.0, 20.0, 1.0);
        let outer_min_x = |toolpaths: &Toolpaths, layer: usize| {
            toolpaths.layers[layer]
                .paths
                .iter()
                .filter(|p| p.kind == PathKind::OuterWall)
                .flat_map(|p| p.points.iter().map(|q| q[0]))
                .fold(f64::INFINITY, f64::min)
        };
        let plain = Settings {
            skirt_loops: 0,
            ..Settings::default()
        };
        let shrunk = Settings {
            elephant_foot_compensation: 0.3,
            ..plain.clone()
        };
        let (a, b) = (
            generate(&mesh, &plain).unwrap(),
            generate(&mesh, &shrunk).unwrap(),
        );
        assert!((outer_min_x(&b, 0) - outer_min_x(&a, 0) - 0.3).abs() < 1e-6);
        assert_eq!(outer_min_x(&b, 1), outer_min_x(&a, 1));
        assert_eq!(a.layers[1..], b.layers[1..]);
    }

    #[test]
    fn walls_do_not_fold_back_onto_themselves() {
        let settings = Settings::default();
        // 0.65 mm, about 1.4 line widths: one line of that width, not a wall
        // loop whose sides overlap; 1.6 mm: two walls would leave 0.25 mm
        // for the second one, which becomes a line of 0.7 mm instead.
        for (thickness, kind) in [(0.65, PathKind::OuterWall), (1.6, PathKind::GapFill)] {
            let toolpaths = generate(&Mesh::cuboid(20.0, thickness, 2.0), &settings).unwrap();
            for layer in &toolpaths.layers[1..] {
                let lines: Vec<&Path> = layer.paths.iter().filter(|p| p.kind == kind).collect();
                let [line] = lines.as_slice() else {
                    panic!("{thickness} z {}: {} lines", layer.z, lines.len());
                };
                assert!(!line.closed && line.length() > 19.0, "{}", line.length());
                let width = line.width.unwrap();
                assert!(width > settings.line_width, "{thickness}: {width}");
                let ratio = deposited_area(layer, &settings) / (20.0 * thickness);
                assert!(
                    (ratio - 1.0).abs() < 0.1,
                    "{thickness} z {}: {ratio}",
                    layer.z
                );
            }
        }
        // Exactly two widths: the sides of the loop just touch.
        let toolpaths = generate(&Mesh::cuboid(20.0, 0.9, 2.0), &settings).unwrap();
        assert!(
            toolpaths
                .layers
                .iter()
                .flat_map(|l| &l.paths)
                .all(|p| p.closed && p.width.is_none())
        );
        // A square with a tab of 0.65 mm in the same layers: the square keeps
        // its wall loops, the tab gets a line of its own.
        let outline = [
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 5.0],
            [20.0, 5.0],
            [20.0, 5.65],
            [10.0, 5.65],
            [10.0, 10.0],
            [0.0, 10.0],
        ];
        let mesh = slicer_core::primitives::extrude(&outline, [0.0, 0.0], 2.0, 1, 0.0);
        let toolpaths = generate(&mesh, &settings).unwrap();
        let alone = generate(&Mesh::cuboid(10.0, 10.0, 2.0), &settings).unwrap();
        for (layer, alone) in toolpaths.layers.iter().zip(&alone.layers).skip(1) {
            let tab: Vec<&Path> = layer
                .paths
                .iter()
                .filter(|p| p.kind == PathKind::OuterWall && p.width.is_some())
                .collect();
            let [tab] = tab.as_slice() else {
                panic!("z {}: {} tab lines", layer.z, tab.len());
            };
            // The tab spans Y 110 to 110.65 on the bed.
            assert!(tab.points.iter().all(|p| p[1] > 109.9 && p[1] < 110.75));
            assert!(tab.length() > 9.0, "{}", tab.length());
            // Without the tab's line, the square gets as much as before.
            let square = deposited_area(layer, &settings) - tab.length() * tab.width.unwrap();
            let ratio = square / deposited_area(alone, &settings);
            assert!((ratio - 1.0).abs() < 0.05, "z {}: {ratio}", layer.z);
        }
    }

    #[test]
    fn gaps_between_walls_are_filled() {
        // 1.2 mm leaves a strip of 0.3 mm between the two sides of the
        // outer wall, too narrow for the second wall.
        let settings = Settings::default();
        let toolpaths = generate(&Mesh::cuboid(20.0, 1.2, 2.0), &settings).unwrap();
        for layer in &toolpaths.layers[1..] {
            let gaps: Vec<&Path> = layer
                .paths
                .iter()
                .filter(|p| p.kind == PathKind::GapFill)
                .collect();
            let [gap] = gaps.as_slice() else {
                panic!("z {}: {} gap lines", layer.z, gaps.len());
            };
            assert!(gap.length() > 18.0, "{}", gap.length());
            let width = gap.width.unwrap();
            assert!((0.25..0.35).contains(&width), "{width}");
            let ratio = deposited_area(layer, &settings) / (20.0 * 1.2);
            assert!((ratio - 1.0).abs() < 0.05, "z {}: {ratio}", layer.z);
        }
        // Walls that fit exactly leave no gap.
        let toolpaths = generate(&Mesh::cuboid(20.0, 20.0, 2.0), &settings).unwrap();
        assert!(
            toolpaths
                .layers
                .iter()
                .flat_map(|l| &l.paths)
                .all(|p| p.kind != PathKind::GapFill && p.width.is_none())
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

    /// A 10 mm column 4 mm high carrying a slab 20 mm wide, 5 mm sticking
    /// out on both sides.
    fn mushroom() -> Mesh {
        let mut mesh = Mesh::cuboid(10.0, 10.0, 4.0);
        let mut slab = Mesh::cuboid(20.0, 10.0, 2.0);
        slab.translate([-5.0, 0.0, 4.0]);
        mesh.merge(&slab);
        mesh
    }

    #[test]
    fn walls_over_the_edge_become_overhang_walls() {
        let mut settings = Settings::default();
        let plain = generate(&mushroom(), &settings).unwrap();
        settings.overhang_speed = 15.0;
        let split = generate(&mushroom(), &settings).unwrap();
        let has = |layer: &Layer| layer.paths.iter().any(|p| p.kind == PathKind::OverhangWall);
        let first = split
            .layers
            .iter()
            .position(|l| (l.z - 4.2).abs() < 1e-6)
            .unwrap();
        // Only the first layer of the slab hangs.
        for (index, layer) in split.layers.iter().enumerate() {
            assert_eq!(has(layer), index == first, "layer {index}");
        }
        assert!(!plain.layers.iter().any(has));
        // The walls keep their length and stay connected.
        let walls = |layer: &Layer| -> f64 {
            [
                PathKind::OuterWall,
                PathKind::InnerWall,
                PathKind::OverhangWall,
            ]
            .into_iter()
            .map(|kind| extruded_length(layer, kind))
            .sum()
        };
        let (before, after) = (&plain.layers[first], &split.layers[first]);
        assert!((walls(before) - walls(after)).abs() < 1e-6);
        // Pieces of a cut wall follow on one another and close the loop.
        let mut chain: Option<(Point, Point)> = None;
        let wall = |kind| {
            matches!(
                kind,
                PathKind::OuterWall | PathKind::InnerWall | PathKind::OverhangWall
            )
        };
        for path in after.paths.iter().filter(|p| !p.closed && wall(p.kind)) {
            let (start, end) = (path.points[0], *path.points.last().unwrap());
            chain = match chain {
                Some((begin, last)) if last == start => Some((begin, end)),
                Some((begin, last)) => {
                    assert_eq!(begin, last, "an open wall");
                    Some((start, end))
                }
                None => Some((start, end)),
            };
        }
        let (begin, last) = chain.unwrap();
        assert_eq!(begin, last);
        // What hangs lies beside the column.
        let column: Vec<f64> = split.layers[first - 1]
            .paths
            .iter()
            .filter(|p| p.kind == PathKind::OuterWall)
            .flat_map(|p| p.points.iter().map(|q| q[0]))
            .collect();
        let left = column.iter().copied().fold(f64::INFINITY, f64::min);
        let right = column.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let mut hanging = 0.0;
        for path in after
            .paths
            .iter()
            .filter(|p| p.kind == PathKind::OverhangWall)
        {
            for pair in path.points.windows(2) {
                let x = (pair[0][0] + pair[1][0]) / 2.0;
                assert!(x < left + 0.5 || x > right - 0.5, "{x} in {left}..{right}");
                hanging += distance(pair[0], pair[1]);
            }
        }
        assert!(hanging > 20.0, "{hanging}");
    }

    #[test]
    fn walls_hanging_free_entirely_get_their_own_kind() {
        let length = |toolpaths: &Toolpaths, kind: PathKind| -> f64 {
            toolpaths
                .layers
                .iter()
                .map(|layer| extruded_length(layer, kind))
                .sum()
        };
        let mut settings = Settings::default();
        settings.overhang_speed = 15.0;
        let partly = generate(&mushroom(), &settings).unwrap();
        settings.overhang_speed_full = 5.0;
        let split = generate(&mushroom(), &settings).unwrap();
        // The slab sticks out 5 mm: what hangs there hangs free entirely,
        // including the bit where the line only starts to hang (evened
        // out), and the same walls hang as before.
        let before = length(&partly, PathKind::OverhangWall);
        let full = length(&split, PathKind::OverhangWallFull);
        assert!(full > 20.0, "{full}");
        assert_eq!(length(&split, PathKind::OverhangWall), 0.0);
        assert!((full - before).abs() < 1e-6, "{full} {before}");

        // A slab only 0.4 mm wider on one side: the outer wall hangs by
        // less than half a line width more than half its width, so partly.
        let mut lip = Mesh::cuboid(10.0, 10.0, 4.0);
        let mut slab = Mesh::cuboid(10.4, 10.0, 2.0);
        slab.translate([-0.4, 0.0, 4.0]);
        lip.merge(&slab);
        let split = generate(&lip, &settings).unwrap();
        assert!(length(&split, PathKind::OverhangWall) > 5.0);
        assert_eq!(length(&split, PathKind::OverhangWallFull), 0.0);
    }

    #[test]
    fn ironing_can_be_limited_to_the_topmost_surface() {
        // Steps: a block 5 mm high beside one 10 mm high.
        let mut steps = Mesh::cuboid(10.0, 10.0, 5.0);
        let mut tower = Mesh::cuboid(10.0, 10.0, 10.0);
        tower.translate([10.0, 0.0, 0.0]);
        steps.merge(&tower);
        let ironed = |toolpaths: &Toolpaths| -> Vec<f64> {
            toolpaths
                .layers
                .iter()
                .filter(|layer| extruded_length(layer, PathKind::Ironing) > 0.0)
                .map(|layer| layer.z)
                .collect()
        };
        let mut settings = Settings::default();
        settings.ironing = true;
        let all = ironed(&generate(&steps, &settings).unwrap());
        assert_eq!(all.len(), 2, "{all:?}");
        assert!((all[0] - 5.0).abs() < 1e-6 && (all[1] - 10.0).abs() < 1e-6);
        settings.ironing_type = slicer_profiles::IroningType::Top;
        let top = ironed(&generate(&steps, &settings).unwrap());
        assert_eq!(top, [all[1]]);
        // Two separate parts each have their topmost surface.
        let mut apart = Mesh::cuboid(10.0, 10.0, 5.0);
        let mut tower = Mesh::cuboid(10.0, 10.0, 10.0);
        tower.translate([15.0, 0.0, 0.0]);
        apart.merge(&tower);
        assert_eq!(ironed(&generate(&apart, &settings).unwrap()), all);
    }

    #[test]
    fn short_overhangs_and_gaps_are_evened_out() {
        let lengths = [0.5; 12];
        // A gap of one piece between overhangs is filled ...
        let mut flags = [
            false, true, true, false, true, true, false, false, false, true, false, false,
        ];
        even_out(&mut flags, &lengths, false);
        assert_eq!(
            flags,
            [
                false, true, true, true, true, true, false, false, false, true, false, false
            ]
        );
        // ... and an overhang of one piece dropped.
        even_out(&mut flags, &lengths, true);
        assert_eq!(
            flags,
            [
                false, true, true, true, true, true, false, false, false, false, false, false
            ]
        );
    }
}
