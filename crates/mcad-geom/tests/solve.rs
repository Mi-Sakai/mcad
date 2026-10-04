//! `mcad_geom::solve`（DESIGN.md M13 設計判断1、タスク95）の結合テスト。
//!
//! 設計判断2 の拘束の方程式（点の一致・距離・主値化した角度・平行・円どうしの外接と内接・
//! 直線と円の接線・円弧上の点）を簡易に再現したテスト用の方程式 [`TestEq`] を用意し、
//! 公開 API だけで次を確かめる:
//!
//! - 解析ヤコビアンと中心差分の一致（proptest、[`check_jacobian`]）
//! - 解から遠い初期値（図形の大きさの 100 倍）・接線の内接と外接・角度の ±π 付近・
//!   階数落ち（矛盾しない重複）で、受理したステップの `F` が単調に減り収束すること
//! - 解の無い系が決定的に失敗すること、自由なパラメータが無い未充足系が反復せずに失敗すること
//! - 原点と 1e6 mm 離れた位置で判定が一致すること、代表長さ 1e-3 mm と 1e5 mm の形が
//!   解けること、遠方でも角度型の許容値が緩まないこと、原点で一致する 2 点・座標が 0 の点で
//!   許容値が正のまま NaN が出ないこと
//! - 実際の残差を減らさずに `L` を大きくするだけの経路を収束と判定しないこと
//! - 自由度、最小ノルム性、重み、棄却フック、決定性、入力の検査

use std::cell::Cell;
use std::f64::consts::PI;

use mcad_geom::solve::{
    ATOL_ANG, ATOL_LEN, Equation, LAMBDA_0, LAMBDA_MAX, LAMBDA_MIN, LAMBDA_RETRY, MAX_TRIALS,
    Problem, RTOL, ResidualKind, Solution, SolveError, check_jacobian, degrees_of_freedom,
    is_satisfied, max_scaled_residual, solve,
};
use proptest::prelude::*;

// ---------------------------------------------------------------------------
// テスト用の方程式
// ---------------------------------------------------------------------------

/// 点はパラメータの添字 `p` から始まる 2 つ（`x[p]`, `x[p + 1]`）で表す。
fn dist(x: &[f64], p: usize, q: usize) -> f64 {
    (x[p] - x[q]).hypot(x[p + 1] - x[q + 1])
}

/// ノルムと単位ベクトル。ノルムが `ATOL_LEN` 未満なら方向は `(1, 0)`（設計判断2）。
fn norm_dir(vx: f64, vy: f64) -> (f64, f64, f64) {
    let n = vx.hypot(vy);
    if n < ATOL_LEN {
        (n, 1.0, 0.0)
    } else {
        (n, vx / n, vy / n)
    }
}

/// `(−π, π]` へ主値化する（設計判断2 の角度残差）。
fn wrap_pi(a: f64) -> f64 {
    a.sin().atan2(a.cos())
}

/// 線分 ab の方向から線分 cd の方向へのなす角 `φ = atan2(d1 × d2, d1 · d2)`。
fn angle_between(x: &[f64], a: usize, b: usize, c: usize, d: usize) -> f64 {
    let (d1x, d1y) = (x[b] - x[a], x[b + 1] - x[a + 1]);
    let (d2x, d2y) = (x[d] - x[c], x[d + 1] - x[c + 1]);
    (d1x * d2y - d1y * d2x).atan2(d1x * d2x + d1y * d2y)
}

/// 大きさを数えるエンティティ。
#[derive(Clone, Copy, Debug)]
enum Ent {
    /// 線分（始点・終点の点の添字）。
    Seg(usize, usize),
    /// 円・円弧（半径のパラメータの添字）。
    Round(usize),
}

/// 長さ型の許容値の尺度（設計判断1: `L` は参照するエンティティの大きさと拘束の値の最大、
/// `C` は対象の点の座標の絶対値の最大）。
#[derive(Clone, Debug, Default)]
struct Size {
    ents: Vec<Ent>,
    values: Vec<f64>,
    points: Vec<usize>,
    /// 保存した座標（点の固定の `p₀`）。
    coords: Vec<f64>,
}

/// NaN を捨てない最大（`f64::max` は NaN を黙って捨て、壊れた尺度を隠してしまう）。
fn max_keep_nan(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}

impl Size {
    fn kind(&self, x: &[f64]) -> ResidualKind {
        let mut size = self
            .values
            .iter()
            .fold(0.0, |m, v| max_keep_nan(m, v.abs()));
        for e in &self.ents {
            let s = match *e {
                Ent::Seg(a, b) => dist(x, a, b),
                Ent::Round(r) => x[r].abs(),
            };
            size = max_keep_nan(size, s);
        }
        let mut coord = self
            .coords
            .iter()
            .fold(0.0, |m, v| max_keep_nan(m, v.abs()));
        for &p in &self.points {
            coord = max_keep_nan(coord, x[p].abs().max(x[p + 1].abs()));
        }
        ResidualKind::Length { size, coord }
    }
}

fn size(ents: &[Ent], values: &[f64], points: &[usize]) -> Option<Size> {
    Some(Size {
        ents: ents.to_vec(),
        values: values.to_vec(),
        points: points.to_vec(),
        coords: Vec::new(),
    })
}

#[derive(Clone, Copy, Debug)]
enum Form {
    /// `x[p + axis] − x[q + axis]`（一致の 1 成分、水平・垂直）。
    Coincident { p: usize, q: usize, axis: usize },
    /// `x[p + axis] − value`（点の固定の 1 成分）。
    FixCoord { p: usize, axis: usize, value: f64 },
    /// `‖p − q‖ − d`。
    Distance { p: usize, q: usize, d: f64 },
    /// `atan2(sin(φ − θ), cos(φ − θ))`、`φ` は線分 ab から cd へのなす角。
    Angle {
        a: usize,
        b: usize,
        c: usize,
        d: usize,
        theta: f64,
    },
    /// 単位方向ベクトルの外積。
    Parallel {
        a: usize,
        b: usize,
        c: usize,
        d: usize,
    },
    /// 円どうしの外接 `‖c1 − c2‖ − (r1 + r2)`。
    External {
        c1: usize,
        r1: usize,
        c2: usize,
        r2: usize,
    },
    /// 円どうしの内接 `‖c1 − c2‖ − s(r1 − r2)`。
    Internal {
        c1: usize,
        r1: usize,
        c2: usize,
        r2: usize,
        s: f64,
    },
    /// 直線 ab と円（中心 c・半径 r）の接線: 中心から直線への符号付き距離 − `s·r`
    /// （符号は ab の向きに対して左側が正）。
    LineCircle {
        a: usize,
        b: usize,
        c: usize,
        r: usize,
        s: f64,
    },
    /// 係数を指定した 1 次式 `a·x[p] + b·x[q]`（自由度・階数のテスト用）。
    Linear2 { p: usize, q: usize, a: f64, b: f64 },
    /// 円弧上の点 `c + r(cos t, sin t)`（`t = x[ang]`）と点 q の `axis` 成分の差。
    ArcPoint {
        c: usize,
        r: usize,
        ang: usize,
        q: usize,
        axis: usize,
    },
}

#[derive(Clone, Debug)]
struct TestEq {
    form: Form,
    params: Vec<usize>,
    /// 長さ型の尺度。`None` なら角度型。
    size: Option<Size>,
}

impl TestEq {
    fn new(form: Form, size: Option<Size>) -> Self {
        let params = match form {
            Form::Coincident { p, q, axis } => vec![p + axis, q + axis],
            Form::FixCoord { p, axis, .. } => vec![p + axis],
            Form::Distance { p, q, .. } => vec![p, p + 1, q, q + 1],
            Form::Angle { a, b, c, d, .. } | Form::Parallel { a, b, c, d } => {
                vec![a, a + 1, b, b + 1, c, c + 1, d, d + 1]
            }
            Form::External { c1, r1, c2, r2 } | Form::Internal { c1, r1, c2, r2, .. } => {
                vec![c1, c1 + 1, r1, c2, c2 + 1, r2]
            }
            Form::LineCircle { a, b, c, r, .. } => vec![a, a + 1, b, b + 1, c, c + 1, r],
            Form::Linear2 { p, q, .. } => vec![p, q],
            Form::ArcPoint { c, r, ang, q, axis } => vec![c + axis, r, ang, q + axis],
        };
        Self { form, params, size }
    }
}

impl Equation for TestEq {
    fn params(&self) -> &[usize] {
        &self.params
    }

