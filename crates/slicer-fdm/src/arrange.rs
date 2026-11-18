//! Arranging objects by their outlines seen from above instead of their
//! bounding rectangles, optionally turning them about Z
//! (`arrange_outlines`, `arrange_rotation`).
//!
//! The outline of an object is its convex hull seen from above, the same
//! outline that the overlap warning and `sequential_print` check, so
//! arranged objects never trip either. It is approximated from outside by a
//! polygon whose edges face [`DIRECTIONS`] fixed directions, stored as its
//! support values: how far the hull reaches along each direction. With
//! shared directions everything needed is sums of these values: the
//! positions where one object would come closer than the gap to another
//! (their no-fit polygon, grown by the gap; Bennell and Oliveira, Eur. J.
//! Oper. Res. 184(2), 2008) is again such a polygon, with the support
//! values of the one plus those of the other turned half round plus the
//! gap, and a position keeps clear exactly when one of the directions
//! separates it from that polygon. The geometry needs only `+ - * /`, and
//! the directions and turns come from
//! [`sin_cos_degrees`](slicer_core::polygon::sin_cos_degrees), so the
//! result is the same on every platform.
//!
//! Placement is bottom-left fill with no-fit polygons (Burke, Hellier,
//! Kendall and Whitwell, Oper. Res. 54(3), 2006), greedy and largest
//! outline first: every object is tried in every allowed turn at the
//! corners of the no-fit polygons of the objects already placed, where two
//! of them cross, where they meet the edges of the bed and in the corners
//! of the bed. Of the positions that keep clear it takes the one where the
//! object reaches least far back, then least far right, then the earlier
//! turn. With `arrange_rotation` this runs three times, unturned, in
//! half and quarter turns and in eighth turns. The rows of bounding
//! rectangles of [`crate::arrange`] compete as well: the packing that
//! places the most objects in the smallest rectangle (its longer side
//! relative to the bed, then its area) wins, and the rows win a tie. So
//! outlines never pack worse than rectangles, turning never worse than not
//! turning, and boxes stay where the rows put them. The whole arrangement
//! is centred on the bed at the end.

use slicer_core::Mesh;
use slicer_core::polygon::{self, Contour, Point};
use slicer_profiles::Settings;

/// Number of directions the outlines are approximated in: every 11.25°.
/// A multiple of 8, so that the axes and the diagonals are among them; an
/// outline grows by at most 0.5 % of its radius on a slanted side.
const DIRECTIONS: usize = 32;
/// Index of the direction +Y; +X is index 0.
const QUARTER: usize = DIRECTIONS / 4;
/// Index of the direction −X.
const HALF: usize = DIRECTIONS / 2;
/// Index of the direction −Y.
const THREE_QUARTERS: usize = 3 * DIRECTIONS / 4;

/// Turns about Z tried with `arrange_rotation`, in degrees, in the order
/// they are preferred when they pack equally well. The runs try the first
/// one, four and all of them.
const TURNS: [f64; 8] = [0.0, 180.0, 90.0, 270.0, 45.0, 225.0, 135.0, 315.0];

/// Distance in millimetres by which a position may reach into the gap
/// around another object or over the edge of the bed, to absorb rounding.
const TOLERANCE: f64 = 1e-9;

/// Difference in millimetres below which two positions count as reaching
/// equally far.
const TIE: f64 = 1e-6;

/// How far an outline reaches along every direction.
type Support = [f64; DIRECTIONS];

/// Where [`arrange_outlines`] puts an object: turned about the origin by
/// `turn` degrees counterclockwise seen from above, then moved by `offset`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spot {
    /// Turn about Z in degrees, counterclockwise seen from above; one of 0,
    /// 45, …, 315.
    pub turn: f64,
    /// Sine of `turn`, from
    /// [`sin_cos_degrees`](slicer_core::polygon::sin_cos_degrees).
    pub sin: f64,
    /// Cosine of `turn`.
    pub cos: f64,
    /// Shift in X and Y after turning, in millimetres.
    pub offset: Point,
}

impl Spot {
    /// An unturned spot.
    fn shifted(offset: Point) -> Self {
        Self {
            turn: 0.0,
            sin: 0.0,
            cos: 1.0,
            offset,
        }
    }

