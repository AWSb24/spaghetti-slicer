//! 3MF reader and writer for the core specification (geometry only).
//!
//! Reads meshes, components, build items and their transforms. Of the
//! production extension it follows component references into other model
//! parts of the same package. The materials of the triangles (base
//! materials of the core specification and colour groups of the materials
//! extension, through the `pid`, `pindex` and `p1` attributes) choose their
//! extruders: every material in use gets one, in the order the triangles
//! first use them (see [`parse_3mf`]). Everything else (textures, slices,
//! vendor data) is ignored, with one exception: project files of Bambu
//! Studio list which objects lie on which build plate, and the reader can
//! pick one plate; they also name the extruder of each object and of each
//! of its parts, which the mesh keeps for their triangles.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::io::{Cursor, Read, Write};

use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::reader::NsReader;
use slicer_core::{Diagnostic, Diagnostics, Mesh};

pub(crate) const CORE_NS: &str = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";
const PRODUCTION_NS: &str = "http://schemas.microsoft.com/3dmanufacturing/production/2015/06";
/// Namespace of the materials and properties extension, whose colour
/// groups are read.
const MATERIAL_NS: &str = "http://schemas.microsoft.com/3dmanufacturing/material/2015/02";
pub(crate) const MODEL_RELATIONSHIP: &str =
    "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel";
pub(crate) const DEFAULT_MODEL_PART: &str = "3D/3dmodel.model";

/// Largest model part that is unpacked, as a guard against archive bombs.
const MAX_PART_BYTES: u64 = 1 << 30;
/// Largest number of triangles a file may expand to through components.
const MAX_TRIANGLES: usize = 50_000_000;
/// Deepest allowed nesting of components.
const MAX_DEPTH: usize = 32;
/// Most materials that get an extruder of their own; the triangles of
/// further materials print with extruder 0 (`model.too_many_materials`).
/// Materials beyond the printer's `extruder_count` fall back to extruder 0
/// when slicing (`slicing.missing_extruder`).
pub const MAX_MATERIALS: usize = 64;
/// Longest material name kept for messages, in characters.
const MAX_MATERIAL_NAME: usize = 64;

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
pub(crate) type Transform = [f64; 12];

pub(crate) const IDENTITY: Transform = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];

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
#[derive(Clone)]
pub(crate) struct Reference {
    /// Model part holding the object; `None` means the referring part.
    pub(crate) path: Option<String>,
    pub(crate) object: u32,
    pub(crate) transform: Transform,
}

/// The property of a triangle as written: the property group (`pid`) and
/// the index into it (`p1`), each `None` where the triangle leaves it to
/// its object.
type TriangleProperty = (Option<u32>, Option<u32>);

#[derive(Default)]
pub(crate) struct Object {
    /// The object's `name` attribute.
    pub(crate) name: Option<String>,
    printable: bool,
    vertices: Vec<[f64; 3]>,
    triangles: Vec<[u32; 3]>,
    /// The property of every triangle, or empty if no triangle names one.
    properties: Vec<TriangleProperty>,
    /// The object's `pid` and `pindex`: the property of triangles that
    /// name none.
    pid: Option<u32>,
    pindex: Option<u32>,
    components: Vec<Reference>,
}

/// One entry of a property group that stands for a material: a `<base>`
/// of `<basematerials>` or a `<color>` of a `<colorgroup>`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Material {
    /// The name of a base material, shortened to [`MAX_MATERIAL_NAME`].
    name: Option<String>,
    /// The colour in red, green and blue (without the alpha the file may
    /// give), if it is well-formed.
    colour: Option<[u8; 3]>,
}

impl Material {
    /// How messages name the material: its name, its colour as `#RRGGBB`,
    /// both or neither.
    fn describe(&self) -> String {
        let colour = self
            .colour
            .map(|[r, g, b]| format!("#{r:02X}{g:02X}{b:02X}"));
        match (&self.name, colour) {
            (Some(name), Some(colour)) if !name.is_empty() => format!("{name} ({colour})"),
            (Some(name), None) if !name.is_empty() => name.clone(),
            (_, Some(colour)) => colour,
            _ => "unnamed".to_string(),
        }
    }
}

/// One parsed model part, already converted to millimetres.
#[derive(Default)]
pub(crate) struct Part {
    pub(crate) objects: HashMap<u32, Object>,
    pub(crate) build: Vec<Reference>,
    /// The property groups that stand for materials, by identifier.
    materials: HashMap<u32, Vec<Material>>,
    extensions: Vec<String>,
}

