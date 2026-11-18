//! Pictures of the plate in the G-code header.
//!
//! Every picture is a PNG image, encoded as base64 and written as comment
//! lines in the block that Klipper front-ends (Mainsail, Fluidd through
//! Moonraker), OctoPrint plugins and many printer screens read:
//!
//! ```text
//! ; thumbnail begin 300x300 12345
//! ; iVBORw0KGgoAAAANSUhEUgAAASwAAAEsCAYAAAB5fY51AAAgAElEQVR4nO3de3xU9Z...
//! ; ...
//! ; thumbnail end
//! ;
//! ```
//!
//! `12345` is the length of the base64 text without line breaks; each
//! line holds at most [`LINE_LENGTH`] characters of it. The blocks follow
//! the statistics at the top of the file, so readers that only look at the
//! beginning find them. Being comments, they change nothing the printer
//! does nor any figure of the print.

use slicer_core::Mesh;
use slicer_core::png::{ColorType, encode};
use slicer_core::thumbnail::{Style, render};
use slicer_fdm::Placement;
use slicer_profiles::Settings;

/// Base64 characters per comment line.
pub const LINE_LENGTH: usize = 78;

/// A picture of the plate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnail {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The image as PNG file content.
    pub png: Vec<u8>,
}

/// The objects as they stand on the plate: with [`Placement::AsGiven`]
/// every object is put down on the bed on its own, as slicing does.
/// [`Placement::Centre`] moves all of them together, which changes nothing
/// in a picture that is framed to the objects anyway.
fn on_the_bed(meshes: &[Mesh], placement: Placement) -> Vec<Mesh> {
    meshes
        .iter()
        .map(|mesh| {
            let mut mesh = mesh.clone();
            if let (Placement::AsGiven, Some((min, _))) = (placement, mesh.bounds()) {
                mesh.translate([0.0, 0.0, -min[2]]);
            }
            mesh
        })
        .collect()
}

/// Draws the objects of a plate, as they are given to
/// `slicer_fdm::generate_plate` with `placement`, once in every size of
/// `sizes` (width, height) and encodes the pictures as PNG.
pub fn render_thumbnails(
    meshes: &[Mesh],
    placement: Placement,
    sizes: &[[u32; 2]],
) -> std::io::Result<Vec<Thumbnail>> {
    if sizes.is_empty() {
        return Ok(Vec::new());
    }
    let meshes = on_the_bed(meshes, placement);
    let style = Style::default();
    sizes
        .iter()
        .map(|&[width, height]| {
            let pixels = render(&meshes, width, height, &style);
            Ok(Thumbnail {
                width,
                height,
                png: encode(width, height, ColorType::Rgba, &pixels)?,
            })
        })
        .collect()
}

/// Draws the pictures that `settings.thumbnails` asks for (see
/// [`render_thumbnails`]) and puts them into `gcode` (see
/// [`insert_thumbnails`]). Does nothing if it asks for none.
pub fn add_thumbnails(
    gcode: &mut String,
    meshes: &[Mesh],
    placement: Placement,
    settings: &Settings,
) -> std::io::Result<()> {
    // Settings are checked when they are read, so sizes that cannot be
    // read do not get here.
    let sizes = settings.thumbnail_sizes().unwrap_or_default();
    insert_thumbnails(gcode, &render_thumbnails(meshes, placement, &sizes)?);
    Ok(())
}

/// Standard base64 (RFC 4648, with `=` padding).
pub fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = String::with_capacity(data.len().div_ceil(3) * 4);
    for group in data.chunks(3) {
        let bytes = [
            group[0],
            group.get(1).copied().unwrap_or(0),
            group.get(2).copied().unwrap_or(0),
        ];
        let bits = u32::from(bytes[0]) << 16 | u32::from(bytes[1]) << 8 | u32::from(bytes[2]);
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= group.len() {
                text.push(char::from(ALPHABET[(bits >> shift & 63) as usize]));
            } else {
                text.push('=');
            }
        }
    }
    text
}

/// The comment block of one picture, ending with a line break.
pub fn comment_block(thumbnail: &Thumbnail) -> String {
    let encoded = base64(&thumbnail.png);
    let mut block = format!(
        "; thumbnail begin {}x{} {}\n",
        thumbnail.width,
        thumbnail.height,
        encoded.len()
    );
    // Base64 is ASCII, so every chunk is whole characters.
    for line in encoded.as_bytes().chunks(LINE_LENGTH) {
        block.push_str("; ");
        block.push_str(&String::from_utf8_lossy(line));
        block.push('\n');
    }
    block.push_str("; thumbnail end\n;\n");
    block
}

