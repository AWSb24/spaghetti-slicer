//! Reading and writing model and project formats (STL, 3MF).

pub mod stl;
pub mod threemf;

pub use stl::{StlError, parse_stl, write_stl_binary};
pub use threemf::{ThreeMfError, looks_like_3mf, parse_3mf, parse_3mf_plate, write_3mf};

use slicer_core::{Diagnose, Diagnostic, Diagnostics, Mesh};

/// Parses a model file, picking the reader by the file's content. `plate`
/// selects one build plate of a project file with several plates.
pub fn parse_model(
    bytes: &[u8],
    plate: Option<u32>,
    warnings: &mut Diagnostics,
) -> Result<Mesh, Diagnostic> {
    if looks_like_3mf(bytes) {
        return parse_3mf_plate(bytes, plate, warnings).map_err(|e| e.diagnostic());
    }
    // An STL file is one plate.
    if let Some(plate) = plate.filter(|&plate| plate != 1) {
        let available = Vec::new();
        return Err(ThreeMfError::UnknownPlate { plate, available }.diagnostic());
    }
    parse_stl(bytes).map_err(|e| e.diagnostic())
}
