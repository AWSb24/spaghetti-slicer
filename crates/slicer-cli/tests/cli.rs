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

/// Decodes standard base64 with padding.
fn from_base64(text: &str) -> Vec<u8> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => panic!("not base64: {c}"),
    };
    let mut bytes = Vec::new();
    for group in text.as_bytes().chunks(4) {
        let digits: Vec<u8> = group
            .iter()
            .filter(|&&c| c != b'=')
            .map(|&c| value(c))
            .collect();
        let bits = digits
            .iter()
            .enumerate()
            .fold(0u32, |bits, (i, &d)| bits | u32::from(d) << (18 - 6 * i));
        bytes.extend(&bits.to_be_bytes()[1..digits.len()]);
    }
    bytes
}

#[test]
fn thumbnails_go_into_the_header_and_change_nothing_else() {
    let dir = workdir("thumbnails");
    let stl = dir.join("box.stl");
    fs::write(
        &stl,
        slicer_formats::write_stl_binary(&Mesh::cuboid(20.0, 10.0, 5.0)),
    )
    .unwrap();
    let plain = dir.join("plain.gcode");
    let pictured = dir.join("pictured.gcode");
    let mut reports = Vec::new();
    for (output, sizes) in [(&plain, ""), (&pictured, "thumbnails=16x16, 40x30")] {
        let mut args = vec![
            "slice",
            stl.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--json",
        ];
        if !sizes.is_empty() {
            args.extend(["--set", sizes]);
        }
        let out = run(&args);
        assert!(out.status.success(), "{}", text(&out.stderr));
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        reports.push(report["stats"].clone());
    }
    // Comments only: the same figures.
    assert_eq!(reports[0], reports[1]);
    let plain = fs::read_to_string(&plain).unwrap();
    let pictured = fs::read_to_string(&pictured).unwrap();
    assert!(pictured.starts_with("; generated by spaghetti-slicer"));

    let mut rest = Vec::new();
    let mut sizes = Vec::new();
    let mut lines = pictured.lines();
    while let Some(line) = lines.next() {
        let Some(head) = line.strip_prefix("; thumbnail begin ") else {
            rest.push(line);
            continue;
        };
        let (size, length) = head.split_once(' ').unwrap();
        let mut encoded = String::new();
        for line in lines.by_ref() {
            if line == "; thumbnail end" {
                break;
            }
            let chunk = line.strip_prefix("; ").unwrap();
            assert!(chunk.len() <= 78, "{line}");
            encoded.push_str(chunk);
        }
        assert_eq!(encoded.len().to_string(), length);
        let png = from_base64(&encoded);
        let (width, height, _) = slicer_core::png::header(&png).unwrap();
        assert_eq!(format!("{width}x{height}"), size);
        sizes.push(size.to_owned());
    }
    assert_eq!(sizes, ["16x16", "40x30"]);
    // Apart from the blocks, their blank comment lines and the setting
    // itself, the files are the same.
    let rest: Vec<&str> = rest
        .into_iter()
        .filter(|line| *line != ";" && !line.starts_with("; thumbnails ="))
        .collect();
    let plain: Vec<&str> = plain
        .lines()
        .filter(|line| *line != ";" && !line.starts_with("; thumbnails ="))
        .collect();
    assert_eq!(rest, plain);
}

