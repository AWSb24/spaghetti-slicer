//! Desktop application of the slicer.
//!
//! Usage: `spaghetti-slicer-ui [MODEL] [SETTINGS.toml ...]`. Files given on
//! the command line are opened as if dropped onto the window.
//!
//! Built for `wasm32-unknown-unknown`, the same app runs in a browser
//! (see `web.rs` and `tools/build-web.sh`).

// No console window next to the application on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
// Files and dialogs: the file system on the desktop, the browser on the web.
#[cfg_attr(target_arch = "wasm32", path = "web.rs")]
#[cfg_attr(not(target_arch = "wasm32"), path = "native.rs")]
mod platform;

#[cfg(not(target_arch = "wasm32"))]
fn main() -> std::process::ExitCode {
    use eframe::egui;
    use std::path::PathBuf;

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
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: cannot open the window: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {
    platform::start();
}