/// Puts the comment blocks of `thumbnails` into G-code written by
/// [`crate::write_gcode`]: after the statistics at the top, before the
/// start G-code. Nothing changes without thumbnails.
pub fn insert_thumbnails(gcode: &mut String, thumbnails: &[Thumbnail]) {
    if thumbnails.is_empty() {
        return;
    }
    // The header ends with the time estimate and the lines of the
    // extruders; the blocks go first if it cannot be found.
    let mut at = 0;
    let mut offset = 0;
    let mut found = false;
    for line in gcode.split_inclusive('\n') {
        offset += line.len();
        if line.starts_with("; estimated printing time")
            || (found && line.starts_with("; extruder T"))
        {
            found = true;
            at = offset;
        } else if found || !line.starts_with("; ") {
            break;
        }
    }
    let blocks: String = thumbnails.iter().map(comment_block).collect();
    gcode.insert_str(at, &format!(";\n{blocks}"));
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn base64_matches_the_examples_of_rfc_4648() {
        for (data, text) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(data.as_bytes()), text);
        }
        assert_eq!(base64(&[0xFB, 0xFF, 0xBF]), "+/+/");
    }

    #[test]
    fn blocks_have_the_length_and_short_lines() {
        let thumbnail = Thumbnail {
            width: 2,
            height: 3,
            png: (0..=255).collect(),
        };
        let block = comment_block(&thumbnail);
        let lines: Vec<&str> = block.lines().collect();
        assert_eq!(lines[0], "; thumbnail begin 2x3 344");
        assert_eq!(lines[lines.len() - 2], "; thumbnail end");
        assert_eq!(lines[lines.len() - 1], ";");
        let body = &lines[1..lines.len() - 2];
        assert_eq!(body.len(), 5);
        assert!(body.iter().all(|l| l.starts_with("; ") && l.len() <= 80));
        let joined: String = body.iter().map(|l| &l[2..]).collect();
        assert_eq!(joined, base64(&thumbnail.png));
    }

    #[test]
    fn blocks_go_after_the_statistics() {
        let thumbnail = Thumbnail {
            width: 1,
            height: 1,
            png: vec![1, 2, 3],
        };
        let mut gcode = "; generated by x\n; estimated printing time [s]: 5\n; extruder T0: a\n; extruder T1: b\n; start\nG28\n".to_owned();
        insert_thumbnails(&mut gcode, std::slice::from_ref(&thumbnail));
        assert_eq!(
            gcode,
            "; generated by x\n; estimated printing time [s]: 5\n; extruder T0: a\n; extruder T1: b\n;\n; thumbnail begin 1x1 4\n; AQID\n; thumbnail end\n;\n; start\nG28\n"
        );
        let mut plain = "G28\n".to_owned();
        insert_thumbnails(&mut plain, std::slice::from_ref(&thumbnail));
        assert!(plain.starts_with(";\n; thumbnail begin 1x1 4\n"));
        assert!(plain.ends_with("; thumbnail end\n;\nG28\n"));
        let mut unchanged = "G28\n".to_owned();
        insert_thumbnails(&mut unchanged, &[]);
        assert_eq!(unchanged, "G28\n");
    }

    #[test]
    fn thumbnails_are_pngs_of_the_sizes() {
        let mut cube = Mesh::cuboid(10.0, 10.0, 10.0);
        cube.translate([0.0, 0.0, 5.0]);
        let thumbnails =
            render_thumbnails(&[cube.clone()], Placement::AsGiven, &[[48, 48], [64, 32]]).unwrap();
        assert_eq!(thumbnails.len(), 2);
        assert_eq!(
            slicer_core::png::header(&thumbnails[1].png),
            Some((64, 32, ColorType::Rgba))
        );
        assert_eq!(
            render_thumbnails(&[cube], Placement::Centre, &[[48, 48]]).unwrap()[0],
            thumbnails[0]
        );
        assert!(
            render_thumbnails(&[], Placement::Centre, &[])
                .unwrap()
                .is_empty()
        );
    }
}
