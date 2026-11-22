//! Saving the session as a project file and opening it again.
//!
//! A project is a 3MF package (see [`slicer_formats::project`]): every model
//! on the plate is an object, as it is in the session (after cutting,
//! mirroring or laying it on a face), and every placement a build item with
//! its transform, so other programs open the models where they lie. What
//! 3MF has no place for is kept in the part [`PROJECT_PART`], a TOML text:
//!
//! ```toml
//! version = 1
//! application = "spaghetti-slicer 0.1.0"
//!
//! [settings]
//! overrides = "brim_width = 5"       # the "More settings" text
//!
//! [settings.common]                  # the values of the side panel
//! layer_height = 0.2
//! infill_density = 15.0
//! wall_loops = 2
//! supports = false
//!
//! [settings.edits]                   # values set in the list of all settings
//! nozzle_temp = "215"
//!
//! [[settings.files]]                 # the settings files in use, in order
//! path = "builtin:printers/..."      # where it came from
//! name = "Generic 220"               # name shown in the list
//! text = "..."                       # its content when the project was saved
//!
//! [[objects]]                        # one per object of the package, in order
//! extruders = [[120, 0], [40, 1]]    # runs of triangles: count, extruder
//!
//! [[items]]                          # one per build item, in order
//! center = [110.0, 110.0]
//! rotation = 0.0
//! scale = 1.0
//! extruder = 1                       # filament slot of the whole object
//! overrides = { infill_density = "40" }
//!
//! [[items.parts]]                    # the parts, in triangle order
//! name = "Cube"
//! triangles = 12
//! extruder = 1                       # only the part's own slot, if any
//! offset = [0.0, 0.0, 2.0]           # moved inside the object, in mm
//! rotation = [0.0, 0.0, 90.0]        # turned around X, Y and Z, in degrees
//! scale = 1.0
//!
//! [[items.ranges]]                   # height ranges with settings
//! from = 5.0
//! to = 10.0
//! values = { infill_density = "100" }
//!
//! [[items.volumes]]                  # meshes that are not printed
//! name = "dense"
//! kind = "modifier"                  # or "negative", "blocker", "enforcer"
//! file = "Metadata/spaghetti-volumes/0-0.stl"
//! offset = [0.0, 0.0, 2.0]
//! rotation = [0.0, 0.0, 90.0]
//! scale = 1.0
//! values = { infill_density = "100" }
//! ```
//!
//! The meshes of the volumes are binary STL files inside the package (see
//! [`slicer_formats::ProjectPackage::files`]), so that other programs see
//! only the printed objects; the project data names them. Parts are stored
//! as triangle counts: the object's mesh holds all of them, in that order.
//!
//! Opening a project uses the stored text of the settings files, so it
//! slices as it did when it was saved, even where the files are missing.
//! Readers ignore keys they do not know and fill in missing ones (an item
//! without placement takes it from its transform), so later versions can
//! add fields, for example build plates or settings per object, without a
//! new `version`; that only goes up when older readers would misread a
//! file.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use slicer_core::{Diagnose, Diagnostic, Diagnostics, Mesh};
use slicer_fdm::HeightRange;
use slicer_formats::{
    ProjectItem, ProjectObject, ProjectPackage, SupportPart, parse_3mf_plate_parts,
    read_project_settings,
};
use slicer_profiles::{Settings, import_orca, import_orca_values};

use crate::plate::{
    Item, MeshTransform, Model, Part, Placement, Plate, Volume, VolumeKind, bounds_2d,
};
use crate::settings::{CommonSettings, SettingsFile, load_settings};

/// Name of the part with the session's own data inside the package.
pub const PROJECT_PART: &str = "Metadata/spaghetti-project.toml";

/// Version of the layout of [`PROJECT_PART`] written by this program.
pub const PROJECT_VERSION: u32 = 1;

/// The settings of a session, as the window keeps them.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectSettings {
    /// The settings files in use, in order, with where they came from
    /// (a path or a `builtin:` profile).
    pub files: Vec<(PathBuf, SettingsFile)>,
    /// The values of the side panel.
    pub common: CommonSettings,
    /// The values set in the list of all settings, by key.
    pub edits: BTreeMap<String, String>,
    /// Single `key = value` lines.
    pub overrides: String,
}

/// An opened project.
#[derive(Debug, Clone)]
pub struct Project {
    /// The plates with their models; nothing is selected. At least one.
    pub plates: Vec<Plate>,
    /// The settings to use.
    pub settings: ProjectSettings,
}

/// The content of [`PROJECT_PART`].
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Document {
    version: u32,
    application: String,
    settings: SettingsDoc,
    objects: Vec<ObjectDoc>,
    items: Vec<ItemDoc>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct SettingsDoc {
    overrides: String,
    common: CommonDoc,
    edits: BTreeMap<String, String>,
    files: Vec<FileDoc>,
}

/// A missing value is taken from the settings files.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct CommonDoc {
    layer_height: Option<f64>,
    infill_density: Option<f64>,
    wall_loops: Option<u32>,
    supports: Option<bool>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct FileDoc {
    path: String,
    name: String,
    text: String,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct ObjectDoc {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    extruders: Vec<[u32; 2]>,
}

/// A missing value is taken from the item's transform.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct ItemDoc {
    center: Option<[f64; 2]>,
    rotation: Option<f64>,
    scale: Option<f64>,
    /// The plate the model is on, counted from 0.
    #[serde(skip_serializing_if = "is_zero")]
    plate: usize,
    /// The filament slot that prints the whole object, counted from 0.
    #[serde(skip_serializing_if = "Option::is_none")]
    extruder: Option<u32>,
    /// The model's settings of its own (see `Item::overrides`).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    overrides: BTreeMap<String, String>,
    /// The parts the model is made of, in the model's triangle order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    parts: Vec<PartDoc>,
    /// Ranges of height with settings of their own.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    ranges: Vec<RangeDoc>,
    /// Modifiers, negative volumes and support volumes; their meshes are
    /// files in the package.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    volumes: Vec<VolumeDoc>,
}

/// A part of an item: how many of the model's triangles belong to it, in
/// order.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct PartDoc {
    name: String,
    triangles: usize,
    extruder: Option<u32>,
    #[serde(default, skip_serializing_if = "is_zero_offset")]
    offset: [f64; 3],
    #[serde(default, skip_serializing_if = "is_zero_offset")]
    rotation: [f64; 3],
    #[serde(default = "unit_scale", skip_serializing_if = "is_unit_scale")]
    scale: f64,
}

