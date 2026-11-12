//! Reading and writing model and project formats (STL, 3MF).

pub mod stl;
pub mod threemf;

pub use stl::{StlError, parse_stl, write_stl_binary};
pub use threemf::{ThreeMfError, looks_like_3mf, parse_3mf, write_3mf};

use slicer_core::{Diagnose, Diagnostic, Diagnostics, Mesh};

/// Parses a model file, picking the reader by the file's content.
pub fn parse_model(bytes: &[u8], warnings: &mut Diagnostics) -> Result<Mesh, Diagnostic> {
    if looks_like_3mf(bytes) {
        parse_3mf(bytes, warnings).map_err(|e| e.diagnostic())
    } else {
        parse_stl(bytes).map_err(|e| e.diagnostic())
    }
}
