//! Raised lettering for the calibration prints: a small font of bars for
//! the characters the values are written with (digits, `.`, `+` and `-`),
//! turned into prisms that stand out of a flat face of a test model.
//!
//! A digit is made of up to seven bars in fixed places, like the display
//! of a pocket calculator: three crossing bars (top, middle, bottom) and
//! four upright ones. Bars of a character overlap, and every bar reaches a
//! little into the model; the slicer prints overlapping shells as their
//! union. Where two bars overlap, an upright one ends in the middle of the
//! crossing one, so that no two bars share an edge.
//!
//! Own design.

use slicer_core::Mesh;

/// Sizes of lettering, in mm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Style {
    /// Height of a character.
    pub height: f64,
    /// Width of a digit.
    pub width: f64,
    /// Width of the upright bars, along the line of text.
    pub upright: f64,
    /// Thickness of the crossing bars, up the characters.
    pub cross: f64,
    /// Space between two characters.
    pub spacing: f64,
    /// How far the bars stand out of the face.
    pub raise: f64,
}

impl Style {
    /// This style, or a narrower one if `text` would be wider than `room`:
    /// digits only as wide as their two upright bars and a gap of one bar,
    /// and less space between the characters.
    // ponytail: text too wide even then sticks out over the face; values
    // with that many digits make no sense for the prints here.
    pub fn fitting(self, text: &str, room: f64) -> Style {
        if width(text, &self) <= room {
            return self;
        }
        Style {
            width: self.width.min(3.0 * self.upright),
            spacing: self.spacing.min(0.8 * self.upright),
            ..self
        }
    }
}

/// How far every bar reaches into the model, in mm.
const SINK: f64 = 0.5;

/// A bar in the plane of the text: left, bottom, right, top, in mm from
/// the left end of the base line of its character.
pub(super) type Bar = [f64; 4];

/// The places of the bars of a digit, as bits.
const TOP: u8 = 1;
const UPPER_RIGHT: u8 = 1 << 1;
const LOWER_RIGHT: u8 = 1 << 2;
const BOTTOM: u8 = 1 << 3;
const LOWER_LEFT: u8 = 1 << 4;
const UPPER_LEFT: u8 = 1 << 5;
const MIDDLE: u8 = 1 << 6;

/// The bars of each digit. The 1 is drawn as a single narrow bar instead
/// (see [`glyph`]); the 6 and the 9 have their tails, the 7 has none.
pub(super) const DIGITS: [u8; 10] = [
    TOP | UPPER_RIGHT | LOWER_RIGHT | BOTTOM | LOWER_LEFT | UPPER_LEFT,
    UPPER_RIGHT | LOWER_RIGHT,
    TOP | UPPER_RIGHT | MIDDLE | LOWER_LEFT | BOTTOM,
    TOP | UPPER_RIGHT | MIDDLE | LOWER_RIGHT | BOTTOM,
    UPPER_LEFT | MIDDLE | UPPER_RIGHT | LOWER_RIGHT,
    TOP | UPPER_LEFT | MIDDLE | LOWER_RIGHT | BOTTOM,
    TOP | UPPER_LEFT | MIDDLE | LOWER_LEFT | LOWER_RIGHT | BOTTOM,
    TOP | UPPER_RIGHT | LOWER_RIGHT,
    TOP | UPPER_RIGHT | LOWER_RIGHT | BOTTOM | LOWER_LEFT | UPPER_LEFT | MIDDLE,
    TOP | UPPER_LEFT | UPPER_RIGHT | MIDDLE | LOWER_RIGHT | BOTTOM,
];

/// The seven places of a digit with all its bars at full length, in the
/// order of the bits. Each place has a middle no other place covers.
pub(super) fn places(style: &Style) -> [Bar; 7] {
    let Style {
        height: h,
        width: w,
        upright: u,
        cross: c,
        ..
    } = *style;
    let (low, high) = ((h - c) / 2.0, (h + c) / 2.0);
    [
        [0.0, h - c, w, h],
        [w - u, low, w, h],
        [w - u, 0.0, w, high],
        [0.0, 0.0, w, c],
        [0.0, 0.0, u, high],
        [0.0, low, u, h],
        [0.0, low, w, high],
    ]
}

