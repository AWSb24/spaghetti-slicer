//! Models on the build plate: loading, placing, rotating, scaling and
//! arranging them. The slicer gets every model where it is placed.

use std::collections::BTreeMap;
use std::sync::Arc;

use slicer_core::{Diagnose, Diagnostic, Diagnostics, Mesh};
use slicer_fdm::{MAX_NEGATIVES, MAX_SUPPORT_VOLUMES, Modifier, SupportVolume, Toolpaths};
use slicer_profiles::Settings;

pub use slicer_fdm::HeightRange;

/// Steps that can be undone.
const UNDO_STEPS: usize = 100;

/// Gap in millimetres between a model and its copy.
const COPY_GAP: f64 = 5.0;

/// Distance in millimetres by which a model may reach over the edge of the
/// bed before it counts as off the bed, to absorb rounding.
const EDGE_TOLERANCE: f64 = 1e-6;

/// Smallest and largest scale factor the plate accepts.
pub const SCALE_RANGE: (f64, f64) = (0.01, 100.0);

/// A linear map of 3D points, by rows.
pub type Mat3 = [[f64; 3]; 3];

/// How far below the lowest point a vertex may lie and still touch the bed,
/// in millimetres, when choosing an orientation.
const CONTACT: f64 = 0.05;

/// Most face directions tried when choosing an orientation.
const ORIENT_CANDIDATES: usize = 40;

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

impl Model {
    /// The model with `m` applied to every point, standing on the bed
    /// again with its centre on the Z axis. Mirroring maps keep the
    /// outside out.
    pub fn transformed(&self, m: Mat3) -> Model {
        // Turns by multiples of 90° give exact 0 and 1, not 6e-17.
        let m = m.map(|row| {
            row.map(|x| {
                if (x - x.round()).abs() < 1e-12 {
                    x.round()
                } else {
                    x
                }
            })
        });
        let mut mesh = self.mesh.clone();
        for v in &mut mesh.vertices {
            *v = apply(m, *v);
        }
        if det(m) < 0.0 {
            mesh.flip();
        }
        Model::from_mesh(self.name.clone(), mesh).unwrap_or_else(|_| self.clone())
    }

    /// The model mirrored along axis 0 (X), 1 (Y) or 2 (Z) of the bed,
    /// while it stands turned by `rotation_deg` around Z.
    pub fn mirrored(&self, axis: usize, rotation_deg: f64) -> Model {
        let mut flip = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        flip[axis.min(2)][axis.min(2)] = -1.0;
        let turn = rotation_z(rotation_deg.to_radians());
        let back = rotation_z(-rotation_deg.to_radians());
        // Into bed directions, mirror there, and back.
        self.transformed(mul(back, mul(flip, turn)))
    }

    /// The model turned so that the direction `down` (in its own
    /// coordinates) points straight down, onto the bed.
    pub fn laid_on(&self, down: [f64; 3]) -> Model {
        self.transformed(rotation_onto(down, [0.0, 0.0, -1.0]))
    }

    /// The direction to lay the model on (see [`Model::laid_on`]) for the
    /// least overhang and the most contact with the bed. The candidates are
    /// the directions of its largest flat faces and of the axes; `[0, 0,
    /// -1]`, as it stands, wins ties.
    pub fn best_down(&self) -> [f64; 3] {
        let faces = faces(&self.mesh);
        // Sum the area of faces pointing the same way.
        let mut groups: Vec<([i64; 3], f64, [f64; 3])> = Vec::new();
        for &(normal, area) in faces.iter().flatten() {
            let key = normal.map(|c| (c * 50.0).round() as i64);
            match groups.iter_mut().find(|(k, ..)| *k == key) {
                Some((_, sum, n)) => {
                    *sum += area;
                    for (a, b) in n.iter_mut().zip(normal) {
                        *a += b * area;
                    }
                }
                None => groups.push((key, area, normal.map(|c| c * area))),
            }
        }
        groups.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut candidates: Vec<[f64; 3]> = vec![
            [0.0, 0.0, -1.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
        ];
        candidates.extend(
            groups
                .iter()
                .take(ORIENT_CANDIDATES)
                .filter_map(|(_, _, n)| unit(*n)),
        );
        let mut best = ([0.0, 0.0, -1.0], f64::INFINITY);
        for down in candidates {
            let cost = orientation_cost(&self.mesh, &faces, down);
            if cost < best.1 - 1e-6 {
                best = (down, cost);
            }
        }
        best.0
    }
}

/// Outward unit normal and area of every triangle; `None` for degenerate
/// ones.
fn faces(mesh: &Mesh) -> Vec<Option<([f64; 3], f64)>> {
    mesh.triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|i| mesh.vertices.get(i as usize).copied());
            let (a, b, c) = (a?, b?, c?);
            let n = cross3(sub3(b, a), sub3(c, a));
            let len = dot3(n, n).sqrt();
            (len > 1e-12).then(|| (n.map(|x| x / len), len / 2.0))
        })
        .collect()
}

/// Area of the faces that hang over (facing down more steeply than 45°)
/// minus the area resting on the bed, with the model laid on `down`;
/// lower is better. `faces` are the normals and areas of the triangles.
fn orientation_cost(mesh: &Mesh, faces: &[Option<([f64; 3], f64)>], down: [f64; 3]) -> f64 {
    let lowest = mesh
        .vertices
        .iter()
        .map(|&v| dot3(v, down))
        .fold(f64::NEG_INFINITY, f64::max);
    let mut cost = 0.0;
    for (t, face) in mesh.triangles.iter().zip(faces) {
        let corners = t.map(|i| mesh.vertices.get(i as usize).copied());
        let ([Some(a), Some(b), Some(c)], Some((normal, area))) = (corners, *face) else {
            continue;
        };
        let facing = dot3(normal, down);
        if facing <= std::f64::consts::FRAC_1_SQRT_2 {
            continue;
        }
        let on_bed = [a, b, c].iter().all(|&p| lowest - dot3(p, down) < CONTACT);
        if on_bed {
            cost -= area;
        } else {
            cost += area * facing;
        }
    }
    cost
}

fn apply(m: Mat3, v: [f64; 3]) -> [f64; 3] {
    m.map(|row| dot3(row, v))
}

fn mul(a: Mat3, b: Mat3) -> Mat3 {
    a.map(|row| [0, 1, 2].map(|c| row[0] * b[0][c] + row[1] * b[1][c] + row[2] * b[2][c]))
}

fn det(m: Mat3) -> f64 {
    dot3(m[0], cross3(m[1], m[2]))
}

fn rotation_x(angle: f64) -> Mat3 {
    let (sin, cos) = angle.sin_cos();
    [[1.0, 0.0, 0.0], [0.0, cos, -sin], [0.0, sin, cos]]
}

fn rotation_y(angle: f64) -> Mat3 {
    let (sin, cos) = angle.sin_cos();
    [[cos, 0.0, sin], [0.0, 1.0, 0.0], [-sin, 0.0, cos]]
}

fn rotation_z(angle: f64) -> Mat3 {
    let (sin, cos) = angle.sin_cos();
    [[cos, -sin, 0.0], [sin, cos, 0.0], [0.0, 0.0, 1.0]]
}

/// The rotation that turns the direction `from` into `to`.
fn rotation_onto(from: [f64; 3], to: [f64; 3]) -> Mat3 {
    let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let (Some(f), Some(t)) = (unit(from), unit(to)) else {
        return identity;
    };
    let axis = cross3(f, t);
    let (sin, cos) = (dot3(axis, axis).sqrt(), dot3(f, t));
    if sin < 1e-9 {
        if cos > 0.0 {
            return identity;
        }
        // Opposite: half a turn around any axis across `from`.
        let across = if f[0].abs() < 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        let k = unit(cross3(f, across)).unwrap_or([0.0, 0.0, 1.0]);
        return [0, 1, 2].map(|r| [0, 1, 2].map(|c| 2.0 * k[r] * k[c] - identity[r][c]));
    }
    // Rodrigues' formula.
    let k = axis.map(|x| x / sin);
    let kx = [[0.0, -k[2], k[1]], [k[2], 0.0, -k[0]], [-k[1], k[0], 0.0]];
    let kk = mul(kx, kx);
    [0, 1, 2].map(|r| [0, 1, 2].map(|c| identity[r][c] + sin * kx[r][c] + (1.0 - cos) * kk[r][c]))
}

fn sub3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit(v: [f64; 3]) -> Option<[f64; 3]> {
    let len = dot3(v, v).sqrt();
    (len > 1e-12).then(|| v.map(|x| x / len))
}

/// Outward unit normal of triangle `index` of `mesh`.
pub fn face_normal(mesh: &Mesh, index: usize) -> Option<[f64; 3]> {
    let t = mesh.triangles.get(index)?;
    let [a, b, c] = t.map(|i| mesh.vertices.get(i as usize).copied());
    let (a, b, c) = (a?, b?, c?);
    unit(cross3(sub3(b, a), sub3(c, a)))
}

