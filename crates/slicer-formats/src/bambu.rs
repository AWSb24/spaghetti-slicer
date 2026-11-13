//! Sliced project file for Bambu Lab printers (`.gcode.3mf`).
//!
//! The container was worked out from files written by Bambu Studio: a 3MF
//! package that carries the G-code of a plate, its MD5 checksum and a few
//! small description files. Which of these the printer needs is not known;
//! the writer includes what could be produced without copying anything.
//! Thumbnails and the full settings file are left out. **No file written
//! here has been loaded on a printer.**

use crate::threemf::{ThreeMfError, write_package};

/// What the description files say about a print.
#[derive(Debug, Clone, PartialEq)]
pub struct PrintInfo {
    /// Estimated print time in seconds.
    pub seconds: f64,
    /// Filament used, in grams.
    pub weight_g: f64,
    /// Filament used, in metres.
    pub filament_m: f64,
    /// Number of layers.
    pub layers: usize,
    /// Nozzle diameter in millimetres.
    pub nozzle_diameter: f64,
}

/// Sine-derived constants of MD5 (RFC 1321).
const MD5_K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];
/// Rotation amounts of the four rounds of MD5.
const MD5_S: [[u32; 4]; 4] = [
    [7, 12, 17, 22],
    [5, 9, 14, 20],
    [4, 11, 16, 23],
    [6, 10, 15, 21],
];

/// MD5 checksum as upper-case hexadecimal text, as the container stores it.
/// Written out here to avoid a dependency for a checksum that is not used
/// for security.
fn md5_hex(data: &[u8]) -> String {
    let mut state: [u32; 4] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend((data.len() as u64).wrapping_mul(8).to_le_bytes());
    for block in message.as_chunks::<64>().0 {
        let mut words = [0u32; 16];
        for (word, bytes) in words.iter_mut().zip(block.as_chunks::<4>().0) {
            *word = u32::from_le_bytes(*bytes);
        }
        let [mut a, mut b, mut c, mut d] = state;
        for i in 0..64 {
            let (mix, index) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let sum = a
                .wrapping_add(mix)
                .wrapping_add(MD5_K[i])
                .wrapping_add(words[index]);
            (a, d, c) = (d, c, b);
            b = b.wrapping_add(sum.rotate_left(MD5_S[i / 16][i % 4]));
        }
        for (total, value) in state.iter_mut().zip([a, b, c, d]) {
            *total = total.wrapping_add(value);
        }
    }
    state
        .iter()
        .flat_map(|word| word.to_le_bytes())
        .map(|byte| format!("{byte:02X}"))
        .collect()
}

