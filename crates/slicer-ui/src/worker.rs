//! Slicing in a Web Worker, for the browser build.
//!
//! The browser has no threads, and a slice on the page itself would freeze
//! it. So every slice starts a worker from `worker.js` (next to the page),
//! which loads the same WebAssembly module and calls [`serve`]. The two
//! sides talk in messages:
//!
//! - page to worker: the request as a `Uint8Array` ([`crate::wire`]);
//! - worker to page: `[step, fraction]` arrays while slicing (step as in
//!   [`crate::wire::step_code`]), then the outcome as a `Uint8Array`, or a
//!   text if the worker failed.
//!
//! Cancel terminates the worker; so does the end of the slice, which frees
//! its memory.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;

use js_sys::{Array, Uint8Array};
use slicer_core::{Diagnose, Diagnostic};
use slicer_fdm::{Progress, Step};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{DedicatedWorkerGlobalScope, ErrorEvent, Event, MessageEvent, Worker};

use crate::job::{SliceRequest, SliceResult, slice};
use crate::wire;

/// The worker script, relative to the page.
const SCRIPT: &str = "./worker.js";

type Outcome = Result<SliceResult, Diagnostic>;

/// The page's side of a slice in a worker.
pub struct WorkerJob {
    /// `None` if the worker could not be started.
    worker: Option<Worker>,
    end: Rc<End>,
    /// The message handlers; they must live as long as the worker.
    handlers: Option<Handlers>,
}

type Handlers = (Closure<dyn FnMut(MessageEvent)>, Closure<dyn FnMut(Event)>);

impl std::fmt::Debug for WorkerJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerJob")
            .field("running", &!self.end.done.get())
            .finish()
    }
}

/// Hands the outcome to the job, once.
struct End {
    sender: Sender<Outcome>,
    notify: RefCell<Option<Box<dyn FnOnce()>>>,
    done: Cell<bool>,
}

impl End {
    fn deliver(&self, outcome: Outcome) {
        if self.done.replace(true) {
            return;
        }
        // The receiver is gone if the job was dropped; nobody waits then.
        let _ = self.sender.send(outcome);
        if let Some(notify) = self.notify.take() {
            notify();
        }
    }
}

impl WorkerJob {
    /// Starts a worker that slices `request`. Progress goes to `progress`,
    /// the outcome to `sender`, then `notify` is called. If no worker can
    /// be started, that is the outcome.
    pub fn start(
        request: &SliceRequest,
        sender: Sender<Outcome>,
        progress: Arc<Progress>,
        notify: impl FnOnce() + 'static,
    ) -> Self {
        let end = Rc::new(End {
            sender,
            notify: RefCell::new(Some(Box::new(notify))),
            done: Cell::new(false),
        });
        match spawn(request, &end, progress) {
            Ok((worker, handlers)) => Self {
                worker: Some(worker),
                end,
                handlers: Some(handlers),
            },
            Err(error) => {
                end.deliver(Err(failed(&format!(
                    "the browser could not start slicing in the background: {}",
                    js_text(&error)
                ))));
                Self {
                    worker: None,
                    end,
                    handlers: None,
                }
            }
        }
    }

    /// Stops the worker; the outcome is the `slicing.cancelled` error
    /// unless the slice had already ended.
    pub fn cancel(&self) {
        if self.end.done.get() {
            return;
        }
        if let Some(worker) = &self.worker {
            worker.terminate();
        }
        self.end
            .deliver(Err(slicer_fdm::FdmError::Cancelled.diagnostic()));
    }
}

impl Drop for WorkerJob {
    fn drop(&mut self) {
        // No message may arrive once the handlers are gone.
        if let Some(worker) = &self.worker {
            worker.terminate();
            worker.set_onmessage(None);
            worker.set_onerror(None);
        }
        drop(self.handlers.take());
    }
}

