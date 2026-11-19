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
//!   the settings changed for this object only. Its `<part id="…"
//!   subtype="…">` elements carry their own entries: `normal_part` for a
//!   printed part, `modifier_part` for a modifier with the settings
//!   changed inside it, and (from the user documentation)
//!   `negative_part`, `support_blocker` and `support_enforcer`. The `id`
//!   of a part is the identifier of the object that the matching
//!   `<component>` of the object in the 3D model refers to. Settings of
//!   modifiers are read; those of other parts are only named.
//! - None of the project files examined so far has a modifier, so how the
//!   settings of a modifier are stored is inferred: as the
//!   `<metadata key="…" value="…"/>` entries of its part, like those of
//!   the object.
//! - Settings for a range of the height of an object ("height range
//!   modifiers") are not in any of the project files seen so far. They are
//!   read from `<range min_z="…" max_z="…">` elements inside the object
//!   (usually wrapped in `<layer_config_ranges>`), with the settings as
//!   `<metadata key="…" value="…"/>` entries or as
//!   `<option opt_key="…">value</option>` elements.
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

/// Settings changed for a range of the height of one object.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RangeSettings {
    /// Height where the range starts, in millimetres above the bottom of
    /// the object; not a number if the file gives none.
    pub min_z: f64,
    /// Height where the range ends; not a number if the file gives none.
    pub max_z: f64,
    /// The settings by the program's own names, with their values as text,
    /// in the order of the file.
    pub values: Vec<(String, String)>,
}

/// Settings changed inside a modifier of an object (a part with
/// `subtype="modifier_part"`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModifierSettings {
    /// Identifier of the part, as `parse_3mf_plate_parts` gives it with the
    /// mesh of the modifier.
    pub id: u32,
    /// Name of the modifier, if the project gives one.
    pub name: Option<String>,
    /// The settings by the program's own names, with their values as text,
    /// in the order of the file.
    pub values: Vec<(String, String)>,
}

/// Settings changed for one object of a project.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ObjectSettings {
    /// Identifier of the object in the 3D model, as build items name it.
    pub id: u32,
    /// Name of the object, if the project gives one.
    pub name: Option<String>,
    /// The settings by the program's own names, with their values as text,
    /// in the order of the file.
    pub values: Vec<(String, String)>,
    /// Names of settings on parts of the object other than modifiers,
    /// which are not read.
    pub part_keys: Vec<String>,
    /// The modifiers of the object that set anything, in the order of the
    /// file.
    pub modifiers: Vec<ModifierSettings>,
    /// Settings changed for ranges of its height, in the order of the file.
    pub ranges: Vec<RangeSettings>,
}