/// A range of height that prints with settings of its own.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct RangeDoc {
    from: f64,
    to: f64,
    values: BTreeMap<String, String>,
}

/// A mesh of an item that is not printed.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct VolumeDoc {
    name: String,
    /// `modifier`, `negative`, `blocker` or `enforcer`.
    kind: String,
    /// Name of the mesh's file in the package.
    file: String,
    #[serde(default, skip_serializing_if = "is_zero_offset")]
    offset: [f64; 3],
    #[serde(default, skip_serializing_if = "is_zero_offset")]
    rotation: [f64; 3],
    #[serde(default = "unit_scale", skip_serializing_if = "is_unit_scale")]
    scale: f64,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    values: BTreeMap<String, String>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

fn is_zero_offset(offset: &[f64; 3]) -> bool {
    *offset == [0.0; 3]
}

fn unit_scale() -> f64 {
    1.0
}

fn is_unit_scale(scale: &f64) -> bool {
    (*scale - 1.0).abs() < f64::EPSILON
}

/// Whether `bytes` are a project file of this program, not just a model.
pub fn is_project(bytes: &[u8]) -> bool {
    slicer_formats::looks_like_3mf(bytes)
        && matches!(slicer_formats::read_part(bytes, PROJECT_PART), Ok(Some(_)))
}

/// Whether `bytes` are a project of Orca Slicer or Bambu Studio, which
/// stores print settings in the package.
pub fn is_orca_project(bytes: &[u8]) -> bool {
    slicer_formats::looks_like_3mf(bytes)
        && read_project_settings(bytes).is_ok_and(|settings| !settings.is_empty())
}

/// Writes the plates and the settings as a project file. Programs that
/// know no plates see the models of all plates where they lie. `current` are
/// the settings the project is sliced with; its filament colours paint the
/// picture of the first plate.
pub fn save_project(
    plates: &[&Plate],
    settings: &ProjectSettings,
    current: &Settings,
) -> Result<Vec<u8>, Diagnostic> {
    let mut package = ProjectPackage::default();
    let mut document = Document {
        version: PROJECT_VERSION,
        application: format!("spaghetti-slicer {}", env!("CARGO_PKG_VERSION")),
        settings: SettingsDoc {
            overrides: settings.overrides.clone(),
            common: CommonDoc {
                layer_height: Some(settings.common.layer_height),
                infill_density: Some(settings.common.infill_density),
                wall_loops: Some(settings.common.wall_loops),
                supports: Some(settings.common.supports),
            },
            edits: settings.edits.clone(),
            files: settings
                .files
                .iter()
                .map(|(path, file)| FileDoc {
                    path: path.to_string_lossy().into_owned(),
                    name: file.name.clone(),
                    text: file.text.clone(),
                })
                .collect(),
        },
        ..Document::default()
    };
    // Copies share their model, and in the file their object.
    let mut models: Vec<&Arc<Model>> = Vec::new();
    let items = plates
        .iter()
        .enumerate()
        .flat_map(|(index, plate)| plate.items.iter().map(move |item| (index, item)));
    for (item_number, (on, item)) in items.enumerate() {
        let object = match models.iter().position(|m| Arc::ptr_eq(m, &item.model)) {
            Some(index) => index,
            None => {
                models.push(&item.model);
                package.objects.push(ProjectObject {
                    name: item.model.name.clone(),
                    mesh: item.model.mesh.clone(),
                });
                document.objects.push(ObjectDoc {
                    extruders: extruder_runs(&item.model.mesh),
                });
                models.len() - 1
            }
        };
        let placement = item.placement;
        package.items.push(ProjectItem {
            object,
            transform: transform(&item.model, &placement),
        });
        let parts: Vec<PartDoc> = (0..item.part_count())
            .map(|index| PartDoc {
                name: item.part_name(index).unwrap_or_default().to_owned(),
                triangles: item
                    .parts
                    .get(index)
                    .map_or(item.model.mesh.triangles.len(), |part| {
                        part.mesh.triangles.len()
                    }),
                // Only an extruder of the part itself; the object's and
                // the mesh's own are stored elsewhere.
                extruder: item.parts.get(index).and_then(|part| part.extruder),
                offset: item
                    .parts
                    .get(index)
                    .map_or([0.0; 3], |part| part.transform.offset),
                rotation: item
                    .parts
                    .get(index)
                    .map_or([0.0; 3], |part| part.transform.rotation_deg),
                scale: item
                    .parts
                    .get(index)
                    .map_or(1.0, |part| part.transform.scale),
            })
            .collect();
        let ranges: Vec<RangeDoc> = item
            .ranges
            .iter()
            .map(|range| RangeDoc {
                from: range.from,
                to: range.to,
                values: range.values.iter().cloned().collect(),
            })
            .collect();
        let volumes: Vec<VolumeDoc> = item
            .volumes
            .iter()
            .enumerate()
            .map(|(index, volume)| {
                let file = format!("Metadata/spaghetti-volumes/{item_number}-{index}.stl");
                package
                    .files
                    .push((file.clone(), slicer_formats::write_stl_binary(&volume.mesh)));
                VolumeDoc {
                    name: volume.name.clone(),
                    kind: volume_kind_code(volume.kind).to_owned(),
                    file,
                    offset: volume.transform.offset,
                    rotation: volume.transform.rotation_deg,
                    scale: volume.transform.scale,
                    values: volume.values.iter().cloned().collect(),
                }
            })
            .collect();
        document.items.push(ItemDoc {
            center: Some(placement.center),
            rotation: Some(placement.rotation_deg),
            scale: Some(placement.scale),
            plate: on,
            extruder: item.extruder,
            overrides: item.overrides.clone(),
            parts,
            ranges,
            volumes,
        });
    }
    let text = toml::to_string(&document).map_err(|e| {
        Diagnostic::error(
            "project.write",
            format!("the project cannot be written: {e}"),
        )
    })?;
    package.parts.push((PROJECT_PART.to_owned(), text));
    package.thumbnail = project_thumbnail(
        &plates.first().map(|p| p.meshes()).unwrap_or_default(),
        current,
    );
    slicer_formats::write_project(&package).map_err(|e| e.diagnostic())
}