/// Reads an STL or 3MF file. Returns the model and the warnings the reader
/// reported, for example about a project with several plates (the first
/// one is used).
pub fn load_model(name: &str, bytes: &[u8]) -> Result<(Model, Diagnostics), Diagnostic> {
    let mut warnings = Diagnostics::new();
    let mesh = slicer_formats::parse_model(bytes, None, &mut warnings)?;
    Ok((Model::from_mesh(name, mesh)?, warnings))
}

/// Reads an STL or 3MF file for a part, modifier, negative volume or
/// support volume of an object, as it is: such a mesh is in the object's
/// coordinates and must not be moved to the middle of the bed. Returns the
/// mesh and the warnings the reader reported.
pub fn load_volume(bytes: &[u8]) -> Result<(Mesh, Diagnostics), Diagnostic> {
    let mut warnings = Diagnostics::new();
    let mesh = slicer_formats::parse_model(bytes, None, &mut warnings)?;
    Ok((mesh, warnings))
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

/// How a part or a volume is moved, turned and scaled inside its object.
///
/// The mesh is transformed around the centre of its own bounding box, then
/// moved by [`MeshTransform::offset`]. Offsets are in the object's
/// coordinates (millimetres), before the object's placement on the bed is
/// applied. The rotation is applied around X, then Y, then Z.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshTransform {
    /// Movement after turning and scaling, in the object's coordinates.
    pub offset: [f64; 3],
    /// Rotation around X, Y and Z in degrees.
    pub rotation_deg: [f64; 3],
    /// Uniform scale factor; 1 keeps the size from the file.
    pub scale: f64,
}

impl Default for MeshTransform {
    fn default() -> Self {
        Self {
            offset: [0.0; 3],
            rotation_deg: [0.0; 3],
            scale: 1.0,
        }
    }
}

impl MeshTransform {
    /// Whether the mesh is left as it is.
    pub fn is_identity(&self) -> bool {
        self.offset == [0.0; 3]
            && self.rotation_deg == [0.0; 3]
            && (self.scale - 1.0).abs() < f64::EPSILON
    }

    /// The rotation and scaling as a matrix, by rows.
    pub fn rotation(&self) -> Mat3 {
        let [x, y, z] = self.rotation_deg.map(f64::to_radians);
        let m = mul(rotation_z(z), mul(rotation_y(y), rotation_x(x)));
        m.map(|row| row.map(|value| value * self.scale))
    }

    /// Where the centre of the mesh's bounding box lands.
    pub fn translation(&self, center: [f64; 3]) -> [f64; 3] {
        let turned = apply(self.rotation(), center);
        [0, 1, 2].map(|axis| center[axis] + self.offset[axis] - turned[axis])
    }

    /// The rotation and scaling that undo [`MeshTransform::rotation`], by
    /// rows.
    fn inverse_rotation(&self) -> Mat3 {
        let [x, y, z] = self.rotation_deg.map(f64::to_radians);
        let m = mul(rotation_x(-x), mul(rotation_y(-y), rotation_z(-z)));
        let inverse = 1.0 / self.scale;
        m.map(|row| row.map(|value| value * inverse))
    }

    /// `mesh` with this transform undone: the geometry before [`Self::applied`].
    pub fn inverse_applied(&self, mesh: &Mesh) -> Mesh {
        if self.is_identity() {
            return mesh.clone();
        }
        let Some((lo, hi)) = mesh.bounds() else {
            return mesh.clone();
        };
        // The centre of the transformed mesh is the old centre moved by the
        // offset; turning and scaling keep the centre.
        let center = [0, 1, 2].map(|axis| (lo[axis] + hi[axis]) / 2.0);
        let origin = [0, 1, 2].map(|axis| center[axis] - self.offset[axis]);
        let rotation = self.inverse_rotation();
        let mut transformed = mesh.clone();
        for vertex in &mut transformed.vertices {
            let relative = [0, 1, 2].map(|axis| vertex[axis] - center[axis]);
            let placed = apply(rotation, relative);
            for (axis, value) in vertex.iter_mut().enumerate() {
                *value = placed[axis] + origin[axis];
            }
        }
        transformed
    }

    /// `mesh` turned and scaled around its centre and moved by the offset.
    pub fn applied(&self, mesh: &Mesh) -> Mesh {
        if self.is_identity() {
            return mesh.clone();
        }
        let Some((lo, hi)) = mesh.bounds() else {
            return mesh.clone();
        };
        let center = [0, 1, 2].map(|axis| (lo[axis] + hi[axis]) / 2.0);
        let rotation = self.rotation();
        let translation = self.translation(center);
        let mut transformed = mesh.clone();
        for vertex in &mut transformed.vertices {
            let placed = apply(rotation, *vertex);
            for (axis, value) in vertex.iter_mut().enumerate() {
                *value = placed[axis] + translation[axis];
            }
        }
        transformed
    }
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

/// A mesh that belongs to `model` (a part, modifier, negative volume or
/// support volume), turned, scaled and moved with it onto the bed.
pub fn placed_with(mesh: &Mesh, model: &Model, placement: &Placement) -> Mesh {
    let mut placed = mesh.clone();
    placed.rotate_z(placement.rotation_deg.to_radians());
    for v in &mut placed.vertices {
        for c in v.iter_mut() {
            *c *= placement.scale;
        }
    }
    let mut boxed = model.mesh.clone();
    boxed.rotate_z(placement.rotation_deg.to_radians());
    for v in &mut boxed.vertices {
        for c in v.iter_mut() {
            *c *= placement.scale;
        }
    }
    if let Some((lo, hi)) = boxed.bounds() {
        placed.translate([
            placement.center[0] - (lo[0] + hi[0]) / 2.0,
            placement.center[1] - (lo[1] + hi[1]) / 2.0,
            0.0,
        ]);
    }
    placed
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

/// What a [`Volume`] of an object does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeKind {
    /// The object prints with settings of its own inside it.
    Modifier,
    /// Its volume is cut out of the object.
    Negative,
    /// Overhangs inside it get no support.
    SupportBlocker,
    /// Overhangs inside it get support.
    SupportEnforcer,
}

impl VolumeKind {
    /// The name for people, as the object tree shows it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Modifier => "Modifier",
            Self::Negative => "Negative volume",
            Self::SupportBlocker => "Support blocker",
            Self::SupportEnforcer => "Support enforcer",
        }
    }

    /// The settings it may set for its part of the object, in the form
    /// [`Settings::set`] takes; none for kinds that change no setting.
    pub fn settings(self) -> &'static [&'static str] {
        match self {
            Self::Modifier => slicer_fdm::MODIFIER_SETTINGS,
            _ => &[],
        }
    }

    /// Largest number of volumes of this kind one object may have; the
    /// slicer takes no more.
    pub fn limit(self) -> usize {
        match self {
            Self::Modifier => slicer_fdm::MAX_MODIFIERS,
            Self::Negative => MAX_NEGATIVES,
            Self::SupportBlocker | Self::SupportEnforcer => MAX_SUPPORT_VOLUMES,
        }
    }
}

/// A mesh of an object that is not printed: a modifier, a negative volume,
/// a support blocker or enforcer (see [`VolumeKind`]). The mesh is in the
/// same coordinates as the object's, so it moves and turns with it.
#[derive(Debug, Clone, PartialEq)]
pub struct Volume {
    /// The name for people; unique among the volumes of its object.
    pub name: String,
    /// What it does.
    pub kind: VolumeKind,
    /// The geometry, in the object's coordinates.
    pub mesh: Mesh,
    /// How it is moved, turned and scaled inside the object.
    pub transform: MeshTransform,
    /// Settings by name with their values as text, in the form
    /// [`Settings::set`] takes; only the keys of [`VolumeKind::settings`]
    /// count. Empty for kinds that change no setting.
    pub values: Vec<(String, String)>,
}

impl Volume {
    /// A volume of `kind` that changes nothing yet.
    pub fn new(name: impl Into<String>, kind: VolumeKind, mesh: Mesh) -> Self {
        Self {
            name: name.into(),
            kind,
            mesh,
            transform: MeshTransform::default(),
            values: Vec::new(),
        }
    }

    /// The mesh as it sits in the object: turned, scaled and moved by its
    /// transform.
    pub fn transformed_mesh(&self) -> Mesh {
        self.transform.applied(&self.mesh)
    }
}

/// A part of an object: a mesh in the object's own coordinates. The object
/// is printed as all its parts together.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    /// The name for people; unique among the parts of its object.
    pub name: String,
    /// The geometry, in the object's coordinates.
    pub mesh: Arc<Mesh>,
    /// How it is moved, turned and scaled inside the object.
    pub transform: MeshTransform,
    /// The extruder that prints the whole part, counted from 0; `None`
    /// keeps the extruders the mesh came with.
    pub extruder: Option<u32>,
}