/// The print settings a project file stores.
#[derive(Debug, Clone, Default, PartialEq)]
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
    // The modifier whose part is being read.
    let mut modifier: Option<ModifierSettings> = None;
    // The height range being read, and the setting of the `<option>`
    // element being read with its text so far.
    let mut range: Option<RangeSettings> = None;
    let mut option: Option<(String, String)> = None;
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
                    range = None;
                    modifier = None;
                }
                "part" => {
                    in_part = true;
                    let [id, subtype] = attributes(&element, ["id", "subtype"]);
                    modifier = id
                        .and_then(|id| id.trim().parse::<u32>().ok())
                        .filter(|_| subtype.as_deref() == Some("modifier_part"))
                        .map(|id| ModifierSettings {
                            id,
                            ..ModifierSettings::default()
                        });
                }
                "range" if current.is_some() && !in_part => {
                    let [min, max] = attributes(&element, ["min_z", "max_z"]);
                    let height = |text: Option<String>| {
                        text.and_then(|t| t.trim().parse::<f64>().ok())
                            .unwrap_or(f64::NAN)
                    };
                    range = Some(RangeSettings {
                        min_z: height(min),
                        max_z: height(max),
                        values: Vec::new(),
                    });
                }
                "option" if range.is_some() => {
                    let [key] = attributes(&element, ["opt_key"]);
                    option = key.map(|key| (key, String::new()));
                }
                _ => {}
            },
            Event::Text(text) => {
                if let Some((_, value)) = option.as_mut() {
                    value.push_str(&text.xml_content(quick_xml::XmlVersion::Implicit1_0));
                }
            }
            Event::Empty(element) if element.local_name().into_inner() == "metadata" => {
                let Some(object) = current.as_mut() else {
                    continue;
                };
                let [key, value] = attributes(&element, ["key", "value"]);
                let (Some(key), Some(value)) = (key, value) else {
                    continue;
                };
                if let Some(range) = range.as_mut() {
                    if !NOT_SETTINGS.contains(&key.as_str()) {
                        range.values.push((key, value));
                    }
                } else if key == "name" && !in_part {
                    object.name = Some(value);
                } else if let Some(modifier) = modifier.as_mut().filter(|_| in_part) {
                    if key == "name" {
                        modifier.name = Some(value);
                    } else if !NOT_SETTINGS.contains(&key.as_str()) {
                        modifier.values.push((key, value));
                    }
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
                        && !(object.values.is_empty()
                            && object.part_keys.is_empty()
                            && object.modifiers.is_empty()
                            && object.ranges.is_empty())
                    {
                        objects.push(object);
                    }
                }
                "part" => {
                    in_part = false;
                    if let (Some(object), Some(modifier)) = (current.as_mut(), modifier.take())
                        && !modifier.values.is_empty()
                    {
                        object.modifiers.push(modifier);
                    }
                }
                "range" => {
                    if let (Some(object), Some(range)) = (current.as_mut(), range.take()) {
                        object.ranges.push(range);
                    }
                }
                "option" => {
                    if let (Some(range), Some((key, value))) = (range.as_mut(), option.take()) {
                        range.values.push((key, value.trim().to_string()));
                    }
                }
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
    <part id="7" subtype="negative_part">
      <metadata key="name" value="Hole"/>
      <metadata key="extruder" value="1"/>
    </part>
  </object>
  <object id="4">
    <metadata key="name" value="Plain"/>
    <metadata key="extruder" value="2"/>
  </object>
  <object id="5">
    <metadata key="name" value="Ranged"/>
    <layer_config_ranges>
      <range min_z="10" max_z="20.5">
        <metadata key="extruder" value="1"/>
        <metadata key="sparse_infill_density" value="100%"/>
        <metadata key="layer_height" value="0.1"/>
      </range>
      <range min_z="25" max_z="30">
        <option opt_key="wall_loops">4</option>
      </range>
      <range max_z="40"/>
    </layer_config_ranges>
  </object>
  <object id="6">
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
        // Objects 4 and 6 set nothing and are left out.
        assert_eq!(read.objects.len(), 2);
        assert_eq!(
            read.objects[..1],
            [ObjectSettings {
                id: 3,
                name: Some("Body & lid".into()),
                values: vec![
                    ("sparse_infill_density".into(), "40%".into()),
                    ("brim_type".into(), "no_brim".into()),
                ],
                part_keys: Vec::new(),
                modifiers: vec![ModifierSettings {
                    id: 2,
                    name: Some("Modifier".into()),
                    values: vec![("wall_loops".into(), "5".into())],
                }],
                ranges: Vec::new(),
            }]
        );
        assert_eq!(read.object(3).map(|o| o.values.len()), Some(2));
        assert!(read.object(4).is_none());
        // Object 5 sets nothing but has height ranges; the empty range is
        // left out.
        let ranged = read.object(5).unwrap();
        assert!(ranged.values.is_empty());
        assert_eq!(
            ranged.ranges,
            [
                RangeSettings {
                    min_z: 10.0,
                    max_z: 20.5,
                    values: vec![
                        ("sparse_infill_density".into(), "100%".into()),
                        ("layer_height".into(), "0.1".into()),
                    ],
                },
                RangeSettings {
                    min_z: 25.0,
                    max_z: 30.0,
                    values: vec![("wall_loops".into(), "4".into())],
                },
            ]
        );
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
