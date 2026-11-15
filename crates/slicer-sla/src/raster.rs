//! Turning layer shapes into grey-scale images and writing them as PNG.
//!
//! Scanline fill with the non-zero winding rule on a grid of
//! `antialiasing × antialiasing` samples per pixel (own design, standard
//! scanline technique). Only additions, multiplications and divisions of
//! `f64` are used, so every platform gets the same pixels.

use std::io::Write;

use slicer_core::{Point, Shape};

use crate::SlaSettings;

/// Renders shapes, given in millimetres on the screen (origin at the
/// bottom-left corner of the display, Y up), into one byte per pixel:
/// 0 is dark, 255 fully lit, values between are partly covered edge pixels.
///
/// Rows run from the top of the image (largest Y) down, so the image shows
/// the layer as seen from above. Outer contours run counter-clockwise and
/// holes clockwise (as `slicer_core::slice` produces them); the non-zero
/// winding rule leaves holes dark.
pub fn rasterize(shapes: &[Shape], settings: &SlaSettings) -> Vec<u8> {
    let width = settings.resolution_x as usize;
    let height = settings.resolution_y as usize;
    let samples = settings.antialiasing as usize;
    let [pixel_x, pixel_y] = settings.pixel_size();
    // Coordinates in samples: u to the right, v down from the top edge.
    let to_samples = |p: Point| {
        [
            p[0] / pixel_x * samples as f64,
            (settings.display_height - p[1]) / pixel_y * samples as f64,
        ]
    };
    let rows = height * samples;
    let columns = width * samples;

    // Crossings of every sample row (at v = j + 0.5) with the contours:
    // position and +1/-1 for the direction the edge runs.
    let mut crossings: Vec<Vec<(f64, i32)>> = vec![Vec::new(); rows];
    for contour in shapes.iter().flatten() {
        for (i, &a) in contour.iter().enumerate() {
            let (a, b) = (to_samples(a), to_samples(contour[(i + 1) % contour.len()]));
            if a[1] == b[1] {
                continue;
            }
            let (direction, lo, hi) = if a[1] < b[1] { (1, a, b) } else { (-1, b, a) };
            // Half-open, so a vertex shared by two edges counts once.
            let first = (lo[1] - 0.5).ceil().clamp(0.0, rows as f64) as usize;
            let end = (hi[1] - 0.5).ceil().clamp(0.0, rows as f64) as usize;
            for (j, row) in crossings.iter_mut().enumerate().take(end).skip(first) {
                let v = j as f64 + 0.5;
                row.push((
                    lo[0] + (v - lo[1]) * (hi[0] - lo[0]) / (hi[1] - lo[1]),
                    direction,
                ));
            }
        }
    }

    let mut pixels = vec![0u8; width * height];
    let full = (samples * samples) as u32;
    let mut coverage = vec![0u16; width];
    for (row, band) in crossings.chunks_mut(samples).enumerate() {
        if band.iter().all(Vec::is_empty) {
            continue;
        }
        coverage.fill(0);
        for line in band {
            line.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut winding = 0;
            let mut start = 0.0;
            for &(u, direction) in line.iter() {
                let before = winding;
                winding += direction;
                if before == 0 {
                    start = u;
                } else if winding == 0 {
                    // Samples k with start <= k + 0.5 < u are inside.
                    let first = (start - 0.5).ceil().clamp(0.0, columns as f64) as usize;
                    let end = (u - 0.5).ceil().clamp(0.0, columns as f64) as usize;
                    let mut k = first;
                    while k < end {
                        let pixel = k / samples;
                        let next = ((pixel + 1) * samples).min(end);
                        coverage[pixel] += (next - k) as u16;
                        k = next;
                    }
                }
            }
        }
        for (pixel, &count) in pixels[row * width..(row + 1) * width]
            .iter_mut()
            .zip(&coverage)
        {
            *pixel = ((u32::from(count) * 255 + full / 2) / full) as u8;
        }
    }
    pixels
}

/// Encodes an 8-bit grey-scale image (rows top to bottom) as PNG
/// (PNG specification, ISO/IEC 15948): one IHDR, one IDAT, one IEND chunk,
/// no filtering, zlib compression.
pub fn encode_png(width: u32, height: u32, pixels: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut raw = Vec::with_capacity(pixels.len() + height as usize);
    for row in pixels.chunks(width.max(1) as usize) {
        raw.push(0); // filter type: none
        raw.extend_from_slice(row);
    }
    let mut zlib = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    zlib.write_all(&raw)?;
    let data = zlib.finish()?;

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    // Bit depth 8, colour type 0 (grey), deflate, adaptive filtering, no interlace.
    header.extend_from_slice(&[8, 0, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &header);
    chunk(&mut png, b"IDAT", &data);
    chunk(&mut png, b"IEND", &[]);
    Ok(png)
}

/// Appends a PNG chunk: length, type, data, CRC-32 of type and data.
fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = png.len();
    png.extend_from_slice(kind);
    png.extend_from_slice(data);
    let crc = crc32fast::hash(&png[start..]);
    png.extend_from_slice(&crc.to_be_bytes());
}
