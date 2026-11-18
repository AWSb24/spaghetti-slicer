//! A slice request and its outcome as bytes, for slicing in a Web Worker.
//!
//! The browser build slices in a worker so that the page keeps reacting
//! (see `worker.rs`); request and outcome cross over as byte arrays, which
//! the browser hands over without copying. Page and worker always come from
//! the same build, so the format needs no versions: little-endian numbers,
//! lengths as `u64` before their items, texts as UTF-8 bytes. The settings
//! travel as TOML, which gives every number back exactly.
//!
//! Every struct is taken apart completely when written, so a new field
//! fails to compile here until it is written and read as well.

use std::collections::BTreeSet;
use std::sync::{Mutex, PoisonError};

use slicer_core::{Diagnose, Diagnostic, Mesh, Severity};
use slicer_fdm::{Layer, Path, PathKind, Step, Toolpaths};
use slicer_gcode::{ExtruderStats, Output, Stats};
use slicer_profiles::Settings;

use crate::job::{SliceRequest, SliceResult};

/// The bytes do not hold what they should.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Malformed;

/// The error for bytes that cannot be read back.
fn malformed() -> Diagnostic {
    Diagnostic::error(
        "ui.slice_failed",
        "the slice could not be passed between the page and its worker",
    )
}

/// Writes `request` as bytes for [`decode_request`].
pub fn encode_request(request: &SliceRequest) -> Vec<u8> {
    let SliceRequest {
        objects,
        settings,
        object_settings,
    } = request;
    let mut out = Writer::default();
    out.text(&settings.to_toml());
    out.len(objects.len());
    for mesh in objects {
        write_mesh(&mut out, mesh);
    }
    out.len(object_settings.len());
    for own in object_settings {
        match own {
            Some(own) => {
                out.u8(1);
                out.text(&own.to_toml());
            }
            None => out.u8(0),
        }
    }
    out.0
}

/// Reads a request written by [`encode_request`].
pub fn decode_request(bytes: &[u8]) -> Result<SliceRequest, Diagnostic> {
    let mut input = Reader(bytes);
    let text = input.text().map_err(|_| malformed())?;
    // `to_toml` gives an empty text only if it failed; the defaults would
    // then be sliced silently.
    if text.is_empty() {
        return Err(malformed());
    }
    let settings = Settings::from_toml(&text).map_err(|e| e.diagnostic())?;
    let objects = input.list(read_mesh).map_err(|_| malformed())?;
    let object_settings = input
        .list(|input| match input.u8()? {
            0 => Ok(None),
            _ => {
                let text = input.text()?;
                Settings::from_toml(&text).map(Some).map_err(|_| Malformed)
            }
        })
        .and_then(|own| input.end().map(|()| own))
        .map_err(|_| malformed())?;
    Ok(SliceRequest {
        objects,
        settings,
        object_settings,
    })
}

/// Writes the outcome of a slice as bytes for [`decode_outcome`].
pub fn encode_outcome(outcome: &Result<SliceResult, Diagnostic>) -> Vec<u8> {
    let mut out = Writer::default();
    match outcome {
        Ok(result) => {
            let SliceResult {
                toolpaths,
                output,
                warnings,
            } = result;
            out.u8(0);
            write_toolpaths(&mut out, toolpaths);
            let Output { gcode, stats } = output;
            out.text(gcode);
            write_stats(&mut out, stats);
            out.len(warnings.len());
            for warning in warnings {
                write_diagnostic(&mut out, warning);
            }
        }
        Err(error) => {
            out.u8(1);
            write_diagnostic(&mut out, error);
        }
    }
    out.0
}

/// Reads an outcome written by [`encode_outcome`]. Bytes that cannot be
/// read give the `ui.slice_failed` error.
pub fn decode_outcome(bytes: &[u8]) -> Result<SliceResult, Diagnostic> {
    let mut input = Reader(bytes);
    let outcome = read_outcome(&mut input).and_then(|outcome| input.end().map(|()| outcome));
    outcome.unwrap_or_else(|_| Err(malformed()))
}

fn read_outcome(input: &mut Reader) -> Result<Result<SliceResult, Diagnostic>, Malformed> {
    if input.u8()? != 0 {
        return Ok(Err(read_diagnostic(input)?));
    }
    let toolpaths = read_toolpaths(input)?;
    let gcode = input.text()?;
    let stats = read_stats(input)?;
    let warnings = input.list(read_diagnostic)?;
    Ok(Ok(SliceResult {
        toolpaths,
        output: Output { gcode, stats },
        warnings,
    }))
}

