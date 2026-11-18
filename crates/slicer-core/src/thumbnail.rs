//! Small pictures of the models on a plate, drawn in software.
//!
//! An orthographic view from the front right and above, framed to the
//! models. Triangles are filled with a z-buffer on a grid of several
//! samples per pixel, shaded flat with one light (Lambert) plus some
//! ambient light, and the samples of a pixel are averaged, which smooths
//! the edges. Only additions, subtractions, multiplications, divisions and
//! square roots of `f64` are used, the view angles go through
//! [`sin_cos_degrees`], and triangles are drawn in the order given, so the
//! same meshes give the same pixels on every platform. Own design, the
//! standard technique of every software rasterizer.

use crate::mesh::Mesh;
use crate::polygon::sin_cos_degrees;

/// Angle about Z from the front (−Y) towards the right (+X), in degrees,
/// at which the models are seen.
pub const AZIMUTH_DEGREES: f64 = 35.0;
/// Angle above the bed at which the models are seen, in degrees.
pub const ELEVATION_DEGREES: f64 = 30.0;

/// Colours of extruders 1, 2, … in [`Style::default`]: orange first.
pub const DEFAULT_COLORS: [[u8; 3]; 8] = [
    [0xE8, 0x8A, 0x1E],
    [0x2F, 0x7F, 0xD0],
    [0x3B, 0xA5, 0x5C],
    [0xD0, 0x3E, 0x3E],
    [0x8E, 0x5C, 0xC8],
    [0xE0, 0xC0, 0x30],
    [0x80, 0x80, 0x80],
    [0xF0, 0xF0, 0xF0],
];

/// Share of the image left free around the models on each side.
const MARGIN: f64 = 0.06;
/// Brightness of a surface that the light does not reach.
const AMBIENT: f64 = 0.38;
/// Brightness added by light falling straight onto a surface.
const DIFFUSE: f64 = 0.62;

/// How a thumbnail looks.
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    /// Colour of the triangles of every extruder ([`Mesh::extruder`]),
    /// starting again from the first for more extruders than colours.
    /// Empty means [`DEFAULT_COLORS`]\[0\] for everything.
    pub colors: Vec<[u8; 3]>,
    /// Red, green, blue and opacity of the background; transparent by
    /// default.
    pub background: [u8; 4],
}

impl Default for Style {
    fn default() -> Self {
        Self {
            colors: DEFAULT_COLORS.to_vec(),
            background: [0, 0, 0, 0],
        }
    }
}

type Vector = [f64; 3];

