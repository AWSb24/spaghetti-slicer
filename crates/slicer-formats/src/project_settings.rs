//! Print settings stored in project files of Orca Slicer and Bambu Studio.
//!
//! The layout was worked out from project files that users saved:
//!
//! - `Metadata/project_settings.config` is a JSON object with every
//!   setting of the project's printer, filaments and process, by the
//!   program's own names. Per-extruder and per-filament values are lists.
//! - `Metadata/model_settings.config` is XML. Every `<object id="…">` (the
//!   identifier of the object in the 3D model) holds
//!   `<metadata key="…" value="…"/>` entries: its name, its extruder and
//!   the settings changed for this object only. Its `<part>` elements
//!   (parts, modifiers) carry their own entries, among them settings of
//!   modifiers, which are not read here.
//!
//! The values are returned as text; turning them into settings is up to the
//! caller (`slicer_profiles::orca`).

use quick_xml::events::Event;

use crate::threemf::{PLATE_PART, Package, ThreeMfError, attributes};

/// Name of the part with the settings of the whole project.
pub const PROJECT_CONFIG_PART: &str = "Metadata/project_settings.config";

/// Largest settings part that is read, in bytes. The parts seen in project
/// files are below 100 kB.
const MAX_CONFIG_BYTES: u64 = 16 << 20;

/// Entries of an object or part that describe it rather than set a print
/// setting.
const NOT_SETTINGS: &[&str] = &[
    "name",
    "extruder",
    "matrix",
    "source_file",
    "source_object_id",
    "source_volume_id",
    "source_offset_x",
    "source_offset_y",
    "source_offset_z",
    "source_in_inches",
    "source_in_meters",
];

/// Settings changed for one object of a project.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectSettings {
    /// Identifier of the object in the 3D model, as build items name it.
    pub id: u32,
    /// Name of the object, if the project gives one.
    pub name: Option<String>,
    /// The settings by the program's own names, with their values as text,
    /// in the order of the file.
    pub values: Vec<(String, String)>,
    /// Names of settings on parts or modifiers of the object, which are not
    /// read.
    pub part_keys: Vec<String>,
}

/// The print settings a project file stores.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectSettings {
    /// The JSON text of the project's settings, if the file has them.
    pub config: Option<String>,
    /// The objects with settings of their own, in the order of the file.
    pub objects: Vec<ObjectSettings>,
}

impl ProjectSettings {
    /// Whether the file stores no settings at all.
    pub fn is_empty(&self) -> bool {
        self.config.is_none() && self.objects.is_empty()
    }

    /// The settings of the object with identifier `id`, if it has any.
    pub fn object(&self, id: u32) -> Option<&ObjectSettings> {
        self.objects.iter().find(|object| object.id == id)
    }
}

/// Reads the settings stored in a project file of Orca Slicer or Bambu
/// Studio. A 3MF file without them gives empty settings.
pub fn read_project_settings(bytes: &[u8]) -> Result<ProjectSettings, ThreeMfError> {
    let archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| ThreeMfError::Archive(e.to_string()))?;
    let mut package = Package::from_archive(archive);
    let config = package.read_limited(PROJECT_CONFIG_PART, MAX_CONFIG_BYTES)?;
    let objects = match package.read_limited(PLATE_PART, MAX_CONFIG_BYTES)? {
        Some(text) => object_settings(&text)?,
        None => Vec::new(),
    };
    Ok(ProjectSettings { config, objects })
}