    /// Where the point `p` of the object ends up.
    pub fn place(&self, p: Point) -> Point {
        let [x, y] = turned(p, self.sin, self.cos);
        [x + self.offset[0], y + self.offset[1]]
    }

    /// Turns and moves `mesh` to the spot.
    pub fn apply(&self, mesh: &mut Mesh) {
        if self.turn == 0.0 {
            mesh.translate([self.offset[0], self.offset[1], 0.0]);
            return;
        }
        for vertex in &mut mesh.vertices {
            [vertex[0], vertex[1]] = self.place([vertex[0], vertex[1]]);
        }
    }
}

/// Whether arranging packs outlines ([`arrange_outlines`]) rather than
/// bounding rectangles: with `arrange_outlines`, and with
/// `arrange_rotation`, which needs them.
pub fn arranges_outlines(settings: &Settings) -> bool {
    settings.arrange_outlines || settings.arrange_rotation
}

/// Places objects with the given outlines seen from above (any points; their
/// convex hull counts) side by side on the bed, turned about Z when
/// `arrange_rotation` allows it and it packs them closer, and returns where
/// each one goes. The outlines keep `gap` millimetres apart, at least
/// `extruder_clearance_radius` with `sequential_print`, plus room for the
/// brim of both. See the module documentation for how.
///
/// Objects that do not fit end up unturned beyond the back edge, where
/// [`crate::generate_plate`] reports them, as with [`crate::arrange`]. The
/// result depends only on the outlines, their order and the settings.
// ponytail: convex outlines, so an L-shape cannot reach into the corner of
// another, and one greedy order. Nesting the real outlines also needs the
// overlap and clearance checks of the plate to use them; trying several
// orders is the step after.
pub fn arrange_outlines(outlines: &[Contour], settings: &Settings, gap: f64) -> Vec<Spot> {
    let bed = [settings.bed_size_x, settings.bed_size_y];
    let gap_asked = gap;
    let gap = crate::plate::spacing(settings, gap);
    let directions = directions();
    let hulls: Vec<Contour> = outlines
        .iter()
        .map(|outline| {
            let hull = polygon::convex_hull(outline);
            if hull.is_empty() {
                vec![[0.0, 0.0]]
            } else {
                hull
            }
        })
        .collect();
    // Largest first; the sort is stable, so equal objects keep their order.
    let areas: Vec<f64> = hulls
        .iter()
        .map(|h| polygon::signed_area(h).abs())
        .collect();
    let mut order: Vec<usize> = (0..hulls.len()).collect();
    order.sort_by(|&a, &b| areas[b].total_cmp(&areas[a]));

    let runs: &[usize] = if settings.arrange_rotation {
        &[1, 4, 8]
    } else {
        &[1]
    };
    let mut best = rows(&hulls, settings, gap_asked, &directions);
    for &turns in runs {
        let packing = pack(&hulls, &order, &TURNS[..turns], bed, gap, &directions);
        if packing.beats(&best, bed) {
            best = packing;
        }
    }
    let Packing {
        spots,
        union,
        centred,
        ..
    } = best;

    // Centre the arrangement on the bed; objects that did not fit go
    // behind it.
    let shift = union.filter(|_| !centred).map_or([0.0, 0.0], |(lo, hi)| {
        [
            bed[0] / 2.0 - (lo[0] + hi[0]) / 2.0,
            bed[1] / 2.0 - (lo[1] + hi[1]) / 2.0,
        ]
    });
    spots
        .into_iter()
        .zip(&hulls)
        .map(|(spot, hull)| match spot {
            Some(spot) => Spot {
                offset: [spot.offset[0] + shift[0], spot.offset[1] + shift[1]],
                ..spot
            },
            None => {
                let (lo, hi) = extent(&support(hull, &directions));
                Spot::shifted([bed[0] / 2.0 - (lo[0] + hi[0]) / 2.0, bed[1] + gap - lo[1]])
            }
        })
        .collect()
}

/// The outcome of one run of the greedy placement.
struct Packing {
    /// Where every object goes, if it fits.
    spots: Vec<Option<Spot>>,
    /// How many objects fit.
    placed: usize,
    /// The bounding box of the objects that fit.
    union: Option<(Point, Point)>,
    /// Whether the spots are centred on the bed already.
    centred: bool,
}

