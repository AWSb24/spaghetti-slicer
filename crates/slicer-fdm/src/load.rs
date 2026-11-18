//! Reading the models of a plate, as the command line tool and the server
//! do: model files (STL, 3MF), with the print settings that project files
//! of Orca Slicer and Bambu Studio store if asked for, split into their
//! objects where objects have settings of their own, and put on the bed.
//!
//! Settings are layered in this order, later ones winning: the defaults,
//! the settings of the projects ([`project_base`]), the caller's settings
//! for the plate. A single object starts from the plate's settings, takes
//! the values the project stores for it ([`model_objects`]) and then the
//! caller's settings for the objects of its model file ([`ObjectLayer`]).
//! Height ranges of an object ([`HeightRange`]) come from the project and
//! from the caller in the same order; they set their keys over whatever
//! the object ends up with.
//! [`read_plate`] does all of it; the steps are public for callers that
//! read their files in between.
//!
//! Nothing here touches the file system: models come as bytes.

use slicer_core::{Diagnose, Diagnostic, Diagnostics, Mesh};
use slicer_formats::{ObjectSettings, ProjectSettings};
use slicer_profiles::Settings;

use crate::{
    ARRANGE_GAP, HeightRange, Placement, apply_object_orca, apply_object_toml, apply_range_orca,
    apply_range_toml,
};

/// Largest number of setting names a warning lists in its message; the
/// `keys` field of the warning holds them all.
const LISTED_KEYS: usize = 12;

/// Largest number of height ranges of one object.
pub const MAX_RANGES: usize = 256;

/// The heights `Z1-Z2` of a height range in millimetres, as the command
/// line and the server take them, e.g. `10-20` or `2.5-7.25`; `None` unless
/// both are numbers with 0 <= Z1 < Z2.
pub fn parse_height_range(text: &str) -> Option<(f64, f64)> {
    let (from, to) = text.trim().split_once('-')?;
    let from: f64 = from.trim().parse().ok()?;
    let to: f64 = to.trim().parse().ok()?;
    (from.is_finite() && to.is_finite() && 0.0 <= from && from < to).then_some((from, to))
}

/// How messages name a model file or a settings text, e.g. by its path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Name {
    /// Put in front of messages, e.g. the path of a file or `model 2`.
    pub text: String,
    /// Key of the context entry naming it, e.g. `path` or `model`.
    pub key: &'static str,
    /// Value of that context entry.
    pub value: String,
}

impl Name {
    /// A name shown as `text` with the context entry `key` = `value`.
    pub fn new(text: impl Into<String>, key: &'static str, value: impl ToString) -> Self {
        Self {
            text: text.into(),
            key,
            value: value.to_string(),
        }
    }

    /// `diagnostic` with this name in front of its message and in its
    /// context.
    pub fn apply(&self, diagnostic: Diagnostic) -> Diagnostic {
        let mut diagnostic = diagnostic.with(self.key, &self.value);
        diagnostic.message = format!("{}: {}", self.text, diagnostic.message);
        diagnostic
    }
}

/// `diagnostic` named by `name`, unchanged without one.
fn named(name: Option<&Name>, diagnostic: Diagnostic) -> Diagnostic {
    match name {
        Some(name) => name.apply(diagnostic),
        None => diagnostic,
    }
}

/// One model file to slice.
#[derive(Debug, Clone, Copy)]
pub struct Model<'a> {
    /// The content of the file (STL or 3MF).
    pub bytes: &'a [u8],
    /// How messages about the file name it; without a name they do not.
    pub name: Option<&'a Name>,
}

/// Settings in TOML form for every object of one model file, layered over
/// the settings of each of them. They may only set
/// [`crate::OBJECT_SETTINGS`], or, for a height range,
/// [`crate::RANGE_SETTINGS`].
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectLayer {
    /// The settings in TOML form.
    pub text: String,
    /// How errors about the text name it.
    pub name: Name,
    /// The heights in millimetres above the bed from which and up to which
    /// the settings count (see [`HeightRange`]); `None` for the whole
    /// object. Layers for the same heights go into one range.
    pub range: Option<(f64, f64)>,
}

