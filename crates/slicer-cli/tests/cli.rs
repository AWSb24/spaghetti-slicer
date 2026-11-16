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
    // A second file overrides the first and keeps what it does not name.
    let material = dir.join("material.toml");
    fs::write(&material, "flavor = \"marlin\"\nbed_temp = 75.0\n").unwrap();
    let output = dir.join("out.gcode");

    let out = run(&[
        "slice",
        stl.to_str().unwrap(),
        "-c",
        config.to_str().unwrap(),
        "-c",
        material.to_str().unwrap(),
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
    assert!(gcode.contains("; flavor: marlin"));
    assert!(gcode.contains("; bed_temp = 75"));
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
fn output_named_gcode_3mf_is_a_project_file() {
    let dir = workdir("container");
    let stl = dir.join("box.stl");
    fs::write(
        &stl,
        slicer_formats::write_stl_binary(&Mesh::cuboid(10.0, 10.0, 2.0)),
    )
    .unwrap();
    let plain = dir.join("box.gcode");
    let packed = dir.join("box.gcode.3mf");
    for output in [&plain, &packed] {
        let out = run(&[
            "slice",
            stl.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
        ]);
        assert!(out.status.success(), "{}", text(&out.stderr));
    }
    let gcode = fs::read(&plain).unwrap();
    let package = fs::read(&packed).unwrap();
    // A ZIP archive, smaller than the text it holds, and not the text itself.
    assert!(slicer_formats::looks_like_3mf(&package));
    assert!(gcode.starts_with(b"; generated by spaghetti-slicer"));
    assert!(package.len() < gcode.len());
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
    assert_eq!(report["warnings"][0]["code"], "mesh.repaired");
    assert_eq!(report["warnings"][0]["severity"], "warning");
    assert_eq!(report["warnings"][0]["context"]["holes"], "1");
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

#[test]
fn orca_profiles_become_a_settings_file() {
    let dir = workdir("orca");
    let printer = dir.join("printer.json");
    let process = dir.join("process.json");
    fs::write(
        &printer,
        r#"{"name": "Printer", "inherits": "Base printer", "printable_area": ["0x0", "180x0", "180x180", "0x180"], "nozzle_diameter": ["0.4"]}"#,
    )
    .unwrap();
    fs::write(
        &process,
        r#"{"name": "Process", "layer_height": "0.12", "sparse_infill_density": "20%", "ironing_type": "top"}"#,
    )
    .unwrap();
    let settings = dir.join("imported.toml");
    let out = run(&[
        "import-orca",
        printer.to_str().unwrap(),
        process.to_str().unwrap(),
        "-o",
        settings.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let report = text(&out.stderr);
    assert!(
        report.contains("ironing_type: no matching setting"),
        "{report}"
    );
    assert!(
        report.contains("'Base printer' are not included"),
        "{report}"
    );

    // The file works as a settings file.
    let stl = dir.join("cube.stl");
    fs::write(
        &stl,
        slicer_formats::write_stl_binary(&Mesh::cuboid(10.0, 10.0, 1.2)),
    )
    .unwrap();
    let out = run(&[
        "slice",
        stl.to_str().unwrap(),
        "-c",
        settings.to_str().unwrap(),
        "--json",
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["stats"]["layers"], 9);
    let gcode = fs::read_to_string(dir.join("cube.gcode")).unwrap();
    assert!(
        gcode.contains("bed_size_x = 180"),
        "settings are listed in the header"
    );

    // A file that is no profile is an error.
    let out = run(&["import-orca", stl.to_str().unwrap()]);
    assert!(!out.status.success());
}

#[test]
fn slices_a_cube_for_a_resin_printer() {
    let dir = workdir("sla");
    let stl = dir.join("cube.stl");
    fs::write(
        &stl,
        slicer_formats::write_stl_binary(&Mesh::cuboid(10.0, 10.0, 10.0)),
    )
    .unwrap();

    let out = run(&[
        "slice-sla",
        stl.to_str().unwrap(),
        "--set",
        "layer_height=0.1",
        "--set",
        "bottom_layers=4",
        "--json",
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["stats"]["layers"], 100);
    let resin = report["stats"]["resin_volume_ml"].as_f64().unwrap();
    assert!((resin - 1.0).abs() < 0.01, "{resin}");
    // 4 x 40 s + 96 x 2 s exposure, 100 x 5 s plate moves.
    assert_eq!(report["stats"]["print_time_s"], 852.0);
    let archive = fs::read(dir.join("cube.sla.zip")).unwrap();
    assert_eq!(&archive[..2], b"PK");

    let out = run(&["slice-sla", stl.to_str().unwrap(), "-s", "nozzle_temp=200"]);
    assert!(!out.status.success());
    assert!(text(&out.stderr).contains("unknown resin setting 'nozzle_temp'"));
}

#[test]
fn sliced_gcode_is_sent_to_a_printer() {
    // A stand-in for Moonraker on 127.0.0.1; no real printer is contacted.
    let printer = slicer_send::mock::Printer::start(201, r#"{"print_started": true}"#).unwrap();
    let dir = workdir("send");
    let stl = dir.join("cube.stl");
    fs::write(
        &stl,
        slicer_formats::write_stl_binary(&Mesh::cuboid(10.0, 10.0, 2.0)),
    )
    .unwrap();
    let output = dir.join("cube.gcode.3mf");
    let target = format!("moonraker://{}", printer.address);
    let out = bin()
        .args([
            "slice",
            stl.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
        ])
        .args(["--send", &target, "--start", "--json"])
        .env("SPAGHETTI_PRINTER_API_KEY", "key-from-env")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        report["sent"],
        serde_json::json!({ "printer": target, "file": "cube.gcode", "started": true })
    );

    // The printer got plain G-code, the same as a .gcode output.
    let request = printer.request().unwrap();
    assert_eq!(request.path, "/server/files/upload");
    assert_eq!(request.header("x-api-key"), Some("key-from-env"));
    assert_eq!(request.field("print").unwrap(), b"true");
    let gcode = request.field("file").unwrap();
    assert!(text(&gcode).contains(";LAYER_CHANGE"));
    assert!(slicer_formats::looks_like_3mf(&fs::read(&output).unwrap()));
    let plain = dir.join("cube.gcode");
    let out = run(&[
        "slice",
        stl.to_str().unwrap(),
        "-o",
        plain.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(gcode, fs::read(&plain).unwrap());

    // The send command uploads an existing file; --api-key wins over the
    // environment.
    let printer = slicer_send::mock::Printer::start(201, r#"{"effectivePrint": false}"#).unwrap();
    let file = dir.join("part.gcode");
    fs::write(&file, "G28\n").unwrap();
    let out = bin()
        .args(["send", file.to_str().unwrap(), "--to"])
        .arg(format!("octoprint://{}", printer.address))
        .args(["--api-key", "key-from-option"])
        .env("SPAGHETTI_PRINTER_API_KEY", "key-from-env")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("Sent part.gcode to octoprint://"));
    let request = printer.request().unwrap();
    assert_eq!(request.path, "/api/files/local");
    assert_eq!(request.header("x-api-key"), Some("key-from-option"));
    assert_eq!(request.field("print").unwrap(), b"false");
    assert_eq!(request.field("file").unwrap(), b"G28\n");
}

#[test]
fn send_errors_have_codes() {
    let dir = workdir("send-errors");
    let file = dir.join("part.gcode");
    fs::write(&file, "G28\n").unwrap();
    let send = |target: &str| {
        let out = run(&["send", file.to_str().unwrap(), "--to", target, "--json"]);
        assert!(!out.status.success());
        let error: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
        error["error"]["code"].as_str().unwrap().to_string()
    };
    assert_eq!(send("http://printer"), "send.invalid_target");

    let printer = slicer_send::mock::Printer::start(401, "{}").unwrap();
    assert_eq!(
        send(&format!("moonraker://{}", printer.address)),
        "send.unauthorized"
    );
    let printer = slicer_send::mock::Printer::start(413, "{}").unwrap();
    assert_eq!(
        send(&format!("moonraker://{}", printer.address)),
        "send.too_large"
    );

    // An unreachable printer: the address is checked before slicing, the
    // connection after the G-code is written.
    let address = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    assert_eq!(send(&format!("moonraker://{address}")), "send.connection");
}

/// X and Y of the outer wall moves of layer `index` (from 0).
fn outer_wall_points(gcode: &str, index: usize) -> Vec<[f64; 2]> {
    let layer = gcode.split(";LAYER_CHANGE").nth(index + 1).unwrap();
    let mut points = Vec::new();
    for section in layer.split(";TYPE:").skip(1) {
        if !section.starts_with("Outer wall") {
            continue;
        }
        for line in section
            .lines()
            .filter(|l| l.starts_with("G1 ") && l.contains(" E"))
        {
            let value = |axis: char| {
                line.split(' ')
                    .find_map(|word| word.strip_prefix(axis))
                    .map(|v| v.parse::<f64>().unwrap())
            };
            if let (Some(x), Some(y)) = (value('X'), value('Y')) {
                points.push([x, y]);
            }
        }
    }
    points
}

/// Smallest and largest X of the points left and right of `split`.
fn x_ranges(points: &[[f64; 2]], split: f64) -> [[f64; 2]; 2] {
    let range = |left: bool| {
        points
            .iter()
            .map(|p| p[0])
            .filter(|&x| (x < split) == left)
            .fold([f64::MAX, f64::MIN], |r, x| [r[0].min(x), r[1].max(x)])
    };
    [range(true), range(false)]
}

#[test]
fn several_models_are_arranged_side_by_side() {
    let dir = workdir("arrange");
    let cube = slicer_formats::write_stl_binary(&Mesh::cuboid(20.0, 20.0, 20.0));
    let (a, b) = (dir.join("a.stl"), dir.join("b.stl"));
    fs::write(&a, &cube).unwrap();
    fs::write(&b, &cube).unwrap();
    let plate = dir.join("plate.gcode");
    let out = run(&[
        "slice",
        a.to_str().unwrap(),
        b.to_str().unwrap(),
        "-o",
        plate.to_str().unwrap(),
        "--json",
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["warnings"].as_array().unwrap().len(), 0);
    let gcode = fs::read_to_string(&plate).unwrap();
    // Two 20 mm cubes 5 mm apart, centred on the 220 mm bed: X 87.5 to
    // 107.5 and 112.5 to 132.5. Outer walls lie half a line width inside.
    for layer in [0, 99] {
        let points = outer_wall_points(&gcode, layer);
        let [left, right] = x_ranges(&points, 110.0);
        for (range, expected) in [(left, [87.725, 107.275]), (right, [112.725, 132.275])] {
            assert!(
                (range[0] - expected[0]).abs() < 1e-3 && (range[1] - expected[1]).abs() < 1e-3,
                "layer {layer}: {range:?}"
            );
        }
        assert!(points.iter().all(|p| (p[1] - 110.0).abs() < 10.0));
    }

    // Kept where the files put them, both cubes stand at the origin.
    let out = run(&[
        "slice",
        a.to_str().unwrap(),
        b.to_str().unwrap(),
        "--keep-positions",
        "--json",
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["warnings"][0]["code"], "slicing.objects_overlap");

    // A plate that does not fit names the file that ends up outside.
    let out = run(&[
        "slice",
        a.to_str().unwrap(),
        b.to_str().unwrap(),
        "-s",
        "bed_size_x=40",
        "-s",
        "bed_size_y=40",
        "--json",
    ]);
    assert!(!out.status.success());
    let error: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["error"]["code"], "slicing.outside_bed");
    assert!(
        error["error"]["context"]["path"]
            .as_str()
            .unwrap()
            .ends_with("b.stl")
    );
}

#[test]
fn build_items_of_a_3mf_can_keep_their_positions() {
    let dir = workdir("keep");
    let cube = Mesh::cuboid(10.0, 10.0, 10.0);
    let vertices: String = cube
        .vertices
        .iter()
        .map(|[x, y, z]| format!("<vertex x=\"{x}\" y=\"{y}\" z=\"{z}\"/>"))
        .collect();
    let triangles: String = cube
        .triangles
        .iter()
        .map(|[a, b, c]| format!("<triangle v1=\"{a}\" v2=\"{b}\" v3=\"{c}\"/>"))
        .collect();
    let model = format!(
        "<?xml version=\"1.0\"?><model unit=\"millimeter\" xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\"><resources><object id=\"1\" type=\"model\"><mesh><vertices>{vertices}</vertices><triangles>{triangles}</triangles></mesh></object></resources><build><item objectid=\"1\" transform=\"1 0 0 0 1 0 0 0 1 30 40 0\"/><item objectid=\"1\" transform=\"1 0 0 0 1 0 0 0 1 150 160 0\"/></build></model>"
    );
    let file = dir.join("two.3mf");
    fs::write(
        &file,
        slicer_formats::write_package(&[("3D/3dmodel.model", &model)]).unwrap(),
    )
    .unwrap();

    let kept = dir.join("kept.gcode");
    let out = run(&[
        "slice",
        file.to_str().unwrap(),
        "--keep-positions",
        "-o",
        kept.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let points = outer_wall_points(&fs::read_to_string(&kept).unwrap(), 0);
    let [left, right] = x_ranges(&points, 100.0);
    assert!((left[0] - 30.225).abs() < 1e-3 && (left[1] - 39.775).abs() < 1e-3);
    assert!((right[0] - 150.225).abs() < 1e-3 && (right[1] - 159.775).abs() < 1e-3);

    // Without the option the items move together to the middle of the bed:
    // the pair spans 130 mm, so it starts at X 45.
    let out = run(&["slice", file.to_str().unwrap()]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let points = outer_wall_points(&fs::read_to_string(dir.join("two.gcode")).unwrap(), 0);
    let [left, right] = x_ranges(&points, 100.0);
    assert!((left[0] - 45.225).abs() < 1e-3, "{left:?}");
    assert!((right[1] - 174.775).abs() < 1e-3, "{right:?}");

    // Kept positions must lie on the bed.
    let out = run(&[
        "slice",
        file.to_str().unwrap(),
        "--keep-positions",
        "-s",
        "bed_size_x=100",
        "--json",
    ]);
    assert!(!out.status.success());
    let error: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["error"]["code"], "slicing.outside_bed");
}
