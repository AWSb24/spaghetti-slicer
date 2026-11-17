// Slices one model in the background so that the page keeps reacting.
// The page starts this worker for every slice and posts it the request;
// the same WebAssembly module as the page's does the work and posts back
// progress and the result (see crates/slicer-ui/src/worker.rs).
import init, { slice_in_worker } from "./slicer.js";

// Started at once, so the module loads while the request is on its way.
const ready = init();

self.onmessage = async (event) => {
  try {
    await ready;
    slice_in_worker(event.data);
  } catch (error) {
    // A text tells the page that the slice failed.
    self.postMessage(String(error));
  }
};
