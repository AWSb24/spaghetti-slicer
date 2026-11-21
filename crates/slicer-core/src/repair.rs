//! Repair of broken meshes before slicing, and the report of what was done
//! and what could not be fixed.
//!
//! [`Mesh::repair`] runs these steps in order:
//!
//! 1. Triangles that face the other way than their neighbours are turned
//!    ([`Mesh::orient`]).
//! 2. Triangles without area are removed ([`Mesh::remove_degenerate`]).
//! 3. Repeated triangles are removed ([`Mesh::remove_duplicates`]); a pair
//!    facing opposite ways goes completely, so two solids that touch along a
//!    face become one. Step 1 runs again if any went.
//! 4. Holes are closed ([`Mesh::close_holes`]).
//! 5. Closed shells that face inward are turned outward
//!    ([`Mesh::orient_outward`]); cavities stay cavities.
//!
//! Every step works on sorted lists or index arrays, never on the iteration
//! order of a hash map, so the result is the same on every platform. The
//! work is bounded: each step is O(n log n) in the number of triangles,
//! except where a budget caps it (filling long rims, finding nested shells).

use crate::diagnostics::{Diagnostic, Diagnostics};
use crate::mesh::{Mesh, cross, dot, sub};

/// Longest rim (in edges) of a hole that [`Mesh::repair`] closes.
pub const MAX_HOLE_EDGES: usize = 100_000;

/// Rims up to this many edges that can be filled neither by ear clipping
/// nor by a fan around their centre still get a fan from their first
/// corner, which may fold over but closes the surface. Longer ones stay
/// open.
const SMALL_HOLE_EDGES: usize = 64;

/// Point tests ear clipping and the check that a rim does not cross itself
/// may spend on one rim, and in one call of [`Mesh::close_holes`] in all.
const RIM_BUDGET: usize = 20_000_000;
const HOLES_BUDGET: usize = 100_000_000;

/// Triangle tests [`Mesh::orient_outward`] may spend on finding out which
/// closed shells lie inside which. Beyond it no shell is turned.
const NESTING_BUDGET: usize = 50_000_000;

/// What [`Mesh::repair`] changed, and the defects that are left.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MeshRepair {
    /// Triangles turned to face the same way as their neighbours.
    pub turned: usize,
    /// Triangles without area removed.
    pub degenerate: usize,
    /// Repeated triangles removed.
    pub duplicates: usize,
    /// Holes closed.
    pub holes: usize,
    /// Closed shells that faced inward and were turned outward.
    pub shells: usize,
    /// Edges left without a matching neighbour (open or inconsistently
    /// oriented).
    pub open_edges: usize,
    /// Edges left that more than two triangles share.
    pub non_manifold_edges: usize,
}

impl MeshRepair {
    /// Whether the repair changed the mesh.
    pub fn changed(&self) -> bool {
        self.turned + self.degenerate + self.duplicates + self.holes + self.shells > 0
    }

    /// Whether the mesh is closed and manifold after the repair.
    pub fn is_clean(&self) -> bool {
        self.open_edges == 0 && self.non_manifold_edges == 0
    }

    /// What was done, for people, such as "2 holes closed"; empty when
    /// nothing was.
    fn done(&self) -> Vec<String> {
        [
            (self.holes, "hole(s) closed"),
            (self.turned, "triangle(s) turned to match their neighbours"),
            (self.shells, "inside-out shell(s) turned outward"),
            (self.degenerate, "triangle(s) without area removed"),
            (self.duplicates, "repeated triangle(s) removed"),
        ]
        .iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, what)| format!("{count} {what}"))
        .collect()
    }

    /// Reports the repair: nothing for a mesh that needed none,
    /// `mesh.repaired` with the counts of what was done for one that is
    /// clean now, and `mesh.not_repairable` with the defects that are left
    /// and the counts of what was done for one that is not.
    pub fn report(&self, diagnostics: &mut Diagnostics) {
        let with_counts = |diagnostic: Diagnostic| {
            diagnostic
                .with("holes", self.holes)
                .with("triangles", self.turned)
                .with("shells", self.shells)
                .with("degenerate", self.degenerate)
                .with("duplicates", self.duplicates)
        };
        let done = self.done().join(", ");
        if !self.is_clean() {
            let done = if done.is_empty() {
                String::new()
            } else {
                format!(" ({done})")
            };
            diagnostics.push(
                with_counts(Diagnostic::warning(
                    "mesh.not_repairable",
                    format!(
                        "the mesh could not be repaired{done}: {} open edge(s) and {} edge(s) shared by more than two triangles are left, so affected layers may be wrong or incomplete; close the remaining holes and separate touching or overlapping parts in a mesh editor",
                        self.open_edges, self.non_manifold_edges
                    ),
                ))
                .with("open_edges", self.open_edges)
                .with("non_manifold_edges", self.non_manifold_edges),
            );
        } else if self.changed() {
            diagnostics.push(with_counts(Diagnostic::warning(
                "mesh.repaired",
                format!("the mesh was repaired before slicing: {done}"),
            )));
        }
    }
}