/// How the objects are put on the bed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placing {
    /// Keep every object where its file puts it, in bed coordinates, instead
    /// of centring or arranging.
    pub keep_positions: bool,
    /// Distance between arranged objects in millimetres, plus the brim.
    pub gap: f64,
}

impl Default for Placing {
    fn default() -> Self {
        Self {
            keep_positions: false,
            gap: ARRANGE_GAP,
        }
    }
}

/// The objects read from model files.
#[derive(Debug, Clone, Default)]
pub struct Objects {
    /// The mesh of every object.
    pub meshes: Vec<Mesh>,
    /// The settings of every object that has its own, for
    /// [`crate::generate_plate_with_object_settings`].
    pub settings: Vec<Option<Settings>>,
    /// The height ranges of every object, for
    /// [`crate::generate_plate_with_own_settings`].
    pub ranges: Vec<Vec<HeightRange>>,
    /// The model file (its index) every object comes from.
    pub model: Vec<usize>,
}

/// Up to [`LISTED_KEYS`] of `keys`, separated by commas, and how many more
/// there are.
fn listed(keys: &[&str]) -> String {
    let mut text = keys
        .iter()
        .take(LISTED_KEYS)
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    if keys.len() > LISTED_KEYS {
        text.push_str(&format!(" and {} more", keys.len() - LISTED_KEYS));
    }
    text
}

/// Layers the settings of the whole project stored in a project file of
/// Orca Slicer or Bambu Studio over `base`. Returns what the file stores,
/// `None` if it is not a 3MF file. Settings without a counterpart are
/// reported (`project.settings_skipped`), as is a 3MF file that stores no
/// settings (`project.no_settings`).
pub fn project_base(
    model: Model,
    base: &mut Settings,
    warnings: &mut Diagnostics,
) -> Result<Option<ProjectSettings>, Diagnostic> {
    if !slicer_formats::looks_like_3mf(model.bytes) {
        return Ok(None);
    }
    let name = model.name;
    let stored = slicer_formats::read_project_settings(model.bytes)
        .map_err(|e| named(name, e.diagnostic()))?;
    if stored.is_empty() {
        warnings.push(named(
            name,
            Diagnostic::warning(
                "project.no_settings",
                "the file stores no settings of Orca Slicer or Bambu Studio",
            ),
        ));
    }
    if let Some(config) = &stored.config {
        let import =
            slicer_profiles::import_orca(config, base).map_err(|e| named(name, e.diagnostic()))?;
        if !import.skipped.is_empty() {
            let keys: Vec<&str> = import.skipped.iter().map(|(k, _)| k.as_str()).collect();
            warnings.push(named(
                name,
                Diagnostic::warning(
                    "project.settings_skipped",
                    format!(
                        "{} of the project's settings have no counterpart and were left out ({} taken): {}",
                        keys.len(),
                        import.taken.len(),
                        listed(&keys)
                    ),
                )
                .with("taken", import.taken.len())
                .with("skipped", keys.len())
                .with("keys", keys.join(" ")),
            ));
        }
    }
    Ok(Some(stored))
}