/// The objects of `Metadata/model_settings.config` that set anything.
fn object_settings(text: &str) -> Result<Vec<ObjectSettings>, ThreeMfError> {
    let mut objects = Vec::new();
    let mut current: Option<ObjectSettings> = None;
    let mut in_part = false;
    let mut reader = quick_xml::Reader::from_str(text);
    loop {
        let event = reader.read_event().map_err(|e| ThreeMfError::Xml {
            part: PLATE_PART.to_string(),
            message: e.to_string(),
        })?;
        match event {
            Event::Start(element) => match element.local_name().into_inner() {
                "object" => {
                    let [id] = attributes(&element, ["id"]);
                    current =
                        id.and_then(|id| id.trim().parse::<u32>().ok())
                            .map(|id| ObjectSettings {
                                id,
                                ..ObjectSettings::default()
                            });
                    in_part = false;
                }
                "part" => in_part = true,
                _ => {}
            },
            Event::Empty(element) if element.local_name().into_inner() == "metadata" => {
                let Some(object) = current.as_mut() else {
                    continue;
                };
                let [key, value] = attributes(&element, ["key", "value"]);
                let (Some(key), Some(value)) = (key, value) else {
                    continue;
                };
                if key == "name" && !in_part {
                    object.name = Some(value);
                } else if NOT_SETTINGS.contains(&key.as_str()) {
                    // Describes the object or part.
                } else if !in_part {
                    object.values.push((key, value));
                } else if !object.part_keys.contains(&key) {
                    object.part_keys.push(key);
                }
            }
            Event::End(element) => match element.local_name().into_inner() {
                "object" => {
                    if let Some(object) = current.take()
                        && !(object.values.is_empty() && object.part_keys.is_empty())
                    {
                        objects.push(object);
                    }
                }
                "part" => in_part = false,
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(objects)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::write_package;

    const MODEL_SETTINGS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<config>
  <object id="3">
    <metadata key="name" value="Body &amp; lid"/>
    <metadata key="extruder" value="1"/>
    <metadata key="sparse_infill_density" value="40%"/>
    <metadata key="brim_type" value="no_brim"/>
    <metadata face_count="12"/>
    <part id="1" subtype="normal_part">
      <metadata key="name" value="Body"/>
      <metadata key="matrix" value="1 0 0 0 0 1 0 0 0 0 1 0 0 0 0 1"/>
      <metadata key="source_in_meters" value="1"/>
      <mesh_stat face_count="12"/>
    </part>
    <part id="2" subtype="modifier_part">
      <metadata key="name" value="Modifier"/>
      <metadata key="wall_loops" value="5"/>
    </part>
  </object>
  <object id="4">
    <metadata key="name" value="Plain"/>
    <metadata key="extruder" value="2"/>
  </object>
  <plate>
    <metadata key="plater_id" value="1"/>
    <model_instance>
      <metadata key="object_id" value="3"/>
    </model_instance>
  </plate>
</config>
"#;

    #[test]
    fn project_and_object_settings_are_read() {
        let bytes = write_package(&[
            (PROJECT_CONFIG_PART, "{ \"layer_height\": \"0.16\" }"),
            (PLATE_PART, MODEL_SETTINGS),
        ])
        .unwrap();
        let read = read_project_settings(&bytes).unwrap();
        assert_eq!(
            read.config.as_deref(),
            Some("{ \"layer_height\": \"0.16\" }")
        );
        // Object 4 sets nothing and is left out.
        assert_eq!(
            read.objects,
            [ObjectSettings {
                id: 3,
                name: Some("Body & lid".into()),
                values: vec![
                    ("sparse_infill_density".into(), "40%".into()),
                    ("brim_type".into(), "no_brim".into()),
                ],
                part_keys: vec!["wall_loops".into()],
            }]
        );
        assert_eq!(read.object(3).map(|o| o.values.len()), Some(2));
        assert!(read.object(4).is_none());
    }

    #[test]
    fn files_without_settings_give_none() {
        let bytes = crate::write_3mf(&slicer_core::Mesh::cuboid(1.0, 1.0, 1.0)).unwrap();
        let read = read_project_settings(&bytes).unwrap();
        assert!(read.is_empty());
    }

    #[test]
    fn broken_files_give_errors() {
        assert!(matches!(
            read_project_settings(b"not a zip"),
            Err(ThreeMfError::Archive(_))
        ));
        let bytes = write_package(&[(PLATE_PART, "<config><object id=\"1\"></config>")]).unwrap();
        assert!(matches!(
            read_project_settings(&bytes),
            Err(ThreeMfError::Xml { .. })
        ));
        // Damaged packages fail or read, but never panic.
        let original = write_package(&[
            (PROJECT_CONFIG_PART, "{ \"layer_height\": \"0.16\" }"),
            (PLATE_PART, MODEL_SETTINGS),
        ])
        .unwrap();
        let mut state = 0x2545_f491_4f6c_dd1d;
        for _ in 0..300 {
            let _ = read_project_settings(&crate::mutate(&original, &mut state));
        }
    }
}