/// A model and where it sits.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// The model as loaded; copies share it. With parts of its own this is
    /// all of them together.
    pub model: Arc<Model>,
    /// Its position, rotation and scale.
    pub placement: Placement,
    /// Settings this model prints with instead of the plate's, by key, as
    /// text; only keys of [`slicer_fdm::OBJECT_SETTINGS`]. Empty: the
    /// plate's settings.
    pub overrides: BTreeMap<String, String>,
    /// The filament slot that prints the whole object, counted from 0;
    /// `None` keeps the extruders the parts and the mesh came with. A part
    /// with an extruder of its own keeps it.
    pub extruder: Option<u32>,
    /// The parts the object is made of, in the model's coordinates. Empty:
    /// the model is its only part.
    pub parts: Vec<Part>,
    /// Modifiers, negative volumes and support volumes of the object, in
    /// the model's coordinates.
    pub volumes: Vec<Volume>,
    /// Ranges of height above the bed that print with settings of their
    /// own (see [`slicer_fdm::HeightRange`]).
    pub ranges: Vec<HeightRange>,
}

impl Item {
    /// A model without parts, volumes or ranges of its own.
    pub fn new(model: Model, placement: Placement) -> Self {
        Self {
            model: Arc::new(model),
            placement,
            overrides: BTreeMap::new(),
            extruder: None,
            parts: Vec::new(),
            volumes: Vec::new(),
            ranges: Vec::new(),
        }
    }

    /// How many parts the object has; one while it is only its model.
    pub fn part_count(&self) -> usize {
        self.parts.len().max(1)
    }

    /// The name of part `index`, or `None` past the last one.
    pub fn part_name(&self, index: usize) -> Option<&str> {
        match self.parts.get(index) {
            Some(part) => Some(&part.name),
            None if index == 0 && self.parts.is_empty() => Some(&self.model.name),
            None => None,
        }
    }

    /// The extruder part `index` prints with: its own, else the object's,
    /// else the one of its mesh if all triangles agree.
    pub fn part_extruder(&self, index: usize) -> Option<u32> {
        match self.parts.get(index) {
            Some(part) => part
                .extruder
                .or(self.extruder)
                .or_else(|| uniform_extruder(&part.mesh)),
            None if index == 0 && self.parts.is_empty() => {
                self.extruder.or_else(|| uniform_extruder(&self.model.mesh))
            }
            None => None,
        }
    }

    /// Makes the model a part of its own, so that its parts can change.
    fn materialize_parts(&mut self) {
        if self.parts.is_empty() {
            self.parts.push(Part {
                name: self.model.name.clone(),
                mesh: Arc::new(self.model.mesh.clone()),
                transform: MeshTransform::default(),
                // The object's extruder (if any) and the mesh's own apply.
                extruder: None,
            });
        }
    }

    /// Builds the model from the parts again, each with its extruder; the
    /// parts stand on the bed with the box of all of them centred on the Z
    /// axis.
    pub fn rebuild(&mut self) {
        if self.parts.is_empty() {
            return;
        }
        let mut merged = Mesh::default();
        for part in &self.parts {
            let mut mesh = part.transform.applied(&part.mesh);
            if let Some(extruder) = part.extruder.or(self.extruder) {
                mesh.set_extruder(0, extruder);
            }
            merged.merge(&mesh);
        }
        if let Ok(model) = Model::from_mesh(self.model.name.clone(), merged) {
            self.model = Arc::new(model);
        }
    }

    /// Adds `mesh` as a part of this object, in the object's coordinates
    /// (for example as a CAD program exported it with the object); the
    /// name gets a number if it is taken already.
    pub fn add_part(&mut self, name: &str, mesh: Mesh) {
        self.materialize_parts();
        let taken: Vec<String> = self.parts.iter().map(|p| p.name.clone()).collect();
        let name = unique_name(taken.iter().map(String::as_str), name);
        self.parts.push(Part {
            name,
            mesh: Arc::new(mesh),
            transform: MeshTransform::default(),
            extruder: None,
        });
        self.rebuild();
    }

    /// Removes part `index`, unless it is the object's only part.
    pub fn remove_part(&mut self, index: usize) -> bool {
        self.materialize_parts();
        if index >= self.parts.len() || self.parts.len() < 2 {
            return false;
        }
        self.parts.remove(index);
        self.rebuild();
        true
    }

    /// Gives part `index` the extruder `extruder` (from 0), or its mesh's
    /// own extruders back with `None`.
    pub fn set_part_extruder(&mut self, index: usize, extruder: Option<u32>) -> bool {
        self.materialize_parts();
        let Some(part) = self.parts.get_mut(index) else {
            return false;
        };
        if part.extruder == extruder {
            return false;
        }
        part.extruder = extruder;
        self.rebuild();
        true
    }

    /// How part `index` is moved, turned and scaled inside the object; the
    /// identity for an object that is only its model.
    pub fn part_transform(&self, index: usize) -> Option<MeshTransform> {
        match self.parts.get(index) {
            Some(part) => Some(part.transform),
            None if index == 0 && self.parts.is_empty() => Some(MeshTransform::default()),
            None => None,
        }
    }

    /// Moves, turns and scales part `index` inside the object (see
    /// [`MeshTransform`]); the object is built from its parts again.
    pub fn set_part_transform(&mut self, index: usize, transform: MeshTransform) -> bool {
        if index >= self.part_count() {
            return false;
        }
        self.materialize_parts();
        let Some(part) = self.parts.get_mut(index) else {
            return false;
        };
        if part.transform == transform {
            return false;
        }
        part.transform = transform;
        self.rebuild();
        true
    }

    /// Gives the whole object the filament slot `extruder` (from 0), or
    /// its parts' and mesh's own extruders back with `None`. Parts with an
    /// extruder of their own keep it. Returns whether the object changed.
    pub fn set_object_extruder(&mut self, extruder: Option<u32>) -> bool {
        let changed = self.extruder != extruder;
        self.extruder = extruder;
        if !self.parts.is_empty() || extruder.is_some() {
            self.materialize_parts();
            self.rebuild();
        }
        changed
    }

    /// Adds a volume of `kind`. Fails if the object has as many volumes of
    /// that kind as the slicer takes (see [`VolumeKind::limit`]).
    pub fn add_volume(&mut self, kind: VolumeKind, name: &str, mesh: Mesh) -> bool {
        let count = self.volumes.iter().filter(|v| v.kind == kind).count();
        if count >= kind.limit() {
            return false;
        }
        let taken: Vec<String> = self.volumes.iter().map(|v| v.name.clone()).collect();
        let name = unique_name(taken.iter().map(String::as_str), name);
        self.volumes.push(Volume::new(name, kind, mesh));
        true
    }

    /// Removes volume `index`.
    pub fn remove_volume(&mut self, index: usize) -> bool {
        if index >= self.volumes.len() {
            return false;
        }
        self.volumes.remove(index);
        true
    }

    /// Moves, turns and scales volume `index` inside the object (see
    /// [`MeshTransform`]).
    pub fn set_volume_transform(&mut self, index: usize, transform: MeshTransform) -> bool {
        let Some(volume) = self.volumes.get_mut(index) else {
            return false;
        };
        if volume.transform == transform {
            return false;
        }
        volume.transform = transform;
        true
    }

    /// The modifiers of this object, as slicing takes them: their meshes
    /// turned, scaled and moved with the object.
    pub fn modifiers(&self) -> Vec<Modifier> {
        self.volumes
            .iter()
            .filter(|v| v.kind == VolumeKind::Modifier)
            .map(|v| Modifier {
                mesh: placed_with(&v.transformed_mesh(), &self.model, &self.placement),
                values: v.values.clone(),
            })
            .collect()
    }

    /// The meshes cut out of this object, as slicing takes them: turned,
    /// scaled and moved with the object.
    pub fn negatives(&self) -> Vec<Mesh> {
        self.volumes
            .iter()
            .filter(|v| v.kind == VolumeKind::Negative)
            .map(|v| placed_with(&v.transformed_mesh(), &self.model, &self.placement))
            .collect()
    }

    /// The support blockers and enforcers of this object, as slicing takes
    /// them: turned, scaled and moved with the object.
    pub fn support_volumes(&self) -> Vec<SupportVolume> {
        self.volumes
            .iter()
            .filter_map(|v| {
                let mesh = placed_with(&v.transformed_mesh(), &self.model, &self.placement);
                match v.kind {
                    VolumeKind::SupportBlocker => Some(SupportVolume::blocker(mesh)),
                    VolumeKind::SupportEnforcer => Some(SupportVolume::enforcer(mesh)),
                    _ => None,
                }
            })
            .collect()
    }
}

/// The extruder of `mesh` if every triangle agrees on one; `None` for a
/// mesh without triangles or with several.
fn uniform_extruder(mesh: &Mesh) -> Option<u32> {
    let mut used = mesh.used_extruders();
    (used.len() == 1).then(|| used.remove(0))
}

/// `name`, with a number in brackets if it is in `taken` already.
fn unique_name<'a>(taken: impl IntoIterator<Item = &'a str>, name: &str) -> String {
    let taken: Vec<&str> = taken.into_iter().collect();
    if !taken.contains(&name) {
        return name.to_owned();
    }
    (2..)
        .map(|n| format!("{name} ({n})"))
        .find(|candidate| !taken.contains(&candidate.as_str()))
        .unwrap_or_else(|| name.to_owned())
}

