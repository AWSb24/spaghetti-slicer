//! Project files: a standard 3MF package with several objects, each placed
//! by a build item with its transform, plus text parts of the application's
//! own (for example its settings) in the `Metadata/` folder. Other programs
//! that read 3MF see the objects where they were placed and skip the extra
//! parts.

use slicer_core::Mesh;

use crate::threemf::{
    CORE_NS, DEFAULT_MODEL_PART, Emit, IDENTITY, MODEL_RELATIONSHIP, Package, Reference,
    ThreeMfError, push_mesh, write_binary_package,
};

/// Where a project keeps the picture of its plate.
pub const THUMBNAIL_PART: &str = "Metadata/thumbnail.png";

/// An object of a project: a mesh in its own coordinates.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectObject {
    /// Name of the object, written as its `name` attribute.
    pub name: String,
    /// The geometry in millimetres. Its extruders are not stored.
    pub mesh: Mesh,
}

/// A build item: an object placed on the plate.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectItem {
    /// Index into [`ProjectPackage::objects`].
    pub object: usize,
    /// Transform in 3MF order (three rows of the linear part, then the
    /// translation in millimetres); a point `p` lands at `p · M`.
    pub transform: [f64; 12],
}

/// The content of a project file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectPackage {
    /// The objects; several items may share one.
    pub objects: Vec<ProjectObject>,
    /// The build items in order.
    pub items: Vec<ProjectItem>,
    /// Further text parts by name inside the package, such as
    /// `Metadata/app.toml`.
    pub parts: Vec<(String, String)>,
    /// Picture of the plate as PNG, written to [`THUMBNAIL_PART`] and
    /// named the package thumbnail (3MF core specification, OPC), which
    /// file managers and other programs show; empty for none. Not read
    /// back.
    pub thumbnail: Vec<u8>,
}

/// Content type of an extra part, by its extension.
fn content_type(name: &str) -> &'static str {
    match name.rsplit('.').next() {
        Some("toml") => "application/toml",
        Some("json") => "application/json",
        Some("xml") => "application/xml",
        _ => "text/plain",
    }
}

/// Writes a project as a 3MF package.
pub fn write_project(project: &ProjectPackage) -> Result<Vec<u8>, ThreeMfError> {
    const XML: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n";
    let mut model = format!(
        "{XML}<model unit=\"millimeter\" xml:lang=\"en-US\" xmlns=\"{CORE_NS}\">\n <metadata name=\"Application\">spaghetti-slicer-{}</metadata>\n <resources>\n",
        env!("CARGO_PKG_VERSION")
    );
    for (index, object) in project.objects.iter().enumerate() {
        model.push_str(&format!(
            "  <object id=\"{}\" name=\"{}\" type=\"model\">\n",
            index + 1,
            quick_xml::escape::escape(object.name.as_str())
        ));
        push_mesh(&mut model, &object.mesh);
        model.push_str("  </object>\n");
    }
    model.push_str(" </resources>\n <build>\n");
    for item in &project.items {
        if item.object >= project.objects.len() {
            return Err(ThreeMfError::UnknownObject {
                object: item.object as u32 + 1,
            });
        }
        let transform: Vec<String> = item.transform.iter().map(f64::to_string).collect();
        model.push_str(&format!(
            "  <item objectid=\"{}\" transform=\"{}\"/>\n",
            item.object + 1,
            transform.join(" ")
        ));
    }
    model.push_str(" </build>\n</model>\n");

    let mut content_types = format!(
        "{XML}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\n <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\n <Default Extension=\"model\" ContentType=\"application/vnd.ms-package.3dmanufacturing-3dmodel+xml\"/>\n"
    );
    if !project.thumbnail.is_empty() {
        content_types.push_str(" <Default Extension=\"png\" ContentType=\"image/png\"/>\n");
    }
    for (name, _) in &project.parts {
        content_types.push_str(&format!(
            " <Override PartName=\"/{}\" ContentType=\"{}\"/>\n",
            quick_xml::escape::escape(name.as_str()),
            content_type(name)
        ));
    }
    content_types.push_str("</Types>\n");
    let thumbnail = if project.thumbnail.is_empty() {
        String::new()
    } else {
        format!(
            " <Relationship Target=\"/{THUMBNAIL_PART}\" Id=\"rel1\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail\"/>\n"
        )
    };
    let rels = format!(
        "{XML}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n <Relationship Target=\"/{DEFAULT_MODEL_PART}\" Id=\"rel0\" Type=\"{MODEL_RELATIONSHIP}\"/>\n{thumbnail}</Relationships>\n"
    );
    let mut parts: Vec<(&str, &[u8])> = vec![
        ("[Content_Types].xml", content_types.as_bytes()),
        ("_rels/.rels", rels.as_bytes()),
        (DEFAULT_MODEL_PART, model.as_bytes()),
    ];
    parts.extend(
        project
            .parts
            .iter()
            .map(|(name, text)| (name.as_str(), text.as_bytes())),
    );
    if !project.thumbnail.is_empty() {
        parts.push((THUMBNAIL_PART, &project.thumbnail));
    }
    write_binary_package(&parts)
}