/// The settings of one object of a project file over `settings`, the
/// plate's, `None` if they change nothing, and its height ranges. Settings
/// that are not taken are reported, as are ranges whose heights are not
/// valid.
fn project_object(
    name: Option<&Name>,
    object: &ObjectSettings,
    settings: &Settings,
    warnings: &mut Diagnostics,
) -> (Option<Settings>, Vec<HeightRange>) {
    let object_name = match &object.name {
        Some(object_name) => format!("object '{object_name}' (id {})", object.id),
        None => format!("object {}", object.id),
    };
    let mut own = settings.clone();
    let skipped = apply_object_orca(&mut own, &object.values);
    if !skipped.is_empty() {
        let keys: Vec<&str> = skipped.iter().map(|(k, _)| k.as_str()).collect();
        let reasons: Vec<String> = skipped
            .iter()
            .take(LISTED_KEYS)
            .map(|(key, reason)| format!("{key} ({reason})"))
            .collect();
        let more = if keys.len() > LISTED_KEYS {
            ", ..."
        } else {
            ""
        };
        warnings.push(named(
            name,
            Diagnostic::warning(
                "project.object_settings_skipped",
                format!(
                    "{object_name}: {} setting(s) were left out: {}{more}",
                    keys.len(),
                    reasons.join(", "),
                ),
            )
            .with("object", object.id)
            .with("keys", keys.join(" ")),
        ));
    }
    if !object.part_keys.is_empty() {
        let keys: Vec<&str> = object.part_keys.iter().map(String::as_str).collect();
        warnings.push(named(
            name,
            Diagnostic::warning(
                "project.part_settings_skipped",
                format!(
                    "{object_name}: settings of its parts and modifiers are not read: {}",
                    listed(&keys)
                ),
            )
            .with("object", object.id)
            .with("keys", keys.join(" ")),
        ));
    }
    let ranges = project_ranges(name, object, &object_name, &own, warnings);
    ((own != *settings).then_some(own), ranges)
}

/// The height ranges of one object of a project file, whose settings are
/// `own`. Settings that are not taken are reported
/// (`project.range_settings_skipped`), as are ranges without valid heights
/// (`project.range_invalid`), which are left out.
// ponytail: the heights of a range are taken as heights above the bed;
// the project counts them from the bottom of the object, which is the
// same for objects standing on the bed.
fn project_ranges(
    name: Option<&Name>,
    object: &ObjectSettings,
    object_name: &str,
    own: &Settings,
    warnings: &mut Diagnostics,
) -> Vec<HeightRange> {
    let mut ranges = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut reasons: Vec<String> = Vec::new();
    for stored in &object.ranges {
        let (from, to) = (stored.min_z, stored.max_z);
        if !(from.is_finite() && to.is_finite() && from < to) {
            warnings.push(named(
                name,
                Diagnostic::warning(
                    "project.range_invalid",
                    format!(
                        "{object_name}: a height range from {from} to {to} mm is not valid and was left out"
                    ),
                )
                .with("object", object.id),
            ));
            continue;
        }
        let mut range = HeightRange::new(from, to);
        for (key, reason) in apply_range_orca(&mut range, own, &stored.values) {
            if !skipped.contains(&key) {
                if reasons.len() < LISTED_KEYS {
                    reasons.push(format!("{key} ({reason})"));
                }
                skipped.push(key);
            }
        }
        if !range.values.is_empty() {
            ranges.push(range);
        }
    }
    if !skipped.is_empty() {
        let more = if skipped.len() > LISTED_KEYS {
            ", ..."
        } else {
            ""
        };
        warnings.push(named(
            name,
            Diagnostic::warning(
                "project.range_settings_skipped",
                format!(
                    "{object_name}: {} setting(s) of its height ranges were left out: {}{more}",
                    skipped.len(),
                    reasons.join(", "),
                ),
            )
            .with("object", object.id)
            .with("keys", skipped.join(" ")),
        ));
    }
    ranges
}

/// One object read by [`model_objects`]: its mesh, its settings if it has
/// its own, and its height ranges.
pub type ModelObject = (Mesh, Option<Settings>, Vec<HeightRange>);

