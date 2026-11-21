//! Slicing in the background, and saving the result.
//!
//! On the desktop a slice runs in a thread of its own; in the browser, which
//! has no threads, in a Web Worker (see `worker.rs`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use web_time::Instant;

use slicer_core::{Diagnose, Diagnostic, Diagnostics, Mesh};
use slicer_fdm::{Progress, Step, Toolpaths};
use slicer_gcode::Output;
use slicer_profiles::Settings;

use crate::plate::toolpath_bounds;

/// Everything a slice needs. Owned, so it can move to another thread.
#[derive(Debug, Clone)]
pub struct SliceRequest {
    /// The models, rotated, scaled and placed in bed coordinates.
    pub objects: Vec<Mesh>,
    /// Settings to slice with.
    pub settings: Settings,
    /// The settings of every object of its own, in the order of `objects`
    /// (see [`crate::plate::Plate::object_settings`]); `None` or missing:
    /// the object prints with `settings`.
    pub object_settings: Vec<Option<Settings>>,
}

/// A finished slice.
#[derive(Debug, Clone)]
pub struct SliceResult {
    /// Paths of every layer, in bed coordinates.
    pub toolpaths: Toolpaths,
    /// The G-code and its statistics.
    pub output: Output,
    /// Warnings about the mesh and the print.
    pub warnings: Vec<Diagnostic>,
}

/// Slices `request` in the calling thread. Gives up with the
/// `slicing.cancelled` error soon after `stop` is set. `progress` follows
/// the steps.
pub fn slice(
    request: &SliceRequest,
    stop: &AtomicBool,
    progress: &Progress,
) -> Result<SliceResult, Diagnostic> {
    let settings = &request.settings;
    let mut warnings = Diagnostics::new();
    let toolpaths = slicer_fdm::generate_plate_with_object_settings(
        &request.objects,
        &request.object_settings,
        slicer_fdm::Placement::AsGiven,
        settings,
        &mut warnings,
        stop,
        progress,
    )
    .map_err(|e| e.diagnostic())?;
    let mut warnings = warnings.as_slice().to_vec();
    if let Some((lo, hi)) = toolpath_bounds(&toolpaths) {
        let bed = [settings.bed_size_x, settings.bed_size_y];
        if (0..2).any(|a| lo[a] < 0.0 || hi[a] > bed[a]) {
            warnings.push(Diagnostic::warning(
                "ui.paths_off_bed",
                "skirt or brim reach over the edge of the bed; move the model inwards",
            ));
        }
    }
    if stop.load(Ordering::Relaxed) {
        return Err(slicer_fdm::FdmError::Cancelled.diagnostic());
    }
    let mut output = slicer_gcode::write_gcode_with_progress(&toolpaths, settings, progress)
        .map_err(|e| e.diagnostic())?;
    slicer_gcode::thumbnails::add_thumbnails(
        &mut output.gcode,
        &request.objects,
        slicer_fdm::Placement::AsGiven,
        settings,
    )
    .map_err(|e| Diagnostic::error("thumbnail.encode", e.to_string()))?;
    Ok(SliceResult {
        toolpaths,
        output,
        warnings,
    })
}

/// A slice running in the background: in a thread on the desktop, in a
/// Web Worker in the browser.
///
/// Dropping the job asks the slice to stop; its result is then discarded.
#[derive(Debug)]
pub struct SliceJob {
    stop: Arc<AtomicBool>,
    progress: Arc<Progress>,
    started: Instant,
    result: Receiver<Result<SliceResult, Diagnostic>>,
    /// The worker that slices; it posts progress and the result back.
    #[cfg(target_arch = "wasm32")]
    worker: crate::worker::WorkerJob,
}

