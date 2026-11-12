//! Slices a corpus of generated models and checks invariants that must hold
//! for every model: no failure, the expected warnings, the expected layer
//! count, all moves on the bed, reproducible output and, for solid models,
//! an extruded volume close to the model volume.
#![allow(clippy::unwrap_used, clippy::field_reassign_with_default, missing_docs)]

use std::f64::consts::PI;

use slicer_core::primitives::{
    cone, cylinder, dome, extrude, gear_contour, regular_polygon, sphere, star_contour, torus, tube,
};
use slicer_core::{Diagnostics, Mesh};
use slicer_profiles::Settings;

struct Model {
    name: &'static str,
    mesh: Mesh,
    /// Warning codes the slicer must report, in order.
    warnings: &'static [&'static str],
    /// Allowed relative deviation of extruded volume from model volume at
    /// 100 % infill; `None` for models that cannot be filled completely.
    volume_tolerance: Option<f64>,
}

fn moved(mut mesh: Mesh, delta: [f64; 3]) -> Mesh {
    mesh.translate(delta);
    mesh
}

fn joined(parts: &[Mesh]) -> Mesh {
    let mut mesh = Mesh::default();
    for part in parts {
        mesh.merge(part);
    }
    mesh
}

fn prism(contour: &[[f64; 2]], height: f64) -> Mesh {
    extrude(contour, [0.0, 0.0], height, 1, 0.0)
}

