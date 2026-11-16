//! Desktop application of the slicer.
//!
//! Usage: `spaghetti-slicer-ui [MODEL] [SETTINGS.toml ...]`. Files given on
//! the command line are opened as if dropped onto the window.

// No console window next to the application on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;

use std::path::PathBuf;
use std::process::ExitCode;

use eframe::egui;

fn main() -> ExitCode {
    let paths: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Spaghetti Slicer")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([800.0, 500.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    let result = eframe::run_native(
        "Spaghetti Slicer",
        options,
        Box::new(move |_| Ok(Box::new(app::App::new(&paths)))),
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: cannot open the window: {error}");
            ExitCode::FAILURE
        }
    }
}
