//! How far a slice has got, for progress bars.
//!
//! The slicing thread writes into a [`Progress`] and any other thread reads
//! it. Every step owns a fixed share of the whole; within a step the share
//! grows with the parts (usually layers) that are done. Writing costs one
//! atomic operation per layer and step. A reader that cannot look while
//! the slice runs (the page, while a Web Worker slices) is told about every
//! change through [`Progress::with_report`] instead.

use std::fmt;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};

/// The steps of a slice, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Checking and repairing the mesh.
    Preparing,
    /// Cutting the mesh into layers.
    Cutting,
    /// Walls, infill and supports of every layer.
    Geometry,
    /// Putting the paths of every layer in printing order.
    Ordering,
    /// Writing the G-code.
    Gcode,
    /// Finished.
    Done,
}

/// Shares of the whole slice in millionths at which the steps start, in the
/// order of [`Step`]; the last entry is the end. Roughly what the steps
/// took on the 3DBenchy with default settings, with and without supports:
/// preparing 45 to 50 %, cutting 10 to 15 %, paths 25 to 35 %, ordering
/// 1 to 3 %, G-code 6 to 8 %.
const STARTS: [u32; 6] = [0, 450_000, 570_000, 880_000, 910_000, 1_000_000];

impl Step {
    const ALL: [Step; 6] = [
        Step::Preparing,
        Step::Cutting,
        Step::Geometry,
        Step::Ordering,
        Step::Gcode,
        Step::Done,
    ];

    /// Text for a status line.
    pub fn label(self) -> &'static str {
        match self {
            Self::Preparing => "Preparing the model",
            Self::Cutting => "Cutting layers",
            Self::Geometry => "Generating paths",
            Self::Ordering => "Ordering paths",
            Self::Gcode => "Writing G-code",
            Self::Done => "Done",
        }
    }

    fn span(self) -> (u32, u32) {
        let index = self as usize;
        (STARTS[index], STARTS[(index + 1).min(STARTS.len() - 1)])
    }
}

/// Receives the step and the share done after every change.
type Report = Box<dyn Fn(Step, f64) + Send + Sync>;

/// Progress of one slice. Shared between the slicing thread and readers,
/// usually in an `Arc`.
#[derive(Default)]
pub struct Progress {
    step: AtomicU8,
    done: AtomicU32,
    total: AtomicU32,
    /// Share of the whole in millionths; never decreases.
    overall: AtomicU32,
    /// Told about every change, if set.
    report: Option<Report>,
}

impl fmt::Debug for Progress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Progress")
            .field("step", &self.step())
            .field("fraction", &self.fraction())
            .field("report", &self.report.is_some())
            .finish()
    }
}

impl Progress {
    /// Nothing done yet.
    pub const fn new() -> Self {
        Self {
            step: AtomicU8::new(0),
            done: AtomicU32::new(0),
            total: AtomicU32::new(1),
            overall: AtomicU32::new(0),
            report: None,
        }
    }

    /// Nothing done yet; `report` is called with the step and the share
    /// done after every change, in the slicing thread. It is called once
    /// per part, so it should be quick.
    pub fn with_report(report: impl Fn(Step, f64) + Send + Sync + 'static) -> Self {
        Self {
            report: Some(Box::new(report)),
            ..Self::new()
        }
    }

    /// Starts `step`, which consists of `parts` equal parts.
    pub fn start(&self, step: Step, parts: usize) {
        self.total.store(
            u32::try_from(parts.max(1)).unwrap_or(u32::MAX),
            Ordering::Relaxed,
        );
        self.done.store(0, Ordering::Relaxed);
        self.step.store(step as u8, Ordering::Relaxed);
        self.raise(step.span().0);
    }