impl Packing {
    /// Whether this packing places more objects than `other`, or as many
    /// in a clearly smaller rectangle (relative to the bed, then by area).
    fn beats(&self, other: &Packing, bed: [f64; 2]) -> bool {
        if self.placed != other.placed {
            return self.placed > other.placed;
        }
        let measure = |union: Option<(Point, Point)>| {
            union.map_or([0.0, 0.0], |(lo, hi)| {
                let size = [hi[0] - lo[0], hi[1] - lo[1]];
                [
                    (size[0] / bed[0]).max(size[1] / bed[1]),
                    size[0] * size[1] / (bed[0] * bed[1]),
                ]
            })
        };
        let (mine, theirs) = (measure(self.union), measure(other.union));
        for (a, b) in mine.into_iter().zip(theirs) {
            if a < b - TIE {
                return true;
            }
            if a > b + TIE {
                return false;
            }
        }
        false
    }
}

/// The rows of bounding rectangles of [`crate::arrange`] as a packing,
/// unturned and exactly where it puts them; the outlines only replace it
/// where they pack better.
fn rows(
    hulls: &[Contour],
    settings: &Settings,
    gap: f64,
    directions: &[Point; DIRECTIONS],
) -> Packing {
    let bed = [settings.bed_size_x, settings.bed_size_y];
    let boxes: Vec<(Point, Point)> = hulls
        .iter()
        .map(|hull| extent(&support(hull, directions)))
        .collect();
    let sizes: Vec<[f64; 2]> = boxes
        .iter()
        .map(|(lo, hi)| [hi[0] - lo[0], hi[1] - lo[1]])
        .collect();
    let corners = crate::plate::arrange(&sizes, settings, gap);
    let mut placed = Vec::new();
    let spots = corners
        .iter()
        .zip(&boxes)
        .zip(&sizes)
        .map(|((corner, (lo, _)), size)| {
            let on_bed = (0..2)
                .all(|a| corner[a] >= -TOLERANCE && corner[a] + size[a] <= bed[a] + TOLERANCE);
            if on_bed {
                placed.push((*corner, [corner[0] + size[0], corner[1] + size[1]]));
            }
            // Those that do not fit stay beyond the back edge.
            Some(Spot::shifted([corner[0] - lo[0], corner[1] - lo[1]]))
        })
        .collect();
    Packing {
        spots,
        placed: placed.len(),
        union: enclosing(placed),
        centred: true,
    }
}

/// Places the objects in `order`, each in the best of `turns`, with their
/// lower left corner at the origin of the bed.
fn pack(
    hulls: &[Contour],
    order: &[usize],
    turns: &[f64],
    bed: [f64; 2],
    gap: f64,
    directions: &[Point; DIRECTIONS],
) -> Packing {
    let mut placed: Vec<Support> = Vec::new();
    let mut spots: Vec<Option<Spot>> = vec![None; hulls.len()];
    for &index in order {
        let mut best: Option<(Score, Spot, Support)> = None;
        for &turn in turns {
            let (sin, cos) = polygon::sin_cos_degrees(turn);
            let points: Vec<Point> = hulls[index].iter().map(|&p| turned(p, sin, cos)).collect();
            let local = support(&points, directions);
            let bar = best.as_ref().map(|(score, ..)| *score);
            if let Some((score, at)) = position(&local, &placed, bed, gap, directions, bar) {
                let spot = Spot {
                    turn,
                    sin,
                    cos,
                    offset: at,
                };
                best = Some((score, spot, moved(&local, at, directions)));
            }
        }
        if let Some((_, spot, global)) = best {
            placed.push(global);
            spots[index] = Some(spot);
        }
    }
    Packing {
        spots,
        placed: placed.len(),
        union: union(&placed),
        centred: false,
    }
}

/// The unit vectors of the directions, counterclockwise from +X.
fn directions() -> [Point; DIRECTIONS] {
    std::array::from_fn(|k| {
        let (sin, cos) = polygon::sin_cos_degrees(360.0 * k as f64 / DIRECTIONS as f64);
        [cos, sin]
    })
}

/// `p` turned about the origin by the angle with this sine and cosine.
fn turned(p: Point, sin: f64, cos: f64) -> Point {
    [p[0] * cos - p[1] * sin, p[0] * sin + p[1] * cos]
}

fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn cross(a: Point, b: Point) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