/// Size in pixels of the picture of the first plate in a project file.
const THUMBNAIL_SIZE: u32 = 256;

/// The picture of a plate for its project file as PNG; empty for a plate
/// without models. The models carry the extruders of their triangles, so
/// `settings.filament_colours` colours them as they print.
fn project_thumbnail(meshes: &[Mesh], settings: &Settings) -> Vec<u8> {
    if meshes.is_empty() {
        return Vec::new();
    }
    let size = [THUMBNAIL_SIZE, THUMBNAIL_SIZE];
    slicer_gcode::thumbnails::render_thumbnails(
        meshes,
        slicer_fdm::Placement::AsGiven,
        &[size],
        settings,
    )
    .ok()
    .and_then(|pictures| pictures.into_iter().next())
    .map_or(Vec::new(), |picture| picture.png)
}

/// Reads a project file. The warnings say what could not be taken over.
pub fn load_project(bytes: &[u8]) -> Result<(Project, Diagnostics), Diagnostic> {
    let package =
        slicer_formats::read_project(bytes, &[PROJECT_PART]).map_err(|e| e.diagnostic())?;
    let Some((_, text)) = package.parts.iter().find(|(name, _)| name == PROJECT_PART) else {
        return Err(Diagnostic::error(
            "project.not_a_project",
            "the file is a model, not a project",
        ));
    };
    let document: Document = toml::from_str(text).map_err(|e| {
        Diagnostic::error(
            "project.invalid",
            format!("the project data cannot be read: {}", e.message()),
        )
    })?;
    let mut warnings = Diagnostics::new();
    if document.version > PROJECT_VERSION {
        warnings.push(
            Diagnostic::warning(
                "project.newer_version",
                format!(
                    "the project was saved by a newer version ({}); what this version does not know is left out",
                    document.application
                ),
            )
            .with("version", document.version),
        );
    }

    let doc = document.settings;
    let files: Vec<(PathBuf, SettingsFile)> = doc
        .files
        .into_iter()
        .map(|file| {
            let name = if file.name.is_empty() {
                file.path.clone()
            } else {
                file.name
            };
            let text = file.text;
            (PathBuf::from(file.path), SettingsFile { name, text })
        })
        .collect();
    let list: Vec<SettingsFile> = files.iter().map(|(_, file)| file.clone()).collect();
    let mut common = CommonSettings::from_settings(&load_settings(&list).unwrap_or_default());
    let saved = doc.common;
    common.layer_height = saved.layer_height.unwrap_or(common.layer_height);
    common.infill_density = saved.infill_density.unwrap_or(common.infill_density);
    common.wall_loops = saved.wall_loops.unwrap_or(common.wall_loops);
    common.supports = saved.supports.unwrap_or(common.supports);
    let settings = ProjectSettings {
        files,
        common,
        edits: doc.edits,
        overrides: doc.overrides,
    };

    // Placements first: they need the objects as stored.
    let placements: Vec<Placement> = package
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let derived = placement_from(&package.objects[item.object].mesh, &item.transform);
            let saved = document.items.get(index);
            Placement {
                center: saved.and_then(|s| s.center).unwrap_or(derived.center),
                rotation_deg: saved
                    .and_then(|s| s.rotation)
                    .unwrap_or(derived.rotation_deg),
                scale: saved.and_then(|s| s.scale).unwrap_or(derived.scale),
            }
        })
        .collect();
    let models: Vec<Option<Arc<Model>>> = package
        .objects
        .into_iter()
        .enumerate()
        .map(|(index, object)| {
            let mut mesh = object.mesh;
            if let Some(doc) = document.objects.get(index) {
                set_extruder_runs(&mut mesh, &doc.extruders);
            }
            Model::from_mesh(object.name, mesh).ok().map(Arc::new)
        })
        .collect();
    let mut plates: Vec<Plate> = vec![Plate::default()];
    for (index, (item, placement)) in package.items.iter().zip(placements).enumerate() {
        let Some(model) = models.get(item.object).cloned().flatten() else {
            continue;
        };
        let doc = document.items.get(index);
        // ponytail: a plate number from a broken file could ask for very
        // many empty plates; 100 is more than anyone arranges by hand.
        let on = doc.map_or(0, |d| d.plate).min(99);
        while plates.len() <= on {
            plates.push(Plate::default());
        }
        let (mut parts, mut ranges, mut volumes) = (Vec::new(), Vec::new(), Vec::new());
        if let Some(doc) = doc {
            parts = parts_from(&model.mesh, &doc.parts);
            ranges = doc.ranges.iter().map(range_from).collect();
            volumes = volumes_from(bytes, &model.name, &doc.volumes, &mut warnings);
        }
        let mut item = Item {
            model,
            placement,
            overrides: doc.map(|d| d.overrides.clone()).unwrap_or_default(),
            extruder: None,
            parts,
            volumes,
            ranges,
        };
        // The object's filament applies to the parts that name none. The
        // saved mesh already carries the object's extruders, so rebuilding
        // from the parts is not needed (and would move transformed parts
        // twice).
        if let Some(extruder) = doc.and_then(|d| d.extruder) {
            item.extruder = Some(extruder);
        }
        plates[on].items.push(item);
    }
    Ok((Project { plates, settings }, warnings))
}