    /// One more part of the current step is done. May be called from
    /// several threads at once.
    pub fn advance(&self) {
        let done = self.done.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        let total = self.total.load(Ordering::Relaxed).max(1);
        let (start, end) = self.step().span();
        let share = u64::from(end - start) * u64::from(done.min(total)) / u64::from(total);
        self.raise(start + u32::try_from(share).unwrap_or(end - start));
    }

    /// Marks the slice as finished.
    pub fn finish(&self) {
        self.step.store(Step::Done as u8, Ordering::Relaxed);
        self.raise(STARTS[STARTS.len() - 1]);
    }

    /// Sets the step and the share done, from 0 to 1, directly. For a copy
    /// of a progress that is written elsewhere, for example in a Web
    /// Worker. The share still never decreases.
    pub fn set(&self, step: Step, fraction: f64) {
        self.step.store(step as u8, Ordering::Relaxed);
        // Saturating cast: NaN becomes 0.
        self.raise((fraction.clamp(0.0, 1.0) * 1e6).round() as u32);
    }

    fn raise(&self, share: u32) {
        self.overall.fetch_max(share, Ordering::Relaxed);
        if let Some(report) = &self.report {
            report(self.step(), self.fraction());
        }
    }

    /// The step being worked on.
    pub fn step(&self) -> Step {
        let index = usize::from(self.step.load(Ordering::Relaxed));
        Step::ALL[index.min(Step::ALL.len() - 1)]
    }

    /// Share of the whole slice that is done, from 0 to 1. Never decreases.
    pub fn fraction(&self) -> f64 {
        f64::from(self.overall.load(Ordering::Relaxed)) / 1e6
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_follow_each_other_and_fill_the_whole() {
        assert_eq!(STARTS[0], 0);
        assert!(STARTS.windows(2).all(|w| w[0] < w[1]));
        for pair in Step::ALL.windows(2) {
            assert_eq!(pair[0].span().1, pair[1].span().0);
        }
        assert_eq!(Step::Gcode.span().1, 1_000_000);
    }

    #[test]
    fn progress_grows_with_the_parts() {
        let progress = Progress::new();
        assert_eq!(progress.fraction(), 0.0);
        progress.start(Step::Geometry, 4);
        assert_eq!(progress.step(), Step::Geometry);
        assert_eq!(progress.fraction(), 0.57);
        progress.advance();
        assert!((progress.fraction() - (0.57 + 0.31 / 4.0)).abs() < 1e-6);
        // Too many parts stop at the end of the step.
        for _ in 0..10 {
            progress.advance();
        }
        assert_eq!(progress.fraction(), 0.88);
        // A step that starts behind the reached share does not go back.
        progress.start(Step::Cutting, 1);
        assert_eq!(progress.fraction(), 0.88);
        progress.finish();
        assert_eq!((progress.step(), progress.fraction()), (Step::Done, 1.0));
        assert_eq!(Step::Gcode.label(), "Writing G-code");
    }

    #[test]
    fn reports_every_change_and_copies_follow() {
        use std::sync::{Arc, Mutex};

        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let progress = Progress::with_report(move |step, fraction| {
            if let Ok(mut seen) = sink.lock() {
                seen.push((step, fraction));
            }
        });
        progress.start(Step::Cutting, 2);
        progress.advance();
        progress.finish();
        let seen = seen.lock().map(|seen| seen.clone()).unwrap_or_default();
        assert_eq!(
            seen,
            [
                (Step::Cutting, 0.45),
                (Step::Cutting, 0.51),
                (Step::Done, 1.0)
            ]
        );
        assert!(format!("{progress:?}").contains("Done"));

        let copy = Progress::new();
        copy.set(Step::Geometry, 0.6);
        assert_eq!((copy.step(), copy.fraction()), (Step::Geometry, 0.6));
        copy.set(Step::Geometry, 0.5);
        assert_eq!(copy.fraction(), 0.6);
        copy.set(Step::Done, f64::NAN);
        assert_eq!((copy.step(), copy.fraction()), (Step::Done, 0.6));
    }
}
