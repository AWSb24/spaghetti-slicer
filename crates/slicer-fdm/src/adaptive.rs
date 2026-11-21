//! Adaptive layer heights: thin layers where the surface is shallow, thick
//! ones where it is steep ([`plan`]); layer heights set by height ranges
//! ([`ranged`]); and both together or with a limit on how fast the layer
//! height may change from one layer to the next ([`smooth`]).
//!
//! A layer of thickness `h` leaves a step on a surface whose normal makes the
//! angle `θ` with the vertical; the height of that step measured along the
//! normal (the cusp height) is `h · |cos θ|` (Dolenc and Mäkelä, "Slicing
//! procedures for layered manufacturing techniques", Computer-Aided Design
//! 26(2), 1994). Every layer is as thick as it can be while that height
//! stays at or below the allowed step height for every triangle it crosses,
//! so a nearly flat surface gets `min_layer_height` and a vertical wall
//! `max_layer_height`. The allowed step height is `min_layer_height` at
//! `adaptive_layer_quality = 100` and `max_layer_height` at 0. Only square
//! roots and basic arithmetic, so the result is the same on every platform.

use slicer_core::Mesh;
use slicer_profiles::Settings;

/// Height of the bands in which the thinnest layer every triangle allows is
/// collected, in millimetres.
const BAND: f64 = 0.01;

/// Differences in height below this count as none, in millimetres.
const EPSILON: f64 = 1e-6;

/// The thickest layer the surface allows in every band of a model `height`
/// tall standing on the bed, between `min_layer_height` and
/// `max_layer_height`, and the heights of all its flat surfaces including
/// its top, sorted.
fn surface(mesh: &Mesh, height: f64, settings: &Settings) -> (Vec<f64>, Vec<f64>) {
    let (thinnest, thickest) = (settings.min_layer_height, settings.max_layer_height);
    // The allowed step height; exactly `thinnest` at quality 100.
    let cusp = thinnest + (1.0 - settings.adaptive_layer_quality / 100.0) * (thickest - thinnest);
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
        // Cusp height h·|cos θ| = cusp  =>  h = cusp / |cos θ|.
        let limit = cusp * length / normal[2].abs();
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
    (allowed, flats)
}

/// The thickest layer starting at `z` that is no thicker than `start` and
/// than every band of `field` below `end` it reaches into allows.
fn grow(field: &[f64], z: f64, start: f64, end: f64) -> f64 {
    let mut h = start;
    let mut band = (z / BAND) as usize;
    while band < field.len() && band as f64 * BAND < z + h && band as f64 * BAND < end - EPSILON {
        h = h.min(field[band]);
        band += 1;
    }
    h
}

