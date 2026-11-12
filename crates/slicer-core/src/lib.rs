//! Geometry, meshes, layering and polygon operations. Must stay free of file system, network and UI dependencies.
//!
//! All lengths are millimetres.

pub mod diagnostics;
pub mod mesh;
pub mod polygon;
pub mod primitives;
pub mod slice;

pub use diagnostics::{Diagnose, Diagnostic, Diagnostics, Severity};
pub use mesh::Mesh;
pub use polygon::{Contour, Point, Shape, Shapes};
