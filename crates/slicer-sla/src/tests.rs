#![allow(clippy::unwrap_used)]

use std::io::Read;

use slicer_core::primitives;

use super::*;

fn slice_ok(mesh: &Mesh, settings: &SlaSettings) -> SlaPrint {
    slice(mesh, settings, &mut Diagnostics::new()).unwrap()
}

/// Decodes the PNGs written by [`raster::encode_png`] (grey, unfiltered).
fn decode_png(png: &[u8]) -> (usize, usize, Vec<u8>) {
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    let (mut at, mut size, mut data) = (8, (0, 0), Vec::new());
    while at < png.len() {
        let length = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
        let kind = &png[at + 4..at + 8];
        let body = &png[at + 8..at + 8 + length];
        let crc = u32::from_be_bytes(png[at + 8 + length..at + 12 + length].try_into().unwrap());
        assert_eq!(crc, crc32fast::hash(&png[at + 4..at + 8 + length]));
        match kind {
            b"IHDR" => {
                size = (
                    u32::from_be_bytes(body[0..4].try_into().unwrap()) as usize,
                    u32::from_be_bytes(body[4..8].try_into().unwrap()) as usize,
                );
                assert_eq!(&body[8..], &[8, 0, 0, 0, 0]);
            }
            b"IDAT" => data.extend_from_slice(body),
            _ => {}
        }
        at += 12 + length;
    }
    let mut raw = Vec::new();
    flate2::read::ZlibDecoder::new(&data[..])
        .read_to_end(&mut raw)
        .unwrap();
    let (width, height) = size;
    assert_eq!(raw.len(), (width + 1) * height);
    let pixels = raw
        .chunks(width + 1)
        .flat_map(|row| {
            assert_eq!(row[0], 0);
            row[1..].to_vec()
        })
        .collect();
    (width, height, pixels)
}

#[test]
fn cube_gives_expected_layers_and_pixels() {
    let settings = SlaSettings::default();
    let print = slice_ok(&Mesh::cuboid(10.0, 10.0, 10.0), &settings);
    // 10 mm / 0.05 mm; 10 mm / 0.05 mm per pixel = 200 pixels per side.
    assert_eq!(print.stats.layers, 200);
    assert_eq!(print.layers[199].z, 10.0);
    for layer in &print.layers {
        assert!(
            (199 * 199..=201 * 201).contains(&layer.lit_pixels),
            "{}",
            layer.lit_pixels
        );
    }
    assert_eq!(print.layers[5].exposure_time, 40.0);
    assert_eq!(print.layers[6].exposure_time, 2.0);
    // 6 × 40 s + 194 × 2 s exposure, 200 × (3 s up + 2 s down).
    assert_eq!(print.stats.print_time_s, 240.0 + 388.0 + 1000.0);

    let (width, height, pixels) = decode_png(&print.layers[100].png);
    assert_eq!((width, height), (3840, 2400));
    let lit: Vec<usize> = (0..pixels.len()).filter(|&i| pixels[i] > 0).collect();
    let columns = lit.iter().map(|i| i % width);
    let rows = lit.iter().map(|i| i / width);
    let (left, right) = (columns.clone().min().unwrap(), columns.max().unwrap());
    let (top, bottom) = (rows.clone().min().unwrap(), rows.max().unwrap());
    // Centred: 1920 ± 100 columns and 1200 ± 100 rows, ± 1 at the edges.
    assert!(
        left.abs_diff(1820) <= 1 && right.abs_diff(2019) <= 1,
        "{left} {right}"
    );
    assert!(
        top.abs_diff(1100) <= 1 && bottom.abs_diff(1299) <= 1,
        "{top} {bottom}"
    );
    // The inside is fully lit.
    assert!((top + 1..bottom).all(|r| (left + 1..right).all(|c| pixels[r * width + c] == 255)));
}

#[test]
fn edges_are_anti_aliased() {
    // 10.025 mm wide: the edges end half way through a pixel.
    let settings = SlaSettings::default();
    let print = slice_ok(&Mesh::cuboid(10.025, 10.0, 1.0), &settings);
    let (_, _, pixels) = decode_png(&print.layers[0].png);
    assert!(pixels.iter().any(|&p| p > 0 && p < 255));
    let off = SlaSettings {
        antialiasing: 1,
        ..SlaSettings::default()
    };
    let print = slice_ok(&Mesh::cuboid(10.025, 10.0, 1.0), &off);
    let (_, _, pixels) = decode_png(&print.layers[0].png);
    assert!(pixels.iter().all(|&p| p == 0 || p == 255));
}