/// Red, green and blue of a `#RRGGBB` or `#RRGGBBAA` colour of a 3MF file.
fn colour(text: &str) -> Option<[u8; 3]> {
    let hex = text.trim().strip_prefix('#')?;
    if !(hex.len() == 6 || hex.len() == 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

/// Reads the attributes of one element into `slots`, matched by local name.
pub(crate) fn attributes<const N: usize>(
    element: &BytesStart<'_>,
    names: [&str; N],
) -> [Option<String>; N] {
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

    let number = |value: Option<String>| value.and_then(|v| v.trim().parse::<u32>().ok());
    let mut part = Part::default();
    let mut scale = 1.0;
    let mut current: Option<(u32, Object)> = None;
    // The property group being read: `<basematerials>` or `<colorgroup>`.
    let mut group: Option<(u32, Vec<Material>)> = None;
    let mut reader = NsReader::from_str(text);
    loop {
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|e| ThreeMfError::Xml {
                part: name.to_string(),
                message: e.to_string(),
            })?;
        let core = matches!(namespace, ResolveResult::Bound(Namespace(CORE_NS)));
        let material = matches!(namespace, ResolveResult::Bound(Namespace(MATERIAL_NS)));
        match event {
            Event::Eof => break,
            Event::End(element) if core && element.local_name().into_inner() == "object" => {
                if let Some((id, object)) = current.take() {
                    part.objects.insert(id, object);
                }
            }
            Event::End(element)
                if (core && element.local_name().into_inner() == "basematerials")
                    || (material && element.local_name().into_inner() == "colorgroup") =>
            {
                if let Some((id, entries)) = group.take() {
                    part.materials.insert(id, entries);
                }
            }
            // An empty group has no end.
            Event::Empty(element)
                if (core && element.local_name().into_inner() == "basematerials")
                    || (material && element.local_name().into_inner() == "colorgroup") =>
            {
                let [id] = attributes(&element, ["id"]);
                if let Some(id) = number(id) {
                    part.materials.insert(id, Vec::new());
                }
            }
            Event::Start(element)
                if (core && element.local_name().into_inner() == "basematerials")
                    || (material && element.local_name().into_inner() == "colorgroup") =>
            {
                let [id] = attributes(&element, ["id"]);
                group = number(id).map(|id| (id, Vec::new()));
            }
            Event::Start(element) | Event::Empty(element)
                if material && element.local_name().into_inner() == "color" =>
            {
                if let Some((_, entries)) = &mut group {
                    let [value] = attributes(&element, ["color"]);
                    entries.push(Material {
                        name: None,
                        colour: value.as_deref().and_then(colour),
                    });
                }
            }
            Event::Start(element) | Event::Empty(element)
                if core && element.local_name().into_inner() == "base" =>
            {
                if let Some((_, entries)) = &mut group {
                    let [name, value] = attributes(&element, ["name", "displaycolor"]);
                    entries.push(Material {
                        name: name.map(|n| n.trim().chars().take(MAX_MATERIAL_NAME).collect()),
                        colour: value.as_deref().and_then(colour),
                    });
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
                                && attribute.value != MATERIAL_NS
                            {
                                part.extensions.push(attribute.value.into_owned());
                            }
                        }
                    }
                    "object" => {
                        let [id, kind, name, pid, pindex] =
                            attributes(&element, ["id", "type", "name", "pid", "pindex"]);
                        let id = id
                            .and_then(|v| v.trim().parse().ok())
                            .ok_or_else(|| invalid("object", "id"))?;
                        let object = Object {
                            name,
                            printable: kind.is_none_or(|k| k == "model"),
                            pid: number(pid),
                            pindex: number(pindex),
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
                            let [v1, v2, v3, pid, p1] =
                                attributes(&element, ["v1", "v2", "v3", "pid", "p1"]);
                            for ((slot, value), corner) in triangle
                                .iter_mut()
                                .zip([v1, v2, v3])
                                .zip(["v1", "v2", "v3"])
                            {
                                *slot = value
                                    .and_then(|v| v.trim().parse().ok())
                                    .ok_or_else(|| invalid("triangle", corner))?;
                            }
                            // ponytail: a triangle whose corners have
                            // different properties (`p2`, `p3`) takes the
                            // first corner's; colour gradients are not
                            // printed.
                            let property = (number(pid), number(p1));
                            if property != (None, None) || !object.properties.is_empty() {
                                object
                                    .properties
                                    .resize(object.triangles.len(), (None, None));
                                object.properties.push(property);
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

/// An opened 3MF package and the model parts read from it so far.
pub(crate) struct Package<'a> {
    archive: zip::ZipArchive<Cursor<&'a [u8]>>,
    pub(crate) parts: HashMap<String, Part>,
}

impl<'a> Package<'a> {
    /// Opens a package and reads its root model part. Returns the package
    /// and the name of the root part.
    pub(crate) fn open(bytes: &'a [u8]) -> Result<(Self, String), ThreeMfError> {
        let archive = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|e| ThreeMfError::Archive(e.to_string()))?;
        let mut package = Package::from_archive(archive);
        let root = match package.read("_rels/.rels")? {
            Some(rels) => root_part(&rels).ok_or_else(|| ThreeMfError::MissingPart {
                part: DEFAULT_MODEL_PART.to_string(),
            })?,
            None => DEFAULT_MODEL_PART.to_string(),
        };
        let root = root.trim_start_matches('/').to_string();
        package.load(&root)?;
        Ok((package, root))
    }

    /// A package with no model part read yet.
    pub(crate) fn from_archive(archive: zip::ZipArchive<Cursor<&'a [u8]>>) -> Self {
        Package {
            archive,
            parts: HashMap::new(),
        }
    }

    /// Reads a part as text. Part names are case-insensitive and may be
    /// written with a leading slash.
    pub(crate) fn read(&mut self, name: &str) -> Result<Option<String>, ThreeMfError> {
        self.read_limited(name, MAX_PART_BYTES)
    }

    /// Like [`Package::read`], but fails with [`ThreeMfError::TooLarge`]
    /// for a part of more than `limit` bytes.
    pub(crate) fn read_limited(
        &mut self,
        name: &str,
        limit: u64,
    ) -> Result<Option<String>, ThreeMfError> {
        let wanted = name.trim_start_matches('/').to_string();
        let Some(bytes) = self.read_bytes(name, limit)? else {
            return Ok(None);
        };
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|e| ThreeMfError::Xml {
                part: wanted,
                message: e.to_string(),
            })
    }

    /// Reads a part as bytes, for parts that are not text such as meshes
    /// in an application's own folder; fails with
    /// [`ThreeMfError::TooLarge`] for a part of more than `limit` bytes.
    pub(crate) fn read_bytes(
        &mut self,
        name: &str,
        limit: u64,
    ) -> Result<Option<Vec<u8>>, ThreeMfError> {
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
        file.take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| ThreeMfError::Archive(e.to_string()))?;
        if bytes.len() as u64 > limit {
            return Err(ThreeMfError::TooLarge);
        }
        Ok(Some(bytes))
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

    /// Appends the object `reference` points to, with all its components
    /// except those of the objects in `top.leave_out`, and gives the new
    /// triangles their extruders as `top` says.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn emit(
        &mut self,
        part: &str,
        reference: &Reference,
        outer: &Transform,
        depth: usize,
        skipped: &mut usize,
        mesh: &mut Mesh,
        mut top: Emit<'_>,
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
        let holder = self.parts.get(&part).ok_or(unknown.clone())?;
        let object = holder
            .objects
            .get(&reference.object)
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
        let first = mesh.triangles.len();
        // The last material looked up, as most triangles in a row share one.
        let mut last: Option<((u32, u32), Option<u32>)> = None;
        for (index, &[a, b, c]) in object.triangles.iter().enumerate() {
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
            let Some(materials) = top.materials.as_deref_mut() else {
                continue;
            };
            let (pid, pindex) = object.properties.get(index).copied().unwrap_or_default();
            let Some(pid) = pid.or(object.pid) else {
                continue;
            };
            let key = (pid, pindex.or(object.pindex).unwrap_or(0));
            let extruder = match last {
                Some((known, extruder)) if known == key => extruder,
                _ => {
                    let extruder = materials.extruder(&part, &holder.materials, key);
                    last = Some((key, extruder));
                    extruder
                }
            };
            match extruder {
                None => materials.unknown += 1,
                Some(0) => {}
                Some(extruder) => {
                    mesh.extruders.resize(mesh.triangles.len() - 1, 0);
                    mesh.extruders.push(extruder);
                }
            }
        }
        if !mesh.extruders.is_empty() {
            mesh.extruders.resize(mesh.triangles.len(), 0);
        }
        if let Some(extruder) = top.extruder {
            mesh.set_extruder(first, extruder);
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
            if top.leave_out.contains(&component.object) {
                continue;
            }
            let first = mesh.triangles.len();
            let inner = Emit {
                materials: top.materials.as_deref_mut(),
                ..Emit::default()
            };
            self.emit(
                &part,
                &component,
                &transform,
                depth + 1,
                skipped,
                mesh,
                inner,
            )?;
            let own = top.parts.iter().find(|(id, _)| *id == component.object);
            if let Some(extruder) = own.map(|(_, extruder)| *extruder).or(top.extruder) {
                mesh.set_extruder(first, extruder);
            }
        }
        Ok(())
    }
}

/// What [`Package::emit`] does with the object it appends besides placing
/// it.
#[derive(Default)]
pub(crate) struct Emit<'a> {
    /// Components of the object (by the identifier of the object they
    /// refer to) that are left out.
    pub(crate) leave_out: &'a [u32],
    /// Extruder of the object, from 0, over those of its materials.
    pub(crate) extruder: Option<u32>,
    /// Extruders of components of the object (by the identifier of the
    /// object they refer to), from 0, over the object's.
    pub(crate) parts: &'a [(u32, u32)],
    /// Gives the materials of the triangles their extruders; without it
    /// they print with extruder 0.
    pub(crate) materials: Option<&'a mut Materials>,
}

/// The materials of the triangles read so far, each with its extruder:
/// a material gets the next extruder, from 0, when a triangle first uses
/// it, up to [`MAX_MATERIALS`] materials. A material is an entry of a
/// property group, so two entries of the same colour are two materials.
#[derive(Default)]
pub(crate) struct Materials {
    /// The extruder of every material in use, by model part, property
    /// group and index.
    extruders: BTreeMap<String, BTreeMap<(u32, u32), u32>>,
    /// The materials with extruders, in the order of their extruders.
    used: Vec<Material>,
    /// Materials in use beyond [`MAX_MATERIALS`], which print with
    /// extruder 0.
    beyond: usize,
    /// Triangles whose property is not a material: an entry of a texture
    /// or of another group that is not read, or one that does not exist.
    unknown: usize,
}

impl Materials {
    /// The extruder of entry `key.1` of property group `key.0` of the model
    /// part `part`, whose material groups are `groups`; `None` if it is not
    /// a material.
    fn extruder(
        &mut self,
        part: &str,
        groups: &HashMap<u32, Vec<Material>>,
        key: (u32, u32),
    ) -> Option<u32> {
        // ponytail: composite materials and multiproperties are not traced
        // back to their base materials, and textures not to a colour; their
        // triangles print as if they had no material.
        let material = groups.get(&key.0)?.get(key.1 as usize)?;
        if let Some(&extruder) = self.extruders.get(part).and_then(|known| known.get(&key)) {
            return Some(extruder);
        }
        let extruder = if self.used.len() < MAX_MATERIALS {
            self.used.push(material.clone());
            self.used.len() as u32 - 1
        } else {
            self.beyond += 1;
            0
        };
        self.extruders
            .entry(part.to_string())
            .or_default()
            .insert(key, extruder);
        Some(extruder)
    }

    /// Says which extruder prints which material, if there are several,
    /// and what was not given one.
    fn report(&self, diagnostics: &mut Diagnostics) {
        if self.used.len() > 1 {
            let list: Vec<String> = (self.used.iter().enumerate())
                .map(|(extruder, material)| format!("{} {}", extruder + 1, material.describe()))
                .collect();
            let colours: Vec<String> = (self.used.iter())
                .map(|material| match material.colour {
                    Some([r, g, b]) => format!("#{r:02X}{g:02X}{b:02X}"),
                    None => "-".to_string(),
                })
                .collect();
            diagnostics.push(
                Diagnostic::warning(
                    "model.materials",
                    format!(
                        "the model has {} materials; they print with extruders 1 to {} in the order the model first uses them ({}); the filament colours of the settings are not changed",
                        self.used.len(),
                        self.used.len(),
                        list.join(", ")
                    ),
                )
                .with("materials", self.used.len())
                .with("colours", colours.join(" ")),
            );
        }
        if self.beyond > 0 {
            diagnostics.push(
                Diagnostic::warning(
                    "model.too_many_materials",
                    format!(
                        "the model has {} materials; the first {MAX_MATERIALS} get extruders of their own, the other {} print with extruder 1",
                        self.used.len() + self.beyond,
                        self.beyond
                    ),
                )
                .with("materials", self.used.len() + self.beyond)
                .with("limit", MAX_MATERIALS),
            );
        }
        if self.unknown > 0 {
            diagnostics.push(
                Diagnostic::warning(
                    "model.unsupported_properties",
                    format!(
                        "{} triangle(s) have a property that is not a base material or a colour (a texture, a composite material or a missing one); they print as if they had none",
                        self.unknown
                    ),
                )
                .with("triangles", self.unknown),
            );
        }
    }
}

/// Name of the part in which Bambu Studio lists the objects of each plate.
pub(crate) const PLATE_PART: &str = "Metadata/model_settings.config";

/// A build plate: its number and the identifiers of the objects on it.
type Plate = (u32, Vec<u32>);

/// What a project file knows about the objects of its model: the build
/// plates, the extruder of every object that names one (from 0), the
/// extruder of every part of an object that names one (by object, the
/// identifier of the part and its extruder from 0), and the parts of every
/// object that are not printed (see [`HiddenPart`]).
#[derive(Default)]
struct ProjectLayout {
    plates: Vec<Plate>,
    extruders: HashMap<u32, u32>,
    part_extruders: HashMap<u32, Vec<(u32, u32)>>,
    hidden: HashMap<u32, Vec<HiddenPart>>,
}

/// A part of an object in a project of Orca Slicer or Bambu Studio that is
/// not printed: a modifier (`subtype="modifier_part"`), a negative volume
/// (`subtype="negative_part"`) or a support blocker or enforcer (any other
/// subtype than `normal_part`). The part's `id` is the identifier of the
/// object that the component of the object refers to, as the project files
/// seen so far have it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct HiddenPart {
    id: u32,
    modifier: bool,
    negative: bool,
    /// A support blocker or enforcer.
    support: Option<SupportPart>,
}

/// What a support part of a project does (`subtype="support_blocker"` or
/// `"support_enforcer"` of a `<part>` in `Metadata/model_settings.config`).
// ponytail: the subtype names follow the part types the user documentation
// of Orca Slicer and Bambu Studio names; no project file with them was seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportPart {
    /// No supports for the overhangs inside it.
    Blocker,
    /// Supports for the overhangs inside it.
    Enforcer,
}

