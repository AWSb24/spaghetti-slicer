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

/// Damages a file in a random but reproducible way, for tests that feed
/// broken input to the readers.
// ponytail: blind mutation, no coverage feedback. It finds shallow crashes
// only; a real fuzzer (cargo-fuzz) is the next step if those matter.
#[cfg(test)]
pub(crate) fn mutate(original: &[u8], state: &mut u64) -> Vec<u8> {
    const TOKENS: [&[u8]; 10] = [
        b"-1",
        b"4294967295",
        b"1e308",
        b"nan",
        b"inf",
        b"\"",
        b"<",
        b"0",
        b" ",
        b"\xff\xff\xff\xff",
    ];
    let mut next = |below: usize| {
        // xorshift64
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        (*state % below.max(1) as u64) as usize
    };
    let mut bytes = original.to_vec();
    for _ in 0..=next(3) {
        if bytes.is_empty() {
            break;
        }
        let at = next(bytes.len());
        let end = (at + 1 + next(40)).min(bytes.len());
        match next(5) {
            0 => bytes[at] = next(256) as u8,
            1 => drop(bytes.drain(at..end)),
            2 => {
                let copy = bytes[at..end].to_vec();
                bytes.splice(at..at, copy);
            }
            3 => bytes.truncate(at),
            _ => {
                let token = TOKENS[next(TOKENS.len())];
                bytes.splice(at..end.min(at + token.len()), token.iter().copied());
            }
        }
    }
    bytes
}
