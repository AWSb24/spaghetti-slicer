//! Procedurally generated solids: boxes, bodies of revolution and prisms.
//!
//! Used as reference geometry in tests and as building blocks for example
//! models. All generators return closed meshes with outward-facing
//! triangles.

use std::collections::HashMap;
use std::f64::consts::{PI, TAU};

use crate::mesh::Mesh;
use crate::polygon::Point;

/// Assembles a mesh from triangles given by coordinates, merging identical
/// vertices and dropping degenerate triangles.
#[derive(Default)]
struct Builder {
    index: HashMap<[u64; 3], u32>,
    mesh: Mesh,
}

impl Builder {
    fn vertex(&mut self, p: [f64; 3]) -> u32 {
        // `+ 0.0` turns -0.0 into 0.0 so both map to the same vertex.
        let p = p.map(|c| c + 0.0);
        *self.index.entry(p.map(f64::to_bits)).or_insert_with(|| {
            self.mesh.vertices.push(p);
            (self.mesh.vertices.len() - 1) as u32
        })
    }

    fn triangle(&mut self, a: [f64; 3], b: [f64; 3], c: [f64; 3]) {
        let ids = [self.vertex(a), self.vertex(b), self.vertex(c)];
        if ids[0] != ids[1] && ids[1] != ids[2] && ids[0] != ids[2] {
            self.mesh.triangles.push(ids);
        }
    }

    fn quad(&mut self, a: [f64; 3], b: [f64; 3], c: [f64; 3], d: [f64; 3]) {
        self.triangle(a, b, c);
        self.triangle(a, c, d);
    }
}

/// Rotates a profile in the (radius, z) half-plane around the Z axis.
///
/// The profile must run counter-clockwise when radius points right and z
/// points up. An open profile must start and end on the axis (radius 0); a
/// `closed` profile connects its last point back to the first and must not
/// touch the axis.
pub fn revolve(profile: &[[f64; 2]], closed: bool, segments: usize) -> Mesh {
    let segments = segments.max(3);
    let at = |i: usize, j: usize| -> [f64; 3] {
        let [radius, z] = profile[i % profile.len()];
        let angle = TAU * (j % segments) as f64 / segments as f64;
        [radius * angle.cos(), radius * angle.sin(), z]
    };
    let spans = if closed {
        profile.len()
    } else {
        profile.len().saturating_sub(1)
    };
    let mut builder = Builder::default();
    for i in 0..spans {
        for j in 0..segments {
            builder.quad(at(i, j), at(i, j + 1), at(i + 1, j + 1), at(i + 1, j));
        }
    }
    builder.mesh
}

/// Extrudes a contour along Z, optionally twisting it around `center`.
///
/// The contour must run counter-clockwise and every point of it must be
/// visible from `center` (the caps are triangle fans around it).
/// `twist` is the total rotation in radians between bottom and top, spread
/// over `z_segments` rings.
pub fn extrude(
    contour: &[Point],
    center: Point,
    height: f64,
    z_segments: usize,
    twist: f64,
) -> Mesh {
    let rings = z_segments.max(1);
    let n = contour.len();
    let at = |i: usize, k: usize| -> [f64; 3] {
        let t = k as f64 / rings as f64;
        let (sin, cos) = (twist * t).sin_cos();
        let (dx, dy) = (contour[i % n][0] - center[0], contour[i % n][1] - center[1]);
        [
            center[0] + dx * cos - dy * sin,
            center[1] + dx * sin + dy * cos,
            height * t,
        ]
    };
    let mut builder = Builder::default();
    for k in 0..rings {
        for i in 0..n {
            builder.quad(at(i, k), at(i + 1, k), at(i + 1, k + 1), at(i, k + 1));
        }
    }
    let (bottom, top) = ([center[0], center[1], 0.0], [center[0], center[1], height]);
    for i in 0..n {
        builder.triangle(bottom, at(i + 1, 0), at(i, 0));
        builder.triangle(top, at(i, rings), at(i + 1, rings));
    }
    builder.mesh
}

/// A cylinder standing on the XY plane, centred on the Z axis.
pub fn cylinder(radius: f64, height: f64, segments: usize) -> Mesh {
    revolve(
        &[[0.0, 0.0], [radius, 0.0], [radius, height], [0.0, height]],
        false,
        segments,
    )
}

/// A cone standing on its base.
pub fn cone(radius: f64, height: f64, segments: usize) -> Mesh {
    revolve(&[[0.0, 0.0], [radius, 0.0], [0.0, height]], false, segments)
}

/// A sphere resting on the XY plane.
pub fn sphere(radius: f64, segments: usize) -> Mesh {
    let stacks = (segments / 2).max(2);
    let profile: Vec<[f64; 2]> = (0..=stacks)
        .map(|k| {
            let phi = PI * k as f64 / stacks as f64;
            // Force the poles exactly onto the axis.
            let r = if k == 0 || k == stacks {
                0.0
            } else {
                radius * phi.sin()
            };
            [r, radius - radius * phi.cos()]
        })
        .collect();
    revolve(&profile, false, segments)
}

