//! Geometry, meshes, layering and polygon operations. Must stay free of file system, network and UI dependencies.
//!
//! All lengths are millimetres.

mod cut;
pub mod diagnostics;
pub mod mesh;
pub mod parallel;
pub mod png;
pub mod polygon;
pub mod primitives;
pub mod slice;
pub mod thumbnail;

pub use diagnostics::{Diagnose, Diagnostic, Diagnostics, Severity};
pub use mesh::Mesh;
pub use parallel::parallel_map;
pub use polygon::{Contour, Point, Shape, Shapes};