/// Opens a project of Orca Slicer or Bambu Studio: the objects of its first
/// plate, with their modifiers, negative volumes, support parts, height
/// ranges and per-object settings, and the project's print settings.
///
/// The objects are taken as they lie on the plate; their original rotation
/// and scale are not kept separately. Unknown Orca settings are left out
/// and counted in a warning. What this version cannot take over is
/// reported in the returned warnings.
pub fn load_orca_project(bytes: &[u8]) -> Result<(Project, Diagnostics), Diagnostic> {
    let stored = read_project_settings(bytes).map_err(|e| e.diagnostic())?;
    let mut warnings = Diagnostics::new();
    let (settings, edits, skipped) = match &stored.config {
        Some(text) => {
            let mut imported = Settings::default();
            let report = import_orca(text, &mut imported).map_err(|e| {
                Diagnostic::error(
                    "project.settings_invalid",
                    format!("the project's print settings cannot be read: {e}"),
                )
            })?;
            let edits = report
                .taken
                .iter()
                .map(|(key, value, _)| (key.clone(), value.clone()))
                .collect();
            (imported, edits, report.skipped.len())
        }
        None => (Settings::default(), BTreeMap::new(), 0),
    };
    if skipped > 0 {
        warnings.push(
            Diagnostic::warning(
                "project.settings_skipped",
                format!(
                    "{skipped} print setting(s) of the project have no counterpart and were left out"
                ),
            )
            .with("skipped", skipped),
        );
    }
    let settings = ProjectSettings {
        files: Vec::new(),
        common: CommonSettings::from_settings(&settings),
        edits,
        overrides: String::new(),
    };
    let shared = Settings::default();
    let objects =
        parse_3mf_plate_parts(bytes, None, &mut warnings).map_err(|e| e.diagnostic())?;
    let mut plate = Plate::default();
    let mut skipped_object_settings = 0;
    for object in objects {
        let Some((lo, hi)) = object.mesh.bounds() else {
            continue;
        };
        let center = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
        let stored_object = stored.object(object.id);
        let name = stored_object
            .and_then(|settings| settings.name.clone())
            .unwrap_or_else(|| format!("Object {}", object.id));
        let model = Model::from_mesh(name, object.mesh).map_err(|e| {
            Diagnostic::error(
                "project.model_invalid",
                format!("Object {}: {}", object.id, e.message),
            )
        })?;
        let mut item = Item::new(
            model,
            Placement {
                center,
                rotation_deg: 0.0,
                scale: 1.0,
            },
        );
        // Meshes of parts are stored on the plate; their object's own
        // coordinates put their centre at the model's centre above the bed.
        let shift = [-center[0], -center[1], -lo[2]];
        add_orca_volumes(&mut item, object.modifiers, object.negatives, object.support_parts, shift, stored_object);
        if let Some(stored_object) = stored_object {
            item.overrides = imported_values(
                &stored_object.values,
                &shared,
                slicer_fdm::OBJECT_SETTINGS,
            );
            skipped_object_settings += stored_object
                .values
                .len()
                .saturating_sub(item.overrides.len());
            item.ranges = stored_object
                .ranges
                .iter()
                .map(|range| HeightRange {
                    from: range.min_z,
                    to: range.max_z,
                    values: imported_values(
                        &range.values,
                        &shared,
                        slicer_fdm::RANGE_SETTINGS,
                    )
                    .into_iter()
                    .collect(),
                })
                .collect();
        }
        plate.items.push(item);
    }
    if skipped_object_settings > 0 {
        warnings.push(
            Diagnostic::warning(
                "project.object_settings_skipped",
                format!(
                    "{skipped_object_settings} setting(s) of objects have no counterpart and were left out"
                ),
            )
            .with("skipped", skipped_object_settings),
        );
    }
    plate.clear_selection();
    plate.serial = 1;
    Ok((Project { plates: vec![plate], settings }, warnings))
}

/// Adds the modifiers, negative volumes and support parts of an Orca
/// object to `item`; their meshes are moved from the plate into the
/// object's coordinates by `shift`.
fn add_orca_volumes(
    item: &mut Item,
    modifiers: Vec<(u32, Mesh)>,
    negatives: Vec<Mesh>,
    support_parts: Vec<(SupportPart, Mesh)>,
    shift: [f64; 3],
    stored: Option<&slicer_formats::ObjectSettings>,
) {
    let shared = Settings::default();
    for (id, mesh) in modifiers {
        let mut volume = Volume::new(
            format!("Modifier {id}"),
            VolumeKind::Modifier,
            translated(mesh, shift),
        );
        if let Some(values) = stored
            .and_then(|settings| settings.modifiers.iter().find(|m| m.id == id))
        {
            volume.values =
                imported_values(&values.values, &shared, VolumeKind::Modifier.settings())
                    .into_iter()
                    .collect();
        }
        item.volumes.push(volume);
    }
    for (index, mesh) in negatives.into_iter().enumerate() {
        item.volumes.push(Volume::new(
            format!("Negative volume {}", index + 1),
            VolumeKind::Negative,
            translated(mesh, shift),
        ));
    }
    for (index, (kind, mesh)) in support_parts.into_iter().enumerate() {
        let kind = match kind {
            SupportPart::Blocker => VolumeKind::SupportBlocker,
            SupportPart::Enforcer => VolumeKind::SupportEnforcer,
        };
        item.volumes.push(Volume::new(
            format!("{} {}", kind.label(), index + 1),
            kind,
            translated(mesh, shift),
        ));
    }
}

/// The Orca values among `values` that map to one of `allowed`, in this
/// version's names; values that do not apply are left out.
fn imported_values(
    values: &[(String, String)],
    base: &Settings,
    allowed: &[&str],
) -> BTreeMap<String, String> {
    let mut imported = base.clone();
    let report = import_orca_values(
        values
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str())),
        &mut imported,
    );
    report
        .taken
        .into_iter()
        .filter(|(key, ..)| allowed.contains(&key.as_str()))
        .map(|(key, value, _)| (key, value))
        .collect()
}

/// `mesh` moved by `offset`.
fn translated(mut mesh: Mesh, offset: [f64; 3]) -> Mesh {
    mesh.translate(offset);
    mesh
}

/// How a volume's kind is written in the project data.
fn volume_kind_code(kind: VolumeKind) -> &'static str {
    match kind {
        VolumeKind::Modifier => "modifier",
        VolumeKind::Negative => "negative",
        VolumeKind::SupportBlocker => "blocker",
        VolumeKind::SupportEnforcer => "enforcer",
    }
}

