//! Desktop user interface for the slicer: the logic behind the window.
//!
//! Everything in this library is plain code without drawing: loading a
//! model and placing it on the plate, building settings, running a slice in
//! the background and mapping the plate to the screen. The window itself
//! (egui) lives in the binary and only calls into these modules, so they
//! can be tested and reused by a later web build.

pub mod job;
pub mod plate;
pub mod settings;
pub mod view;
