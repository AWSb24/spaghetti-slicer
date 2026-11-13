//! 3MF reader and writer for the core specification (geometry only).
//!
//! Reads meshes, components, build items and their transforms. Of the
//! production extension it follows component references into other model
//! parts of the same package. Everything else (materials, colours, slices,
//! vendor data) is ignored, with one exception: project files of Bambu
//! Studio list which objects lie on which build plate, and the reader can
//! pick one plate.

use std::collections::HashMap;
use std::fmt;
use std::io::{Cursor, Read, Write};

use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::reader::NsReader;
use slicer_core::{Diagnostic, Diagnostics, Mesh};

const CORE_NS: &str = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";
const PRODUCTION_NS: &str = "http://schemas.microsoft.com/3dmanufacturing/production/2015/06";
const MODEL_RELATIONSHIP: &str = "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel";
const DEFAULT_MODEL_PART: &str = "3D/3dmodel.model";

/// Largest model part that is unpacked, as a guard against archive bombs.
const MAX_PART_BYTES: u64 = 1 << 30;
/// Largest number of triangles a file may expand to through components.
const MAX_TRIANGLES: usize = 50_000_000;
/// Deepest allowed nesting of components.
const MAX_DEPTH: usize = 32;

/// Why a 3MF file could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreeMfError {
    /// The file is not a readable ZIP archive.
    Archive(String),
    /// The package has no 3D model part, or a referenced part is missing.
    MissingPart {
        /// Name of the part inside the package.
        part: String,
    },
    /// A model part is not well-formed XML.
    Xml {
        /// Name of the part inside the package.
        part: String,
        /// Description from the XML parser.
        message: String,
    },
    /// A required attribute is missing or cannot be parsed.
    InvalidAttribute {
        /// Name of the part inside the package.
        part: String,
        /// Element carrying the attribute.
        element: &'static str,
        /// Name of the attribute.
        attribute: &'static str,
    },
    /// The model uses a unit this reader does not know.
    UnsupportedUnit {
        /// The unit as written in the file.
        unit: String,
    },
    /// A triangle refers to a vertex that does not exist.
    InvalidIndex {
        /// Identifier of the object containing the triangle.
        object: u32,
    },
    /// A build item or component refers to an object that does not exist.
    UnknownObject {
        /// Identifier that could not be resolved.
        object: u32,
    },
    /// Components are nested too deeply, most likely in a cycle.
    ComponentCycle,
    /// A coordinate is NaN or infinite.
    NonFiniteCoordinate,
    /// The file contains no printable triangles.
    NoTriangles,
    /// A model part or the expanded model exceeds the size limits.
    TooLarge,
    /// The requested build plate does not exist in the file.
    UnknownPlate {
        /// The plate that was asked for.
        plate: u32,
        /// The plates the file has; empty if it has no plate information.
        available: Vec<u32>,
    },
}

impl fmt::Display for ThreeMfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Archive(message) => write!(f, "not a valid 3MF package: {message}"),
            Self::MissingPart { part } => write!(f, "3MF package has no part '{part}'"),
            Self::Xml { part, message } => write!(f, "'{part}' is not valid XML: {message}"),
            Self::InvalidAttribute {
                part,
                element,
                attribute,
            } => write!(
                f,
                "'{part}': <{element}> has a missing or invalid '{attribute}' attribute"
            ),
            Self::UnsupportedUnit { unit } => write!(f, "unsupported unit '{unit}'"),
            Self::InvalidIndex { object } => write!(
                f,
                "object {object} has a triangle that refers to a missing vertex"
            ),
            Self::UnknownObject { object } => {
                write!(f, "reference to object {object}, which does not exist")
            }
            Self::ComponentCycle => write!(f, "components are nested too deeply or in a cycle"),
            Self::NonFiniteCoordinate => write!(f, "file contains NaN or infinite coordinates"),
            Self::NoTriangles => write!(f, "file contains no printable triangles"),
            Self::TooLarge => write!(f, "model exceeds the size limits of the 3MF reader"),
            Self::UnknownPlate { plate, available } if available.is_empty() => write!(
                f,
                "plate {plate} was requested, but the file has no build plates to choose from"
            ),
            Self::UnknownPlate { plate, available } => {
                let list: Vec<String> = available.iter().map(u32::to_string).collect();
                write!(
                    f,
                    "the file has no plate {plate}; it has {}",
                    list.join(", ")
                )
            }
        }
    }
}

impl std::error::Error for ThreeMfError {}