/// Notes the part `element` of `object` in `hidden` if it is not printed
/// (see [`HiddenPart`]).
fn hide_part(
    element: &BytesStart,
    object: Option<u32>,
    hidden: &mut HashMap<u32, Vec<HiddenPart>>,
) {
    let [id, subtype] = attributes(element, ["id", "subtype"]);
    let id = id.and_then(|v| v.trim().parse::<u32>().ok());
    let subtype = subtype.unwrap_or_default();
    if let (Some(object), Some(id)) = (object, id)
        && !subtype.is_empty()
        && subtype != "normal_part"
    {
        hidden.entry(object).or_default().push(HiddenPart {
            id,
            modifier: subtype == "modifier_part",
            negative: subtype == "negative_part",
            support: match subtype.as_str() {
                "support_blocker" => Some(SupportPart::Blocker),
                "support_enforcer" => Some(SupportPart::Enforcer),
                _ => None,
            },
        });
    }
}

/// A build item of a project file read on its own (see
/// [`parse_3mf_plate_parts`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlateObject {
    /// Identifier of the object the item places.
    pub id: u32,
    /// The printed parts, placed as the file positions the item.
    pub mesh: Mesh,
    /// The modifier parts of the object, which are not printed, placed
    /// like the item: the identifier of every part (as
    /// `Metadata/model_settings.config` names it) with its mesh.
    pub modifiers: Vec<(u32, Mesh)>,
    /// The negative volumes of the object (parts with
    /// `subtype="negative_part"`), which are cut out of it, placed like
    /// the item.
    pub negatives: Vec<Mesh>,
    /// The support blockers and enforcers of the object, which are not
    /// printed, placed like the item.
    pub support_parts: Vec<(SupportPart, Mesh)>,
}