/// The upper half of a sphere, flat side down.
pub fn dome(radius: f64, segments: usize) -> Mesh {
    let stacks = (segments / 4).max(2);
    let mut profile = vec![[0.0, 0.0]];
    profile.extend((0..=stacks).map(|k| {
        let t = PI / 2.0 * k as f64 / stacks as f64;
        if k == stacks {
            [0.0, radius]
        } else {
            [radius * t.cos(), radius * t.sin()]
        }
    }));
    revolve(&profile, false, segments)
}

/// A torus lying flat on the XY plane. `major` is the distance from the
/// centre to the middle of the tube, `minor` the tube radius.
pub fn torus(major: f64, minor: f64, segments: usize, tube_segments: usize) -> Mesh {
    let n = tube_segments.max(3);
    let profile: Vec<[f64; 2]> = (0..n)
        .map(|k| {
            let t = TAU * k as f64 / n as f64;
            [major + minor * t.cos(), minor + minor * t.sin()]
        })
        .collect();
    revolve(&profile, true, segments)
}

/// A hollow cylinder (pipe section) standing on the XY plane.
pub fn tube(outer_radius: f64, inner_radius: f64, height: f64, segments: usize) -> Mesh {
    let profile = [
        [inner_radius, 0.0],
        [outer_radius, 0.0],
        [outer_radius, height],
        [inner_radius, height],
    ];
    revolve(&profile, true, segments)
}

/// A regular polygon contour around the origin, counter-clockwise.
pub fn regular_polygon(radius: f64, sides: usize) -> Vec<Point> {
    (0..sides)
        .map(|i| {
            let a = TAU * i as f64 / sides as f64;
            [radius * a.cos(), radius * a.sin()]
        })
        .collect()
}

/// A star contour around the origin with `points` tips, counter-clockwise.
pub fn star_contour(points: usize, outer_radius: f64, inner_radius: f64) -> Vec<Point> {
    (0..points * 2)
        .map(|i| {
            let a = PI * i as f64 / points as f64;
            let r = if i % 2 == 0 {
                outer_radius
            } else {
                inner_radius
            };
            [r * a.cos(), r * a.sin()]
        })
        .collect()
}

/// A gear-like contour around the origin with rectangular teeth,
/// counter-clockwise.
pub fn gear_contour(teeth: usize, outer_radius: f64, tooth_depth: f64) -> Vec<Point> {
    (0..teeth * 4)
        .map(|i| {
            let a = TAU * i as f64 / (teeth * 4) as f64;
            let r = if i % 4 < 2 {
                outer_radius
            } else {
                outer_radius - tooth_depth
            };
            [r * a.cos(), r * a.sin()]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &str, mesh: &Mesh, expected_volume: f64, tolerance: f64) {
        assert_eq!(mesh.open_edge_count(), 0, "{name} is not closed");
        let volume = mesh.signed_volume();
        assert!(volume > 0.0, "{name} is inside out");
        assert!(
            (volume - expected_volume).abs() <= tolerance * expected_volume,
            "{name}: volume {volume}, expected {expected_volume}"
        );
    }

    #[test]
    fn solids_are_closed_outward_and_have_the_right_volume() {
        let (r, h) = (10.0, 20.0);
        check("cylinder", &cylinder(r, h, 128), PI * r * r * h, 0.002);
        check("cone", &cone(r, h, 128), PI * r * r * h / 3.0, 0.002);
        check("sphere", &sphere(r, 128), 4.0 / 3.0 * PI * r.powi(3), 0.003);
        check("dome", &dome(r, 128), 2.0 / 3.0 * PI * r.powi(3), 0.003);
        check(
            "torus",
            &torus(20.0, 5.0, 128, 64),
            2.0 * PI * PI * 20.0 * 25.0,
            0.004,
        );
        check(
            "tube",
            &tube(r, 6.0, h, 128),
            PI * (r * r - 36.0) * h,
            0.002,
        );
    }

    #[test]
    fn prisms() {
        let square = vec![[-5.0, -5.0], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]];
        check(
            "box",
            &extrude(&square, [0.0, 0.0], 7.0, 1, 0.0),
            700.0,
            1e-12,
        );
        // Twisting a prism around its axis keeps every cross-section, and so
        // the volume, almost unchanged.
        check(
            "twisted",
            &extrude(&square, [0.0, 0.0], 30.0, 60, PI / 2.0),
            3000.0,
            0.01,
        );
        let star = star_contour(5, 10.0, 4.0);
        let area = crate::polygon::signed_area(&star);
        check(
            "star",
            &extrude(&star, [0.0, 0.0], 3.0, 1, 0.0),
            area * 3.0,
            1e-9,
        );
        let gear = gear_contour(12, 10.0, 2.0);
        let area = crate::polygon::signed_area(&gear);
        check(
            "gear",
            &extrude(&gear, [0.0, 0.0], 3.0, 1, 0.0),
            area * 3.0,
            1e-9,
        );
        assert_eq!(regular_polygon(3.0, 6).len(), 6);
    }
}