    fn residual(&self, x: &[f64]) -> f64 {
        match self.form {
            Form::Coincident { p, q, axis } => x[p + axis] - x[q + axis],
            Form::FixCoord { p, axis, value } => x[p + axis] - value,
            Form::Distance { p, q, d } => dist(x, p, q) - d,
            Form::Angle { a, b, c, d, theta } => wrap_pi(angle_between(x, a, b, c, d) - theta),
            Form::Parallel { a, b, c, d } => {
                let (_, u1x, u1y) = norm_dir(x[b] - x[a], x[b + 1] - x[a + 1]);
                let (_, u2x, u2y) = norm_dir(x[d] - x[c], x[d + 1] - x[c + 1]);
                u1x * u2y - u1y * u2x
            }
            Form::External { c1, r1, c2, r2 } => dist(x, c1, c2) - (x[r1] + x[r2]),
            Form::Internal { c1, r1, c2, r2, s } => dist(x, c1, c2) - s * (x[r1] - x[r2]),
            Form::LineCircle { a, b, c, r, s } => {
                let (dx, dy) = (x[b] - x[a], x[b + 1] - x[a + 1]);
                let (wx, wy) = (x[c] - x[a], x[c + 1] - x[a + 1]);
                (dx * wy - dy * wx) / dx.hypot(dy) - s * x[r]
            }
            Form::Linear2 { p, q, a, b } => a * x[p] + b * x[q],
            Form::ArcPoint { c, r, ang, q, axis } => {
                let t = x[ang];
                let trig = if axis == 0 { t.cos() } else { t.sin() };
                x[c + axis] + x[r] * trig - x[q + axis]
            }
        }
    }

    fn gradient(&self, x: &[f64], g: &mut [f64]) {
        match self.form {
            Form::Coincident { .. } => g.copy_from_slice(&[1.0, -1.0]),
            Form::FixCoord { .. } => g[0] = 1.0,
            Form::Distance { p, q, .. } => {
                let (_, ux, uy) = norm_dir(x[p] - x[q], x[p + 1] - x[q + 1]);
                g.copy_from_slice(&[ux, uy, -ux, -uy]);
            }
            Form::Angle { a, b, c, d, .. } => {
                // φ = ang(d2) − ang(d1)、∂ang(v)/∂v = (−v_y, v_x) / |v|²。
                let (d1x, d1y) = (x[b] - x[a], x[b + 1] - x[a + 1]);
                let (d2x, d2y) = (x[d] - x[c], x[d + 1] - x[c + 1]);
                let l1 = d1x * d1x + d1y * d1y;
                let l2 = d2x * d2x + d2y * d2y;
                let (e1x, e1y) = (d1y / l1, -d1x / l1);
                let (e2x, e2y) = (-d2y / l2, d2x / l2);
                g.copy_from_slice(&[-e1x, -e1y, e1x, e1y, -e2x, -e2y, e2x, e2y]);
            }
            Form::Parallel { a, b, c, d } => {
                let (l1, u1x, u1y) = norm_dir(x[b] - x[a], x[b + 1] - x[a + 1]);
                let (l2, u2x, u2y) = norm_dir(x[d] - x[c], x[d + 1] - x[c + 1]);
                let f = u1x * u2y - u1y * u2x;
                // ∂f/∂d1 = ((u2y, −u2x) − f u1) / |d1|、∂f/∂d2 = ((−u1y, u1x) − f u2) / |d2|。
                let (e1x, e1y) = ((u2y - f * u1x) / l1, (-u2x - f * u1y) / l1);
                let (e2x, e2y) = ((-u1y - f * u2x) / l2, (u1x - f * u2y) / l2);
                g.copy_from_slice(&[-e1x, -e1y, e1x, e1y, -e2x, -e2y, e2x, e2y]);
            }
            Form::External { c1, c2, .. } => {
                let (_, ux, uy) = norm_dir(x[c1] - x[c2], x[c1 + 1] - x[c2 + 1]);
                g.copy_from_slice(&[ux, uy, -1.0, -ux, -uy, -1.0]);
            }
            Form::Internal { c1, c2, s, .. } => {
                let (_, ux, uy) = norm_dir(x[c1] - x[c2], x[c1 + 1] - x[c2 + 1]);
                g.copy_from_slice(&[ux, uy, -s, -ux, -uy, s]);
            }
            Form::LineCircle { a, b, c, s, .. } => {
                let (dx, dy) = (x[b] - x[a], x[b + 1] - x[a + 1]);
                let (wx, wy) = (x[c] - x[a], x[c + 1] - x[a + 1]);
                let l = dx.hypot(dy);
                let (ux, uy) = (dx / l, dy / l);
                let sd = (dx * wy - dy * wx) / l;
                // ∂sd/∂d = ((w_y, −w_x) − sd·u) / |d|、∂sd/∂w = (−u_y, u_x)。
                let (edx, edy) = ((wy - sd * ux) / l, (-wx - sd * uy) / l);
                let (ewx, ewy) = (-uy, ux);
                g.copy_from_slice(&[-edx - ewx, -edy - ewy, edx, edy, ewx, ewy, -s]);
            }
            Form::Linear2 { a, b, .. } => g.copy_from_slice(&[a, b]),
            Form::ArcPoint { r, ang, axis, .. } => {
                let (t, rad) = (x[ang], x[r]);
                if axis == 0 {
                    g.copy_from_slice(&[1.0, t.cos(), -rad * t.sin(), -1.0]);
                } else {
                    g.copy_from_slice(&[1.0, t.sin(), rad * t.cos(), -1.0]);
                }
            }
        }
    }

    fn kind(&self, x: &[f64]) -> ResidualKind {
        self.size
            .as_ref()
            .map_or(ResidualKind::Angle, |s| s.kind(x))
    }
}

// ---------------------------------------------------------------------------
// 系の組み立て
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
struct Sys {
    x: Vec<f64>,
    fixed: Vec<bool>,
    w: Vec<f64>,
    eqs: Vec<TestEq>,
}

impl Sys {
    fn point(&mut self, (x, y): (f64, f64)) -> usize {
        let i = self.x.len();
        self.x.extend([x, y]);
        self.fixed.extend([false, false]);
        self.w.extend([1.0, 1.0]);
        i
    }

    fn scalar(&mut self, v: f64) -> usize {
        let i = self.x.len();
        self.x.push(v);
        self.fixed.push(false);
        self.w.push(1.0);
        i
    }

    fn fix_point(&mut self, p: usize) {
        self.fixed[p] = true;
        self.fixed[p + 1] = true;
    }

    fn eq(&mut self, form: Form, size: Option<Size>) {
        self.eqs.push(TestEq::new(form, size));
    }

    /// 点 p・q の一致（2 本）。`ents` は点を持つ図形。
    fn coincident(&mut self, p: usize, q: usize, ents: &[Ent]) {
        for axis in 0..2 {
            self.eq(Form::Coincident { p, q, axis }, size(ents, &[], &[p, q]));
        }
    }

    /// 線分 ab の水平（axis = 1）・垂直（axis = 0）。
    fn axis_aligned(&mut self, a: usize, b: usize, axis: usize) {
        self.eq(
            Form::Coincident { p: a, q: b, axis },
            size(&[Ent::Seg(a, b)], &[], &[a, b]),
        );
    }

    fn distance(&mut self, p: usize, q: usize, d: f64, ents: &[Ent]) {
        self.eq(Form::Distance { p, q, d }, size(ents, &[d], &[p, q]));
    }

    /// 点 p を `p0` に固定（2 本）。
    fn fix_at(&mut self, p: usize, p0: (f64, f64), ents: &[Ent]) {
        for (axis, value) in [(0, p0.0), (1, p0.1)] {
            let mut s = size(ents, &[], &[p]);
            s.as_mut().unwrap().coords = vec![p0.0, p0.1];
            self.eq(Form::FixCoord { p, axis, value }, s);
        }
    }

    fn problem(&self) -> Problem<'_, TestEq> {
        Problem::new(&self.x, &self.fixed, &self.w, &self.eqs)
    }

    fn solve(&self) -> Result<Solution, SolveError> {
        solve(&self.problem())
    }

    fn dof(&self) -> usize {
        degrees_of_freedom(&self.problem()).unwrap()
    }
}

/// 成功を確かめる: 受理したステップの `F` が狭義単調減少、解いた形が（その形で計算した
/// 許容値で）満たされる、固定したパラメータは動かない。
fn assert_solved(sys: &Sys, sol: &Solution) {
    for w in sol.objective.windows(2) {
        assert!(w[1] < w[0], "F が減っていない: {:?}", sol.objective);
    }
    assert!(sol.trials <= MAX_TRIALS);
    assert!(
        is_satisfied(&sol.params, &sys.eqs).unwrap(),
        "満たされていない: {}",
        max_scaled_residual(&sol.params, &sys.eqs).unwrap()
    );
    for (i, &f) in sys.fixed.iter().enumerate() {
        if f {
            assert_eq!(
                sol.params[i].to_bits(),
                sys.x[i].to_bits(),
                "固定 {i} が動いた"
            );
        }
    }
}

fn solved(sys: &Sys) -> Solution {
    let sol = sys
        .solve()
        .unwrap_or_else(|e| panic!("解けない: {e:?} ({sys:?})"));
    assert_solved(sys, &sol);
    sol
}