impl slicer_core::Diagnose for ThreeMfError {
    fn code(&self) -> &'static str {
        match self {
            Self::Archive(_) => "model.archive",
            Self::MissingPart { .. } => "model.missing_part",
            Self::Xml { .. } => "model.xml",
            Self::InvalidAttribute { .. } => "model.invalid_attribute",
            Self::UnsupportedUnit { .. } => "model.unsupported_unit",
            Self::InvalidIndex { .. } => "model.invalid_index",
            Self::UnknownObject { .. } => "model.unknown_object",
            Self::ComponentCycle => "model.component_cycle",
            Self::NonFiniteCoordinate => "model.non_finite",
            Self::NoTriangles => "model.no_triangles",
            Self::TooLarge => "model.too_large",
            Self::UnknownPlate { .. } => "model.unknown_plate",
        }
    }

    fn context(&self) -> Vec<(&'static str, String)> {
        match self {
            Self::MissingPart { part } | Self::Xml { part, .. } => vec![("part", part.clone())],
            Self::InvalidAttribute {
                part,
                element,
                attribute,
            } => vec![
                ("part", part.clone()),
                ("element", (*element).to_string()),
                ("attribute", (*attribute).to_string()),
            ],
            Self::UnsupportedUnit { unit } => vec![("unit", unit.clone())],
            Self::UnknownPlate { plate, .. } => vec![("plate", plate.to_string())],
            Self::InvalidIndex { object } | Self::UnknownObject { object } => {
                vec![("object", object.to_string())]
            }
            _ => Vec::new(),
        }
    }
}

/// An affine transform in 3MF order: three rows of the linear part, then the
/// translation. Points are row vectors, so `p' = p · M`.
type Transform = [f64; 12];

const IDENTITY: Transform = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];

fn apply(m: &Transform, p: [f64; 3]) -> [f64; 3] {
    [
        p[0] * m[0] + p[1] * m[3] + p[2] * m[6] + m[9],
        p[0] * m[1] + p[1] * m[4] + p[2] * m[7] + m[10],
        p[0] * m[2] + p[1] * m[5] + p[2] * m[8] + m[11],
    ]
}

/// The transform that applies `first` and then `second`.
fn then(first: &Transform, second: &Transform) -> Transform {
    let mut out = [0.0; 12];
    for row in 0..4 {
        for col in 0..3 {
            let mut sum = 0.0;
            for k in 0..3 {
                sum += first[row * 3 + k] * second[k * 3 + col];
            }
            if row == 3 {
                sum += second[9 + col];
            }
            out[row * 3 + col] = sum;
        }
    }
    out
}

fn determinant(m: &Transform) -> f64 {
    m[0] * (m[4] * m[8] - m[5] * m[7]) - m[1] * (m[3] * m[8] - m[5] * m[6])
        + m[2] * (m[3] * m[7] - m[4] * m[6])
}

/// A reference to an object, from a build item or a component.
struct Reference {
    /// Model part holding the object; `None` means the referring part.
    path: Option<String>,
    object: u32,
    transform: Transform,
}

#[derive(Default)]
struct Object {
    printable: bool,
    vertices: Vec<[f64; 3]>,
    triangles: Vec<[u32; 3]>,
    components: Vec<Reference>,
}

/// One parsed model part, already converted to millimetres.
#[derive(Default)]
struct Part {
    objects: HashMap<u32, Object>,
    build: Vec<Reference>,
    extensions: Vec<String>,
}

/// Reads the attributes of one element into `slots`, matched by local name.
fn attributes<const N: usize>(element: &BytesStart<'_>, names: [&str; N]) -> [Option<String>; N] {
    let mut slots = [const { None }; N];
    for attribute in element.attributes().flatten() {
        let local = attribute.key.local_name();
        if let Some(index) = names.iter().position(|name| *name == local.into_inner()) {
            slots[index] = attribute
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .ok()
                .map(std::borrow::Cow::into_owned);
        }
    }
    slots
}

fn parse_transform(text: &str, scale: f64) -> Option<Transform> {
    let mut out = [0.0; 12];
    let mut tokens = text.split_whitespace();
    for (index, slot) in out.iter_mut().enumerate() {
        let value: f64 = tokens.next()?.parse().ok()?;
        *slot = if index >= 9 { value * scale } else { value };
    }
    tokens.next().is_none().then_some(out)
}

fn unit_scale(unit: &str) -> Option<f64> {
    Some(match unit {
        "micron" => 0.001,
        "millimeter" => 1.0,
        "centimeter" => 10.0,
        "inch" => 25.4,
        "foot" => 304.8,
        "meter" => 1000.0,
        _ => return None,
    })
}

