//! Models on the build plate: loading, placing, rotating, scaling and
//! arranging them. The slicer gets every model where it is placed.

use slicer_core::{Diagnostic, Diagnostics, Mesh};
use slicer_fdm::Toolpaths;
use slicer_profiles::Settings;

/// Distance in millimetres by which a model may reach over the edge of the
/// bed before it counts as off the bed, to absorb rounding.
const EDGE_TOLERANCE: f64 = 1e-6;

/// Smallest and largest scale factor the plate accepts.
pub const SCALE_RANGE: (f64, f64) = (0.01, 100.0);

/// A model loaded from a file, with what the plate view needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    /// File name shown to the user.
    pub name: String,
    /// The mesh as read, moved so that the centre of its bounding box lies
    /// on the Z axis and its lowest point at Z = 0.
    pub mesh: Mesh,
    /// Convex hull of the mesh seen from above, counterclockwise, in the
    /// same coordinates as [`Model::mesh`].
    pub hull: Vec<[f64; 2]>,
    /// Size of the bounding box in X, Y and Z, in millimetres.
    pub size: [f64; 3],
}

impl Model {
    /// Prepares a mesh for the plate. Fails if the mesh has no triangles.
    pub fn from_mesh(name: impl Into<String>, mut mesh: Mesh) -> Result<Self, Diagnostic> {
        let (min, max) = mesh
            .bounds()
            .ok_or_else(|| Diagnostic::error("ui.empty_model", "the model contains no geometry"))?;
        mesh.translate([-(min[0] + max[0]) / 2.0, -(min[1] + max[1]) / 2.0, -min[2]]);
        let used = mesh
            .triangles
            .iter()
            .flatten()
            .map(|&i| mesh.vertices[i as usize])
            .map(|v| [v[0], v[1]]);
        let hull = convex_hull(used.collect());
        Ok(Self {
            name: name.into(),
            hull,
            size: [max[0] - min[0], max[1] - min[1], max[2] - min[2]],
            mesh,
        })
    }
}

/// Reads an STL or 3MF file. Returns the model and the warnings the reader
/// reported, for example about a project with several plates (the first
/// one is used).
pub fn load_model(name: &str, bytes: &[u8]) -> Result<(Model, Diagnostics), Diagnostic> {
    let mut warnings = Diagnostics::new();
    let mesh = slicer_formats::parse_model(bytes, None, &mut warnings)?;
    Ok((Model::from_mesh(name, mesh)?, warnings))
}

/// Where and how the model sits on the plate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Centre of the model's bounding box seen from above, in bed
    /// coordinates (millimetres).
    pub center: [f64; 2],
    /// Rotation around the Z axis in degrees, counterclockwise seen from
    /// above.
    pub rotation_deg: f64,
    /// Uniform scale factor; 1 keeps the size from the file.
    pub scale: f64,
}

impl Placement {
    /// The model in the middle of the bed, unrotated, at its original size.
    pub fn centered(settings: &Settings) -> Self {
        Self {
            center: bed_center(settings),
            rotation_deg: 0.0,
            scale: 1.0,
        }
    }

    /// Rotates and scales a point of the model around its centre.
    fn turn(&self, p: [f64; 2]) -> [f64; 2] {
        let (sin, cos) = self.rotation_deg.to_radians().sin_cos();
        [
            (p[0] * cos - p[1] * sin) * self.scale,
            (p[0] * sin + p[1] * cos) * self.scale,
        ]
    }
}

/// Middle of the bed in millimetres.
pub fn bed_center(settings: &Settings) -> [f64; 2] {
    [settings.bed_size_x / 2.0, settings.bed_size_y / 2.0]
}

/// Smallest and largest corner of a set of points.
pub fn bounds_2d(points: impl IntoIterator<Item = [f64; 2]>) -> Option<([f64; 2], [f64; 2])> {
    let mut points = points.into_iter();
    let first = points.next()?;
    Some(points.fold((first, first), |(lo, hi), p| {
        (
            [lo[0].min(p[0]), lo[1].min(p[1])],
            [hi[0].max(p[0]), hi[1].max(p[1])],
        )
    }))
}

/// The outline of the model as placed, in bed coordinates: its convex hull
/// seen from above.
pub fn placed_outline(model: &Model, placement: &Placement) -> Vec<[f64; 2]> {
    let turned: Vec<[f64; 2]> = model.hull.iter().map(|&p| placement.turn(p)).collect();
    // The bounding box of a point set is that of its hull, so the centre
    // computed here is the one the slicer centres.
    let Some((lo, hi)) = bounds_2d(turned.iter().copied()) else {
        return Vec::new();
    };
    let shift = [
        placement.center[0] - (lo[0] + hi[0]) / 2.0,
        placement.center[1] - (lo[1] + hi[1]) / 2.0,
    ];
    turned
        .into_iter()
        .map(|p| [p[0] + shift[0], p[1] + shift[1]])
        .collect()
}

