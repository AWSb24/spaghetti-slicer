//! Reading and writing model and project formats (STL, 3MF).
//!
//! Only STL is implemented so far.

pub mod stl;

pub use stl::{StlError, parse_stl, write_stl_binary};