/// 初期形の単位座標を `origin + k·(u, v)` へ置く。
fn place(origin: (f64, f64), k: f64, (u, v): (f64, f64)) -> (f64, f64) {
    (origin.0 + k * u, origin.1 + k * v)
}

// ---------------------------------------------------------------------------
// シナリオ（原点・尺度を変えて使い回す）
// ---------------------------------------------------------------------------

/// 線分 4 本（下・右・上・左）の矩形。幅 10k・高さ 5k、左下の角を `origin` に固定する。
/// 角の一致 4 組（8 本）・水平 2・垂直 2・幅と高さの距離 2・点の固定 2 の 16 本で、
/// パラメータ 16 個を完全に拘束する。`start` は 8 点の初期座標（単位座標）。
fn rectangle(origin: (f64, f64), k: f64, start: [(f64, f64); 8]) -> Sys {
    let mut s = Sys::default();
    let p: Vec<usize> = start
        .iter()
        .map(|&uv| s.point(place(origin, k, uv)))
        .collect();
    let bottom = Ent::Seg(p[0], p[1]);
    let right = Ent::Seg(p[2], p[3]);
    let top = Ent::Seg(p[4], p[5]);
    let left = Ent::Seg(p[6], p[7]);
    s.coincident(p[1], p[2], &[bottom, right]);
    s.coincident(p[3], p[4], &[right, top]);
    s.coincident(p[5], p[6], &[top, left]);
    s.coincident(p[7], p[0], &[left, bottom]);
    s.axis_aligned(p[0], p[1], 1);
    s.axis_aligned(p[4], p[5], 1);
    s.axis_aligned(p[2], p[3], 0);
    s.axis_aligned(p[6], p[7], 0);
    s.distance(p[0], p[1], 10.0 * k, &[bottom]);
    s.distance(p[2], p[3], 5.0 * k, &[right]);
    s.fix_at(p[0], origin, &[bottom]);
    s
}

/// 解の近くから始める矩形の初期形。
const RECT_NEAR: [(f64, f64); 8] = [
    (0.1, -0.2),
    (10.3, 0.4),
    (9.8, -0.1),
    (10.2, 5.3),
    (9.9, 4.8),
    (0.3, 5.1),
    (-0.2, 5.2),
    (0.1, 0.3),
];

/// 上の辺を図形の大きさ（10）の 100 倍（1000, 700）離した初期形。
const RECT_FAR: [(f64, f64); 8] = [
    (0.1, -0.2),
    (10.3, 0.4),
    (9.8, -0.1),
    (10.2, 5.3),
    (1009.9, 704.8),
    (1000.3, 705.1),
    (-0.2, 5.2),
    (0.1, 0.3),
];

/// 矩形の期待する解（単位座標）。
const RECT_SOLUTION: [(f64, f64); 8] = [
    (0.0, 0.0),
    (10.0, 0.0),
    (10.0, 0.0),
    (10.0, 5.0),
    (10.0, 5.0),
    (0.0, 5.0),
    (0.0, 5.0),
    (0.0, 0.0),
];

/// 円 1（中心 `origin`・半径 10k、固定）と円 2（中心 `c2`・半径 `r2`、単位座標、自由）の
/// 外接（`internal = false`）または内接。内接の `s` は初期形の半径の大小から決める。
fn circles(origin: (f64, f64), k: f64, c2: (f64, f64), r2: f64, internal: bool) -> Sys {
    let mut s = Sys::default();
    let c1 = s.point(origin);
    let r1 = s.scalar(10.0 * k);
    let c2 = s.point(place(origin, k, c2));
    let r2 = s.scalar(r2 * k);
    s.fix_point(c1);
    s.fixed[r1] = true;
    let sz = size(&[Ent::Round(r1), Ent::Round(r2)], &[], &[c1, c2]);
    if internal {
        let sign = (s.x[r1] - s.x[r2]).signum();
        s.eq(
            Form::Internal {
                c1,
                r1,
                c2,
                r2,
                s: sign,
            },
            sz,
        );
    } else {
        s.eq(Form::External { c1, r1, c2, r2 }, sz);
    }
    s
}

/// 固定した線分 (0,0)→(20k,0) と、中心 `c`・半径 `r`（単位座標、自由）の円の接線（左側）。
fn line_circle(origin: (f64, f64), k: f64, c: (f64, f64), r: f64) -> Sys {
    let mut s = Sys::default();
    let a = s.point(origin);
    let b = s.point(place(origin, k, (20.0, 0.0)));
    let c = s.point(place(origin, k, c));
    let r = s.scalar(r * k);
    s.fix_point(a);
    s.fix_point(b);
    s.eq(
        Form::LineCircle { a, b, c, r, s: 1.0 },
        size(&[Ent::Seg(a, b), Ent::Round(r)], &[], &[c]),
    );
    s
}

/// 固定した線分 1（`origin` → +x 方向へ 10k）と、始点を `origin` に固定し長さ 10k の
/// 線分 2（初期の向き `phi`、終点が自由）に角度 `theta` を課す。
fn angle_sys(origin: (f64, f64), k: f64, phi: f64, theta: f64) -> Sys {
    let mut s = Sys::default();
    let a = s.point(origin);
    let b = s.point(place(origin, k, (10.0, 0.0)));
    let c = s.point(origin);
    let d = s.point(place(origin, k, (10.0 * phi.cos(), 10.0 * phi.sin())));
    s.fix_point(a);
    s.fix_point(b);
    s.fix_point(c);
    s.eq(Form::Angle { a, b, c, d, theta }, None);
    s.distance(c, d, 10.0 * k, &[Ent::Seg(c, d)]);
    s
}

/// 固定した点 p と自由な点 q に、距離 10k と 20k を同時に課す（解が無い）。
fn inconsistent(origin: (f64, f64), k: f64) -> Sys {
    let mut s = Sys::default();
    let p = s.point(origin);
    let q = s.point(place(origin, k, (12.0, 3.0)));
    s.fix_point(p);
    let seg = Ent::Seg(p, q);
    s.distance(p, q, 10.0 * k, &[seg]);
    s.distance(p, q, 20.0 * k, &[seg]);
    s
}

/// 三角不等式を満たさない辺の長さ（1k, 1k, 10k）の三角形（解が無い）。
fn bad_triangle(origin: (f64, f64), k: f64) -> Sys {
    let mut s = Sys::default();
    let a = s.point(origin);
    let b = s.point(place(origin, k, (1.0, 0.0)));
    let c = s.point(place(origin, k, (0.5, 0.8)));
    s.fix_point(a);
    s.distance(a, b, 1.0 * k, &[Ent::Seg(a, b)]);
    s.distance(b, c, 1.0 * k, &[Ent::Seg(b, c)]);
    s.distance(c, a, 10.0 * k, &[Ent::Seg(c, a)]);
    s
}