impl Mesh {
    /// Repairs what can be repaired (see the [module](self) for the steps)
    /// and says what was done and what is left. `tolerance` (mm) is that of
    /// [`Mesh::remove_degenerate`].
    pub fn repair(&mut self, tolerance: f64) -> MeshRepair {
        let mut repair = MeshRepair {
            turned: self.orient(),
            degenerate: self.remove_degenerate(tolerance),
            duplicates: self.remove_duplicates(),
            ..MeshRepair::default()
        };
        if repair.duplicates > 0 {
            repair.turned += self.orient();
        }
        repair.holes = self.close_holes(MAX_HOLE_EDGES);
        repair.shells = self.orient_outward();
        (repair.open_edges, repair.non_manifold_edges) = self.edge_defects();
        repair
    }

    /// Removes repeated triangles (the same three corners) and returns how
    /// many were removed. Of copies facing the same way one stays. Copies
    /// facing opposite ways cancel out: as many as face one way go with as
    /// many facing the other, and of the surplus one stays.
    pub fn remove_duplicates(&mut self) -> usize {
        // The corners in ascending order, and whether the triangle runs
        // through them in that order (up to rotation).
        let key = |t: [u32; 3]| {
            let r = if t[0] <= t[1] && t[0] <= t[2] {
                t
            } else if t[1] <= t[2] {
                [t[1], t[2], t[0]]
            } else {
                [t[2], t[0], t[1]]
            };
            if r[1] <= r[2] {
                (r, true)
            } else {
                ([r[0], r[2], r[1]], false)
            }
        };
        let keys: Vec<([u32; 3], bool)> = self.triangles.iter().map(|&t| key(t)).collect();
        let mut order: Vec<usize> = (0..keys.len()).collect();
        order.sort_unstable_by_key(|&i| (keys[i].0, i));
        let mut keep = vec![true; keys.len()];
        let mut removed = 0;
        for group in order.chunk_by(|&a, &b| keys[a].0 == keys[b].0) {
            if group.len() < 2 {
                continue;
            }
            let forward = group.iter().filter(|&&i| keys[i].1).count();
            let backward = group.len() - forward;
            let survivor = (forward != backward)
                .then(|| {
                    group
                        .iter()
                        .copied()
                        .find(|&i| keys[i].1 == (forward > backward))
                })
                .flatten();
            for &i in group {
                if Some(i) != survivor {
                    keep[i] = false;
                    removed += 1;
                }
            }
        }
        if removed > 0 {
            self.retain_triangles(&keep);
        }
        removed
    }

    /// Keeps the triangles whose entry in `keep` is true, with their
    /// extruders.
    fn retain_triangles(&mut self, keep: &[bool]) {
        let mut index = 0;
        self.triangles.retain(|_| {
            index += 1;
            keep[index - 1]
        });
        if !self.extruders.is_empty() {
            self.extruders.resize(keep.len(), 0);
            let mut index = 0;
            self.extruders.retain(|_| {
                index += 1;
                keep[index - 1]
            });
        }
    }