/// The settings, ranges and volumes of every object of a plate, as slicing
/// takes them (see [`slicer_fdm::OwnSettings`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlateOwnSettings {
    /// The settings of every object of its own, in the order of the
    /// objects; `None` where it prints with the plate's.
    pub object_settings: Vec<Option<Settings>>,
    /// The height ranges of every object.
    pub ranges: Vec<Vec<HeightRange>>,
    /// The modifiers of every object.
    pub modifiers: Vec<Vec<Modifier>>,
    /// The negative volumes of every object.
    pub negatives: Vec<Vec<Mesh>>,
    /// The support blockers and enforcers of every object.
    pub support_volumes: Vec<Vec<SupportVolume>>,
}

impl PlateOwnSettings {
    /// These settings for [`slicer_fdm::generate_plate_with_own_settings`].
    pub fn borrowed(&self) -> slicer_fdm::OwnSettings<'_> {
        slicer_fdm::OwnSettings {
            objects: &self.object_settings,
            ranges: &self.ranges,
            modifiers: &self.modifiers,
            negatives: &self.negatives,
            support_volumes: &self.support_volumes,
        }
    }
}

/// Checks that `values` only name settings of `allowed` and that each fits
/// `settings`; `what` names the volume or range in the message.
fn check_values(
    settings: &Settings,
    allowed: &[&str],
    values: &[(String, String)],
    what: &str,
) -> Result<(), Diagnostic> {
    for (key, value) in values {
        if !allowed.contains(&key.as_str()) {
            return Err(Diagnostic::error(
                "ui.setting_not_allowed",
                format!("{what}: '{key}' cannot be set there"),
            )
            .with("key", key)
            .with("allowed", allowed.join(", ")));
        }
        settings.clone().set(key, value).map_err(|e| {
            let mut error = e.diagnostic().with("where", what);
            error.message = format!("{what}: {}", error.message);
            error
        })?;
    }
    Ok(())
}

/// The models on the build plate and which are selected.
#[derive(Debug, Clone, Default)]
pub struct Plate {
    /// The models in the order they were added.
    pub items: Vec<Item>,
    /// Index of the selected model, which the side panel edits; with
    /// several selected, the last one chosen.
    pub selected: Option<usize>,
    /// Every selected model, in the order they were chosen. Empty while the
    /// selection was set through [`Plate::selected`] alone.
    pub multi: Vec<usize>,
    /// Counts models added and removed, to tell whether a slice shows the
    /// current plate.
    pub serial: u64,
}

impl Plate {
    /// Puts a model in the middle of the bed and selects it.
    pub fn add(&mut self, model: Model, settings: &Settings) {
        self.items
            .push(Item::new(model, Placement::centered(settings)));
        self.select_only(self.items.len() - 1);
        self.serial += 1;
    }

    /// Adds a copy of `item` beside it, to the right if that stays on the
    /// bed and to the left otherwise, and selects the copy.
    pub fn add_copy(&mut self, item: &Item, settings: &Settings) {
        let width = bounds_2d(placed_outline(&item.model, &item.placement))
            .map_or(0.0, |(lo, hi)| hi[0] - lo[0]);
        let mut placement = item.placement;
        let step = width + COPY_GAP;
        placement.center[0] = if placement.center[0] + step + width / 2.0 <= settings.bed_size_x {
            placement.center[0] + step
        } else {
            (placement.center[0] - step).max(width / 2.0)
        };
        let mut copy = item.clone();
        copy.placement = placement;
        self.items.push(copy);
        self.select_only(self.items.len() - 1);
        self.serial += 1;
    }

    /// Whether `other` has the same models in the same places; the
    /// selection does not count.
    pub fn same_as(&self, other: &Plate) -> bool {
        self.items.len() == other.items.len()
            && self.items.iter().zip(&other.items).all(|(a, b)| {
                Arc::ptr_eq(&a.model, &b.model)
                    && a.placement == b.placement
                    && a.overrides == b.overrides
                    && a.extruder == b.extruder
                    && a.ranges == b.ranges
                    && same_parts(a, b)
                    && same_volumes(a, b)
            })
    }

    /// Selects model `index` alone.
    pub fn select_only(&mut self, index: usize) {
        if index < self.items.len() {
            self.selected = Some(index);
            self.multi = vec![index];
        }
    }

    /// Adds model `index` to the selection, or takes it out.
    pub fn toggle_selected(&mut self, index: usize) {
        if index >= self.items.len() {
            return;
        }
        if self.is_selected(index) {
            self.multi.retain(|&i| i != index);
            if self.selected == Some(index) {
                self.selected = self.multi.last().copied();
            }
        } else {
            self.multi.push(index);
            self.selected = Some(index);
        }
    }

    /// Selects the models from `from` to `to`, in their order on the plate.
    pub fn select_range(&mut self, from: usize, to: usize) {
        if from >= self.items.len() || to >= self.items.len() {
            return;
        }
        let (lo, hi) = (from.min(to), from.max(to));
        self.multi = (lo..=hi).collect();
        self.selected = Some(to);
    }

    /// Whether model `index` is selected, alone or together with others.
    pub fn is_selected(&self, index: usize) -> bool {
        self.selected == Some(index) || self.multi.contains(&index)
    }

    /// Every selected model, in the order of the plate.
    pub fn selected_indices(&self) -> Vec<usize> {
        let mut indices = self.multi.clone();
        if let Some(index) = self.selected
            && !indices.contains(&index)
        {
            indices.push(index);
        }
        indices.retain(|&i| i < self.items.len());
        indices.sort_unstable();
        indices.dedup();
        indices
    }

    /// Selects nothing.
    pub fn clear_selection(&mut self) {
        self.selected = None;
        self.multi.clear();
    }

    /// Gives model `index` the settings `overrides` of its own (see
    /// [`Item::overrides`]); counts as a change of the plate if they differ.
    pub fn set_overrides(&mut self, index: usize, overrides: BTreeMap<String, String>) {
        if let Some(item) = self.items.get_mut(index)
            && item.overrides != overrides
        {
            item.overrides = overrides;
            self.serial += 1;
        }
    }

    /// Adds a volume of `kind` to model `index`; false if it has as many
    /// of that kind as the slicer takes.
    pub fn add_volume(&mut self, index: usize, kind: VolumeKind, name: &str, mesh: Mesh) -> bool {
        let Some(item) = self.items.get_mut(index) else {
            return false;
        };
        if !item.add_volume(kind, name, mesh) {
            return false;
        }
        self.serial += 1;
        true
    }

    /// Removes volume `volume` of model `index`.
    pub fn remove_volume(&mut self, index: usize, volume: usize) -> bool {
        let Some(item) = self.items.get_mut(index) else {
            return false;
        };
        if !item.remove_volume(volume) {
            return false;
        }
        self.serial += 1;
        true
    }

    /// Sets the settings of volume `volume` of model `index`; fails if
    /// they are not valid for its kind.
    pub fn set_volume_values(
        &mut self,
        index: usize,
        volume: usize,
        values: Vec<(String, String)>,
        settings: &Settings,
    ) -> Result<bool, Diagnostic> {
        let Some(item) = self.items.get_mut(index) else {
            return Ok(false);
        };
        let Some(entry) = item.volumes.get(volume) else {
            return Ok(false);
        };
        check_values(settings, entry.kind.settings(), &values, &entry.name)?;
        if entry.values == values {
            return Ok(false);
        }
        item.volumes[volume].values = values;
        self.serial += 1;
        Ok(true)
    }

    /// Adds a height range to model `index`.
    pub fn add_range(&mut self, index: usize, range: HeightRange) -> bool {
        let Some(item) = self.items.get_mut(index) else {
            return false;
        };
        item.ranges.push(range);
        self.serial += 1;
        true
    }

    /// Removes height range `range` of model `index`.
    pub fn remove_range(&mut self, index: usize, range: usize) -> bool {
        let Some(item) = self.items.get_mut(index) else {
            return false;
        };
        if range >= item.ranges.len() {
            return false;
        }
        item.ranges.remove(range);
        self.serial += 1;
        true
    }

    /// Sets height range `range` of model `index` to `from` to `to`
    /// millimetres with `values`; fails if the bounds or values are not
    /// valid.
    pub fn set_range(
        &mut self,
        index: usize,
        range: usize,
        from: f64,
        to: f64,
        values: Vec<(String, String)>,
        settings: &Settings,
    ) -> Result<bool, Diagnostic> {
        let Some(item) = self.items.get_mut(index) else {
            return Ok(false);
        };
        let Some(entry) = item.ranges.get(range) else {
            return Ok(false);
        };
        if !(from.is_finite() && to.is_finite() && from < to) {
            return Err(Diagnostic::error(
                "ui.invalid_range",
                format!("the height range must start below where it ends ({from} to {to} mm)"),
            ));
        }
        check_values(
            settings,
            slicer_fdm::RANGE_SETTINGS,
            &values,
            "height range",
        )?;
        if entry.from == from && entry.to == to && entry.values == values {
            return Ok(false);
        }
        let entry = &mut item.ranges[range];
        entry.from = from;
        entry.to = to;
        entry.values = values;
        self.serial += 1;
        Ok(true)
    }