/// The kind written as `code`; `None` for one this version does not know.
fn volume_kind_from(code: &str) -> Option<VolumeKind> {
    [
        VolumeKind::Modifier,
        VolumeKind::Negative,
        VolumeKind::SupportBlocker,
        VolumeKind::SupportEnforcer,
    ]
    .into_iter()
    .find(|kind| volume_kind_code(*kind) == code)
}

/// The height range `doc` describes.
fn range_from(doc: &RangeDoc) -> HeightRange {
    HeightRange {
        from: doc.from,
        to: doc.to,
        values: doc
            .values
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    }
}

/// The parts of `mesh` as `docs` split it, by triangle counts; empty if the
/// counts do not cover the mesh or there is only one part.
fn parts_from(mesh: &Mesh, docs: &[PartDoc]) -> Vec<Part> {
    let total: usize = docs.iter().map(|doc| doc.triangles).sum();
    if docs.len() < 2 || total != mesh.triangles.len() {
        return Vec::new();
    }
    let mut parts = Vec::new();
    let mut first = 0;
    for doc in docs {
        let end = first + doc.triangles;
        let name = if doc.name.is_empty() {
            format!("Part {}", parts.len() + 1)
        } else {
            doc.name.clone()
        };
        let transform = MeshTransform {
            offset: doc.offset,
            rotation_deg: doc.rotation,
            scale: doc.scale,
        };
        // The package holds the object as it prints, after the parts'
        // transforms; the parts are stored as they were before.
        let part = transform.inverse_applied(&sub_mesh(mesh, first..end));
        parts.push(Part {
            name,
            mesh: Arc::new(part),
            transform,
            extruder: doc.extruder,
        });
        first = end;
    }
    parts
}

/// The triangles from `first` to `last` of `mesh` as a mesh of their own,
/// with their extruders and without unused vertices.
fn sub_mesh(mesh: &Mesh, range: std::ops::Range<usize>) -> Mesh {
    let mut remap: BTreeMap<u32, u32> = BTreeMap::new();
    let mut part = Mesh::default();
    for index in range {
        let Some(triangle) = mesh.triangles.get(index) else {
            continue;
        };
        let mut ids = [0u32; 3];
        for (slot, &old) in ids.iter_mut().zip(triangle) {
            *slot = *remap.entry(old).or_insert_with(|| {
                part.vertices
                    .push(mesh.vertices.get(old as usize).copied().unwrap_or_default());
                (part.vertices.len() - 1) as u32
            });
        }
        part.triangles.push(ids);
        if !mesh.extruders.is_empty() {
            part.extruders.push(mesh.extruder(index));
        }
    }
    part
}

/// The volumes `docs` describe, their meshes read from the package of
/// `bytes`; what cannot be read is reported in `warnings` and left out.
fn volumes_from(
    bytes: &[u8],
    model: &str,
    docs: &[VolumeDoc],
    warnings: &mut Diagnostics,
) -> Vec<Volume> {
    let mut volumes = Vec::new();
    for doc in docs {
        let Some(kind) = volume_kind_from(&doc.kind) else {
            warnings.push(Diagnostic::warning(
                "project.volume_skipped",
                format!(
                    "{model}: '{0}' is not a kind of volume this version knows; left out",
                    doc.kind
                ),
            ));
            continue;
        };
        let mesh = slicer_formats::read_file(bytes, &doc.file)
            .ok()
            .flatten()
            .and_then(|data| slicer_formats::parse_model(&data, None, warnings).ok());
        let Some(mesh) = mesh else {
            warnings.push(Diagnostic::warning(
                "project.volume_skipped",
                format!(
                    "{model}: the mesh of '{}' cannot be read; left out",
                    doc.name
                ),
            ));
            continue;
        };
        let name = if doc.name.is_empty() {
            kind.label().to_owned()
        } else {
            doc.name.clone()
        };
        let mut volume = Volume::new(name, kind, mesh);
        volume.transform = MeshTransform {
            offset: doc.offset,
            rotation_deg: doc.rotation,
            scale: doc.scale,
        };
        volume.values = doc
            .values
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        volumes.push(volume);
    }
    volumes
}

/// The file name to suggest for a project of a model called `model_name`.
pub fn project_file_name(model_name: &str) -> String {
    let lower = model_name.to_lowercase();
    let stem = [".gcode.3mf", ".3mf", ".stl"]
        .iter()
        .find(|ext| lower.ends_with(*ext))
        .map_or(model_name, |ext| {
            &model_name[..model_name.len() - ext.len()]
        });
    let stem = if stem.is_empty() { "project" } else { stem };
    format!("{stem}.3mf")
}

/// `name` with `.3mf` added unless it is there already.
pub fn with_project_extension(name: &str) -> String {
    if name.to_lowercase().ends_with(".3mf") {
        name.to_owned()
    } else {
        format!("{name}.3mf")
    }
}

/// The 3MF transform that puts the model where [`crate::plate::placed_mesh`]
/// puts it: turned and scaled around the origin, then moved so that the
/// centre of its box lies at the placement's centre.
fn transform(model: &Model, placement: &Placement) -> [f64; 12] {
    let (sin, cos) = placement.rotation_deg.to_radians().sin_cos();
    let s = placement.scale;
    let (a, b) = (s * cos, s * sin);
    let turned = used_vertices(&model.mesh).map(|v| [a * v[0] - b * v[1], b * v[0] + a * v[1]]);
    let middle = bounds_2d(turned).map_or([0.0, 0.0], |(lo, hi)| {
        [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0]
    });
    [
        a,
        b,
        0.0,
        -b,
        a,
        0.0,
        0.0,
        0.0,
        s,
        placement.center[0] - middle[0],
        placement.center[1] - middle[1],
        0.0,
    ]
}