#[test]
fn gcode_3mf_holds_pictures_of_the_plate() {
    let dir = workdir("container-pictures");
    let stl = dir.join("box.stl");
    fs::write(
        &stl,
        slicer_formats::write_stl_binary(&Mesh::cuboid(10.0, 10.0, 2.0)),
    )
    .unwrap();
    let packed = dir.join("box.gcode.3mf");
    let out = run(&[
        "slice",
        stl.to_str().unwrap(),
        "-o",
        packed.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let package = fs::read(&packed).unwrap();
    // The pictures are stored uncompressed, so their headers can be found.
    let headers: Vec<(u32, u32)> = package
        .windows(8)
        .enumerate()
        .filter(|(_, window)| *window == b"\x89PNG\r\n\x1a\n")
        .filter_map(|(at, _)| slicer_core::png::header(&package[at..]))
        .map(|(width, height, _)| (width, height))
        .collect();
    assert_eq!(headers, [(512, 512), (128, 128)]);
    let contains = |name: &[u8]| package.windows(name.len()).any(|w| w == name);
    assert!(contains(b"Metadata/plate_1.png"));
    assert!(contains(b"Metadata/plate_1_small.png"));
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
fn outlines_and_turns_arrange_closer() {
    let dir = workdir("arrange_outlines");
    let triangle = slicer_core::primitives::extrude(
        &[[0.0, 0.0], [60.0, 0.0], [0.0, 60.0]],
        [20.0, 20.0],
        5.0,
        1,
        0.0,
    );
    let (a, b) = (dir.join("a.stl"), dir.join("b.stl"));
    fs::write(&a, slicer_formats::write_stl_binary(&triangle)).unwrap();
    fs::write(&b, slicer_formats::write_stl_binary(&triangle)).unwrap();
    // Two cubes 10 mm apart in one file: 30 by 10 mm together.
    let cube = Mesh::cuboid(10.0, 10.0, 5.0);
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
        "<?xml version=\"1.0\"?><model unit=\"millimeter\" xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\"><resources><object id=\"1\" type=\"model\"><mesh><vertices>{vertices}</vertices><triangles>{triangles}</triangles></mesh></object></resources><build><item objectid=\"1\" transform=\"1 0 0 0 1 0 0 0 1 0 0 0\"/><item objectid=\"1\" transform=\"1 0 0 0 1 0 0 0 1 20 0 0\"/></build></model>"
    );
    let pair = dir.join("pair.3mf");
    fs::write(
        &pair,
        slicer_formats::write_package(&[("3D/3dmodel.model", &model)]).unwrap(),
    )
    .unwrap();
    let slice = |files: [&PathBuf; 2], settings: &[&str]| {
        let mut command = bin();
        command.args([
            "slice",
            "--json",
            "-s",
            "bed_size_x=72",
            "-s",
            "bed_size_y=72",
        ]);
        command.args(files).arg("-o").arg(dir.join("plate.gcode"));
        for setting in settings {
            command.args(["-s", setting]);
        }
        command.output().unwrap()
    };

    // In rows of rectangles neither pair fits the 72 mm bed.
    for files in [[&a, &b], [&a, &pair]] {
        let out = slice(files, &[]);
        assert!(!out.status.success());
        let error: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
        assert_eq!(error["error"]["code"], "slicing.outside_bed");
    }
    // By outlines the cubes stand beside the slanted side of the triangle.
    let out = slice([&a, &pair], &["arrange_outlines=true"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["warnings"].as_array().unwrap().len(), 0);
    // Two triangles need a half turn as well.
    let out = slice([&a, &b], &["arrange_outlines=true"]);
    assert!(!out.status.success());
    let out = slice([&a, &b], &["arrange_rotation=true"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["warnings"].as_array().unwrap().len(), 0);
}

#[test]
fn an_object_gets_settings_of_its_own() {
    let dir = workdir("object_settings");
    let cube = slicer_formats::write_stl_binary(&Mesh::cuboid(10.0, 10.0, 4.0));
    let (a, b) = (dir.join("a.stl"), dir.join("b.stl"));
    fs::write(&a, &cube).unwrap();
    fs::write(&b, &cube).unwrap();
    let dense = dir.join("dense.toml");
    fs::write(&dense, "infill_density = 40\n").unwrap();
    // The plate's own value.
    let same = dir.join("same.toml");
    fs::write(&same, "infill_density = 15\n").unwrap();
    let slice = |name: &str, objects: &[&PathBuf]| {
        let output = dir.join(name);
        let arguments: Vec<String> = objects
            .iter()
            .flat_map(|file| ["--object".to_string(), format!("2:{}", file.display())])
            .collect();
        let out = bin()
            .args(["slice", a.to_str().unwrap(), b.to_str().unwrap(), "-o"])
            .arg(&output)
            .args(&arguments)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", text(&out.stderr));
        fs::read_to_string(output).unwrap()
    };
    let plain = slice("plain.gcode", &[]);
    assert_ne!(plain, slice("dense.gcode", &[&dense]));
    // Files for the same object are layered in order; values equal to the
    // plate's print as without settings of its own.
    assert_eq!(plain, slice("same.gcode", &[&dense, &same]));
}

#[test]
fn object_settings_are_checked() {
    let dir = workdir("object_settings_errors");
    let stl = dir.join("cube.stl");
    fs::write(
        &stl,
        slicer_formats::write_stl_binary(&Mesh::cuboid(10.0, 10.0, 4.0)),
    )
    .unwrap();
    let layers = dir.join("layers.toml");
    fs::write(&layers, "wall_loops = 3\nlayer_height = 0.1\n").unwrap();
    let object = format!("1:{}", layers.display());
    let out = run(&["slice", stl.to_str().unwrap(), "--object", &object]);
    assert!(!out.status.success());
    let message = text(&out.stderr);
    assert!(
        message.contains("'layer_height' cannot be set per object")
            && message.contains("wall_loops"),
        "{message}"
    );

    let out = run(&[
        "slice",
        stl.to_str().unwrap(),
        "--object",
        &object,
        "--json",
    ]);
    let error: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["error"]["code"], "settings.not_per_object");
    assert_eq!(error["error"]["context"]["key"], "layer_height");

    // There is no second object.
    let walls = dir.join("walls.toml");
    fs::write(&walls, "wall_loops = 3\n").unwrap();
    let object = format!("2:{}", walls.display());
    let out = run(&[
        "slice",
        stl.to_str().unwrap(),
        "--object",
        &object,
        "--json",
    ]);
    let error: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["error"]["code"], "cli.invalid_object");
}

#[test]
fn height_ranges_get_settings_of_their_own() {
    let dir = workdir("range_settings");
    let cube = slicer_formats::write_stl_binary(&Mesh::cuboid(10.0, 10.0, 8.0));
    let (a, b) = (dir.join("a.stl"), dir.join("b.stl"));
    fs::write(&a, &cube).unwrap();
    fs::write(&b, &cube).unwrap();
    let dense = dir.join("dense.toml");
    fs::write(&dense, "infill_density = 100\n").unwrap();
    // The plate's own value.
    let same = dir.join("same.toml");
    fs::write(&same, "infill_density = 15\n").unwrap();
    let layers = dir.join("layers.toml");
    fs::write(&layers, "layer_height = 0.1\n").unwrap();
    let slice = |name: &str, ranges: &[String]| {
        let output = dir.join(name);
        let mut command = bin();
        command
            .args(["slice", a.to_str().unwrap(), b.to_str().unwrap(), "-o"])
            .arg(&output)
            .arg("--json");
        for range in ranges {
            command.arg(format!("--range={range}"));
        }
        let out = command.output().unwrap();
        let text = fs::read_to_string(&output).unwrap_or_default();
        (out, text)
    };
    let range = |prefix: &str, file: &PathBuf| format!("{prefix}{}", file.display());
    let (out, plain) = slice("plain.gcode", &[]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let (_, all) = slice("all.gcode", &[range("4-8:", &dense)]);
    let (_, one) = slice("one.gcode", &[range("2:4-8:", &dense)]);
    assert_ne!(plain, all);
    assert_ne!(plain, one);
    assert_ne!(all, one);
    // A range with the object's own values prints as without it.
    let (_, unchanged) = slice("same.gcode", &[range("1:2-6:", &same)]);
    assert_eq!(plain, unchanged);

    let error = |ranges: &[String]| {
        let (out, _) = slice("error.gcode", ranges);
        assert!(!out.status.success());
        let error: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
        error["error"].clone()
    };
    let wrong = error(&[range("4-8:", &layers)]);
    assert_eq!(wrong["code"], "settings.not_per_range");
    assert_eq!(wrong["context"]["key"], "layer_height");
    for prefix in ["3:4-8:", "0:4-8:", "8-4:", "x:4-8:", "4:", "-1-4:"] {
        let wrong = error(&[range(prefix, &dense)]);
        assert_eq!(wrong["code"], "cli.invalid_range", "{prefix}");
    }
}

#[test]
fn modifiers_change_the_settings_inside_them() {
    let dir = workdir("modifiers");
    let a = dir.join("a.stl");
    fs::write(
        &a,
        slicer_formats::write_stl_binary(&Mesh::cuboid(20.0, 20.0, 20.0)),
    )
    .unwrap();
    // A 10 mm cube in the middle of the model, in the model's coordinates.
    let mut core = Mesh::cuboid(10.0, 10.0, 10.0);
    core.translate([5.0, 5.0, 5.0]);
    let mesh = dir.join("core.stl");
    fs::write(&mesh, slicer_formats::write_stl_binary(&core)).unwrap();
    let dense = dir.join("dense.toml");
    fs::write(&dense, "infill_density = 100\n").unwrap();
    let same = dir.join("same.toml");
    fs::write(&same, "infill_density = 15\n").unwrap();
    let walls = dir.join("walls.toml");
    fs::write(&walls, "wall_loops = 4\n").unwrap();
    let slice = |name: &str, modifiers: &[String]| {
        let output = dir.join(name);
        let mut command = bin();
        command
            .args(["slice", a.to_str().unwrap(), "-o"])
            .arg(&output)
            .arg("--json");
        for modifier in modifiers {
            command.arg(format!("--modifier={modifier}"));
        }
        let out = command.output().unwrap();
        let text = fs::read_to_string(&output).unwrap_or_default();
        (out, text)
    };
    let modifier = |prefix: &str, settings: &PathBuf| {
        format!("{prefix}{}:{}", mesh.display(), settings.display())
    };
    let (out, plain) = slice("plain.gcode", &[]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let (out, dense_gcode) = slice("dense.gcode", &[modifier("1:", &dense)]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_ne!(plain, dense_gcode);
    // More solid infill, the same number of layers.
    let solid = |gcode: &str| gcode.matches(";TYPE:Solid infill").count();
    assert!(solid(&dense_gcode) > solid(&plain));
    // A modifier with the object's own values prints as without it.
    let (_, unchanged) = slice("same.gcode", &[modifier("1:", &same)]);
    assert_eq!(plain, unchanged);

    let error = |modifiers: &[String]| {
        let (out, _) = slice("error.gcode", modifiers);
        assert!(!out.status.success());
        let error: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
        error["error"].clone()
    };
    let wrong = error(&[modifier("1:", &walls)]);
    assert_eq!(wrong["code"], "settings.not_per_modifier");
    assert_eq!(wrong["context"]["key"], "wall_loops");
    for prefix in ["2:", "0:", "x:", ""] {
        let wrong = error(&[modifier(prefix, &dense)]);
        assert_eq!(wrong["code"], "cli.invalid_modifier", "{prefix}");
    }
    let wrong = error(&[format!("1:{}", dense.display())]);
    assert_eq!(wrong["code"], "cli.invalid_modifier");
    let missing = error(&[format!(
        "1:{}:{}",
        dir.join("none.stl").display(),
        dense.display()
    )]);
    assert_eq!(missing["code"], "io.read");
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

#[test]
fn writes_calibration_prints() {
    let dir = workdir("calibrate");
    let tower = dir.join("tower.gcode");
    let out = run(&[
        "calibrate",
        "temperature",
        "--start",
        "220",
        "--end",
        "200",
        "--step",
        "10",
        "-o",
        tower.to_str().unwrap(),
        "--json",
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        report["calibration"]["values"],
        serde_json::json!([220.0, 210.0, 200.0])
    );
    // Three blocks of 10 mm.
    assert_eq!(report["stats"]["layers"], 150);
    let gcode = fs::read_to_string(&tower).unwrap();
    assert!(gcode.contains("M109 S220\n"));
    assert!(gcode.contains("\nM104 S210\n") && gcode.contains("\nM104 S200\n"));

    // Negative values for the flow test; without -o the file goes into the
    // working directory.
    let out = bin()
        .current_dir(&dir)
        .args(["calibrate", "flow", "--start", "-5", "--end", "5"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let summary = text(&out.stdout);
    assert!(summary.contains("Flow -5 % at X"), "{summary}");
    assert!(dir.join("calibration-flow.gcode").exists());

    let out = run(&["calibrate", "retraction", "--step", "0", "--json"]);
    assert!(!out.status.success());
    let error: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["error"]["code"], "calibration.invalid_range");
    assert!(!run(&["calibrate", "pressure"]).status.success());
}

/// A project file as Orca Slicer and Bambu Studio write it, reduced to
/// what is read: two cubes (objects 1 and 2), the project's settings and,
/// with `object_settings`, settings of object 2 of its own.
fn orca_project(object_settings: bool) -> Vec<u8> {
    orca_project_on(object_settings, 1)
}

/// [`orca_project`] with the right object on extruder `right` (from 1).
fn orca_project_on(object_settings: bool, right: u32) -> Vec<u8> {
    let cube = Mesh::cuboid(20.0, 20.0, 6.0);
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
    let mesh =
        format!("<mesh><vertices>{vertices}</vertices><triangles>{triangles}</triangles></mesh>");
    let model = format!(
        "<?xml version=\"1.0\"?><model unit=\"millimeter\" xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\"><resources><object id=\"1\" type=\"model\">{mesh}</object><object id=\"2\" type=\"model\">{mesh}</object></resources><build><item objectid=\"1\" transform=\"1 0 0 0 1 0 0 0 1 80 100 0\"/><item objectid=\"2\" transform=\"1 0 0 0 1 0 0 0 1 140 100 0\"/></build></model>"
    );
    let config = r#"{
    "name": "project_settings",
    "from": "project",
    "layer_height": "0.25",
    "initial_layer_print_height": "0.25",
    "wall_loops": "3",
    "sparse_infill_density": ["15%"],
    "brim_type": "no_brim",
    "brim_width": "5",
    "flush_volumes_matrix": ["0", "50", "200", "0"],
    "flush_multiplier": ["1"],
    "made_up_setting": "7"
}"#;
    let own = if object_settings {
        "<metadata key=\"sparse_infill_density\" value=\"60%\"/><metadata key=\"layer_height\" value=\"0.1\"/><part id=\"1\" subtype=\"normal_part\"><metadata key=\"name\" value=\"Cube\"/></part><part id=\"2\" subtype=\"modifier_part\"><metadata key=\"wall_loops\" value=\"6\"/></part>"
    } else {
        ""
    };
    let settings = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><config><object id=\"1\"><metadata key=\"name\" value=\"Left\"/><metadata key=\"extruder\" value=\"1\"/></object><object id=\"2\"><metadata key=\"name\" value=\"Right\"/><metadata key=\"extruder\" value=\"{right}\"/>{own}</object><plate><metadata key=\"plater_id\" value=\"1\"/><model_instance><metadata key=\"object_id\" value=\"1\"/></model_instance><model_instance><metadata key=\"object_id\" value=\"2\"/></model_instance></plate></config>"
    );
    slicer_formats::write_package(&[
        ("3D/3dmodel.model", &model),
        ("Metadata/project_settings.config", config),
        ("Metadata/model_settings.config", &settings),
    ])
    .unwrap()
}

#[test]
fn flush_volumes_of_a_project_are_purged_and_counted() {
    let dir = workdir("flush_volumes");
    let project = dir.join("two.3mf");
    fs::write(&project, orca_project_on(false, 2)).unwrap();
    let slice = |args: &[&str]| {
        let out = bin()
            .arg("slice")
            .arg(&project)
            .arg("-o")
            .arg(dir.join("two.gcode"))
            .args(["--set", "extruder_count=2", "--set", "prime_tower_width=30"])
            .args(["--set", "prime_tower_x=180", "--set", "prime_tower_y=180"])
            .args(args)
            .output()
            .unwrap();
        (out.status.success(), text(&out.stdout), text(&out.stderr))
    };
    let (ok, stdout, stderr) = slice(&["--project-settings", "--json"]);
    assert!(ok, "{stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    // 24 layers, one change each: 12 of 50 mm³ and 12 of 200 mm³.
    let flush = report["stats"]["flush_volume_cm3"].as_f64().unwrap();
    assert!((flush / 3.0 - 1.0).abs() < 0.05, "{flush}");
    assert!(report["stats"]["flush_weight_g"].as_f64().unwrap() > 3.0);
    let (ok, stdout, _) = slice(&["--project-settings"]);
    assert!(ok);
    assert!(
        stdout.contains("of that into the prime tower: 3."),
        "{stdout}"
    );

    // A table that does not fit is refused with the setting named.
    let (ok, _, stderr) = slice(&["--set", "flush_volumes=0,50,200", "--json"]);
    assert!(!ok);
    let report: serde_json::Value = serde_json::from_str(&stderr).unwrap();
    assert_eq!(report["error"]["code"], "settings.parse");
    let message = report["error"]["message"].as_str().unwrap();
    assert!(message.contains("flush_volumes: 3 values"), "{message}");
}

#[test]
fn settings_stored_in_a_project_file_are_used_on_request() {
    let dir = workdir("project_settings");
    let project = dir.join("project.3mf");
    fs::write(&project, orca_project(true)).unwrap();
    let plain = dir.join("plain.3mf");
    fs::write(&plain, orca_project(false)).unwrap();
    let dense = dir.join("dense.toml");
    fs::write(&dense, "infill_density = 60\n").unwrap();
    let sparse = dir.join("sparse.toml");
    fs::write(&sparse, "infill_density = 15\n").unwrap();
    let slice = |name: &str, model: &PathBuf, args: &[&str]| {
        let output = dir.join(name);
        let out = bin()
            .arg("slice")
            .arg(model)
            .arg("-o")
            .arg(&output)
            .args(args)
            .arg("--json")
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", text(&out.stderr));
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        (fs::read_to_string(output).unwrap(), report)
    };
    let codes = |report: &serde_json::Value| -> Vec<String> {
        report["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["code"].as_str().unwrap().to_string())
            .collect()
    };

    // Without the option the stored settings are ignored.
    let (without, report) = slice("without.gcode", &project, &[]);
    assert!(without.contains("; layer_height = 0.2\n"));
    assert!(codes(&report).iter().all(|c| !c.starts_with("project.")));
    assert_eq!(without, slice("plain.gcode", &plain, &[]).0);

    // With it the project's settings apply; what has no counterpart is
    // reported, for the plate and for the object.
    let (with, report) = slice("with.gcode", &project, &["--project-settings"]);
    assert!(with.contains("; layer_height = 0.25\n"), "{with}");
    assert!(with.contains("; wall_loops = 3\n"));
    assert!(with.contains("; brim_width = 0\n"));
    assert!(with.contains("; flush_volumes = 0,50;200,0\n"), "{with}");
    let warnings = report["warnings"].as_array().unwrap();
    let skipped = warnings
        .iter()
        .find(|w| w["code"] == "project.settings_skipped")
        .unwrap();
    assert_eq!(skipped["context"]["keys"], "made_up_setting");
    let object = warnings
        .iter()
        .find(|w| w["code"] == "project.object_settings_skipped")
        .unwrap();
    assert_eq!(object["context"]["keys"], "layer_height");
    assert!(object["message"].as_str().unwrap().contains("'Right'"));
    assert!(codes(&report).contains(&"project.modifier_settings_skipped".to_string()));

    // The object's own infill makes a difference ...
    let (plate_only, _) = slice("plate_only.gcode", &plain, &["--project-settings"]);
    assert_ne!(with, plate_only);
    // ... and is 60 %: with the plate at 60 % too, both objects print alike.
    let set = ["--project-settings", "--set", "infill_density=60"];
    assert_eq!(
        slice("all_dense.gcode", &project, &set).0,
        slice("all_dense_plain.gcode", &plain, &set).0
    );
    // --object goes over the object's settings from the project.
    let object = format!("1:{}", sparse.display());
    assert_eq!(
        slice(
            "object.gcode",
            &project,
            &["--project-settings", "--object", &object]
        )
        .0,
        plate_only
    );
    let object = format!("1:{}", dense.display());
    assert_ne!(
        slice(
            "object_dense.gcode",
            &project,
            &["--project-settings", "--object", &object]
        )
        .0,
        with
    );
    // -c files and --set go over the project's settings.
    let (set, _) = slice(
        "set.gcode",
        &project,
        &["--project-settings", "--set", "layer_height=0.3"],
    );
    assert!(set.contains("; layer_height = 0.3\n"));
    assert!(set.contains("; wall_loops = 3\n"));

    // A model file without stored settings gives a warning.
    let stl = dir.join("cube.stl");
    fs::write(
        &stl,
        slicer_formats::write_stl_binary(&Mesh::cuboid(10.0, 10.0, 4.0)),
    )
    .unwrap();
    let bare = dir.join("bare.3mf");
    fs::write(
        &bare,
        slicer_formats::write_3mf(&Mesh::cuboid(10.0, 10.0, 4.0)).unwrap(),
    )
    .unwrap();
    let (_, report) = slice("bare.gcode", &bare, &["--project-settings"]);
    assert_eq!(codes(&report), ["project.no_settings"]);
    let (_, report) = slice("stl.gcode", &stl, &["--project-settings"]);
    assert!(codes(&report).is_empty());
}

#[test]
fn support_blockers_and_enforcers_move_supports() {
    let dir = workdir("support_volumes");
    // A T: a column with a wide plate on top.
    let mut t = Mesh::cuboid(10.0, 10.0, 10.0);
    t.translate([10.0, 0.0, 0.0]);
    let mut plate = Mesh::cuboid(30.0, 10.0, 2.0);
    plate.translate([0.0, 0.0, 10.0]);
    t.merge(&plate);
    let model = dir.join("t.stl");
    fs::write(&model, slicer_formats::write_stl_binary(&t)).unwrap();
    // A box around the whole plate, in the model's coordinates.
    let mut around = Mesh::cuboid(32.0, 12.0, 4.0);
    around.translate([-1.0, -1.0, 9.0]);
    let volume = dir.join("around.stl");
    fs::write(&volume, slicer_formats::write_stl_binary(&around)).unwrap();
    let slice = |name: &str, args: &[String]| {
        let output = dir.join(name);
        let out = bin()
            .args(["slice", model.to_str().unwrap(), "-o"])
            .arg(&output)
            .arg("--json")
            .args(args)
            .output()
            .unwrap();
        let text = fs::read_to_string(&output).unwrap_or_default();
        (out, text)
    };
    let supported = |gcode: &str| gcode.contains(";TYPE:Support");
    let on = "--set=supports=true".to_string();
    let blocker = format!("--support-blocker=1:{}", volume.display());
    let enforcer = format!("--support-enforcer=1:{}", volume.display());

    let (out, plain) = slice("plain.gcode", &[]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(!supported(&plain));
    let (out, enforced) = slice("enforced.gcode", std::slice::from_ref(&enforcer));
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(supported(&enforced));
    let (_, on_gcode) = slice("on.gcode", std::slice::from_ref(&on));
    assert!(supported(&on_gcode));
    let (out, blocked) = slice("blocked.gcode", &[on, blocker]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(!supported(&blocked));

    let error = |args: &[String]| {
        let (out, _) = slice("error.gcode", args);
        assert!(!out.status.success());
        let error: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
        error["error"].clone()
    };
    for prefix in ["2:", "0:", "x:", ""] {
        let wrong = error(&[format!("--support-enforcer={prefix}{}", volume.display())]);
        assert_eq!(wrong["code"], "cli.invalid_support_volume", "{prefix}");
    }
    let missing = error(&[format!(
        "--support-blocker=1:{}",
        dir.join("none.stl").display()
    )]);
    assert_eq!(missing["code"], "io.read");
}