fn corpus() -> Vec<Model> {
    let model = |name, mesh, warnings, volume_tolerance| Model {
        name,
        mesh,
        warnings,
        volume_tolerance,
    };
    const NONE: &[&str] = &[];
    const THIN: &[&str] = &["slicing.thin_regions"];
    const LEAKY: &[&str] = &["mesh.not_watertight"];

    let rotated_z = {
        let mut m = Mesh::cuboid(20.0, 20.0, 10.0);
        m.rotate_z(PI / 4.0);
        m
    };
    let tilted = {
        let mut m = Mesh::cuboid(20.0, 20.0, 20.0);
        m.rotate_x(0.5);
        m
    };
    let ramp = {
        let mut m = extrude(
            &[[0.0, 0.0], [20.0, 0.0], [0.0, 20.0]],
            [5.0, 5.0],
            15.0,
            1,
            0.0,
        );
        m.rotate_x(PI / 2.0);
        m
    };
    let inverted = {
        let mut m = Mesh::cuboid(15.0, 15.0, 5.0);
        m.flip();
        m
    };
    let with_junk = {
        let mut m = Mesh::cuboid(15.0, 15.0, 5.0);
        m.triangles.push([0, 2, 1]); // duplicate of a bottom triangle
        m
    };
    let open_top = {
        let mut m = Mesh::cuboid(15.0, 15.0, 5.0);
        m.triangles.retain(|t| *t != [4, 5, 6] && *t != [4, 6, 7]);
        m
    };
    // One missing triangle in the side of a tall prism built from 60 rings:
    // only the few layers crossing it are affected.
    let hole_in_side = {
        let ring = [[-8.0, -8.0], [8.0, -8.0], [8.0, 8.0], [-8.0, 8.0]];
        let mut m = extrude(&ring, [0.0, 0.0], 30.0, 60, 0.0);
        m.triangles.remove(0);
        m
    };
    let l_shape = [
        [0.0, 0.0],
        [20.0, 0.0],
        [20.0, 8.0],
        [8.0, 8.0],
        [8.0, 20.0],
        [0.0, 20.0],
    ];
    let square = [[-8.0, -8.0], [8.0, -8.0], [8.0, 8.0], [-8.0, 8.0]];

    vec![
        model("cube_20", Mesh::cuboid(20.0, 20.0, 20.0), NONE, Some(0.03)),
        model(
            "plate_100x100x1",
            Mesh::cuboid(100.0, 100.0, 1.0),
            NONE,
            Some(0.03),
        ),
        model(
            "big_plate_200x200x0.6",
            Mesh::cuboid(200.0, 200.0, 0.6),
            NONE,
            Some(0.03),
        ),
        model("pin_2x2x40", Mesh::cuboid(2.0, 2.0, 40.0), NONE, None),
        model("wall_0.9", Mesh::cuboid(20.0, 0.9, 5.0), NONE, None),
        model(
            "wall_0.3_too_thin",
            Mesh::cuboid(20.0, 0.3, 5.0),
            THIN,
            None,
        ),
        model("tiny_cube_0.5", Mesh::cuboid(0.5, 0.5, 0.5), THIN, None),
        model("cylinder", cylinder(10.0, 20.0, 64), NONE, Some(0.03)),
        model(
            "cylinder_1024_segments",
            cylinder(15.0, 4.0, 1024),
            NONE,
            Some(0.03),
        ),
        model("hex_prism", cylinder(10.0, 10.0, 6), NONE, Some(0.03)),
        model("triangle_prism", cylinder(10.0, 10.0, 3), NONE, Some(0.04)),
        model("cone", cone(10.0, 20.0, 64), THIN, Some(0.04)),
        model("pyramid", cone(12.0, 15.0, 4), THIN, Some(0.05)),
        model("sphere", sphere(10.0, 64), NONE, Some(0.04)),
        model("sphere_low_poly", sphere(8.0, 8), THIN, Some(0.05)),
        model("dome", dome(12.0, 64), NONE, Some(0.04)),
        model("tube", tube(12.0, 7.0, 10.0, 64), NONE, Some(0.03)),
        model("thin_tube_1mm_wall", tube(12.0, 11.0, 6.0, 96), NONE, None),
        model("torus", torus(15.0, 4.0, 64, 32), NONE, Some(0.05)),
        model(
            "star_5",
            prism(&star_contour(5, 15.0, 6.0), 5.0),
            NONE,
            Some(0.05),
        ),
        model(
            "star_needles",
            prism(&star_contour(40, 15.0, 5.0), 3.0),
            THIN,
            None,
        ),
        model(
            "gear_24",
            prism(&gear_contour(24, 20.0, 2.0), 5.0),
            NONE,
            Some(0.05),
        ),
        model(
            "l_shape",
            extrude(&l_shape, [4.0, 4.0], 6.0, 1, 0.0),
            NONE,
            Some(0.03),
        ),
        model(
            "octagon",
            prism(&regular_polygon(10.0, 8), 4.0),
            NONE,
            Some(0.03),
        ),
        model(
            "twisted_prism",
            extrude(&square, [0.0, 0.0], 30.0, 60, PI / 2.0),
            NONE,
            Some(0.03),
        ),
        model("cube_rotated_45", rotated_z, NONE, Some(0.03)),
        model("cube_tilted", tilted, THIN, Some(0.04)),
        model("ramp", ramp, THIN, Some(0.04)),
        model(
            "steps",
            joined(&[
                Mesh::cuboid(30.0, 30.0, 5.0),
                moved(Mesh::cuboid(10.0, 10.0, 10.0), [10.0, 10.0, 5.0]),
            ]),
            NONE,
            Some(0.03),
        ),
        model(
            "two_islands",
            joined(&[
                Mesh::cuboid(10.0, 10.0, 5.0),
                moved(Mesh::cuboid(10.0, 10.0, 8.0), [30.0, 5.0, 0.0]),
            ]),
            NONE,
            Some(0.03),
        ),
        model(
            "nested_rings",
            joined(&[
                tube(20.0, 17.0, 5.0, 64),
                tube(10.0, 7.0, 8.0, 64),
                cylinder(3.0, 10.0, 32),
            ]),
            NONE,
            Some(0.04),
        ),
        model(
            "fin_on_base",
            joined(&[
                Mesh::cuboid(20.0, 20.0, 3.0),
                moved(Mesh::cuboid(20.0, 0.3, 10.0), [0.0, 10.0, 3.0]),
            ]),
            THIN,
            None,
        ),
        model(
            "far_from_origin",
            moved(Mesh::cuboid(20.0, 20.0, 5.0), [1.0e5, -2.0e5, 3.0e4]),
            NONE,
            None,
        ),
        model("inside_out", inverted, NONE, Some(0.03)),
        model("duplicate_triangle", with_junk, LEAKY, Some(0.03)),
        model("open_top", open_top, LEAKY, None),
        model(
            "hole_in_side",
            hole_in_side,
            &["mesh.not_watertight", "slicing.open_contours"],
            None,
        ),
    ]
}