    /// The settings of every model of its own, in order: `settings` with
    /// its overrides, or `None` for a model without. Fails, naming the
    /// model, if an override is not valid with `settings`.
    pub fn object_settings(
        &self,
        settings: &Settings,
    ) -> Result<Vec<Option<Settings>>, Diagnostic> {
        self.items
            .iter()
            .map(|item| {
                if item.overrides.is_empty() {
                    return Ok(None);
                }
                let mut own = settings.clone();
                let values = item.overrides.iter().map(|(k, v)| (k.as_str(), v.as_str()));
                own.set_all(values).map_err(|e| {
                    let mut error = e.diagnostic().with("object", &item.model.name);
                    error.message = format!(
                        "{} (settings of the object): {}",
                        item.model.name, error.message
                    );
                    error
                })?;
                Ok(Some(own))
            })
            .collect()
    }

    /// Removes the selected model alone; nothing is selected afterwards.
    pub fn remove_selected(&mut self) {
        let indices: Vec<usize> = self.selected.into_iter().collect();
        self.remove_indices(indices);
    }

    /// Removes every selected model; nothing is selected afterwards.
    pub fn remove_selected_all(&mut self) {
        let indices = self.selected_indices();
        self.remove_indices(indices);
    }

    /// Removes the models at `indices`; nothing is selected afterwards.
    pub fn remove_indices(&mut self, indices: impl IntoIterator<Item = usize>) {
        let mut indices: Vec<usize> = indices
            .into_iter()
            .filter(|&index| index < self.items.len())
            .collect();
        indices.sort_unstable();
        indices.dedup();
        if indices.is_empty() {
            return;
        }
        for index in indices.into_iter().rev() {
            self.items.remove(index);
        }
        self.clear_selection();
        self.serial += 1;
    }

    /// Puts `model` in place of the model at `index` (for example turned
    /// or mirrored); copies that shared the old model keep it. Parts and
    /// volumes were placed against the old geometry and are dropped; the
    /// height ranges stay.
    pub fn set_model(&mut self, index: usize, model: Model) {
        if let Some(item) = self.items.get_mut(index) {
            item.model = Arc::new(model);
            item.parts.clear();
            item.volumes.clear();
            self.serial += 1;
        }
    }

    /// Adds `mesh` as another part of model `index`, in the object's
    /// coordinates (see [`Item::add_part`]).
    pub fn add_part(&mut self, index: usize, name: &str, mesh: Mesh) -> bool {
        let Some(item) = self.items.get_mut(index) else {
            return false;
        };
        item.add_part(name, mesh);
        self.serial += 1;
        true
    }

    /// Removes part `part` of model `index`, unless it is its only part.
    pub fn remove_part(&mut self, index: usize, part: usize) -> bool {
        let Some(item) = self.items.get_mut(index) else {
            return false;
        };
        if !item.remove_part(part) {
            return false;
        }
        self.serial += 1;
        true
    }

    /// Gives part `part` of model `index` the extruder `extruder` (from 0),
    /// or its mesh's own back with `None`.
    pub fn set_part_extruder(&mut self, index: usize, part: usize, extruder: Option<u32>) -> bool {
        let Some(item) = self.items.get_mut(index) else {
            return false;
        };
        if !item.set_part_extruder(part, extruder) {
            return false;
        }
        self.serial += 1;
        true
    }

    /// Moves, turns and scales part `part` of model `index` inside the
    /// object (see [`MeshTransform`]).
    pub fn set_part_transform(
        &mut self,
        index: usize,
        part: usize,
        transform: MeshTransform,
    ) -> bool {
        let Some(item) = self.items.get_mut(index) else {
            return false;
        };
        if !item.set_part_transform(part, transform) {
            return false;
        }
        self.serial += 1;
        true
    }

    /// Moves, turns and scales volume `volume` of model `index` inside the
    /// object (see [`MeshTransform`]).
    pub fn set_volume_transform(
        &mut self,
        index: usize,
        volume: usize,
        transform: MeshTransform,
    ) -> bool {
        let Some(item) = self.items.get_mut(index) else {
            return false;
        };
        if !item.set_volume_transform(volume, transform) {
            return false;
        }
        self.serial += 1;
        true
    }

    /// Gives model `index` the filament slot `extruder` (from 0) for its
    /// whole object, or its parts' and mesh's own back with `None`.
    pub fn set_object_extruder(&mut self, index: usize, extruder: Option<u32>) -> bool {
        let Some(item) = self.items.get_mut(index) else {
            return false;
        };
        if !item.set_object_extruder(extruder) {
            return false;
        }
        self.serial += 1;
        true
    }

    /// How many parts print with each extruder, counted from 0; a model
    /// that is its only part counts once. The list ends with the last
    /// extruder used.
    pub fn extruder_usage(&self) -> Vec<usize> {
        let mut usage: Vec<usize> = Vec::new();
        for item in &self.items {
            for part in 0..item.part_count() {
                let extruder = item.part_extruder(part).unwrap_or(0) as usize;
                if usage.len() <= extruder {
                    usage.resize(extruder + 1, 0);
                }
                usage[extruder] += 1;
            }
        }
        usage
    }