/// Reads `model` as one object, or, if `stored` (what [`project_base`]
/// returned for it) gives settings of their own or height ranges to
/// objects on the plate (over `settings`, the plate's), as one object per
/// build item, each with the settings and ranges of its object. `plate`
/// selects a build plate of a project file with several plates.
pub fn model_objects(
    model: Model,
    stored: Option<&ProjectSettings>,
    settings: &Settings,
    plate: Option<u32>,
    warnings: &mut Diagnostics,
) -> Result<Vec<ModelObject>, Diagnostic> {
    let name = model.name;
    if let Some(stored) = stored.filter(|stored| !stored.objects.is_empty()) {
        let mut found = Diagnostics::new();
        let objects = slicer_formats::parse_3mf_plate_objects(model.bytes, plate, &mut found)
            .map_err(|e| named(name, e.diagnostic()))?;
        // The settings of every object on the plate, once per object.
        let mut by_id: Vec<(u32, Option<Settings>, Vec<HeightRange>)> = Vec::new();
        for (id, _) in &objects {
            if by_id.iter().all(|(known, ..)| known != id) {
                let (own, ranges) = match stored.object(*id) {
                    Some(object) => project_object(name, object, settings, &mut found),
                    None => (None, Vec::new()),
                };
                by_id.push((*id, own, ranges));
            }
        }
        // Without settings of their own the objects are read as one, as
        // without the project's settings.
        if by_id
            .iter()
            .any(|(_, own, ranges)| own.is_some() || !ranges.is_empty())
        {
            for warning in found.as_slice() {
                warnings.push(warning.clone());
            }
            return Ok(objects
                .into_iter()
                .map(
                    |(id, mesh)| match by_id.iter().find(|(known, ..)| *known == id) {
                        Some((_, own, ranges)) => (mesh, own.clone(), ranges.clone()),
                        None => (mesh, None, Vec::new()),
                    },
                )
                .collect());
        }
        // Only what was found about the settings counts, the rest is
        // found again below.
        for warning in found.as_slice() {
            if warning.code.starts_with("project.") {
                warnings.push(warning.clone());
            }
        }
    }
    let mesh =
        slicer_formats::parse_model(model.bytes, plate, warnings).map_err(|e| named(name, e))?;
    Ok(vec![(mesh, None, Vec::new())])
}

impl Objects {
    /// Reads the objects of every one of `models` with [`model_objects`];
    /// `stored[i]` is what [`project_base`] returned for model `i` (none if
    /// the slice is shorter), `layers[i]` the settings for the objects of
    /// model `i`, layered over each of them in order: first those for the
    /// whole object, then those for height ranges. Fails if an object ends
    /// up with more than [`MAX_RANGES`] height ranges
    /// (`settings.too_many_ranges`).
    pub fn read(
        models: &[Model],
        stored: &[Option<ProjectSettings>],
        settings: &Settings,
        layers: &[Vec<ObjectLayer>],
        plate: Option<u32>,
        warnings: &mut Diagnostics,
    ) -> Result<Self, Diagnostic> {
        let mut objects = Self::default();
        for (index, model) in models.iter().enumerate() {
            let stored = stored.get(index).and_then(Option::as_ref);
            let layers = layers.get(index).map_or(&[][..], Vec::as_slice);
            for (mesh, mut own, mut ranges) in
                model_objects(*model, stored, settings, plate, warnings)?
            {
                for layer in layers.iter().filter(|layer| layer.range.is_none()) {
                    let object = own.get_or_insert_with(|| settings.clone());
                    apply_object_toml(object, &layer.text).map_err(|e| layer.name.apply(e))?;
                }
                let object = own.as_ref().unwrap_or(settings);
                for layer in layers {
                    let Some((from, to)) = layer.range else {
                        continue;
                    };
                    let at = match ranges.iter().position(|r| r.from == from && r.to == to) {
                        Some(at) => at,
                        None => {
                            ranges.push(HeightRange::new(from, to));
                            ranges.len() - 1
                        }
                    };
                    apply_range_toml(&mut ranges[at], object, &layer.text)
                        .map_err(|e| layer.name.apply(e))?;
                }
                if ranges.len() > MAX_RANGES {
                    return Err(named(
                        model.name,
                        Diagnostic::error(
                            "settings.too_many_ranges",
                            format!(
                                "an object has {} height ranges; at most {MAX_RANGES} are allowed",
                                ranges.len()
                            ),
                        )
                        .with("ranges", ranges.len())
                        .with("max", MAX_RANGES),
                    ));
                }
                objects.meshes.push(mesh);
                objects.settings.push(own);
                objects.ranges.push(ranges);
                objects.model.push(index);
            }
        }
        Ok(objects)
    }