/// Value of the word starting with `letter` in a G-code line.
fn word(line: &str, letter: char) -> Option<f64> {
    line.split(';')
        .next()?
        .split_whitespace()
        .skip(1)
        .find_map(|w| w.strip_prefix(letter).and_then(|v| v.parse().ok()))
}

#[test]
fn corpus_invariants() {
    let defaults = Settings::default();
    let mut solid = Settings::default();
    solid.infill_density = 100.0;
    solid.skirt_loops = 0;
    solid.infill_overlap = 0.0;

    let models = corpus();
    assert!(
        models.len() >= 30,
        "corpus has only {} models",
        models.len()
    );
    let mut failures = Vec::new();
    println!(
        "{:26} {:>6} {:>7} {:>9} {:>8}  warnings",
        "model", "tris", "layers", "volume", "ratio"
    );

    for model in &models {
        // Go through the STL writer and reader, as a real model would.
        let mesh =
            slicer_formats::parse_stl(&slicer_formats::write_stl_binary(&model.mesh)).unwrap();
        let (min, max) = mesh.bounds().unwrap();
        let height = max[2] - min[2];

        let mut warnings = Diagnostics::new();
        let toolpaths = match slicer_fdm::generate_reporting(&mesh, &defaults, &mut warnings) {
            Ok(toolpaths) => toolpaths,
            Err(error) => {
                failures.push(format!("{}: slicing failed: {error}", model.name));
                continue;
            }
        };
        if warnings.codes() != model.warnings {
            failures.push(format!(
                "{}: warnings {:?}, expected {:?}",
                model.name,
                warnings.codes(),
                model.warnings
            ));
        }

        let expected_layers = ((height - defaults.first_layer_height + 1e-6)
            / defaults.layer_height)
            .floor() as usize
            + 1;
        if toolpaths.layers.len() != expected_layers {
            failures.push(format!(
                "{}: {} layers, expected {expected_layers}",
                model.name,
                toolpaths.layers.len()
            ));
        }

        let output = slicer_gcode::write_gcode(&toolpaths, &defaults).unwrap();
        for line in output
            .gcode
            .lines()
            .filter(|l| l.starts_with("G0 ") || l.starts_with("G1 "))
        {
            let on_bed = [('X', defaults.bed_size_x), ('Y', defaults.bed_size_y)]
                .iter()
                .all(|&(axis, size)| word(line, axis).is_none_or(|v| (0.0..=size).contains(&v)));
            if !on_bed || line.contains("NaN") || line.contains("inf") {
                failures.push(format!("{}: bad move '{line}'", model.name));
                break;
            }
        }
        if !output.stats.filament_length_mm.is_finite() || output.stats.filament_length_mm < 0.0 {
            failures.push(format!(
                "{}: filament {}",
                model.name, output.stats.filament_length_mm
            ));
        }

        // Same input, same output.
        let again = slicer_fdm::generate(&mesh, &defaults).unwrap();
        if slicer_gcode::write_gcode(&again, &defaults).unwrap().gcode != output.gcode {
            failures.push(format!("{}: output is not reproducible", model.name));
        }

        let mut ratio_text = "-".to_owned();
        if let Some(tolerance) = model.volume_tolerance {
            let filled = slicer_fdm::generate(&mesh, &solid).unwrap();
            let extruded = slicer_gcode::write_gcode(&filled, &solid)
                .unwrap()
                .stats
                .filament_volume_cm3
                * 1000.0;
            let ratio = extruded / mesh.volume();
            ratio_text = format!("{ratio:.4}");
            if (ratio - 1.0).abs() > tolerance {
                failures.push(format!(
                    "{}: extruded {extruded:.1} mm3 for a {:.1} mm3 model (ratio {ratio:.4}, allowed +-{tolerance})",
                    model.name,
                    mesh.volume()
                ));
            }
        }
        println!(
            "{:26} {:>6} {:>7} {:>9.1} {:>8}  {:?}",
            model.name,
            mesh.triangles.len(),
            toolpaths.layers.len(),
            mesh.volume(),
            ratio_text,
            warnings.codes()
        );
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn model_open_on_every_layer_is_an_error() {
    use slicer_core::Diagnose;
    // Half of one side face is missing over the full height, so no layer
    // has a closed contour.
    let mut mesh = Mesh::cuboid(15.0, 15.0, 5.0);
    mesh.triangles.retain(|t| *t != [0, 1, 5]);
    let mut warnings = Diagnostics::new();
    let error =
        slicer_fdm::generate_reporting(&mesh, &Settings::default(), &mut warnings).unwrap_err();
    assert_eq!(error.code(), "slicing.nothing_to_print");
    assert_eq!(
        warnings.codes(),
        ["mesh.not_watertight", "slicing.open_contours"]
    );
}

#[test]
fn defective_files_are_rejected_with_a_code() {
    use slicer_core::Diagnose;
    let good = slicer_formats::write_stl_binary(&Mesh::cuboid(1.0, 1.0, 1.0));
    let mut huge_count = good.clone();
    huge_count[80..84].copy_from_slice(&u32::MAX.to_le_bytes());
    let mut nan = good.clone();
    nan[96..100].copy_from_slice(&f32::NAN.to_le_bytes());
    let mut all_same_point = vec![0u8; 84 + 50];
    all_same_point[80] = 1;

    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        ("empty file", Vec::new(), "model.unrecognized"),
        (
            "random text",
            b"this is not a model".to_vec(),
            "model.unrecognized",
        ),
        ("header only", good[..84].to_vec(), "model.truncated"),
        ("cut in the middle", good[..300].to_vec(), "model.truncated"),
        (
            "one byte too long",
            [good.clone(), vec![0]].concat(),
            "model.truncated",
        ),
        ("absurd triangle count", huge_count, "model.truncated"),
        ("NaN coordinate", nan, "model.non_finite"),
        (
            "only degenerate triangles",
            all_same_point,
            "model.no_triangles",
        ),
        (
            "ASCII without facets",
            b"solid a\nendsolid a\n".to_vec(),
            "model.no_triangles",
        ),
        (
            "ASCII with a broken number",
            b"solid a\nvertex 1 2 x\n".to_vec(),
            "model.invalid_vertex",
        ),
        (
            "ASCII with two vertices",
            b"solid a\nvertex 0 0 0\nvertex 1 0 0\n".to_vec(),
            "model.incomplete_triangle",
        ),
        (
            "ASCII with infinity",
            b"solid a\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 inf 0\n".to_vec(),
            "model.non_finite",
        ),
    ];
    for (name, bytes, code) in cases {
        match slicer_formats::parse_stl(&bytes) {
            Ok(_) => panic!("{name}: accepted"),
            Err(error) => assert_eq!(error.code(), code, "{name}: {error}"),
        }
    }

    // Windows line endings and indentation are fine.
    let ascii = "solid a\r\n  facet normal 0 0 1\r\n    outer loop\r\n      vertex 0 0 0\r\n      vertex 1 0 0\r\n      vertex 0 1 0\r\n    endloop\r\n  endfacet\r\nendsolid a\r\n";
    assert_eq!(
        slicer_formats::parse_stl(ascii.as_bytes())
            .unwrap()
            .triangles
            .len(),
        1
    );
}

#[test]
fn reference_models_slice_cleanly() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut count = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "stl") {
            continue;
        }
        let mesh = slicer_formats::parse_stl(&std::fs::read(&path).unwrap()).unwrap();
        let mut warnings = Diagnostics::new();
        let toolpaths =
            slicer_fdm::generate_reporting(&mesh, &Settings::default(), &mut warnings).unwrap();
        assert!(
            warnings.is_empty(),
            "{}: {:?}",
            path.display(),
            warnings.codes()
        );
        assert!(!toolpaths.layers.is_empty());
        count += 1;
    }
    assert_eq!(count, 6);
}