/// Whether a point in bed coordinates lies inside the placed outline.
pub fn outline_contains(outline: &[[f64; 2]], point: [f64; 2]) -> bool {
    // A convex counterclockwise polygon contains the points left of every
    // edge.
    outline.len() >= 3
        && outline
            .iter()
            .zip(outline.iter().cycle().skip(1))
            .all(|(a, b)| cross(*a, *b, point) >= 0.0)
}

/// The mesh to slice: rotated, scaled and moved to its place on the bed.
pub fn placed_mesh(model: &Model, placement: &Placement) -> Mesh {
    let mut mesh = model.mesh.clone();
    mesh.rotate_z(placement.rotation_deg.to_radians());
    for v in &mut mesh.vertices {
        for c in v.iter_mut() {
            *c *= placement.scale;
        }
    }
    if let Some((lo, hi)) = mesh.bounds() {
        mesh.translate([
            placement.center[0] - (lo[0] + hi[0]) / 2.0,
            placement.center[1] - (lo[1] + hi[1]) / 2.0,
            0.0,
        ]);
    }
    mesh
}

/// Checks that the placed model can be sliced: a sensible scale, inside the
/// bed and not taller than the printer allows.
pub fn check_placement(
    model: &Model,
    placement: &Placement,
    settings: &Settings,
) -> Result<(), Diagnostic> {
    let (min_scale, max_scale) = SCALE_RANGE;
    if !(placement.scale.is_finite()
        && placement.scale >= min_scale
        && placement.scale <= max_scale)
    {
        return Err(Diagnostic::error(
            "ui.invalid_scale",
            format!(
                "scale {} is out of range (allowed: {min_scale} to {max_scale})",
                placement.scale
            ),
        ));
    }
    let height = model.size[2] * placement.scale;
    if height > settings.max_print_height + EDGE_TOLERANCE {
        return Err(Diagnostic::error(
            "ui.too_tall",
            format!(
                "the model is {height:.1} mm tall, the printer allows {:.1} mm",
                settings.max_print_height
            ),
        ));
    }
    let outline = placed_outline(model, placement);
    if let Some((lo, hi)) = bounds_2d(outline) {
        let bed = [settings.bed_size_x, settings.bed_size_y];
        if (0..2).any(|a| lo[a] < -EDGE_TOLERANCE || hi[a] > bed[a] + EDGE_TOLERANCE) {
            return Err(Diagnostic::error(
                "ui.off_bed",
                "the model reaches over the edge of the bed; move it or make it smaller",
            ));
        }
    }
    Ok(())
}

/// A model and where it sits.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// The model as loaded.
    pub model: Model,
    /// Its position, rotation and scale.
    pub placement: Placement,
}

/// The models on the build plate and which one is selected.
#[derive(Debug, Clone, Default)]
pub struct Plate {
    /// The models in the order they were added.
    pub items: Vec<Item>,
    /// Index of the selected model, which the side panel edits.
    pub selected: Option<usize>,
    /// Counts models added and removed, to tell whether a slice shows the
    /// current plate.
    pub serial: u64,
}

impl Plate {
    /// Puts a model in the middle of the bed and selects it.
    pub fn add(&mut self, model: Model, settings: &Settings) {
        self.items.push(Item {
            model,
            placement: Placement::centered(settings),
        });
        self.selected = Some(self.items.len() - 1);
        self.serial += 1;
    }

    /// Removes the selected model; nothing is selected afterwards.
    pub fn remove_selected(&mut self) {
        if let Some(index) = self.selected.take().filter(|&i| i < self.items.len()) {
            self.items.remove(index);
            self.serial += 1;
        }
    }

    /// The selected model.
    pub fn selected_mut(&mut self) -> Option<&mut Item> {
        self.items.get_mut(self.selected?)
    }

    /// The topmost (last added) model whose outline contains `point`.
    pub fn at(&self, point: [f64; 2]) -> Option<usize> {
        self.items.iter().rposition(|item| {
            outline_contains(&placed_outline(&item.model, &item.placement), point)
        })
    }

    /// Where every model sits, in order.
    pub fn placements(&self) -> Vec<Placement> {
        self.items.iter().map(|item| item.placement).collect()
    }