    /// Puts the objects on the bed: as the files place them, centred (one
    /// model file, `files` < 2) or arranged side by side (several). The
    /// objects of one model file keep their places relative to each other.
    /// Returns how [`crate::generate_plate`] is to take them.
    pub fn place(&mut self, files: usize, placing: Placing, settings: &Settings) -> Placement {
        if placing.keep_positions {
            return Placement::AsGiven;
        }
        if files < 2 {
            return Placement::Centre;
        }
        let meshes = &mut self.meshes;
        if meshes.len() == files {
            crate::arrange_objects(meshes, settings, placing.gap);
            return Placement::AsGiven;
        }
        if crate::arranges_outlines(settings) {
            // The outline of every file's objects together, arranged as one
            // and turned together.
            let mut points: Vec<Vec<[f64; 2]>> = vec![Vec::new(); files];
            for (mesh, &file) in meshes.iter().zip(&self.model) {
                if let Some(slot) = points.get_mut(file) {
                    slot.extend(crate::footprint(mesh));
                }
            }
            let spots = crate::arrange_outlines(&points, settings, placing.gap);
            for (mesh, &file) in meshes.iter_mut().zip(&self.model) {
                if let Some(spot) = spots.get(file) {
                    spot.apply(mesh);
                }
            }
            return Placement::AsGiven;
        }
        // The bounds of every file's objects together, arranged as one.
        let mut bounds: Vec<Option<([f64; 3], [f64; 3])>> = vec![None; files];
        for (mesh, &file) in meshes.iter().zip(&self.model) {
            let Some(slot) = bounds.get_mut(file) else {
                continue;
            };
            if let Some((lo, hi)) = mesh.bounds() {
                *slot = Some(match *slot {
                    Some((a, b)) => (
                        [0, 1, 2].map(|i| a[i].min(lo[i])),
                        [0, 1, 2].map(|i| b[i].max(hi[i])),
                    ),
                    None => (lo, hi),
                });
            }
        }
        let sizes: Vec<[f64; 2]> = bounds
            .iter()
            .map(|b| b.map_or([0.0, 0.0], |(lo, hi)| [hi[0] - lo[0], hi[1] - lo[1]]))
            .collect();
        let corners = crate::arrange(&sizes, settings, placing.gap);
        for (mesh, &file) in meshes.iter_mut().zip(&self.model) {
            if let (Some(Some((lo, _))), Some(corner)) = (bounds.get(file), corners.get(file)) {
                mesh.translate([corner[0] - lo[0], corner[1] - lo[1], 0.0]);
            }
        }
        Placement::AsGiven
    }
}

/// What [`read_plate`] does besides reading the models.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Options {
    /// Use the print settings stored in project files of Orca Slicer or
    /// Bambu Studio.
    pub project_settings: bool,
    /// Build plate of a project file with several plates.
    pub plate: Option<u32>,
    /// How the objects are put on the bed.
    pub placing: Placing,
}

/// A plate read by [`read_plate`], ready for
/// [`crate::generate_plate_with_object_settings`].
#[derive(Debug, Clone)]
pub struct Plate {
    /// The settings of the plate.
    pub settings: Settings,
    /// The objects, put on the bed.
    pub objects: Objects,
    /// How the objects are to be taken.
    pub placement: Placement,
}