fn parse_part(name: &str, text: &str) -> Result<Part, ThreeMfError> {
    let invalid = |element: &'static str, attribute: &'static str| ThreeMfError::InvalidAttribute {
        part: name.to_string(),
        element,
        attribute,
    };
    let reference = |element: &BytesStart<'_>,
                     tag: &'static str,
                     scale: f64|
     -> Result<Reference, ThreeMfError> {
        let [object, transform, path] = attributes(element, ["objectid", "transform", "path"]);
        Ok(Reference {
            path,
            object: object
                .and_then(|v| v.trim().parse().ok())
                .ok_or_else(|| invalid(tag, "objectid"))?,
            transform: match transform {
                Some(text) => {
                    parse_transform(&text, scale).ok_or_else(|| invalid(tag, "transform"))?
                }
                None => IDENTITY,
            },
        })
    };

    let mut part = Part::default();
    let mut scale = 1.0;
    let mut current: Option<(u32, Object)> = None;
    let mut reader = NsReader::from_str(text);
    loop {
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|e| ThreeMfError::Xml {
                part: name.to_string(),
                message: e.to_string(),
            })?;
        let core = matches!(namespace, ResolveResult::Bound(Namespace(CORE_NS)));
        match event {
            Event::Eof => break,
            Event::End(element) if core && element.local_name().into_inner() == "object" => {
                if let Some((id, object)) = current.take() {
                    part.objects.insert(id, object);
                }
            }
            Event::Start(element) | Event::Empty(element) if core => {
                match element.local_name().into_inner() {
                    "model" => {
                        let [unit, required] = attributes(&element, ["unit", "requiredextensions"]);
                        let required = required.unwrap_or_default();
                        if let Some(unit) = unit {
                            scale =
                                unit_scale(&unit).ok_or(ThreeMfError::UnsupportedUnit { unit })?;
                        }
                        for attribute in element.attributes().flatten() {
                            // Most writers declare every extension they know;
                            // only the ones marked as required matter.
                            let prefix = attribute.key.into_inner().strip_prefix("xmlns:");
                            if prefix.is_some_and(|p| required.split_whitespace().any(|r| r == p))
                                && attribute.value != CORE_NS
                                && attribute.value != PRODUCTION_NS
                            {
                                part.extensions.push(attribute.value.into_owned());
                            }
                        }
                    }
                    "object" => {
                        let [id, kind] = attributes(&element, ["id", "type"]);
                        let id = id
                            .and_then(|v| v.trim().parse().ok())
                            .ok_or_else(|| invalid("object", "id"))?;
                        let object = Object {
                            printable: kind.is_none_or(|k| k == "model"),
                            ..Object::default()
                        };
                        // Also covers an empty `<object/>`, which has no end tag.
                        part.objects.insert(id, Object::default());
                        current = Some((id, object));
                    }
                    "vertex" => {
                        if let Some((_, object)) = &mut current {
                            let mut vertex = [0.0; 3];
                            let values = attributes(&element, ["x", "y", "z"]);
                            for ((slot, value), axis) in
                                vertex.iter_mut().zip(values).zip(["x", "y", "z"])
                            {
                                let value: f64 = value
                                    .and_then(|v| v.trim().parse().ok())
                                    .ok_or_else(|| invalid("vertex", axis))?;
                                *slot = value * scale;
                            }
                            object.vertices.push(vertex);
                        }
                    }
                    "triangle" => {
                        if let Some((_, object)) = &mut current {
                            let mut triangle = [0u32; 3];
                            let values = attributes(&element, ["v1", "v2", "v3"]);
                            for ((slot, value), corner) in
                                triangle.iter_mut().zip(values).zip(["v1", "v2", "v3"])
                            {
                                *slot = value
                                    .and_then(|v| v.trim().parse().ok())
                                    .ok_or_else(|| invalid("triangle", corner))?;
                            }
                            object.triangles.push(triangle);
                        }
                    }
                    "component" => {
                        if let Some((_, object)) = &mut current {
                            object
                                .components
                                .push(reference(&element, "component", scale)?);
                        }
                    }
                    "item" => part.build.push(reference(&element, "item", scale)?),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    Ok(part)
}

/// Finds the root model part through the package relationships.
fn root_part(rels: &str) -> Option<String> {
    let mut reader = NsReader::from_str(rels);
    loop {
        match reader.read_resolved_event().ok()?.1 {
            Event::Eof => return None,
            Event::Start(element) | Event::Empty(element)
                if element.local_name().into_inner() == "Relationship" =>
            {
                let [kind, target] = attributes(&element, ["Type", "Target"]);
                if kind.as_deref() == Some(MODEL_RELATIONSHIP) {
                    return target;
                }
            }
            _ => {}
        }
    }
}

struct Package<'a> {
    archive: zip::ZipArchive<Cursor<&'a [u8]>>,
    parts: HashMap<String, Part>,
}

impl Package<'_> {
    /// Reads a part as text. Part names are case-insensitive and may be
    /// written with a leading slash.
    fn read(&mut self, name: &str) -> Result<Option<String>, ThreeMfError> {
        let wanted = name.trim_start_matches('/');
        let Some(actual) = self
            .archive
            .file_names()
            .flatten()
            .find(|candidate| candidate.eq_ignore_ascii_case(wanted))
            .map(std::borrow::Cow::into_owned)
        else {
            return Ok(None);
        };
        let file = self
            .archive
            .by_name(&actual)
            .map_err(|e| ThreeMfError::Archive(e.to_string()))?;
        let mut bytes = Vec::new();
        file.take(MAX_PART_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| ThreeMfError::Archive(e.to_string()))?;
        if bytes.len() as u64 > MAX_PART_BYTES {
            return Err(ThreeMfError::TooLarge);
        }
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|e| ThreeMfError::Xml {
                part: wanted.to_string(),
                message: e.to_string(),
            })
    }

    fn load(&mut self, name: &str) -> Result<(), ThreeMfError> {
        if !self.parts.contains_key(name) {
            let text = self.read(name)?.ok_or_else(|| ThreeMfError::MissingPart {
                part: name.to_string(),
            })?;
            self.parts
                .insert(name.to_string(), parse_part(name, &text)?);
        }
        Ok(())
    }

    /// Appends the object `reference` points to, with all its components.
    fn emit(
        &mut self,
        part: &str,
        reference: &Reference,
        outer: &Transform,
        depth: usize,
        skipped: &mut usize,
        mesh: &mut Mesh,
    ) -> Result<(), ThreeMfError> {
        if depth > MAX_DEPTH {
            return Err(ThreeMfError::ComponentCycle);
        }
        let part = match &reference.path {
            Some(path) => path.trim_start_matches('/').to_string(),
            None => part.to_string(),
        };
        self.load(&part)?;
        let transform = then(&reference.transform, outer);
        let unknown = ThreeMfError::UnknownObject {
            object: reference.object,
        };
        let object = self
            .parts
            .get(&part)
            .and_then(|p| p.objects.get(&reference.object))
            .ok_or(unknown.clone())?;
        if !object.printable {
            *skipped += 1;
            return Ok(());
        }

        if mesh.triangles.len() + object.triangles.len() > MAX_TRIANGLES {
            return Err(ThreeMfError::TooLarge);
        }
        let offset = mesh.vertices.len() as u32;
        let count = object.vertices.len() as u32;
        let mirrored = determinant(&transform) < 0.0;
        for vertex in &object.vertices {
            let moved = apply(&transform, *vertex);
            if moved.iter().any(|c| !c.is_finite()) {
                return Err(ThreeMfError::NonFiniteCoordinate);
            }
            mesh.vertices.push(moved);
        }
        for &[a, b, c] in &object.triangles {
            if a >= count || b >= count || c >= count {
                return Err(ThreeMfError::InvalidIndex {
                    object: reference.object,
                });
            }
            if a == b || b == c || a == c {
                continue;
            }
            // A mirroring transform turns the solid inside out.
            let (b, c) = if mirrored { (c, b) } else { (b, c) };
            mesh.triangles.push([a + offset, b + offset, c + offset]);
        }

        // Components are indexed rather than borrowed, because loading the
        // parts they refer to changes the part table.
        let components = object.components.len();
        for index in 0..components {
            let component = self
                .parts
                .get(&part)
                .and_then(|p| p.objects.get(&reference.object))
                .map(|o| &o.components[index])
                .ok_or(unknown.clone())?;
            let component = Reference {
                path: component.path.clone(),
                object: component.object,
                transform: component.transform,
            };
            self.emit(&part, &component, &transform, depth + 1, skipped, mesh)?;
        }
        Ok(())
    }
}

