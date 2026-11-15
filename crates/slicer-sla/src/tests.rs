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
}