/// Top and thickness of every layer of a model standing on the bed and
/// `height` tall. The first layer is `first_layer_height` thick; the others
/// lie between `min_layer_height` and `max_layer_height` and end on every
/// flat surface they can reach. What is left above the last layer is less
/// than `min_layer_height`.
// ponytail: a flat surface closer than `min_layer_height` above the start of
// a layer is not hit; that layer straddles it. Thickness may jump from one
// layer to the next; `smooth` limits that.
pub(crate) fn plan(mesh: &Mesh, height: f64, settings: &Settings) -> (Vec<f64>, Vec<f64>) {
    let (thinnest, thickest) = (settings.min_layer_height, settings.max_layer_height);
    let first = settings.first_layer_height;
    if first > height + EPSILON {
        return (Vec::new(), Vec::new());
    }
    // The thickest layer allowed in every band, and the heights of all flat
    // surfaces including the top of the model.
    let (allowed, flats) = surface(mesh, height, settings);

    let (mut tops, mut heights) = (vec![first], vec![first]);
    let mut z = first;
    loop {
        // Grow the layer band by band while it is thinner than every band
        // it reaches into allows.
        let h = grow(&allowed, z, thickest, f64::INFINITY).max(thinnest);
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

/// Top and thickness of every layer of a model standing on the bed and
/// `height` tall, whose height ranges `ranges` (from, to, layer height in
/// millimetres; later ones count where they overlap) set the layer height
/// there. The first layer is `first_layer_height` thick. The bounds of the
/// ranges split the height above it into stretches; every stretch gets the
/// layer height of the range holding its middle (`layer_height` outside
/// all of them) and as few layers as keep every one no thicker than that,
/// all equally thick, so that no layer crosses a bound and the middle of
/// every layer lies in the range it was made for. Above the last bound the
/// layers are as thick as asked for up to the top, as without ranges: what
/// is left above the last one is thinner than a layer.
// ponytail: a stretch that is not a whole number of its layer height gets
// slightly thinner layers throughout, and a stretch much shorter than its
// layer height (a bound just above another or above the first layer) one
// thin layer; `smooth` spreads the change over several layers instead.
pub(crate) fn ranged(
    height: f64,
    settings: &Settings,
    ranges: &[(f64, f64, f64)],
) -> (Vec<f64>, Vec<f64>) {
    let first = settings.first_layer_height;
    if first > height + EPSILON {
        return (Vec::new(), Vec::new());
    }
    let layer_at = |z: f64| range_at(ranges, z).unwrap_or(settings.layer_height);
    let mut bounds = range_bounds(ranges, first, height);
    bounds.dedup_by(|later, earlier| *later - *earlier < EPSILON);
    let (mut tops, mut heights) = (vec![first], vec![first]);
    let mut z = first;
    for end in bounds {
        let wanted = layer_at((z + end) / 2.0);
        let count = ((end - z) / wanted - EPSILON).ceil().max(1.0) as usize;
        let each = (end - z) / count as f64;
        for k in 1..count {
            tops.push(z + k as f64 * each);
            heights.push(each);
        }
        tops.push(end);
        heights.push(each);
        z = end;
    }
    let wanted = layer_at((z + height) / 2.0);
    for k in 1.. {
        let top = z + k as f64 * wanted;
        if top > height + EPSILON {
            break;
        }
        tops.push(top);
        heights.push(wanted);
    }
    (tops, heights)
}

/// Top and thickness of every layer of a model standing on the bed and
/// `height` tall, with adaptive layers (`adaptive_layers`) or with the
/// height ranges `ranges` (as in [`ranged`]) or both, and with
/// `layer_height_max_change` limiting how much thicker or thinner a layer
/// may be than the one below.
///
/// Every height gets the thickest layer it allows: inside a range with a
/// layer height that height; outside, the slope of the surface with
/// adaptive layers, or else `layer_height`. With a limit on the change,
/// these allowed heights are lowered so that they grow by no more than the
/// limit per millimetre of height (a forward and a backward pass); since a
/// layer is about as thick as the distance to the next, layers then change
/// by about the limit from one to the next and get thinner before thin
/// layers are needed, on the side of the thicker layers. The bounds of the
/// ranges, the flat surfaces of the model (with adaptive layers) and its
/// top split the height above the first layer into stretches that layers
/// end on exactly. Every stretch gets as few layers as the allowed heights
/// need; their heights lie between an upper bound (the allowed height, and
/// the limit on the change towards the layer below and between them) and
/// a lower bound (`min_layer_height` with adaptive layers, the thinnest
/// height asked for without, and the limit on the change), at the same
/// fraction of the way from one to the other, so that they fill the
/// stretch exactly. Both bounds keep the limit, so the layers in between do
/// too: no single thin layer below a bound. The first layer keeps
/// `first_layer_height` and is left out of the limit. What is built only
/// from additions, subtractions, multiplications and divisions is the same
/// on every platform.
// ponytail: the allowed height of a layer is looked up where the layer
// would lie if every layer below in its stretch were as thick as allowed;
// layers that end up thinner lie slightly lower, so on a slope that gets
// shallower a layer may exceed the allowed step height a little. A flat
// surface closer than `min_layer_height` to a bound or another flat surface
// is not ended on. A stretch that cannot be filled within the limit (a
// flat surface just more than `min_layer_height` above another with thin
// layers around, or a bound just above another) first gets one layer less,
// up to `max_layer_height` (with adaptive layers outside ranges); a flat
// surface that still cannot be ended on is crossed. Below a bound or the
// top come layers thinner than the lower bound, and as a last resort equal
// layers, which may break the limit.
pub(crate) fn smooth(
    mesh: &Mesh,
    height: f64,
    settings: &Settings,
    ranges: &[(f64, f64, f64)],
) -> (Vec<f64>, Vec<f64>) {
    let first = settings.first_layer_height;
    if first > height + EPSILON {
        return (Vec::new(), Vec::new());
    }
    let adaptive = settings.adaptive_layers;
    let (mut field, flats, mut thinnest) = if adaptive {
        let (allowed, flats) = surface(mesh, height, settings);
        (allowed, flats, settings.min_layer_height)
    } else {
        let bands = (height / BAND) as usize + 1;
        (
            vec![settings.layer_height; bands],
            Vec::new(),
            settings.layer_height,
        )
    };
    // Inside a range its layer height counts, also where it is thicker than
    // the surface allows; a band reaching across a bound takes the thinner.
    for (band, value) in field.iter_mut().enumerate() {
        let low = band as f64 * BAND;
        let base = *value;
        *value = range_at(ranges, low)
            .unwrap_or(base)
            .min(range_at(ranges, low + BAND - EPSILON).unwrap_or(base));
    }
    for range in ranges {
        thinnest = thinnest.min(range.2);
    }
    let ratio = (settings.layer_height_max_change > 0.0)
        .then(|| 1.0 + settings.layer_height_max_change / 100.0);

    // Where layers must end: the first layer, the bounds of the ranges and
    // the top, and the flat surfaces not too close to any of them.
    let mut anchors = range_bounds(ranges, first, height);
    anchors.push(first);
    anchors.push(height.max(first));
    anchors.sort_by(f64::total_cmp);
    anchors.dedup_by(|later, earlier| *later - *earlier < EPSILON);
    let must = anchors.clone();
    for &flat in &flats {
        if flat > first
            && anchors
                .iter()
                .all(|&a| (a - flat).abs() > thinnest - EPSILON)
        {
            anchors.push(flat);
        }
    }
    if anchors.len() > must.len() {
        anchors.sort_by(f64::total_cmp);
    }

    if let Some(ratio) = ratio {
        // No layer is thicker than the stretch it lies in, a stretch in a
        // range has equal layers, and the allowed height grows by at most
        // the limit per millimetre.
        for pair in anchors.windows(2) {
            let mut length = pair[1] - pair[0];
            if let Some(wanted) = range_at(ranges, (pair[0] + pair[1]) / 2.0) {
                length /= (length / wanted - EPSILON).ceil().max(1.0);
            }
            let low = (pair[0] / BAND) as usize;
            let high = ((pair[1] / BAND) as usize).min(field.len() - 1);
            for value in field.get_mut(low..=high).unwrap_or_default() {
                *value = value.min(length);
            }
        }
        let step = (ratio - 1.0) * BAND;
        for band in 1..field.len() {
            field[band] = field[band].min(field[band - 1] + step);
        }
        for band in (1..field.len()).rev() {
            field[band - 1] = field[band - 1].min(field[band] + step);
        }
    }
    let tallest = field.iter().fold(0.0, |a: f64, &b| a.max(b));
    // Every layer is at least as thick as the thinnest band allows, so the
    // layers below always reach the top of their stretch.
    let least = field
        .iter()
        .fold(tallest, |a: f64, &b| a.min(b))
        .max(EPSILON);

    let (mut tops, mut heights) = (vec![first], vec![first]);
    // The layer below, for the limit; the first layer does not count.
    let mut below: Option<f64> = None;
    let mut start = first;
    for &end in &anchors[1..] {
        let length = end - start;
        // As thick as allowed from the bottom of the stretch up, until it
        // is filled.
        let mut upper = Vec::new();
        let (mut z, mut previous) = (start, below);
        while z < end - EPSILON {
            let mut h = grow(&field, z, tallest, end);
            if let (Some(ratio), Some(previous)) = (ratio, previous) {
                h = h.min(previous * ratio);
            }
            let h = h.max(least);
            upper.push(h);
            previous = Some(h);
            z += h;
        }
        let in_range = range_at(ranges, (start + end) / 2.0).is_some();
        let relaxed = (adaptive && !in_range).then_some(settings.max_layer_height);
        // A flat surface the layers cannot end on within the limits is
        // left out, and the layers go on to the next place they must end.
        let flat = !must.contains(&end);
        let Some(layers) = place(&upper, length, below, ratio, thinnest, relaxed, !flat) else {
            continue;
        };
        let mut bottom = start;
        for (k, h) in layers.iter().enumerate() {
            let top = if k + 1 == layers.len() {
                end
            } else {
                bottom + h
            };
            tops.push(top);
            heights.push(top - bottom);
            bottom = top;
        }
        below = heights.last().copied();
        start = end;
    }
    (tops, heights)
}

/// Heights of layers that fill `length` exactly, where `upper` are the
/// thickest the stretch allows from its bottom up (at least one), `below`
/// is the layer under the stretch, `ratio` the largest factor between one
/// layer and the next and `floor` the thinnest layer wanted (only where
/// `relaxed` is given; otherwise down to equal layers filling the stretch).
/// Tries as many layers as `upper` and up to two more; then, where
/// `relaxed` is given, one layer less, each up to `relaxed` thick. Where
/// the layers `must` end on top of the stretch, then layers thinner than
/// `floor`, and finally equal layers; otherwise `None`.
fn place(
    upper: &[f64],
    length: f64,
    below: Option<f64>,
    ratio: Option<f64>,
    floor: f64,
    relaxed: Option<f64>,
    must: bool,
) -> Option<Vec<f64>> {
    let Some(&last) = upper.last() else {
        return Some(vec![length]);
    };
    // Inside a range, or without adaptive layers, equal layers may be
    // thinner than `floor`, as without a limit.
    let floor_of = |count: usize| {
        if relaxed.is_some() {
            floor
        } else {
            floor.min(length / count as f64)
        }
    };
    for extra in 0..3 {
        let mut more = upper.to_vec();
        more.extend(std::iter::repeat_n(last, extra));
        if let Some(layers) = fit(&more, length, below, ratio, floor_of(more.len())) {
            return Some(layers);
        }
    }
    if let (Some(thickest), true) = (relaxed, upper.len() > 1) {
        let fewer = fit(
            &vec![thickest; upper.len() - 1],
            length,
            below,
            ratio,
            floor,
        );
        if fewer.is_some() {
            return fewer;
        }
    }
    if !must {
        return None;
    }
    if let Some(layers) = fit(upper, length, below, ratio, 0.0) {
        return Some(layers);
    }
    // Equal layers, one more or less than allowed where that keeps them
    // between the thinnest and the thickest, changing least from the layer
    // below.
    let count = upper.len();
    let thickest = relaxed.unwrap_or_else(|| upper.iter().fold(0.0, |a: f64, &b| a.max(b)));
    let lowest = if relaxed.is_some() { floor } else { 0.0 };
    let change = |each: f64| below.map_or(1.0, |below| (each / below).max(below / each));
    let mut best = count;
    for other in [count - 1, count + 1] {
        let each = length / other as f64;
        if other > 0
            && each > lowest - EPSILON
            && each < thickest + EPSILON
            && change(each) < change(length / best as f64)
        {
            best = other;
        }
    }
    Some(vec![length / best as f64; best])
}

/// One layer for every entry of `upper`, no thicker than it and filling
/// `length` exactly, each at most `ratio` times as thick or as thin as its
/// neighbours and as `below`, the layer under the first, and as thick as
/// `floor` where `upper` allows; `None` where that is not possible. Lowers
/// the upper bounds and raises the lower ones to keep the ratio (forward
/// and backward), then takes every layer at the same fraction of the way
/// from its lower to its upper bound. Since both bounds keep the ratio, so
/// does every layer between them.
fn fit(
    upper: &[f64],
    length: f64,
    below: Option<f64>,
    ratio: Option<f64>,
    floor: f64,
) -> Option<Vec<f64>> {
    let count = upper.len();
    let mut high = upper.to_vec();
    if let Some(ratio) = ratio {
        if let Some(below) = below {
            high[0] = high[0].min(below * ratio);
        }
        for i in 1..count {
            high[i] = high[i].min(high[i - 1] * ratio);
        }
        for i in (1..count).rev() {
            high[i - 1] = high[i - 1].min(high[i] * ratio);
        }
    }
    // Taken from the upper bounds once they keep the ratio, the lower
    // bounds stay below them while they are raised to keep it too.
    let mut low: Vec<f64> = high.iter().map(|&h| floor.min(h)).collect();
    if let Some(ratio) = ratio {
        if let Some(below) = below {
            low[0] = low[0].max(below / ratio);
        }
        for i in 1..count {
            low[i] = low[i].max(low[i - 1] / ratio);
        }
        for i in (1..count).rev() {
            low[i - 1] = low[i - 1].max(low[i] / ratio);
        }
    }
    let least: f64 = low.iter().sum();
    let most: f64 = high.iter().sum();
    if low.iter().zip(&high).any(|(l, h)| l > h)
        || least > length + EPSILON
        || most < length - EPSILON
    {
        return None;
    }
    let share = if most - least > EPSILON {
        ((length - least) / (most - least)).clamp(0.0, 1.0)
    } else {
        1.0
    };
    Some(
        low.iter()
            .zip(&high)
            .map(|(l, h)| l + share * (h - l))
            .collect(),
    )
}

/// The layer height of the last of `ranges` holding `z`.
fn range_at(ranges: &[(f64, f64, f64)], z: f64) -> Option<f64> {
    ranges
        .iter()
        .rev()
        .find(|(from, to, _)| *from <= z && z < *to)
        .map(|range| range.2)
}

/// The bounds of `ranges` above `first` and below `height`, sorted.
fn range_bounds(ranges: &[(f64, f64, f64)], first: f64, height: f64) -> Vec<f64> {
    let mut bounds: Vec<f64> = ranges
        .iter()
        .flat_map(|&(from, to, _)| [from, to])
        .filter(|&bound| bound > first + EPSILON && bound < height - EPSILON)
        .collect();
    bounds.sort_by(f64::total_cmp);
    bounds
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_set_the_layer_height_between_their_bounds() {
        let settings = Settings::default();
        // Ranges above the model change nothing.
        let (tops, heights) = ranged(3.0, &settings, &[(5.0, 6.0, 0.1)]);
        let plain: Vec<f64> = (0..15).map(|i| 0.2 + i as f64 * 0.2).collect();
        assert_eq!(tops, plain);
        assert!(heights.iter().all(|&h| h == 0.2));
        // A range that is not a whole number of its layers gets slightly
        // thinner ones; a later range counts over an earlier one.
        let ranges = [(1.0, 2.05, 0.1), (1.5, 1.7, 0.05)];
        let (tops, heights) = ranged(3.0, &settings, &ranges);
        assert_eq!(tops.len(), heights.len());
        let mut bottom = 0.0;
        for (&top, &height) in tops.iter().zip(&heights) {
            assert!((top - bottom - height).abs() < 1e-9);
            let middle = top - height / 2.0;
            let wanted = match middle {
                _ if bottom == 0.0 => 0.2,
                m if (1.5..1.7).contains(&m) => 0.05,
                m if (1.0..2.05).contains(&m) => 0.1,
                _ => 0.2,
            };
            assert!(
                height <= wanted + 1e-9 && height > wanted * 0.8,
                "{top} {height}"
            );
            bottom = top;
        }
        for bound in [1.0, 1.5, 1.7, 2.05] {
            assert!(tops.iter().any(|&t| (t - bound).abs() < 1e-9), "{bound}");
        }
        // Above the last bound, whole layers up to the top.
        assert!((tops[tops.len() - 1] - 2.85).abs() < 1e-9);
        assert!(ranged(0.1, &settings, &ranges).0.is_empty());
    }

    /// Checks that the layers stack up to `height`, end on every bound in
    /// `bounds` and change by no more than `ratio` after the first; returns
    /// the largest change.
    fn check(tops: &[f64], heights: &[f64], height: f64, bounds: &[f64], ratio: f64) -> f64 {
        assert_eq!(tops.len(), heights.len());
        let mut bottom = 0.0;
        for (&top, &h) in tops.iter().zip(heights) {
            assert!((top - bottom - h).abs() < 1e-9, "{top} {h}");
            bottom = top;
        }
        assert!((heights.iter().sum::<f64>() - height).abs() < 1e-9);
        for bound in bounds {
            assert!(tops.iter().any(|&t| (t - bound).abs() < 1e-9), "{bound}");
        }
        let mut most: f64 = 1.0;
        for pair in heights[1..].windows(2) {
            let change = (pair[1] / pair[0]).max(pair[0] / pair[1]);
            assert!(change <= ratio + 1e-9, "{pair:?}");
            most = most.max(change);
        }
        most
    }

    #[test]
    fn plain_ranges_change_the_layer_height_gradually() {
        let mut settings = Settings {
            layer_height: 0.3,
            ..Settings::default()
        };
        let ranges = [(5.0, 5.25, 0.1)];
        // Without a limit: thick layers right up to the range.
        let (_, heights) = ranged(10.0, &settings, &ranges);
        assert!(heights.windows(2).any(|p| p[0] / p[1] > 2.9));
        settings.layer_height_max_change = 20.0;
        let mesh = Mesh::cuboid(10.0, 10.0, 10.0);
        let (tops, heights) = smooth(&mesh, 10.0, &settings, &ranges);
        let most = check(&tops, &heights, 10.0, &[5.0, 5.25], 1.2);
        assert!(most > 1.15, "{most}");
        let mut bottom = 0.0;
        for (&top, &h) in tops.iter().zip(&heights) {
            // Never thicker than asked for; the range keeps its height.
            let wanted = if bottom >= 5.0 - 1e-9 && top <= 5.25 + 1e-9 {
                0.1
            } else {
                0.3
            };
            assert!(h <= wanted + 1e-9, "{top} {h}");
            if wanted == 0.1 {
                assert!(h > 0.08, "{top} {h}");
            }
            // Far from the range the layers are about as asked for; every
            // stretch is filled exactly.
            if bottom > 0.5 && (top < 3.0 || bottom > 7.5) {
                assert!(h > 0.27, "{top} {h}");
            }
            bottom = top;
        }
        // A bound just above another: no single thin layer between thick
        // ones.
        let ranges = [(3.0, 3.05, 0.3)];
        let settings = Settings {
            layer_height_max_change: 25.0,
            ..settings
        };
        let (tops, heights) = smooth(&mesh, 10.0, &settings, &ranges);
        check(&tops, &heights, 10.0, &[3.0, 3.05], 1.25);
        assert!(heights.iter().all(|&h| h > 0.04));
    }

    #[test]
    fn adaptive_layers_change_gradually_and_keep_their_limits() {
        let mesh = slicer_core::primitives::sphere(15.0, 96);
        let settings = Settings {
            adaptive_layers: true,
            layer_height_max_change: 10.0,
            ..Settings::default()
        };
        let (tops, heights) = smooth(&mesh, 30.0, &settings, &[]);
        check(&tops, &heights, 30.0, &[], 1.1);
        for &h in &heights[1..] {
            assert!(h > 0.1 - 1e-9 && h < 0.3 + 1e-9, "{h}");
        }
        // Without a limit the change is abrupt in places.
        let (_, abrupt) = plan(
            &mesh,
            30.0,
            &Settings {
                layer_height_max_change: 0.0,
                ..settings.clone()
            },
        );
        let jump = abrupt[1..]
            .windows(2)
            .map(|p| (p[1] / p[0]).max(p[0] / p[1]))
            .fold(1.0, f64::max);
        assert!(jump > 1.1, "{jump}");
        // A lower quality makes fewer, thicker layers.
        let coarse = Settings {
            adaptive_layer_quality: 50.0,
            ..settings.clone()
        };
        let (fewer, _) = smooth(&mesh, 30.0, &coarse, &[]);
        assert!(fewer.len() < tops.len(), "{} {}", fewer.len(), tops.len());
    }

    #[test]
    fn ranges_keep_their_height_among_adaptive_layers() {
        // Vertical walls: adaptive layers are as thick as allowed.
        let mesh = Mesh::cuboid(10.0, 10.0, 10.0);
        let mut settings = Settings {
            adaptive_layers: true,
            ..Settings::default()
        };
        let ranges = [(4.0, 6.05, 0.1)];
        for change in [0.0, 20.0] {
            settings.layer_height_max_change = change;
            let (tops, heights) = smooth(&mesh, 10.0, &settings, &ranges);
            let ratio = if change > 0.0 { 1.2 } else { 3.0 + 1e-6 };
            check(&tops, &heights, 10.0, &[4.0, 6.05], ratio);
            let mut bottom = 0.0;
            for (&top, &h) in tops.iter().zip(&heights) {
                if bottom >= 4.0 - 1e-9 && top <= 6.05 + 1e-9 {
                    // 21 equal layers of 0.0976 mm.
                    assert!((h - 2.05 / 21.0).abs() < 1e-9, "{top} {h}");
                } else if bottom > 0.5 && (top < 2.0 || bottom > 8.0) {
                    // The stretch is filled exactly: slightly thinner.
                    assert!(h > 0.27 && h < 0.3 + 1e-9, "{top} {h}");
                } else {
                    // Towards the range down to its height.
                    assert!(h > 2.05 / 21.0 - 1e-9 && h < 0.3 + 1e-9, "{top} {h}");
                }
                bottom = top;
            }
        }
    }

    #[test]
    fn lower_quality_allows_higher_steps() {
        let mesh = slicer_core::primitives::sphere(15.0, 48);
        let settings = Settings {
            adaptive_layers: true,
            ..Settings::default()
        };
        let before = plan(&mesh, 30.0, &settings);
        let coarse = Settings {
            adaptive_layer_quality: 0.0,
            ..settings
        };
        let after = plan(&mesh, 30.0, &coarse);
        assert!(after.0.len() < before.0.len());
        // At quality 0 only the top makes thin layers.
        let count = after.1.len();
        assert!(after.1[1..count - 2].iter().all(|&h| h > 0.29));
    }
}