/// The number that stands for `step` in progress messages.
pub fn step_code(step: Step) -> u8 {
    match step {
        Step::Preparing => 0,
        Step::Cutting => 1,
        Step::Geometry => 2,
        Step::Ordering => 3,
        Step::Gcode => 4,
        Step::Done => 5,
    }
}

/// The step for a number from [`step_code`].
pub fn step_from_code(code: u8) -> Option<Step> {
    [
        Step::Preparing,
        Step::Cutting,
        Step::Geometry,
        Step::Ordering,
        Step::Gcode,
        Step::Done,
    ]
    .into_iter()
    .find(|step| step_code(*step) == code)
}

/// Every path kind, for reading them back.
const KINDS: [PathKind; 13] = [
    PathKind::Skirt,
    PathKind::Brim,
    PathKind::Support,
    PathKind::OuterWall,
    PathKind::InnerWall,
    PathKind::GapFill,
    PathKind::SolidInfill,
    PathKind::Bridge,
    PathKind::SparseInfill,
    PathKind::PrimeTower,
    PathKind::Ironing,
    PathKind::OverhangWall,
    PathKind::OverhangWallFull,
];

fn kind_code(kind: PathKind) -> u8 {
    // A new kind must be added here and to `KINDS`.
    match kind {
        PathKind::Skirt => 0,
        PathKind::Brim => 1,
        PathKind::Support => 2,
        PathKind::OuterWall => 3,
        PathKind::InnerWall => 4,
        PathKind::GapFill => 5,
        PathKind::SolidInfill => 6,
        PathKind::Bridge => 7,
        PathKind::SparseInfill => 8,
        PathKind::PrimeTower => 9,
        PathKind::Ironing => 10,
        PathKind::OverhangWall => 11,
        PathKind::OverhangWallFull => 12,
    }
}

fn kind_from_code(code: u8) -> Result<PathKind, Malformed> {
    KINDS
        .into_iter()
        .find(|kind| kind_code(*kind) == code)
        .ok_or(Malformed)
}

/// Stands for "no kind" in the times of the statistics.
const NO_KIND: u8 = u8::MAX;

fn write_mesh(out: &mut Writer, mesh: &Mesh) {
    let Mesh {
        vertices,
        triangles,
        extruders,
    } = mesh;
    out.len(vertices.len());
    for vertex in vertices {
        vertex.iter().for_each(|&v| out.f64(v));
    }
    out.len(triangles.len());
    for triangle in triangles {
        triangle.iter().for_each(|&i| out.u32(i));
    }
    out.len(extruders.len());
    extruders.iter().for_each(|&e| out.u32(e));
}

fn read_mesh(input: &mut Reader) -> Result<Mesh, Malformed> {
    Ok(Mesh {
        vertices: input.list(|input| Ok([input.f64()?, input.f64()?, input.f64()?]))?,
        triangles: input.list(|input| Ok([input.u32()?, input.u32()?, input.u32()?]))?,
        extruders: input.list(Reader::u32)?,
    })
}

fn write_toolpaths(out: &mut Writer, toolpaths: &Toolpaths) {
    let Toolpaths { layers } = toolpaths;
    out.len(layers.len());
    for layer in layers {
        let Layer {
            z,
            height,
            paths,
            spiral,
        } = layer;
        out.f64(*z);
        out.f64(*height);
        out.u8(u8::from(*spiral));
        out.len(paths.len());
        for path in paths {
            write_path(out, path);
        }
    }
}

fn read_toolpaths(input: &mut Reader) -> Result<Toolpaths, Malformed> {
    let layers = input.list(|input| {
        Ok(Layer {
            z: input.f64()?,
            height: input.f64()?,
            spiral: input.u8()? != 0,
            paths: input.list(read_path)?,
        })
    })?;
    Ok(Toolpaths { layers })
}

fn write_path(out: &mut Writer, path: &Path) {
    let Path {
        kind,
        points,
        closed,
        travel_inside,
        extruder,
        object,
        width,
        travel,
    } = path;
    out.u8(kind_code(*kind));
    out.bool(*closed);
    out.bool(*travel_inside);
    out.u32(*extruder);
    out.u32(*object);
    out.bool(width.is_some());
    out.f64(width.unwrap_or(0.0));
    for list in [points, travel] {
        out.len(list.len());
        for point in list {
            out.f64(point[0]);
            out.f64(point[1]);
        }
    }
}