/// 判定の比較に使うシナリオの一覧（名前・組み立て）。
type Scenario = (&'static str, fn((f64, f64), f64) -> Sys);

fn scenarios() -> Vec<Scenario> {
    vec![
        ("rect_near", |o, k| rectangle(o, k, RECT_NEAR)),
        ("rect_far", |o, k| rectangle(o, k, RECT_FAR)),
        ("external", |o, k| circles(o, k, (30.0, 5.0), 4.0, false)),
        ("internal", |o, k| circles(o, k, (3.0, 1.0), 4.0, true)),
        ("line_circle", |o, k| line_circle(o, k, (5.0, 7.0), 3.0)),
        ("angle_pi", |o, k| angle_sys(o, k, -PI + 0.05, PI - 1e-3)),
        ("inconsistent", inconsistent),
        ("bad_triangle", bad_triangle),
    ]
}

// ---------------------------------------------------------------------------
// ヤコビアンの数値微分照合
// ---------------------------------------------------------------------------

fn coord() -> impl Strategy<Value = f64> {
    -100.0f64..100.0
}

fn pt2() -> impl Strategy<Value = (f64, f64)> {
    (coord(), coord())
}

fn radius() -> impl Strategy<Value = f64> {
    0.5f64..100.0
}

/// 点 4 つ（線分 2 本）を並べたパラメータ。
fn four_points(p: [(f64, f64); 4]) -> Vec<f64> {
    p.iter().flat_map(|&(x, y)| [x, y]).collect()
}

fn assert_jacobian(eq: &TestEq, x: &[f64]) -> Result<(), TestCaseError> {
    let r = check_jacobian(eq, x, 1e-6, 1e-5);
    prop_assert!(r.is_ok(), "{:?} {:?} at {:?}", r, eq.form, x);
    Ok(())
}

proptest! {
    #[test]
    fn jacobian_coincident_and_fix(p in pt2(), q in pt2(), v in coord(), axis in 0usize..2) {
        let x = four_points([p, q, (0.0, 0.0), (0.0, 0.0)]);
        assert_jacobian(&TestEq::new(Form::Coincident { p: 0, q: 2, axis }, None), &x)?;
        assert_jacobian(&TestEq::new(Form::FixCoord { p: 0, axis, value: v }, None), &x)?;
        assert_jacobian(&TestEq::new(Form::Linear2 { p: 0, q: 3, a: v, b: -2.5 }, None), &x)?;
    }

    #[test]
    fn jacobian_distance(p in pt2(), q in pt2(), d in radius()) {
        prop_assume!((p.0 - q.0).hypot(p.1 - q.1) > 1e-2);
        let x = four_points([p, q, (0.0, 0.0), (0.0, 0.0)]);
        assert_jacobian(&TestEq::new(Form::Distance { p: 0, q: 2, d }, None), &x)?;
    }

    #[test]
    fn jacobian_angle(a in pt2(), b in pt2(), c in pt2(), d in pt2(), theta in -PI..PI) {
        let x = four_points([a, b, c, d]);
        prop_assume!(dist(&x, 0, 2) > 1.0 && dist(&x, 4, 6) > 1.0);
        let eq = TestEq::new(Form::Angle { a: 0, b: 2, c: 4, d: 6, theta }, None);
        // 主値化の不連続（φ − θ = ±π）の近くは除く。
        prop_assume!(eq.residual(&x).abs() < PI - 1e-3);
        assert_jacobian(&eq, &x)?;
    }

    /// θ が ±π・0 の近くで、φ がその近く（±π をまたぐ形を含む）にある場合。
    #[test]
    fn jacobian_angle_near_branch(
        theta in prop_oneof![Just(PI), Just(-PI + 1e-9), Just(0.0), Just(PI - 1e-3)],
        dphi in -0.5f64..0.5,
        len in 1.0f64..100.0,
        base in pt2(),
    ) {
        let phi = theta + dphi;
        let x = four_points([
            base,
            (base.0 + len, base.1),
            (base.0 - 3.0, base.1 + 2.0),
            (base.0 - 3.0 + len * phi.cos(), base.1 + 2.0 + len * phi.sin()),
        ]);
        let eq = TestEq::new(Form::Angle { a: 0, b: 2, c: 4, d: 6, theta }, None);
        prop_assert!((eq.residual(&x) - dphi).abs() < 1e-9, "{} vs {}", eq.residual(&x), dphi);
        assert_jacobian(&eq, &x)?;
    }

    #[test]
    fn jacobian_parallel(a in pt2(), b in pt2(), c in pt2(), d in pt2()) {
        let x = four_points([a, b, c, d]);
        prop_assume!(dist(&x, 0, 2) > 1.0 && dist(&x, 4, 6) > 1.0);
        assert_jacobian(&TestEq::new(Form::Parallel { a: 0, b: 2, c: 4, d: 6 }, None), &x)?;
    }

    #[test]
    fn jacobian_circle_tangents(c1 in pt2(), c2 in pt2(), r1 in radius(), r2 in radius()) {
        prop_assume!((c1.0 - c2.0).hypot(c1.1 - c2.1) > 1e-2);
        let x = vec![c1.0, c1.1, r1, c2.0, c2.1, r2];
        assert_jacobian(&TestEq::new(Form::External { c1: 0, r1: 2, c2: 3, r2: 5 }, None), &x)?;
        let s = (r1 - r2).signum();
        assert_jacobian(&TestEq::new(Form::Internal { c1: 0, r1: 2, c2: 3, r2: 5, s }, None), &x)?;
    }

    #[test]
    fn jacobian_line_circle(a in pt2(), b in pt2(), c in pt2(), r in radius(), left in any::<bool>()) {
        let x = vec![a.0, a.1, b.0, b.1, c.0, c.1, r];
        prop_assume!(dist(&x, 0, 2) > 1.0);
        let s = if left { 1.0 } else { -1.0 };
        assert_jacobian(&TestEq::new(Form::LineCircle { a: 0, b: 2, c: 4, r: 6, s }, None), &x)?;
    }

    #[test]
    fn jacobian_arc_point(c in pt2(), r in radius(), t in -10.0f64..10.0, q in pt2(), axis in 0usize..2) {
        let x = vec![c.0, c.1, r, t, q.0, q.1];
        assert_jacobian(&TestEq::new(Form::ArcPoint { c: 0, r: 2, ang: 3, q: 4, axis }, None), &x)?;
    }

    /// 解の近くの矩形を乱した初期形から、受理したステップの F が単調に減って収束する。
    #[test]
    fn rectangle_converges_from_perturbed_start(
        noise in proptest::collection::vec((-2.0f64..2.0, -2.0f64..2.0), 8),
    ) {
        let mut start = RECT_SOLUTION;
        for (s, n) in start.iter_mut().zip(&noise) {
            s.0 += n.0;
            s.1 += n.1;
        }
        let sys = rectangle((0.0, 0.0), 1.0, start);
        let sol = sys.solve();
        prop_assert!(sol.is_ok(), "{:?}", sol);
        assert_solved(&sys, &sol.unwrap());
    }
}

/// 照合の仕組み自体が、間違ったヤコビアンを見逃さないこと。
#[test]
fn check_jacobian_detects_wrong_gradient() {
    struct Wrong([usize; 1]);
    impl Equation for Wrong {
        fn params(&self) -> &[usize] {
            &self.0
        }
        fn residual(&self, x: &[f64]) -> f64 {
            x[0] * x[0]
        }
        fn gradient(&self, x: &[f64], g: &mut [f64]) {
            g[0] = x[0]; // 正しくは 2x
        }
        fn kind(&self, _: &[f64]) -> ResidualKind {
            ResidualKind::Angle
        }
    }
    let err = check_jacobian(&Wrong([0]), &[3.0], 1e-6, 1e-5).unwrap_err();
    assert_eq!(err.param, 0);
    assert_eq!(err.analytic, 3.0);
    assert!((err.numeric - 6.0).abs() < 1e-6);
    // dyn でも使える。
    let dynamic: &dyn Equation = &Wrong([0]);
    assert!(check_jacobian(dynamic, &[3.0], 1e-6, 1e-5).is_err());
}

// ---------------------------------------------------------------------------
// 収束（完了条件: 単調減少と収束）
// ---------------------------------------------------------------------------

fn assert_close(got: (f64, f64), want: (f64, f64), tol: f64) {
    assert!(
        (got.0 - want.0).abs() <= tol && (got.1 - want.1).abs() <= tol,
        "{got:?} != {want:?}"
    );
}

fn pt_of(x: &[f64], p: usize) -> (f64, f64) {
    (x[p], x[p + 1])
}

#[test]
fn rectangle_from_near_start() {
    let sys = rectangle((0.0, 0.0), 1.0, RECT_NEAR);
    let sol = solved(&sys);
    for (i, &want) in RECT_SOLUTION.iter().enumerate() {
        assert_close(pt_of(&sol.params, 2 * i), want, 1e-7);
    }
}

/// 解から図形の大きさの 100 倍離れた初期値から収束する。
#[test]
fn rectangle_from_far_start() {
    let sys = rectangle((0.0, 0.0), 1.0, RECT_FAR);
    let sol = solved(&sys);
    assert!(sol.objective.len() > 2, "反復していない: {sol:?}");
    for (i, &want) in RECT_SOLUTION.iter().enumerate() {
        assert_close(pt_of(&sol.params, 2 * i), want, 1e-7);
    }
}

#[test]
fn circles_far_external() {
    // 円 2 を半径の 100 倍以上離れた位置から外接させる。
    let sys = circles((0.0, 0.0), 1.0, (2000.0, 1500.0), 4.0, false);
    let sol = solved(&sys);
    let d = dist(&sol.params, 0, 3);
    assert!((d - (10.0 + sol.params[5])).abs() < 1e-7);
}

#[test]
fn circles_external_and_internal() {
    let ext = circles((0.0, 0.0), 1.0, (30.0, 5.0), 4.0, false);
    let sol = solved(&ext);
    let (d, r2) = (dist(&sol.params, 0, 3), sol.params[5]);
    assert!((d - (10.0 + r2)).abs() < 1e-7 && r2 > 0.0);

    let int = circles((0.0, 0.0), 1.0, (3.0, 1.0), 4.0, true);
    let sol = solved(&int);
    let (d, r2) = (dist(&sol.params, 0, 3), sol.params[5]);
    assert!((d - (10.0 - r2)).abs() < 1e-7 && r2 > 0.0 && r2 < 10.0);

    // 大きい方の円が動く内接（s = −1）。
    let int = circles((0.0, 0.0), 1.0, (1.0, 2.0), 15.0, true);
    let sol = solved(&int);
    let (d, r2) = (dist(&sol.params, 0, 3), sol.params[5]);
    assert!((d - (r2 - 10.0)).abs() < 1e-7 && r2 > 10.0);
}

#[test]
fn line_circle_tangent_left_side() {
    let sol = solved(&line_circle((0.0, 0.0), 1.0, (5.0, 7.0), 3.0));
    let (cy, r) = (sol.params[5], sol.params[6]);
    assert!(cy > 0.0 && (cy - r).abs() < 1e-7);
    // 中心が直線上（作成時に s = +1 と決めた形）からでも、左側へ距離 r まで解ける。
    let sol = solved(&line_circle((0.0, 0.0), 1.0, (5.0, 0.0), 3.0));
    let (cy, r) = (sol.params[5], sol.params[6]);
    assert!(cy > 0.0 && (cy - r).abs() < 1e-7, "{:?}", sol.params);
}

/// 角度の ±π 付近: なす角 φ が ±π をまたいで θ へ回る。
#[test]
fn angle_across_pi() {
    let cases = [
        (-PI + 0.05, PI - 1e-3),
        (PI - 0.05, -PI + 1e-3),
        (-PI + 0.1, PI),
        (PI - 0.1, PI),
        (0.3, 0.0),
        (-2.0, 2.5),
    ];
    for (phi, theta) in cases {
        let sys = angle_sys((0.0, 0.0), 1.0, phi, theta);
        let sol = solved(&sys);
        let got = angle_between(&sol.params, 0, 2, 4, 6);
        assert!(
            wrap_pi(got - theta).abs() <= ATOL_ANG,
            "phi={phi} theta={theta} got={got}"
        );
    }
}

/// 階数落ち（矛盾しない重複拘束）でも収束し、自由度は重複の無い系と変わらない。
#[test]
fn rank_deficient_consistent_duplicates() {
    let base = rectangle((0.0, 0.0), 1.0, RECT_NEAR);
    let mut sys = base.clone();
    // 同じ水平拘束をもう一度、上下の辺の平行（水平 2 本から従う）、同じ距離をもう一度。
    sys.axis_aligned(0, 2, 1);
    sys.eq(
        Form::Parallel {
            a: 0,
            b: 2,
            c: 8,
            d: 10,
        },
        None,
    );
    sys.distance(0, 2, 10.0, &[Ent::Seg(0, 2)]);
    let sol = solved(&sys);
    for (i, &want) in RECT_SOLUTION.iter().enumerate() {
        assert_close(pt_of(&sol.params, 2 * i), want, 1e-7);
    }
    assert_eq!(base.dof(), 0);
    assert_eq!(sys.dof(), 0);

    // 同一の方程式だけを重ねた系（A Aᵀ が厳密に特異）。
    let mut dup = Sys::default();
    let p = dup.point((0.0, 0.0));
    let q = dup.point((3.0, 4.0));
    dup.fix_point(p);
    for _ in 0..3 {
        dup.distance(p, q, 10.0, &[Ent::Seg(p, q)]);
    }
    let sol = solved(&dup);
    assert!((dist(&sol.params, p, q) - 10.0).abs() < 1e-7);
    assert_eq!(dup.dof(), 1);
}

// ---------------------------------------------------------------------------
// 失敗（完了条件: 解の無い系・自由なパラメータが無い系）
// ---------------------------------------------------------------------------

#[test]
fn unsolvable_fails_deterministically() {
    for sys in [inconsistent((0.0, 0.0), 1.0), bad_triangle((0.0, 0.0), 1.0)] {
        let first = sys.solve();
        assert!(
            matches!(first, Err(SolveError::Stalled | SolveError::TrialLimit)),
            "{first:?}"
        );
        for _ in 0..3 {
            assert_eq!(sys.solve(), first);
        }
    }
    // 解の無い系は停滞として止まる（試行回数の上限まで回らない）。
    assert_eq!(
        inconsistent((0.0, 0.0), 1.0).solve(),
        Err(SolveError::Stalled)
    );
}

/// 自由なパラメータが 0 個で満たされていない系は、反復せずに Err。
#[test]
fn no_free_parameters_fails_without_iterating() {
    let mut sys = Sys::default();
    let a = sys.point((0.0, 0.0));
    let b = sys.point((10.0, 1.0));
    sys.fix_point(a);
    sys.fix_point(b);
    sys.axis_aligned(a, b, 1); // 水平拘束のついた線分を回転した形
    let calls = Cell::new(0);
    let hook = |_: &[f64]| {
        calls.set(calls.get() + 1);
        true
    };
    let problem = sys.problem().with_admissible(&hook);
    assert_eq!(solve(&problem), Err(SolveError::NoFreeParameters));
    assert_eq!(calls.get(), 0, "試行点を作った");

    // 自由なパラメータはあるが、どの方程式も参照しない場合も同じ。
    let mut sys2 = sys.clone();
    sys2.point((5.0, 5.0));
    assert_eq!(sys2.solve(), Err(SolveError::NoFreeParameters));

    // 満たされていれば、自由なパラメータが無くても成功（入力のまま）。
    let mut ok = sys.clone();
    ok.x[b + 1] = 0.0;
    let sol = ok.solve().unwrap();
    assert_eq!(sol.trials, 0);
    assert_eq!(sol.params, ok.x);
}

#[test]
fn invalid_inputs() {
    let sys = rectangle((0.0, 0.0), 1.0, RECT_NEAR);
    let short = &sys.fixed[..3];
    let p = Problem::new(&sys.x, short, &sys.w, &sys.eqs);
    assert_eq!(solve(&p), Err(SolveError::InvalidInput));
    let mut w = sys.w.clone();
    w[0] = 0.0;
    let p = Problem::new(&sys.x, &sys.fixed, &w, &sys.eqs);
    assert_eq!(solve(&p), Err(SolveError::InvalidInput));
    // 範囲外の添字・重複した添字。
    let mut eqs = sys.eqs.clone();
    eqs[0].params[0] = 99;
    let p = Problem::new(&sys.x, &sys.fixed, &sys.w, &eqs);
    assert_eq!(solve(&p), Err(SolveError::InvalidInput));
    let mut eqs = sys.eqs.clone();
    eqs[0].params[1] = eqs[0].params[0];
    let p = Problem::new(&sys.x, &sys.fixed, &sys.w, &eqs);
    assert_eq!(solve(&p), Err(SolveError::InvalidInput));

    // 減衰係数の初期値が範囲外・非有限。自由度の計算は初期値を使わないので、検査もしない。
    for mu in [
        0.0,
        -1e-3,
        LAMBDA_MIN / 2.0,
        LAMBDA_MAX * 2.0,
        f64::INFINITY,
        f64::NAN,
    ] {
        let p = sys.problem().with_initial_damping(mu);
        assert_eq!(solve(&p), Err(SolveError::InvalidInput), "mu {mu}");
        assert_eq!(degrees_of_freedom(&p), degrees_of_freedom(&sys.problem()));
    }

    // 開始時に非有限。
    let mut nan = sys.clone();
    nan.x[3] = f64::NAN;
    assert_eq!(nan.solve(), Err(SolveError::NonFinite));
    // 線分が退化していて角度のヤコビアンが非有限（0 除算）。
    let mut degenerate = angle_sys((0.0, 0.0), 1.0, 0.3, 0.0);
    degenerate.x[6] = 0.0;
    degenerate.x[7] = 0.0;
    assert_eq!(degenerate.solve(), Err(SolveError::NonFinite));

    // 許容値の尺度が非有限。
    let mut bad = sys.clone();
    bad.eqs[4].size.as_mut().unwrap().values.push(f64::NAN);
    assert_eq!(
        bad.solve(),
        Err(SolveError::InvalidTolerance { equation: 4 })
    );
    assert_eq!(
        max_scaled_residual(&bad.x, &bad.eqs),
        Err(SolveError::InvalidTolerance { equation: 4 })
    );

    // どれも入力を書き換えない（借用なので型で保証されるが、値でも確かめる）。
    let before = sys.x.clone();
    let _ = sys.solve();
    assert_eq!(sys.x, before);
}

#[test]
fn empty_system_is_trivially_solved() {
    let x = [1.0, 2.0];
    let eqs: [TestEq; 0] = [];
    let sol = solve(&Problem::new(&x, &[false, false], &[1.0, 1.0], &eqs)).unwrap();
    assert_eq!(sol.params, x);
    assert_eq!(sol.trials, 0);
    assert_eq!(
        degrees_of_freedom(&Problem::new(&x, &[false, true], &[1.0, 1.0], &eqs)),
        Ok(1)
    );
}

// ---------------------------------------------------------------------------
// 許容値と位置・尺度（完了条件）
// ---------------------------------------------------------------------------

/// 同じ形を原点と 1e6 mm 離れた位置に置いたとき、収束・拒否の判定と解いた形が一致する。
#[test]
fn translation_does_not_change_verdict() {
    let far = (1e6, -1e6);
    for (name, build) in scenarios() {
        let near_sys = build((0.0, 0.0), 1.0);
        let far_sys = build(far, 1.0);
        let (a, b) = (near_sys.solve(), far_sys.solve());
        match (&a, &b) {
            (Ok(sa), Ok(sb)) => {
                assert_solved(&near_sys, sa);
                assert_solved(&far_sys, sb);
                // 座標を原点へ戻して比べる（初期値の差が平行移動量。半径は 0）。
                for (i, (va, vb)) in sa.params.iter().zip(&sb.params).enumerate() {
                    let shift = (far_sys.x[i] - near_sys.x[i]).round();
                    assert!(
                        (va - (vb - shift)).abs() < 1e-6,
                        "{name}: param {i}: {va} vs {}",
                        vb - shift
                    );
                }
            }
            (Err(ea), Err(eb)) => assert_eq!(ea, eb, "{name}"),
            _ => panic!("{name}: 判定が違う {a:?} / {b:?}"),
        }
        // 開始時の充足判定（読込時の検査に相当）も一致する。
        assert_eq!(
            is_satisfied(&near_sys.x, &near_sys.eqs),
            is_satisfied(&far_sys.x, &far_sys.eqs),
            "{name}"
        );
    }
}

/// 代表長さ 1e-3 mm と 1e5 mm の形がともに解け、判定が 1 mm の形と一致する。
#[test]
fn tiny_and_huge_shapes_solve() {
    for (name, build) in scenarios() {
        let unit = build((0.0, 0.0), 1.0).solve();
        for k in [1e-3, 1e5] {
            let sys = build((0.0, 0.0), k);
            let res = sys.solve();
            match (&unit, &res) {
                (Ok(su), Ok(sk)) => {
                    assert_solved(&sys, sk);
                    for (i, (vu, vk)) in su.params.iter().zip(&sk.params).enumerate() {
                        assert!(
                            (vk / k - vu).abs() < 1e-6,
                            "{name} k={k}: param {i}: {} vs {vu}",
                            vk / k
                        );
                    }
                }
                (Err(eu), Err(ek)) => assert_eq!(eu, ek, "{name} k={k}"),
                _ => panic!("{name} k={k}: 判定が違う {unit:?} / {res:?}"),
            }
        }
    }
}

/// 遠方でも角度型の許容値は緩まない（長さ型の座標の項だけが分解能の下限として効く）。
#[test]
fn angle_tolerance_does_not_loosen_far_away() {
    let dev = 5e-9; // 5 × ATOL_ANG
    let ratio_at = |origin: (f64, f64)| {
        let sys = angle_sys(origin, 1.0, dev, 0.0);
        max_scaled_residual(&sys.x, &sys.eqs[..1]).unwrap()
    };
    let (near, far) = (ratio_at((0.0, 0.0)), ratio_at((1e6, 1e6)));
    assert!((near - 5.0).abs() < 0.05, "{near}");
    assert!((far - 5.0).abs() < 0.05, "{far}");
    // 解いた後も、遠方で角度の残差が ATOL_ANG 以下になっている。
    let sys = angle_sys((1e6, 1e6), 1.0, 0.4, 0.1);
    let sol = solved(&sys);
    let got = angle_between(&sol.params, 0, 2, 4, 6);
    assert!((got - 0.1).abs() <= ATOL_ANG, "{}", got - 0.1);
}

/// 原点で一致する 2 点・すべての座標が 0 の点を含む拘束で、許容値が正のまま NaN が出ない。
#[test]
fn zero_coordinates_keep_tolerance_positive() {
    assert_eq!(
        ResidualKind::Length {
            size: 0.0,
            coord: 0.0
        }
        .tolerance(),
        Some(ATOL_LEN)
    );
    // 大きさを持たない一致（L = C = 0）: 満たされていて、割った残差は 0（NaN ではない）。
    let mut sys = Sys::default();
    let p = sys.point((0.0, 0.0));
    let q = sys.point((0.0, 0.0));
    sys.coincident(p, q, &[]);
    assert_eq!(max_scaled_residual(&sys.x, &sys.eqs), Ok(0.0));
    let sol = sys.solve().unwrap();
    assert_eq!(sol.trials, 0);
    assert_eq!(sol.objective, vec![0.0]);

    // 原点の 2 本の線分の始点どうしの一致。
    let mut sys = Sys::default();
    let (a, b) = (sys.point((0.0, 0.0)), sys.point((10.0, 0.0)));
    let (c, d) = (sys.point((0.0, 0.0)), sys.point((0.0, 5.0)));
    sys.coincident(a, c, &[Ent::Seg(a, b), Ent::Seg(c, d)]);
    assert!(is_satisfied(&sys.x, &sys.eqs).unwrap());
    assert_eq!(sys.dof(), 6);

    // 原点で一致した 2 点に距離拘束: 固定の方向 (1, 0) で離れ、NaN を出さない。
    let mut sys = Sys::default();
    let p = sys.point((0.0, 0.0));
    let q = sys.point((0.0, 0.0));
    sys.distance(p, q, 5.0, &[]);
    let sol = solved(&sys);
    assert_close(pt_of(&sol.params, p), (2.5, 0.0), 1e-9);
    assert_close(pt_of(&sol.params, q), (-2.5, 0.0), 1e-9);

    // L = C = 0 の許容値（ATOL_LEN）まで詰める一致。
    let mut sys = Sys::default();
    let p = sys.point((0.0, 0.0));
    let q = sys.point((1e-3, 0.0));
    sys.fix_point(p);
    sys.eq(
        Form::Coincident { p, q, axis: 0 },
        Some(Size {
            points: vec![p],
            ..Size::default()
        }),
    );
    let sol = solved(&sys);
    assert!(sol.params[q].abs() <= ATOL_LEN);
}

/// 図形を 100 倍以上に伸ばせる自由度を持つ系で、実際の残差を減らさずに L を大きくする
/// だけの経路を収束と判定しない（許容値を開始時に固定する理由。設計判断1）。
#[test]
fn stretching_does_not_fake_convergence() {
    // A（固定）と Q（固定）は 8e-7 mm ずれている。A を始点に持つ線分 AB（L = 10）が
    // 一致の許容値の尺度に入り、距離拘束が AB を 1000 mm（100 倍）へ伸ばす。
    let mut sys = Sys::default();
    let a = sys.point((0.0, 0.0));
    let b = sys.point((10.0, 0.0));
    let q = sys.point((8e-7, 0.0));
    sys.fix_point(a);
    sys.fix_point(q);
    let ab = Ent::Seg(a, b);
    sys.eq(
        Form::Coincident { p: a, q, axis: 0 },
        size(&[ab], &[], &[a, q]),
    );
    sys.distance(a, b, 1000.0, &[ab]);
    assert!(max_scaled_residual(&sys.x, &sys.eqs[..1]).unwrap() > 10.0);

    assert_eq!(sys.solve(), Err(SolveError::Stalled));

    // 伸ばした形で許容値を計算し直すと 1 以下に見える（反復中に計算し直していたら
    // 収束と誤判定していた経路）。
    let mut stretched = sys.x.clone();
    stretched[b] = 1000.0;
    assert!(is_satisfied(&stretched, &sys.eqs).unwrap());
}

/// 開始時の許容値では満たしていても、解いた後の形の許容値で満たさなければ成功にしない
/// （保存した図面が読込時の検査を必ず通るため。設計判断1）。
#[test]
fn success_requires_final_tolerance_too() {
    // A と Q は 5e-9 mm ずれている。開始時は L = 10（許容値 1e-8）で満たすが、
    // 距離拘束で AB を 0.1 mm へ縮めると許容値が 1e-10 になり満たさない。
    let mut sys = Sys::default();
    let a = sys.point((0.0, 0.0));
    let b = sys.point((10.0, 0.0));
    let q = sys.point((5e-9, 0.0));
    sys.fix_point(a);
    sys.fix_point(q);
    let ab = Ent::Seg(a, b);
    sys.eq(
        Form::Coincident { p: a, q, axis: 0 },
        size(&[ab], &[], &[a, q]),
    );
    sys.distance(a, b, 0.1, &[ab]);
    let tol0 = RTOL * 10.0;
    assert!(5e-9 / tol0 <= 1.0);

    assert_eq!(sys.solve(), Err(SolveError::Stalled));

    let mut shrunk = sys.x.clone();
    shrunk[b] = 0.1;
    assert!(!is_satisfied(&shrunk, &sys.eqs).unwrap());
}

// ---------------------------------------------------------------------------
// 自由度
// ---------------------------------------------------------------------------

#[test]
fn degrees_of_freedom_counts() {
    // 自由な線分: 4。
    let mut seg = Sys::default();
    let a = seg.point((1.0, 2.0));
    let b = seg.point((11.0, 3.0));
    assert_eq!(seg.dof(), 4);
    // 水平: 3。
    let mut s = seg.clone();
    s.axis_aligned(a, b, 1);
    assert_eq!(s.dof(), 3);
    // 水平 + 長さ: 2。
    s.distance(a, b, 10.0, &[Ent::Seg(a, b)]);
    assert_eq!(s.dof(), 2);
    // + 始点の固定（方程式 2 本）: 0。
    s.fix_at(a, (1.0, 2.0), &[Ent::Seg(a, b)]);
    assert_eq!(s.dof(), 0);
    // 矛盾しない重複（水平をもう一度）を足しても 0 のまま。
    s.axis_aligned(a, b, 1);
    assert_eq!(s.dof(), 0);

    // 図形の固定は固定集合: 始点を固定集合へ入れた線分は 2、水平を足すと 1。
    let mut s = seg.clone();
    s.fix_point(a);
    assert_eq!(s.dof(), 2);
    s.axis_aligned(a, b, 1);
    assert_eq!(s.dof(), 1);

    // 円弧（中心・半径・開始角・終了角の 5 つ）の始点を点の固定（2 本）で止めると 5 − 2 = 3。
    let mut arc = Sys::default();
    let c = arc.point((0.0, 0.0));
    let r = arc.scalar(10.0);
    let a0 = arc.scalar(0.3);
    let _a1 = arc.scalar(2.0);
    let q = arc.point((10.0 * 0.3f64.cos(), 10.0 * 0.3f64.sin()));
    arc.fix_point(q);
    for axis in 0..2 {
        arc.eq(
            Form::ArcPoint {
                c,
                r,
                ang: a0,
                q,
                axis,
            },
            size(&[Ent::Round(r)], &[], &[c]),
        );
    }
    assert_eq!(arc.dof(), 3);

    // 完全拘束の矩形は 0、幅と高さの距離を外すと 2。
    let rect = rectangle((0.0, 0.0), 1.0, RECT_NEAR);
    assert_eq!(degrees_of_freedom(&rect.problem()), Ok(0));
    let mut loose = rect.clone();
    loose
        .eqs
        .retain(|e| !matches!(e.form, Form::Distance { .. }));
    assert_eq!(loose.dof(), 2);
    // 解いた後の形でも同じ。
    let sol = solved(&rect);
    let p = Problem::new(&sol.params, &rect.fixed, &rect.w, &rect.eqs);
    assert_eq!(degrees_of_freedom(&p), Ok(0));
}

/// 重みを何桁変えても自由度は変わらない（Codex adversarial review の指摘への回帰テスト:
/// 旧実装は列を重みで割ってから行だけを正規化し、絶対閾値で判定していたため、独立な行
/// [1, 1]・[1, 1 + δ] に大きく違う重みを与えると階数 1 と誤判定しえた）。
#[test]
fn degrees_of_freedom_ignore_weights() {
    let weights = [1e-6, 1e-3, 1.0, 1e3, 1e6];
    // 自由な u・v・w に、u + v = 0、u + (1 + δ)v = 0、w = 0 を課す。
    // δ = 1e-4・1e-6 は独立（自由度 0）、δ = 1e-15・0 は（数値的に）従属（自由度 1）。
    for (delta, want) in [(1e-4, 0), (1e-6, 0), (1e-15, 1), (0.0, 1)] {
        let mut s = Sys::default();
        let u = s.scalar(0.3);
        let v = s.scalar(-0.7);
        let w = s.scalar(2.0);
        s.eq(
            Form::Linear2 {
                p: u,
                q: v,
                a: 1.0,
                b: 1.0,
            },
            None,
        );
        s.eq(
            Form::Linear2 {
                p: u,
                q: v,
                a: 1.0,
                b: 1.0 + delta,
            },
            None,
        );
        s.eq(
            Form::FixCoord {
                p: w,
                axis: 0,
                value: 0.0,
            },
            None,
        );
        for &wu in &weights {
            for &wv in &weights {
                for &ww in &weights {
                    s.w = vec![wu, wv, ww];
                    assert_eq!(s.dof(), want, "delta={delta} w=({wu}, {wv}, {ww})");
                }
            }
        }
        // ヤコビアン自体の列の尺度（係数の桁）を変えても同じ。
        for &k in &weights {
            let mut scaled = s.clone();
            for eq in &mut scaled.eqs {
                if let Form::Linear2 { p, q, a, b } = eq.form {
                    *eq = TestEq::new(Form::Linear2 { p, q, a, b: b * k }, None);
                }
            }
            assert_eq!(scaled.dof(), want, "delta={delta} column scale {k}");
        }
    }
}

/// 半径 1e-3 mm と 1e5 mm の円弧で、同じ拘束構造の自由度が一致する（角度の列は半径に
/// 比例し、角度の重みにも半径を使うため、図形の大きさで自由度の表示が変わらないこと）。
#[test]
fn degrees_of_freedom_do_not_depend_on_arc_size() {
    // 円弧（中心・半径・開始角・終了角）。(a) 始点を固定点へ一致: 5 − 2 = 3、
    // (b) 終点も固定点へ一致: 1（中心が 2 点の垂直二等分線上を動ける）、(c) 半径の値も: 0。
    let build = |radius: f64, angle_weight: f64, with_end: bool, with_radius: bool| {
        let mut s = Sys::default();
        let c = s.point((0.0, 0.0));
        let r = s.scalar(radius);
        let a0 = s.scalar(0.3);
        let a1 = s.scalar(2.0);
        s.w[a0] = angle_weight;
        s.w[a1] = angle_weight;
        let q0 = s.point((radius * 0.3f64.cos(), radius * 0.3f64.sin()));
        let q1 = s.point((radius * 2.0f64.cos(), radius * 2.0f64.sin()));
        s.fix_point(q0);
        s.fix_point(q1);
        let mut ends = vec![(a0, q0)];
        if with_end {
            ends.push((a1, q1));
        }
        for (ang, q) in ends {
            for axis in 0..2 {
                s.eq(
                    Form::ArcPoint { c, r, ang, q, axis },
                    size(&[Ent::Round(r)], &[], &[c]),
                );
            }
        }
        if with_radius {
            s.eq(
                Form::FixCoord {
                    p: r,
                    axis: 0,
                    value: radius,
                },
                size(&[Ent::Round(r)], &[radius], &[]),
            );
        }
        s
    };
    for (with_end, with_radius, want) in [(false, false, 3), (true, false, 1), (true, true, 0)] {
        for radius in [1e-3, 1.0, 1e5] {
            for angle_weight in [1.0, radius, 1e-6, 1e6] {
                let s = build(radius, angle_weight, with_end, with_radius);
                assert_eq!(
                    s.dof(),
                    want,
                    "radius={radius} w={angle_weight} end={with_end} r={with_radius}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 最小ノルム性と重み
// ---------------------------------------------------------------------------

#[test]
fn minimum_norm_moves_as_little_as_possible() {
    // 自由な 2 点の一致: 両方が半分ずつ動いて中点で会う。
    let mut s = Sys::default();
    let p = s.point((0.0, 0.0));
    let q = s.point((2.0, 4.0));
    s.coincident(p, q, &[]);
    let sol = solved(&s);
    assert_close(pt_of(&sol.params, p), (1.0, 2.0), 1e-12);
    assert_close(pt_of(&sol.params, q), (1.0, 2.0), 1e-12);

    // 片方を固定すれば、もう片方だけが動く。
    s.fix_point(q);
    let sol = solved(&s);
    assert_close(pt_of(&sol.params, p), (2.0, 4.0), 1e-12);
    assert_eq!(pt_of(&sol.params, q), (2.0, 4.0));

    // 水平拘束: 動くのは y だけで、両端が半分ずつ。x は 1 ビットも動かない。
    let mut s = Sys::default();
    let a = s.point((0.0, 0.0));
    let b = s.point((10.0, 1.0));
    s.axis_aligned(a, b, 1);
    let sol = solved(&s);
    assert_eq!(sol.params[a], 0.0);
    assert_eq!(sol.params[b], 10.0);
    // 許容値（1e-9 × 長さ 10）に入った時点で止まるので、中点との差もその程度まで残る
    // （初期減衰が小さいと 1 回目のステップでほぼ届き、そこで収束と判定される）。
    assert!((sol.params[a + 1] - 0.5).abs() < 1e-8);
    assert!((sol.params[b + 1] - 0.5).abs() < 1e-8);
    // 両端は対称に動く（最小ノルム）。
    assert!((sol.params[a + 1] + sol.params[b + 1] - 1.0).abs() < 1e-12);

    // 長さ拘束: 線分の向きに沿って両端が対称に伸びる。
    let mut s = Sys::default();
    let a = s.point((0.0, 0.0));
    let b = s.point((10.0, 0.0));
    s.distance(a, b, 20.0, &[Ent::Seg(a, b)]);
    let sol = solved(&s);
    // 長さの許容値（1e-9 × 20）に入った時点で止まるので、端点の位置もその程度まで残る。
    assert_close(pt_of(&sol.params, a), (-5.0, 0.0), 2e-8);
    assert_close(pt_of(&sol.params, b), (15.0, 0.0), 2e-8);
    // 両端は対称に動く（最小ノルム）。
    assert!((sol.params[a] + sol.params[b] - 10.0).abs() < 1e-12);
    assert_eq!((sol.params[a + 1], sol.params[b + 1]), (0.0, 0.0));

    // 拘束と関係の無い線分は動かない（連結していない自由なパラメータ）。
    let mut s = rectangle((0.0, 0.0), 1.0, RECT_NEAR);
    let e = s.point((50.0, 60.0));
    let f = s.point((70.0, 80.0));
    let sol = solved(&s);
    assert_eq!(pt_of(&sol.params, e), (50.0, 60.0));
    assert_eq!(pt_of(&sol.params, f), (70.0, 80.0));
}

/// 角度パラメータの重み（半径）で、角度と長さの動きが弧長の物差しでそろう。
#[test]
fn angle_weight_balances_arc_length() {
    // 半径 100 の円弧の始点（角 0）を、円周に沿って δ = 1e-3 rad 先の固定点へ一致させる。
    // y 成分の 1 次近似は Δc_y + r Δt = r δ。重みなし（w = 1）なら角度だけがほぼ動き、
    // 重み w = r なら中心と角度が弧長で半分ずつ動く。
    let build = |angle_weight: f64| {
        let mut s = Sys::default();
        let c = s.point((0.0, 0.0));
        let r = s.scalar(100.0);
        let t = s.scalar(0.0);
        s.w[t] = angle_weight;
        let delta: f64 = 1e-3;
        let q = s.point((100.0 * delta.cos(), 100.0 * delta.sin()));
        s.fix_point(q);
        for axis in 0..2 {
            s.eq(
                Form::ArcPoint {
                    c,
                    r,
                    ang: t,
                    q,
                    axis,
                },
                size(&[Ent::Round(r)], &[], &[c]),
            );
        }
        (s, c)
    };
    let (unweighted, c) = build(1.0);
    let sol = solved(&unweighted);
    let cy = sol.params[c + 1];
    // 1 次近似の値は 0.1 / (1 + 100²) ≈ 1e-5。重みを付けた場合（0.05）の 1/2500 程度。
    assert!(cy.abs() < 2e-5, "重みなしで中心が動いた: {cy}");

    let (weighted, c) = build(100.0);
    let sol = solved(&weighted);
    let cy = sol.params[c + 1];
    assert!(
        (cy - 0.05).abs() < 1e-3 * 0.05,
        "中心の動き {cy}（期待 0.05）"
    );
}

// ---------------------------------------------------------------------------
// 棄却フック・決定性
// ---------------------------------------------------------------------------

#[test]
fn admissible_hook_rejects_trial_points() {
    // 線分 AB（A 固定）を長さ 1 へ縮める。
    let mut s = Sys::default();
    let a = s.point((0.0, 0.0));
    let b = s.point((10.0, 0.0));
    s.fix_point(a);
    s.distance(a, b, 1.0, &[Ent::Seg(a, b)]);

    // 何でも受け入れるフックは試行ごとに呼ばれる。
    let calls = Cell::new(0);
    let all = |_: &[f64]| {
        calls.set(calls.get() + 1);
        true
    };
    let sol = solve(&s.problem().with_admissible(&all)).unwrap();
    assert_solved(&s, &sol);
    assert_eq!(calls.get(), sol.trials);

    // B.x < 2 を棄却するフックでは、解（B.x = 1）へ行けず失敗する（境界へ漸近する間に
    // 試行回数の上限へ達するか、停滞する）。
    let forbid = |x: &[f64]| x[b] >= 2.0;
    let res = solve(&s.problem().with_admissible(&forbid));
    assert!(
        matches!(res, Err(SolveError::Stalled | SolveError::TrialLimit)),
        "{res:?}"
    );

    // 一時的に通れない領域があっても、棄却と減衰で回り込んで解ける: 最初の大きな
    // ステップ（B.x ≈ 1 へ直行）だけを禁じ、B.x < 5 を 1 回目の試行でだけ棄却する。
    let first = Cell::new(true);
    let once = |x: &[f64]| {
        let ok = !(first.get() && x[b] < 5.0);
        first.set(false);
        ok
    };
    let sol = solve(&s.problem().with_admissible(&once)).unwrap();
    assert_solved(&s, &sol);
    assert!(sol.trials >= 2);
}

#[test]
fn initial_damping_can_be_overridden() {
    // 既定は LAMBDA_0。同じ値を明示しても結果は bit 単位で同じ。
    let sys = rectangle((0.0, 0.0), 1.0, RECT_FAR);
    assert_eq!(sys.problem().initial_damping, LAMBDA_0);
    let default = solved(&sys);
    let explicit = solve(&sys.problem().with_initial_damping(LAMBDA_0)).unwrap();
    assert_eq!(default, explicit);

    // 解き直し用の大きい初期値でも解け、受理したステップの F は単調に減る。減衰が効くぶん
    // 1 回目のステップが短く、試行回数は増える。範囲の両端の値も受け付ける。
    let damped = solve(&sys.problem().with_initial_damping(LAMBDA_RETRY)).unwrap();
    assert_solved(&sys, &damped);
    assert!(
        damped.trials > default.trials,
        "{} vs {}",
        damped.trials,
        default.trials
    );
    assert!(damped.objective[1] > default.objective[1]);
    for mu in [LAMBDA_MIN, 1.0] {
        let sol = solve(&sys.problem().with_initial_damping(mu)).unwrap();
        assert_solved(&sys, &sol);
    }
    // 上限の値も入力としては受け付ける（不正な入力ではない）。ステップが小さすぎて F の
    // 減少が表せず、停滞として失敗する。
    assert_eq!(
        solve(&sys.problem().with_initial_damping(LAMBDA_MAX)),
        Err(SolveError::Stalled)
    );

    // 解の無い系は、初期値を変えても決定的に失敗する。
    let sys = inconsistent((0.0, 0.0), 1.0);
    for mu in [LAMBDA_0, LAMBDA_RETRY] {
        let p = sys.problem().with_initial_damping(mu);
        assert_eq!(solve(&p), Err(SolveError::Stalled));
        assert_eq!(solve(&p), Err(SolveError::Stalled));
    }
}

#[test]
fn deterministic_results() {
    for (name, build) in scenarios() {
        let sys = build((0.0, 0.0), 1.0);
        let first = sys.solve();
        for _ in 0..3 {
            let again = sys.solve();
            match (&first, &again) {
                (Ok(a), Ok(b)) => {
                    assert_eq!(a.trials, b.trials, "{name}");
                    let bits =
                        |s: &Solution| s.params.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
                    assert_eq!(bits(a), bits(b), "{name}");
                }
                _ => assert_eq!(first, again, "{name}"),
            }
        }
    }
}

/// 試行回数の実測（`MAX_TRIALS` の根拠。`cargo test -- --nocapture` で表示）。
/// 解ける系は少ない試行で収束し、解の無い系も上限に届く前に停滞として止まる。
///
/// 代表長さ 1e-3 mm（線分の長さ 0.01 mm）の角度拘束を原点から 1e6 mm に置いた組み合わせは、
/// 座標の分解能（約 1.2e-10 mm）で決まる向きの分解能が `ATOL_ANG` より粗く、停滞する
/// （`ATOL_ANG` の doc の既知の限界）。ここでは停滞までの試行回数として数える。
#[test]
fn trial_counts_stay_well_below_limit() {
    let (mut worst_ok, mut worst_stalled) = (0, 0);
    for (name, build) in scenarios() {
        for origin in [(0.0, 0.0), (1e6, -1e6)] {
            for k in [1e-3, 1.0, 1e5] {
                let sys = build(origin, k);
                match sys.solve() {
                    Ok(s) => {
                        println!(
                            "{name:>14} origin={origin:?} k={k:e}: ok, trials={}",
                            s.trials
                        );
                        worst_ok = worst_ok.max(s.trials);
                    }
                    Err(SolveError::Stalled) => {
                        // 停滞までの試行回数を数える: 試行点を数えるフックで測る。
                        let calls = Cell::new(0);
                        let hook = |_: &[f64]| {
                            calls.set(calls.get() + 1);
                            true
                        };
                        let _ = solve(&sys.problem().with_admissible(&hook));
                        println!(
                            "{name:>14} origin={origin:?} k={k:e}: stalled, trials={}",
                            calls.get()
                        );
                        worst_stalled = worst_stalled.max(calls.get());
                    }
                    Err(e) => panic!("{name} {origin:?} k={k}: {e:?}"),
                }
            }
        }
    }
    println!("worst: ok={worst_ok} stalled={worst_stalled} (MAX_TRIALS = {MAX_TRIALS})");
    assert!(worst_ok * 4 <= MAX_TRIALS, "収束に {worst_ok} 回");
    assert!(
        worst_stalled < MAX_TRIALS,
        "停滞の判定に {worst_stalled} 回"
    );
}
