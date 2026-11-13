//! STL reader (binary and ASCII) and binary writer.

use std::fmt;

use slicer_core::Mesh;

const HEADER_LEN: usize = 84;
const TRIANGLE_LEN: usize = 50;

/// Why an STL file could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StlError {
    /// The file is neither a complete binary STL nor an ASCII STL.
    Unrecognized,
    /// A binary STL whose length does not match its triangle count.
    Truncated {
        /// Length implied by the triangle count in the header.
        expected: u64,
        /// Actual length of the data.
        actual: u64,
    },
    /// An ASCII STL with a malformed `vertex` line.
    InvalidVertex {
        /// One-based line number.
        line: usize,
    },
    /// An ASCII STL whose vertex count is not a multiple of three.
    IncompleteTriangle,
    /// A coordinate is NaN or infinite.
    NonFiniteCoordinate,
    /// The file contains no usable triangles.
    NoTriangles,
}

impl fmt::Display for StlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unrecognized => write!(f, "not a valid binary or ASCII STL file"),
            Self::Truncated { expected, actual } => write!(
                f,
                "binary STL is truncated or corrupt: header implies {expected} bytes, file has {actual}"
            ),
            Self::InvalidVertex { line } => write!(f, "malformed vertex on line {line}"),
            Self::IncompleteTriangle => write!(f, "vertex count is not a multiple of three"),
            Self::NonFiniteCoordinate => write!(f, "file contains NaN or infinite coordinates"),
            Self::NoTriangles => write!(f, "file contains no triangles"),
        }
    }
}

impl std::error::Error for StlError {}

impl slicer_core::Diagnose for StlError {
    fn code(&self) -> &'static str {
        match self {
            Self::Unrecognized => "model.unrecognized",
            Self::Truncated { .. } => "model.truncated",
            Self::InvalidVertex { .. } => "model.invalid_vertex",
            Self::IncompleteTriangle => "model.incomplete_triangle",
            Self::NonFiniteCoordinate => "model.non_finite",
            Self::NoTriangles => "model.no_triangles",
        }
    }

    fn context(&self) -> Vec<(&'static str, String)> {
        match self {
            Self::Truncated { expected, actual } => vec![
                ("expected_bytes", expected.to_string()),
                ("actual_bytes", actual.to_string()),
            ],
            Self::InvalidVertex { line } => vec![("line", line.to_string())],
            _ => Vec::new(),
        }
    }
}

/// Parses a binary or ASCII STL file.
pub fn parse_stl(bytes: &[u8]) -> Result<Mesh, StlError> {
    let triangles = match binary_triangle_count(bytes) {
        Some(count) if HEADER_LEN as u64 + count * TRIANGLE_LEN as u64 == bytes.len() as u64 => {
            parse_binary(bytes)
        }
        declared => {
            if looks_like_ascii(bytes) {
                parse_ascii(bytes)?
            } else if let Some(count) = declared {
                return Err(StlError::Truncated {
                    expected: HEADER_LEN as u64 + count * TRIANGLE_LEN as u64,
                    actual: bytes.len() as u64,
                });
            } else {
                return Err(StlError::Unrecognized);
            }
        }
    };
    if triangles.iter().flatten().flatten().any(|c| !c.is_finite()) {
        return Err(StlError::NonFiniteCoordinate);
    }
    let mesh = Mesh::from_triangle_soup(&triangles);
    if mesh.triangles.is_empty() {
        return Err(StlError::NoTriangles);
    }
    Ok(mesh)
}

fn binary_triangle_count(bytes: &[u8]) -> Option<u64> {
    let count = bytes.get(80..HEADER_LEN)?;
    Some(u64::from(u32::from_le_bytes([
        count[0], count[1], count[2], count[3],
    ])))
}

fn looks_like_ascii(bytes: &[u8]) -> bool {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    bytes[start..].starts_with(b"solid")
}

fn parse_binary(bytes: &[u8]) -> Vec<[[f32; 3]; 3]> {
    bytes[HEADER_LEN..]
        .as_chunks::<TRIANGLE_LEN>()
        .0
        .iter()
        .map(|record| {
            let mut triangle = [[0f32; 3]; 3];
            // Skip the 12-byte normal; it is recomputed from the winding.
            for (i, value) in record[12..48].as_chunks::<4>().0.iter().enumerate() {
                triangle[i / 3][i % 3] = f32::from_le_bytes(*value);
            }
            triangle
        })
        .collect()
}