fn read_path(input: &mut Reader) -> Result<Path, Malformed> {
    let kind = kind_from_code(input.u8()?)?;
    let closed = input.bool()?;
    let travel_inside = input.bool()?;
    let extruder = input.u32()?;
    let object = input.u32()?;
    let has_width = input.bool()?;
    let width = input.f64()?;
    let points = input.list(|input| Ok([input.f64()?, input.f64()?]))?;
    let travel = input.list(|input| Ok([input.f64()?, input.f64()?]))?;
    Ok(Path {
        kind,
        points,
        closed,
        travel_inside,
        extruder,
        object,
        width: has_width.then_some(width),
        travel,
    })
}

fn write_stats(out: &mut Writer, stats: &Stats) {
    let Stats {
        layers,
        filament_length_mm,
        filament_volume_cm3,
        filament_weight_g,
        print_time_s,
        extruders,
        times,
    } = stats;
    out.len(*layers);
    for value in [
        filament_length_mm,
        filament_volume_cm3,
        filament_weight_g,
        print_time_s,
    ] {
        out.f64(*value);
    }
    out.len(extruders.len());
    for extruder in extruders {
        let ExtruderStats {
            extruder,
            filament_length_mm,
            filament_volume_cm3,
            filament_weight_g,
            print_time_s,
        } = extruder;
        out.u32(*extruder);
        for value in [
            filament_length_mm,
            filament_volume_cm3,
            filament_weight_g,
            print_time_s,
        ] {
            out.f64(*value);
        }
    }
    out.len(times.len());
    for (kind, seconds) in times {
        out.u8(kind.map_or(NO_KIND, kind_code));
        out.f64(*seconds);
    }
}

fn read_stats(input: &mut Reader) -> Result<Stats, Malformed> {
    Ok(Stats {
        layers: input.len()?,
        filament_length_mm: input.f64()?,
        filament_volume_cm3: input.f64()?,
        filament_weight_g: input.f64()?,
        print_time_s: input.f64()?,
        extruders: input.list(|input| {
            Ok(ExtruderStats {
                extruder: input.u32()?,
                filament_length_mm: input.f64()?,
                filament_volume_cm3: input.f64()?,
                filament_weight_g: input.f64()?,
                print_time_s: input.f64()?,
            })
        })?,
        times: input.list(|input| {
            let kind = match input.u8()? {
                NO_KIND => None,
                code => Some(kind_from_code(code)?),
            };
            Ok((kind, input.f64()?))
        })?,
    })
}

fn write_diagnostic(out: &mut Writer, diagnostic: &Diagnostic) {
    let Diagnostic {
        severity,
        code,
        message,
        context,
    } = diagnostic;
    out.bool(*severity == Severity::Error);
    out.text(code);
    out.text(message);
    out.len(context.len());
    for (key, value) in context {
        out.text(key);
        out.text(value);
    }
}

fn read_diagnostic(input: &mut Reader) -> Result<Diagnostic, Malformed> {
    let severity = if input.bool()? {
        Severity::Error
    } else {
        Severity::Warning
    };
    let code = intern(&input.text()?);
    let message = input.text()?;
    let context = input.list(|input| Ok((intern(&input.text()?), input.text()?)))?;
    Ok(Diagnostic {
        severity,
        code,
        message,
        context: context.into_iter().collect(),
    })
}

/// Codes and context keys read back so far. They are `&'static str` in a
/// [`Diagnostic`], so each distinct text is leaked once and kept here; there
/// are only a few dozen of them.
static NAMES: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());

/// `text` as a `&'static str`, leaking it the first time only.
fn intern(text: &str) -> &'static str {
    let mut names = NAMES.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(name) = names.get(text) {
        return name;
    }
    let name: &'static str = Box::leak(text.into());
    names.insert(name);
    name
}

/// Collects bytes.
#[derive(Default)]
struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, value: u8) {
        self.0.push(value);
    }

    fn bool(&mut self, value: bool) {
        self.u8(u8::from(value));
    }

    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    fn f64(&mut self, value: f64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    fn len(&mut self, len: usize) {
        // `usize` is at most 64 bits wide on every target.
        self.0.extend_from_slice(&(len as u64).to_le_bytes());
    }

    fn text(&mut self, text: &str) {
        self.len(text.len());
        self.0.extend_from_slice(text.as_bytes());
    }
}