/// The placement that comes closest to `m` for an object `mesh`: its turn
/// around Z and its scale in X, and the centre of the placed box. Tilts
/// and uneven scales are not kept.
fn placement_from(mesh: &Mesh, m: &[f64; 12]) -> Placement {
    let placed = used_vertices(mesh).map(|v| {
        [
            v[0] * m[0] + v[1] * m[3] + v[2] * m[6] + m[9],
            v[0] * m[1] + v[1] * m[4] + v[2] * m[7] + m[10],
        ]
    });
    let center = bounds_2d(placed).map_or([0.0, 0.0], |(lo, hi)| {
        [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0]
    });
    Placement {
        center,
        rotation_deg: m[1].atan2(m[0]).to_degrees(),
        scale: m[0].hypot(m[1]),
    }
}

/// The vertices that belong to a triangle.
fn used_vertices(mesh: &Mesh) -> impl Iterator<Item = [f64; 3]> + '_ {
    mesh.triangles
        .iter()
        .flatten()
        .filter_map(|&i| mesh.vertices.get(i as usize).copied())
}

/// The extruders of the triangles as runs of `[count, extruder]`; empty if
/// the mesh lists none.
fn extruder_runs(mesh: &Mesh) -> Vec<[u32; 2]> {
    let mut runs: Vec<[u32; 2]> = Vec::new();
    for &extruder in &mesh.extruders {
        match runs.last_mut() {
            Some([count, last]) if *last == extruder => *count += 1,
            _ => runs.push([1, extruder]),
        }
    }
    runs
}