    /// Turns closed shells that face inward so that they face outward, and
    /// returns how many were turned. Call it after [`Mesh::orient`].
    ///
    /// A shell is a piece of the surface whose triangles are joined by
    /// edges with exactly two triangles; it is closed when each of its
    /// edges is used once in each direction. A closed shell inside another
    /// one (all of three sample points, and its bounding box) is that
    /// shell's cavity or an embedded part: it keeps its way relative to the
    /// shell around it and is turned exactly when that one is. Any other
    /// closed shell is turned when its volume is negative. Open shells are
    /// left alone, as are all shells when finding out which lie inside
    /// which would take too long.
    pub fn orient_outward(&mut self) -> usize {
        let n = self.triangles.len();
        // Triangles joined by edges with exactly two users.
        let mut edges: Vec<(u32, u32, usize)> = Vec::with_capacity(3 * n);
        for (i, t) in self.triangles.iter().enumerate() {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                edges.push((a.min(b), a.max(b), i));
            }
        }
        edges.sort_unstable();
        let mut root: Vec<usize> = (0..n).collect();
        let find = |root: &mut Vec<usize>, mut i: usize| {
            while root[i] != i {
                root[i] = root[root[i]];
                i = root[i];
            }
            i
        };
        for pair in edges.chunk_by(|x, y| (x.0, x.1) == (y.0, y.1)) {
            if let [x, y] = pair {
                let (a, b) = (find(&mut root, x.2), find(&mut root, y.2));
                root[a.max(b)] = a.min(b);
            }
        }
        // Shells numbered in the order of their first triangle.
        let mut shell_of = vec![usize::MAX; n];
        let mut members: Vec<Vec<usize>> = Vec::new();
        for i in 0..n {
            let r = find(&mut root, i);
            if shell_of[r] == usize::MAX {
                shell_of[r] = members.len();
                members.push(Vec::new());
            }
            shell_of[i] = shell_of[r];
            members[shell_of[i]].push(i);
        }
        // A shell is closed when every edge of it is used as often in one
        // direction as in the other.
        let mut directed: Vec<(usize, u32, u32, i32)> = Vec::with_capacity(3 * n);
        for (i, t) in self.triangles.iter().enumerate() {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                directed.push((shell_of[i], a.min(b), a.max(b), if a < b { 1 } else { -1 }));
            }
        }
        directed.sort_unstable();
        let mut closed = vec![true; members.len()];
        for edge in directed.chunk_by(|x, y| (x.0, x.1, x.2) == (y.0, y.1, y.2)) {
            if edge.iter().map(|e| e.3).sum::<i32>() != 0 {
                closed[edge[0].0] = false;
            }
        }

        struct Shell {
            index: usize,
            volume: f64,
            lo: [f64; 3],
            hi: [f64; 3],
        }
        let mut shells: Vec<Shell> = Vec::new();
        for (index, list) in members.iter().enumerate() {
            if !closed[index] {
                continue;
            }
            let origin = self.vertices[self.triangles[list[0]][0] as usize];
            let (mut lo, mut hi) = (origin, origin);
            let mut six_v = 0.0;
            for &i in list {
                let [a, b, c] = self.triangles[i].map(|v| self.vertices[v as usize]);
                for p in [a, b, c] {
                    for axis in 0..3 {
                        lo[axis] = lo[axis].min(p[axis]);
                        hi[axis] = hi[axis].max(p[axis]);
                    }
                }
                six_v += dot(sub(a, origin), cross(sub(b, origin), sub(c, origin)));
            }
            if six_v != 0.0 {
                shells.push(Shell {
                    index,
                    volume: six_v,
                    lo,
                    hi,
                });
            }
        }
        // Largest first, so that a shell's surroundings are decided before
        // the shell itself.
        shells.sort_by(|a, b| {
            b.volume
                .abs()
                .total_cmp(&a.volume.abs())
                .then(a.index.cmp(&b.index))
        });
        let mut budget = NESTING_BUDGET;
        let mut turn = vec![false; shells.len()];
        for s in 0..shells.len() {
            let shell = &shells[s];
            let list = &members[shell.index];
            let samples: Vec<[f64; 3]> = [list[0], list[list.len() / 2], list[list.len() - 1]]
                .iter()
                .map(|&i| {
                    let [a, b, c] = self.triangles[i].map(|v| self.vertices[v as usize]);
                    [0, 1, 2].map(|axis| (a[axis] + b[axis] + c[axis]) / 3.0)
                })
                .collect();
            // The smallest larger shell that holds it.
            let mut parent = None;
            for o in (0..s).rev() {
                let outer = &shells[o];
                let within = (0..3).all(|axis| {
                    outer.lo[axis] <= shell.lo[axis] && shell.hi[axis] <= outer.hi[axis]
                });
                if !within {
                    continue;
                }
                let around = &members[outer.index];
                let cost = around.len() * samples.len();
                if cost > budget {
                    return 0;
                }
                budget -= cost;
                if samples.iter().all(|&p| self.encloses(around, p)) {
                    parent = Some(o);
                    break;
                }
            }
            turn[s] = match parent {
                Some(o) => turn[o],
                None => shell.volume < 0.0,
            };
        }
        let mut turned = 0;
        for (shell, _) in shells.iter().zip(&turn).filter(|(_, turn)| **turn) {
            for &i in &members[shell.index] {
                self.triangles[i].swap(1, 2);
            }
            turned += 1;
        }
        turned
    }

    /// Whether the point `p` lies inside the closed surface made of
    /// `triangles`: whether a ray from it straight up crosses the surface an
    /// odd number of times. A ray through an edge or corner counts for
    /// exactly one of the triangles there (a fixed rule for which side an
    /// edge belongs to, as rasterizers use).
    fn encloses(&self, triangles: &[usize], p: [f64; 3]) -> bool {
        let mut inside = false;
        for &i in triangles {
            let [a, mut b, mut c] = self.triangles[i].map(|v| self.vertices[v as usize]);
            let edge = |u: [f64; 3], v: [f64; 3]| {
                (v[0] - u[0]) * (p[1] - u[1]) - (v[1] - u[1]) * (p[0] - u[0])
            };
            let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            if area == 0.0 {
                continue;
            }
            if area < 0.0 {
                (b, c) = (c, b);
            }
            // Counter-clockwise now; a point on an edge belongs to it when
            // the edge runs up, or exactly left.
            let owns = |u: [f64; 3], v: [f64; 3], e: f64| {
                let (dx, dy) = (v[0] - u[0], v[1] - u[1]);
                e > 0.0 || (e == 0.0 && (dy > 0.0 || (dy == 0.0 && dx < 0.0)))
            };
            let (wa, wb, wc) = (edge(b, c), edge(c, a), edge(a, b));
            if !(owns(b, c, wa) && owns(c, a, wb) && owns(a, b, wc)) {
                continue;
            }
            let area = area.abs();
            let z = (wa * a[2] + wb * b[2] + wc * c[2]) / area;
            if z > p[2] {
                inside = !inside;
            }
        }
        inside
    }

    /// Closes holes in the surface whose rim has at most `max_edges` edges
    /// and returns how many were closed.
    ///
    /// A hole is a closed chain of edges that belong to one triangle only,
    /// running against the direction of those triangles. Where several
    /// holes meet in one corner, the chains are split there into simple
    /// loops. Each loop is projected onto the plane that fits it best
    /// (normal after Newell) and filled:
    ///
    /// 1. by ear clipping (Meisters' two-ears theorem) when the projected
    ///    loop does not cross itself, which covers concave and moderately
    ///    curved rims without folds;
    /// 2. otherwise by a fan around a new corner at the mean of the loop,
    ///    when every triangle of that fan faces the same way in the plane
    ///    and the loop goes around the mean once (a star-shaped rim);
    /// 3. otherwise, for loops of up to 64 edges, by a fan from the first
    ///    corner, which may fold over but closes the surface.
    ///
    /// Longer loops that fit none of these stay open. Each patch takes the
    /// extruder of a triangle at its rim.
    // ponytail: patches are not checked against the rest of the surface, so
    // a large curved hole can be filled with a patch that cuts through the
    // model. Rims are split at shared corners in the order of their
    // vertex indices, not by the angle between the edges there.
    pub fn close_holes(&mut self, max_edges: usize) -> usize {
        let mut edges: Vec<(u32, u32, u32, u32, usize)> =
            Vec::with_capacity(3 * self.triangles.len());
        for (i, t) in self.triangles.iter().enumerate() {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                edges.push((a.min(b), a.max(b), a, b, i));
            }
        }
        edges.sort_unstable();
        // Edges of the rims, each from b to a of its triangle's a → b,
        // sorted by where they start.
        let mut rim_edges: Vec<(u32, u32, usize)> = edges
            .chunk_by(|x, y| (x.0, x.1) == (y.0, y.1))
            .filter_map(|users| match users {
                [(_, _, a, b, i)] => Some((*b, *a, *i)),
                _ => None,
            })
            .collect();
        drop(edges);
        if rim_edges.is_empty() {
            return 0;
        }
        rim_edges.sort_unstable();
        let vertices = self.vertices.len();
        let mut first = vec![0usize; vertices + 1];
        for &(from, ..) in &rim_edges {
            first[from as usize + 1] += 1;
        }
        for v in 0..vertices {
            first[v + 1] += first[v];
        }
        // The next edge out of each corner not yet walked.
        let mut cursor: Vec<usize> = first[..vertices].to_vec();
        let mut used = vec![false; rim_edges.len()];
        let mut next_edge = |v: u32, used: &mut Vec<bool>| {
            let v = v as usize;
            while cursor[v] < first[v + 1] && used[cursor[v]] {
                cursor[v] += 1;
            }
            (cursor[v] < first[v + 1]).then(|| {
                used[cursor[v]] = true;
                cursor[v]
            })
        };

        // Walk the rims; whenever the walk comes back to a corner it has
        // passed, the part since then is a loop.
        let mut loops: Vec<(Vec<u32>, usize)> = Vec::new();
        let mut slot = vec![u32::MAX; vertices];
        for start in 0..rim_edges.len() {
            if used[start] {
                continue;
            }
            used[start] = true;
            let (from, to, triangle) = rim_edges[start];
            let mut path = vec![from];
            let mut by = vec![triangle];
            slot[from as usize] = 0;
            let mut current = to;
            loop {
                let at = slot[current as usize];
                if at == u32::MAX {
                    slot[current as usize] = path.len() as u32;
                    path.push(current);
                } else {
                    let at = at as usize;
                    for &v in &path[at + 1..] {
                        slot[v as usize] = u32::MAX;
                    }
                    loops.push((path[at..].to_vec(), by[at]));
                    path.truncate(at + 1);
                    by.truncate(at);
                }
                let Some(e) = next_edge(current, &mut used) else {
                    for &v in &path {
                        slot[v as usize] = u32::MAX;
                    }
                    break;
                };
                by.push(rim_edges[e].2);
                current = rim_edges[e].1;
            }
        }

        let mut budget = HOLES_BUDGET;
        let mut closed = 0;
        for (rim, neighbour) in loops {
            if rim.len() < 3 || rim.len() > max_edges {
                continue;
            }
            let first = self.triangles.len();
            let Some(fill) = self.fill(&rim, &mut budget) else {
                continue;
            };
            self.triangles.extend(fill);
            let extruder = self.extruder(neighbour);
            self.set_extruder(first, extruder);
            closed += 1;
        }
        closed
    }

    /// Triangles that fill the closed loop `rim` and keep its direction,
    /// or `None` if it cannot be filled well (see [`Mesh::close_holes`]).
    /// May add a corner at the mean of the loop.
    fn fill(&mut self, rim: &[u32], budget: &mut usize) -> Option<Vec<[u32; 3]>> {
        let n = rim.len();
        let fan =
            || -> Vec<[u32; 3]> { rim[1..].windows(2).map(|w| [rim[0], w[0], w[1]]).collect() };
        let small = (n <= SMALL_HOLE_EDGES).then(fan);
        let origin = self.vertices[rim[0] as usize];
        let at: Vec<[f64; 3]> = rim
            .iter()
            .map(|&i| sub(self.vertices[i as usize], origin))
            .collect();
        let mut normal = [0.0; 3];
        for (k, a) in at.iter().enumerate() {
            let b = at[(k + 1) % n];
            normal[0] += (a[1] - b[1]) * (a[2] + b[2]);
            normal[1] += (a[2] - b[2]) * (a[0] + b[0]);
            normal[2] += (a[0] - b[0]) * (a[1] + b[1]);
        }
        if dot(normal, normal) == 0.0 {
            return small;
        }
        // Two axes in the plane with u × v pointing along the normal, so a
        // rim that runs around the normal counter-clockwise does so in 2D.
        let mut axis = [0.0; 3];
        let smallest = (0..3).fold(0, |m, a| {
            if normal[a].abs() < normal[m].abs() {
                a
            } else {
                m
            }
        });
        axis[smallest] = 1.0;
        let u = cross(normal, axis);
        let v = cross(normal, u);
        let flat: Vec<[f64; 2]> = at.iter().map(|&p| [dot(p, u), dot(p, v)]).collect();

        let mut allowance = (*budget).min(RIM_BUDGET);
        let before = allowance;
        let ears = if simple(&flat, &mut allowance) {
            ear_clip(&flat, &mut allowance)
        } else {
            None
        };
        *budget -= before - allowance;
        if let Some(ears) = ears {
            return Some(ears.iter().map(|t| t.map(|k| rim[k])).collect());
        }

        // A fan around the mean of the rim.
        let mut sum = [0.0; 3];
        for p in &at {
            for axis in 0..3 {
                sum[axis] += p[axis];
            }
        }
        let centre = sum.map(|c| c / n as f64);
        let c = [dot(centre, u), dot(centre, v)];
        let turns = (0..n).all(|k| turn(c, flat[k], flat[(k + 1) % n]) > 0.0);
        // Going around the centre once: a ray from it to the right is
        // crossed exactly once (every step turns the same way, so every
        // crossing goes the same way).
        let crossings = (0..n)
            .filter(|&k| {
                let (a, b) = (flat[k], flat[(k + 1) % n]);
                (a[1] > c[1]) != (b[1] > c[1]) && {
                    let x = a[0] + (c[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
                    x > c[0]
                }
            })
            .count();
        if turns && crossings == 1 {
            let middle = self.vertices.len() as u32;
            self.vertices.push([
                origin[0] + centre[0],
                origin[1] + centre[1],
                origin[2] + centre[2],
            ]);
            return Some((0..n).map(|k| [middle, rim[k], rim[(k + 1) % n]]).collect());
        }
        small
    }
}

/// How far `c` lies to the left of the line from `a` to `b` (twice the
/// signed area of the triangle).
fn turn(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// Whether the closed polygon `flat` has no two edges that touch, other
/// than neighbours at their shared corner. `false` also when the check
/// would cost more than `budget` pair tests.
fn simple(flat: &[[f64; 2]], budget: &mut usize) -> bool {
    let n = flat.len();
    let segment = |k: usize| (flat[k], flat[(k + 1) % n]);
    let mut order: Vec<usize> = (0..n).collect();
    let left = |k: usize| flat[k][0].min(flat[(k + 1) % n][0]);
    order.sort_by(|&a, &b| left(a).total_cmp(&left(b)).then(a.cmp(&b)));
    for (index, &i) in order.iter().enumerate() {
        let (p, q) = segment(i);
        let right = p[0].max(q[0]);
        for &j in &order[index + 1..] {
            if left(j) > right {
                break;
            }
            if *budget == 0 {
                return false;
            }
            *budget -= 1;
            if j == (i + 1) % n || i == (j + 1) % n {
                continue;
            }
            let (r, s) = segment(j);
            if touches(p, q, r, s) {
                return false;
            }
        }
    }
    true
}

/// Whether the segments from `p` to `q` and from `r` to `s` share a point.
fn touches(p: [f64; 2], q: [f64; 2], r: [f64; 2], s: [f64; 2]) -> bool {
    let (d1, d2) = (turn(r, s, p), turn(r, s, q));
    let (d3, d4) = (turn(p, q, r), turn(p, q, s));
    let apart = |x: f64, y: f64| (x > 0.0 && y > 0.0) || (x < 0.0 && y < 0.0);
    if apart(d1, d2) || apart(d3, d4) {
        return false;
    }
    if d1 != 0.0 || d2 != 0.0 || d3 != 0.0 || d4 != 0.0 {
        return true;
    }
    // All four on one line: they touch when their extents overlap.
    let overlap = |axis: usize| {
        p[axis].min(q[axis]) <= r[axis].max(s[axis]) && r[axis].min(s[axis]) <= p[axis].max(q[axis])
    };
    overlap(0) && overlap(1)
}

/// Triangulates the counter-clockwise polygon `flat` by ear clipping and
/// returns the corners of the triangles as indices into it, or `None` if it
/// gets stuck without an ear (a polygon that is not simple or has corners
/// on one line) or would cost more than `budget` point tests.
fn ear_clip(flat: &[[f64; 2]], budget: &mut usize) -> Option<Vec<[usize; 3]>> {
    let n = flat.len();
    let t = |a: usize, b: usize, c: usize| turn(flat[a], flat[b], flat[c]);
    let mut prev: Vec<usize> = (0..n).map(|i| (i + n - 1) % n).collect();
    let mut next: Vec<usize> = (0..n).map(|i| (i + 1) % n).collect();
    let mut triangles = Vec::with_capacity(n - 2);
    let (mut left, mut i, mut misses) = (n, 0, 0);
    while left > 3 {
        let (a, c) = (prev[i], next[i]);
        // Convex, and no other corner inside or on the triangle.
        let mut ear = t(a, i, c) > 0.0;
        let mut p = next[c];
        while ear && p != a {
            *budget = budget.checked_sub(1)?;
            ear = t(a, i, p) < 0.0 || t(i, c, p) < 0.0 || t(c, a, p) < 0.0;
            p = next[p];
        }
        if ear {
            triangles.push([a, i, c]);
            next[a] = c;
            prev[c] = a;
            left -= 1;
            misses = 0;
            i = a;
        } else {
            *budget = budget.checked_sub(1)?;
            i = next[i];
            misses += 1;
            if misses > left {
                return None;
            }
        }
    }
    let (a, c) = (prev[i], next[i]);
    (t(a, i, c) > 0.0).then(|| {
        triangles.push([a, i, c]);
        triangles
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn duplicates_are_removed_and_opposite_twins_cancel() {
        let cube = Mesh::cuboid(2.0, 3.0, 4.0);
        assert_eq!(cube.clone().remove_duplicates(), 0);

        // Two copies of a triangle facing the same way, one rotated.
        let mut doubled = cube.clone();
        doubled.triangles.push([2, 1, 0]);
        doubled.triangles.push([1, 0, 2]);
        doubled.set_extruder(12, 1);
        assert_eq!(doubled.remove_duplicates(), 2);
        assert_eq!(doubled.triangles, cube.triangles);
        assert_eq!(doubled.extruders, vec![0; 12]);

        // Two boxes touching along a face, written as one surface with the
        // shared face twice, once each way: the face goes and the boxes
        // become one.
        let mut a = Mesh::cuboid(1.0, 1.0, 1.0);
        let mut b = Mesh::cuboid(1.0, 1.0, 1.0);
        b.translate([0.0, 0.0, 1.0]);
        a.merge(&b);
        let mut joined = Mesh::from_triangle_soup(&a.to_triangle_soup());
        assert_eq!(joined.remove_duplicates(), 4);
        assert_eq!(joined.edge_defects(), (0, 0));
        assert!((joined.signed_volume() - 2.0).abs() < 1e-12);
    }

    /// A cube whose top is a grid of `n` × `n` squares, with the top
    /// missing: a hole with a rim of 4 `n` edges.
    fn cube_with_open_top(n: usize) -> Mesh {
        let size = 10.0;
        let step = size / n as f64;
        let mut mesh = Mesh::default();
        let mut ring = Vec::new();
        // The rim of the top, counter-clockwise from above, at z = size,
        // and the same at z = 0.
        for side in 0..4 {
            for k in 0..n {
                let s = k as f64 * step;
                ring.push(match side {
                    0 => [s, 0.0],
                    1 => [size, s],
                    2 => [size - s, size],
                    _ => [0.0, size - s],
                });
            }
        }
        let m = ring.len() as u32;
        for p in &ring {
            mesh.vertices.push([p[0], p[1], 0.0]);
        }
        for p in &ring {
            mesh.vertices.push([p[0], p[1], size]);
        }
        for k in 0..m {
            let j = (k + 1) % m;
            mesh.triangles.push([k, j, m + j]);
            mesh.triangles.push([k, m + j, m + k]);
        }
        // The bottom, as a fan around its middle.
        mesh.vertices.push([size / 2.0, size / 2.0, 0.0]);
        let middle = 2 * m;
        for k in 0..m {
            mesh.triangles.push([middle, (k + 1) % m, k]);
        }
        mesh
    }

    #[test]
    fn a_hole_with_a_hundred_edges_is_closed() {
        let mut mesh = cube_with_open_top(25);
        assert_eq!(mesh.open_edge_count(), 100);
        let repair = mesh.repair(1e-6);
        assert_eq!(repair.holes, 1);
        assert!(repair.is_clean());
        assert_eq!(mesh.edge_defects(), (0, 0));
        assert!((mesh.signed_volume() - 1000.0).abs() < 1e-9);
        let mut diagnostics = Diagnostics::new();
        repair.report(&mut diagnostics);
        assert_eq!(diagnostics.codes(), ["mesh.repaired"]);
        assert_eq!(diagnostics.as_slice()[0].context["holes"], "1");
    }

    #[test]
    fn a_repeated_triangle_at_the_rim_of_a_hole_goes_alone() {
        // The second triangle has an edge on the rim of the open top; its
        // copy shares that edge with nothing else and must not be taken for
        // a neighbour, turned and then cancelled with the original.
        let mut mesh = cube_with_open_top(4);
        let rim = mesh.triangles[1];
        mesh.triangles.push(rim);
        let repair = mesh.repair(1e-6);
        assert_eq!(repair.turned, 0);
        assert_eq!(repair.duplicates, 1);
        assert_eq!(repair.holes, 1);
        assert!(repair.is_clean());
        assert!(mesh.triangles.contains(&rim));
        assert!((mesh.signed_volume() - 1000.0).abs() < 1e-9);
    }

    #[test]
    fn a_curved_rim_is_filled_without_folds() {
        // A sphere with a cap cut off at a slant along its triangles: a
        // zig-zag rim of more than 64 edges that is not flat.
        let mut mesh = crate::primitives::sphere(10.0, 96);
        let vertices = mesh.vertices.clone();
        let below = |i: u32| {
            let v = vertices[i as usize];
            v[2] + 0.5 * v[0] < 15.0
        };
        mesh.triangles.retain(|t| t.iter().all(|&i| below(i)));
        let kept = mesh.triangles.len();
        assert!(mesh.open_edge_count() > 64, "{}", mesh.open_edge_count());
        assert_eq!(mesh.repair(1e-6).holes, 1);
        assert_eq!(mesh.edge_defects(), (0, 0));
        assert!(mesh.signed_volume() > 0.0);
        // Every triangle of the patch faces away from the sphere.
        for t in &mesh.triangles[kept..] {
            let [a, b, c] = t.map(|i| mesh.vertices[i as usize]);
            assert!(
                dot(cross(sub(b, a), sub(c, a)), [0.5, 0.0, 1.0]) > 0.0,
                "{t:?}"
            );
        }
    }

    #[test]
    fn long_rims_that_cross_themselves_stay_open() {
        // A tube without ends and with a strip of its side missing: the
        // one rim runs round both ends and folds over itself in every
        // plane.
        let mut mesh = crate::primitives::cylinder(10.0, 5.0, 100);
        let vertices = mesh.vertices.clone();
        mesh.triangles.retain(|t| {
            let corners = t.map(|i| vertices[i as usize]);
            let flat = corners.iter().all(|c| c[2] == corners[0][2]);
            let strip = corners.iter().all(|c| c[0] > 9.97);
            !flat && !strip
        });
        let open = mesh.open_edge_count();
        let repair = mesh.repair(1e-6);
        assert_eq!(repair.holes, 0);
        assert_eq!(repair.open_edges, open);
        let mut diagnostics = Diagnostics::new();
        repair.report(&mut diagnostics);
        assert_eq!(diagnostics.codes(), ["mesh.not_repairable"]);
    }

    #[test]
    fn rims_meeting_in_a_corner_are_split() {
        // Two triangles of a cube's top and side missing that share only
        // corner 6: two holes meeting there.
        let cube = Mesh::cuboid(2.0, 3.0, 4.0);
        let mut mesh = cube.clone();
        mesh.triangles
            .retain(|t| *t != [4, 6, 7] && *t != [1, 2, 6]);
        assert_eq!(mesh.close_holes(64), 2);
        assert_eq!(mesh.edge_defects(), (0, 0));
        assert!((mesh.signed_volume() - 24.0).abs() < 1e-9);
    }

    #[test]
    fn a_rim_too_long_for_ear_clipping_gets_a_fan_around_its_middle() {
        let n = 200;
        let mut mesh = Mesh {
            vertices: (0..n)
                .map(|k| {
                    let a = 2.0 * std::f64::consts::PI * k as f64 / n as f64;
                    [10.0 * a.cos(), 10.0 * a.sin(), 0.0]
                })
                .collect(),
            ..Mesh::default()
        };
        let rim: Vec<u32> = (0..n as u32).collect();
        // Plenty of budget: ear clipping.
        let fill = mesh.fill(&rim, &mut HOLES_BUDGET.clone()).unwrap();
        assert_eq!((fill.len(), mesh.vertices.len()), (n - 2, n));
        // Almost none: a fan around a new corner in the middle.
        let fill = mesh.fill(&rim, &mut 10).unwrap();
        assert_eq!((fill.len(), mesh.vertices.len()), (n, n + 1));
        assert!(mesh.vertices[n].iter().all(|c| c.abs() < 1e-9));
        for t in &fill {
            let [a, b, c] = t.map(|i| mesh.vertices[i as usize]);
            assert!(cross(sub(b, a), sub(c, a))[2] > 0.0);
        }
    }

    #[test]
    fn flipped_triangles_of_a_cube_are_turned_back() {
        let cube = Mesh::cuboid(2.0, 3.0, 4.0);
        let mut mesh = cube.clone();
        for i in [0, 5, 6, 11] {
            mesh.triangles[i].swap(1, 2);
        }
        let repair = mesh.repair(1e-6);
        assert_eq!(repair.turned, 4);
        assert!(repair.is_clean());
        assert_eq!(mesh, cube);

        // Turned around as a whole, it is turned back as a shell.
        let mut inside_out = cube.clone();
        inside_out.flip();
        let repair = inside_out.repair(1e-6);
        assert_eq!((repair.turned, repair.shells), (0, 1));
        assert_eq!(inside_out, cube);
        let mut diagnostics = Diagnostics::new();
        repair.report(&mut diagnostics);
        assert_eq!(diagnostics.codes(), ["mesh.repaired"]);
        assert_eq!(diagnostics.as_slice()[0].context["shells"], "1");
    }

    #[test]
    fn cavities_keep_their_way_and_follow_their_shell() {
        // A hollow box: the cavity faces inward and stays so.
        let mut outer = Mesh::cuboid(10.0, 10.0, 10.0);
        let mut cavity = Mesh::cuboid(4.0, 4.0, 4.0);
        cavity.translate([3.0, 3.0, 3.0]);
        cavity.flip();
        outer.merge(&cavity);
        let hollow = outer;
        assert!((hollow.signed_volume() - 936.0).abs() < 1e-9);
        assert_eq!(hollow.clone().orient_outward(), 0);

        // All of it inside out: both are turned.
        let mut inverted = hollow.clone();
        inverted.flip();
        assert_eq!(inverted.orient_outward(), 2);
        assert_eq!(inverted, hollow);

        // A separate inside-out box next to it is turned on its own, and
        // so is one overlapping it, which is not inside.
        let mut beside = Mesh::cuboid(2.0, 2.0, 2.0);
        beside.translate([20.0, 0.0, 0.0]);
        beside.flip();
        let mut overlapping = Mesh::cuboid(4.0, 4.0, 4.0);
        overlapping.translate([8.0, 8.0, 8.0]);
        overlapping.flip();
        let mut mesh = hollow.clone();
        mesh.merge(&beside);
        mesh.merge(&overlapping);
        assert_eq!(mesh.orient_outward(), 2);
        assert!((mesh.signed_volume() - (936.0 + 8.0 + 64.0)).abs() < 1e-9);
    }

    #[test]
    fn touching_cubes_are_not_repairable() {
        // Two cubes sharing an edge: closed, but four triangles meet there.
        let cube = Mesh::cuboid(1.0, 2.0, 3.0);
        let mut touching = cube.clone();
        let mut second = cube.clone();
        second.translate([1.0, 2.0, 0.0]);
        touching.merge(&second);
        let mut touching = Mesh::from_triangle_soup(&touching.to_triangle_soup());
        let repair = touching.repair(1e-6);
        assert!(!repair.changed());
        assert_eq!((repair.open_edges, repair.non_manifold_edges), (0, 1));
        let mut diagnostics = Diagnostics::new();
        repair.report(&mut diagnostics);
        assert_eq!(diagnostics.codes(), ["mesh.not_repairable"]);
        let warning = &diagnostics.as_slice()[0];
        assert_eq!(warning.context["non_manifold_edges"], "1");
        assert_eq!(warning.context["open_edges"], "0");
        assert!(warning.message.contains("mesh editor"));

        // A fan of triangles around one corner that goes round twice,
        // lying over itself: its rim of 70 edges crosses itself in every
        // plane, so no patch closes it.
        let n = 70;
        let mut vertices = vec![[0.0, 0.0, 0.0]];
        for k in 0..n {
            let a = 4.0 * std::f64::consts::PI * k as f64 / n as f64;
            let r = 10.0 + k as f64 / 10.0;
            vertices.push([r * a.cos(), r * a.sin(), 0.0]);
        }
        let mut fan = Mesh {
            vertices,
            triangles: (0..n).map(|k| [0, k + 1, (k + 1) % n + 1]).collect(),
            extruders: Vec::new(),
        };
        let repair = fan.repair(1e-6);
        assert_eq!((repair.holes, repair.open_edges), (0, 70));
        let mut diagnostics = Diagnostics::new();
        repair.report(&mut diagnostics);
        assert_eq!(diagnostics.codes(), ["mesh.not_repairable"]);
        assert_eq!(diagnostics.as_slice()[0].context["open_edges"], "70");

        // A clean mesh reports nothing.
        let mut diagnostics = Diagnostics::new();
        cube.clone().repair(1e-6).report(&mut diagnostics);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn repair_is_fast_on_large_meshes() {
        // A sphere of 200 000 triangles with a band of holes: well under a
        // second even unoptimised.
        let mut mesh = crate::primitives::sphere(50.0, 320);
        let count = mesh.triangles.len();
        mesh.triangles = mesh
            .triangles
            .iter()
            .enumerate()
            .filter(|(i, _)| i % 997 != 0)
            .map(|(_, t)| *t)
            .collect();
        let started = std::time::Instant::now();
        let repair = mesh.repair(1e-6);
        assert!(count > 100_000, "{count}");
        assert!(repair.holes > 100);
        assert!(repair.is_clean());
        assert!(started.elapsed().as_secs_f64() < 10.0);
    }
}