#[test]
fn hole_stays_dark() {
    let settings = SlaSettings::default();
    let print = slice_ok(&primitives::tube(5.0, 2.0, 2.0, 64), &settings);
    let (width, _, pixels) = decode_png(&print.layers[20].png);
    let at = |dx: i64, dy: i64| pixels[((1200 + dy) as usize) * width + (1920 + dx) as usize];
    // Hole radius 2 mm = 40 pixels around the centre of the screen.
    for (dx, dy) in [(0, 0), (-1, -1), (30, 0), (0, -30), (-21, 21)] {
        assert_eq!(at(dx, dy), 0, "({dx}, {dy})");
    }
    // The wall between 2 and 5 mm is lit.
    for (dx, dy) in [(70, 0), (-70, 0), (0, 70), (0, -70), (49, 49)] {
        assert_eq!(at(dx, dy), 255, "({dx}, {dy})");
    }
    let ring = std::f64::consts::PI * (25.0 - 4.0) * 0.05 / 1000.0;
    assert!((print.layers[20].resin_volume / 1000.0 - ring).abs() < 0.02 * ring);
}

#[test]
fn resin_volume_matches_mesh_volume() {
    let settings = SlaSettings {
        layer_height: 0.1,
        ..SlaSettings::default()
    };
    let mut sphere = primitives::sphere(6.0, 64);
    sphere.translate([0.0, 0.0, 6.0]);
    let print = slice_ok(&sphere, &settings);
    let mesh_ml = sphere.volume() / 1000.0;
    assert!(
        (print.stats.resin_volume_ml - mesh_ml).abs() < 0.01 * mesh_ml,
        "{} vs {mesh_ml}",
        print.stats.resin_volume_ml
    );
}

#[test]
fn output_is_deterministic() {
    let settings = SlaSettings {
        layer_height: 0.2,
        ..SlaSettings::default()
    };
    let mesh = primitives::torus(8.0, 3.0, 48, 24);
    let first = write_archive(&slice_ok(&mesh, &settings)).unwrap();
    let second = write_archive(&slice_ok(&mesh, &settings)).unwrap();
    assert_eq!(first, second);

    let mut archive = zip::ZipArchive::new(Cursor::new(&first)).unwrap();
    assert_eq!(archive.len(), 1 + 30);
    let mut text = String::new();
    archive
        .by_name("manifest.json")
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(manifest["format"], "spaghetti-sla");
    assert_eq!(manifest["layers"][29]["file"], "layers/00030.png");
    assert_eq!(manifest["layers"][29]["z"], 6.0);
    assert_eq!(manifest["settings"]["resolution_x"], 3840);
    assert!(archive.by_name("layers/00001.png").is_ok());
}

#[test]
fn settings_are_checked() {
    let mut settings = SlaSettings::default();
    settings.set("exposure_time", "2.5").unwrap();
    settings.set("bottom_layers", "8").unwrap();
    assert_eq!((settings.exposure_time, settings.bottom_layers), (2.5, 8));
    assert_eq!(
        settings.set("nozzle_temp", "200"),
        Err(SlaError::UnknownKey("nozzle_temp".into()))
    );
    assert!(matches!(
        settings.set("bottom_layers", "2.5"),
        Err(SlaError::OutOfRange { .. })
    ));
    assert!(matches!(
        settings.set("layer_height", "abc"),
        Err(SlaError::OutOfRange { .. })
    ));
    let too_large = slice(
        &Mesh::cuboid(200.0, 10.0, 10.0),
        &SlaSettings::default(),
        &mut Diagnostics::new(),
    );
    assert!(matches!(too_large, Err(SlaError::TooLarge { .. })));

    settings.set("supports", "1").unwrap();
    assert!(settings.supports);
    settings.validate().unwrap();
    assert!(matches!(
        settings.set("supports", "2"),
        Err(SlaError::OutOfRange { .. })
    ));
    // The raft and elevation count towards the height.
    let raised = SlaSettings {
        elevation: 100.0,
        ..SlaSettings::default()
    };
    let too_tall = slice(
        &Mesh::cuboid(10.0, 10.0, 150.0),
        &raised,
        &mut Diagnostics::new(),
    );
    assert!(matches!(too_tall, Err(SlaError::TooLarge { .. })));
}