/// Reads the build plates of a Bambu Studio project, with the number of each
/// plate and the identifiers of the objects on it, and the extruder of each
/// object and of each of its parts that names one, counted from 0 (the file
/// counts from 1, and 0 leaves a part to its object's). The layout was
/// worked out from project files; text that does not fit it yields no
/// plates and no extruders.
// ponytail: the extruder of a part is read like that of its object, as a
// `<metadata key="extruder">` entry of the `<part>`; none of the project
// files seen so far gives a part an extruder of its own.
fn project(text: &str) -> ProjectLayout {
    let mut plates: Vec<Plate> = Vec::new();
    let mut extruders = HashMap::new();
    let mut part_extruders: HashMap<u32, Vec<(u32, u32)>> = HashMap::new();
    let mut hidden: HashMap<u32, Vec<HiddenPart>> = HashMap::new();
    let (mut in_plate, mut in_instance) = (false, false);
    let (mut object, mut in_part) = (None, false);
    // The identifier of the part being read.
    let mut part = None;
    let mut reader = quick_xml::Reader::from_str(text);
    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) if element.local_name().into_inner() == "object" => {
                let [id] = attributes(&element, ["id"]);
                object = id.and_then(|v| v.trim().parse::<u32>().ok());
            }
            Ok(Event::Start(element)) if element.local_name().into_inner() == "part" => {
                in_part = true;
                let [id] = attributes(&element, ["id"]);
                part = id.and_then(|v| v.trim().parse::<u32>().ok());
                hide_part(&element, object, &mut hidden);
            }
            // A part written as an empty element has no end.
            Ok(Event::Empty(element)) if element.local_name().into_inner() == "part" => {
                hide_part(&element, object, &mut hidden);
            }
            Ok(Event::Start(element) | Event::Empty(element)) => {
                match element.local_name().into_inner() {
                    "plate" => {
                        in_plate = true;
                        plates.push((0, Vec::new()));
                    }
                    "model_instance" => in_instance = true,
                    "metadata" if in_part => {
                        let [key, value] = attributes(&element, ["key", "value"]);
                        let value = value.and_then(|v| v.trim().parse::<u32>().ok());
                        if let (Some(id), Some(part), Some("extruder"), Some(number @ 1..)) =
                            (object, part, key.as_deref(), value)
                        {
                            part_extruders
                                .entry(id)
                                .or_default()
                                .push((part, number - 1));
                        }
                    }
                    "metadata" if object.is_some() => {
                        let [key, value] = attributes(&element, ["key", "value"]);
                        let value = value.and_then(|v| v.trim().parse::<u32>().ok());
                        if let (Some(id), Some("extruder"), Some(number @ 1..)) =
                            (object, key.as_deref(), value)
                        {
                            extruders.insert(id, number - 1);
                        }
                    }
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
                "object" => object = None,
                "part" => {
                    in_part = false;
                    part = None;
                }
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            Ok(_) => {}
        }
    }
    ProjectLayout {
        plates,
        extruders,
        part_extruders,
        hidden,
    }
}

/// Parses a 3MF package into a single mesh in millimetres.
///
/// All build items are placed as the file positions them and returned as one
/// mesh. Extensions the reader does not understand are reported as warnings.
///
/// Every material in use (an entry of a `<basematerials>` or
/// `<m:colorgroup>`, given to an object by `pid` and `pindex` or to a
/// triangle by `pid` and `p1`) gets an extruder of its own, from 0, in the
/// order the triangles of the printed build items first use them; triangles
/// without a material keep extruder 0, and so do those of materials beyond
/// [`MAX_MATERIALS`] (`model.too_many_materials`) or with a property that
/// is no material (`model.unsupported_properties`). Several materials are
/// reported as `model.materials` with their colours. The extruder a project
/// of Bambu Studio names for an object goes over those of its materials,
/// and the one it names for a part of the object over the object's.
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
    let mut objects = read_plate(bytes, plate, false, diagnostics)?;
    Ok(objects.pop().map(|object| object.mesh).unwrap_or_default())
}