/// Reads `models` onto one plate. The plate's settings are the defaults,
/// with [`Options::project_settings`] the projects' settings over them
/// ([`project_base`], in the order of the models), and over those what
/// `settings` makes of them (the caller's settings for the plate). Then the
/// objects are read ([`Objects::read`]) with `layers` over the settings of
/// the objects of each model, and put on the bed ([`Objects::place`]).
/// Warnings, among them those about stored settings that are left out, go
/// to `warnings`.
pub fn read_plate(
    models: &[Model],
    options: Options,
    settings: impl FnOnce(Settings) -> Result<Settings, Diagnostic>,
    layers: &[Vec<ObjectLayer>],
    warnings: &mut Diagnostics,
) -> Result<Plate, Diagnostic> {
    let mut base = Settings::default();
    let mut stored = Vec::new();
    if options.project_settings {
        for model in models {
            stored.push(project_base(*model, &mut base, warnings)?);
        }
    }
    let settings = settings(base)?;
    let mut objects = Objects::read(models, &stored, &settings, layers, options.plate, warnings)?;
    let placement = objects.place(models.len(), options.placing, &settings);
    Ok(Plate {
        settings,
        objects,
        placement,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// A project file of Orca Slicer reduced to what is read: two cubes,
    /// the project's settings and settings of object 2 of its own.
    fn project() -> Vec<u8> {
        let settings = "<config><object id=\"2\"><metadata key=\"name\" value=\"Right\"/><metadata key=\"sparse_infill_density\" value=\"60%\"/><metadata key=\"layer_height\" value=\"0.1\"/></object></config>";
        project_with(settings)
    }

    /// The project of [`project`] with `settings` as its
    /// `Metadata/model_settings.config`.
    fn project_with(settings: &str) -> Vec<u8> {
        let cube = Mesh::cuboid(20.0, 20.0, 6.0);
        let vertices: String = cube
            .vertices
            .iter()
            .map(|[x, y, z]| format!("<vertex x=\"{x}\" y=\"{y}\" z=\"{z}\"/>"))
            .collect();
        let triangles: String = cube
            .triangles
            .iter()
            .map(|[a, b, c]| format!("<triangle v1=\"{a}\" v2=\"{b}\" v3=\"{c}\"/>"))
            .collect();
        let mesh = format!(
            "<mesh><vertices>{vertices}</vertices><triangles>{triangles}</triangles></mesh>"
        );
        let model = format!(
            "<?xml version=\"1.0\"?><model unit=\"millimeter\" xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\"><resources><object id=\"1\" type=\"model\">{mesh}</object><object id=\"2\" type=\"model\">{mesh}</object></resources><build><item objectid=\"1\" transform=\"1 0 0 0 1 0 0 0 1 80 100 0\"/><item objectid=\"2\" transform=\"1 0 0 0 1 0 0 0 1 140 100 0\"/></build></model>"
        );
        let config = r#"{ "layer_height": "0.25", "wall_loops": "3", "made_up_setting": "7" }"#;
        slicer_formats::write_package(&[
            ("3D/3dmodel.model", &model),
            ("Metadata/project_settings.config", config),
            ("Metadata/model_settings.config", settings),
        ])
        .unwrap()
    }

    #[test]
    fn settings_are_layered_in_order() {
        let bytes = project();
        let name = Name::new("a.3mf", "path", "a.3mf");
        let models = [Model {
            bytes: &bytes,
            name: Some(&name),
        }];
        let options = Options {
            project_settings: true,
            ..Options::default()
        };
        let layers = vec![vec![ObjectLayer {
            text: "wall_loops = 5".into(),
            name: Name::new("one.toml", "path", "one.toml"),
            range: None,
        }]];
        let mut warnings = Diagnostics::new();
        let plate = read_plate(
            &models,
            options,
            |mut base| {
                // The caller's settings go over the project's.
                assert_eq!(base.layer_height, 0.25);
                base.set("wall_loops", "4").unwrap();
                Ok(base)
            },
            &layers,
            &mut warnings,
        )
        .unwrap();
        assert_eq!(plate.settings.layer_height, 0.25);
        assert_eq!(plate.settings.wall_loops, 4);
        // Split into its two objects, which keep their places on the bed.
        assert_eq!(plate.objects.meshes.len(), 2);
        assert_eq!(plate.objects.model, [0, 0]);
        assert_eq!(plate.placement, Placement::Centre);
        // Object 2 has the project's infill over the plate's, the layer
        // over both; object 1 only the layer.
        let [first, second] = [0, 1].map(|i| plate.objects.settings[i].clone().unwrap());
        assert_eq!((first.wall_loops, second.wall_loops), (5, 5));
        assert_eq!(second.infill_density, 60.0);
        assert_eq!(first.infill_density, plate.settings.infill_density);
        assert_eq!(
            warnings.codes(),
            [
                "project.settings_skipped",
                "project.object_settings_skipped"
            ]
        );
        let skipped = &warnings.as_slice()[1];
        assert_eq!(skipped.context["path"], "a.3mf");
        assert!(skipped.message.starts_with("a.3mf: object 'Right' (id 2)"));

        // Without the option nothing of the project counts.
        let mut warnings = Diagnostics::new();
        let plate = read_plate(&models, Options::default(), Ok, &[], &mut warnings).unwrap();
        assert_eq!(plate.settings, Settings::default());
        assert_eq!(plate.objects.meshes.len(), 1);
        assert!(warnings.is_empty());
    }

    #[test]
    fn height_ranges_come_from_the_project_and_the_caller() {
        let settings = concat!(
            "<config><object id=\"1\"><metadata key=\"name\" value=\"Left\"/>",
            "<layer_config_ranges>",
            "<range min_z=\"2\" max_z=\"4\"><metadata key=\"sparse_infill_density\" value=\"100%\"/>",
            "<metadata key=\"layer_height\" value=\"0.1\"/></range>",
            "<range min_z=\"5\" max_z=\"1\"><metadata key=\"wall_loops\" value=\"4\"/></range>",
            "</layer_config_ranges></object></config>"
        );
        let bytes = project_with(settings);
        let name = Name::new("a.3mf", "path", "a.3mf");
        let models = [Model {
            bytes: &bytes,
            name: Some(&name),
        }];
        let options = Options {
            project_settings: true,
            ..Options::default()
        };
        let layer = |text: &str, range| ObjectLayer {
            text: text.into(),
            name: Name::new("r.toml", "path", "r.toml"),
            range,
        };
        let layers = vec![vec![
            layer("wall_loops = 4", Some((2.0, 4.0))),
            layer("top_layers = 2", Some((4.0, 6.0))),
            layer("infill_density = 30.0", None),
        ]];
        let mut warnings = Diagnostics::new();
        let plate = read_plate(&models, options, Ok, &layers, &mut warnings).unwrap();
        // The ranges alone split the project into its objects.
        let objects = &plate.objects;
        assert_eq!(objects.meshes.len(), 2);
        let first = &objects.ranges[0];
        assert_eq!(first.len(), 2);
        assert_eq!((first[0].from, first[0].to), (2.0, 4.0));
        let low = crate::range_settings(objects.settings[0].as_ref().unwrap(), &first[0]);
        assert_eq!((low.infill_density, low.wall_loops), (100.0, 4));
        // The caller's ranges go to every object of the model.
        assert_eq!(objects.ranges[1].len(), 2);
        assert_eq!(
            objects.ranges[1][0].values,
            [("wall_loops".into(), "4".into())]
        );
        assert_eq!(
            objects.ranges[1][1].values,
            [("top_layers".into(), "2".into())]
        );
        assert_eq!(objects.settings[1].as_ref().unwrap().infill_density, 30.0);
        assert_eq!(
            warnings.codes(),
            [
                "project.settings_skipped",
                "project.range_invalid",
                "project.range_settings_skipped"
            ]
        );
        assert_eq!(warnings.as_slice()[2].context["keys"], "layer_height");

        // Settings that a range cannot have fail, naming their file.
        let layers = vec![vec![layer("layer_height = 0.1", Some((1.0, 2.0)))]];
        let error = read_plate(&models, options, Ok, &layers, &mut Diagnostics::new()).unwrap_err();
        assert_eq!(error.code, "settings.not_per_range");
        assert!(error.message.starts_with("r.toml: "));
        assert_eq!(parse_height_range(" 2.5-7.25 "), Some((2.5, 7.25)));
        for text in ["", "5", "5-5", "6-5", "-1-5", "a-b", "inf-5"] {
            assert_eq!(parse_height_range(text), None, "{text}");
        }
    }

    #[test]
    fn errors_name_the_model() {
        let name = Name::new("model 2", "model", 2);
        let models = [Model {
            bytes: b"not a model",
            name: Some(&name),
        }];
        let options = Options {
            project_settings: true,
            ..Options::default()
        };
        let error = read_plate(&models, options, Ok, &[], &mut Diagnostics::new()).unwrap_err();
        assert_eq!(error.code, "model.unrecognized");
        assert!(error.message.starts_with("model 2: "));
        assert_eq!(error.context["model"], "2");
    }
}