fn parse_ascii(bytes: &[u8]) -> Result<Vec<[[f32; 3]; 3]>, StlError> {
    let text = String::from_utf8_lossy(bytes);
    let mut vertices: Vec<[f32; 3]> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let mut tokens = line.split_whitespace();
        if tokens.next() != Some("vertex") {
            continue;
        }
        let mut vertex = [0f32; 3];
        for slot in &mut vertex {
            *slot = tokens
                .next()
                .and_then(|t| t.parse().ok())
                .ok_or(StlError::InvalidVertex { line: index + 1 })?;
        }
        vertices.push(vertex);
    }
    if !vertices.len().is_multiple_of(3) {
        return Err(StlError::IncompleteTriangle);
    }
    Ok(vertices.as_chunks::<3>().0.to_vec())
}

/// Serializes a mesh as binary STL.
pub fn write_stl_binary(mesh: &Mesh) -> Vec<u8> {
    let soup = mesh.to_triangle_soup();
    let mut out = vec![0u8; 80];
    out.extend((soup.len() as u32).to_le_bytes());
    for [a, b, c] in soup {
        let (u, v) = (
            [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
            [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
        );
        let n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        let n = if len > 0.0 { n.map(|c| c / len) } else { n };
        for value in n.iter().chain(&a).chain(&b).chain(&c) {
            out.extend(value.to_le_bytes());
        }
        out.extend([0u8; 2]);
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn damaged_files_never_panic() {
        let mesh = Mesh::cuboid(20.0, 10.0, 5.0);
        let binary = write_stl_binary(&mesh);
        let ascii = b"solid a\n facet normal 0 0 1\n  outer loop\n   vertex 0 0 0\n   vertex 1 0 0\n   vertex 0 1 0\n  endloop\n endfacet\n facet normal 0 0 1\n  outer loop\n   vertex 1 0 0\n   vertex 1 1 0\n   vertex 0 1 0\n  endloop\n endfacet\nendsolid a\n";
        let mut state = 0x9e37_79b9_7f4a_7c15;
        let mut accepted = 0;
        for round in 0..4000 {
            let original: &[u8] = if round % 2 == 0 { &binary } else { ascii };
            // Only the absence of a panic matters; a mesh that is returned
            // must at least be consistent.
            if let Ok(mesh) = parse_stl(&crate::mutate(original, &mut state)) {
                accepted += 1;
                let count = mesh.vertices.len() as u32;
                assert!(mesh.triangles.iter().flatten().all(|&v| v < count));
                assert!(mesh.vertices.iter().flatten().all(|c| c.is_finite()));
            }
        }
        // Some damage must go unnoticed, or the test exercises nothing.
        assert!(accepted > 0);
    }

    #[test]
    fn binary_roundtrip() {
        let mesh = Mesh::cuboid(20.0, 10.0, 5.0);
        let parsed = parse_stl(&write_stl_binary(&mesh)).unwrap();
        assert_eq!(parsed.triangles.len(), 12);
        assert_eq!(parsed.vertices.len(), 8);
        assert_eq!(parsed.open_edge_count(), 0);
        assert!((parsed.volume() - 1000.0).abs() < 1e-6);
    }

    #[test]
    fn ascii() {
        let text = "solid t\n facet normal 0 0 1\n  outer loop\n   vertex 0 0 0\n   vertex 1 0 0\n   vertex 0 1e0 0\n  endloop\n endfacet\nendsolid t\n";
        let mesh = parse_stl(text.as_bytes()).unwrap();
        assert_eq!(mesh.triangles, vec![[0, 1, 2]]);
    }

    #[test]
    fn binary_with_solid_in_header_is_still_binary() {
        let mut bytes = write_stl_binary(&Mesh::cuboid(1.0, 1.0, 1.0));
        bytes[..5].copy_from_slice(b"solid");
        assert_eq!(parse_stl(&bytes).unwrap().triangles.len(), 12);
    }

    #[test]
    fn broken_files_give_errors() {
        let good = write_stl_binary(&Mesh::cuboid(1.0, 1.0, 1.0));
        assert!(matches!(
            parse_stl(&good[..good.len() - 7]),
            Err(StlError::Truncated {
                expected: 684,
                actual: 677
            })
        ));
        assert_eq!(parse_stl(b""), Err(StlError::Unrecognized));
        assert_eq!(parse_stl(b"hello"), Err(StlError::Unrecognized));
        assert_eq!(
            parse_stl(b"solid x\nendsolid x\n"),
            Err(StlError::NoTriangles)
        );
        assert_eq!(
            parse_stl(b"solid x\nvertex 0 0\n"),
            Err(StlError::InvalidVertex { line: 2 })
        );
        assert_eq!(
            parse_stl(b"solid x\nvertex 0 0 0\nvertex 1 0 0\n"),
            Err(StlError::IncompleteTriangle)
        );
        assert_eq!(
            parse_stl(b"solid x\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 nan 0\n"),
            Err(StlError::NonFiniteCoordinate)
        );
    }
}