/// Packs G-code into a sliced project file with one plate.
pub fn write_gcode_3mf(gcode: &str, info: &PrintInfo) -> Result<Vec<u8>, ThreeMfError> {
    const XML: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n";
    let content_types = format!(
        "{XML}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\n <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\n <Default Extension=\"model\" ContentType=\"application/vnd.ms-package.3dmanufacturing-3dmodel+xml\"/>\n <Default Extension=\"gcode\" ContentType=\"text/x.gcode\"/>\n</Types>\n"
    );
    let rels = format!(
        "{XML}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n <Relationship Target=\"/3D/3dmodel.model\" Id=\"rel-1\" Type=\"http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel\"/>\n</Relationships>\n"
    );
    // The model part is required by the 3MF format; the geometry itself is
    // not needed to print.
    let model = format!(
        "{XML}<model unit=\"millimeter\" xml:lang=\"en-US\" xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\">\n <metadata name=\"Application\">spaghetti-slicer-{}</metadata>\n <resources/>\n <build/>\n</model>\n",
        env!("CARGO_PKG_VERSION")
    );
    let settings_rels = format!(
        "{XML}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n <Relationship Target=\"/Metadata/plate_1.gcode\" Id=\"rel-1\" Type=\"http://schemas.bambulab.com/package/2021/gcode\"/>\n</Relationships>\n"
    );
    let model_settings = format!(
        "{XML}<config>\n  <plate>\n    <metadata key=\"plater_id\" value=\"1\"/>\n    <metadata key=\"plater_name\" value=\"\"/>\n    <metadata key=\"locked\" value=\"false\"/>\n    <metadata key=\"gcode_file\" value=\"Metadata/plate_1.gcode\"/>\n  </plate>\n</config>\n"
    );
    let slice_info = format!(
        "{XML}<config>\n  <header>\n    <header_item key=\"X-BBL-Client-Type\" value=\"slicer\"/>\n    <header_item key=\"X-BBL-Client-Version\" value=\"\"/>\n  </header>\n  <plate>\n    <metadata key=\"index\" value=\"1\"/>\n    <metadata key=\"nozzle_diameters\" value=\"{}\"/>\n    <metadata key=\"prediction\" value=\"{:.0}\"/>\n    <metadata key=\"weight\" value=\"{:.2}\"/>\n    <metadata key=\"outside\" value=\"false\"/>\n    <metadata key=\"support_used\" value=\"false\"/>\n    <filament id=\"1\" type=\"PLA\" color=\"#FFFFFF\" used_m=\"{:.2}\" used_g=\"{:.2}\"/>\n    <layer_filament_lists>\n      <layer_filament_list filament_list=\"0\" layer_ranges=\"0 {}\" />\n    </layer_filament_lists>\n  </plate>\n</config>\n",
        info.nozzle_diameter,
        info.seconds,
        info.weight_g,
        info.filament_m,
        info.weight_g,
        info.layers.saturating_sub(1)
    );
    write_package(&[
        ("[Content_Types].xml", &content_types),
        ("_rels/.rels", &rels),
        ("3D/3dmodel.model", &model),
        ("Metadata/plate_1.gcode", gcode),
        ("Metadata/plate_1.gcode.md5", &md5_hex(gcode.as_bytes())),
        ("Metadata/_rels/model_settings.config.rels", &settings_rels),
        ("Metadata/model_settings.config", &model_settings),
        ("Metadata/slice_info.config", &slice_info),
    ])
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use std::io::Read;

    use super::*;

    #[test]
    fn md5_matches_known_checksums() {
        assert_eq!(md5_hex(b""), "D41D8CD98F00B204E9800998ECF8427E");
        assert_eq!(md5_hex(b"abc"), "900150983CD24FB0D6963F7D28E17F72");
        assert_eq!(md5_hex(b"spaghetti spaghetti spaghetti spaghetti spaghetti spaghetti spaghetti spaghetti spaghetti spaghetti spaghetti spaghetti spaghetti spaghetti spaghetti spaghetti spaghetti spaghetti spaghetti spaghetti "), "72CE641BEF77BD87F56C27CDFCA973F4");
        // Lengths around the padding boundary.
        assert_eq!(md5_hex(&[b'x'; 55]), "04364420E25C512FD958A70738AA8F72");
        assert_eq!(md5_hex(&[b'x'; 56]), "668A72D5BA17F08E62DABCAFAD6DB14B");
        assert_eq!(md5_hex(&[b'x'; 63]), "7DC2CA208106A2F703567BDFF99D8981");
        assert_eq!(md5_hex(&[b'x'; 64]), "C1BB4F81D892B2D57947682AEB252456");
        assert_eq!(md5_hex(&[b'x'; 65]), "1BC932052302D074BDEC39795FE00CF6");
        assert_eq!(md5_hex(&[b'x'; 119]), "AB347A5F68C8A443CFCDDC633F12C24F");
        assert_eq!(md5_hex(&[b'x'; 120]), "FB98667F98096DE92620B64F46E1C5B5");
    }

    #[test]
    fn container_holds_the_gcode_and_its_checksum() {
        let gcode = "; test\nG28\nG1 X10 Y10 E1\n";
        let info = PrintInfo {
            seconds: 1234.4,
            weight_g: 3.456,
            filament_m: 1.2,
            layers: 100,
            nozzle_diameter: 0.4,
        };
        let bytes = write_gcode_3mf(gcode, &info).unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut read = |name: &str| {
            let mut text = String::new();
            archive
                .by_name(name)
                .unwrap()
                .read_to_string(&mut text)
                .unwrap();
            text
        };
        assert_eq!(read("Metadata/plate_1.gcode"), gcode);
        assert_eq!(
            read("Metadata/plate_1.gcode.md5"),
            md5_hex(gcode.as_bytes())
        );
        let info = read("Metadata/slice_info.config");
        assert!(info.contains("key=\"prediction\" value=\"1234\""), "{info}");
        assert!(info.contains("key=\"weight\" value=\"3.46\""), "{info}");
        assert!(info.contains("layer_ranges=\"0 99\""), "{info}");
        assert!(read("Metadata/model_settings.config").contains("Metadata/plate_1.gcode"));
        for name in ["[Content_Types].xml", "_rels/.rels", "3D/3dmodel.model"] {
            assert!(read(name).starts_with("<?xml"), "{name}");
        }
    }
}