/// Gives the triangles the extruders of `runs`, if the runs cover exactly
/// the triangles of the mesh.
fn set_extruder_runs(mesh: &mut Mesh, runs: &[[u32; 2]]) {
    let total: u64 = runs.iter().map(|&[count, _]| u64::from(count)).sum();
    if runs.is_empty() || total != mesh.triangles.len() as u64 {
        return;
    }
    mesh.extruders = runs
        .iter()
        .flat_map(|&[count, extruder]| std::iter::repeat_n(extruder, count as usize))
        .collect();
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::plate::placed_mesh;
    use slicer_profiles::Settings;

    /// A box with corners at whole millimetres, already standing as the
    /// plate keeps it.
    fn block(name: &str, size: [f64; 3]) -> Model {
        let [x, y, z] = size;
        let mesh = Mesh {
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
        };
        Model::from_mesh(name, mesh).unwrap()
    }

    fn sample() -> (Plate, ProjectSettings) {
        let mut cut = block("part (top).stl", [10.0, 20.0, 6.0]);
        cut.mesh.extruders = vec![0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 0, 2];
        let cut = Arc::new(cut);
        let other = Arc::new(block("Ölkanne \"klein\".stl", [4.0, 4.0, 30.0]));
        let at = |center: [f64; 2], rotation_deg: f64, scale: f64| Placement {
            center,
            rotation_deg,
            scale,
        };
        let halves = |name: &str| Part {
            name: name.into(),
            mesh: Arc::new(sub_mesh(&other.mesh, 0..6)),
            transform: MeshTransform::default(),
            extruder: None,
        };
        let mut plate = Plate {
            items: vec![
                Item {
                    model: Arc::clone(&cut),
                    placement: at([50.0, 60.0], 0.0, 1.0),
                    overrides: BTreeMap::new(),
                    extruder: None,
                    parts: Vec::new(),
                    volumes: Vec::new(),
                    ranges: Vec::new(),
                },
                Item {
                    model: Arc::clone(&other),
                    placement: at([120.5, 80.25], 33.3, 1.5),
                    overrides: BTreeMap::from([("infill_density".to_owned(), "40".to_owned())]),
                    extruder: Some(1),
                    parts: vec![
                        halves("lower"),
                        Part {
                            name: "upper".into(),
                            mesh: Arc::new(sub_mesh(&other.mesh, 6..12)),
                            transform: MeshTransform::default(),
                            extruder: None,
                        },
                    ],
                    volumes: vec![
                        Volume {
                            name: "dense".into(),
                            kind: VolumeKind::Modifier,
                            mesh: Mesh::cuboid(2.0, 2.0, 2.0),
                            transform: MeshTransform {
                                offset: [0.0, 0.0, 1.0],
                                rotation_deg: [0.0, 0.0, 30.0],
                                scale: 1.25,
                            },
                            values: vec![("infill_density".to_owned(), "100".to_owned())],
                        },
                        Volume {
                            name: "hole".into(),
                            kind: VolumeKind::Negative,
                            mesh: Mesh::cuboid(1.0, 1.0, 1.0),
                            transform: MeshTransform::default(),
                            values: Vec::new(),
                        },
                        Volume {
                            name: "block".into(),
                            kind: VolumeKind::SupportBlocker,
                            mesh: Mesh::cuboid(1.0, 1.0, 1.0),
                            transform: MeshTransform::default(),
                            values: Vec::new(),
                        },
                        Volume {
                            name: "force".into(),
                            kind: VolumeKind::SupportEnforcer,
                            mesh: Mesh::cuboid(1.0, 1.0, 1.0),
                            transform: MeshTransform::default(),
                            values: Vec::new(),
                        },
                    ],
                    ranges: vec![HeightRange {
                        from: 1.0,
                        to: 5.0,
                        values: vec![("infill_density".to_owned(), "100".to_owned())],
                    }],
                },
                Item {
                    model: cut,
                    placement: at([150.0, 150.0], -90.0, 0.1 + 0.2),
                    overrides: BTreeMap::new(),
                    extruder: None,
                    parts: Vec::new(),
                    volumes: Vec::new(),
                    ranges: Vec::new(),
                },
            ],
            selected: Some(1),
            multi: vec![1],
            serial: 7,
        };
        // The object's filament, as the window sets it.
        plate.items[1].set_object_extruder(Some(1));
        let settings = ProjectSettings {
            files: vec![
                (
                    PathBuf::from("builtin:printers/generic-220.toml"),
                    SettingsFile {
                        name: "Generic 220".into(),
                        text: "bed_size_x = 220.0\nstart_gcode = \"\"\"\nG28 ; home \\\\ all\n\"\"\"\n"
                            .into(),
                    },
                ),
                (
                    PathBuf::from("C:\\Users\\me\\pla.toml"),
                    SettingsFile {
                        name: "pla.toml".into(),
                        text: "nozzle_temp = 210\r\n".into(),
                    },
                ),
            ],
            common: CommonSettings {
                layer_height: 0.12,
                infill_density: 35.0,
                wall_loops: 4,
                supports: true,
            },
            edits: BTreeMap::from([
                ("brim_width".to_owned(), "3".to_owned()),
                ("seam_position".to_owned(), "rear".to_owned()),
            ]),
            overrides: "skirt_loops = 2\n# note\n".into(),
        };
        (plate, settings)
    }

    /// Compares saved and loaded items; STL files store single precision
    /// and no vertex order, so volume meshes are compared as triangles.
    fn assert_items(loaded: &[Item], saved: &[Item]) {
        assert_eq!(loaded.len(), saved.len());
        for (a, b) in loaded.iter().zip(saved) {
            assert_eq!(a.model, b.model);
            assert_eq!(a.placement, b.placement);
            assert_eq!(a.overrides, b.overrides);
            assert_eq!(a.extruder, b.extruder);
            assert_eq!(a.ranges, b.ranges);
            assert_eq!(a.part_count(), b.part_count());
            for index in 0..a.part_count() {
                assert_eq!(a.part_name(index), b.part_name(index));
                assert_eq!(a.part_extruder(index), b.part_extruder(index));
            }
            assert_eq!(a.parts.len(), b.parts.len());
            for (x, y) in a.parts.iter().zip(&b.parts) {
                assert_eq!(x.transform, y.transform);
                assert_eq!(x.mesh.to_triangle_soup(), y.mesh.to_triangle_soup());
            }
            assert_eq!(a.volumes.len(), b.volumes.len());
            for (x, y) in a.volumes.iter().zip(&b.volumes) {
                assert_eq!((&x.name, x.kind, &x.values), (&y.name, y.kind, &y.values));
                assert_eq!(x.transform, y.transform);
                assert_eq!(x.mesh.to_triangle_soup(), y.mesh.to_triangle_soup());
            }
        }
    }

    #[test]
    fn a_project_opens_as_it_was_saved() {
        let (plate, settings) = sample();
        let bytes = save_project(&[&plate], &settings, &Settings::default()).unwrap();
        assert!(is_project(&bytes));
        let (project, warnings) = load_project(&bytes).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(project.settings, settings);
        assert_items(&project.plates[0].items, &plate.items);
        assert_eq!(project.plates[0].items[1].extruder, Some(1));
        assert_eq!(project.plates[0].selected, None);
        // The copies share their model again.
        let items = &project.plates[0].items;
        assert!(Arc::ptr_eq(&items[0].model, &items[2].model));
        assert!(!Arc::ptr_eq(&items[0].model, &items[1].model));
    }

    #[test]
    fn an_orca_project_opens_with_its_print_settings() {
        let mut package = ProjectPackage::default();
        package.objects.push(ProjectObject {
            name: "cube".into(),
            mesh: block("cube", [10.0, 10.0, 10.0]).mesh,
        });
        package.items.push(ProjectItem {
            object: 0,
            transform: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 50.0, 60.0, 0.0],
        });
        package.parts.push((
            "Metadata/project_settings.config".into(),
            r##"{
                "name": "My project",
                "layer_height": "0.16",
                "sparse_infill_density": "25%",
                "enable_support": "1",
                "filament_colour": ["#FF0000"],
                "filament_type": ["PLA"]
            }"##
            .into(),
        ));
        package.parts.push((
            "Metadata/model_settings.config".into(),
            r#"<config>
                <object id="1">
                    <metadata key="name" value="Orca cube"/>
                    <metadata key="sparse_infill_density" value="40"/>
                </object>
            </config>"#
            .into(),
        ));
        let bytes = slicer_formats::write_project(&package).unwrap();
        assert!(is_orca_project(&bytes));
        assert!(!is_project(&bytes));

        let (project, warnings) = load_orca_project(&bytes).unwrap();
        assert!(
            warnings
                .as_slice()
                .iter()
                .all(|warning| warning.severity != slicer_core::Severity::Error)
        );
        let item = &project.plates[0].items[0];
        assert_eq!(item.model.name, "Orca cube");
        assert!(
            (item.placement.center[0] - 50.0).abs() < 1e-9,
            "{:?}",
            item.placement
        );
        assert!(
            (item.placement.center[1] - 60.0).abs() < 1e-9,
            "{:?}",
            item.placement
        );
        assert_eq!(
            item.overrides.get("infill_density").map(String::as_str),
            Some("40")
        );
        let edits = &project.settings.edits;
        assert_eq!(edits.get("layer_height").map(String::as_str), Some("0.16"));
        assert_eq!(
            edits.get("infill_density").map(String::as_str),
            Some("25")
        );
        assert_eq!(
            edits.get("filament_colours").map(String::as_str),
            Some("#FF0000")
        );
        assert_eq!(edits.get("supports").map(String::as_str), Some("true"));
        assert_eq!(project.settings.files, Vec::new());
        assert_eq!(project.settings.common.layer_height, 0.16);
        assert_eq!(project.settings.common.infill_density, 25.0);
    }

    #[test]
    fn the_picture_uses_the_filament_colours_of_the_project() {
        let (plate, settings) = sample();
        let picture = |colour: &str| {
            let slice = Settings {
                filament_colours: colour.to_owned(),
                ..Settings::default()
            };
            let bytes = save_project(&[&plate], &settings, &slice).unwrap();
            slicer_formats::read_file(&bytes, "Metadata/thumbnail.png")
                .unwrap()
                .unwrap()
        };
        let red = picture("#FF0000");
        let blue = picture("#0000FF");
        assert!(red.starts_with(b"\x89PNG"), "not a PNG");
        assert_ne!(red, blue, "the colours did not reach the picture");
    }

    #[test]
    fn other_programs_see_the_models_where_they_lie() {
        let (plate, settings) = sample();
        let bytes = save_project(&[&plate], &settings, &Settings::default()).unwrap();
        let mut warnings = Diagnostics::new();
        let read = slicer_formats::parse_3mf(&bytes, &mut warnings).unwrap();
        let mut expected = Mesh::default();
        for mesh in plate.meshes() {
            let offset = expected.vertices.len() as u32;
            expected.vertices.extend(mesh.vertices);
            expected
                .triangles
                .extend(mesh.triangles.iter().map(|t| t.map(|i| i + offset)));
        }
        assert_eq!(read.triangles, expected.triangles);
        for (a, b) in read.vertices.iter().zip(&expected.vertices) {
            for axis in 0..3 {
                assert!((a[axis] - b[axis]).abs() < 1e-9, "{a:?} {b:?}");
            }
        }
    }

    #[test]
    fn models_are_not_projects() {
        let model = block("a", [1.0, 1.0, 1.0]);
        let plain = slicer_formats::write_3mf(&model.mesh).unwrap();
        assert!(!is_project(&plain));
        assert!(!is_project(b"solid cube\nendsolid cube\n"));
        assert_eq!(
            load_project(&plain).unwrap_err().code,
            "project.not_a_project"
        );
    }

    #[test]
    fn unknown_keys_are_ignored_and_missing_ones_filled_in() {
        let model = block("a", [10.0, 10.0, 10.0]);
        let quarter = [
            0.0, 2.0, 0.0, -2.0, 0.0, 0.0, 0.0, 0.0, 2.0, 30.0, 40.0, 0.0,
        ];
        let text = "version = 9\napplication = \"spaghetti-slicer 9.0\"\nfuture = true\n\n[settings]\noverrides = \"brim_width = 2\"\nplates = 3\n\n[settings.common]\nwall_loops = 5\n\n[[settings.files]]\npath = \"x.toml\"\ntext = \"layer_height = 0.28\"\n\n[[objects]]\nextruders = [[5, 1]]\nmaterial = \"PLA\"\n";
        let package = ProjectPackage {
            objects: vec![ProjectObject {
                name: "a".into(),
                mesh: model.mesh.clone(),
            }],
            items: vec![ProjectItem {
                object: 0,
                transform: quarter,
            }],
            parts: vec![(PROJECT_PART.into(), text.into())],
            files: Vec::new(),
            thumbnail: Vec::new(),
        };
        let bytes = slicer_formats::write_project(&package).unwrap();
        let (project, warnings) = load_project(&bytes).unwrap();
        assert_eq!(warnings.as_slice()[0].code, "project.newer_version");
        let settings = &project.settings;
        assert_eq!(settings.overrides, "brim_width = 2");
        assert_eq!(settings.files[0].1.name, "x.toml");
        // From the files, except what the project names.
        assert_eq!(settings.common.layer_height, 0.28);
        assert_eq!(settings.common.wall_loops, 5);
        assert_eq!(settings.common.supports, Settings::default().supports);
        // Five runs do not cover twelve triangles.
        let item = &project.plates[0].items[0];
        assert!(item.model.mesh.extruders.is_empty());
        // The placement comes from the transform: a quarter turn, twice
        // the size, the box centred on (30, 40).
        assert!((item.placement.rotation_deg - 90.0).abs() < 1e-9);
        assert!((item.placement.scale - 2.0).abs() < 1e-12);
        let center = item.placement.center;
        assert!((center[0] - 30.0).abs() < 1e-9 && (center[1] - 40.0).abs() < 1e-9);
        let placed = placed_mesh(&item.model, &item.placement);
        let (lo, hi) = placed.bounds().unwrap();
        assert!((lo[0] - 20.0).abs() < 1e-9 && (hi[1] - 50.0).abs() < 1e-9);
    }

    #[test]
    fn broken_project_data_is_an_error() {
        let package = ProjectPackage {
            parts: vec![(PROJECT_PART.into(), "version = [".into())],
            ..ProjectPackage::default()
        };
        let bytes = slicer_formats::write_project(&package).unwrap();
        assert_eq!(load_project(&bytes).unwrap_err().code, "project.invalid");
    }

    #[test]
    fn file_names() {
        assert_eq!(project_file_name("Cube.STL"), "Cube.3mf");
        assert_eq!(project_file_name("plate.gcode.3mf"), "plate.3mf");
        assert_eq!(project_file_name(".stl"), "project.3mf");
        assert_eq!(with_project_extension("a"), "a.3mf");
        assert_eq!(with_project_extension("a.3MF"), "a.3MF");
    }

    #[test]
    fn an_opened_project_restores_the_edits_and_starts_a_new_history() {
        let (plate, settings) = sample();
        let bytes = save_project(&[&plate], &settings, &Settings::default()).unwrap();
        let (project, _) = load_project(&bytes).unwrap();

        let mut editor = crate::settings_list::SettingsEditor::default();
        editor.drafts.insert("layer_height".into(), "0.".into());
        editor.replace_edits(project.settings.edits.clone());
        assert_eq!(editor.edits(), &settings.edits);
        assert!(editor.drafts.is_empty());

        let (plates, first) = crate::plates::Plates::<()>::from_plates(project.plates);
        assert_eq!(first.items.len(), 3);
        assert_eq!(plates.count(), 1);
        assert!(!plates.can_undo() && !plates.can_redo());
    }

    #[test]
    fn plates_and_object_settings_are_kept() {
        let (plate, settings) = sample();
        let mut second = Plate::default();
        second.items.push(plate.items[1].clone());
        let empty = Plate::default();
        let bytes =
            save_project(&[&plate, &empty, &second], &settings, &Settings::default()).unwrap();
        let (project, _) = load_project(&bytes).unwrap();
        // The empty plate in the middle stays.
        assert_eq!(project.plates.len(), 3);
        assert_items(&project.plates[0].items, &plate.items);
        assert!(project.plates[1].items.is_empty());
        assert_items(&project.plates[2].items, &second.items);
        assert_eq!(project.plates[2].items[0].overrides["infill_density"], "40");
        let (plates, first) = crate::plates::Plates::<()>::from_plates(project.plates);
        assert_eq!(plates.count(), 3);
        assert_eq!(plates.all(&first)[2].items.len(), 1);
    }
}
