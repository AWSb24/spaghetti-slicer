//! Compares pure-Rust polygon libraries on robustness and speed.
//! See docs/decisions/0001-polygon-library.md. Run with `cargo run --release`.

use std::f64::consts::PI;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Instant;

type C = Vec<[f64; 2]>;
type Cs = Vec<C>;

#[derive(Clone, Copy)]
enum Op {
    Union,
    Inter,
    Diff,
}

const TOL: f64 = 0.002; // max chord deviation of round joins, mm

trait Lib {
    fn name(&self) -> &'static str;
    fn clean(&self, a: &Cs) -> Result<Cs, String>;
    fn boolean(&self, a: &Cs, b: &Cs, op: Op) -> Result<Cs, String>;
    fn offset(&self, a: &Cs, d: f64) -> Result<Cs, String>;
}

// ---------- i_overlay ----------
mod io {
    pub use i_overlay::core::fill_rule::FillRule;
    pub use i_overlay::core::overlay_rule::OverlayRule;
    pub use i_overlay::float::scale::FixedScaleFloatOverlay;
    pub use i_overlay::float::single::SingleFloatOverlay;
    pub use i_overlay::mesh::float::outline::offset::OutlineOffset;
    pub use i_overlay::mesh::float::style::{LineJoin, OutlineStyle};
}
#[derive(Clone, Copy)]
enum IoMode {
    Adaptive,
    Fixed32(f64),
    Fixed64(f64),
}
struct IOverlay(IoMode, &'static str);
fn flat(s: Vec<Vec<C>>) -> Cs {
    s.into_iter().flatten().collect()
}
impl IOverlay {
    fn run(&self, a: &Cs, b: &Cs, rule: io::OverlayRule, fill: io::FillRule) -> Result<Cs, String> {
        use io::*;
        Ok(flat(match self.0 {
            IoMode::Adaptive => a.overlay(b, rule, fill),
            IoMode::Fixed32(s) => a.overlay_with_fixed_scale(b, rule, fill, s).map_err(|e| format!("{e:?}"))?,
            IoMode::Fixed64(s) => {
                a.overlay_with_fixed_scale_as::<i64>(b, rule, fill, s).map_err(|e| format!("{e:?}"))?
            }
        }))
    }
}
impl Lib for IOverlay {
    fn name(&self) -> &'static str {
        self.1
    }
    fn clean(&self, a: &Cs) -> Result<Cs, String> {
        if a.is_empty() {
            return Ok(vec![]);
        }
        self.run(a, &vec![], io::OverlayRule::Subject, io::FillRule::EvenOdd)
    }
    fn boolean(&self, a: &Cs, b: &Cs, op: Op) -> Result<Cs, String> {
        match (a.is_empty(), b.is_empty(), op) {
            (true, _, Op::Union) => return Ok(b.clone()),
            (_, true, Op::Union | Op::Diff) => return Ok(a.clone()),
            (true, _, _) | (_, true, _) => return Ok(vec![]),
            _ => {}
        }
        let rule = match op {
            Op::Union => io::OverlayRule::Union,
            Op::Inter => io::OverlayRule::Intersect,
            Op::Diff => io::OverlayRule::Difference,
        };
        self.run(a, b, rule, io::FillRule::NonZero)
    }
    fn offset(&self, a: &Cs, d: f64) -> Result<Cs, String> {
        use io::*;
        if a.is_empty() {
            return Ok(vec![]);
        }
        let step = (2.0 * (1.0 - TOL / d.abs()).clamp(-1.0, 1.0).acos()).max(0.0315);
        let style = OutlineStyle::new(d).line_join(LineJoin::Round(step));
        Ok(flat(match self.0 {
            IoMode::Adaptive => a.outline(&style),
            IoMode::Fixed32(s) => a.outline_fixed_scale(&style, s).map_err(|e| format!("{e:?}"))?,
            IoMode::Fixed64(s) => a.outline_fixed_scale_as::<i64>(&style, s).map_err(|e| format!("{e:?}"))?,
        }))
    }
}

// ---------- Clipper2 ports ----------
const S: f64 = 1_000_000.0;

