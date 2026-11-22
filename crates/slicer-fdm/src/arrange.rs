//! Arranging objects by their outlines seen from above instead of their
//! bounding rectangles, optionally turning them about Z
//! (`arrange_outlines`, `arrange_rotation`, `arrange_nonconvex`).
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
//! half and quarter turns and in eighth turns.
//!
//! Further runs try other orders and other measures: the largest outline,
//! the deepest and the widest bounding rectangle first, each with
//! bottom-left fill and with two measures that keep the bounding rectangle
//! of everything placed so far small (its area, then its longer side): one
//! starts in the middle of the bed and then prefers positions close to
//! it, the other starts in the front left corner, then prefers bottom-left
//! and counts in steps of 5 mm, so that tucking an object a millimetre
//! deeper into a crevice does not count. With `arrange_rotation` the
//! largest-first runs try two, four and eight turns, the others eight.
//! Greedy placement often decides how many objects fit: eight right
//! triangles with 90 mm legs on a 220 mm bed fit only when every second
//! one is put against the one before rather than beside it and every pair
//! stays a square, which the corner-based measure does.
//!
//! With `arrange_nonconvex` the objects are packed by their real outlines
//! seen from above ([`crate::top_view`]): an object is covered by a
//! few convex pieces of its outline grown by half the gap, cut into
//! horizontal slabs whose trapezoids are joined again wherever two of them
//! make a convex piece (Hertel and Mehlhorn's idea applied to a
//! trapezoidation). The no-fit polygon of two objects is then the union of
//! those of their pieces, each again a sum of support values without a
//! further gap, and a position keeps clear when it keeps clear of every
//! one of them. So an L can reach into the corner of another and a small
//! object can stand in the hollow of a U. These runs come on top of the
//! convex ones; with `sequential_print` the convex outlines stay, as the
//! print head needs its clearance around them.
//!
//! The rows of bounding rectangles of [`crate::arrange`] compete as well:
//! the packing that places the most objects in the smallest rectangle (its
//! longer side relative to the bed, then its area) wins, the rows win a
//! tie, then the runs in the order above. So outlines never pack worse than
//! rectangles, turning never worse than not turning, a further run never
//! worse than the first, and boxes stay where the rows put them unless
//! a run packs them clearly tighter. The whole arrangement is centred on
//! the bed at the end. The runs are independent and run in parallel; which
//! one wins does not depend on that.

use slicer_core::Mesh;
use slicer_core::parallel_map;
use slicer_core::polygon::{self, Contour, Point, Shapes};
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

/// Tolerance in millimetres to which a real outline is thinned out before
/// it is grown by half the gap.
const FINE: f64 = 0.05;

/// Tolerance in millimetres to which the grown outline is thinned out. The
/// outline is grown by this and [`FINE`] more, so that neither thinning
/// brings it closer to the object than half the gap.
const COARSE: f64 = 0.1;

/// Most convex pieces an object is cut into; more come from many notches,
/// which are filled until it is no more (see [`LOOSENESS`]).
const MAX_PIECES: usize = 12;

/// How much area, relative to the joined piece, joining two pieces may
/// add while an object has more than [`MAX_PIECES`] pieces; the pair that
/// adds least goes first. With more pieces left, its convex hull is the
/// only piece.
const LOOSENESS: f64 = 0.3;

/// Steps in millimetres in which [`Fill::Tight`] measures.
const GRAIN: f64 = 5.0;