impl SliceJob {
    /// Starts slicing. `notify` is called when the result is ready, for
    /// example to wake up the window: on the desktop from the background
    /// thread.
    pub fn start(request: SliceRequest, notify: impl FnOnce() + Send + 'static) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Progress::new());
        let (sender, result) = mpsc::channel();
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (thread_stop, thread_progress) = (Arc::clone(&stop), Arc::clone(&progress));
            std::thread::spawn(move || {
                let outcome = slice(&request, &thread_stop, &thread_progress);
                // The receiver is gone if the job was dropped; nobody waits
                // then.
                let _ = sender.send(outcome);
                notify();
            });
        }
        #[cfg(target_arch = "wasm32")]
        let worker =
            crate::worker::WorkerJob::start(&request, sender, Arc::clone(&progress), notify);
        Self {
            stop,
            progress,
            started: Instant::now(),
            result,
            #[cfg(target_arch = "wasm32")]
            worker,
        }
    }

    /// Asks the slice to stop. The result then is the `slicing.cancelled`
    /// error, or the finished slice if it was already done. In the browser
    /// the worker is stopped at once.
    pub fn cancel(&self) {
        self.stop.store(true, Ordering::Relaxed);
        #[cfg(target_arch = "wasm32")]
        self.worker.cancel();
    }

    /// Whether [`SliceJob::cancel`] was called.
    pub fn is_cancelling(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// The step the slice is in and the share of the whole that is done,
    /// from 0 to 1.
    pub fn progress(&self) -> (Step, f64) {
        (self.progress.step(), self.progress.fraction())
    }

    /// Time since the slice started.
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// The result once the slice has ended, `None` while it runs.
    pub fn try_result(&self) -> Option<Result<SliceResult, Diagnostic>> {
        match self.result.try_recv() {
            Ok(outcome) => Some(outcome),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(Diagnostic::error(
                "ui.slice_failed",
                "slicing stopped unexpectedly",
            ))),
        }
    }

    /// Waits for the result. For tests and command line use.
    pub fn wait(self) -> Result<SliceResult, Diagnostic> {
        self.result.recv().unwrap_or_else(|_| {
            Err(Diagnostic::error(
                "ui.slice_failed",
                "slicing stopped unexpectedly",
            ))
        })
    }
}

impl Drop for SliceJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Whether a file name asks for the `.gcode.3mf` container of Bambu Lab
/// printers instead of plain G-code.
pub fn is_gcode_3mf(name: &str) -> bool {
    name.to_lowercase().ends_with(".gcode.3mf")
}

/// The file content to save for `name`: plain G-code, or the G-code in a
/// `.gcode.3mf` container if the name ends that way.
pub fn encode_output(
    output: &Output,
    settings: &Settings,
    name: &str,
) -> Result<Vec<u8>, Diagnostic> {
    if !is_gcode_3mf(name) {
        return Ok(output.gcode.clone().into_bytes());
    }
    let info = slicer_formats::PrintInfo {
        seconds: output.stats.print_time_s,
        weight_g: output.stats.filament_weight_g,
        filament_m: output.stats.filament_length_mm / 1000.0,
        layers: output.stats.layers,
        nozzle_diameter: settings.nozzle_diameter,
        filaments: slicer_gcode::filaments(&output.stats, settings),
        layer_filaments: output.stats.layer_filaments.clone(),
        // ponytail: no picture of the plate here, the meshes are not at
        // hand; the command line tool and the server add one.
        ..slicer_formats::PrintInfo::default()
    };
    slicer_formats::write_gcode_3mf(&output.gcode, &info).map_err(|e| e.diagnostic())
}

/// Suggested name of the G-code file for a model file: its stem with the
/// new extension, e.g. `benchy.stl` becomes `benchy.gcode`.
pub fn output_name(model_name: &str, gcode_3mf: bool) -> String {
    let lower = model_name.to_lowercase();
    let stem = [".gcode.3mf", ".3mf", ".stl"]
        .iter()
        .find(|ext| lower.ends_with(*ext))
        .map_or(model_name, |ext| {
            &model_name[..model_name.len() - ext.len()]
        });
    let stem = if stem.is_empty() { "model" } else { stem };
    let extension = if gcode_3mf { "gcode.3mf" } else { "gcode" };
    format!("{stem}.{extension}")
}