    /// Places all models side by side without overlap (see
    /// [`slicer_fdm::arrange`]); rotation and scale stay.
    pub fn arrange(&mut self, settings: &Settings) {
        let sizes: Vec<[f64; 2]> = self
            .items
            .iter()
            .map(|item| {
                bounds_2d(placed_outline(&item.model, &item.placement))
                    .map_or([0.0, 0.0], |(lo, hi)| [hi[0] - lo[0], hi[1] - lo[1]])
            })
            .collect();
        let corners = slicer_fdm::arrange(&sizes, settings, slicer_fdm::ARRANGE_GAP);
        for ((item, size), corner) in self.items.iter_mut().zip(&sizes).zip(corners) {
            item.placement.center = [corner[0] + size[0] / 2.0, corner[1] + size[1] / 2.0];
        }
    }

    /// Checks every model with [`check_placement`]; the message names the
    /// model when there are several.
    pub fn check(&self, settings: &Settings) -> Result<(), Diagnostic> {
        if self.items.is_empty() {
            return Err(Diagnostic::error("ui.no_model", "Open a model first."));
        }
        for item in &self.items {
            check_placement(&item.model, &item.placement, settings).map_err(|mut error| {
                if self.items.len() > 1 {
                    error.message = format!("{}: {}", item.model.name, error.message);
                }
                error
            })?;
        }
        Ok(())
    }

    /// The meshes to slice, each where it is placed.
    pub fn meshes(&self) -> Vec<Mesh> {
        self.items
            .iter()
            .map(|item| placed_mesh(&item.model, &item.placement))
            .collect()
    }
}

/// Smallest and largest corner of all paths, or `None` without paths.
pub fn toolpath_bounds(toolpaths: &Toolpaths) -> Option<([f64; 2], [f64; 2])> {
    bounds_2d(
        toolpaths
            .layers
            .iter()
            .flat_map(|layer| &layer.paths)
            .flat_map(|path| path.points.iter().copied()),
    )
}

/// Positive if `p` lies left of the line from `a` to `b`.
fn cross(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}