struct ClipperRust;
impl ClipperRust {
    fn to(a: &Cs) -> clipper2_rust::Paths64 {
        a.iter()
            .map(|c| c.iter().map(|p| clipper2_rust::Point64::new((p[0] * S).round() as i64, (p[1] * S).round() as i64)).collect())
            .collect()
    }
    fn from(a: clipper2_rust::Paths64) -> Cs {
        a.iter().map(|c| c.iter().map(|p| [p.x as f64 / S, p.y as f64 / S]).collect()).collect()
    }
}
impl Lib for ClipperRust {
    fn name(&self) -> &'static str {
        "clipper2-rust"
    }
    fn clean(&self, a: &Cs) -> Result<Cs, String> {
        Ok(Self::from(clipper2_rust::union_subjects_64(&Self::to(a), clipper2_rust::FillRule::EvenOdd)))
    }
    fn boolean(&self, a: &Cs, b: &Cs, op: Op) -> Result<Cs, String> {
        use clipper2_rust::*;
        let ct = match op {
            Op::Union => ClipType::Union,
            Op::Inter => ClipType::Intersection,
            Op::Diff => ClipType::Difference,
        };
        Ok(Self::from(boolean_op_64(ct, FillRule::NonZero, &Self::to(a), &Self::to(b))))
    }
    fn offset(&self, a: &Cs, d: f64) -> Result<Cs, String> {
        use clipper2_rust::*;
        Ok(Self::from(inflate_paths_64(&Self::to(a), d * S, JoinType::Round, EndType::Polygon, 2.0, TOL * S)))
    }
}

struct ClipperRs;
impl ClipperRs {
    fn to(a: &Cs) -> clipper2_rs::Paths64 {
        a.iter()
            .map(|c| c.iter().map(|p| clipper2_rs::Point64::new((p[0] * S).round() as i64, (p[1] * S).round() as i64)).collect())
            .collect()
    }
    fn from(a: clipper2_rs::Paths64) -> Cs {
        a.iter().map(|c| c.iter().map(|p| [p.x as f64 / S, p.y as f64 / S]).collect()).collect()
    }
}
impl Lib for ClipperRs {
    fn name(&self) -> &'static str {
        "clipper2-rs"
    }
    fn clean(&self, a: &Cs) -> Result<Cs, String> {
        clipper2_rs::union_self(&Self::to(a), clipper2_rs::FillRule::EvenOdd).map(Self::from).map_err(|e| format!("{e:?}"))
    }
    fn boolean(&self, a: &Cs, b: &Cs, op: Op) -> Result<Cs, String> {
        use clipper2_rs::*;
        let ct = match op {
            Op::Union => ClipType::Union,
            Op::Inter => ClipType::Intersection,
            Op::Diff => ClipType::Difference,
        };
        boolean_op(ct, FillRule::NonZero, &Self::to(a), &Self::to(b)).map(Self::from).map_err(|e| format!("{e:?}"))
    }
    fn offset(&self, a: &Cs, d: f64) -> Result<Cs, String> {
        use clipper2_rs::*;
        inflate_paths(&Self::to(a), d * S, JoinType::Round, EndType::Polygon, 2.0, TOL * S)
            .map(Self::from)
            .map_err(|e| format!("{e:?}"))
    }
}

// ---------- helpers ----------
fn signed(c: &C) -> f64 {
    let n = c.len();
    (0..n).map(|i| c[i][0] * c[(i + 1) % n][1] - c[(i + 1) % n][0] * c[i][1]).sum::<f64>() / 2.0
}
fn area(a: &Cs) -> f64 {
    a.iter().map(signed).sum::<f64>().abs()
}
fn verts(a: &Cs) -> usize {
    a.iter().map(Vec::len).sum()
}
fn rect(x: f64, y: f64, w: f64, h: f64) -> C {
    vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]]
}
fn circle(cx: f64, cy: f64, r: f64, n: usize, phase: f64) -> C {
    (0..n).map(|i| { let a = phase + 2.0 * PI * i as f64 / n as f64; [cx + r * a.cos(), cy + r * a.sin()] }).collect()
}
fn gear(cx: f64, cy: f64, r: f64, teeth: usize, depth: f64) -> C {
    (0..teeth * 4)
        .map(|i| { let a = 2.0 * PI * i as f64 / (teeth * 4) as f64; let rr = if i % 4 < 2 { r } else { r - depth }; [cx + rr * a.cos(), cy + rr * a.sin()] })
        .collect()
}
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
    }
}