/// How far `points` reach along every direction.
fn support(points: &[Point], directions: &[Point; DIRECTIONS]) -> Support {
    std::array::from_fn(|k| {
        points
            .iter()
            .map(|&p| dot(p, directions[k]))
            .fold(f64::NEG_INFINITY, f64::max)
    })
}

/// The support values after moving by `at`.
fn moved(local: &Support, at: Point, directions: &[Point; DIRECTIONS]) -> Support {
    std::array::from_fn(|k| local[k] + dot(at, directions[k]))
}

/// The bounding box of the polygon with these support values: exact, since
/// the axes are among the directions.
fn extent(support: &Support) -> (Point, Point) {
    (
        [-support[HALF], -support[THREE_QUARTERS]],
        [support[0], support[QUARTER]],
    )
}

/// The bounding box of all placed outlines.
fn union(placed: &[Support]) -> Option<(Point, Point)> {
    enclosing(placed.iter().map(extent))
}

/// The bounding box of bounding boxes.
fn enclosing(boxes: impl IntoIterator<Item = (Point, Point)>) -> Option<(Point, Point)> {
    boxes.into_iter().reduce(|(a, b), (c, d)| {
        (
            [a[0].min(c[0]), a[1].min(c[1])],
            [b[0].max(d[0]), b[1].max(d[1])],
        )
    })
}

/// The corners of the polygon with these support values (the edge facing
/// direction `k` runs from corner `k - 1` to corner `k`).
fn corners(support: &Support, directions: &[Point; DIRECTIONS]) -> [Point; DIRECTIONS] {
    std::array::from_fn(|k| {
        let (a, b) = (directions[k], directions[(k + 1) % DIRECTIONS]);
        let (h, g) = (support[k], support[(k + 1) % DIRECTIONS]);
        let det = cross(a, b);
        [(h * b[1] - g * a[1]) / det, (a[0] * g - b[0] * h) / det]
    })
}

/// How good a position is: how far back the object reaches, then how far
/// right; smaller is better, compared in order.
type Score = [f64; 2];

fn better(a: &Score, b: &Score) -> bool {
    for (x, y) in a.iter().zip(b) {
        if *x < y - TIE {
            return true;
        }
        if *x > y + TIE {
            return false;
        }
    }
    false
}

/// A no-fit polygon: the positions of the new object that come closer than
/// the gap to one placed object, by its support values, bounding box and
/// edges longer than the tolerance.
struct NoFit {
    support: Support,
    lo: Point,
    hi: Point,
    edges: Vec<[Point; 2]>,
}

impl NoFit {
    /// Whether the position `p` keeps clear of it.
    fn clear(&self, p: Point, directions: &[Point; DIRECTIONS]) -> bool {
        // The axes first: most placed objects are far away to one side.
        [0, QUARTER, HALF, THREE_QUARTERS]
            .into_iter()
            .chain(0..DIRECTIONS)
            .any(|k| dot(p, directions[k]) >= self.support[k] - TOLERANCE)
    }
}