/// Convex hull of a set of points, counterclockwise, without collinear
/// points. Andrew's monotone chain algorithm.
pub fn convex_hull(mut points: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    points.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    points.dedup();
    if points.len() < 3 {
        return points;
    }
    let mut hull: Vec<[f64; 2]> = Vec::with_capacity(2 * points.len());
    let chain = |hull: &mut Vec<[f64; 2]>, p: [f64; 2], floor: usize| {
        while hull.len() >= floor + 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0
        {
            hull.pop();
        }
        hull.push(p);
    };
    for &p in &points {
        chain(&mut hull, p, 0);
    }
    let lower = hull.len() - 1;
    for &p in points.iter().rev().skip(1) {
        chain(&mut hull, p, lower);
    }
    hull.pop();
    hull
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn near(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
    }

    fn cube_model(x: f64, y: f64, z: f64) -> Model {
        let mut mesh = Mesh::cuboid(x, y, z);
        // Somewhere off the origin, as models in files often are.
        mesh.translate([37.0, -12.0, 5.0]);
        Model::from_mesh("cube", mesh).unwrap()
    }

    #[test]
    fn hull_of_square_with_inner_and_edge_points() {
        let points = vec![
            [0.0, 0.0],
            [2.0, 0.0],
            [1.0, 0.0],
            [1.0, 1.0],
            [2.0, 2.0],
            [0.0, 2.0],
            [0.0, 2.0],
        ];
        assert_eq!(
            convex_hull(points),
            [[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0]]
        );
        assert_eq!(convex_hull(vec![[1.0, 1.0]]), [[1.0, 1.0]]);
    }

    #[test]
    fn model_is_centred_and_on_the_bed() {
        let model = cube_model(20.0, 10.0, 5.0);
        assert_eq!(model.size, [20.0, 10.0, 5.0]);
        let (lo, hi) = model.mesh.bounds().unwrap();
        assert_eq!(lo, [-10.0, -5.0, 0.0]);
        assert_eq!(hi, [10.0, 5.0, 5.0]);
        assert_eq!(model.hull.len(), 4);
        assert!(Model::from_mesh("empty", Mesh::default()).is_err());
    }

    #[test]
    fn outline_follows_placement() {
        let model = cube_model(20.0, 10.0, 5.0);
        let settings = Settings::default();
        let mut placement = Placement::centered(&settings);
        placement.center = [50.0, 60.0];
        let (lo, hi) = bounds_2d(placed_outline(&model, &placement)).unwrap();
        assert!(near(lo, [40.0, 55.0]) && near(hi, [60.0, 65.0]));

        placement.rotation_deg = 90.0;
        placement.scale = 2.0;
        let outline = placed_outline(&model, &placement);
        let (lo, hi) = bounds_2d(outline.iter().copied()).unwrap();
        assert!(near(lo, [40.0, 40.0]) && near(hi, [60.0, 80.0]));
        assert!(outline_contains(&outline, [50.0, 79.0]));
        assert!(!outline_contains(&outline, [61.0, 60.0]));
    }

    #[test]
    fn placed_mesh_matches_outline() {
        let model = cube_model(20.0, 10.0, 5.0);
        let placement = Placement {
            center: [70.0, 30.0],
            rotation_deg: 30.0,
            scale: 1.5,
        };
        let (lo, hi) = placed_mesh(&model, &placement).bounds().unwrap();
        let (outline_lo, outline_hi) = bounds_2d(placed_outline(&model, &placement)).unwrap();
        // Same place seen from above.
        assert!(near([lo[0], lo[1]], outline_lo));
        assert!(near([hi[0], hi[1]], outline_hi));
        assert!((hi[2] - 7.5).abs() < 1e-9);
    }

    #[test]
    fn placement_checks() {
        let model = cube_model(20.0, 10.0, 5.0);
        let settings = Settings::default();
        let mut placement = Placement::centered(&settings);
        assert!(check_placement(&model, &placement, &settings).is_ok());

        placement.center = [5.0, 110.0];
        assert_eq!(
            check_placement(&model, &placement, &settings)
                .unwrap_err()
                .code,
            "ui.off_bed"
        );
        placement.center = [10.0, 5.0];
        assert!(check_placement(&model, &placement, &settings).is_ok());

        placement = Placement::centered(&settings);
        placement.scale = 60.0;
        assert_eq!(
            check_placement(&model, &placement, &settings)
                .unwrap_err()
                .code,
            "ui.too_tall"
        );
        placement.scale = 0.0;
        assert_eq!(
            check_placement(&model, &placement, &settings)
                .unwrap_err()
                .code,
            "ui.invalid_scale"
        );
    }

    #[test]
    fn models_are_sliced_where_they_are_placed() {
        let settings = Settings {
            skirt_loops: 0,
            ..Settings::default()
        };
        let mut plate = Plate::default();
        plate.add(cube_model(20.0, 10.0, 2.0), &settings);
        plate.add(cube_model(10.0, 10.0, 2.0), &settings);
        plate.items[0].placement.center = [30.0, 40.0];
        plate.items[1].placement.center = [150.0, 60.0];
        assert!(plate.check(&settings).is_ok());
        let toolpaths = slicer_fdm::generate_plate(
            &plate.meshes(),
            slicer_fdm::Placement::AsGiven,
            &settings,
            &mut Diagnostics::new(),
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
        let (lo, hi) = toolpath_bounds(&toolpaths).unwrap();
        // The outer walls lie half a line width inside the outlines.
        assert!(near(lo, [20.225, 35.225]) && near(hi, [154.775, 64.775]));
    }

    #[test]
    fn plate_holds_several_models() {
        let settings = Settings::default();
        let mut plate = Plate::default();
        assert_eq!(plate.check(&settings).unwrap_err().code, "ui.no_model");
        plate.add(cube_model(20.0, 10.0, 5.0), &settings);
        plate.add(cube_model(30.0, 30.0, 5.0), &settings);
        assert_eq!(plate.selected, Some(1));
        // Both in the middle: the one added last is on top.
        assert_eq!(plate.at([110.0, 110.0]), Some(1));
        assert_eq!(plate.at([10.0, 10.0]), None);

        plate.arrange(&settings);
        let placements = plate.placements();
        // The deeper one comes first, 5 mm gap, centred: X 82.5 to 112.5
        // and 117.5 to 137.5, both from Y 95 backwards.
        assert!(near(placements[1].center, [97.5, 110.0]), "{placements:?}");
        assert!(near(placements[0].center, [127.5, 100.0]), "{placements:?}");
        assert_eq!(plate.at([127.5, 100.0]), Some(0));
        plate.arrange(&settings);
        assert_eq!(plate.placements(), placements, "arranging is repeatable");

        plate.items[0].placement.center = [5.0, 110.0];
        let error = plate.check(&settings).unwrap_err();
        assert_eq!(error.code, "ui.off_bed");
        assert!(error.message.starts_with("cube: "), "{}", error.message);

        let serial = plate.serial;
        plate.selected = Some(0);
        plate.remove_selected();
        assert_eq!(plate.items.len(), 1);
        assert_eq!(plate.selected, None);
        assert!(plate.serial > serial);
        plate.remove_selected();
        assert_eq!(plate.items.len(), 1);
    }

    #[test]
    fn loading_reports_broken_files() {
        let error = load_model("broken.stl", b"solid nothing\nfacet").unwrap_err();
        assert_eq!(error.severity, slicer_core::Severity::Error);
        let bytes = slicer_formats::write_stl_binary(&Mesh::cuboid(10.0, 10.0, 10.0));
        let (model, warnings) = load_model("cube.stl", &bytes).unwrap();
        assert_eq!(model.name, "cube.stl");
        assert_eq!(model.size, [10.0, 10.0, 10.0]);
        assert!(warnings.is_empty());
    }
}