/// Starts the worker and sends it the request.
fn spawn(
    request: &SliceRequest,
    end: &Rc<End>,
    progress: Arc<Progress>,
) -> Result<(Worker, Handlers), JsValue> {
    let options = web_sys::WorkerOptions::new();
    options.set_type(web_sys::WorkerType::Module);
    let worker = Worker::new_with_options(SCRIPT, &options)?;

    let (end_message, worker_message) = (Rc::clone(end), worker.clone());
    let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let data = event.data();
        if let Some(bytes) = data.dyn_ref::<Uint8Array>() {
            worker_message.terminate();
            end_message.deliver(wire::decode_outcome(&bytes.to_vec()));
        } else if Array::is_array(&data) {
            let report: Array = data.unchecked_into();
            let step = report
                .get(0)
                .as_f64()
                .and_then(|code| wire::step_from_code(code as u8));
            if let (Some(step), Some(fraction)) = (step, report.get(1).as_f64()) {
                progress.set(step, fraction);
            }
        } else {
            worker_message.terminate();
            end_message.deliver(Err(failed(&format!(
                "slicing stopped unexpectedly: {}",
                js_text(&data)
            ))));
        }
    });
    worker.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

    // The script could not be loaded, or the worker failed outside a slice.
    let (end_error, worker_error) = (Rc::clone(end), worker.clone());
    let on_error = Closure::<dyn FnMut(Event)>::new(move |event: Event| {
        event.prevent_default();
        let detail = event
            .dyn_ref::<ErrorEvent>()
            .map(ErrorEvent::message)
            .filter(|message| !message.is_empty())
            .unwrap_or_else(|| format!("{SCRIPT} could not be loaded"));
        worker_error.terminate();
        end_error.deliver(Err(failed(&format!(
            "slicing stopped unexpectedly: {detail}"
        ))));
    });
    worker.set_onerror(Some(on_error.as_ref().unchecked_ref()));

    let message = Uint8Array::from(wire::encode_request(request).as_slice());
    worker.post_message_with_transfer(&message, &Array::of1(&message.buffer()))?;
    Ok((worker, (on_message, on_error)))
}

/// The last progress posted to the page, as step × 1000 + per mille; posts
/// in between are left out.
static POSTED: AtomicU32 = AtomicU32::new(u32::MAX);

/// Runs in the worker (`worker.js` calls it with the message from the
/// page): slices the request and posts progress and the outcome back.
pub fn serve(request: &JsValue) {
    let scope: DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
    let outcome = match request.dyn_ref::<Uint8Array>() {
        Some(bytes) => wire::decode_request(&bytes.to_vec()).and_then(|request| {
            POSTED.store(u32::MAX, Ordering::Relaxed);
            let progress = Progress::with_report(post_progress);
            // Cancel terminates the worker; nothing sets this.
            slice(&request, &AtomicBool::new(false), &progress)
        }),
        None => Err(failed("the worker got no slice request")),
    };
    let message = Uint8Array::from(wire::encode_outcome(&outcome).as_slice());
    if let Err(error) = scope.post_message_with_transfer(&message, &Array::of1(&message.buffer())) {
        web_sys::console::error_1(&error);
    }
}

/// Posts the progress to the page if it moved by a tenth of a per cent or
/// went to another step.
fn post_progress(step: Step, fraction: f64) {
    let code = wire::step_code(step);
    let key = u32::from(code) * 1000 + (fraction.clamp(0.0, 1.0) * 999.0) as u32;
    if POSTED.swap(key, Ordering::Relaxed) == key {
        return;
    }
    let scope: DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
    let report = Array::of2(&JsValue::from(code), &JsValue::from(fraction));
    // A lost progress message only delays the bar.
    let _ = scope.post_message(&report);
}

/// The error for a slice that failed in the browser.
fn failed(message: &str) -> Diagnostic {
    Diagnostic::error("ui.slice_failed", message)
}

/// A JavaScript value as text, for messages.
fn js_text(value: &JsValue) -> String {
    value
        .as_string()
        .or_else(|| {
            value
                .dyn_ref::<js_sys::Error>()
                .map(|error| String::from(error.message()))
        })
        .unwrap_or_else(|| format!("{value:?}"))
}
