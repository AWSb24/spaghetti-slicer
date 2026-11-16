//! Files on the desktop: native dialogs and the file system. The browser
//! build has the same functions in `web.rs`.

use std::io;
use std::path::{Path, PathBuf};

use eframe::egui;

/// Reads a file.
pub fn read(path: &Path) -> io::Result<Vec<u8>> {
    std::fs::read(path)
}

/// Reads a text file.
pub fn read_to_string(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path)
}

/// Writes a file.
pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    std::fs::write(path, bytes)
}

/// Asks for files to open with the extensions listed under `filter`.
pub fn pick_files(
    _ctx: &egui::Context,
    filter: &str,
    extensions: &[&str],
    multiple: bool,
) -> Vec<PathBuf> {
    let dialog = rfd::FileDialog::new().add_filter(filter, extensions);
    if multiple {
        dialog.pick_files().unwrap_or_default()
    } else {
        dialog.pick_file().into_iter().collect()
    }
}

/// Asks where to save, suggesting `name`.
pub fn save_path(name: &str, filter: &str, extension: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_file_name(name)
        .add_filter(filter, &[extension])
        .save_file()
}

/// Files dropped onto the window in this frame.
pub fn opened_files(ctx: &egui::Context) -> Vec<PathBuf> {
    ctx.input(|i| {
        i.raw
            .dropped_files
            .iter()
            .map(|file| file.path().to_path_buf())
            .collect()
    })
}