type Case = (&'static str, f64, f64, Box<dyn Fn(&dyn Lib) -> Result<Cs, String>>);

fn cases() -> Vec<Case> {
    // (name, expected area, absolute tolerance, operation)
    let c = |f: fn(&dyn Lib) -> Result<Cs, String>| -> Box<dyn Fn(&dyn Lib) -> Result<Cs, String>> { Box::new(f) };
    vec![
        ("square shrink 1", 324.0, 0.01, c(|l| l.offset(&l.clean(&vec![rect(0.0, 0.0, 20.0, 20.0)])?, -1.0))),
        ("square grow 1 (round)", 400.0 + 80.0 + PI, 0.05, c(|l| l.offset(&l.clean(&vec![rect(0.0, 0.0, 20.0, 20.0)])?, 1.0))),
        ("square collapse", 0.0, 1e-6, c(|l| l.offset(&l.clean(&vec![rect(0.0, 0.0, 20.0, 20.0)])?, -10.5))),
        ("exact collapse (-10)", 0.0, 1e-6, c(|l| l.offset(&l.clean(&vec![rect(0.0, 0.0, 20.0, 20.0)])?, -10.0))),
        ("thin strip 0.4 shrink .225", 0.0, 1e-6, c(|l| l.offset(&l.clean(&vec![rect(0.0, 0.0, 20.0, 0.4)])?, -0.225))),
        ("strip 0.5 shrink .225", 19.55 * 0.05, 1e-3, c(|l| l.offset(&l.clean(&vec![rect(0.0, 0.0, 20.0, 0.5)])?, -0.225))),
        ("hole, shrink 1", 324.0 - (100.0 + 40.0 + PI), 0.05, c(|l| l.offset(&l.clean(&vec![rect(0.0, 0.0, 20.0, 20.0), rect(5.0, 5.0, 10.0, 10.0)])?, -1.0))),
        ("hole swallows part", 0.0, 1e-6, c(|l| l.offset(&l.clean(&vec![rect(0.0, 0.0, 20.0, 20.0), rect(1.0, 1.0, 18.0, 18.0)])?, -0.6))),
        ("shared edge union", 200.0, 1e-4, c(|l| l.boolean(&l.clean(&vec![rect(0.0, 0.0, 10.0, 10.0)])?, &l.clean(&vec![rect(10.0, 0.0, 10.0, 10.0)])?, Op::Union))),
        ("corner touch union", 200.0, 1e-4, c(|l| l.boolean(&l.clean(&vec![rect(0.0, 0.0, 10.0, 10.0)])?, &l.clean(&vec![rect(10.0, 10.0, 10.0, 10.0)])?, Op::Union))),
        ("identical inter", 100.0, 1e-4, c(|l| { let a = l.clean(&vec![rect(0.0, 0.0, 10.0, 10.0)])?; l.boolean(&a, &a, Op::Inter) })),
        ("identical diff", 0.0, 1e-6, c(|l| { let a = l.clean(&vec![rect(0.0, 0.0, 10.0, 10.0)])?; l.boolean(&a, &a, Op::Diff) })),
        ("bow-tie even-odd", 50.0, 1e-4, c(|l| l.clean(&vec![vec![[0.0, 0.0], [10.0, 10.0], [10.0, 0.0], [0.0, 10.0]]]))),
        ("dup + collinear pts", 324.0, 0.01, c(|l| l.offset(&l.clean(&vec![vec![[0.0, 0.0], [0.0, 0.0], [10.0, 0.0], [20.0, 0.0], [20.0, 20.0], [20.0, 20.0], [0.0, 20.0], [0.0, 10.0]]])?, -1.0))),
        ("zero-width spike", 400.0, 1e-4, c(|l| l.clean(&vec![vec![[0.0, 0.0], [20.0, 0.0], [20.0, 10.0], [30.0, 10.0], [20.0, 10.0], [20.0, 20.0], [0.0, 20.0]]]))),
        ("degenerate (2 pts, 0 area)", 0.0, 1e-6, c(|l| l.clean(&vec![vec![[0.0, 0.0], [5.0, 5.0]], vec![[0.0, 0.0], [5.0, 0.0], [9.0, 0.0]]]))),
        ("overlap 1e-7", 200.0, 1e-4, c(|l| l.boolean(&l.clean(&vec![rect(0.0, 0.0, 10.0, 10.0)])?, &l.clean(&vec![rect(10.0 - 1e-7, 0.0, 10.0, 10.0)])?, Op::Union))),
        ("gap 1e-7 union", 200.0, 1e-4, c(|l| l.boolean(&l.clean(&vec![rect(0.0, 0.0, 10.0, 10.0)])?, &l.clean(&vec![rect(10.0 + 1e-7, 0.0, 10.0, 10.0)])?, Op::Union))),
        ("dumbbell splits (compare only)", f64::NAN, 0.0, c(|l| {
            let d = vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 4.7], [20.0, 4.7], [20.0, 0.0], [30.0, 0.0], [30.0, 10.0], [20.0, 10.0], [20.0, 5.3], [10.0, 5.3], [10.0, 10.0], [0.0, 10.0]]];
            l.offset(&l.clean(&d)?, -0.45)
        })),
        ("shrink then regrow square", 400.0 - (4.0 - PI) * 1.0, 0.05, c(|l| l.offset(&l.offset(&l.clean(&vec![rect(0.0, 0.0, 20.0, 20.0)])?, -1.0)?, 1.0))),
        ("far from origin (x=1900)", 324.0, 0.01, c(|l| l.offset(&l.clean(&vec![rect(1900.0, 1900.0, 20.0, 20.0)])?, -1.0))),
        ("tiny square 0.01", 1e-4, 1e-7, c(|l| l.clean(&vec![rect(100.0, 100.0, 0.01, 0.01)]))),
        ("needle star shrink", f64::NAN, 0.0, c(|l| l.offset(&l.clean(&vec![gear(50.0, 50.0, 40.0, 500, 39.0)])?, -0.05))),
        ("needle star grow", f64::NAN, 0.0, c(|l| l.offset(&l.clean(&vec![gear(50.0, 50.0, 40.0, 500, 39.0)])?, 0.2))),
        ("circle minus circle", PI * (900.0 - 100.0), 2.0, c(|l| l.boolean(&l.clean(&vec![circle(0.0, 0.0, 30.0, 720, 0.0)])?, &l.clean(&vec![circle(0.0, 0.0, 10.0, 720, 0.1)])?, Op::Diff))),
        ("empty inputs", 0.0, 1e-9, c(|l| { let e = l.clean(&vec![])?; let o = l.offset(&e, 1.0)?; l.boolean(&o, &e, Op::Inter) })),
    ]
}

