//! Writing images as PNG (PNG specification, ISO/IEC 15948), in memory.
//!
//! The simplest valid form: one IHDR, one IDAT and one IEND chunk, 8 bits
//! per channel, no filtering, zlib compression. Small and fast enough for
//! layer images and thumbnails; the same pixels always give the same bytes.

use std::io::Write;

/// How the bytes of a pixel are to be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorType {
    /// One byte per pixel: grey from 0 (black) to 255 (white).
    Grey,
    /// Four bytes per pixel: red, green, blue and opacity (0 transparent,
    /// 255 opaque), not multiplied by the opacity.
    Rgba,
}

impl ColorType {
    /// Bytes per pixel.
    pub fn channels(self) -> usize {
        match self {
            Self::Grey => 1,
            Self::Rgba => 4,
        }
    }

    /// The colour type number of the PNG header.
    fn code(self) -> u8 {
        match self {
            Self::Grey => 0,
            Self::Rgba => 6,
        }
    }
}

/// Encodes an image with rows from top to bottom as PNG. `pixels` holds
/// `width × height` pixels of `color`; a shorter slice gives fewer rows.
/// Only fails if the compressor does, which it does not for memory.
pub fn encode(
    width: u32,
    height: u32,
    color: ColorType,
    pixels: &[u8],
) -> std::io::Result<Vec<u8>> {
    let row_bytes = (width.max(1) as usize) * color.channels();
    let mut raw = Vec::with_capacity(pixels.len() + height as usize);
    for row in pixels.chunks(row_bytes) {
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
    // Bit depth 8, the colour type, deflate, adaptive filtering, no interlace.
    header.extend_from_slice(&[8, color.code(), 0, 0, 0]);
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

/// Width, height and colour type from the header of a PNG written by
/// [`encode`]; `None` if `png` does not start like one.
pub fn header(png: &[u8]) -> Option<(u32, u32, ColorType)> {
    if png.len() < 33 || !png.starts_with(b"\x89PNG\r\n\x1a\n") || &png[12..16] != b"IHDR" {
        return None;
    }
    let number = |at: usize| u32::from_be_bytes([png[at], png[at + 1], png[at + 2], png[at + 3]]);
    let color = match (png[24], png[25]) {
        (8, 0) => ColorType::Grey,
        (8, 6) => ColorType::Rgba,
        _ => return None,
    };
    Some((number(16), number(20), color))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use std::io::Read;

    use super::*;

    #[test]
    fn rgba_image_round_trips() {
        let pixels: Vec<u8> = (0..3 * 2 * 4).map(|i| (i * 10) as u8).collect();
        let png = encode(3, 2, ColorType::Rgba, &pixels).unwrap();
        assert_eq!(header(&png), Some((3, 2, ColorType::Rgba)));
        // IDAT follows the 8 byte signature and the 25 byte IHDR chunk.
        let length = u32::from_be_bytes(png[33..37].try_into().unwrap()) as usize;
        assert_eq!(&png[37..41], b"IDAT");
        let mut raw = Vec::new();
        flate2::read::ZlibDecoder::new(&png[41..41 + length])
            .read_to_end(&mut raw)
            .unwrap();
        assert_eq!(raw.len(), 2 * (1 + 12));
        assert_eq!(&raw[1..13], &pixels[..12]);
        assert_eq!(&raw[14..], &pixels[12..]);
        assert!(png.ends_with(&[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]));
    }

    #[test]
    fn grey_header_is_recognised() {
        let png = encode(4, 1, ColorType::Grey, &[0, 64, 128, 255]).unwrap();
        assert_eq!(header(&png), Some((4, 1, ColorType::Grey)));
        assert_eq!(header(b"not a png"), None);
    }
}
