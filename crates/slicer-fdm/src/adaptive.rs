//! Adaptive layer heights: thin layers where the surface is shallow, thick
//! ones where it is steep.
//!
//! A layer of thickness `h` leaves a step on a surface whose normal makes the
//! angle `θ` with the vertical; the height of that step measured along the
//! normal (the cusp height) is `h · |cos θ|` (Dolenc and Mäkelä, "Slicing
//! procedures for layered manufacturing techniques", Computer-Aided Design
//! 26(2), 1994). Every layer is as thick as it can be while that height
//! stays at or below `min_layer_height` for every triangle it crosses, so a
//! nearly flat surface gets `min_layer_height` and a vertical wall
//! `max_layer_height`. Only square roots and basic arithmetic, so the result
//! is the same on every platform.

use slicer_core::Mesh;
use slicer_profiles::Settings;

/// Height of the bands in which the thinnest layer every triangle allows is
/// collected, in millimetres.
const BAND: f64 = 0.01;

/// Differences in height below this count as none, in millimetres.
const EPSILON: f64 = 1e-6;

/// Top and thickness of every layer of a model standing on the bed and
/// `height` tall. The first layer is `first_layer_height` thick; the others
/// lie between `min_layer_height` and `max_layer_height` and end on every
/// flat surface they can reach. What is left above the last layer is less
/// than `min_layer_height`.
// ponytail: a flat surface closer than `min_layer_height` above the start of
// a layer is not hit; that layer straddles it. Thickness may jump from one
// layer to the next without a gradual transition.
pub(crate) fn plan(mesh: &Mesh, height: f64, settings: &Settings) -> (Vec<f64>, Vec<f64>) {
    let (thinnest, thickest) = (settings.min_layer_height, settings.max_layer_height);
    let first = settings.first_layer_height;
    if first > height + EPSILON {
        return (Vec::new(), Vec::new());
    }
    // The thickest layer allowed in every band, and the heights of all flat
    // surfaces including the top of the model.
    let bands = (height / BAND) as usize + 1;
    let mut allowed = vec![thickest; bands];
    let mut flats = vec![height];
    for triangle in &mesh.triangles {
        let [a, b, c] = triangle.map(|i| mesh.vertices[i as usize]);
        let low = a[2].min(b[2]).min(c[2]);
        let high = a[2].max(b[2]).max(c[2]);
        if high - low < EPSILON {
            flats.push(low);
            continue;
        }
        let (u, v) = (sub(b, a), sub(c, a));
        let normal = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let length = normal.iter().map(|n| n * n).sum::<f64>().sqrt();
        if normal[2] == 0.0 {
            continue;
        }
        // Cusp height h·|cos θ| = thinnest  =>  h = thinnest / |cos θ|.
        let limit = thinnest * length / normal[2].abs();
        if limit >= thickest {
            continue;
        }
        let last = ((high / BAND) as usize).min(bands - 1);
        let first_band = ((low.max(0.0) / BAND) as usize).min(last);
        for band in &mut allowed[first_band..=last] {
            *band = band.min(limit);
        }
    }
    flats.sort_by(f64::total_cmp);
    flats.dedup();

    let (mut tops, mut heights) = (vec![first], vec![first]);
    let mut z = first;
    loop {
        // Grow the layer band by band while it is thinner than every band
        // it reaches into allows.
        let mut h = thickest;
        let mut band = (z / BAND) as usize;
        while band < bands && band as f64 * BAND < z + h {
            h = h.min(allowed[band]);
            band += 1;
        }
        let h = h.max(thinnest);
        // End on the next flat surface the layer reaches; when none is at
        // least `thinnest` above, the model is finished.
        let next = flats.partition_point(|&f| f <= z + thinnest - EPSILON);
        let Some(&flat) = flats.get(next) else {
            break;
        };
        // Two equal layers instead of one too thin to print before it.
        let gap = flat - z;
        let top = if gap < h + EPSILON {
            flat
        } else if gap < h + thinnest && gap / 2.0 > thinnest - EPSILON {
            z + gap / 2.0
        } else {
            z + h
        };
        heights.push(top - z);
        tops.push(top);
        z = top;
    }
    (tops, heights)
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