/// Name of the part in which Bambu Studio lists the objects of each plate.
const PLATE_PART: &str = "Metadata/model_settings.config";

/// Reads the build plates of a Bambu Studio project: the number of each
/// plate and the identifiers of the objects on it. The layout was worked out
/// from project files; text that does not fit it yields no plates.
fn plates(text: &str) -> Vec<(u32, Vec<u32>)> {
    let mut plates: Vec<(u32, Vec<u32>)> = Vec::new();
    let (mut in_plate, mut in_instance) = (false, false);
    let mut reader = quick_xml::Reader::from_str(text);
    loop {
        match reader.read_event() {
            Ok(Event::Start(element) | Event::Empty(element)) => {
                match element.local_name().into_inner() {
                    "plate" => {
                        in_plate = true;
                        plates.push((0, Vec::new()));
                    }
                    "model_instance" => in_instance = true,
                    "metadata" if in_plate => {
                        let [key, value] = attributes(&element, ["key", "value"]);
                        let value = value.and_then(|v| v.trim().parse::<u32>().ok());
                        if let (Some(plate), Some(value)) = (plates.last_mut(), value) {
                            match key.as_deref() {
                                Some("plater_id") if !in_instance => plate.0 = value,
                                Some("object_id") if in_instance => plate.1.push(value),
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(element)) => match element.local_name().into_inner() {
                "plate" => in_plate = false,
                "model_instance" => in_instance = false,
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            Ok(_) => {}
        }
    }
    plates
}

/// Parses a 3MF package into a single mesh in millimetres.
///
/// All build items are placed as the file positions them and returned as one
/// mesh. Extensions the reader does not understand are reported as warnings.
pub fn parse_3mf(bytes: &[u8], diagnostics: &mut Diagnostics) -> Result<Mesh, ThreeMfError> {
    parse_3mf_plate(bytes, None, diagnostics)
}

/// Like [`parse_3mf`], but keeps only the objects on one build plate of a
/// project with several plates. Without a choice the first plate is used
/// and a warning says so.
pub fn parse_3mf_plate(
    bytes: &[u8],
    plate: Option<u32>,
    diagnostics: &mut Diagnostics,
) -> Result<Mesh, ThreeMfError> {
    let archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| ThreeMfError::Archive(e.to_string()))?;
    let mut package = Package {
        archive,
        parts: HashMap::new(),
    };
    let root = match package.read("_rels/.rels")? {
        Some(rels) => root_part(&rels).ok_or_else(|| ThreeMfError::MissingPart {
            part: DEFAULT_MODEL_PART.to_string(),
        })?,
        None => DEFAULT_MODEL_PART.to_string(),
    };
    let root = root.trim_start_matches('/').to_string();
    package.load(&root)?;

    let plates = package
        .read(PLATE_PART)?
        .map(|text| plates(&text))
        .unwrap_or_default();
    // A file with a single plate is read as a whole, so nothing is lost if
    // its plate list is incomplete.
    let chosen = match plate {
        Some(number) => match plates.iter().find(|(id, _)| *id == number) {
            Some(found) => Some(found),
            None if number == 1 && plates.is_empty() => None,
            None => {
                return Err(ThreeMfError::UnknownPlate {
                    plate: number,
                    available: plates.iter().map(|(id, _)| *id).collect(),
                });
            }
        },
        None if plates.len() > 1 => {
            diagnostics.push(
                Diagnostic::warning(
                    "model.multiple_plates",
                    format!(
                        "the file has {} build plates; only plate {} is sliced, choose another with the plate option",
                        plates.len(),
                        plates[0].0
                    ),
                )
                .with("plates", plates.len())
                .with("plate", plates[0].0),
            );
            plates.first()
        }
        None => None,
    };

    let mut mesh = Mesh::default();
    let mut skipped = 0;
    let items = package.parts.get(&root).map_or(0, |part| part.build.len());
    for index in 0..items {
        let Some(item) = package.parts.get(&root).map(|part| &part.build[index]) else {
            break;
        };
        if chosen.is_some_and(|(_, objects)| !objects.contains(&item.object)) {
            continue;
        }
        let item = Reference {
            path: item.path.clone(),
            object: item.object,
            transform: item.transform,
        };
        package.emit(&root, &item, &IDENTITY, 0, &mut skipped, &mut mesh)?;
    }
    if mesh.triangles.is_empty() {
        return Err(ThreeMfError::NoTriangles);
    }

    let mut extensions: Vec<&str> = package
        .parts
        .values()
        .flat_map(|part| part.extensions.iter().map(String::as_str))
        .collect();
    extensions.sort_unstable();
    extensions.dedup();
    if !extensions.is_empty() {
        diagnostics.push(
            Diagnostic::warning(
                "model.unsupported_extension",
                format!(
                    "the file requires {} 3MF extension(s) that are not supported and ignored: {}",
                    extensions.len(),
                    extensions.join(", ")
                ),
            )
            .with("extensions", extensions.join(" ")),
        );
    }
    if skipped > 0 {
        diagnostics.push(
            Diagnostic::warning(
                "model.skipped_objects",
                format!("{skipped} object(s) that are not of type 'model' were skipped"),
            )
            .with("objects", skipped),
        );
    }
    Ok(mesh)
}

/// Returns whether the data starts like a ZIP archive, as every 3MF does.
pub fn looks_like_3mf(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
}

/// Serializes a mesh as a 3MF package with a single object.
pub fn write_3mf(mesh: &Mesh) -> Result<Vec<u8>, ThreeMfError> {
    let mut model = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    model.push_str(&format!(
        "<model unit=\"millimeter\" xml:lang=\"en-US\" xmlns=\"{CORE_NS}\">\n <resources>\n  <object id=\"1\" type=\"model\">\n   <mesh>\n    <vertices>\n"
    ));
    for [x, y, z] in &mesh.vertices {
        model.push_str(&format!("     <vertex x=\"{x}\" y=\"{y}\" z=\"{z}\"/>\n"));
    }
    model.push_str("    </vertices>\n    <triangles>\n");
    for [a, b, c] in &mesh.triangles {
        model.push_str(&format!(
            "     <triangle v1=\"{a}\" v2=\"{b}\" v3=\"{c}\"/>\n"
        ));
    }
    model.push_str(
        "    </triangles>\n   </mesh>\n  </object>\n </resources>\n <build>\n  <item objectid=\"1\"/>\n </build>\n</model>\n",
    );
    let content_types = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\n <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\n <Default Extension=\"model\" ContentType=\"application/vnd.ms-package.3dmanufacturing-3dmodel+xml\"/>\n</Types>\n";
    let rels = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n <Relationship Target=\"/{DEFAULT_MODEL_PART}\" Id=\"rel0\" Type=\"{MODEL_RELATIONSHIP}\"/>\n</Relationships>\n"
    );
    write_package(&[
        ("[Content_Types].xml", content_types),
        ("_rels/.rels", &rels),
        (DEFAULT_MODEL_PART, &model),
    ])
}

/// Packs named parts into a ZIP archive.
fn write_package(parts: &[(&str, &str)]) -> Result<Vec<u8>, ThreeMfError> {
    let archive = |e: zip::result::ZipError| ThreeMfError::Archive(e.to_string());
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, content) in parts {
        writer.start_file(*name, options).map_err(archive)?;
        writer
            .write_all(content.as_bytes())
            .map_err(|e| ThreeMfError::Archive(e.to_string()))?;
    }
    Ok(writer.finish().map_err(archive)?.into_inner())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    const TYPES: &str =
        "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"/>";

    /// A tetrahedron with unit legs, as the content of an `<object>`.
    const TETRAHEDRON: &str = "<mesh><vertices><vertex x=\"0\" y=\"0\" z=\"0\"/><vertex x=\"1\" y=\"0\" z=\"0\"/><vertex x=\"0\" y=\"1\" z=\"0\"/><vertex x=\"0\" y=\"0\" z=\"1\"/></vertices><triangles><triangle v1=\"0\" v2=\"2\" v3=\"1\"/><triangle v1=\"0\" v2=\"1\" v3=\"3\"/><triangle v1=\"1\" v2=\"2\" v3=\"3\"/><triangle v1=\"2\" v2=\"0\" v3=\"3\"/></triangles></mesh>";

    fn model(attributes: &str, body: &str) -> String {
        format!("<?xml version=\"1.0\"?><model xmlns=\"{CORE_NS}\" {attributes}>{body}</model>")
    }

    fn package(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut all = vec![("[Content_Types].xml", TYPES)];
        all.extend_from_slice(parts);
        write_package(&all).unwrap()
    }

    fn parse(parts: &[(&str, &str)]) -> Result<(Mesh, Diagnostics), ThreeMfError> {
        let mut diagnostics = Diagnostics::new();
        parse_3mf(&package(parts), &mut diagnostics).map(|mesh| (mesh, diagnostics))
    }

    #[test]
    fn roundtrip() {
        let mesh = Mesh::cuboid(20.0, 10.0, 5.0);
        let bytes = write_3mf(&mesh).unwrap();
        assert!(looks_like_3mf(&bytes));
        let mut diagnostics = Diagnostics::new();
        let parsed = parse_3mf(&bytes, &mut diagnostics).unwrap();
        assert_eq!(parsed, mesh);
        assert!(diagnostics.is_empty());
        assert_eq!(write_3mf(&mesh).unwrap(), bytes);
    }

    #[test]
    fn units_items_and_transforms() {
        // Two items of the same object: one as is, one moved by 5 cm in x.
        let text = model(
            "unit=\"centimeter\"",
            &format!(
                "<resources><object id=\"7\">{TETRAHEDRON}</object></resources><build><item objectid=\"7\"/><item objectid=\"7\" transform=\"1 0 0 0 1 0 0 0 1 5 0 0\"/></build>"
            ),
        );
        let (mesh, diagnostics) = parse(&[("3D/3dmodel.model", &text)]).unwrap();
        assert!(diagnostics.is_empty());
        assert_eq!(mesh.triangles.len(), 8);
        assert_eq!(mesh.open_edge_count(), 0);
        let (lo, hi) = mesh.bounds().unwrap();
        assert_eq!(lo, [0.0, 0.0, 0.0]);
        assert_eq!(hi, [60.0, 10.0, 10.0]);
        // Each tetrahedron has 10 mm legs: 1000 / 6 mm³.
        assert!((mesh.signed_volume() - 2000.0 / 6.0).abs() < 1e-9);
    }

    #[test]
    fn nested_components_compose_transforms() {
        // Object 2 holds object 1 scaled by two; the item rotates it by a
        // quarter turn around z and lifts it by 3 mm.
        let text = model(
            "",
            &format!(
                "<resources><object id=\"1\">{TETRAHEDRON}</object><object id=\"2\"><components><component objectid=\"1\" transform=\"2 0 0 0 2 0 0 0 2 1 0 0\"/></components></object></resources><build><item objectid=\"2\" transform=\"0 1 0 -1 0 0 0 0 1 0 0 3\"/></build>"
            ),
        );
        let (mesh, _) = parse(&[("3D/3dmodel.model", &text)]).unwrap();
        let (lo, hi) = mesh.bounds().unwrap();
        // Before the item transform the part spans x 1..3, y 0..2, z 0..2.
        assert_eq!(lo, [-2.0, 1.0, 3.0]);
        assert_eq!(hi, [0.0, 3.0, 5.0]);
        assert!((mesh.signed_volume() - 8.0 / 6.0).abs() < 1e-9);
    }

    #[test]
    fn mirrored_objects_stay_outward_facing() {
        let text = model(
            "",
            &format!(
                "<resources><object id=\"1\">{TETRAHEDRON}</object></resources><build><item objectid=\"1\" transform=\"-1 0 0 0 1 0 0 0 1 0 0 0\"/></build>"
            ),
        );
        let (mesh, _) = parse(&[("3D/3dmodel.model", &text)]).unwrap();
        assert!(mesh.signed_volume() > 0.0);
    }

    #[test]
    fn components_in_other_parts_and_root_from_relationships() {
        let rels = format!(
            "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Target=\"/Main/root.model\" Id=\"r\" Type=\"{MODEL_RELATIONSHIP}\"/></Relationships>"
        );
        let root = model(
            &format!("xmlns:p=\"{PRODUCTION_NS}\""),
            "<resources><object id=\"1\"><components><component p:path=\"/3D/Objects/part.model\" objectid=\"4\"/></components></object></resources><build><item objectid=\"1\"/></build>",
        );
        let child = model(
            "unit=\"inch\"",
            &format!("<resources><object id=\"4\">{TETRAHEDRON}</object></resources>"),
        );
        let (mesh, diagnostics) = parse(&[
            ("_rels/.rels", &rels),
            ("Main/root.model", &root),
            ("3D/Objects/part.model", &child),
        ])
        .unwrap();
        assert!(diagnostics.is_empty());
        assert_eq!(mesh.bounds().unwrap().1, [25.4, 25.4, 25.4]);
    }

    #[test]
    fn one_plate_of_a_project_can_be_chosen() {
        let text = model(
            "",
            &format!(
                "<resources><object id=\"1\">{TETRAHEDRON}</object><object id=\"2\">{TETRAHEDRON}</object></resources><build><item objectid=\"1\"/><item objectid=\"2\" transform=\"1 0 0 0 1 0 0 0 1 300 0 0\"/></build>"
            ),
        );
        let plate = |id: u32, object: u32| {
            format!(
                "<plate><metadata key=\"plater_id\" value=\"{id}\"/><metadata key=\"locked\" value=\"false\"/><model_instance><metadata key=\"object_id\" value=\"{object}\"/><metadata key=\"instance_id\" value=\"0\"/></model_instance></plate>"
            )
        };
        let config = format!(
            "<config><object id=\"1\"/>{}{}</config>",
            plate(1, 1),
            plate(2, 2)
        );
        let bytes = package(&[("3D/3dmodel.model", &text), (PLATE_PART, &config)]);
        let read = |plate| {
            let mut diagnostics = Diagnostics::new();
            parse_3mf_plate(&bytes, plate, &mut diagnostics).map(|mesh| {
                let (min, max) = mesh.bounds().unwrap();
                (mesh.triangles.len(), min[0], max[0], diagnostics.codes())
            })
        };
        assert_eq!(read(None), Ok((4, 0.0, 1.0, vec!["model.multiple_plates"])));
        assert_eq!(read(Some(1)), Ok((4, 0.0, 1.0, vec![])));
        assert_eq!(read(Some(2)), Ok((4, 300.0, 301.0, vec![])));
        assert_eq!(
            read(Some(3)),
            Err(ThreeMfError::UnknownPlate {
                plate: 3,
                available: vec![1, 2]
            })
        );

        // Without plate information everything is one plate.
        let plain = package(&[("3D/3dmodel.model", &text)]);
        let mut diagnostics = Diagnostics::new();
        let whole = parse_3mf_plate(&plain, Some(1), &mut diagnostics).unwrap();
        assert_eq!(whole.triangles.len(), 8);
        assert!(parse_3mf_plate(&plain, Some(2), &mut diagnostics).is_err());
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn extensions_and_other_object_types_are_reported() {
        let text = model(
            "xmlns:v=\"http://example.com/vendor\" xmlns:m=\"http://example.com/materials\" xmlns:x=\"http://example.com/declared-only\" requiredextensions=\"m v\"",
            &format!(
                "<metadata name=\"Title\">x</metadata><resources><v:thing v:x=\"1\"><v:vertex x=\"9\"/></v:thing><object id=\"1\" v:colour=\"red\">{TETRAHEDRON}</object><object id=\"2\" type=\"support\">{TETRAHEDRON}</object></resources><build><item objectid=\"1\"/><item objectid=\"2\"/></build>"
            ),
        );
        let (mesh, diagnostics) = parse(&[("3D/3dmodel.model", &text)]).unwrap();
        assert_eq!(mesh.triangles.len(), 4);
        assert_eq!(
            diagnostics.codes(),
            ["model.unsupported_extension", "model.skipped_objects"]
        );
        assert_eq!(
            diagnostics.as_slice()[0].context["extensions"],
            "http://example.com/materials http://example.com/vendor"
        );
    }

    #[test]
    fn broken_files_give_errors() {
        let mut diagnostics = Diagnostics::new();
        assert!(matches!(
            parse_3mf(b"PK\x03\x04 not really", &mut diagnostics),
            Err(ThreeMfError::Archive(_))
        ));
        assert!(matches!(
            parse(&[("other.txt", "x")]),
            Err(ThreeMfError::MissingPart { .. })
        ));
        assert!(matches!(
            parse(&[("3D/3dmodel.model", "<model><oops></model>")]),
            Err(ThreeMfError::Xml { .. })
        ));

        let with = |attributes: &str, body: &str| {
            parse(&[("3D/3dmodel.model", &model(attributes, body))]).map(|_| ())
        };
        assert_eq!(
            with("unit=\"furlong\"", ""),
            Err(ThreeMfError::UnsupportedUnit {
                unit: "furlong".to_string()
            })
        );
        assert_eq!(
            with("", "<resources/><build/>"),
            Err(ThreeMfError::NoTriangles)
        );
        assert_eq!(
            with("", "<build><item objectid=\"3\"/></build>"),
            Err(ThreeMfError::UnknownObject { object: 3 })
        );
        assert_eq!(
            with(
                "",
                "<resources><object id=\"1\"><mesh><vertices><vertex x=\"0\" y=\"0\" z=\"0\"/></vertices><triangles><triangle v1=\"0\" v2=\"1\" v3=\"2\"/></triangles></mesh></object></resources><build><item objectid=\"1\"/></build>"
            ),
            Err(ThreeMfError::InvalidIndex { object: 1 })
        );
        assert!(matches!(
            with(
                "",
                "<resources><object id=\"1\"><mesh><vertices><vertex x=\"0\" y=\"zero\" z=\"0\"/></vertices></mesh></object></resources>"
            ),
            Err(ThreeMfError::InvalidAttribute {
                element: "vertex",
                attribute: "y",
                ..
            })
        ));
        assert!(matches!(
            with(
                "",
                "<resources/><build><item objectid=\"1\" transform=\"1 0 0\"/></build>"
            ),
            Err(ThreeMfError::InvalidAttribute {
                element: "item",
                attribute: "transform",
                ..
            })
        ));
        assert_eq!(
            with(
                "",
                &format!(
                    "<resources><object id=\"1\">{TETRAHEDRON}<components><component objectid=\"1\"/></components></object></resources><build><item objectid=\"1\"/></build>"
                )
            ),
            Err(ThreeMfError::ComponentCycle)
        );
        assert_eq!(
            with(
                "",
                &format!(
                    "<resources><object id=\"1\">{TETRAHEDRON}</object></resources><build><item objectid=\"1\" transform=\"1e308 0 0 0 1e308 0 0 0 1 1e308 0 0\"/></build>"
                )
            ),
            Err(ThreeMfError::NonFiniteCoordinate)
        );
    }
}