/// The best position on the bed for the reference point of an object with
/// the support values `local` (unmoved), if one keeps clear of all placed
/// objects and scores better than `bar`.
fn position(
    local: &Support,
    placed: &[Support],
    bed: [f64; 2],
    gap: f64,
    directions: &[Point; DIRECTIONS],
    bar: Option<Score>,
) -> Option<(Score, Point)> {
    let (lo, hi) = extent(local);
    // Where the reference point may go with the object on the bed.
    let fit_lo = [-lo[0], -lo[1]];
    let fit_hi = [bed[0] - hi[0], bed[1] - hi[1]];
    if fit_lo[0] > fit_hi[0] + TOLERANCE || fit_lo[1] > fit_hi[1] + TOLERANCE {
        return None;
    }
    let no_fits: Vec<NoFit> = placed
        .iter()
        .map(|other| {
            let support: Support =
                std::array::from_fn(|k| other[k] + local[(k + HALF) % DIRECTIONS] + gap);
            let corners = corners(&support, directions);
            let edges = (0..DIRECTIONS)
                .map(|k| [corners[(k + DIRECTIONS - 1) % DIRECTIONS], corners[k]])
                .filter(|[a, b]| polygon::distance(*a, *b) > TOLERANCE)
                .collect();
            let (lo, hi) = extent(&support);
            NoFit {
                support,
                lo,
                hi,
                edges,
            }
        })
        .collect();

    let mut candidates: Vec<Point> = vec![
        fit_lo,
        [fit_hi[0], fit_lo[1]],
        [fit_lo[0], fit_hi[1]],
        fit_hi,
    ];
    for (i, no_fit) in no_fits.iter().enumerate() {
        for &[a, b] in &no_fit.edges {
            candidates.push(b);
            // Where the edge meets the edges of the bed.
            for axis in 0..2 {
                for limit in [fit_lo[axis], fit_hi[axis]] {
                    let (from, to) = (a[axis] - limit, b[axis] - limit);
                    if (from < 0.0) != (to < 0.0) {
                        let t = from / (from - to);
                        candidates.push([a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]);
                    }
                }
            }
        }
        // Where it crosses the no-fit polygons of the others.
        for other in &no_fits[i + 1..] {
            if other.lo[0] > no_fit.hi[0]
                || other.hi[0] < no_fit.lo[0]
                || other.lo[1] > no_fit.hi[1]
                || other.hi[1] < no_fit.lo[1]
            {
                continue;
            }
            for &e in &no_fit.edges {
                for &f in &other.edges {
                    if let Some(p) = crossing(e, f) {
                        candidates.push(p);
                    }
                }
            }
        }
    }

    let mut best: Option<(Score, Point)> = None;
    for at in candidates {
        // How far back and how far right the object reaches.
        let score = [at[1] + hi[1], at[0] + hi[0]];
        let bar = best.as_ref().map(|(score, _)| score).or(bar.as_ref());
        if bar.is_some_and(|bar| !better(&score, bar)) {
            continue;
        }
        let inside =
            (0..2).all(|a| at[a] >= fit_lo[a] - TOLERANCE && at[a] <= fit_hi[a] + TOLERANCE);
        if inside && no_fits.iter().all(|n| n.clear(at, directions)) {
            best = Some((score, at));
        }
    }
    best
}