    /// Cuts model `index` at `height` millimetres above the bed. `keep`
    /// says whether the part below and the part above stay. The lower part
    /// stays where it was; the upper part, turned upside down if
    /// `flip_upper`, goes beside it (right, or left at the bed edge), or
    /// in its place if the lower part is not kept. The kept part is
    /// selected. Returns false and changes nothing if no part with volume
    /// is kept. The upper part already stands on its cut face; `flip_upper`
    /// turns it upside down, for a top that is flat.
    pub fn cut(
        &mut self,
        index: usize,
        height: f64,
        keep: [bool; 2],
        flip_upper: bool,
        settings: &Settings,
    ) -> bool {
        let Some(item) = self.items.get(index).cloned() else {
            return false;
        };
        let placement = item.placement;
        let parts = item.model.mesh.cut(height / placement.scale);
        // Each part with where its middle lies on the bed.
        let [lower, upper] = [0, 1].map(|side| {
            let mesh = &parts[side];
            let (lo, hi) = mesh.bounds()?;
            if !keep[side] || mesh.volume() < 1e-9 {
                return None;
            }
            let name = format!("{} ({})", item.model.name, ["bottom", "top"][side]);
            let mut model = Model::from_mesh(name, mesh.clone()).ok()?;
            if side == 1 && flip_upper {
                model = model.laid_on([0.0, 0.0, 1.0]);
            }
            let offset = placement.turn([(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0]);
            let mut at = placement;
            at.center = [
                placement.center[0] + offset[0],
                placement.center[1] + offset[1],
            ];
            let mut new = Item::new(model, at);
            new.overrides = item.overrides.clone();
            new.extruder = item.extruder;
            Some(new)
        });
        let width = |item: &Item| {
            bounds_2d(placed_outline(&item.model, &item.placement))
                .map_or(0.0, |(lo, hi)| hi[0] - lo[0])
        };
        match (lower, upper) {
            (None, None) => return false,
            (Some(lower), None) => self.items[index] = lower,
            (None, Some(upper)) => self.items[index] = upper,
            (Some(lower), Some(mut upper)) => {
                let step = (width(&lower) + width(&upper)) / 2.0 + COPY_GAP;
                let x = lower.placement.center[0];
                upper.placement.center[0] = if x + step + width(&upper) / 2.0 <= settings.bed_size_x
                {
                    x + step
                } else {
                    (x - step).max(width(&upper) / 2.0)
                };
                upper.placement.center[1] = lower.placement.center[1];
                self.items[index] = lower;
                self.items.push(upper);
            }
        }
        self.select_only(index);
        self.serial += 1;
        true
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
    /// [`slicer_fdm::arrange`]); rotation and scale stay. With
    /// `arrange_outlines` or `arrange_rotation` it packs their outlines
    /// instead ([`slicer_fdm::arrange_outlines`]), with `arrange_nonconvex`
    /// their real outlines ([`slicer_fdm::arrange_footprints`]), and with
    /// `arrange_rotation` adds the turns it chose to their rotation.
    pub fn arrange(&mut self, settings: &Settings) {
        if slicer_fdm::arranges_outlines(settings) {
            let outlines: Vec<Vec<[f64; 2]>> = self
                .items
                .iter()
                .map(|item| placed_outline(&item.model, &item.placement))
                .collect();
            let gap = slicer_fdm::ARRANGE_GAP;
            let spots = if settings.arrange_nonconvex {
                let footprints: Vec<slicer_core::Shapes> = self
                    .items
                    .iter()
                    .map(|item| slicer_fdm::top_view(&placed_mesh(&item.model, &item.placement)))
                    .collect();
                slicer_fdm::arrange_footprints(&footprints, settings, gap)
            } else {
                slicer_fdm::arrange_outlines(&outlines, settings, gap)
            };
            for ((item, outline), spot) in self.items.iter_mut().zip(&outlines).zip(spots) {
                // The centre of the bounding box of the outline as placed,
                // which is what the placement keeps.
                let Some((lo, hi)) = bounds_2d(outline.iter().map(|&p| spot.place(p))) else {
                    continue;
                };
                item.placement.center = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
                if spot.turn != 0.0 {
                    item.placement.rotation_deg = (item.placement.rotation_deg + spot.turn) % 360.0;
                }
            }
            return;
        }
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

    /// The settings, ranges and volumes of every object, for slicing.
    /// Fails, naming the object, if a value there is not valid.
    pub fn own_settings(&self, settings: &Settings) -> Result<PlateOwnSettings, Diagnostic> {
        let own = PlateOwnSettings {
            object_settings: self.object_settings(settings)?,
            ranges: self.items.iter().map(|item| item.ranges.clone()).collect(),
            modifiers: self.items.iter().map(Item::modifiers).collect(),
            negatives: self.items.iter().map(Item::negatives).collect(),
            support_volumes: self.items.iter().map(Item::support_volumes).collect(),
        };
        for (item, ranges) in self.items.iter().zip(&own.ranges) {
            for range in ranges {
                if !(range.from.is_finite() && range.to.is_finite() && range.from < range.to) {
                    return Err(Diagnostic::error(
                        "ui.invalid_range",
                        format!(
                            "{}: the height range from {} to {} mm does not end above where it starts",
                            item.model.name, range.from, range.to
                        ),
                    ));
                }
                check_values(
                    settings,
                    slicer_fdm::RANGE_SETTINGS,
                    &range.values,
                    "height range",
                )
                .map_err(|error| named(error, &item.model.name))?;
            }
            for volume in &item.volumes {
                let count = item
                    .volumes
                    .iter()
                    .filter(|v| v.kind == volume.kind)
                    .count();
                if count > volume.kind.limit() {
                    return Err(Diagnostic::error(
                        "ui.too_many_volumes",
                        format!(
                            "{}: {} {} volumes, but the slicer takes {}",
                            item.model.name,
                            count,
                            volume.kind.label().to_lowercase(),
                            volume.kind.limit()
                        ),
                    ));
                }
                check_values(
                    settings,
                    volume.kind.settings(),
                    &volume.values,
                    &volume.name,
                )
                .map_err(|error| named(error, &item.model.name))?;
            }
        }
        Ok(own)
    }

    /// Checks every model with [`check_placement`], and its own settings;
    /// the message names the model when there are several.
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
        self.own_settings(settings).map(|_| ())
    }

    /// The meshes to slice, each where it is placed.
    pub fn meshes(&self) -> Vec<Mesh> {
        self.items
            .iter()
            .map(|item| placed_mesh(&item.model, &item.placement))
            .collect()
    }
}

/// Whether two objects have the same parts, judged by their names,
/// extruders and transforms; a change of geometry always comes with another
/// model.
fn same_parts(a: &Item, b: &Item) -> bool {
    a.parts.len() == b.parts.len()
        && a.parts.iter().zip(&b.parts).all(|(x, y)| {
            x.name == y.name && x.extruder == y.extruder && x.transform == y.transform
        })
}

/// Whether two objects have the same volumes, judged by their names, kinds,
/// transforms and values; their meshes never change in place.
fn same_volumes(a: &Item, b: &Item) -> bool {
    a.volumes.len() == b.volumes.len()
        && a.volumes.iter().zip(&b.volumes).all(|(x, y)| {
            x.name == y.name
                && x.kind == y.kind
                && x.transform == y.transform
                && x.values == y.values
        })
}

/// `error` with the name of `object` in front of its message.
fn named(mut error: Diagnostic, object: &str) -> Diagnostic {
    error.message = format!("{object}: {}", error.message);
    error
}

/// The states of the plate that can be undone and redone. Changes are
/// recorded when they are finished, so dragging a model is one step.
#[derive(Debug, Default)]
pub struct History {
    /// The last recorded state.
    current: Plate,
    undo: Vec<Plate>,
    redo: Vec<Plate>,
}

impl History {
    /// Records `plate` as a new step if it differs from the last one;
    /// call it when no change is in progress.
    pub fn settle(&mut self, plate: &Plate) {
        if !plate.same_as(&self.current) {
            let before = std::mem::replace(&mut self.current, plate.clone());
            self.undo.push(before);
            if self.undo.len() > UNDO_STEPS {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
    }

    /// Whether there is a step to undo.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether there is a step to redo.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Takes `plate` back one step.
    pub fn undo(&mut self, plate: &mut Plate) {
        if let Some(previous) = self.undo.pop() {
            let now = std::mem::replace(&mut self.current, previous);
            self.redo.push(now);
            self.restore(plate);
        }
    }

    /// Takes `plate` forward one undone step.
    pub fn redo(&mut self, plate: &mut Plate) {
        if let Some(next) = self.redo.pop() {
            let now = std::mem::replace(&mut self.current, next);
            self.undo.push(now);
            self.restore(plate);
        }
    }

    /// Puts the current state on the plate. Its serial goes up, so that
    /// views and slices count it as changed.
    fn restore(&self, plate: &mut Plate) {
        let serial = plate.serial + 1;
        *plate = self.current.clone();
        plate.serial = serial;
        plate.multi.retain(|&i| i < plate.items.len());
        if plate.selected.is_some_and(|i| i >= plate.items.len()) {
            plate.selected = plate.multi.last().copied();
        }
        if plate.selected.is_none() {
            plate.multi.clear();
        }
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
    fn mesh_transforms_turn_and_scale_around_the_mesh_centre() {
        let mesh = Mesh::cuboid(2.0, 4.0, 6.0);
        let (lo, hi) = mesh.bounds().unwrap();
        let center = [0, 1, 2].map(|axis| (lo[axis] + hi[axis]) / 2.0);
        let transform = MeshTransform {
            offset: [10.0, 20.0, 30.0],
            rotation_deg: [0.0, 0.0, 90.0],
            scale: 2.0,
        };
        let moved = transform.applied(&mesh);
        let (lo, hi) = moved.bounds().unwrap();
        // 2 × 2 mm across becomes 8 × 4 mm after scaling and a quarter turn.
        let expected_center = [center[0] + 10.0, center[1] + 20.0, center[2] + 30.0];
        for axis in 0..3 {
            assert!(((lo[axis] + hi[axis]) / 2.0 - expected_center[axis]).abs() < 1e-9);
        }
        assert!((hi[0] - lo[0] - 8.0).abs() < 1e-9, "{lo:?} {hi:?}");
        assert!((hi[1] - lo[1] - 4.0).abs() < 1e-9);
        assert!((hi[2] - lo[2] - 12.0).abs() < 1e-9);
        let back = transform.inverse_applied(&moved);
        for (original, recovered) in mesh.vertices.iter().zip(&back.vertices) {
            for axis in 0..3 {
                assert!((original[axis] - recovered[axis]).abs() < 1e-9);
            }
        }

        // The identity leaves the mesh alone.
        let same = MeshTransform::default().applied(&mesh);
        assert_eq!(same.vertices, mesh.vertices);
        assert_eq!(same.triangles, mesh.triangles);
    }

    #[test]
    fn part_and_volume_transforms_change_what_is_sliced() {
        let settings = Settings::default();
        let mut item = Item::new(cube_model(10.0, 10.0, 10.0), Placement::centered(&settings));
        item.add_part("second", Mesh::cuboid(10.0, 10.0, 10.0));
        let before = item.model.size[0];
        assert!(item.set_part_transform(
            1,
            MeshTransform {
                offset: [30.0, 0.0, 0.0],
                ..MeshTransform::default()
            }
        ));
        assert!(item.model.size[0] > before + 20.0);

        assert!(item.add_volume(
            VolumeKind::Modifier,
            "modifier",
            Mesh::cuboid(2.0, 2.0, 2.0)
        ));
        assert!(item.set_volume_transform(
            0,
            MeshTransform {
                offset: [50.0, 0.0, 1.0],
                ..MeshTransform::default()
            }
        ));
        let modifier = &item.modifiers()[0].mesh;
        let (lo, hi) = modifier.bounds().unwrap();
        assert!((lo[2] + hi[2]) / 2.0 > 1.0);
        assert!(lo[0] > 140.0, "{lo:?}");

        // The transforms count as changes that can be undone.
        let mut plate = Plate {
            items: vec![item],
            ..Plate::default()
        };
        let mut history = History::default();
        history.settle(&plate);
        plate.set_volume_transform(
            0,
            0,
            MeshTransform {
                offset: [0.0, 0.0, 0.0],
                ..MeshTransform::default()
            },
        );
        history.settle(&plate);
        assert!(history.can_undo());
        history.undo(&mut plate);
        assert!(plate.items[0].volumes[0].transform.offset[0] > 40.0);
    }

    #[test]
    fn models_keep_settings_of_their_own() {
        let settings = Settings::default();
        let mut plate = Plate::default();
        let mut history = History::default();
        plate.add(cube_model(10.0, 10.0, 10.0), &settings);
        plate.add(cube_model(10.0, 10.0, 10.0), &settings);
        history.settle(&plate);
        assert_eq!(plate.object_settings(&settings).unwrap(), [None, None]);

        let own = BTreeMap::from([("infill_density".to_owned(), "100".to_owned())]);
        let serial = plate.serial;
        plate.set_overrides(1, own.clone());
        assert!(plate.serial > serial);
        // The same values again change nothing.
        let serial = plate.serial;
        plate.set_overrides(1, own.clone());
        assert_eq!(plate.serial, serial);
        let resolved = plate.object_settings(&settings).unwrap();
        assert_eq!(resolved[0], None);
        assert_eq!(resolved[1].as_ref().unwrap().infill_density, 100.0);
        // Changing them is a step that can be undone.
        history.settle(&plate);
        assert!(history.can_undo());
        history.undo(&mut plate);
        assert!(plate.items[1].overrides.is_empty());
        history.redo(&mut plate);
        assert_eq!(plate.items[1].overrides, own);

        // Copies and cut parts keep them.
        let item = plate.items[1].clone();
        plate.add_copy(&item, &settings);
        assert_eq!(plate.items[2].overrides, own);
        assert!(plate.cut(2, 5.0, [true, true], false, &settings));
        assert_eq!(plate.items[2].overrides, own);
        assert_eq!(plate.items[3].overrides, own);

        // Values that are not valid name the model.
        let bad = BTreeMap::from([("infill_density".to_owned(), "300".to_owned())]);
        plate.set_overrides(0, bad);
        let error = plate.check(&settings).unwrap_err();
        assert!(error.message.starts_with("cube"), "{}", error.message);
        assert_eq!(error.code, "settings.out_of_range");
    }

    #[test]
    fn copies_undo_and_redo() {
        let settings = Settings::default();
        let model = Model::from_mesh("cube", Mesh::cuboid(20.0, 10.0, 5.0)).unwrap();
        let mut plate = Plate::default();
        let mut history = History::default();
        history.settle(&plate);
        assert!(!history.can_undo());

        plate.add(model, &settings);
        history.settle(&plate);
        let first = plate.items[0].clone();
        plate.add_copy(&first, &settings);
        // Beside the original, sharing its model.
        assert_eq!(plate.items[1].placement.center, [135.0, 110.0]);
        assert!(Arc::ptr_eq(&plate.items[0].model, &plate.items[1].model));
        assert_eq!(plate.selected, Some(1));
        history.settle(&plate);
        // Only the selection changes: no new step.
        plate.selected = Some(0);
        history.settle(&plate);
        plate.items[0].placement.center = [50.0, 50.0];
        history.settle(&plate);

        let serial = plate.serial;
        history.undo(&mut plate);
        assert_eq!(plate.items[0].placement.center, [110.0, 110.0]);
        assert!(plate.serial > serial);
        history.undo(&mut plate);
        assert_eq!(plate.items.len(), 1);
        history.redo(&mut plate);
        assert_eq!(plate.items.len(), 2);
        assert!(history.can_redo());
        // A new change drops what could be redone.
        plate.remove_selected();
        history.settle(&plate);
        assert!(!history.can_redo());
        history.undo(&mut plate);
        assert_eq!(plate.items.len(), 2);

        // At the right edge the copy goes to the left.
        let mut edge = plate.items[0].clone();
        edge.placement.center = [210.0, 110.0];
        plate.add_copy(&edge, &settings);
        assert_eq!(plate.items.last().unwrap().placement.center, [185.0, 110.0]);
    }

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-9)
    }

    /// A wedge: a 20 x 10 mm right triangle in XZ, 5 mm deep in Y. Its
    /// sloped face points to +X and up.
    fn wedge() -> Model {
        let v = vec![
            [0.0, 0.0, 0.0],
            [20.0, 0.0, 0.0],
            [0.0, 0.0, 10.0],
            [0.0, 5.0, 0.0],
            [20.0, 5.0, 0.0],
            [0.0, 5.0, 10.0],
        ];
        let triangles = vec![
            [0, 1, 2],
            [3, 5, 4],
            [0, 4, 1],
            [0, 3, 4],
            [0, 2, 5],
            [0, 5, 3],
            [1, 5, 2],
            [1, 4, 5],
        ];
        let mesh = Mesh {
            vertices: v,
            triangles,
            ..Mesh::default()
        };
        Model::from_mesh("wedge", mesh).unwrap()
    }

    #[test]
    fn models_mirror_turn_and_lie_down() {
        let model = wedge();
        assert!(model.mesh.signed_volume() > 0.0);
        // Mirrored in X the slope faces -X; the outside stays out.
        let mirrored = model.mirrored(0, 0.0);
        assert!((mirrored.mesh.signed_volume() - model.mesh.signed_volume()).abs() < 1e-9);
        assert_eq!(mirrored.size, model.size);
        let tall = mirrored.mesh.vertices.iter().find(|v| v[2] > 9.0).unwrap();
        assert!(tall[0] > 0.0, "{tall:?}");
        // Turned by 90°, mirroring along the bed's X flips the model's Y.
        let turned = model.mirrored(0, 90.0);
        assert!((turned.mesh.signed_volume() - model.mesh.signed_volume()).abs() < 1e-9);
        assert_eq!(turned.size, model.size);

        // Lying on the side that faced -Y makes it 5 mm tall.
        let side = model.laid_on([0.0, -1.0, 0.0]);
        assert!((side.size[2] - 5.0).abs() < 1e-9, "{:?}", side.size);
        // Upside down keeps the height.
        let flipped = model.laid_on([0.0, 0.0, 1.0]);
        assert!((flipped.size[2] - 10.0).abs() < 1e-9);
        assert!(flipped.mesh.signed_volume() > 0.0);
        // The normal of a face comes out of the model.
        let normal = face_normal(&model.mesh, 0).unwrap();
        assert!(close(normal, [0.0, -1.0, 0.0]), "{normal:?}");
        let r = rotation_onto([1.0, 0.0, 0.0], [0.0, 0.0, -1.0]);
        assert!(close(apply(r, [1.0, 0.0, 0.0]), [0.0, 0.0, -1.0]));
        assert!((det(r) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn best_orientation_rests_on_the_largest_face() {
        // A wedge rests best on its slope, the largest face, which leaves
        // nothing hanging over.
        let model = wedge();
        let slope = unit([1.0, 0.0, 2.0]).unwrap();
        assert!(close(model.best_down(), slope), "{:?}", model.best_down());
        let tipped = model.laid_on([1.0, 0.0, 1.0]);
        let best = tipped.laid_on(tipped.best_down());
        assert!(best.size[2] <= 10.0 + 1e-6, "{:?}", best.size);
        // A flat plate stood on its edge goes back to lying flat.
        let plate = Model::from_mesh("plate", Mesh::cuboid(40.0, 30.0, 2.0)).unwrap();
        let standing = plate.laid_on([1.0, 0.0, 0.0]);
        assert!((standing.size[2] - 40.0).abs() < 1e-9);
        let down = standing.laid_on(standing.best_down());
        assert!((down.size[2] - 2.0).abs() < 1e-9, "{:?}", down.size);
    }

    #[test]
    fn cutting_keeps_parts_beside_each_other() {
        let settings = Settings::default();
        let mut plate = Plate::default();
        plate.add(cube_model(20.0, 10.0, 10.0), &settings);
        plate.items[0].placement.scale = 2.0;
        let serial = plate.serial;
        // 6 mm of the 20 mm tall (scaled) cube below, 14 mm above.
        assert!(plate.cut(0, 6.0, [true, true], true, &settings));
        assert!(plate.serial > serial);
        assert_eq!(plate.items.len(), 2);
        let [lower, upper] = [&plate.items[0], &plate.items[1]];
        assert!((lower.model.size[2] * 2.0 - 6.0).abs() < 1e-3);
        assert!((upper.model.size[2] * 2.0 - 14.0).abs() < 1e-3);
        assert_eq!(lower.placement.center, [110.0, 110.0]);
        // 40 mm wide parts with a 5 mm gap.
        assert!(
            near(upper.placement.center, [155.0, 110.0]),
            "{:?}",
            upper.placement
        );
        assert_eq!(upper.model.name, "cube (top)");
        assert!(upper.model.mesh.signed_volume() > 0.0);

        // Keeping only the top, it takes the place of the model.
        let mut plate = Plate::default();
        plate.add(cube_model(20.0, 10.0, 10.0), &settings);
        assert!(plate.cut(0, 4.0, [false, true], false, &settings));
        assert_eq!(plate.items.len(), 1);
        assert!((plate.items[0].model.size[2] - 6.0).abs() < 1e-3);
        // Above the model nothing is left on top: nothing changes.
        assert!(!plate.cut(0, 50.0, [false, true], false, &settings));
        assert_eq!(plate.items.len(), 1);
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
    fn arranging_turns_models_with_arrange_rotation() {
        let settings = Settings {
            bed_size_x: 72.0,
            bed_size_y: 72.0,
            arrange_rotation: true,
            ..Settings::default()
        };
        let triangle = slicer_core::primitives::extrude(
            &[[0.0, 0.0], [60.0, 0.0], [0.0, 60.0]],
            [20.0, 20.0],
            5.0,
            1,
            0.0,
        );
        let mut plate = Plate::default();
        for _ in 0..2 {
            plate.add(
                Model::from_mesh("triangle", triangle.clone()).unwrap(),
                &settings,
            );
        }
        plate.items[0].placement.rotation_deg = 90.0;
        // In rows they do not fit the bed; turned against each other they do.
        plate.arrange(&Settings {
            arrange_rotation: false,
            ..settings.clone()
        });
        assert!(plate.check(&settings).is_err());
        plate.arrange(&settings);
        assert!(plate.check(&settings).is_ok());
        let turns: Vec<f64> = plate.placements().iter().map(|p| p.rotation_deg).collect();
        assert!(
            (turns[0] - turns[1]).rem_euclid(360.0) == 180.0,
            "{turns:?}"
        );
        let mut diagnostics = Diagnostics::new();
        slicer_fdm::generate_plate(
            &plate.meshes(),
            slicer_fdm::Placement::AsGiven,
            &settings,
            &mut diagnostics,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn arranging_nests_models_with_arrange_nonconvex() {
        let settings = Settings {
            bed_size_x: 82.0,
            bed_size_y: 82.0,
            arrange_nonconvex: true,
            ..Settings::default()
        };
        // An L turned a quarter on the plate and a cube that fits into
        // its corner.
        let ell = slicer_core::primitives::extrude(
            &[
                [0.0, 0.0],
                [80.0, 0.0],
                [80.0, 15.0],
                [15.0, 15.0],
                [15.0, 80.0],
                [0.0, 80.0],
            ],
            [7.0, 7.0],
            5.0,
            1,
            0.0,
        );
        let mut plate = Plate::default();
        plate.add(Model::from_mesh("ell", ell).unwrap(), &settings);
        plate.add(
            Model::from_mesh("cube", Mesh::cuboid(35.0, 35.0, 5.0)).unwrap(),
            &settings,
        );
        plate.items[0].placement.rotation_deg = 90.0;
        // By convex outlines the cube does not fit beside the L.
        plate.arrange(&Settings {
            arrange_nonconvex: false,
            arrange_outlines: true,
            ..settings.clone()
        });
        assert!(plate.check(&settings).is_err());
        plate.arrange(&settings);
        assert!(plate.check(&settings).is_ok());
        let mut diagnostics = Diagnostics::new();
        slicer_fdm::generate_plate(
            &plate.meshes(),
            slicer_fdm::Placement::AsGiven,
            &settings,
            &mut diagnostics,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
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
    #[test]
    fn models_are_selected_together() {
        let settings = Settings::default();
        let mut plate = Plate::default();
        for _ in 0..3 {
            plate.add(cube_model(10.0, 10.0, 10.0), &settings);
        }
        assert!(plate.is_selected(2) && !plate.is_selected(0));
        plate.toggle_selected(0);
        assert!(plate.is_selected(0) && plate.is_selected(2));
        assert_eq!(plate.selected_indices(), vec![0, 2]);
        plate.select_range(0, 2);
        assert_eq!(plate.selected_indices(), vec![0, 1, 2]);
        plate.remove_selected_all();
        assert!(plate.items.is_empty());
        assert!(plate.selected.is_none() && plate.multi.is_empty());
    }

    #[test]
    fn parts_are_merged_and_keep_their_extruder() {
        let settings = Settings::default();
        let mut plate = Plate::default();
        plate.add(cube_model(10.0, 10.0, 10.0), &settings);
        let triangles = plate.items[0].model.mesh.triangles.len();
        assert_eq!(plate.items[0].part_count(), 1);

        // A 5 mm cube on top of the first, in the object's coordinates.
        let mut second = Mesh::cuboid(5.0, 5.0, 5.0);
        second.translate([-2.5, -2.5, 10.0]);
        assert!(plate.add_part(0, "cube", second));
        let item = &plate.items[0];
        assert_eq!(item.part_count(), 2);
        assert_eq!(item.parts[1].name, "cube (2)");
        assert_eq!(item.model.mesh.triangles.len(), triangles * 2);
        assert!((item.model.size[2] - 15.0).abs() < 1e-9);
        assert_eq!(item.model.name, "cube");

        // The second part prints with the second extruder.
        assert!(plate.set_part_extruder(0, 1, Some(1)));
        let item = &plate.items[0];
        let second_triangles = item.parts[1].mesh.triangles.len();
        assert_eq!(
            item.model
                .mesh
                .extruders
                .iter()
                .filter(|&&e| e == 1)
                .count(),
            second_triangles
        );

        assert!(plate.remove_part(0, 1));
        assert_eq!(plate.items[0].part_count(), 1);
        assert_eq!(plate.items[0].model.mesh.triangles.len(), triangles);
        // The only part stays.
        assert!(!plate.remove_part(0, 0));
    }

    #[test]
    fn an_object_extruder_applies_to_the_parts_without_one() {
        let settings = Settings::default();
        let mut plate = Plate::default();
        plate.add(cube_model(10.0, 10.0, 10.0), &settings);
        let mut second = Mesh::cuboid(5.0, 5.0, 5.0);
        second.translate([-2.5, -2.5, 10.0]);
        assert!(plate.add_part(0, "cap", second));
        assert_eq!(plate.extruder_usage(), vec![2]);

        // The object's filament prints both parts.
        assert!(plate.set_object_extruder(0, Some(1)));
        let item = &plate.items[0];
        assert_eq!(item.extruder, Some(1));
        assert!(item.part_extruder(0).is_some_and(|extruder| extruder == 1));
        assert!(
            item.model
                .mesh
                .extruders
                .iter()
                .all(|&extruder| extruder == 1)
        );
        assert_eq!(plate.extruder_usage(), vec![0, 2]);

        // A part of its own keeps its extruder.
        assert!(plate.set_part_extruder(0, 1, Some(0)));
        assert_eq!(plate.items[0].part_extruder(1), Some(0));
        assert_eq!(plate.extruder_usage(), vec![1, 1]);

        // Back to the parts and the mesh.
        assert!(plate.set_object_extruder(0, None));
        assert_eq!(plate.items[0].extruder, None);
        assert_eq!(plate.extruder_usage()[0], 2);
    }

    #[test]
    fn volumes_and_ranges_reach_the_slicer() {
        let settings = Settings::default();
        let mut plate = Plate::default();
        plate.add(cube_model(20.0, 20.0, 20.0), &settings);
        let mesh = Mesh::cuboid(5.0, 5.0, 5.0);
        assert!(plate.add_volume(0, VolumeKind::Modifier, "dense", mesh.clone()));
        let values = vec![("infill_density".to_owned(), "100".to_owned())];
        assert!(
            plate
                .set_volume_values(0, 0, values.clone(), &settings)
                .unwrap()
        );
        assert!(plate.add_volume(0, VolumeKind::Negative, "hole", mesh.clone()));
        assert!(plate.add_volume(0, VolumeKind::SupportBlocker, "block", mesh.clone()));
        assert!(plate.add_volume(0, VolumeKind::SupportEnforcer, "force", mesh));
        assert!(plate.add_range(0, HeightRange::new(1.0, 2.0)));
        assert!(
            plate
                .set_range(
                    0,
                    0,
                    1.0,
                    2.0,
                    vec![("wall_loops".to_owned(), "1".to_owned())],
                    &settings,
                )
                .unwrap()
        );

        let own = plate.own_settings(&settings).unwrap();
        assert_eq!(own.modifiers[0].len(), 1);
        assert_eq!(own.modifiers[0][0].values, values);
        assert_eq!(own.negatives[0].len(), 1);
        assert_eq!(own.support_volumes[0].len(), 2);
        assert_eq!(own.ranges[0][0].to, 2.0);
        assert_eq!(own.ranges[0][0].values[0].0, "wall_loops");

        // Settings that cannot be set there are refused, as are bounds
        // that do not end above where they start.
        let error = plate
            .set_volume_values(
                0,
                0,
                vec![("layer_height".to_owned(), "0.1".to_owned())],
                &settings,
            )
            .unwrap_err();
        assert_eq!(error.code, "ui.setting_not_allowed");
        let error = plate
            .set_range(0, 0, 3.0, 2.0, Vec::new(), &settings)
            .unwrap_err();
        assert_eq!(error.code, "ui.invalid_range");

        // The slicer takes only so many volumes of a kind.
        for _ in 0..slicer_fdm::MAX_NEGATIVES {
            plate.add_volume(0, VolumeKind::Negative, "hole", Mesh::cuboid(1.0, 1.0, 1.0));
        }
        let negatives = plate.items[0]
            .volumes
            .iter()
            .filter(|volume| volume.kind == VolumeKind::Negative)
            .count();
        assert_eq!(negatives, slicer_fdm::MAX_NEGATIVES);
    }
}
