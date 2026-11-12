//! Cutting a mesh into horizontal layers.

use std::collections::HashMap;

use crate::diagnostics::{Diagnostic, Diagnostics};
use crate::mesh::Mesh;
use crate::polygon::{self, Contour, Point, Shapes};

/// One end of a cut segment: the mesh edge it lies on and its position.
#[derive(Clone, Copy)]
struct End {
    edge: u64,
    point: Point,
}

fn edge_key(a: u32, b: u32) -> u64 {
    (u64::from(a.min(b)) << 32) | u64::from(a.max(b))
}

/// Intersects the mesh with horizontal planes at the given heights.
///
/// `heights` must be sorted ascending. Returns one set of shapes per height.
/// A vertex lying exactly on a plane counts as below it, so closed meshes
/// always yield closed contours. Chains that do not close (holes in the
/// mesh) are dropped.
pub fn slice_mesh(mesh: &Mesh, heights: &[f64]) -> Vec<Shapes> {
    slice_mesh_reporting(mesh, heights, &mut Diagnostics::new())
}

/// Like [`slice_mesh`], and reports layers in which contours did not close.
pub fn slice_mesh_reporting(
    mesh: &Mesh,
    heights: &[f64],
    diagnostics: &mut Diagnostics,
) -> Vec<Shapes> {
    let mut segments: Vec<Vec<[End; 2]>> = vec![Vec::new(); heights.len()];

    for tri in &mesh.triangles {
        let v = tri.map(|i| mesh.vertices[i as usize]);
        let z_min = v[0][2].min(v[1][2]).min(v[2][2]);
        let z_max = v[0][2].max(v[1][2]).max(v[2][2]);
        let first = heights.partition_point(|&z| z < z_min);
        for (layer, &z) in heights.iter().enumerate().skip(first) {
            if z >= z_max {
                break;
            }
            let mut ends: [Option<End>; 2] = [None, None];
            let mut count = 0;
            for k in 0..3 {
                let (i, j) = (tri[k], tri[(k + 1) % 3]);
                // Always interpolate from the lower index so both triangles
                // sharing this edge compute bit-identical points.
                let (lo, hi) = (i.min(j), i.max(j));
                let (p, q) = (mesh.vertices[lo as usize], mesh.vertices[hi as usize]);
                if (p[2] > z) == (q[2] > z) {
                    continue;
                }
                let t = (z - p[2]) / (q[2] - p[2]);
                // With outward-facing triangles, walking from the edge that
                // goes down through the plane to the one that comes up keeps
                // the solid on the left.
                let rises = mesh.vertices[i as usize][2] <= z;
                if count < 2 {
                    ends[usize::from(rises)] = Some(End {
                        edge: edge_key(i, j),
                        point: [p[0] + t * (q[0] - p[0]), p[1] + t * (q[1] - p[1])],
                    });
                }
                count += 1;
            }
            if let (2, [Some(a), Some(b)]) = (count, ends) {
                segments[layer].push([a, b]);
            }
        }
    }

    // A consistently oriented mesh tells inside from outside, so overlapping
    // shells can be united. Otherwise fall back to counting crossings, which
    // tolerates flipped triangles but turns overlaps into holes.
    let oriented = mesh.open_edge_count() == 0;
    let mut open_layers = Vec::new();
    let slices = segments
        .iter()
        .enumerate()
        .map(|(index, layer)| {
            let (loops, open) = chain_segments(layer);
            if open > 0 {
                open_layers.push(index);
            }
            if oriented {
                polygon::from_oriented_loops(&loops)
            } else {
                polygon::from_loops(&loops)
            }
        })
        .collect();
    if let Some(first) = open_layers.first() {
        diagnostics.push(
            Diagnostic::warning(
                "slicing.open_contours",
                format!(
                    "{} of {} layers have contours that do not close and were dropped (first: layer {})",
                    open_layers.len(),
                    heights.len(),
                    first + 1
                ),
            )
            .with("layers", open_layers.len())
            .with("first_layer", first + 1),
        );
    }
    slices
}