fn dot(a: Vector, b: Vector) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn sub(a: Vector, b: Vector) -> Vector {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: Vector, b: Vector) -> Vector {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// `v` scaled to length 1; `None` for a zero vector.
fn normalized(v: Vector) -> Option<Vector> {
    let length = dot(v, v).sqrt();
    (length > 0.0).then(|| [v[0] / length, v[1] / length, v[2] / length])
}

/// The directions of the view in model coordinates.
struct View {
    /// To the right on the image.
    right: Vector,
    /// Up on the image.
    up: Vector,
    /// From the eye into the picture.
    forward: Vector,
    /// Towards the light.
    light: Vector,
}

impl View {
    fn new() -> Self {
        let (sin_a, cos_a) = sin_cos_degrees(AZIMUTH_DEGREES);
        let (sin_e, cos_e) = sin_cos_degrees(ELEVATION_DEGREES);
        // Seen from the front (azimuth 0) the eye looks along +Y.
        let level = [-sin_a, cos_a, 0.0];
        let right = [cos_a, sin_a, 0.0];
        let forward = [level[0] * cos_e, level[1] * cos_e, -sin_e];
        let up = [level[0] * sin_e, level[1] * sin_e, cos_e];
        // Light from behind the eye, above and from the left.
        let towards = [
            -forward[0] + 0.9 * up[0] - 0.5 * right[0],
            -forward[1] + 0.9 * up[1] - 0.5 * right[1],
            -forward[2] + 0.9 * up[2] - 0.5 * right[2],
        ];
        Self {
            right,
            up,
            forward,
            light: normalized(towards).unwrap_or(up),
        }
    }

    /// Position on the image (right, up) and depth (away from the eye).
    fn project(&self, p: Vector) -> Vector {
        [dot(p, self.right), dot(p, self.up), dot(p, self.forward)]
    }
}

/// Draws `meshes` (in plate coordinates, Z up) into an image of
/// `width × height` pixels, rows from top to bottom, four bytes per pixel
/// (red, green, blue, opacity; see [`crate::png::ColorType::Rgba`]). The
/// models fill the image apart from a small margin, whatever their size.
/// Without triangles the image is all background.
pub fn render(meshes: &[Mesh], width: u32, height: u32, style: &Style) -> Vec<u8> {
    let (width, height) = (width as usize, height as usize);
    let mut image = Vec::with_capacity(width * height * 4);
    for _ in 0..width * height {
        image.extend_from_slice(&style.background);
    }
    // More samples for small images, where a pixel is a large step.
    let samples = if width.max(height) > 400 { 2 } else { 3 };
    let (columns, rows) = (width * samples, height * samples);

    let view = View::new();
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for mesh in meshes {
        for triangle in &mesh.triangles {
            for &index in triangle {
                if let Some(&vertex) = mesh.vertices.get(index as usize) {
                    let p = view.project(vertex);
                    for axis in 0..2 {
                        lo[axis] = lo[axis].min(p[axis]);
                        hi[axis] = hi[axis].max(p[axis]);
                    }
                }
            }
        }
    }
    if columns == 0 || rows == 0 || !(lo[0] <= hi[0] && lo[1] <= hi[1]) {
        return image;
    }
    let span = [(hi[0] - lo[0]).max(1e-9), (hi[1] - lo[1]).max(1e-9)];
    let usable = 1.0 - 2.0 * MARGIN;
    let scale = (columns as f64 * usable / span[0]).min(rows as f64 * usable / span[1]);
    let centre = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    // Sample coordinates: x to the right, y down from the top edge.
    let to_samples = |p: Vector| {
        [
            columns as f64 / 2.0 + (p[0] - centre[0]) * scale,
            rows as f64 / 2.0 - (p[1] - centre[1]) * scale,
            p[2],
        ]
    };

    let mut depth = vec![f64::INFINITY; columns * rows];
    let mut colour = vec![[0u8; 3]; columns * rows];
    let base_colors: &[[u8; 3]] = if style.colors.is_empty() {
        &DEFAULT_COLORS[..1]
    } else {
        &style.colors
    };
    for mesh in meshes {
        for (index, triangle) in mesh.triangles.iter().enumerate() {
            let vertex = |corner: usize| mesh.vertices.get(triangle[corner] as usize).copied();
            let (Some(p0), Some(p1), Some(p2)) = (vertex(0), vertex(1), vertex(2)) else {
                continue;
            };
            let corners = [p0, p1, p2];
            let Some(mut normal) = normalized(cross(
                sub(corners[1], corners[0]),
                sub(corners[2], corners[0]),
            )) else {
                continue;
            };
            // Both sides of a surface are lit alike, so that meshes wound
            // the wrong way still look right.
            if dot(normal, view.forward) > 0.0 {
                normal = [-normal[0], -normal[1], -normal[2]];
            }
            let brightness = AMBIENT + DIFFUSE * dot(normal, view.light).max(0.0);
            let base = base_colors[mesh.extruder(index) as usize % base_colors.len()];
            let shaded = base.map(|c| (f64::from(c) * brightness + 0.5).min(255.0) as u8);
            let [a, b, c] = [0, 1, 2].map(|i| to_samples(view.project(corners[i])));
            fill([a, b, c], [columns, rows], &mut depth, &mut colour, shaded);
        }
    }

    // Average the samples of every pixel over the background.
    let count = (samples * samples) as f64;
    let background_alpha = f64::from(style.background[3]) / 255.0;
    for y in 0..height {
        for x in 0..width {
            let mut covered = 0.0;
            let mut sum = [0.0; 3];
            for sy in 0..samples {
                let row = (y * samples + sy) * columns;
                for sx in 0..samples {
                    let at = row + x * samples + sx;
                    if depth[at].is_finite() {
                        covered += 1.0;
                        for (total, &c) in sum.iter_mut().zip(&colour[at]) {
                            *total += f64::from(c);
                        }
                    }
                }
            }
            if covered == 0.0 {
                continue;
            }
            let open = count - covered;
            let alpha = (covered + open * background_alpha) / count;
            let pixel = &mut image[(y * width + x) * 4..][..4];
            for (channel, (total, &back)) in sum.iter().zip(&style.background).enumerate() {
                let premultiplied = (total + open * background_alpha * f64::from(back)) / count;
                pixel[channel] = (premultiplied / alpha + 0.5).min(255.0) as u8;
            }
            pixel[3] = (alpha * 255.0 + 0.5).min(255.0) as u8;
        }
    }
    image
}

/// Twice the signed area of the triangle `a`, `b`, `p`.
fn edge(a: Vector, b: Vector, p: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}

/// Fills a triangle given in sample coordinates (with depth) into the
/// buffers of `size` samples where it is nearer than what is there. A
/// sample is covered if its centre lies inside or on an edge.
fn fill(
    [a, b, c]: [Vector; 3],
    size: [usize; 2],
    depth: &mut [f64],
    colour: &mut [[u8; 3]],
    shade: [u8; 3],
) {
    let area = edge(a, b, [c[0], c[1]]);
    if area == 0.0 || !area.is_finite() {
        return;
    }
    let range = |axis: usize, limit: usize| {
        let lo = a[axis].min(b[axis]).min(c[axis]) - 0.5;
        let hi = a[axis].max(b[axis]).max(c[axis]) - 0.5;
        let first = lo.ceil().clamp(0.0, limit as f64) as usize;
        let end = (hi.floor() + 1.0).clamp(0.0, limit as f64) as usize;
        first..end
    };
    let xs = range(0, size[0]);
    for y in range(1, size[1]) {
        let py = y as f64 + 0.5;
        for x in xs.clone() {
            let p = [x as f64 + 0.5, py];
            let wa = edge(b, c, p) / area;
            let wb = edge(c, a, p) / area;
            let wc = edge(a, b, p) / area;
            if wa < 0.0 || wb < 0.0 || wc < 0.0 {
                continue;
            }
            let z = wa * a[2] + wb * b[2] + wc * c[2];
            let at = y * size[0] + x;
            if z < depth[at] {
                depth[at] = z;
                colour[at] = shade;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn alpha(image: &[u8], width: usize, x: usize, y: usize) -> u8 {
        image[(y * width + x) * 4 + 3]
    }

    #[test]
    fn cube_fills_the_middle_and_leaves_the_corners() {
        let cube = Mesh::cuboid(20.0, 20.0, 20.0);
        let image = render(std::slice::from_ref(&cube), 48, 32, &Style::default());
        assert_eq!(image.len(), 48 * 32 * 4);
        assert_eq!(alpha(&image, 48, 24, 16), 255);
        for (x, y) in [(0, 0), (47, 0), (0, 31), (47, 31)] {
            assert_eq!(alpha(&image, 48, x, y), 0, "corner {x}, {y}");
        }
        // The top, the front and the right side are lit differently.
        let middle = &image[(16 * 48 + 24) * 4..][..3];
        let top = &image[(5 * 48 + 24) * 4..][..3];
        assert_ne!(middle, top);
        // The same mesh gives the same bytes, also moved: the view is framed.
        assert_eq!(
            render(std::slice::from_ref(&cube), 48, 32, &Style::default()),
            image
        );
        let mut moved = cube;
        moved.translate([100.0, 50.0, 0.0]);
        assert_eq!(render(&[moved], 48, 32, &Style::default()), image);
    }

    #[test]
    fn background_and_colours_come_from_the_style() {
        let style = Style {
            colors: vec![[0, 0, 255]],
            background: [255, 255, 255, 255],
        };
        let image = render(&[Mesh::cuboid(10.0, 10.0, 10.0)], 16, 16, &style);
        assert_eq!(&image[..4], &[255, 255, 255, 255]);
        let middle = &image[(8 * 16 + 8) * 4..][..4];
        assert_eq!((middle[0], middle[1], middle[3]), (0, 0, 255));
        assert!(middle[2] > 0);
    }

    #[test]
    fn nothing_to_draw_gives_the_background() {
        let image = render(&[], 4, 3, &Style::default());
        assert_eq!(image, vec![0; 4 * 3 * 4]);
        assert!(render(&[Mesh::cuboid(1.0, 1.0, 1.0)], 0, 0, &Style::default()).is_empty());
    }
}
