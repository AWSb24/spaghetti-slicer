//! End-to-end tests running the real binary.
#![allow(clippy::unwrap_used, missing_docs)]

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use slicer_core::Mesh;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_spaghetti-slicer"))
}

/// A fresh, empty directory for one test.
fn workdir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn run(args: &[&str]) -> Output {
    bin().args(args).output().unwrap()
}

#[test]
fn slices_a_cube_and_reports_json() {
    let dir = workdir("cube");
    let stl = dir.join("cube.stl");
    fs::write(
        &stl,
        slicer_formats::write_stl_binary(&Mesh::cuboid(20.0, 20.0, 20.0)),
    )
    .unwrap();
    let svg = dir.join("svg");

    let out = run(&[
        "slice",
        stl.to_str().unwrap(),
        "--json",
        "--svg",
        svg.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["stats"]["layers"], 100);
    assert!(report["stats"]["filament_weight_g"].as_f64().unwrap() > 1.0);
    assert_eq!(report["warnings"].as_array().unwrap().len(), 0);

    let gcode = fs::read_to_string(dir.join("cube.gcode")).unwrap();
    assert!(gcode.contains(";TYPE:Sparse infill"));
    assert_eq!(fs::read_dir(&svg).unwrap().count(), 100);
    assert!(
        fs::read_to_string(svg.join("layer_0001.svg"))
            .unwrap()
            .starts_with("<svg")
    );
}

#[test]
fn config_file_and_overrides_are_applied() {
    let dir = workdir("config");
    let stl = dir.join("box.stl");
    fs::write(
        &stl,
        slicer_formats::write_stl_binary(&Mesh::cuboid(10.0, 10.0, 2.0)),
    )
    .unwrap();
    let config = dir.join("settings.toml");
    fs::write(
        &config,
        "layer_height = 0.1\nfirst_layer_height = 0.2\nflavor = \"klipper\"\n",
    )
    .unwrap();
    let output = dir.join("out.gcode");

    let out = run(&[
        "slice",
        stl.to_str().unwrap(),
        "-c",
        config.to_str().unwrap(),
        "-s",
        "nozzle_temp=230",
        "-o",
        output.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        text(&out.stdout).contains("layers:    19"),
        "{}",
        text(&out.stdout)
    );
    let gcode = fs::read_to_string(output).unwrap();
    assert!(gcode.contains("; flavor: klipper"));
    assert!(gcode.contains("M104 S230\n"));
}

#[test]
fn errors_use_exit_code_and_json() {
    let dir = workdir("errors");
    let broken = dir.join("broken.stl");
    fs::write(&broken, b"not an stl").unwrap();
    let good = dir.join("good.stl");
    fs::write(
        &good,
        slicer_formats::write_stl_binary(&Mesh::cuboid(5.0, 5.0, 5.0)),
    )
    .unwrap();

    let out = run(&["slice", broken.to_str().unwrap(), "--json"]);
    assert_eq!(out.status.code(), Some(1));
    let error: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["error"]["code"], "model.unrecognized");
    assert_eq!(error["error"]["severity"], "error");
    assert!(
        error["error"]["context"]["path"]
            .as_str()
            .unwrap()
            .ends_with("broken.stl")
    );
    assert!(!dir.join("broken.gcode").exists());

    let out = run(&["slice", dir.join("missing.stl").to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).starts_with("error: "));

    let out = run(&[
        "slice",
        good.to_str().unwrap(),
        "--json",
        "-s",
        "layer_height=5",
    ]);
    let error: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["error"]["code"], "settings.out_of_range");
    assert_eq!(error["error"]["context"]["key"], "layer_height");
    assert_eq!(error["error"]["context"]["value"], "5");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("layer_height")
    );

    let out = run(&[
        "slice",
        good.to_str().unwrap(),
        "--json",
        "-s",
        "bed_size_x=10",
        "-s",
        "max_print_height=4",
    ]);
    assert_eq!(out.status.code(), Some(1));
    let error: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["error"]["code"], "slicing.too_large");
}

#[test]
fn open_mesh_produces_a_warning() {
    let dir = workdir("open");
    let mut mesh = Mesh::cuboid(10.0, 10.0, 10.0);
    mesh.triangles.remove(2); // half of the top face
    let stl = dir.join("open.stl");
    fs::write(&stl, slicer_formats::write_stl_binary(&mesh)).unwrap();

    let out = run(&["slice", stl.to_str().unwrap(), "--json"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["warnings"].as_array().unwrap().len(), 1);
    assert_eq!(report["warnings"][0]["code"], "mesh.not_watertight");
    assert_eq!(report["warnings"][0]["severity"], "warning");
    assert_eq!(report["warnings"][0]["context"]["open_edges"], "3");
}

#[test]
fn default_config_is_valid_toml_accepted_by_slice() {
    let dir = workdir("defaults");
    let out = run(&["default-config"]);
    assert!(out.status.success());
    let config = dir.join("defaults.toml");
    fs::write(&config, &out.stdout).unwrap();
    let stl = dir.join("c.stl");
    fs::write(
        &stl,
        slicer_formats::write_stl_binary(&Mesh::cuboid(5.0, 5.0, 1.0)),
    )
    .unwrap();
    let out = run(&[
        "slice",
        stl.to_str().unwrap(),
        "-c",
        config.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
}

#[test]
fn a_3mf_file_gives_the_same_gcode_as_the_stl() {
    let dir = workdir("threemf");
    let mesh = Mesh::cuboid(20.0, 20.0, 20.0);
    let stl = dir.join("stl.stl");
    fs::write(&stl, slicer_formats::write_stl_binary(&mesh)).unwrap();
    let threemf = dir.join("threemf.3mf");
    fs::write(&threemf, slicer_formats::write_3mf(&mesh).unwrap()).unwrap();

    for file in [&stl, &threemf] {
        let out = run(&["slice", file.to_str().unwrap(), "--json"]);
        assert!(out.status.success(), "{}", text(&out.stderr));
    }
    assert_eq!(
        fs::read(dir.join("stl.gcode")).unwrap(),
        fs::read(dir.join("threemf.gcode")).unwrap()
    );

    // A ZIP archive that is not a 3MF package is rejected with a code.
    let broken = dir.join("broken.3mf");
    fs::write(&broken, b"PK\x03\x04garbage").unwrap();
    let out = run(&["slice", broken.to_str().unwrap(), "--json"]);
    assert!(!out.status.success());
    let report: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(report["error"]["code"], "model.archive");
}
