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
//! ```
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
use slicer_formats::{ProjectItem, ProjectObject, ProjectPackage};

use crate::plate::{Item, Model, Placement, Plate, bounds_2d};
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
    /// The models and where they sit; nothing is selected.
    pub plate: Plate,
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
}

/// Whether `bytes` are a project file of this program, not just a model.
pub fn is_project(bytes: &[u8]) -> bool {
    slicer_formats::looks_like_3mf(bytes)
        && matches!(slicer_formats::read_part(bytes, PROJECT_PART), Ok(Some(_)))
}

/// Writes the plate and the settings as a project file.
pub fn save_project(plate: &Plate, settings: &ProjectSettings) -> Result<Vec<u8>, Diagnostic> {
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
    for item in &plate.items {
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
        document.items.push(ItemDoc {
            center: Some(placement.center),
            rotation: Some(placement.rotation_deg),
            scale: Some(placement.scale),
        });
    }
    let text = toml::to_string(&document).map_err(|e| {
        Diagnostic::error(
            "project.write",
            format!("the project cannot be written: {e}"),
        )
    })?;
    package.parts.push((PROJECT_PART.to_owned(), text));
    slicer_formats::write_project(&package).map_err(|e| e.diagnostic())
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
    let items = package
        .items
        .iter()
        .zip(placements)
        .filter_map(|(item, placement)| {
            let model = models.get(item.object)?.clone()?;
            Some(Item { model, placement })
        })
        .collect();
    let plate = Plate {
        items,
        ..Plate::default()
    };
    Ok((Project { plate, settings }, warnings))
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
        let plate = Plate {
            items: vec![
                Item {
                    model: Arc::clone(&cut),
                    placement: at([50.0, 60.0], 0.0, 1.0),
                },
                Item {
                    model: other,
                    placement: at([120.5, 80.25], 33.3, 1.5),
                },
                Item {
                    model: cut,
                    placement: at([150.0, 150.0], -90.0, 0.1 + 0.2),
                },
            ],
            selected: Some(1),
            serial: 7,
        };
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

    #[test]
    fn a_project_opens_as_it_was_saved() {
        let (plate, settings) = sample();
        let bytes = save_project(&plate, &settings).unwrap();
        assert!(is_project(&bytes));
        let (project, warnings) = load_project(&bytes).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(project.settings, settings);
        assert_eq!(project.plate.items, plate.items);
        assert_eq!(project.plate.selected, None);
        // The copies share their model again.
        let items = &project.plate.items;
        assert!(Arc::ptr_eq(&items[0].model, &items[2].model));
        assert!(!Arc::ptr_eq(&items[0].model, &items[1].model));
    }

    #[test]
    fn other_programs_see_the_models_where_they_lie() {
        let (plate, settings) = sample();
        let bytes = save_project(&plate, &settings).unwrap();
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
        let item = &project.plate.items[0];
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
        let bytes = save_project(&plate, &settings).unwrap();
        let (project, _) = load_project(&bytes).unwrap();

        let mut editor = crate::settings_list::SettingsEditor::default();
        editor.drafts.insert("layer_height".into(), "0.".into());
        editor.set_edits(project.settings.edits.clone());
        assert_eq!(editor.edits(), &settings.edits);
        assert!(editor.drafts.is_empty());

        let mut history = crate::plate::History::default();
        history.settle(&plate);
        assert!(history.can_undo());
        history.reset(&project.plate);
        history.settle(&project.plate);
        assert!(!history.can_undo() && !history.can_redo());
    }
}
