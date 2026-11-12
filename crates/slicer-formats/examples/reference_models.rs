//! Writes the reference models into a directory as binary STL.
//!
//! The files in `examples/` at the repository root were produced with
//! `cargo run -p slicer-formats --example reference_models -- examples`.
//! They are committed so every platform slices byte-identical input.
//! All of them print without supports.

use std::path::PathBuf;

use slicer_core::Mesh;
use slicer_core::primitives::{cylinder, dome, extrude, gear_contour, tube};

fn main() -> std::io::Result<()> {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "examples".to_owned()),
    );
    std::fs::create_dir_all(&dir)?;

    // A base plate with a narrower block on top: exercises top surfaces
    // that are only partly exposed.
    let mut steps = Mesh::cuboid(30.0, 30.0, 5.0);
    let mut block = Mesh::cuboid(12.0, 12.0, 10.0);
    block.translate([9.0, 9.0, 5.0]);
    steps.merge(&block);

    // A gear with a raised hub: many short edges, concave corners and two
    // overlapping shells that the slicer has to unite.
    let mut gear = extrude(&gear_contour(16, 18.0, 3.0), [0.0, 0.0], 6.0, 1, 0.0);
    gear.merge(&cylinder(8.0, 10.0, 48));

    let models: [(&str, Mesh); 6] = [
        ("cube_20mm", Mesh::cuboid(20.0, 20.0, 20.0)),
        ("cylinder_20mm", cylinder(10.0, 20.0, 96)),
        ("dome_30mm", dome(15.0, 96)),
        ("tube_24mm", tube(12.0, 7.0, 10.0, 96)),
        ("steps", steps),
        ("gear_with_hub", gear),
    ];
    for (name, mesh) in models {
        let path = dir.join(format!("{name}.stl"));
        std::fs::write(&path, slicer_formats::write_stl_binary(&mesh))?;
        println!("{} ({} triangles)", path.display(), mesh.triangles.len());
    }
    Ok(())
}