/// Reads bytes from the front.
struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], Malformed> {
        let (head, rest) = self.0.split_first_chunk::<N>().ok_or(Malformed)?;
        self.0 = rest;
        Ok(*head)
    }

    fn u8(&mut self) -> Result<u8, Malformed> {
        Ok(self.take::<1>()?[0])
    }

    fn bool(&mut self) -> Result<bool, Malformed> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Malformed),
        }
    }

    fn u32(&mut self) -> Result<u32, Malformed> {
        Ok(u32::from_le_bytes(self.take()?))
    }

    fn f64(&mut self) -> Result<f64, Malformed> {
        Ok(f64::from_le_bytes(self.take()?))
    }

    fn len(&mut self) -> Result<usize, Malformed> {
        usize::try_from(u64::from_le_bytes(self.take()?)).map_err(|_| Malformed)
    }

    fn text(&mut self) -> Result<String, Malformed> {
        let len = self.len()?;
        if len > self.0.len() {
            return Err(Malformed);
        }
        let (text, rest) = self.0.split_at(len);
        self.0 = rest;
        String::from_utf8(text.to_vec()).map_err(|_| Malformed)
    }

    /// A length and that many items.
    fn list<T>(
        &mut self,
        mut item: impl FnMut(&mut Self) -> Result<T, Malformed>,
    ) -> Result<Vec<T>, Malformed> {
        let len = self.len()?;
        // Every item takes at least one byte; a wrong length must not
        // reserve memory for nothing.
        let mut items = Vec::with_capacity(len.min(self.0.len()));
        for _ in 0..len {
            items.push(item(self)?);
        }
        Ok(items)
    }

    /// Checks that nothing is left over.
    fn end(&self) -> Result<(), Malformed> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(Malformed)
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::job::slice;
    use slicer_fdm::Progress;
    use std::sync::atomic::AtomicBool;

    fn request() -> SliceRequest {
        let mut cube = Mesh::cuboid(10.0, 10.0, 2.0);
        cube.translate([105.0, 105.0, 0.0]);
        let mut second = Mesh::cuboid(5.0, 5.0, 3.0);
        second.translate([60.0, 60.0, 0.0]);
        second.set_extruder(2, 1);
        // 0.1 + 0.2 has no short decimal form; it must come back exactly.
        let settings = Settings {
            layer_height: 0.1 + 0.2,
            extruder_count: 2,
            ..Settings::default()
        };
        let own = Settings {
            infill_density: 100.0,
            ..settings.clone()
        };
        SliceRequest {
            objects: vec![cube, second],
            settings,
            object_settings: vec![None, Some(own)],
        }
    }

    #[test]
    fn request_round_trips() {
        let request = request();
        let bytes = encode_request(&request);
        let back = decode_request(&bytes).unwrap();
        assert_eq!(back.objects, request.objects);
        assert_eq!(back.settings, request.settings);
        assert_eq!(back.object_settings, request.object_settings);
        // Cut short or with bytes left over, it is rejected.
        assert!(decode_request(&bytes[..bytes.len() - 1]).is_err());
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(decode_request(&longer).is_err());
        assert!(decode_request(&[]).is_err());
    }

    #[test]
    fn outcome_round_trips() {
        let request = request();
        let result = slice(&request, &AtomicBool::new(false), &Progress::new()).unwrap();
        assert!(result.output.stats.extruders.len() == 2);
        assert!(!result.output.stats.times.is_empty());
        let mut result = result;
        result.warnings.push(
            Diagnostic::warning("mesh.repaired", "closed 2 holes")
                .with("holes", 2)
                .with("object", "cube"),
        );
        result.toolpaths.layers[0].paths[0].width = Some(0.37);
        let outcome = Ok(result.clone());
        let bytes = encode_outcome(&outcome);
        let back = decode_outcome(&bytes).unwrap();
        assert_eq!(back.toolpaths, result.toolpaths);
        assert_eq!(back.output, result.output);
        assert_eq!(back.warnings, result.warnings);

        let error = Diagnostic::error("slicing.cancelled", "slicing was cancelled");
        let back = decode_outcome(&encode_outcome(&Err(error.clone())));
        assert_eq!(back.unwrap_err(), error);

        let error = decode_outcome(&bytes[..bytes.len() / 2]).unwrap_err();
        assert_eq!(error.code, "ui.slice_failed");
    }

    #[test]
    fn codes_are_kept_once() {
        let first = intern("model.some_new_code");
        let second = intern(&String::from("model.some_new_code"));
        assert!(std::ptr::eq(first, second));
    }

    #[test]
    fn steps_and_kinds_have_codes() {
        for code in 0..=u8::MAX {
            if let Some(step) = step_from_code(code) {
                assert_eq!(step_code(step), code);
            }
        }
        assert_eq!(step_from_code(5), Some(Step::Done));
        assert_eq!(step_from_code(6), None);
        for (index, kind) in KINDS.into_iter().enumerate() {
            assert_eq!(usize::from(kind_code(kind)), index);
            assert_eq!(kind_from_code(kind_code(kind)), Ok(kind));
        }
        assert!(kind_from_code(NO_KIND).is_err());
    }
}