/// Like [`parse_3mf_plate`], but every build item becomes a mesh of its
/// own, placed as the file positions it, together with the identifier of
/// the object it places. Several items may place the same object.
pub fn parse_3mf_plate_objects(
    bytes: &[u8],
    plate: Option<u32>,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<(u32, Mesh)>, ThreeMfError> {
    let objects = read_plate(bytes, plate, true, diagnostics)?;
    Ok(objects.into_iter().map(|o| (o.id, o.mesh)).collect())
}

/// Like [`parse_3mf_plate_objects`], with the modifier parts, the
/// negative volumes and the support blockers and enforcers of every object
/// of a project of Orca Slicer or Bambu Studio as meshes of their own.
/// Parts of a project that are not printed (modifiers, negative volumes,
/// support blockers and enforcers; see the `subtype` of the `<part>`
/// elements in `Metadata/model_settings.config`) are left out of the
/// printed meshes by every reader; the readers that give no parts report
/// negative volumes (`model.parts_ignored`, see [`has_negative_parts`]),
/// modifiers (`model.modifiers_not_printed`) and support blockers and
/// enforcers (`model.support_parts_not_applied`).
pub fn parse_3mf_plate_parts(
    bytes: &[u8],
    plate: Option<u32>,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<PlateObject>, ThreeMfError> {
    read_plate(bytes, plate, true, diagnostics)
}

/// Whether some object of a project of Orca Slicer or Bambu Studio has a
/// negative volume (a part with `subtype="negative_part"`), which only
/// [`parse_3mf_plate_parts`] gives; reads only the project's part list,
/// not the model. False for anything else, also for broken files.
pub fn has_negative_parts(bytes: &[u8]) -> bool {
    let Ok(archive) = zip::ZipArchive::new(Cursor::new(bytes)) else {
        return false;
    };
    let mut package = Package::from_archive(archive);
    let Ok(Some(text)) = package.read(PLATE_PART) else {
        return false;
    };
    project(&text)
        .hidden
        .values()
        .flatten()
        .any(|part| part.negative)
}

/// Reads the build items of one plate: all into a single mesh (with the
/// identifier 0), or with `split` each into its own with its modifiers and
/// negative volumes.
fn read_plate(
    bytes: &[u8],
    plate: Option<u32>,
    split: bool,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<PlateObject>, ThreeMfError> {
    let (mut package, root) = Package::open(bytes)?;

    let ProjectLayout {
        plates,
        extruders,
        part_extruders,
        hidden,
    } = package
        .read(PLATE_PART)?
        .map(|text| project(&text))
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
    let mut objects = Vec::new();
    let mut materials = Materials::default();
    let mut total = 0;
    let mut skipped = 0;
    // Negative volumes left out, and modifiers, support blockers and
    // enforcers left out without being read.
    let (mut ignored, mut unprinted, mut unapplied) = (0, 0, 0);
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
        let parts = hidden.get(&item.object).map_or(&[][..], Vec::as_slice);
        let leave_out: Vec<u32> = parts.iter().map(|part| part.id).collect();
        let top = Emit {
            leave_out: &leave_out,
            extruder: extruders.get(&item.object).copied(),
            parts: part_extruders
                .get(&item.object)
                .map_or(&[][..], Vec::as_slice),
            materials: Some(&mut materials),
        };
        package.emit(&root, &item, &IDENTITY, 0, &mut skipped, &mut mesh, top)?;
        // The parts left out: modifiers, negative volumes, support blockers
        // and enforcers as meshes of their own, placed like the item, the
        // others only counted.
        let (mut modifiers, mut negatives) = (Vec::new(), Vec::new());
        let mut support_parts = Vec::new();
        if !parts.is_empty() {
            let holder = item.path.as_deref().map_or(root.clone(), |path| {
                path.trim_start_matches('/').to_string()
            });
            let components: Vec<Reference> = package
                .parts
                .get(&holder)
                .and_then(|p| p.objects.get(&item.object))
                .map(|object| object.components.clone())
                .unwrap_or_default();
            let placed = then(&item.transform, &IDENTITY);
            for component in &components {
                let Some(part) = parts.iter().find(|part| part.id == component.object) else {
                    continue;
                };
                if let Some(kind) = part.support {
                    if split {
                        let mut volume = Mesh::default();
                        package.emit(
                            &holder,
                            component,
                            &placed,
                            1,
                            &mut skipped,
                            &mut volume,
                            Emit::default(),
                        )?;
                        total += volume.triangles.len();
                        if total > MAX_TRIANGLES {
                            return Err(ThreeMfError::TooLarge);
                        }
                        if !volume.triangles.is_empty() {
                            support_parts.push((kind, volume));
                        }
                    } else {
                        unapplied += 1;
                    }
                } else if !part.modifier && !(part.negative && split) {
                    ignored += 1;
                } else if split {
                    let mut volume = Mesh::default();
                    package.emit(
                        &holder,
                        component,
                        &placed,
                        1,
                        &mut skipped,
                        &mut volume,
                        Emit::default(),
                    )?;
                    total += volume.triangles.len();
                    if total > MAX_TRIANGLES {
                        return Err(ThreeMfError::TooLarge);
                    }
                    if !volume.triangles.is_empty() {
                        if part.modifier {
                            modifiers.push((part.id, volume));
                        } else {
                            negatives.push(volume);
                        }
                    }
                } else {
                    unprinted += 1;
                }
            }
        }
        if split && !mesh.triangles.is_empty() {
            total += mesh.triangles.len();
            if total > MAX_TRIANGLES {
                return Err(ThreeMfError::TooLarge);
            }
            objects.push(PlateObject {
                id: item.object,
                mesh: std::mem::take(&mut mesh),
                modifiers,
                support_parts,
                negatives,
            });
        }
    }
    if !split && !mesh.triangles.is_empty() {
        objects.push(PlateObject {
            id: 0,
            mesh,
            modifiers: Vec::new(),
            support_parts: Vec::new(),
            negatives: Vec::new(),
        });
    }
    if objects.is_empty() {
        return Err(ThreeMfError::NoTriangles);
    }
    materials.report(diagnostics);

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
    if ignored > 0 {
        diagnostics.push(
            Diagnostic::warning(
                "model.parts_ignored",
                format!(
                    "{ignored} negative part(s) of the project are not printed and have no effect"
                ),
            )
            .with("parts", ignored),
        );
    }
    if unprinted > 0 {
        diagnostics.push(
            Diagnostic::warning(
                "model.modifiers_not_printed",
                format!(
                    "{unprinted} modifier part(s) of the project are not printed; their settings count only with the project's settings"
                ),
            )
            .with("parts", unprinted),
        );
    }
    if unapplied > 0 {
        diagnostics.push(
            Diagnostic::warning(
                "model.support_parts_not_applied",
                format!(
                    "{unapplied} support blocker(s) or enforcer(s) of the project are not printed; they count only with the project's settings"
                ),
            )
            .with("parts", unapplied),
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
    Ok(objects)
}

/// Returns whether the data starts like a ZIP archive, as every 3MF does.
pub fn looks_like_3mf(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
}

/// Serializes a mesh as a 3MF package with a single object.
pub fn write_3mf(mesh: &Mesh) -> Result<Vec<u8>, ThreeMfError> {
    let mut model = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    model.push_str(&format!(
        "<model unit=\"millimeter\" xml:lang=\"en-US\" xmlns=\"{CORE_NS}\">\n <resources>\n  <object id=\"1\" type=\"model\">\n"
    ));
    push_mesh(&mut model, mesh);
    model.push_str(
        "  </object>\n </resources>\n <build>\n  <item objectid=\"1\"/>\n </build>\n</model>\n",
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

/// Appends the `<mesh>` element of an object to model XML.
pub(crate) fn push_mesh(model: &mut String, mesh: &Mesh) {
    model.push_str("   <mesh>\n    <vertices>\n");
    for [x, y, z] in &mesh.vertices {
        model.push_str(&format!("     <vertex x=\"{x}\" y=\"{y}\" z=\"{z}\"/>\n"));
    }
    model.push_str("    </vertices>\n    <triangles>\n");
    for [a, b, c] in &mesh.triangles {
        model.push_str(&format!(
            "     <triangle v1=\"{a}\" v2=\"{b}\" v3=\"{c}\"/>\n"
        ));
    }
    model.push_str("    </triangles>\n   </mesh>\n");
}

/// Packs named parts into a ZIP archive.
pub fn write_package(parts: &[(&str, &str)]) -> Result<Vec<u8>, ThreeMfError> {
    let parts: Vec<(&str, &[u8])> = parts
        .iter()
        .map(|&(name, content)| (name, content.as_bytes()))
        .collect();
    write_binary_package(&parts)
}

/// Packs named parts with any content into a ZIP archive. PNG images,
/// compressed already, are stored as they are; everything else is
/// compressed.
pub fn write_binary_package(parts: &[(&str, &[u8])]) -> Result<Vec<u8>, ThreeMfError> {
    let archive = |e: zip::result::ZipError| ThreeMfError::Archive(e.to_string());
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, content) in parts {
        let method = if name.ends_with(".png") {
            zip::CompressionMethod::Stored
        } else {
            zip::CompressionMethod::Deflated
        };
        let options = zip::write::SimpleFileOptions::default().compression_method(method);
        writer.start_file(*name, options).map_err(archive)?;
        writer
            .write_all(content)
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
    fn fuzz_seeds_are_valid_models() {
        let seed = include_str!("../../../fuzz/seeds/threemf_model/tetrahedron.model");
        let (mesh, _) = parse(&[("3D/3dmodel.model", seed)]).unwrap();
        assert_eq!(mesh.triangles.len(), 4);
        let seed = include_str!("../../../fuzz/seeds/threemf_model/materials.model");
        let (mesh, diagnostics) = parse(&[("3D/3dmodel.model", seed)]).unwrap();
        assert_eq!(extruders(&mesh), [0, 1, 2, 0]);
        assert_eq!(
            diagnostics.codes(),
            ["model.materials", "model.unsupported_properties"]
        );
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
    fn damaged_files_never_panic() {
        let root = model(
            &format!(
                "unit=\"inch\" xmlns:p=\"{PRODUCTION_NS}\" xmlns:m=\"{MATERIAL_NS}\" requiredextensions=\"p\""
            ),
            &format!(
                "<resources><basematerials id=\"8\"><base name=\"a\" displaycolor=\"#FF0000\"/><base name=\"b\" displaycolor=\"#00FF00\"/></basematerials><m:colorgroup id=\"9\"><m:color color=\"#0000FF\"/></m:colorgroup><object id=\"1\" pid=\"8\" pindex=\"1\">{}</object><object id=\"2\"><components><component objectid=\"1\" transform=\"1 0 0 0 1 0 0 0 -1 5 0 0\"/><component p:path=\"/3D/other.model\" objectid=\"7\"/></components></object></resources><build><item objectid=\"1\"/><item objectid=\"2\" transform=\"0 1 0 -1 0 0 0 0 1 300 0 0\"/></build>",
                tetrahedron_with(["", "p1=\"0\"", "pid=\"9\" p1=\"0\"", "pid=\"7\" p1=\"0\""])
            ),
        );
        let other = model(
            "",
            &format!("<resources><object id=\"7\">{TETRAHEDRON}</object></resources>"),
        );
        let config = "<config><plate><metadata key=\"plater_id\" value=\"1\"/><model_instance><metadata key=\"object_id\" value=\"1\"/></model_instance></plate><plate><metadata key=\"plater_id\" value=\"2\"/><model_instance><metadata key=\"object_id\" value=\"2\"/></model_instance></plate><object id=\"2\"><metadata key=\"extruder\" value=\"2\"/><part id=\"7\"><metadata key=\"extruder\" value=\"3\"/></part></object></config>";
        let parts = [root.as_str(), other.as_str(), config];
        let names = ["3D/3dmodel.model", "3D/other.model", PLATE_PART];
        let whole = package(&[
            (names[0], parts[0]),
            (names[1], parts[1]),
            (names[2], parts[2]),
        ]);

        let mut state = 0x9e37_79b9_7f4a_7c15;
        let mut accepted = 0;
        for round in 0..3000 {
            // Damage one part and pack it again, or damage the archive.
            let bytes = if round % 4 == 3 {
                crate::mutate(&whole, &mut state)
            } else {
                let target = round % 4;
                let damaged = crate::mutate(parts[target].as_bytes(), &mut state);
                let damaged = String::from_utf8_lossy(&damaged);
                let mut all = [
                    (names[0], parts[0]),
                    (names[1], parts[1]),
                    (names[2], parts[2]),
                ];
                all[target].1 = &damaged;
                package(&all)
            };
            let plate = [None, Some(1), Some(2)][round % 3];
            if let Ok(mesh) = parse_3mf_plate(&bytes, plate, &mut Diagnostics::new()) {
                accepted += 1;
                let count = mesh.vertices.len() as u32;
                assert!(mesh.triangles.iter().flatten().all(|&v| v < count));
                assert!(mesh.vertices.iter().flatten().all(|c| c.is_finite()));
                let extruders = mesh.extruders.len();
                assert!(extruders == 0 || extruders == mesh.triangles.len());
            }
        }
        assert!(accepted > 0);
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
    fn parts_of_a_project_that_are_not_printed_are_left_out() {
        // Object 10 has a body (object 1), a modifier (object 2, shifted by
        // 5 mm in X) and a negative part (object 3); the item moves it by
        // 20 mm in Y. Object 11 is a plain tetrahedron.
        let text = model(
            "",
            &format!(
                "<resources><object id=\"1\">{TETRAHEDRON}</object><object id=\"2\">{TETRAHEDRON}</object><object id=\"3\">{TETRAHEDRON}</object><object id=\"10\"><components><component objectid=\"1\"/><component objectid=\"2\" transform=\"1 0 0 0 1 0 0 0 1 5 0 0\"/><component objectid=\"3\"/></components></object><object id=\"11\">{TETRAHEDRON}</object></resources><build><item objectid=\"10\" transform=\"1 0 0 0 1 0 0 0 1 0 20 0\"/><item objectid=\"11\"/></build>"
            ),
        );
        let config = concat!(
            "<config><object id=\"10\"><metadata key=\"name\" value=\"Body\"/>",
            "<part id=\"1\" subtype=\"normal_part\"><metadata key=\"name\" value=\"a\"/></part>",
            "<part id=\"2\" subtype=\"modifier_part\"><metadata key=\"sparse_infill_density\" value=\"100%\"/></part>",
            "<part id=\"3\" subtype=\"negative_part\"/>",
            "</object></config>"
        );
        let bytes = package(&[("3D/3dmodel.model", &text), (PLATE_PART, config)]);
        // As one mesh: only the body and object 11 are printed.
        let mut diagnostics = Diagnostics::new();
        let mesh = parse_3mf(&bytes, &mut diagnostics).unwrap();
        assert_eq!(mesh.triangles.len(), 8);
        assert_eq!(
            diagnostics.codes(),
            ["model.parts_ignored", "model.modifiers_not_printed"]
        );
        // Split: the modifier and the negative part come with their
        // object, placed like it.
        assert!(has_negative_parts(&bytes));
        assert!(!has_negative_parts(b"not a package"));
        let mut diagnostics = Diagnostics::new();
        let objects = parse_3mf_plate_parts(&bytes, None, &mut diagnostics).unwrap();
        assert!(diagnostics.is_empty());
        assert_eq!(objects.len(), 2);
        assert_eq!(objects[0].id, 10);
        assert_eq!(objects[0].mesh.triangles.len(), 4);
        assert_eq!(objects[0].mesh.bounds().unwrap().0, [0.0, 20.0, 0.0]);
        let [(part, volume)] = objects[0].modifiers.as_slice() else {
            panic!("one modifier expected");
        };
        assert_eq!(*part, 2);
        assert_eq!(volume.bounds().unwrap().0, [5.0, 20.0, 0.0]);
        let [negative] = objects[0].negatives.as_slice() else {
            panic!("one negative part expected");
        };
        assert_eq!(negative.bounds().unwrap().0, [0.0, 20.0, 0.0]);
        assert!(objects[1].modifiers.is_empty() && objects[1].negatives.is_empty());
        let plain = parse_3mf_plate_objects(&bytes, None, &mut Diagnostics::new()).unwrap();
        assert_eq!(plain.len(), 2);
        assert_eq!(plain[0].1, objects[0].mesh);
    }

    #[test]
    fn support_blockers_and_enforcers_come_with_their_object() {
        // Object 10 has a body (object 1), a blocker (object 2, shifted by
        // 5 mm in X) and an enforcer (object 3); the item moves it by 20 mm
        // in Y.
        let text = model(
            "",
            &format!(
                "<resources><object id=\"1\">{TETRAHEDRON}</object><object id=\"2\">{TETRAHEDRON}</object><object id=\"3\">{TETRAHEDRON}</object><object id=\"10\"><components><component objectid=\"1\"/><component objectid=\"2\" transform=\"1 0 0 0 1 0 0 0 1 5 0 0\"/><component objectid=\"3\"/></components></object></resources><build><item objectid=\"10\" transform=\"1 0 0 0 1 0 0 0 1 0 20 0\"/></build>"
            ),
        );
        let config = concat!(
            "<config><object id=\"10\"><metadata key=\"name\" value=\"Body\"/>",
            "<part id=\"1\" subtype=\"normal_part\"/>",
            "<part id=\"2\" subtype=\"support_blocker\"/>",
            "<part id=\"3\" subtype=\"support_enforcer\"/>",
            "</object></config>"
        );
        let bytes = package(&[("3D/3dmodel.model", &text), (PLATE_PART, config)]);
        // As one mesh they are not printed, and say so.
        let mut diagnostics = Diagnostics::new();
        let mesh = parse_3mf(&bytes, &mut diagnostics).unwrap();
        assert_eq!(mesh.triangles.len(), 4);
        assert_eq!(diagnostics.codes(), ["model.support_parts_not_applied"]);
        // Split: they come with their object, placed like it.
        let mut diagnostics = Diagnostics::new();
        let objects = parse_3mf_plate_parts(&bytes, None, &mut diagnostics).unwrap();
        assert!(diagnostics.is_empty(), "{:?}", diagnostics.codes());
        let [object] = objects.as_slice() else {
            panic!("one object expected");
        };
        assert_eq!(object.mesh.triangles.len(), 4);
        assert!(object.modifiers.is_empty());
        let [(blocker, a), (enforcer, b)] = object.support_parts.as_slice() else {
            panic!("two support parts expected");
        };
        assert_eq!(
            (*blocker, *enforcer),
            (SupportPart::Blocker, SupportPart::Enforcer)
        );
        assert_eq!(a.bounds().unwrap().0, [5.0, 20.0, 0.0]);
        assert_eq!(b.bounds().unwrap().0, [0.0, 20.0, 0.0]);
    }

    #[test]
    fn objects_keep_the_extruder_the_project_names() {
        let text = model(
            "",
            &format!(
                "<resources><object id=\"1\">{TETRAHEDRON}</object><object id=\"2\">{TETRAHEDRON}</object><object id=\"3\">{TETRAHEDRON}</object></resources><build><item objectid=\"1\"/><item objectid=\"2\" transform=\"1 0 0 0 1 0 0 0 1 5 0 0\"/><item objectid=\"3\" transform=\"1 0 0 0 1 0 0 0 1 10 0 0\"/></build>"
            ),
        );
        // Object 1 says extruder 2, its part says 3 (but object 1 has no
        // components the part could be), object 2 says nothing, object 3
        // says 1.
        let config = "<config><object id=\"1\"><metadata key=\"name\" value=\"a\"/><metadata key=\"extruder\" value=\"2\"/><part id=\"1\"><metadata key=\"extruder\" value=\"3\"/></part></object><object id=\"3\"><metadata key=\"extruder\" value=\"1\"/></object></config>";
        let mut diagnostics = Diagnostics::new();
        let bytes = package(&[("3D/3dmodel.model", &text), (PLATE_PART, config)]);
        let mesh = parse_3mf(&bytes, &mut diagnostics).unwrap();
        let per_object: Vec<u32> = (0..3).map(|object| mesh.extruder(4 * object)).collect();
        assert_eq!(per_object, [1, 0, 0]);
        assert_eq!(mesh.extruders.len(), 12);
        assert_eq!(mesh.used_extruders(), [0, 1]);
        assert!(diagnostics.is_empty());

        // Split into objects, the same triangles in the same places.
        let objects = parse_3mf_plate_objects(&bytes, None, &mut diagnostics).unwrap();
        let ids: Vec<u32> = objects.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, [1, 2, 3]);
        let mut merged = Mesh::default();
        for (_, object) in &objects {
            assert_eq!(object.triangles.len(), 4);
            merged.merge(object);
        }
        assert_eq!(merged.vertices, mesh.vertices);
        assert_eq!(merged.triangles, mesh.triangles);
        assert_eq!(objects[0].1.used_extruders(), [1]);
        assert_eq!(objects[1].1.used_extruders(), [0]);
    }

    /// Extruders of the triangles of a mesh.
    fn extruders(mesh: &Mesh) -> Vec<u32> {
        (0..mesh.triangles.len())
            .map(|t| mesh.extruder(t))
            .collect()
    }

    #[test]
    fn parts_of_a_project_keep_their_own_extruders() {
        // Object 10 has three parts (objects 1 to 3) and a modifier
        // (object 4).
        let text = model(
            "",
            &format!(
                "<resources><object id=\"1\">{TETRAHEDRON}</object><object id=\"2\">{TETRAHEDRON}</object><object id=\"3\">{TETRAHEDRON}</object><object id=\"4\">{TETRAHEDRON}</object><object id=\"10\"><components><component objectid=\"1\"/><component objectid=\"2\" transform=\"1 0 0 0 1 0 0 0 1 5 0 0\"/><component objectid=\"3\" transform=\"1 0 0 0 1 0 0 0 1 10 0 0\"/><component objectid=\"4\"/></components></object></resources><build><item objectid=\"10\"/></build>"
            ),
        );
        let config = |object: &str| {
            format!(
                "<config><object id=\"10\"><metadata key=\"name\" value=\"Body\"/>{object}<part id=\"1\" subtype=\"normal_part\"><metadata key=\"name\" value=\"a\"/></part><part id=\"2\" subtype=\"normal_part\"><metadata key=\"extruder\" value=\"2\"/></part><part id=\"3\" subtype=\"normal_part\"><metadata key=\"extruder\" value=\"0\"/></part><part id=\"4\" subtype=\"modifier_part\"><metadata key=\"extruder\" value=\"4\"/></part></object></config>"
            )
        };
        let read = |object: &str| {
            let bytes = package(&[("3D/3dmodel.model", &text), (PLATE_PART, &config(object))]);
            let mut diagnostics = Diagnostics::new();
            let mesh = parse_3mf(&bytes, &mut diagnostics).unwrap();
            let objects = parse_3mf_plate_parts(&bytes, None, &mut diagnostics).unwrap();
            assert_eq!(objects.len(), 1);
            assert_eq!(objects[0].mesh, mesh);
            assert_eq!(objects[0].modifiers.len(), 1);
            mesh
        };
        // Part 2 prints with extruder 2; the others, part 3 with 0 too,
        // with their object's: the first by default, or the one it names.
        let mesh = read("");
        assert_eq!(mesh.triangles.len(), 12);
        assert_eq!(extruders(&mesh), [[0; 4], [1; 4], [0; 4]].concat());
        assert_eq!(mesh.used_extruders(), [0, 1]);
        let mesh = read("<metadata key=\"extruder\" value=\"3\"/>");
        assert_eq!(extruders(&mesh), [[2; 4], [1; 4], [2; 4]].concat());
    }

    /// A tetrahedron as in [`TETRAHEDRON`] whose triangles carry
    /// `properties`, written into each `<triangle>` element.
    fn tetrahedron_with(properties: [&str; 4]) -> String {
        let corners = [[0, 2, 1], [0, 1, 3], [1, 2, 3], [2, 0, 3]];
        let triangles: String = corners
            .iter()
            .zip(properties)
            .map(|([a, b, c], p)| format!("<triangle v1=\"{a}\" v2=\"{b}\" v3=\"{c}\" {p}/>"))
            .collect();
        format!(
            "<mesh><vertices><vertex x=\"0\" y=\"0\" z=\"0\"/><vertex x=\"1\" y=\"0\" z=\"0\"/><vertex x=\"0\" y=\"1\" z=\"0\"/><vertex x=\"0\" y=\"0\" z=\"1\"/></vertices><triangles>{triangles}</triangles></mesh>"
        )
    }

    #[test]
    fn materials_choose_extruders_in_the_order_of_first_use() {
        // Object 1 is green by default; its second triangle is red, its
        // third blue. Object 2 takes the second colour of a colour group,
        // object 3 the first, which no triangle uses before.
        let text = model(
            &format!("xmlns:m=\"{MATERIAL_NS}\" requiredextensions=\"m\""),
            &format!(
                "<resources><basematerials id=\"5\"><base name=\"Red PLA\" displaycolor=\"#FF0000\"/><base name=\"Green PLA\" displaycolor=\"#00ff00FF\"/><base name=\"Blue PLA\" displaycolor=\"blue\"/></basematerials><m:colorgroup id=\"6\"><m:color color=\"#FFFFFF\"/><m:color color=\"#000000\"/></m:colorgroup><object id=\"1\" pid=\"5\" pindex=\"1\">{}</object><object id=\"2\" pid=\"6\" pindex=\"1\">{TETRAHEDRON}</object><object id=\"3\" pid=\"6\" pindex=\"0\">{TETRAHEDRON}</object></resources><build><item objectid=\"1\"/><item objectid=\"2\" transform=\"1 0 0 0 1 0 0 0 1 5 0 0\"/><item objectid=\"3\" transform=\"1 0 0 0 1 0 0 0 1 10 0 0\"/></build>",
                tetrahedron_with([
                    "",
                    "pid=\"5\" p1=\"0\" p2=\"1\" p3=\"1\"",
                    "p1=\"2\"",
                    "pid=\"5\" p1=\"1\""
                ])
            ),
        );
        let (mesh, diagnostics) = parse(&[("3D/3dmodel.model", &text)]).unwrap();
        assert_eq!(extruders(&mesh), [0, 1, 2, 0, 3, 3, 3, 3, 4, 4, 4, 4]);
        // The material extension is understood, so not reported.
        assert_eq!(diagnostics.codes(), ["model.materials"]);
        let report = &diagnostics.as_slice()[0];
        assert_eq!(report.context["materials"], "5");
        assert_eq!(
            report.context["colours"],
            "#00FF00 #FF0000 - #000000 #FFFFFF"
        );
        assert!(
            report
                .message
                .contains("1 Green PLA (#00FF00), 2 Red PLA (#FF0000), 3 Blue PLA, 4 #000000")
        );

        // Split, the objects keep the extruders of the whole.
        let objects = parse_3mf_plate_objects(
            &package(&[("3D/3dmodel.model", &text)]),
            None,
            &mut Diagnostics::new(),
        )
        .unwrap();
        let split: Vec<Vec<u32>> = objects.iter().map(|(_, mesh)| extruders(mesh)).collect();
        assert_eq!(split, [vec![0, 1, 2, 0], vec![3; 4], vec![4; 4]]);
    }

    #[test]
    fn one_material_or_none_changes_nothing() {
        // As some CAD programs write it: one colour for the whole object.
        let text = model(
            &format!("xmlns:m=\"{MATERIAL_NS}\""),
            &format!(
                "<resources><m:colorgroup id=\"2\"><m:color color=\"#F5F5F6FF\"/><m:color color=\"#F5F5F6FF\"/></m:colorgroup><m:colorgroup id=\"3\"/><object id=\"1\" pid=\"2\" pindex=\"1\">{TETRAHEDRON}</object></resources><build><item objectid=\"1\"/></build>"
            ),
        );
        let plain = model(
            "",
            &format!(
                "<resources><object id=\"1\">{TETRAHEDRON}</object></resources><build><item objectid=\"1\"/></build>"
            ),
        );
        let (mesh, diagnostics) = parse(&[("3D/3dmodel.model", &text)]).unwrap();
        let (without, _) = parse(&[("3D/3dmodel.model", &plain)]).unwrap();
        assert_eq!(mesh, without);
        assert!(mesh.extruders.is_empty());
        assert!(diagnostics.is_empty(), "{:?}", diagnostics.codes());
    }

    #[test]
    fn too_many_materials_fall_back_to_extruder_0() {
        // 66 objects, each with a colour of its own.
        let count = MAX_MATERIALS + 2;
        let colours: String = (0..count)
            .map(|i| format!("<m:color color=\"#{i:06X}\"/>"))
            .collect();
        let objects: String = (0..count)
            .map(|i| {
                format!(
                    "<object id=\"{}\" pid=\"1\" pindex=\"{i}\">{TETRAHEDRON}</object>",
                    i + 2
                )
            })
            .collect();
        let items: String = (0..count)
            .map(|i| {
                format!(
                    "<item objectid=\"{}\" transform=\"1 0 0 0 1 0 0 0 1 {} 0 0\"/>",
                    i + 2,
                    2 * i
                )
            })
            .collect();
        // The second triangle of every object names a group that does not
        // exist.
        let objects = objects.replacen(
            "<triangle v1=\"0\" v2=\"1\" v3=\"3\"/>",
            "<triangle v1=\"0\" v2=\"1\" v3=\"3\" pid=\"99\" p1=\"0\"/>",
            count,
        );
        let text = model(
            &format!("xmlns:m=\"{MATERIAL_NS}\""),
            &format!(
                "<resources><m:colorgroup id=\"1\">{colours}</m:colorgroup>{objects}</resources><build>{items}</build>"
            ),
        );
        let (mesh, diagnostics) = parse(&[("3D/3dmodel.model", &text)]).unwrap();
        let per_object: Vec<u32> = (0..count).map(|o| mesh.extruder(4 * o)).collect();
        let expected: Vec<u32> = (0..MAX_MATERIALS as u32).chain([0, 0]).collect();
        assert_eq!(per_object, expected);
        assert_eq!(mesh.extruder(1), 0);
        assert_eq!(
            diagnostics.codes(),
            [
                "model.materials",
                "model.too_many_materials",
                "model.unsupported_properties"
            ]
        );
        assert_eq!(diagnostics.as_slice()[1].context["materials"], "66");
        assert_eq!(
            diagnostics.as_slice()[2].context["triangles"],
            count.to_string()
        );
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