/// 0.1 mm pixels and layers: quick enough for tests.
fn coarse(pixel: f64) -> SlaSettings {
    SlaSettings {
        resolution_x: (192.0 / pixel).round() as u32,
        resolution_y: (120.0 / pixel).round() as u32,
        layer_height: pixel,
        ..SlaSettings::default()
    }
}

/// A T: a 4 × 4 × 10 mm stem carrying a 20 × 4 × 4 mm bar, whose arms
/// overhang by 8 mm on either side.
fn t_shape() -> Mesh {
    let mut t = Mesh::cuboid(4.0, 4.0, 10.0);
    t.translate([8.0, 0.0, 0.0]);
    let mut bar = Mesh::cuboid(20.0, 4.0, 4.0);
    bar.translate([0.0, 0.0, 10.0]);
    t.merge(&bar);
    t
}

#[test]
fn overhang_gets_supports_from_raft_to_model() {
    let settings = SlaSettings {
        raft_thickness: 1.0,
        supports: true,
        support_brace_height: 5.0,
        ..coarse(0.1)
    };
    let print = slice_ok(&t_shape(), &settings);
    // Raft 1 mm, stem up to 11 mm, bar up to 15 mm.
    assert_eq!(print.stats.layers, 150);
    let images: Vec<Vec<u8>> = print.layers.iter().map(|l| decode_png(&l.png).2).collect();
    let width = 1920;
    // Layer 109 (10.9 to 11 mm) lies right under the bar: only support tips.
    // Columns 940 to 979 hold the stem, the arms reach from 860 to 1059.
    let tips: Vec<usize> = (0..images[109].len())
        .filter(|&i| images[109][i] > 0 && !(935..985).contains(&(i % width)))
        .collect();
    assert!(
        tips.iter().any(|&i| i % width < 935),
        "left arm unsupported"
    );
    assert!(
        tips.iter().any(|&i| i % width >= 985),
        "right arm unsupported"
    );
    // Within two pixels of the bar (rows 580 to 619).
    assert!(tips.iter().all(|&i| (858..1062).contains(&(i % width))));
    assert!(tips.iter().all(|&i| (578..622).contains(&(i / width))));
    // Every pixel of a tip stays lit at least as much all the way down
    // through pillar and raft to the build plate.
    for &i in &tips {
        for (index, image) in images.iter().enumerate().take(109) {
            assert!(
                image[i] >= images[109][i],
                "pixel {i} dark in layer {index}"
            );
        }
    }
    // The tips touch the bar: the layer above is the bar itself. Tips at the
    // bar's corners (local minima of the mesh) stick out by half.
    let under_bar = tips.iter().filter(|&&i| images[110][i] == 255).count();
    assert!(
        under_bar * 3 >= tips.len() * 2,
        "{under_bar} of {}",
        tips.len()
    );
    // The raft is wider than the pillars it carries.
    assert!(print.layers[5].lit_pixels > print.layers[50].lit_pixels * 2);

    // Braces add material between pillars that plain pillars lack.
    let unbraced = SlaSettings {
        support_brace_height: 0.0,
        ..settings.clone()
    };
    let plain = slice_ok(&t_shape(), &unbraced);
    assert!(print.layers[40].lit_pixels > plain.layers[40].lit_pixels);
    assert_eq!(print.layers[109].png, plain.layers[109].png);
}

/// Volume in mm³ of the dark voxels that cannot be reached from outside the
/// box around the model: closed cavities.
fn enclosed_volume(print: &SlaPrint, box_pixels: (usize, usize, usize, usize)) -> f64 {
    let (left, right, top, bottom) = box_pixels;
    let (nx, ny, nz) = (right - left, bottom - top, print.layers.len());
    let mut dark = Vec::with_capacity(nx * ny * nz);
    for layer in &print.layers {
        let (width, _, pixels) = decode_png(&layer.png);
        for row in top..bottom {
            dark.extend(
                pixels[row * width + left..row * width + right]
                    .iter()
                    .map(|&p| p < 128),
            );
        }
    }
    let at = |x: usize, y: usize, z: usize| (z * ny + y) * nx + x;
    let mut reached = vec![false; dark.len()];
    let mut queue = Vec::new();
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let edge = x == 0 || y == 0 || z == 0 || x == nx - 1 || y == ny - 1 || z == nz - 1;
                if edge && dark[at(x, y, z)] {
                    reached[at(x, y, z)] = true;
                    queue.push((x, y, z));
                }
            }
        }
    }
    while let Some((x, y, z)) = queue.pop() {
        let neighbours = [
            (x.wrapping_sub(1), y, z),
            (x + 1, y, z),
            (x, y.wrapping_sub(1), z),
            (x, y + 1, z),
            (x, y, z.wrapping_sub(1)),
            (x, y, z + 1),
        ];
        for (x, y, z) in neighbours {
            if x < nx && y < ny && z < nz && dark[at(x, y, z)] && !reached[at(x, y, z)] {
                reached[at(x, y, z)] = true;
                queue.push((x, y, z));
            }
        }
    }
    let closed = dark
        .iter()
        .zip(&reached)
        .filter(|&(&d, &r)| d && !r)
        .count();
    let [pixel_x, pixel_y] = print.settings.pixel_size();
    closed as f64 * pixel_x * pixel_y * print.settings.layer_height
}