fn guarded<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| Err("PANIC".into()))
}

fn main() {
    std::panic::set_hook(Box::new(|_| {}));
    let libs: Vec<Box<dyn Lib>> = vec![
        Box::new(IOverlay(IoMode::Adaptive, "i_overlay adaptive")),
        Box::new(IOverlay(IoMode::Fixed32(1e5), "i_overlay fix32 1e5")),
        Box::new(IOverlay(IoMode::Fixed64(1e6), "i_overlay fix64 1e6")),
        Box::new(ClipperRust),
        Box::new(ClipperRs),
    ];

    println!("## correctness (area | contours | vertices), FAIL = outside tolerance");
    let mut fails = vec![0usize; libs.len()];
    for (name, expected, tol, f) in cases() {
        print!("{name:28} exp {expected:10.4}");
        let mut areas = vec![];
        for (i, lib) in libs.iter().enumerate() {
            match guarded(|| f(lib.as_ref())) {
                Ok(r) => {
                    let a = area(&r);
                    areas.push(a);
                    let bad = !expected.is_nan() && (a - expected).abs() > tol;
                    if bad {
                        fails[i] += 1;
                    }
                    print!(" | {a:10.4} {:2}c {:5}v{}", r.len(), verts(&r), if bad { " FAIL" } else { "" });
                }
                Err(e) => {
                    fails[i] += 1;
                    print!(" | ERR {e}");
                }
            }
        }
        println!();
    }
    println!("failures per lib: {:?}", libs.iter().map(|l| l.name()).zip(&fails).collect::<Vec<_>>());

    println!("\n## random stress: 400 self-intersecting polygons, clean -> shrink .3 -> grow .3 -> clip");
    let reference: Vec<f64> = {
        let mut rng = Rng(42);
        (0..400).map(|_| { let p: C = (0..10).map(|_| [rng.next() * 50.0, rng.next() * 50.0]).collect(); area(&ClipperRs.clean(&vec![p]).unwrap_or_default()) }).collect()
    };
    for lib in &libs {
        let mut rng = Rng(42);
        let (mut errs, mut max_dev, mut grew, mut total) = (0, 0.0f64, 0, 0.0);
        for reference_area in &reference {
            let p: C = (0..10).map(|_| [rng.next() * 50.0, rng.next() * 50.0]).collect();
            let r = guarded(|| {
                let c = lib.clean(&vec![p.clone()])?;
                let a0 = area(&c);
                let s = lib.offset(&c, -0.3)?;
                let g = lib.offset(&s, 0.3)?;
                let clip = lib.clean(&vec![rect(10.0, 10.0, 30.0, 30.0)])?;
                let k = lib.boolean(&g, &clip, Op::Inter)?;
                Ok((a0, area(&s), area(&g), area(&k)))
            });
            match r {
                Ok((a0, s, g, k)) => {
                    max_dev = max_dev.max((a0 - reference_area).abs());
                    // Opening must never grow a shape, shrinking must never grow it.
                    if s > a0 + 1e-3 || g > a0 + 0.05 || k > g + 1e-3 || !k.is_finite() {
                        grew += 1;
                    }
                    total += k;
                }
                Err(_) => errs += 1,
            }
        }
        println!("{:22} errors {errs:3}  invariant violations {grew:3}  max |clean area - ref| {max_dev:.6}  sum {total:.3}", lib.name());
    }

    println!("\n## speed (release), best of 3");
    let layer_job = |lib: &dyn Lib| -> Result<f64, String> {
        // One layer of a ring-shaped part: 3 wall offsets, infill inset,
        // 9 neighbour intersections, 1 difference, 1 opening.
        let mut acc = 0.0;
        for i in 0..200 {
            let ph = i as f64 * 0.001;
            let s = lib.clean(&vec![circle(110.0, 110.0, 30.0, 256, ph), circle(110.0, 110.0, 10.0, 96, ph)])?;
            for k in 0..3 {
                acc += area(&lib.offset(&s, -(0.225 + 0.45 * k as f64))?);
            }
            let fill = lib.offset(&s, -1.0125)?;
            let mut inner = fill.clone();
            for n in 1..10 {
                let nb = lib.clean(&vec![circle(110.0, 110.0, 30.0 - 0.01 * n as f64, 256, ph + 0.01), circle(110.0, 110.0, 10.0, 96, ph)])?;
                inner = lib.boolean(&inner, &nb, Op::Inter)?;
            }
            let inner = lib.offset(&lib.offset(&inner, -0.45)?, 0.45)?;
            acc += area(&lib.boolean(&fill, &inner, Op::Diff)?);
        }
        Ok(acc)
    };
    let big_job = |lib: &dyn Lib| -> Result<f64, String> {
        let g1 = lib.clean(&vec![gear(110.0, 110.0, 100.0, 5000, 0.3)])?; // 20k vertices
        let g2 = lib.clean(&vec![gear(110.5, 110.2, 100.0, 5000, 0.3)])?;
        let mut acc = 0.0;
        for _ in 0..5 {
            acc += area(&lib.offset(&g1, -0.45)?);
            acc += area(&lib.boolean(&g1, &g2, Op::Inter)?);
            acc += area(&lib.boolean(&g1, &g2, Op::Diff)?);
        }
        Ok(acc)
    };
    for lib in &libs {
        let mut line = format!("{:22}", lib.name());
        for (label, job) in [("200 layers", &layer_job as &dyn Fn(&dyn Lib) -> Result<f64, String>), ("20k-vertex ops", &big_job)] {
            let mut best = f64::INFINITY;
            let mut result = String::new();
            for _ in 0..3 {
                let t = Instant::now();
                let r = guarded(|| job(lib.as_ref()));
                best = best.min(t.elapsed().as_secs_f64() * 1000.0);
                result = match r { Ok(a) => format!("{a:.1}"), Err(e) => e };
            }
            line += &format!("  {label}: {best:8.1} ms (checksum {result})");
        }
        println!("{line}");
    }

    println!("\n## grid stability: same square shrunk by -1 inside different bounding boxes");
    for lib in &libs {
        let mut outs = vec![];
        for far in [0.0, 50.0, 1000.0] {
            let input = vec![rect(100.0, 100.0, 20.0, 20.0), rect(100.0 + far + 30.0, 100.0, 1.0, 1.0)];
            let r = guarded(|| lib.offset(&lib.clean(&input)?, -0.123457));
            outs.push(r.map(|cs| { let mut pts: Vec<[f64; 2]> = cs.into_iter().filter(|c| signed(c).abs() > 10.0).flatten().collect(); pts.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1]))); pts }));
        }
        let same = outs.windows(2).all(|w| w[0] == w[1]);
        let dev = outs.windows(2).map(|w| match (&w[0], &w[1]) {
            (Ok(a), Ok(b)) if a.len() == b.len() => a.iter().zip(b).map(|(p, q)| (p[0] - q[0]).abs().max((p[1] - q[1]).abs())).fold(0.0, f64::max),
            _ => f64::NAN,
        }).fold(0.0, f64::max);
        print!("max deviation {dev:.3e} mm  ");
        let first = outs[0].as_ref().map(|p| format!("{:?}", p.first())).unwrap_or_else(|e| e.clone());
        println!("{:22} identical across bounding boxes: {same}   first vertex {first}", lib.name());
    }
}
