//! The browser build: starts the app on the page's canvas and replaces the
//! file system with the browser's file picker, dropped files and
//! downloads. The desktop build has the same functions in `native.rs`.
//! The same module also runs in the slicing worker, which only calls
//! [`slice_in_worker`].
//!
//! Browsers read files asynchronously, so picking or dropping a file does
//! not open it at once: its bytes are kept here under its name, and
//! [`opened_files`] hands the names to the app in a later frame, which then
//! reads them with [`read`] like files on disk.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use eframe::egui;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::{JsCast, JsValue};

use crate::app::App;

thread_local! {
    /// Files read from the browser, by name, with the bytes or why they
    /// could not be read.
    // ponytail: every file ever opened stays in memory (the latest one per
    // name); drop them once nothing refers to them if memory gets tight.
    static FILES: RefCell<HashMap<PathBuf, Result<Vec<u8>, String>>> = RefCell::new(HashMap::new());
    /// Files that arrived since the app last asked, in the order they were
    /// picked or dropped.
    static ARRIVED: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    /// The object URL of the last download; freed at the next one, since
    /// freeing it right after the click can cancel the download.
    static LAST_DOWNLOAD: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Starts the app on the canvas with the id `slicer`. Does nothing in the
/// slicing worker, which has no window.
pub fn start() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let document = window.document();
    let Some(canvas) = document
        .as_ref()
        .and_then(|document| document.get_element_by_id("slicer"))
        .and_then(|element| element.dyn_into::<web_sys::HtmlCanvasElement>().ok())
    else {
        web_sys::console::error_1(&"no canvas with the id 'slicer' on the page".into());
        return;
    };
    wasm_bindgen_futures::spawn_local(async move {
        let started = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|cc| Ok(Box::new(App::new(&[], cc.storage)))),
            )
            .await;
        // The page shows a message until the app runs.
        if let Some(loading) = document.and_then(|d| d.get_element_by_id("loading")) {
            match started {
                Ok(()) => loading.remove(),
                Err(error) => loading.set_text_content(Some(&format!(
                    "The slicer could not start: {}",
                    js_error(&error)
                ))),
            }
        }
    });
}

/// Entry point of the slicing worker: `worker.js` calls it with the
/// request the page posted (see `slicer_ui::worker`).
#[wasm_bindgen]
pub fn slice_in_worker(request: JsValue) {
    slicer_ui::worker::serve(&request);
}

fn js_error(error: &JsValue) -> String {
    error.as_string().unwrap_or_else(|| format!("{error:?}"))
}

fn not_found(path: &Path) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, path.display().to_string())
}

/// Reads a file the browser handed over.
pub fn read(path: &Path) -> io::Result<Vec<u8>> {
    FILES.with_borrow(|files| match files.get(path) {
        Some(Ok(bytes)) => Ok(bytes.clone()),
        Some(Err(error)) => Err(io::Error::other(error.clone())),
        None => Err(not_found(path)),
    })
}

/// Reads a text file the browser handed over.
pub fn read_to_string(path: &Path) -> io::Result<String> {
    String::from_utf8(read(path)?).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Offers `bytes` as a download named after the last part of `path`.
pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let name = path
        .file_name()
        .map_or_else(|| "output".into(), |name| name.to_string_lossy());
    download(&name, bytes).map_err(|e| io::Error::other(js_error(&e)))
}

fn download(name: &str, bytes: &[u8]) -> Result<(), JsValue> {
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let options = web_sys::BlobPropertyBag::new();
    options.set_type("application/octet-stream");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)?;
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| JsValue::from_str("no document"))?;
    let anchor: web_sys::HtmlAnchorElement = document.create_element("a")?.dyn_into()?;
    anchor.set_href(&url);
    anchor.set_download(name);
    anchor.click();
    if let Some(previous) = LAST_DOWNLOAD.replace(Some(url)) {
        web_sys::Url::revoke_object_url(&previous)?;
    }
    Ok(())
}

/// Opens the browser's file picker. Returns nothing: the picked files
/// arrive later through [`opened_files`].
pub fn pick_files(
    ctx: &egui::Context,
    filter: &str,
    extensions: &[&str],
    multiple: bool,
) -> Vec<PathBuf> {
    let dialog = rfd::AsyncFileDialog::new().add_filter(filter, extensions);
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let picked = if multiple {
            dialog.pick_files().await.unwrap_or_default()
        } else {
            dialog.pick_file().await.into_iter().collect()
        };
        let mut files = Vec::new();
        for file in picked {
            files.push((PathBuf::from(file.file_name()), Ok(file.read().await)));
        }
        arrive(files, &ctx);
    });
    Vec::new()
}

/// The name to download as: there is no save dialog in the browser.
pub fn save_path(name: &str, _filter: &str, _extension: &str) -> Option<PathBuf> {
    Some(PathBuf::from(name))
}

/// Starts reading files dropped onto the page in this frame, and returns
/// the files that finished arriving since the last call.
pub fn opened_files(ctx: &egui::Context) -> Vec<PathBuf> {
    let dropped = ctx.input(|i| i.raw.dropped_files.clone());
    if !dropped.is_empty() {
        let ctx = ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let mut files = Vec::new();
            for file in dropped {
                files.push((file.path().to_path_buf(), file.bytes_async().await));
            }
            arrive(files, &ctx);
        });
    }
    ARRIVED.take()
}

fn arrive(files: Vec<(PathBuf, Result<Vec<u8>, String>)>, ctx: &egui::Context) {
    for (path, bytes) in files {
        FILES.with_borrow_mut(|store| store.insert(path.clone(), bytes));
        ARRIVED.with_borrow_mut(|arrived| arrived.push(path));
    }
    ctx.request_repaint();
}