/// The bars of the digit with the bits `bits`. An upright bar runs to the
/// middle of the crossing bar at either end, or to the edge of the digit
/// where that one is missing.
fn digit(bits: u8, style: &Style) -> Vec<Bar> {
    let Style {
        height: h,
        cross: c,
        ..
    } = *style;
    let place = places(style);
    // How far up an upright bar may reach from the bottom and down from
    // the top, and where the two halves meet.
    let foot = if bits & BOTTOM != 0 { c / 2.0 } else { 0.0 };
    let head = if bits & TOP != 0 { h - c / 2.0 } else { h };
    let (low, high) = if bits & MIDDLE != 0 {
        (h / 2.0, h / 2.0)
    } else {
        // Without a middle bar the halves overlap instead.
        ((h - c) / 2.0, (h + c) / 2.0)
    };
    (0..7)
        .filter(|k| bits & (1 << k) != 0)
        .map(|k| {
            let [left, _, right, _] = place[k];
            match 1 << k {
                UPPER_RIGHT | UPPER_LEFT => [left, low, right, head],
                LOWER_RIGHT | LOWER_LEFT => [left, foot, right, high],
                _ => place[k],
            }
        })
        .collect()
}

/// The bars of character `c` and its width; `None` for a character
/// without a glyph.
pub(super) fn glyph(c: char, style: &Style) -> Option<(Vec<Bar>, f64)> {
    let Style {
        height: h,
        width: w,
        upright: u,
        cross: x,
        ..
    } = *style;
    let middle = [0.0, (h - x) / 2.0, w, (h + x) / 2.0];
    Some(match c {
        '1' => (vec![[0.0, 0.0, u, h]], u),
        '.' => (vec![[0.0, 0.0, u, x]], u),
        '-' => (vec![middle], w),
        '+' => {
            let stem = [(w - u) / 2.0, (h - w) / 2.0, (w + u) / 2.0, (h + w) / 2.0];
            (vec![middle, stem], w)
        }
        _ => {
            let bits = DIGITS[c.to_digit(10)? as usize];
            (digit(bits, style), w)
        }
    })
}

/// The characters of `text` with the left end of each; characters without
/// a glyph leave a gap as wide as a digit.
pub(super) fn layout(text: &str, style: &Style) -> Vec<(char, f64)> {
    let mut left = 0.0;
    let mut placed = Vec::new();
    for c in text.chars() {
        placed.push((c, left));
        let advance = glyph(c, style).map_or(style.width, |(_, width)| width);
        left += advance + style.spacing;
    }
    placed
}

/// Width of `text` in `style`, in mm.
pub(super) fn width(text: &str, style: &Style) -> f64 {
    let end =
        |&(c, left): &(char, f64)| left + glyph(c, style).map_or(style.width, |(_, width)| width);
    layout(text, style).last().map_or(0.0, end)
}

/// A flat face of a model to put lettering on. The directions are unit
/// vectors at right angles to each other.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Face {
    /// Point on the face where the middle of the lettering goes.
    pub centre: [f64; 3],
    /// Reading direction.
    pub across: [f64; 3],
    /// Up the characters.
    pub up: [f64; 3],
    /// Out of the face.
    pub out: [f64; 3],
}

impl Face {
    /// The point `along` the line of text and `up` from the base line of
    /// lettering `width` wide and `height` high, `out` in front of the face.
    pub fn point(&self, [along, up, out]: [f64; 3], width: f64, height: f64) -> [f64; 3] {
        let (along, up) = (along - width / 2.0, up - height / 2.0);
        std::array::from_fn(|a| {
            self.centre[a] + along * self.across[a] + up * self.up[a] + out * self.out[a]
        })
    }
}

/// `text` in `style` standing out of `face`, centred on it. On an upright
/// face the underside of every bar slopes at 45°, so it prints without
/// support.
pub(super) fn raised(text: &str, style: &Style, face: &Face) -> Mesh {
    let width = width(text, style);
    let upright_face = face.out[2] == 0.0;
    let raise = style.raise;
    let mut mesh = Mesh::default();
    for (c, left) in layout(text, style) {
        let Some((bars, _)) = glyph(c, style) else {
            continue;
        };
        for [l, bottom, r, top] in bars {
            // The cross-section of the bar: (up, out) pairs.
            let mut profile = vec![[bottom, -SINK], [top, -SINK], [top, raise]];
            if upright_face && top - bottom > raise {
                profile.extend([[bottom + raise, raise], [bottom, 0.0]]);
            } else {
                profile.push([bottom, raise]);
            }
            let end = |along: f64| -> Vec<[f64; 3]> {
                (profile.iter())
                    .map(|&[up, out]| face.point([left + along, up, out], width, style.height))
                    .collect()
            };
            mesh.merge(&prism(&end(l), &end(r)));
        }
    }
    mesh
}

