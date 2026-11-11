//! Geometry, meshes, layering and polygon operations. Must stay free of file system, network and UI dependencies.
//!
//! All lengths are millimetres.

pub mod mesh;
pub mod polygon;
pub mod slice;

pub use mesh::Mesh;
pub use polygon::{Contour, Point, Shape, Shapes};