#[test]
fn hollow_cube_has_walls_and_an_open_cavity() {
    let cube = Mesh::cuboid(20.0, 20.0, 20.0);
    let hollow = SlaSettings {
        hollow_wall: 2.0,
        ..coarse(0.2)
    };
    let mut warnings = Diagnostics::new();
    let print = slice(&cube, &hollow, &mut warnings).unwrap();
    // 20³ minus the 16³ cavity minus the drain hole through the 2 mm floor.
    let expected = 8000.0 - 4096.0 - std::f64::consts::PI * 2.0;
    let resin = print.stats.resin_volume_ml * 1000.0;
    assert!(
        (resin - expected).abs() < 0.01 * expected,
        "{resin} vs {expected}"
    );
    // On the plate the drain hole is closed by it.
    assert!(
        warnings
            .as_slice()
            .iter()
            .any(|d| d.code == "sla.drain_on_plate")
    );
    // Top and bottom layers are solid, the middle ones a 2 mm ring.
    let solid = 100 * 100;
    assert!(print.layers[95].lit_pixels.abs_diff(solid) <= 400);
    assert!(print.layers[50].lit_pixels.abs_diff(solid - 80 * 80) <= 400);

    // Raised on supports: the cavity drains through its hole, no resin is
    // trapped. The cube spans pixels 430..530 and 250..350.
    let raised = SlaSettings {
        elevation: 3.0,
        raft_thickness: 1.0,
        supports: true,
        ..hollow.clone()
    };
    let print = slice_ok(&cube, &raised);
    let around = (415, 545, 235, 365);
    let trapped = enclosed_volume(&print, around);
    assert!(trapped < 1.0, "{trapped} mm³ trapped");
    // Without the drain hole the whole cavity is closed.
    let sealed = SlaSettings {
        drain_hole_diameter: 0.0,
        ..raised
    };
    let mut warnings = Diagnostics::new();
    let print = slice(&cube, &sealed, &mut warnings).unwrap();
    let trapped = enclosed_volume(&print, around);
    assert!(
        (trapped - 4096.0).abs() < 0.02 * 4096.0,
        "{trapped} mm³ trapped"
    );
    assert!(
        warnings
            .as_slice()
            .iter()
            .any(|d| d.code == "sla.sealed_cavity")
    );
}

#[test]
fn tilted_cube_with_everything_is_deterministic() {
    let mut cube = Mesh::cuboid(12.0, 12.0, 12.0);
    cube.rotate_x(0.5);
    cube.rotate_z(0.3);
    let settings = SlaSettings {
        elevation: 4.0,
        raft_thickness: 0.8,
        supports: true,
        support_brace_height: 2.0,
        hollow_wall: 1.5,
        ..coarse(0.2)
    };
    let print = slice_ok(&cube, &settings);
    let first = write_archive(&print).unwrap();
    assert_eq!(first, write_archive(&slice_ok(&cube, &settings)).unwrap());
    // Raft, then supports only, then the model.
    assert!(print.layers[..4].iter().all(|l| l.lit_pixels > 0));
    assert!(print.layers[4..24].iter().all(|l| l.lit_pixels > 0));
    let manifest = manifest(&print);
    assert_eq!(manifest["settings"]["supports"], true);
    assert_eq!(manifest["settings"]["hollow_wall"], 1.5);
}

#[test]
fn switched_off_features_leave_the_manifest_alone() {
    let print = slice_ok(&Mesh::cuboid(5.0, 5.0, 1.0), &coarse(0.2));
    let settings = manifest(&print)["settings"].clone();
    let keys: Vec<&String> = settings.as_object().unwrap().keys().collect();
    assert_eq!(keys.len(), 13, "{keys:?}");
}