/// The closed solid between two copies of a convex polygon, `start` and
/// `end` (corresponding corners in the same order), facing outward.
fn prism(start: &[[f64; 3]], end: &[[f64; 3]]) -> Mesh {
    let n = start.len() as u32;
    let mut mesh = Mesh {
        vertices: [start, end].concat(),
        ..Mesh::default()
    };
    for k in 1..n - 1 {
        mesh.triangles
            .extend([[0, k + 1, k], [n, n + k, n + k + 1]]);
    }
    for k in 0..n {
        let j = (k + 1) % n;
        mesh.triangles.extend([[k, j, n + j], [k, n + j, n + k]]);
    }
    if mesh.signed_volume() < 0.0 {
        mesh.flip();
    }
    mesh
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const STYLE: Style = Style {
        height: 6.0,
        width: 3.6,
        upright: 1.0,
        cross: 1.0,
        spacing: 1.2,
        raise: 0.6,
    };

    const FRONT: Face = Face {
        centre: [0.0, 0.0, 10.0],
        across: [1.0, 0.0, 0.0],
        up: [0.0, 0.0, 1.0],
        out: [0.0, -1.0, 0.0],
    };

    #[test]
    fn characters_are_laid_out_left_to_right() {
        let placed = layout("+12.5", &STYLE);
        let lefts: Vec<f64> = placed.iter().map(|&(_, left)| left).collect();
        // + and digits 3.6 wide, 1 and . 1.0 wide, 1.2 between them.
        let expected = [0.0, 4.8, 7.0, 11.8, 14.0];
        assert!(
            lefts
                .iter()
                .zip(expected)
                .all(|(a, b)| (a - b).abs() < 1e-9)
        );
        assert!((width("+12.5", &STYLE) - 17.6).abs() < 1e-9);
        assert_eq!(width("", &STYLE), 0.0);
        // Unknown characters leave a gap.
        assert!((width("2 2", &STYLE) - (3.0 * 3.6 + 2.0 * 1.2)).abs() < 1e-9);
        assert_eq!(glyph('x', &STYLE), None);
        // An 8 has all seven bars, a 7 three.
        assert_eq!(glyph('8', &STYLE).unwrap().0.len(), 7);
        assert_eq!(glyph('7', &STYLE).unwrap().0.len(), 3);
    }

    #[test]
    fn wide_text_gets_narrower() {
        let narrow = STYLE.fitting("222.5", 18.0);
        assert!(width("222.5", &STYLE) > 18.0);
        assert!(width("222.5", &narrow) <= 18.0);
        assert_eq!(narrow.height, STYLE.height);
        assert_eq!(STYLE.fitting("215", 18.0), STYLE);
    }

    #[test]
    fn bars_are_closed_solids_reaching_into_the_face() {
        let mesh = raised("0123456789.-+", &STYLE, &FRONT);
        assert_eq!(mesh.open_edge_count(), 0);
        let mut diagnostics = slicer_core::Diagnostics::new();
        mesh.check(&mut diagnostics);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let (lo, hi) = mesh.bounds().unwrap();
        let width = width("0123456789.-+", &STYLE);
        let expected_lo = [-width / 2.0, -STYLE.raise, 7.0];
        let expected_hi = [width / 2.0, SINK, 13.0];
        for a in 0..3 {
            assert!((lo[a] - expected_lo[a]).abs() < 1e-9, "{lo:?}");
            assert!((hi[a] - expected_hi[a]).abs() < 1e-9, "{hi:?}");
        }
        // Every bar faces outward on its own.
        let bar = raised("-", &STYLE, &FRONT);
        assert!(bar.signed_volume() > 0.0);
        // Chamfered underneath on an upright face: a 3.6 long bar, 1.0
        // high, 1.1 deep, less the 0.6 x 0.6 / 2 slope.
        let expected = 3.6 * (1.0 * 1.1 - 0.18);
        assert!((bar.volume() - expected).abs() < 1e-9, "{}", bar.volume());
        // On a level face it is a plain box.
        let top = Face {
            centre: [0.0, 0.0, 3.0],
            across: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            out: [0.0, 0.0, 1.0],
        };
        let bar = raised("-", &STYLE, &top);
        assert!((bar.volume() - 3.6 * 1.0 * 1.1).abs() < 1e-9);
        assert!(bar.signed_volume() > 0.0);
    }
}