/// Reads one part of a 3MF package as text; `None` if the package has no
/// part of that name.
pub fn read_part(bytes: &[u8], name: &str) -> Result<Option<String>, ThreeMfError> {
    let archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| ThreeMfError::Archive(e.to_string()))?;
    Package::from_archive(archive).read(name)
}

/// Reads a 3MF package as a project: every object that a build item
/// places, in its own coordinates and with its components merged, and the
/// build items with their transforms. `parts` names the extra text parts
/// to read; those missing from the package are left out. Objects that are
/// not of type `model` and items placing nothing are skipped.
pub fn read_project(bytes: &[u8], parts: &[&str]) -> Result<ProjectPackage, ThreeMfError> {
    let (mut package, root) = Package::open(bytes)?;
    let build: Vec<Reference> = package
        .parts
        .get(&root)
        .map(|part| part.build.clone())
        .unwrap_or_default();
    let mut project = ProjectPackage::default();
    // The objects read so far, by part and identifier, with their index in
    // `project.objects` (`None` if they hold nothing to print).
    let mut known: Vec<((String, u32), Option<usize>)> = Vec::new();
    for item in build {
        let part = item.path.as_deref().map_or(root.clone(), |path| {
            path.trim_start_matches('/').to_string()
        });
        let key = (part.clone(), item.object);
        let index = match known.iter().find(|(k, _)| *k == key) {
            Some((_, index)) => *index,
            None => {
                let local = Reference {
                    transform: IDENTITY,
                    ..item.clone()
                };
                let mut mesh = Mesh::default();
                let mut skipped = 0;
                package.emit(
                    &root,
                    &local,
                    &IDENTITY,
                    0,
                    &mut skipped,
                    &mut mesh,
                    Emit::default(),
                )?;
                let index = (!mesh.triangles.is_empty()).then(|| {
                    let name = package
                        .parts
                        .get(&part)
                        .and_then(|p| p.objects.get(&item.object))
                        .and_then(|object| object.name.clone())
                        .unwrap_or_else(|| format!("Object {}", item.object));
                    project.objects.push(ProjectObject { name, mesh });
                    project.objects.len() - 1
                });
                known.push((key, index));
                index
            }
        };
        if let Some(object) = index {
            project.items.push(ProjectItem {
                object,
                transform: item.transform,
            });
        }
    }
    for name in parts {
        if let Some(text) = package.read(name)? {
            project.parts.push(((*name).to_string(), text));
        }
    }
    Ok(project)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::{parse_3mf, write_3mf};
    use slicer_core::Diagnostics;

    fn tetrahedron(size: f64) -> Mesh {
        Mesh {
            vertices: vec![
                [0.0, 0.0, 0.0],
                [size, 0.0, 0.0],
                [0.0, size, 0.0],
                [0.0, 0.0, size],
            ],
            triangles: vec![[0, 2, 1], [0, 1, 3], [1, 2, 3], [2, 0, 3]],
            extruders: Vec::new(),
        }
    }

    fn sample() -> ProjectPackage {
        let turn = std::f64::consts::FRAC_1_SQRT_2;
        ProjectPackage {
            objects: vec![
                ProjectObject {
                    name: "cube & <friends>.stl".into(),
                    mesh: tetrahedron(10.0),
                },
                ProjectObject {
                    name: "second".into(),
                    mesh: tetrahedron(0.1 + 0.2),
                },
            ],
            items: vec![
                ProjectItem {
                    object: 0,
                    transform: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 50.0, 60.0, 0.0],
                },
                ProjectItem {
                    object: 1,
                    transform: [
                        turn, turn, 0.0, -turn, turn, 0.0, 0.0, 0.0, 1.0, -0.125, 1e-7, 0.0,
                    ],
                },
                ProjectItem {
                    object: 0,
                    transform: [
                        2.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 2.0, 100.0, 100.0, 0.0,
                    ],
                },
            ],
            parts: vec![(
                "Metadata/test.toml".into(),
                "version = 1\nname = \"ä\"\n".into(),
            )],
            thumbnail: Vec::new(),
        }
    }

    #[test]
    fn a_project_reads_back_as_written() {
        let project = sample();
        let bytes = write_project(&project).unwrap();
        let read = read_project(&bytes, &["Metadata/test.toml", "Metadata/missing.toml"]).unwrap();
        assert_eq!(read, project);
        assert_eq!(
            read_part(&bytes, "Metadata/test.toml").unwrap().as_deref(),
            Some("version = 1\nname = \"ä\"\n")
        );
        assert_eq!(read_part(&bytes, "Metadata/missing.toml").unwrap(), None);
    }

    #[test]
    fn other_readers_see_the_placed_objects() {
        let bytes = write_project(&sample()).unwrap();
        let mesh = parse_3mf(&bytes, &mut Diagnostics::new()).unwrap();
        assert_eq!(mesh.triangles.len(), 12);
        let (lo, hi) = mesh.bounds().unwrap();
        // The third item doubles the first object at (100, 100).
        assert_eq!(hi, [120.0, 120.0, 20.0]);
        assert!(lo[0] < 0.0, "{lo:?}");
    }

    #[test]
    fn a_plain_3mf_reads_as_a_project_without_parts() {
        let bytes = write_3mf(&tetrahedron(5.0)).unwrap();
        let read = read_project(&bytes, &["Metadata/test.toml"]).unwrap();
        assert_eq!(read.objects.len(), 1);
        assert_eq!(read.objects[0].mesh, tetrahedron(5.0));
        assert_eq!(read.objects[0].name, "Object 1");
        assert_eq!(read.items[0].transform, IDENTITY);
        assert!(read.parts.is_empty());
    }

    #[test]
    fn an_empty_project_is_valid() {
        let project = ProjectPackage {
            parts: vec![("Metadata/test.toml".into(), String::new())],
            ..ProjectPackage::default()
        };
        let bytes = write_project(&project).unwrap();
        assert_eq!(
            read_project(&bytes, &["Metadata/test.toml"]).unwrap(),
            project
        );
    }

    #[test]
    fn the_picture_is_the_package_thumbnail() {
        let png = slicer_core::png::encode(1, 1, slicer_core::png::ColorType::Rgba, &[1, 2, 3, 4])
            .unwrap();
        let project = ProjectPackage {
            thumbnail: png.clone(),
            ..sample()
        };
        let bytes = write_project(&project).unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes[..])).unwrap();
        let mut stored = Vec::new();
        std::io::Read::read_to_end(&mut archive.by_name(THUMBNAIL_PART).unwrap(), &mut stored)
            .unwrap();
        assert_eq!(stored, png);
        let rels = read_part(&bytes, "_rels/.rels").unwrap().unwrap();
        assert!(rels.contains("Target=\"/Metadata/thumbnail.png\" Id=\"rel1\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail\""), "{rels}");
        let types = read_part(&bytes, "[Content_Types].xml").unwrap().unwrap();
        assert!(types.contains("Extension=\"png\""), "{types}");
        // The objects read as before; the picture is not read back.
        assert_eq!(
            read_project(&bytes, &["Metadata/test.toml"]).unwrap(),
            sample()
        );
    }

    #[test]
    fn items_must_name_an_object() {
        let mut project = sample();
        project.items[0].object = 7;
        assert_eq!(
            write_project(&project).unwrap_err(),
            ThreeMfError::UnknownObject { object: 8 }
        );
    }
}