/// Joins segments that share mesh edges into closed loops. Also returns the
/// number of chains that did not close.
fn chain_segments(segments: &[[End; 2]]) -> (Vec<Contour>, usize) {
    let mut by_edge: HashMap<u64, Vec<usize>> = HashMap::new();
    for (index, segment) in segments.iter().enumerate() {
        for end in segment {
            by_edge.entry(end.edge).or_default().push(index);
        }
    }

    let mut used = vec![false; segments.len()];
    let mut loops = Vec::new();
    let mut open = 0;
    for start in 0..segments.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let origin = segments[start][0].edge;
        let mut contour = vec![segments[start][0].point];
        let mut current = segments[start][1];
        // Counts segments walked in their natural direction minus those
        // walked backwards.
        let mut forward: i64 = 1;
        let closed = loop {
            if current.edge == origin {
                break true;
            }
            contour.push(current.point);
            let next = by_edge
                .get(&current.edge)
                .and_then(|candidates| candidates.iter().copied().find(|&i| !used[i]));
            let Some(next) = next else { break false };
            used[next] = true;
            let segment = segments[next];
            current = if segment[0].edge == current.edge {
                forward += 1;
                segment[1]
            } else {
                forward -= 1;
                segment[0]
            };
        };
        if forward < 0 {
            contour.reverse();
        }
        if !closed {
            open += 1;
        } else if contour.len() >= 3 {
            loops.push(contour);
        }
    }
    (loops, open)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::polygon::area;

    #[test]
    fn cube_layers_are_squares() {
        let mesh = Mesh::cuboid(20.0, 20.0, 20.0);
        let heights: Vec<f64> = (0..100).map(|i| 0.1 + 0.2 * f64::from(i)).collect();
        let layers = slice_mesh(&mesh, &heights);
        assert_eq!(layers.len(), 100);
        for shapes in &layers {
            assert_eq!(shapes.len(), 1);
            assert_eq!(shapes[0].len(), 1);
            assert!((area(shapes) - 400.0).abs() < 1e-3);
        }
    }

    #[test]
    fn planes_through_vertices_and_outside_the_mesh() {
        let mesh = Mesh::cuboid(10.0, 10.0, 10.0);
        let layers = slice_mesh(&mesh, &[-1.0, 0.0, 5.0, 10.0, 11.0]);
        let areas: Vec<f64> = layers.iter().map(|s| area(s)).collect();
        // The bottom face (z = 0) counts as inside, the top face does not.
        assert_eq!(
            areas.iter().map(|a| a.round() as i32).collect::<Vec<_>>(),
            [0, 100, 100, 0, 0]
        );
    }

    #[test]
    fn open_mesh_does_not_panic() {
        let mut mesh = Mesh::cuboid(10.0, 10.0, 10.0);
        mesh.triangles.remove(4);
        let mut diagnostics = Diagnostics::new();
        let layers = slice_mesh_reporting(&mesh, &[2.0, 5.0], &mut diagnostics);
        assert_eq!(layers.len(), 2);
        assert_eq!(diagnostics.codes(), ["slicing.open_contours"]);
        assert_eq!(diagnostics.as_slice()[0].context["layers"], "2");
    }

    #[test]
    fn overlapping_shells_are_united() {
        // Two boxes overlapping by half: the shared part must stay solid.
        let mut mesh = Mesh::cuboid(10.0, 10.0, 10.0);
        let mut other = Mesh::cuboid(10.0, 10.0, 10.0);
        other.translate([5.0, 0.0, 0.0]);
        mesh.merge(&other);
        let layers = slice_mesh(&mesh, &[5.0]);
        assert_eq!(layers[0].len(), 1);
        assert!((area(&layers[0]) - 150.0).abs() < 1e-3);

        // Turning the whole mesh inside out must not change the result.
        mesh.flip();
        let flipped = slice_mesh(&mesh, &[5.0]);
        assert!((area(&flipped[0]) - 150.0).abs() < 1e-3);
    }

    #[test]
    fn holes_survive_in_oriented_meshes() {
        let mesh = crate::primitives::tube(10.0, 6.0, 5.0, 64);
        let layers = slice_mesh(&mesh, &[2.5]);
        assert_eq!(layers[0].len(), 1);
        assert_eq!(layers[0][0].len(), 2);
        let expected = std::f64::consts::PI * (100.0 - 36.0);
        assert!((area(&layers[0]) - expected).abs() < 0.01 * expected);
    }

    #[test]
    fn closed_mesh_reports_nothing() {
        let mut diagnostics = Diagnostics::new();
        slice_mesh_reporting(&Mesh::cuboid(10.0, 10.0, 10.0), &[5.0], &mut diagnostics);
        assert!(diagnostics.is_empty());
        Mesh::cuboid(1.0, 1.0, 1.0).check(&mut diagnostics);
        assert!(diagnostics.is_empty());
    }
}