/// `name` with the extension for the chosen format added unless it is
/// there already. File dialogs do not always add it.
pub fn with_output_extension(name: &str, gcode_3mf: bool) -> String {
    let lower = name.to_lowercase();
    match (gcode_3mf, lower.ends_with(".gcode"), is_gcode_3mf(&lower)) {
        (true, _, true) | (false, true, _) => name.to_owned(),
        (true, true, _) => format!("{name}.3mf"),
        (true, false, _) => format!("{name}.gcode.3mf"),
        (false, false, _) => format!("{name}.gcode"),
    }
}

/// A duration in seconds as `1h 02m 03s`.
pub fn format_duration(seconds: f64) -> String {
    let seconds = seconds.max(0.0).round() as u64;
    format!(
        "{}h {:02}m {:02}s",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// A 10 mm cube centred at `center`.
    fn cube_at(center: [f64; 2]) -> Mesh {
        let mut mesh = Mesh::cuboid(10.0, 10.0, 2.0);
        mesh.translate([center[0] - 5.0, center[1] - 5.0, 0.0]);
        mesh
    }

    fn request() -> SliceRequest {
        SliceRequest {
            objects: vec![cube_at([60.0, 130.0])],
            settings: Settings::default(),
            object_settings: Vec::new(),
        }
    }

    #[test]
    fn background_slice_delivers_gcode() {
        let (tx, rx) = mpsc::channel();
        let job = SliceJob::start(request(), move || {
            let _ = tx.send(());
        });
        rx.recv_timeout(Duration::from_secs(60)).unwrap();
        let result = job.try_result().unwrap().unwrap();
        assert_eq!(result.output.stats.layers, 10);
        assert_eq!(result.toolpaths.layers.len(), 10);
        assert!(result.output.gcode.contains(";LAYER_CHANGE"));
        // The model sits 50 mm left of and 20 mm behind the bed centre.
        let (lo, hi) = toolpath_bounds(&result.toolpaths).unwrap();
        let center = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
        assert!((center[0] - 60.0).abs() < 0.01 && (center[1] - 130.0).abs() < 0.01);
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn thumbnails_leave_the_layers_of_the_gcode_view() {
        let plain = SliceJob::start(request(), || {}).wait().unwrap();
        let mut pictured = request();
        pictured.settings.thumbnails = "32x32".into();
        let pictured = SliceJob::start(pictured, || {}).wait().unwrap();
        let gcode = &pictured.output.gcode;
        assert!(gcode.contains("\n; thumbnail begin 32x32 "));
        assert_eq!(pictured.output.stats, plain.output.stats);
        let (before, after) = (
            crate::view::GcodeIndex::new(&plain.output.gcode),
            crate::view::GcodeIndex::new(gcode),
        );
        let lines = |index: &crate::view::GcodeIndex, layer| index.layer(layer).len();
        for layer in 0..10 {
            assert_eq!(lines(&before, layer), lines(&after, layer), "layer {layer}");
            let first = after.layer(layer).start;
            assert_eq!(after.line(gcode, first), ";LAYER_CHANGE");
        }
        assert!(after.layer(10).is_empty());
    }

    #[test]
    fn same_gcode_as_the_command_line_when_centred() {
        let mut centred = request();
        centred.objects = vec![cube_at([110.0, 110.0])];
        let job = SliceJob::start(centred.clone(), || {});
        let progress = Arc::clone(&job.progress);
        let result = job.wait().unwrap();
        // The progress bar of the window ends full.
        assert_eq!((progress.step(), progress.fraction()), (Step::Done, 1.0));
        let cube = Mesh::cuboid(10.0, 10.0, 2.0);
        let toolpaths = slicer_fdm::generate(&cube, &centred.settings).unwrap();
        let direct = slicer_gcode::write_gcode(&toolpaths, &centred.settings).unwrap();
        assert_eq!(result.output.gcode, direct.gcode);
    }

    #[test]
    fn objects_slice_with_their_own_settings() {
        let mut two = request();
        two.objects.push(cube_at([160.0, 130.0]));
        let plain = SliceJob::start(two.clone(), || {}).wait().unwrap();
        let solid = Settings {
            infill_density: 100.0,
            ..Settings::default()
        };
        two.object_settings = vec![None, Some(solid)];
        let own = SliceJob::start(two, || {}).wait().unwrap();
        let sparse = |result: &SliceResult, x: std::ops::Range<f64>| {
            result.toolpaths.layers.iter().any(|layer| {
                layer.paths.iter().any(|p| {
                    p.kind == slicer_fdm::PathKind::SparseInfill && x.contains(&p.points[0][0])
                })
            })
        };
        assert!(sparse(&plain, 100.0..200.0) && sparse(&own, 0.0..100.0));
        assert!(!sparse(&own, 100.0..200.0));
    }

    #[test]
    fn cancelled_slice_reports_cancellation() {
        let stop = AtomicBool::new(true);
        let error = slice(&request(), &stop, &Progress::new()).unwrap_err();
        assert_eq!(error.code, "slicing.cancelled");

        let job = SliceJob::start(request(), || {});
        job.cancel();
        assert!(job.is_cancelling());
        // Either it noticed in time or it was already done.
        match job.wait() {
            Ok(result) => assert_eq!(result.output.stats.layers, 10),
            Err(error) => assert_eq!(error.code, "slicing.cancelled"),
        }
    }

    #[test]
    fn paths_over_the_edge_are_reported() {
        let mut near_edge = request();
        // The 10 mm cube touches the bed edge; the skirt reaches beyond.
        near_edge.objects = vec![cube_at([215.0, 110.0])];
        let result = SliceJob::start(near_edge, || {}).wait().unwrap();
        assert_eq!(result.warnings[0].code, "ui.paths_off_bed");
    }

    #[test]
    fn errors_come_back_as_diagnostics() {
        let mut empty = request();
        empty.objects = vec![Mesh::default()];
        assert_eq!(
            SliceJob::start(empty, || {}).wait().unwrap_err().code,
            "slicing.empty_model"
        );
    }

    #[test]
    fn output_formats() {
        let settings = Settings::default();
        let result = SliceJob::start(request(), || {}).wait().unwrap();
        let plain = encode_output(&result.output, &settings, "cube.gcode").unwrap();
        assert_eq!(plain, result.output.gcode.as_bytes());
        let container = encode_output(&result.output, &settings, "Cube.GCODE.3MF").unwrap();
        assert!(container.starts_with(b"PK"));
        assert!(is_gcode_3mf("a.gcode.3mf") && !is_gcode_3mf("a.3mf"));
    }

    #[test]
    fn names_and_durations() {
        assert_eq!(output_name("Benchy.STL", false), "Benchy.gcode");
        assert_eq!(output_name("plate.3mf", true), "plate.gcode.3mf");
        assert_eq!(output_name("part", false), "part.gcode");
        assert_eq!(output_name(".stl", false), "model.gcode");
        assert_eq!(with_output_extension("a", false), "a.gcode");
        assert_eq!(with_output_extension("a.GCODE", false), "a.GCODE");
        assert_eq!(with_output_extension("a", true), "a.gcode.3mf");
        assert_eq!(with_output_extension("a.gcode", true), "a.gcode.3mf");
        assert_eq!(with_output_extension("a.gcode.3mf", true), "a.gcode.3mf");
        assert_eq!(format_duration(3723.4), "1h 02m 03s");
        assert_eq!(format_duration(-5.0), "0h 00m 00s");
    }
}