/// `length` rounded up to whole [`GRAIN`]s, ignoring rounding noise.
fn coarse(length: f64) -> f64 {
    (length / GRAIN - 1e-6).ceil() * GRAIN
}

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
    pub(crate) fn shifted(offset: Point) -> Self {
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

/// Whether arranging packs outlines ([`arrange_outlines`],
/// [`arrange_footprints`]) rather than bounding rectangles: with
/// `arrange_outlines`, and with `arrange_rotation` and `arrange_nonconvex`,
/// which need them.
pub fn arranges_outlines(settings: &Settings) -> bool {
    settings.arrange_outlines || settings.arrange_rotation || settings.arrange_nonconvex
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
pub fn arrange_outlines(outlines: &[Contour], settings: &Settings, gap: f64) -> Vec<Spot> {
    let hulls: Vec<Contour> = outlines.iter().map(|outline| hull_of(outline)).collect();
    arrange_all(&hulls, None, settings, gap)
}

/// Like [`arrange_outlines`], but packs the real outlines seen from above
/// ([`crate::top_view`]; overlapping shapes of one object count
/// together) as well as their convex hulls, so that L- and U-shapes reach
/// into each other and small objects stand in hollows of large ones. The
/// real outlines keep at least `gap` apart and the brims, as with
/// [`arrange_outlines`]. With `sequential_print` only the convex hulls
/// count, since the print head needs its clearance around them.
// ponytail: an outline with many deep notches is filled until it has at
// most `MAX_PIECES` pieces, and the gap is kept up to 0.3 mm wider than
// asked for (`FINE`, `COARSE`); an exact no-fit polygon of the real
// outlines would nest gears and combs too.
pub fn arrange_footprints(footprints: &[Shapes], settings: &Settings, gap: f64) -> Vec<Spot> {
    let hulls: Vec<Contour> = footprints
        .iter()
        .map(|footprint| {
            let points: Vec<Point> = footprint.iter().flatten().flatten().copied().collect();
            hull_of(&points)
        })
        .collect();
    if settings.sequential_print {
        return arrange_all(&hulls, None, settings, gap);
    }
    let half = crate::plate::spacing(settings, gap) / 2.0;
    let pieces: Vec<Vec<Contour>> = footprints
        .iter()
        .zip(&hulls)
        .map(|(footprint, hull)| convex_pieces(footprint, hull, half))
        .collect();
    arrange_all(&hulls, Some(&pieces), settings, gap)
}

/// The convex hull of `points`, at least the origin.
fn hull_of(points: &[Point]) -> Contour {
    let hull = polygon::convex_hull(points);
    if hull.is_empty() {
        vec![[0.0, 0.0]]
    } else {
        hull
    }
}

/// Packs the objects with the convex hulls `hulls` and, for
/// `arrange_nonconvex`, the convex pieces `pieces` of their grown real
/// outlines; see the module documentation.
// ponytail: a handful of greedy runs in fixed orders; a search over orders
// and turns (local search or a genetic algorithm, as in the literature)
// would pack tighter at more cost.
fn arrange_all(
    hulls: &[Contour],
    pieces: Option<&[Vec<Contour>]>,
    settings: &Settings,
    gap_asked: f64,
) -> Vec<Spot> {
    let bed = [settings.bed_size_x, settings.bed_size_y];
    let gap = crate::plate::spacing(settings, gap_asked);
    let directions = directions();
    let orders = orders(hulls, &directions);
    let runs = runs(orders.len(), settings.arrange_rotation, pieces.is_some());
    let packings = parallel_map(runs.len(), |r| {
        let run = runs[r];
        let (order, turns) = (&orders[run.order], &TURNS[..run.turns]);
        match pieces.filter(|_| run.nonconvex) {
            Some(pieces) => pack_pieces(hulls, pieces, order, turns, bed, run.fill, &directions),
            None => pack(hulls, order, turns, bed, gap, run.fill, &directions),
        }
    });
    let mut best = rows(hulls, settings, gap_asked, &directions);
    for packing in packings {
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
        .zip(hulls)
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

/// The orders the objects are placed in, each tried: largest outline
/// first, deepest bounding rectangle first and widest first, unturned.
/// The sorts are stable, so equal objects keep their order; an order that
/// came up before is left out.
fn orders(hulls: &[Contour], directions: &[Point; DIRECTIONS]) -> Vec<Vec<usize>> {
    let areas: Vec<f64> = hulls
        .iter()
        .map(|h| polygon::signed_area(h).abs())
        .collect();
    let sizes: Vec<Point> = hulls
        .iter()
        .map(|hull| {
            let (lo, hi) = extent(&support(hull, directions));
            [hi[0] - lo[0], hi[1] - lo[1]]
        })
        .collect();
    let depths: Vec<f64> = sizes.iter().map(|s| s[1]).collect();
    let widths: Vec<f64> = sizes.iter().map(|s| s[0]).collect();
    let mut orders: Vec<Vec<usize>> = Vec::new();
    for key in [&areas, &depths, &widths] {
        let mut order: Vec<usize> = (0..hulls.len()).collect();
        order.sort_by(|&a, &b| key[b].total_cmp(&key[a]));
        if !orders.contains(&order) {
            orders.push(order);
        }
    }
    orders
}

/// How one run of the greedy placement scores a position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fill {
    /// How far back the object reaches, then how far right.
    BottomLeft,
    /// The bounding rectangle of everything placed so far with the object
    /// (its area, then its longer side relative to the bed), then how
    /// close the object stands to the middle of the bed. The first object
    /// goes in the middle, so that the others can gather all around it.
    Compact,
    /// As [`Fill::Compact`], but from the front left corner of the bed and
    /// then as [`Fill::BottomLeft`], with the rectangle and how far back
    /// the object reaches in steps of [`GRAIN`]: a millimetre or two gained
    /// by tucking an object into a crevice does not count.
    Tight,
}

/// One run of the greedy placement.
#[derive(Debug, Clone, Copy)]
struct Run {
    /// Index of the order among [`orders`].
    order: usize,
    /// How positions are scored.
    fill: Fill,
    /// How many of [`TURNS`] are tried.
    turns: usize,
    /// Whether the run packs the pieces of the real outlines.
    nonconvex: bool,
}

/// The runs, in the order they compete: for the convex hulls and then, with
/// `nonconvex`, for the real outlines, each first the largest-first
/// bottom-left fill (unturned, then in four and eight turns with
/// `rotation`), then the other measures largest first (in two, four and
/// eight turns with `rotation`), then the other orders with all three
/// measures (in eight turns with `rotation`).
fn runs(orders: usize, rotation: bool, nonconvex: bool) -> Vec<Run> {
    let first: &[usize] = if rotation { &[1, 4, 8] } else { &[1] };
    let more: &[usize] = if rotation { &[2, 4, 8] } else { &[1] };
    let mut runs = Vec::new();
    for nonconvex in [false, true].into_iter().take(1 + usize::from(nonconvex)) {
        for &turns in first {
            runs.push(Run {
                order: 0,
                fill: Fill::BottomLeft,
                turns,
                nonconvex,
            });
        }
        for order in 0..orders {
            for fill in [Fill::BottomLeft, Fill::Compact, Fill::Tight] {
                if order == 0 && fill == Fill::BottomLeft {
                    continue;
                }
                let turns: &[usize] = if order == 0 {
                    more
                } else {
                    &more[more.len() - 1..]
                };
                for &turns in turns {
                    runs.push(Run {
                        order,
                        fill,
                        turns,
                        nonconvex,
                    });
                }
            }
        }
    }
    runs
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
        let size = |union: Option<(Point, Point)>| {
            union.map_or([0.0, 0.0], |(lo, hi)| [hi[0] - lo[0], hi[1] - lo[1]])
        };
        let (mine, theirs) = (
            measure(size(self.union), bed),
            measure(size(other.union), bed),
        );
        better(&mine, &theirs)
    }
}

/// How large a rectangle of `size` is: its longer side relative to the bed,
/// then its area relative to that of the bed.
fn measure(size: [f64; 2], bed: [f64; 2]) -> [f64; 2] {
    [
        (size[0] / bed[0]).max(size[1] / bed[1]),
        size[0] * size[1] / (bed[0] * bed[1]),
    ]
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
    fill: Fill,
    directions: &[Point; DIRECTIONS],
) -> Packing {
    let mut placed: Vec<Support> = Vec::new();
    let mut spots: Vec<Option<Spot>> = vec![None; hulls.len()];
    let mut scorer = Scorer {
        fill,
        bed,
        union: None,
    };
    for &index in order {
        let mut best: Option<(Score, Spot, Support)> = None;
        for &turn in turns {
            let (sin, cos) = polygon::sin_cos_degrees(turn);
            let points: Vec<Point> = hulls[index].iter().map(|&p| turned(p, sin, cos)).collect();
            let local = support(&points, directions);
            let bar = best.as_ref().map(|(score, ..)| *score);
            if let Some((score, at)) = position(&local, &placed, bed, gap, &scorer, directions, bar)
            {
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
            scorer.union = enclosing(scorer.union.into_iter().chain([extent(&global)]));
            placed.push(global);
            spots[index] = Some(spot);
        }
    }
    Packing {
        spots,
        placed: placed.len(),
        union: scorer.union,
        centred: false,
    }
}

/// An object placed by [`pack_pieces`]: the support values of its convex
/// pieces where it stands.
struct Placed {
    pieces: Vec<Support>,
}

/// Places the objects with the convex hulls `hulls` and the grown convex
/// pieces `pieces` in `order`, each in the best of `turns`, with their
/// lower left corner at the origin of the bed: like [`pack`], but the
/// pieces keep clear of each other, which are grown by half the gap
/// already, and the hulls stay on the bed and measure the packing.
fn pack_pieces(
    hulls: &[Contour],
    pieces: &[Vec<Contour>],
    order: &[usize],
    turns: &[f64],
    bed: [f64; 2],
    fill: Fill,
    directions: &[Point; DIRECTIONS],
) -> Packing {
    let mut placed: Vec<Placed> = Vec::new();
    let mut spots: Vec<Option<Spot>> = vec![None; hulls.len()];
    let mut scorer = Scorer {
        fill,
        bed,
        union: None,
    };
    for &index in order {
        let mut best: Option<(Score, Spot, Support, Vec<Support>)> = None;
        for &turn in turns {
            let (sin, cos) = polygon::sin_cos_degrees(turn);
            let turn_all = |points: &[Point]| -> Support {
                let points: Vec<Point> = points.iter().map(|&p| turned(p, sin, cos)).collect();
                support(&points, directions)
            };
            let hull = turn_all(&hulls[index]);
            let local: Vec<Support> = pieces[index].iter().map(|p| turn_all(p)).collect();
            let bar = best.as_ref().map(|(score, ..)| *score);
            if let Some((score, at)) =
                position_pieces(&hull, &local, &placed, bed, &scorer, directions, bar)
            {
                let spot = Spot {
                    turn,
                    sin,
                    cos,
                    offset: at,
                };
                best = Some((score, spot, hull, local));
            }
        }
        if let Some((_, spot, hull, local)) = best {
            let at = spot.offset;
            let hull = moved(&hull, at, directions);
            scorer.union = enclosing(scorer.union.into_iter().chain([extent(&hull)]));
            placed.push(Placed {
                pieces: local.iter().map(|p| moved(p, at, directions)).collect(),
            });
            spots[index] = Some(spot);
        }
    }
    Packing {
        spots,
        placed: placed.len(),
        union: scorer.union,
        centred: false,
    }
}

/// Convex pieces that together cover `footprint` grown by `grow`
/// millimetres: thinned out, grown, thinned out again, cut into trapezoids
/// between the heights of its corners, and those joined again as long as
/// they stay convex, or as long as there are too many
/// ([`loosely_joined`]). An empty footprint is
/// covered by its hull `hull` grown by `grow` in X and Y.
fn convex_pieces(footprint: &Shapes, hull: &Contour, grow: f64) -> Vec<Contour> {
    let thin = |shapes: &Shapes, tolerance: f64| -> Shapes {
        let loops: Vec<Contour> = shapes
            .iter()
            .flatten()
            .map(|contour| polygon::simplify(contour, tolerance))
            .filter(|contour| contour.len() >= 3)
            .collect();
        polygon::from_oriented_loops(&loops)
    };
    let grown = thin(
        &polygon::offset(&thin(footprint, FINE), grow + FINE + COARSE),
        COARSE,
    );
    let mut pieces: Vec<(Contour, f64)> = trapezoids(&grown)
        .into_iter()
        .map(|t| {
            let area = polygon::signed_area(&t).abs();
            (t, area)
        })
        .collect();
    pieces = joined(pieces, 0.0);
    if pieces.len() > MAX_PIECES {
        pieces = loosely_joined(pieces);
    }
    if !pieces.is_empty() && pieces.len() <= MAX_PIECES {
        return pieces.into_iter().map(|(piece, _)| piece).collect();
    }
    let points: Vec<Point> = grown.iter().flatten().flatten().copied().collect();
    if points.len() >= 3 {
        return vec![polygon::convex_hull(&points)];
    }
    let g = grow + FINE + COARSE;
    vec![
        hull.iter()
            .flat_map(|p| {
                [
                    [p[0] - g, p[1] - g],
                    [p[0] + g, p[1] - g],
                    [p[0] + g, p[1] + g],
                    [p[0] - g, p[1] + g],
                ]
            })
            .collect(),
    ]
}

/// Cuts `shapes` into trapezoids between the heights of their corners
/// (even-odd), counterclockwise, from the front and from the left.
fn trapezoids(shapes: &Shapes) -> Vec<Contour> {
    let edges: Vec<[Point; 2]> = shapes
        .iter()
        .flatten()
        .flat_map(|contour| {
            (0..contour.len()).map(|i| [contour[i], contour[(i + 1) % contour.len()]])
        })
        .filter(|[a, b]| a[1] != b[1])
        .collect();
    let mut heights: Vec<f64> = shapes.iter().flatten().flatten().map(|p| p[1]).collect();
    heights.sort_by(f64::total_cmp);
    heights.dedup();
    let x_at = |[a, b]: [Point; 2], y: f64| a[0] + (y - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
    let mut trapezoids = Vec::new();
    for pair in heights.windows(2) {
        let (y0, y1) = (pair[0], pair[1]);
        let middle = (y0 + y1) / 2.0;
        // No corner lies between the two heights, so every edge across
        // the middle runs across the whole slab.
        let mut across: Vec<[f64; 3]> = edges
            .iter()
            .filter(|[a, b]| a[1].min(b[1]) < middle && a[1].max(b[1]) > middle)
            .map(|&e| [x_at(e, middle), x_at(e, y0), x_at(e, y1)])
            .collect();
        across.sort_by(|a, b| a[0].total_cmp(&b[0]));
        for &[left, right] in across.as_chunks::<2>().0 {
            trapezoids.push(vec![
                [left[1], y0],
                [right[1], y0],
                [right[2], y1],
                [left[2], y1],
            ]);
        }
    }
    trapezoids
}

/// Joins pieces (with their areas) into their convex hull wherever that
/// adds no more than `looseness` of the hull's area, until none can be
/// joined; only pieces whose bounding boxes touch are tried.
fn joined(mut pieces: Vec<(Contour, f64)>, looseness: f64) -> Vec<(Contour, f64)> {
    let touch = |a: &Contour, b: &Contour| {
        let (Some((alo, ahi)), Some((blo, bhi))) = (bounds(a), bounds(b)) else {
            return false;
        };
        (0..2).all(|i| alo[i] <= bhi[i] + 1e-6 && blo[i] <= ahi[i] + 1e-6)
    };
    loop {
        let before = pieces.len();
        let mut i = 0;
        while i < pieces.len() {
            let mut j = i + 1;
            while j < pieces.len() {
                if touch(&pieces[i].0, &pieces[j].0) {
                    let points: Vec<Point> =
                        pieces[i].0.iter().chain(&pieces[j].0).copied().collect();
                    let hull = polygon::convex_hull(&points);
                    let area = polygon::signed_area(&hull).abs();
                    let sum = pieces[i].1 + pieces[j].1;
                    if area <= sum + looseness * area + 1e-9 * sum + 1e-6 {
                        pieces[i] = (hull, area);
                        pieces.remove(j);
                        j = i + 1;
                        continue;
                    }
                }
                j += 1;
            }
            i += 1;
        }
        if pieces.len() == before {
            return pieces;
        }
    }
}

/// Joins the two pieces whose convex hull adds least area, relative to
/// it, as long as there are more than [`MAX_PIECES`] and the hull adds no
/// more than [`LOOSENESS`] of it; only pieces whose bounding boxes touch
/// are tried. Equal pairs go in the order of the pieces.
fn loosely_joined(pieces: Vec<(Contour, f64)>) -> Vec<(Contour, f64)> {
    let mut pieces: Vec<Option<(Contour, f64)>> = pieces.into_iter().map(Some).collect();
    // How much the hull of two pieces adds, relative to it, with the hull.
    let pair = |a: &(Contour, f64), b: &(Contour, f64)| -> Option<(f64, Contour, f64)> {
        let (Some((alo, ahi)), Some((blo, bhi))) = (bounds(&a.0), bounds(&b.0)) else {
            return None;
        };
        if !(0..2).all(|i| alo[i] <= bhi[i] + 1e-6 && blo[i] <= ahi[i] + 1e-6) {
            return None;
        }
        let points: Vec<Point> = a.0.iter().chain(&b.0).copied().collect();
        let hull = polygon::convex_hull(&points);
        let area = polygon::signed_area(&hull).abs();
        let added = ((area - a.1 - b.1) / area.max(1e-12)).max(0.0);
        (added <= LOOSENESS).then_some((added, hull, area))
    };
    let mut pairs: Vec<(f64, usize, usize, Contour, f64)> = Vec::new();
    for i in 0..pieces.len() {
        for j in i + 1..pieces.len() {
            if let (Some(a), Some(b)) = (&pieces[i], &pieces[j])
                && let Some((added, hull, area)) = pair(a, b)
            {
                pairs.push((added, i, j, hull, area));
            }
        }
    }
    let mut left = pieces.len();
    while left > MAX_PIECES {
        let Some(best) = (0..pairs.len()).min_by(|&x, &y| {
            let (a, b) = (&pairs[x], &pairs[y]);
            a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2))
        }) else {
            break;
        };
        let (_, i, j, hull, area) = pairs.swap_remove(best);
        pieces[i] = Some((hull, area));
        pieces[j] = None;
        left -= 1;
        pairs.retain(|p| p.1 != i && p.2 != i && p.1 != j && p.2 != j);
        for k in 0..pieces.len() {
            if k == i {
                continue;
            }
            if let (Some(a), Some(b)) = (&pieces[i.min(k)], &pieces[i.max(k)])
                && let Some((added, hull, area)) = pair(a, b)
            {
                pairs.push((added, i.min(k), i.max(k), hull, area));
            }
        }
    }
    pieces.into_iter().flatten().collect()
}

/// The bounding box of some points.
fn bounds(points: &[Point]) -> Option<(Point, Point)> {
    enclosing(points.iter().map(|&p| (p, p)))
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

/// How good a position is, compared in order; smaller is better. See
/// [`Scorer`].
type Score = [f64; 4];

fn better(a: &[f64], b: &[f64]) -> bool {
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

/// Scores the positions of an object in one run.
struct Scorer {
    /// What counts.
    fill: Fill,
    /// Size of the bed.
    bed: [f64; 2],
    /// The bounding box of the objects placed so far.
    union: Option<(Point, Point)>,
}

impl Scorer {
    /// The score of the object reaching from `lo` to `hi` around its
    /// reference point, placed at `at`.
    fn score(&self, at: Point, lo: Point, hi: Point) -> Score {
        self.bound(at, at, lo, hi)
    }

    /// No more than the score anywhere with the reference point in the
    /// box from `from` to `to`; the score at `from` if that is `to`.
    fn bound(&self, from: Point, to: Point, lo: Point, hi: Point) -> Score {
        let reach = [from[1] + hi[1], from[0] + hi[0]];
        match self.fill {
            Fill::BottomLeft => [reach[0], reach[1], 0.0, 0.0],
            Fill::Compact | Fill::Tight => {
                // Per axis the size is smallest with the object as far
                // inside the union as the box lets it, and the distance to
                // the middle of the bed with the object as close to it.
                let mut size = [0.0; 2];
                let mut distance = 0.0;
                for a in 0..2 {
                    size[a] = self.union.map_or(hi[a] - lo[a], |(l, h)| {
                        let middle = (l[a] - lo[a] + h[a] - hi[a]) / 2.0;
                        let at = middle.max(from[a]).min(to[a]);
                        (at + hi[a]).max(h[a]) - (at + lo[a]).min(l[a])
                    });
                    let centre = self.bed[a] / 2.0 - (lo[a] + hi[a]) / 2.0;
                    let off = (from[a] - centre).max(centre - to[a]).max(0.0);
                    distance += off * off;
                }
                if self.fill == Fill::Tight {
                    let [side, area] = measure(size.map(coarse), self.bed);
                    return [area, side, coarse(reach[0]), reach[1]];
                }
                let [side, area] = measure(size, self.bed);
                [area, side, distance, 0.0]
            }
        }
    }

    /// Further positions to try for [`Fill::Compact`], for the object
    /// reaching from `lo` to `hi` around its reference point: in the middle
    /// of the bed, and in the corners of the rectangle around the objects
    /// placed so far.
    fn extra(&self, lo: Point, hi: Point) -> Vec<Point> {
        if self.fill == Fill::BottomLeft {
            return Vec::new();
        }
        let middle = [
            self.bed[0] / 2.0 - (lo[0] + hi[0]) / 2.0,
            self.bed[1] / 2.0 - (lo[1] + hi[1]) / 2.0,
        ];
        let middle = (self.fill == Fill::Compact).then_some(middle);
        let lines = self.lines(lo, hi);
        let (xs, ys) = lines.split_at(lines.len() / 2);
        let corners = xs
            .iter()
            .flat_map(|&(_, x)| ys.iter().map(move |&(_, y)| [x, y]));
        middle.into_iter().chain(corners).collect()
    }

    /// For [`Fill::Compact`], where the reference point puts an edge of
    /// the object reaching from `lo` to `hi` on the same edge of the
    /// rectangle around the objects placed so far, by axis: no-fit
    /// polygons are tried where they cross these lines too.
    fn lines(&self, lo: Point, hi: Point) -> Vec<(usize, f64)> {
        match (self.fill, self.union) {
            (Fill::Compact | Fill::Tight, Some((l, h))) => vec![
                (0, l[0] - lo[0]),
                (0, h[0] - hi[0]),
                (1, l[1] - lo[1]),
                (1, h[1] - hi[1]),
            ],
            _ => Vec::new(),
        }
    }
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
    /// The no-fit polygon with these support values.
    fn new(support: Support, directions: &[Point; DIRECTIONS]) -> Self {
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
    }

    /// Whether the position `p` keeps clear of it, looking at its bounding
    /// box first; the same as [`NoFit::clear`].
    fn clear_fast(&self, p: Point, directions: &[Point; DIRECTIONS]) -> bool {
        p[0] >= self.hi[0] - TOLERANCE
            || p[0] <= self.lo[0] + TOLERANCE
            || p[1] >= self.hi[1] - TOLERANCE
            || p[1] <= self.lo[1] + TOLERANCE
            || self.clear(p, directions)
    }

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
    scorer: &Scorer,
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
            NoFit::new(support, directions)
        })
        .collect();

    let mut candidates: Vec<Point> = vec![
        fit_lo,
        [fit_hi[0], fit_lo[1]],
        [fit_lo[0], fit_hi[1]],
        fit_hi,
    ];
    candidates.extend(scorer.extra(lo, hi));
    let lines = bed_lines(fit_lo, fit_hi, scorer.lines(lo, hi));
    let mut near = (Vec::new(), Vec::new());
    for (i, no_fit) in no_fits.iter().enumerate() {
        for &[a, b] in &no_fit.edges {
            candidates.push(b);
            // Where the edge meets the edges of the bed.
            line_crossings(a, b, &lines, &mut candidates);
        }
        // Where it crosses the no-fit polygons of the others.
        for other in &no_fits[i + 1..] {
            if overlap(no_fit, other) {
                crossings_of(no_fit, other, &mut near, &mut candidates);
            }
        }
    }

    let mut best: Option<(Score, Point)> = None;
    for at in candidates {
        let score = scorer.score(at, lo, hi);
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

/// Whether the bounding boxes of two no-fit polygons overlap.
fn overlap(a: &NoFit, b: &NoFit) -> bool {
    !(b.lo[0] > a.hi[0] || b.hi[0] < a.lo[0] || b.lo[1] > a.hi[1] || b.hi[1] < a.lo[1])
}

/// The lines through the edges of the box from `fit_lo` to `fit_hi`, by
/// axis and value, followed by `more`.
fn bed_lines(fit_lo: Point, fit_hi: Point, more: Vec<(usize, f64)>) -> Vec<(usize, f64)> {
    let mut lines = vec![
        (0, fit_lo[0]),
        (0, fit_hi[0]),
        (1, fit_lo[1]),
        (1, fit_hi[1]),
    ];
    lines.extend(more);
    lines
}

/// Adds where the segment from `a` to `b` meets the `lines`, given by the
/// axis they cross and their value on it.
fn line_crossings(a: Point, b: Point, lines: &[(usize, f64)], candidates: &mut Vec<Point>) {
    for &(axis, limit) in lines {
        let (from, to) = (a[axis] - limit, b[axis] - limit);
        if (from < 0.0) != (to < 0.0) {
            let t = from / (from - to);
            candidates.push([a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]);
        }
    }
}

/// Like [`position`] for an object with the convex hull `hull` and the
/// grown convex pieces `local` (support values, unmoved) among the placed
/// objects `placed`: a position keeps clear when every piece does of every
/// placed piece. Faster for many pieces: the no-fit polygons that cannot
/// reach the bed are left out, a position is first checked against the
/// polygon that rejected the one before, and where two polygons cross is
/// only computed where that could beat the best position found at their
/// corners.
fn position_pieces(
    hull: &Support,
    local: &[Support],
    placed: &[Placed],
    bed: [f64; 2],
    scorer: &Scorer,
    directions: &[Point; DIRECTIONS],
    bar: Option<Score>,
) -> Option<(Score, Point)> {
    let (lo, hi) = extent(hull);
    let fit_lo = [-lo[0], -lo[1]];
    let fit_hi = [bed[0] - hi[0], bed[1] - hi[1]];
    if fit_lo[0] > fit_hi[0] + TOLERANCE || fit_lo[1] > fit_hi[1] + TOLERANCE {
        return None;
    }
    let mut no_fits: Vec<NoFit> = Vec::new();
    for other in placed {
        for piece in &other.pieces {
            for own in local {
                let support: Support =
                    std::array::from_fn(|k| piece[k] + own[(k + HALF) % DIRECTIONS]);
                let (nlo, nhi) = extent(&support);
                let reaches_bed = (0..2)
                    .all(|a| nlo[a] < fit_hi[a] + TOLERANCE && nhi[a] > fit_lo[a] - TOLERANCE);
                if reaches_bed {
                    no_fits.push(NoFit::new(support, directions));
                }
            }
        }
    }

    let mut best: Option<(Score, Point)> = None;
    let mut blocker = 0;
    let mut consider = |at: Point, best: &mut Option<(Score, Point)>| {
        let score = scorer.score(at, lo, hi);
        let bar = best.as_ref().map(|(score, _)| score).or(bar.as_ref());
        if bar.is_some_and(|bar| !better(&score, bar)) {
            return;
        }
        let inside =
            (0..2).all(|a| at[a] >= fit_lo[a] - TOLERANCE && at[a] <= fit_hi[a] + TOLERANCE);
        if !inside {
            return;
        }
        if no_fits
            .get(blocker)
            .is_some_and(|n| !n.clear_fast(at, directions))
        {
            return;
        }
        if let Some(k) = no_fits.iter().position(|n| !n.clear_fast(at, directions)) {
            blocker = k;
            return;
        }
        *best = Some((score, at));
    };
    let corners = [
        fit_lo,
        [fit_hi[0], fit_lo[1]],
        [fit_lo[0], fit_hi[1]],
        fit_hi,
    ];
    for at in corners.into_iter().chain(scorer.extra(lo, hi)) {
        consider(at, &mut best);
    }
    let lines = bed_lines(fit_lo, fit_hi, scorer.lines(lo, hi));
    let mut on_edges = Vec::new();
    for no_fit in &no_fits {
        for &[a, b] in &no_fit.edges {
            consider(b, &mut best);
            on_edges.clear();
            line_crossings(a, b, &lines, &mut on_edges);
            for &at in &on_edges {
                consider(at, &mut best);
            }
        }
    }

    // Where two no-fit polygons cross, only where that could still beat
    // the best so far: a crossing lies in the bounding boxes of both.
    let could_beat = |from: Point, to: Point, best: &Option<(Score, Point)>| {
        let from = [from[0].max(fit_lo[0]), from[1].max(fit_lo[1])];
        let to = [to[0].min(fit_hi[0]), to[1].min(fit_hi[1])];
        if from[0] > to[0] + TOLERANCE || from[1] > to[1] + TOLERANCE {
            return false;
        }
        let bound = scorer.bound(from, to, lo, hi);
        match best.as_ref().map(|(score, _)| score).or(bar.as_ref()) {
            // A score no smaller than the bound can beat the bar only if
            // the bound does.
            Some(bar) => better(&bound, bar),
            None => true,
        }
    };
    let mut active: Vec<usize> = (0..no_fits.len())
        .filter(|&i| could_beat(no_fits[i].lo, no_fits[i].hi, &best))
        .collect();
    active.sort_by(|&a, &b| {
        no_fits[a].lo[0]
            .total_cmp(&no_fits[b].lo[0])
            .then(a.cmp(&b))
    });
    let mut crossings = Vec::new();
    let mut near = (Vec::new(), Vec::new());
    for (n, &i) in active.iter().enumerate() {
        let a = &no_fits[i];
        for &j in &active[n + 1..] {
            let b = &no_fits[j];
            if b.lo[0] > a.hi[0] {
                break;
            }
            if !overlap(a, b) {
                continue;
            }
            let from = [a.lo[0].max(b.lo[0]), a.lo[1].max(b.lo[1])];
            let to = [a.hi[0].min(b.hi[0]), a.hi[1].min(b.hi[1])];
            if !could_beat(from, to, &best) {
                continue;
            }
            crossings.clear();
            crossings_of(a, b, &mut near, &mut crossings);
            for &at in &crossings {
                consider(at, &mut best);
            }
        }
    }
    best
}

/// Adds where the edges of two no-fit polygons whose bounding boxes
/// overlap cross, in the order of their edges; `near` is room for the edges
/// that reach into both boxes, the only ones that can.
fn crossings_of(
    a: &NoFit,
    b: &NoFit,
    near: &mut (Vec<[Point; 2]>, Vec<[Point; 2]>),
    out: &mut Vec<Point>,
) {
    let from = [a.lo[0].max(b.lo[0]), a.lo[1].max(b.lo[1])];
    let to = [a.hi[0].min(b.hi[0]), a.hi[1].min(b.hi[1])];
    let reaching = |edges: &[[Point; 2]], into: &mut Vec<[Point; 2]>| {
        into.clear();
        into.extend(edges.iter().filter(|[p, q]| {
            (0..2).all(|k| {
                p[k].min(q[k]) <= to[k] + TOLERANCE && p[k].max(q[k]) >= from[k] - TOLERANCE
            })
        }));
    };
    reaching(&a.edges, &mut near.0);
    reaching(&b.edges, &mut near.1);
    for &e in &near.0 {
        for &f in &near.1 {
            if let Some(p) = crossing(e, f) {
                out.push(p);
            }
        }
    }
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

    /// A U of `width` by `depth` with walls `wall` thick, open to the back,
    /// standing 5 mm tall: three boxes in one mesh.
    fn u_shape(width: f64, depth: f64, wall: f64) -> Mesh {
        let mut mesh = Mesh::cuboid(width, wall, 5.0);
        let mut side = Mesh::cuboid(wall, depth, 5.0);
        mesh.merge(&side);
        side.translate([width - wall, 0.0, 0.0]);
        mesh.merge(&side);
        mesh
    }

    /// Distance between the segments from `a` to `b` and from `c` to `d`,
    /// which do not cross.
    fn segment_gap(a: Point, b: Point, c: Point, d: Point) -> f64 {
        let to_segment = |p: Point, s: Point, t: Point| {
            let e = [t[0] - s[0], t[1] - s[1]];
            let w = [p[0] - s[0], p[1] - s[1]];
            let u = (dot(w, e) / dot(e, e).max(1e-30)).clamp(0.0, 1.0);
            polygon::distance(p, [s[0] + u * e[0], s[1] + u * e[1]])
        };
        to_segment(a, c, d)
            .min(to_segment(b, c, d))
            .min(to_segment(c, a, b))
            .min(to_segment(d, a, b))
    }

    /// Distance between the real outlines of two meshes seen from above; 0
    /// where they overlap.
    fn real_gap(a: &Mesh, b: &Mesh) -> f64 {
        let (a, b) = (plate::top_view(a), plate::top_view(b));
        if polygon::area(&polygon::intersection(&a, &b)) > 1e-6 {
            return 0.0;
        }
        let edges = |shapes: &Shapes| -> Vec<[Point; 2]> {
            shapes
                .iter()
                .flatten()
                .flat_map(|c| (0..c.len()).map(move |i| [c[i], c[(i + 1) % c.len()]]))
                .collect()
        };
        let (ea, eb) = (edges(&a), edges(&b));
        ea.iter()
            .flat_map(|e| eb.iter().map(move |f| segment_gap(e[0], e[1], f[0], f[1])))
            .fold(f64::INFINITY, f64::min)
    }

    /// The area of the rectangle around `meshes` seen from above.
    fn rectangle(meshes: &[Mesh]) -> f64 {
        let (lo, hi) = enclosing(meshes.iter().map(|m| {
            let (lo, hi) = m.bounds().unwrap();
            ([lo[0], lo[1]], [hi[0], hi[1]])
        }))
        .unwrap();
        (hi[0] - lo[0]) * (hi[1] - lo[1])
    }

    /// Checks that the arranged objects keep their real outlines at least
    /// `gap` apart and raise no overlap warning.
    fn assert_nested(objects: &[Mesh], settings: &Settings, gap: f64) {
        for (i, a) in objects.iter().enumerate() {
            for b in &objects[i + 1..] {
                let distance = real_gap(a, b);
                assert!(distance >= gap - 1e-6, "{distance} < {gap}");
            }
        }
        let mut diagnostics = Diagnostics::new();
        plate::check(objects, Placement::AsGiven, settings, &mut diagnostics).unwrap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
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
    fn boxes_leave_their_rows_only_for_a_smaller_rectangle() {
        // The rows put seven boxes 25 mm deep into a rectangle of 190 by
        // 55 mm. Largest first and bottom-left, outlines pack them no
        // closer, so they stayed there before further runs came; keeping
        // the rectangle small packs them into a clearly smaller one.
        let objects: Vec<Mesh> = (1..8)
            .map(|i| Mesh::cuboid(10.0 * f64::from(i), 25.0, 5.0))
            .collect();
        let mut rows = objects.clone();
        arrange_objects(&mut rows, &Settings::default(), ARRANGE_GAP);
        let settings = Settings {
            arrange_outlines: true,
            ..Settings::default()
        };
        let outlines: Vec<Contour> = objects.iter().map(plate::footprint).collect();
        let order: Vec<usize> = (0..7).rev().collect();
        let bed = [settings.bed_size_x, settings.bed_size_y];
        let first = pack(
            &outlines,
            &order,
            &TURNS[..1],
            bed,
            ARRANGE_GAP,
            Fill::BottomLeft,
            &directions(),
        );
        let rows_packing = rows_of(&outlines, &settings);
        assert!(!first.beats(&rows_packing, bed));
        for settings in [
            settings,
            Settings {
                arrange_rotation: true,
                ..Settings::default()
            },
        ] {
            let mut packed = objects.clone();
            arrange_objects(&mut packed, &settings, ARRANGE_GAP);
            assert_arranged(&packed, &settings, ARRANGE_GAP);
            assert!(rectangle(&packed) < 0.9 * rectangle(&rows));
        }
    }

    /// The rows of rectangles as a packing.
    fn rows_of(outlines: &[Contour], settings: &Settings) -> Packing {
        rows(outlines, settings, ARRANGE_GAP, &directions())
    }

    #[test]
    fn eight_triangles_fit_with_several_orders() {
        // Right triangles with 90 mm legs on the 220 mm bed: largest first
        // and bottom-left, the second one goes beside the first instead of
        // against it, and only seven fit. Keeping the rectangle small pairs
        // them into squares, and all eight fit.
        let settings = Settings {
            arrange_rotation: true,
            ..Settings::default()
        };
        let objects = vec![triangle(90.0); 8];
        let outlines: Vec<Contour> = objects.iter().map(plate::footprint).collect();
        let directions = directions();
        let order: Vec<usize> = (0..8).collect();
        let bed = [settings.bed_size_x, settings.bed_size_y];
        let first = [1, 4, 8].map(|turns| {
            let packing = pack(
                &outlines,
                &order,
                &TURNS[..turns],
                bed,
                ARRANGE_GAP,
                Fill::BottomLeft,
                &directions,
            );
            packing.placed
        });
        assert_eq!(first.into_iter().max(), Some(7));
        assert_eq!(off_bed(&objects, &settings), 0);
        let mut arranged = objects.clone();
        arrange_objects(&mut arranged, &settings, ARRANGE_GAP);
        assert_arranged(&arranged, &settings, ARRANGE_GAP);
        // So they do with the real outlines.
        let nonconvex = Settings {
            arrange_nonconvex: true,
            ..settings
        };
        assert_eq!(off_bed(&objects, &nonconvex), 0);
    }

    #[test]
    fn ells_nest_closer_than_their_hulls() {
        let convex = Settings {
            arrange_outlines: true,
            ..Settings::default()
        };
        let nonconvex = Settings {
            arrange_nonconvex: true,
            ..Settings::default()
        };
        let objects = vec![ell(60.0, 20.0); 4];
        let mut hulls = objects.clone();
        arrange_objects(&mut hulls, &convex, ARRANGE_GAP);
        assert_arranged(&hulls, &convex, ARRANGE_GAP);
        let mut nested = objects.clone();
        arrange_objects(&mut nested, &nonconvex, ARRANGE_GAP);
        assert_nested(&nested, &nonconvex, ARRANGE_GAP);
        let (by_hulls, by_outlines) = (rectangle(&hulls), rectangle(&nested));
        assert!(by_outlines < 0.8 * by_hulls, "{by_outlines} {by_hulls}");
        // Nested, their convex outlines overlap: the default warning would
        // report them.
        let mut diagnostics = Diagnostics::new();
        plate::check(&nested, Placement::AsGiven, &convex, &mut diagnostics).unwrap();
        assert!(diagnostics.codes().contains(&"slicing.objects_overlap"));
        // Turned as well, and deterministic.
        let turning = Settings {
            arrange_rotation: true,
            ..nonconvex.clone()
        };
        let mut turned = objects.clone();
        arrange_objects(&mut turned, &turning, ARRANGE_GAP);
        assert_nested(&turned, &turning, ARRANGE_GAP);
        assert!(rectangle(&turned) <= by_outlines + 1e-6);
        let mut again = objects;
        arrange_objects(&mut again, &turning, ARRANGE_GAP);
        assert_eq!(turned, again);
    }

    #[test]
    fn a_small_part_stands_in_a_u() {
        let settings = Settings {
            arrange_nonconvex: true,
            brim_width: 2.0,
            ..Settings::default()
        };
        let objects = vec![u_shape(80.0, 80.0, 10.0), Mesh::cuboid(25.0, 25.0, 5.0)];
        let mut arranged = objects.clone();
        arrange_objects(&mut arranged, &settings, ARRANGE_GAP);
        assert_nested(&arranged, &settings, ARRANGE_GAP + 4.0);
        let (lo, hi) = arranged[0].bounds().unwrap();
        let (small_lo, small_hi) = arranged[1].bounds().unwrap();
        assert!(
            (0..2).all(|a| small_lo[a] > lo[a] && small_hi[a] < hi[a]),
            "{arranged:?}"
        );
        // By convex outlines it stands beside it.
        let convex = Settings {
            arrange_nonconvex: false,
            arrange_outlines: true,
            ..settings
        };
        let mut beside = objects;
        arrange_objects(&mut beside, &convex, ARRANGE_GAP);
        assert!(rectangle(&beside) > 1.2 * rectangle(&arranged));
    }

    #[test]
    fn real_outlines_keep_the_clearance_convex() {
        // Printed one after the other, the convex outlines keep the
        // clearance: the L-shapes do not nest.
        let settings = Settings {
            sequential_print: true,
            arrange_nonconvex: true,
            bed_size_x: 300.0,
            bed_size_y: 300.0,
            ..Settings::default()
        };
        let mut arranged = vec![ell(80.0, 20.0); 3];
        arrange_objects(&mut arranged, &settings, ARRANGE_GAP);
        assert_arranged(&arranged, &settings, settings.extruder_clearance_radius);
    }

    #[test]
    fn real_outlines_are_off_by_default() {
        let settings = Settings::default();
        assert!(!settings.arrange_nonconvex);
        assert!(!arranges_outlines(&settings));
        // A convex object is one piece, an L two and a U three.
        let cylinder = primitives::cylinder(15.0, 5.0, 64);
        let hull = plate::footprint(&cylinder);
        let pieces = |mesh: &Mesh| convex_pieces(&plate::top_view(mesh), &hull, 2.5).len();
        assert_eq!(pieces(&cylinder), 1);
        assert_eq!(pieces(&ell(60.0, 20.0)), 2);
        assert_eq!(pieces(&u_shape(80.0, 80.0, 10.0)), 3);
    }

    #[test]
    fn fifty_objects_arrange_quickly() {
        // Mixed objects with real outlines and turns: well under a second
        // in an optimised build; the bound leaves room for slow machines.
        let shapes = [
            ell(50.0, 15.0),
            triangle(40.0),
            primitives::cylinder(12.0, 5.0, 48),
            u_shape(50.0, 40.0, 8.0),
            Mesh::cuboid(30.0, 15.0, 5.0),
        ];
        let objects: Vec<Mesh> = (0..50).map(|i| shapes[i % 5].clone()).collect();
        let settings = Settings {
            arrange_nonconvex: true,
            arrange_rotation: true,
            bed_size_x: 400.0,
            bed_size_y: 400.0,
            ..Settings::default()
        };
        let start = std::time::Instant::now();
        let mut arranged = objects.clone();
        arrange_objects(&mut arranged, &settings, ARRANGE_GAP);
        let took = start.elapsed();
        eprintln!("50 objects: {took:?}");
        assert_eq!(off_bed(&objects, &settings), 0);
        assert_nested(&arranged, &settings, ARRANGE_GAP);
        assert!(took.as_secs_f64() < 10.0, "{took:?}");
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