/// Where the segments `e` and `f` cross, if they do.
fn crossing(e: [Point; 2], f: [Point; 2]) -> Option<Point> {
    let r = [e[1][0] - e[0][0], e[1][1] - e[0][1]];
    let s = [f[1][0] - f[0][0], f[1][1] - f[0][1]];
    if e[0][0].min(e[1][0]) > f[0][0].max(f[1][0])
        || f[0][0].min(f[1][0]) > e[0][0].max(e[1][0])
        || e[0][1].min(e[1][1]) > f[0][1].max(f[1][1])
        || f[0][1].min(f[1][1]) > e[0][1].max(e[1][1])
    {
        return None;
    }
    let det = cross(r, s);
    if det.abs() < TOLERANCE * TOLERANCE {
        return None;
    }
    let w = [f[0][0] - e[0][0], f[0][1] - e[0][1]];
    let t = cross(w, s) / det;
    let u = cross(w, r) / det;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u))
        .then(|| [e[0][0] + t * r[0], e[0][1] + t * r[1]])
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::plate::{self, Placement};
    use crate::{ARRANGE_GAP, arrange, arrange_objects};
    use slicer_core::{Diagnostics, primitives};

    /// A right triangle with legs `leg` long, standing 5 mm tall.
    fn triangle(leg: f64) -> Mesh {
        primitives::extrude(
            &[[0.0, 0.0], [leg, 0.0], [0.0, leg]],
            [leg / 3.0, leg / 3.0],
            5.0,
            1,
            0.0,
        )
    }

    /// An L of `size` with arms `arm` wide.
    fn ell(size: f64, arm: f64) -> Mesh {
        primitives::extrude(
            &[
                [0.0, 0.0],
                [size, 0.0],
                [size, arm],
                [arm, arm],
                [arm, size],
                [0.0, size],
            ],
            [arm / 2.0, arm / 2.0],
            5.0,
            1,
            0.0,
        )
    }

    /// Distance between two convex polygons that do not overlap.
    fn apart(a: &[Point], b: &[Point]) -> f64 {
        let to_segment = |p: Point, s: Point, t: Point| {
            let d = [t[0] - s[0], t[1] - s[1]];
            let w = [p[0] - s[0], p[1] - s[1]];
            let u = (dot(w, d) / dot(d, d).max(1e-30)).clamp(0.0, 1.0);
            polygon::distance(p, [s[0] + u * d[0], s[1] + u * d[1]])
        };
        let one_way = |a: &[Point], b: &[Point]| {
            a.iter()
                .flat_map(|&p| (0..b.len()).map(move |i| (p, i)))
                .map(|(p, i)| to_segment(p, b[i], b[(i + 1) % b.len()]))
                .fold(f64::INFINITY, f64::min)
        };
        one_way(a, b).min(one_way(b, a))
    }

    /// How many of `objects` stand off the bed after arranging them.
    fn off_bed(objects: &[Mesh], settings: &Settings) -> usize {
        let mut arranged = objects.to_vec();
        arrange_objects(&mut arranged, settings, ARRANGE_GAP);
        arranged
            .iter()
            .filter(|mesh| {
                let (lo, hi) = mesh.bounds().unwrap();
                lo[0] < -1e-6
                    || lo[1] < -1e-6
                    || hi[0] > settings.bed_size_x + 1e-6
                    || hi[1] > settings.bed_size_y + 1e-6
            })
            .count()
    }

    /// Checks that the arranged objects stay on the bed, keep `gap` apart
    /// and raise no overlap warning.
    fn assert_arranged(objects: &[Mesh], settings: &Settings, gap: f64) {
        let outlines: Vec<Contour> = objects.iter().map(plate::footprint).collect();
        for (i, a) in outlines.iter().enumerate() {
            for b in &outlines[i + 1..] {
                let distance = apart(a, b);
                assert!(distance >= gap - 1e-6, "{distance} < {gap}");
            }
        }
        let mut diagnostics = Diagnostics::new();
        plate::check(objects, Placement::AsGiven, settings, &mut diagnostics).unwrap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn turned_triangles_pair_up_where_rectangles_do_not_fit() {
        let objects = vec![triangle(60.0); 20];
        let bed = Settings {
            bed_size_x: 256.0,
            bed_size_y: 256.0,
            ..Settings::default()
        };
        let outlines = Settings {
            arrange_outlines: true,
            ..bed.clone()
        };
        let turning = Settings {
            arrange_rotation: true,
            ..bed.clone()
        };
        // Rows of rectangles take 16, and so do unturned outlines.
        assert_eq!(off_bed(&objects, &bed), 4);
        assert_eq!(off_bed(&objects, &outlines), 4);
        // Turned half round, two triangles make a square: all 20 fit.
        assert_eq!(off_bed(&objects, &turning), 0);
        let mut arranged = objects.clone();
        arrange_objects(&mut arranged, &turning, ARRANGE_GAP);
        assert_arranged(&arranged, &turning, ARRANGE_GAP);
        let outlines: Vec<Contour> = objects.iter().map(plate::footprint).collect();
        let spots = arrange_outlines(&outlines, &turning, ARRANGE_GAP);
        assert!(spots.iter().any(|spot| spot.turn == 180.0), "{spots:?}");
        // Two make a square as well, in the middle of the bed; one alone
        // stays unturned.
        let spots = arrange_outlines(&outlines[..2], &turning, ARRANGE_GAP);
        assert_eq!(spots[1].turn, 180.0);
        let spots = arrange_outlines(&outlines[..1], &turning, ARRANGE_GAP);
        assert_eq!(spots[0].turn, 0.0);
        assert_eq!(spots[0].offset, [98.0, 98.0]);
    }

    #[test]
    fn round_objects_stand_closer_than_their_rectangles() {
        let settings = Settings {
            bed_size_x: 256.0,
            bed_size_y: 256.0,
            ..Settings::default()
        };
        let outlines = Settings {
            arrange_outlines: true,
            ..settings.clone()
        };
        // Cylinders 30 mm across: rows of rectangles fit 7 × 7 in 256 mm,
        // outlines staggered rows of 56.
        let cylinders = vec![primitives::cylinder(15.0, 5.0, 64); 56];
        assert_eq!(off_bed(&cylinders, &settings), 7);
        assert_eq!(off_bed(&cylinders, &outlines), 0);
        let mut arranged = cylinders.clone();
        arrange_objects(&mut arranged, &outlines, ARRANGE_GAP);
        assert_arranged(&arranged, &outlines, ARRANGE_GAP);
    }

    #[test]
    fn mixed_objects_keep_the_gap_brim_and_clearance() {
        let mut objects = vec![
            ell(60.0, 20.0),
            triangle(50.0),
            primitives::cylinder(15.0, 10.0, 48),
            ell(40.0, 10.0),
            Mesh::cuboid(30.0, 10.0, 5.0),
            triangle(30.0),
        ];
        // Somewhere off the bed and turned, as loaded.
        objects[1].translate([500.0, -300.0, 0.0]);
        objects[3].rotate_z(0.3);
        let settings = Settings {
            arrange_rotation: true,
            brim_width: 3.0,
            ..Settings::default()
        };
        let mut arranged = objects.clone();
        arrange_objects(&mut arranged, &settings, ARRANGE_GAP);
        assert_arranged(&arranged, &settings, ARRANGE_GAP + 6.0);
        // Deterministic: the same input gives the same output.
        let mut again = objects.clone();
        arrange_objects(&mut again, &settings, ARRANGE_GAP);
        assert_eq!(arranged, again);

        // Printed one after the other they keep the clearance.
        let sequential = Settings {
            sequential_print: true,
            arrange_outlines: true,
            bed_size_x: 300.0,
            bed_size_y: 300.0,
            ..Settings::default()
        };
        let mut arranged = objects;
        arrange_objects(&mut arranged, &sequential, ARRANGE_GAP);
        assert_arranged(&arranged, &sequential, sequential.extruder_clearance_radius);
    }

    #[test]
    fn boxes_stay_in_rows() {
        // Outlines pack boxes no closer than rows, so they stay there.
        let objects: Vec<Mesh> = (1..8)
            .map(|i| Mesh::cuboid(10.0 * f64::from(i), 25.0, 5.0))
            .collect();
        let mut rows = objects.clone();
        arrange_objects(&mut rows, &Settings::default(), ARRANGE_GAP);
        let mut outlines = objects;
        let settings = Settings {
            arrange_rotation: true,
            ..Settings::default()
        };
        arrange_objects(&mut outlines, &settings, ARRANGE_GAP);
        assert_eq!(rows, outlines);
    }

    #[test]
    fn rectangles_by_default() {
        // Without the settings, arranging moves the objects where the rows
        // of rectangles put them, unturned.
        let objects = vec![ell(60.0, 20.0), triangle(50.0), ell(40.0, 10.0)];
        let mut arranged = objects.clone();
        arrange_objects(&mut arranged, &Settings::default(), ARRANGE_GAP);
        let sizes: Vec<[f64; 2]> = objects
            .iter()
            .map(|m| {
                let (lo, hi) = m.bounds().unwrap();
                [hi[0] - lo[0], hi[1] - lo[1]]
            })
            .collect();
        let corners = arrange(&sizes, &Settings::default(), ARRANGE_GAP);
        for (mesh, corner) in arranged.iter().zip(corners) {
            let (lo, _) = mesh.bounds().unwrap();
            assert_eq!([lo[0], lo[1]], corner);
        }
    }

    #[test]
    fn objects_that_do_not_fit_go_beyond_the_back() {
        let settings = Settings {
            arrange_rotation: true,
            ..Settings::default()
        };
        let mut objects = vec![
            Mesh::cuboid(1.0, 1.0, 1.0),
            Mesh::cuboid(200.0, 150.0, 1.0),
            Mesh::cuboid(200.0, 150.0, 1.0),
        ];
        arrange_objects(&mut objects, &settings, ARRANGE_GAP);
        // The big ones are placed first; the second does not fit.
        let error = plate::check(
            &objects,
            Placement::AsGiven,
            &settings,
            &mut Diagnostics::new(),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            crate::FdmError::OutsideBed { object: 2, .. }
        ));
        // Too large for the bed in any turn, it reaches over the edge where
        // the rows put it, and the error names it.
        let mut huge = vec![
            Mesh::cuboid(10.0, 10.0, 1.0),
            Mesh::cuboid(400.0, 10.0, 1.0),
        ];
        let mut rows = huge.clone();
        arrange_objects(&mut huge, &settings, ARRANGE_GAP);
        arrange_objects(&mut rows, &Settings::default(), ARRANGE_GAP);
        assert_eq!(huge, rows);
        let error = plate::check(
            &huge,
            Placement::AsGiven,
            &settings,
            &mut Diagnostics::new(),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            crate::FdmError::OutsideBed { object: 1, .. }
        ));
    }
}
