//! 拘束の方程式（残差と解析ヤコビアン）と、方程式系の組み立て
//! （DESIGN.md M13 設計判断1・2、タスク96）。
//!
//! `mcad_geom::solve` はエンティティを知らない汎用ソルバーなので、ここで
//! 「エンティティの集合 → パラメータの配列」「拘束 → [`Equation`] の列」「図形の固定 →
//! 固定集合」へ落とす。タスク96 では充足検査（残差の評価）にだけ使い、解くのは
//! タスク97（[`System`] の `fixed`・`unit_weights`・`admissible`・`shape` はそのための足場）。
//!
//! # パラメータの並び
//!
//! エンティティを [`EntityId`] の昇順に並べ、種類ごとに次の順で詰める:
//!
//! | 種類 | パラメータ |
//! |---|---|
//! | 線分 | 始点 x・y、終点 x・y（4） |
//! | 円 | 中心 x・y、半径（3） |
//! | 円弧 | 中心 x・y、半径、開始角、終了角（5） |
//!
//! 円弧の端点は独立したパラメータではなく `center + r(cos a, sin a)` で導き、
//! ヤコビアンは中心・半径・角度へ連鎖させる（`PointExpr::OnArc`）。
//!
//! # 方程式の並び
//!
//! 拘束を [`ConstraintId`] の昇順に並べ、拘束ごとに 0〜2 本を連続して置く（一致・点の固定は
//! x・y の 2 本、図形の固定は 0 本）。[`System::rows`] が拘束と、その方程式の範囲・参照する
//! エンティティを対応づける。
//! 乱数を使わず、同じ入力からは同じ並びになる。
//!
//! # 許容値の尺度
//!
//! 長さ型の方程式の `L` は、**拘束が参照するすべてのエンティティ**の大きさ（線分の長さ・
//! 円と円弧の半径）と拘束の値の最大。`C` は拘束が対象にする点（指定された点、線分の両端、
//! 円・円弧の中心、点の固定の `p₀`）の座標の絶対値の最大。半径だけを見る拘束（半径・等半径）は
//! 座標を読まないので `C = 0`。どちらも現在のパラメータから計算する。平行・直交・角度は角度型。
//!
//! # 特異点
//!
//! - ノルム（点どうしの距離・円の中心間距離・等長の線分の長さ）は、`ATOL_LEN` 未満のとき
//!   微分の方向に `(1, 0)` を使う（設計判断2）。残差そのものは 0 でも有限。
//! - **線分の方向**を使う方程式（平行・直交・角度・点と線の距離・線と円の接線）は、線分の長さが
//!   `ATOL_LEN` 未満のとき残差と偏微分に **NaN** を返す（方向が定まらない）。ソルバーは
//!   開始時の NaN を `SolveError::NonFinite` で拒否し、反復中は [`System::admissible`] で
//!   その形へ入らない。充足検査（[`System::first_unsatisfied`]）は満たされていない扱いにする。
//!
//! # 退化した図形
//!
//! 長さが `ATOL_LEN` 未満の線分・半径が `ATOL_LEN` 未満の円と円弧は、ソルバーが試行点として
//! 受け入れない形（[`System::admissible`]）。充足検査も同じ条件（[`Slot::admissible`] の
//! 1 か所）を使い、**退化した図形を参照する拘束は、残差が 0 でも満たされていない扱い**にする
//! （長さ 0 の線分でも水平・垂直・等長の残差は 0 になる。それを通すと、ソルバーが初期状態と
//! して使えない文書ができる）。

use std::collections::BTreeSet;
use std::ops::Range;

use mcad_geom::solve::{ATOL_LEN, Equation, ResidualKind, SolveError, max_scaled_residual};
use mcad_geom::{Arc, Circle, LineSeg, Point2, Shape};

use super::{CircleTangency, Constraint, PointKind, PointRef, ShapeKind, shape_kind};
use crate::{ConstraintId, CoreError, Document, EntityGeom, EntityId};

impl ShapeKind {
    /// この種類の図形が持つパラメータの数。
    const fn param_count(self) -> usize {
        match self {
            Self::Line => 4,
            Self::Circle => 3,
            Self::Arc => 5,
        }
    }
}

/// 方程式系の中のエンティティ 1 つ（パラメータの置き場所）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Slot {
    pub(crate) entity: EntityId,
    pub(crate) kind: ShapeKind,
    /// 先頭のパラメータの添字。
    pub(crate) offset: usize,
}

impl Slot {
    /// このエンティティのパラメータの添字の範囲。
    pub(crate) fn params(&self) -> Range<usize> {
        self.offset..self.offset + self.kind.param_count()
    }

    /// パラメータ `x` でのこの図形が退化していないか: 線分は長さ、円・円弧は半径が
    /// `ATOL_LEN` 以上。ソルバーの試行点の棄却（[`System::admissible`]）と充足検査
    /// （[`System::first_unsatisfied`]）が共有する唯一の判定。NaN は退化扱い。
    pub(crate) fn admissible(&self, x: &[f64]) -> bool {
        match self.kind {
            ShapeKind::Line => unit_dir(x, self.offset).is_some(),
            ShapeKind::Circle | ShapeKind::Arc => x[self.offset + 2] >= ATOL_LEN,
        }
    }
}

/// 方程式系の中の拘束 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) constraint: ConstraintId,
    /// この拘束の方程式の範囲（図形の固定は空）。
    pub(crate) equations: Range<usize>,
    /// 参照するエンティティ（[`System::slots`] の添字。フィールド順、重複なし）。
    pub(crate) slots: Vec<usize>,
}

/// パラメータから導かれる点。
#[derive(Debug, Clone, Copy, PartialEq)]
enum PointExpr {
    /// `(x[i], x[i + 1])`（線分の端点、円・円弧の中心）。
    Free(usize),
    /// 円弧の端点 `(x[c], x[c + 1]) + x[c + 2] (cos x[ang], sin x[ang])`。
    OnArc { c: usize, ang: usize },
}

impl PointExpr {
    fn eval(self, x: &[f64]) -> (f64, f64) {
        match self {
            Self::Free(i) => (x[i], x[i + 1]),
            Self::OnArc { c, ang } => {
                let (r, t) = (x[c + 2], x[ang]);
                (x[c] + r * t.cos(), x[c + 1] + r * t.sin())
            }
        }
    }

    /// 依存するパラメータを `out` へ足す。
    fn params(self, out: &mut Vec<usize>) {
        match self {
            Self::Free(i) => out.extend([i, i + 1]),
            Self::OnArc { c, ang } => out.extend([c, c + 1, c + 2, ang]),
        }
    }

    /// 残差のこの点に対する偏微分 `(gx, gy)` を、パラメータへ連鎖して足す。
    fn chain(self, x: &[f64], gx: f64, gy: f64, g: &mut Grad<'_>) {
        match self {
            Self::Free(i) => {
                g.add(i, gx);
                g.add(i + 1, gy);
            }
            Self::OnArc { c, ang } => {
                let (r, t) = (x[c + 2], x[ang]);
                let (sin, cos) = t.sin_cos();
                g.add(c, gx);
                g.add(c + 1, gy);
                g.add(c + 2, gx * cos + gy * sin);
                g.add(ang, r * (gy * cos - gx * sin));
            }
        }
    }
}

/// 偏微分の書き込み先。パラメータの添字から `params()` の位置を引いて**足し込む**
/// （同じ円弧の 2 点を指す拘束のように、複数の経路が同じパラメータへ寄与するため）。
struct Grad<'a> {
    params: &'a [usize],
    out: &'a mut [f64],
}

impl Grad<'_> {
    fn add(&mut self, param: usize, value: f64) {
        let k = self
            .params
            .iter()
            .position(|&p| p == param)
            .expect("gradient parameter must be listed in params()");
        self.out[k] += value;
    }

    /// 線分（先頭の添字 `l`）の方向ベクトル `終点 − 始点` に対する偏微分 `(ex, ey)` を足す。
    fn add_dir(&mut self, l: usize, ex: f64, ey: f64) {
        self.add(l, -ex);
        self.add(l + 1, -ey);
        self.add(l + 2, ex);
        self.add(l + 3, ey);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    X,
    Y,
}

impl Axis {
    fn of(self, (x, y): (f64, f64)) -> f64 {
        match self {
            Self::X => x,
            Self::Y => y,
        }
    }

    /// この軸の成分に対する偏微分 1 を `(gx, gy)` で表したもの。
    fn unit(self) -> (f64, f64) {
        match self {
            Self::X => (1.0, 0.0),
            Self::Y => (0.0, 1.0),
        }
    }
}

/// 符号付き距離の目標。
#[derive(Debug, Clone, Copy, PartialEq)]
enum Target {
    /// 定数（点と線の距離の `s·d`）。
    Const(f64),
    /// `sign × x[r]`（線と円の接線の `s·r`）。
    Radius { r: usize, sign: f64 },
}

/// 方程式の形。線分は先頭のパラメータの添字、円・円弧は中心の先頭の添字（半径は `+ 2`）で指す。
#[derive(Debug, Clone, Copy, PartialEq)]
enum Form {
    /// `p[axis] − q[axis]`（一致の 1 成分、水平・垂直）。
    CoordDiff {
        p: PointExpr,
        q: PointExpr,
        axis: Axis,
    },
    /// `p[axis] − value`（点の固定の 1 成分）。
    FixCoord {
        p: PointExpr,
        axis: Axis,
        value: f64,
    },
    /// `‖p − q‖ − d`。
    Distance { p: PointExpr, q: PointExpr, d: f64 },
    /// 線分 `line` を延長した直線から `p` までの符号付き距離 − 目標。
    SignedDistance {
        p: PointExpr,
        line: usize,
        target: Target,
    },
    /// 単位方向ベクトルの外積（平行）。
    Cross { l1: usize, l2: usize },
    /// 単位方向ベクトルの内積（直交）。
    Dot { l1: usize, l2: usize },
    /// 線分の長さの差（等長）。
    LengthDiff { l1: usize, l2: usize },
    /// `x[r1] − x[r2]`（等半径）。
    RadiusDiff { r1: usize, r2: usize },
    /// `x[r] − d`（半径）。
    RadiusValue { r: usize, d: f64 },
    /// `atan2(sin(φ − θ), cos(φ − θ))`、`φ` は線分 `l1` から `l2` へのなす角。
    Angle { l1: usize, l2: usize, theta: f64 },
    /// `‖c1 − c2‖ − (k1·r1 + k2·r2)`。外接は `k1 = k2 = 1`、内接は `k1 = s`・`k2 = −s`。
    CenterDistance {
        c1: usize,
        c2: usize,
        k1: f64,
        k2: f64,
    },
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

/// 線分（先頭の添字 `l`）の方向ベクトル `終点 − 始点`。
fn dir(x: &[f64], l: usize) -> (f64, f64) {
    (x[l + 2] - x[l], x[l + 3] - x[l + 1])
}

/// 線分の長さと単位方向。長さが `ATOL_LEN` 未満（方向が定まらない）なら `None`。
fn unit_dir(x: &[f64], l: usize) -> Option<(f64, f64, f64)> {
    let (dx, dy) = dir(x, l);
    let len = dx.hypot(dy);
    (len >= ATOL_LEN).then(|| (len, dx / len, dy / len))
}

/// NaN を捨てない最大（`f64::max` は NaN を黙って捨て、壊れた尺度を隠してしまう）。
fn max_keep_nan(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}

/// 長さ型の許容値の尺度の元（モジュール doc「許容値の尺度」）。
#[derive(Debug, Clone, Default, PartialEq)]
struct Scale {
    /// 大きさを数える線分（先頭の添字）。
    lines: Vec<usize>,
    /// 大きさを数える半径（パラメータの添字）。
    radii: Vec<usize>,
    /// 拘束の値の絶対値（無ければ 0）。
    value: f64,
    /// 座標の大きさを数える点。
    points: Vec<PointExpr>,
    /// 保存した座標（点の固定の `p₀`）の絶対値の最大（無ければ 0）。
    coord: f64,
}

impl Scale {
    fn kind(&self, x: &[f64]) -> ResidualKind {
        let mut size = self.value;
        for &l in &self.lines {
            let (dx, dy) = dir(x, l);
            size = max_keep_nan(size, dx.hypot(dy));
        }
        for &r in &self.radii {
            size = max_keep_nan(size, x[r].abs());
        }
        let mut coord = self.coord;
        for p in &self.points {
            let (px, py) = p.eval(x);
            coord = max_keep_nan(max_keep_nan(coord, px.abs()), py.abs());
        }
        ResidualKind::Length { size, coord }
    }
}

/// 拘束から作った方程式 1 本（スカラーの残差 1 つ）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ConstraintEq {
    form: Form,
    /// 残差が依存するパラメータの添字（重複なし）。
    params: Vec<usize>,
    /// 長さ型の尺度。`None` なら角度型。
    scale: Option<Scale>,
}

impl ConstraintEq {
    fn new(form: Form, scale: Option<Scale>) -> Self {
        let mut raw = Vec::with_capacity(8);
        let line = |l: usize, out: &mut Vec<usize>| out.extend(l..l + 4);
        match form {
            Form::CoordDiff { p, q, .. } | Form::Distance { p, q, .. } => {
                p.params(&mut raw);
                q.params(&mut raw);
            }
            Form::FixCoord { p, .. } => p.params(&mut raw),
            Form::SignedDistance { p, line: l, target } => {
                line(l, &mut raw);
                p.params(&mut raw);
                if let Target::Radius { r, .. } = target {
                    raw.push(r);
                }
            }
            Form::Cross { l1, l2 }
            | Form::Dot { l1, l2 }
            | Form::LengthDiff { l1, l2 }
            | Form::Angle { l1, l2, .. } => {
                line(l1, &mut raw);
                line(l2, &mut raw);
            }
            Form::RadiusDiff { r1, r2 } => raw.extend([r1, r2]),
            Form::RadiusValue { r, .. } => raw.push(r),
            Form::CenterDistance { c1, c2, .. } => {
                raw.extend([c1, c1 + 1, c1 + 2, c2, c2 + 1, c2 + 2]);
            }
        }
        // 同じ円弧の 2 点を指す拘束では、中心・半径が両方の点から挙がる。ソルバーは重複した
        // 添字を受け付けないので、現れた順を保って重複を除く。
        let mut params = Vec::with_capacity(raw.len());
        for p in raw {
            if !params.contains(&p) {
                params.push(p);
            }
        }
        Self {
            form,
            params,
            scale,
        }
    }
}

impl Equation for ConstraintEq {
    fn params(&self) -> &[usize] {
        &self.params
    }

    fn residual(&self, x: &[f64]) -> f64 {
        match self.form {
            Form::CoordDiff { p, q, axis } => axis.of(p.eval(x)) - axis.of(q.eval(x)),
            Form::FixCoord { p, axis, value } => axis.of(p.eval(x)) - value,
            Form::Distance { p, q, d } => {
                let ((px, py), (qx, qy)) = (p.eval(x), q.eval(x));
                (px - qx).hypot(py - qy) - d
            }
            Form::SignedDistance { p, line, target } => {
                let Some((_, ux, uy)) = unit_dir(x, line) else {
                    return f64::NAN;
                };
                let (px, py) = p.eval(x);
                let (wx, wy) = (px - x[line], py - x[line + 1]);
                let goal = match target {
                    Target::Const(v) => v,
                    Target::Radius { r, sign } => sign * x[r],
                };
                ux * wy - uy * wx - goal
            }
            Form::Cross { l1, l2 } => match (unit_dir(x, l1), unit_dir(x, l2)) {
                (Some((_, u1x, u1y)), Some((_, u2x, u2y))) => u1x * u2y - u1y * u2x,
                _ => f64::NAN,
            },
            Form::Dot { l1, l2 } => match (unit_dir(x, l1), unit_dir(x, l2)) {
                (Some((_, u1x, u1y)), Some((_, u2x, u2y))) => u1x * u2x + u1y * u2y,
                _ => f64::NAN,
            },
            Form::LengthDiff { l1, l2 } => {
                let ((d1x, d1y), (d2x, d2y)) = (dir(x, l1), dir(x, l2));
                d1x.hypot(d1y) - d2x.hypot(d2y)
            }
            Form::RadiusDiff { r1, r2 } => x[r1] - x[r2],
            Form::RadiusValue { r, d } => x[r] - d,
            Form::Angle { l1, l2, theta } => {
                if unit_dir(x, l1).is_none() || unit_dir(x, l2).is_none() {
                    return f64::NAN;
                }
                let ((d1x, d1y), (d2x, d2y)) = (dir(x, l1), dir(x, l2));
                let phi = (d1x * d2y - d1y * d2x).atan2(d1x * d2x + d1y * d2y);
                let diff = phi - theta;
                diff.sin().atan2(diff.cos())
            }
            Form::CenterDistance { c1, c2, k1, k2 } => {
                (x[c1] - x[c2]).hypot(x[c1 + 1] - x[c2 + 1]) - (k1 * x[c1 + 2] + k2 * x[c2 + 2])
            }
        }
    }

    fn gradient(&self, x: &[f64], grad: &mut [f64]) {
        grad.fill(0.0);
        let mut g = Grad {
            params: &self.params,
            out: grad,
        };
        match self.form {
            Form::CoordDiff { p, q, axis } => {
                let (gx, gy) = axis.unit();
                p.chain(x, gx, gy, &mut g);
                q.chain(x, -gx, -gy, &mut g);
            }
            Form::FixCoord { p, axis, .. } => {
                let (gx, gy) = axis.unit();
                p.chain(x, gx, gy, &mut g);
            }
            Form::Distance { p, q, .. } => {
                let ((px, py), (qx, qy)) = (p.eval(x), q.eval(x));
                let (_, ux, uy) = norm_dir(px - qx, py - qy);
                p.chain(x, ux, uy, &mut g);
                q.chain(x, -ux, -uy, &mut g);
            }
            Form::SignedDistance { p, line, target } => {
                let Some((len, ux, uy)) = unit_dir(x, line) else {
                    g.out.fill(f64::NAN);
                    return;
                };
                let (px, py) = p.eval(x);
                let (wx, wy) = (px - x[line], py - x[line + 1]);
                let sd = ux * wy - uy * wx;
                // sd = u × w、w = p − a。∂sd/∂w = (−u_y, u_x)、∂sd/∂d = ((w_y, −w_x) − sd·u) / |d|。
                let (ewx, ewy) = (-uy, ux);
                g.add_dir(line, (wy - sd * ux) / len, (-wx - sd * uy) / len);
                g.add(line, -ewx);
                g.add(line + 1, -ewy);
                p.chain(x, ewx, ewy, &mut g);
                if let Target::Radius { r, sign } = target {
                    g.add(r, -sign);
                }
            }
            Form::Cross { l1, l2 } => {
                let (Some((n1, u1x, u1y)), Some((n2, u2x, u2y))) =
                    (unit_dir(x, l1), unit_dir(x, l2))
                else {
                    g.out.fill(f64::NAN);
                    return;
                };
                let f = u1x * u2y - u1y * u2x;
                // ∂f/∂d1 = ((u2_y, −u2_x) − f·u1) / |d1|、∂f/∂d2 = ((−u1_y, u1_x) − f·u2) / |d2|。
                g.add_dir(l1, (u2y - f * u1x) / n1, (-u2x - f * u1y) / n1);
                g.add_dir(l2, (-u1y - f * u2x) / n2, (u1x - f * u2y) / n2);
            }
            Form::Dot { l1, l2 } => {
                let (Some((n1, u1x, u1y)), Some((n2, u2x, u2y))) =
                    (unit_dir(x, l1), unit_dir(x, l2))
                else {
                    g.out.fill(f64::NAN);
                    return;
                };
                let f = u1x * u2x + u1y * u2y;
                // ∂f/∂d1 = (u2 − f·u1) / |d1|、∂f/∂d2 = (u1 − f·u2) / |d2|。
                g.add_dir(l1, (u2x - f * u1x) / n1, (u2y - f * u1y) / n1);
                g.add_dir(l2, (u1x - f * u2x) / n2, (u1y - f * u2y) / n2);
            }
            Form::LengthDiff { l1, l2 } => {
                let ((d1x, d1y), (d2x, d2y)) = (dir(x, l1), dir(x, l2));
                let (_, u1x, u1y) = norm_dir(d1x, d1y);
                let (_, u2x, u2y) = norm_dir(d2x, d2y);
                g.add_dir(l1, u1x, u1y);
                g.add_dir(l2, -u2x, -u2y);
            }
            Form::RadiusDiff { r1, r2 } => {
                g.add(r1, 1.0);
                g.add(r2, -1.0);
            }
            Form::RadiusValue { r, .. } => g.add(r, 1.0),
            Form::Angle { l1, l2, .. } => {
                if unit_dir(x, l1).is_none() || unit_dir(x, l2).is_none() {
                    g.out.fill(f64::NAN);
                    return;
                }
                // φ = ang(d2) − ang(d1)、∂ang(v)/∂v = (−v_y, v_x) / |v|²。主値化は定数の 2π の
                // 差しか生まないので、∂φ をそのまま使う。
                let ((d1x, d1y), (d2x, d2y)) = (dir(x, l1), dir(x, l2));
                let s1 = d1x * d1x + d1y * d1y;
                let s2 = d2x * d2x + d2y * d2y;
                g.add_dir(l1, d1y / s1, -d1x / s1);
                g.add_dir(l2, -d2y / s2, d2x / s2);
            }
            Form::CenterDistance { c1, c2, k1, k2 } => {
                let (_, ux, uy) = norm_dir(x[c1] - x[c2], x[c1 + 1] - x[c2 + 1]);
                g.add(c1, ux);
                g.add(c1 + 1, uy);
                g.add(c1 + 2, -k1);
                g.add(c2, -ux);
                g.add(c2 + 1, -uy);
                g.add(c2 + 2, -k2);
            }
        }
    }

    fn kind(&self, x: &[f64]) -> ResidualKind {
        self.scale
            .as_ref()
            .map_or(ResidualKind::Angle, |s| s.kind(x))
    }
}

/// 拘束の集合から組み立てた方程式系（モジュール doc 参照）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct System {
    /// 参照されるエンティティ（[`EntityId`] の昇順）。
    pub(crate) slots: Vec<Slot>,
    /// パラメータの現在値。
    pub(crate) params: Vec<f64>,
    /// 固定するパラメータ（図形の固定の対象の全パラメータ）。ロックされたレイヤー・
    /// 利用者が動かしたパラメータはタスク97 が足す。
    pub(crate) fixed: Vec<bool>,
    /// 方程式（拘束の [`ConstraintId`] の昇順、拘束ごとに連続）。
    pub(crate) equations: Vec<ConstraintEq>,
    /// 拘束ごとの方程式の範囲と参照するエンティティ（[`ConstraintId`] の昇順）。
    pub(crate) rows: Vec<Row>,
}

impl System {
    /// 拘束の集合から方程式系を組み立てる。エンティティの集合は拘束が参照するもの全部。
    ///
    /// `geom_of` はエンティティの現在の幾何を返す。`constraints` はどの順で渡してもよい
    /// （[`ConstraintId`] の昇順に並べ直す。同じ ID どうしは渡した順）。
    ///
    /// # Errors
    ///
    /// 参照先が無ければ [`CoreError::EntityNotFound`]、種類が合わなければ
    /// [`CoreError::InvalidConstraint`]（どちらも [`Constraint::validate`] が先に弾くので、
    /// 文書に入っている拘束では起きない）。
    pub(crate) fn build<'a>(
        geom_of: impl Fn(EntityId) -> Option<&'a EntityGeom>,
        constraints: impl IntoIterator<Item = (ConstraintId, &'a Constraint)>,
    ) -> Result<Self, CoreError> {
        let mut list: Vec<(ConstraintId, &Constraint)> = constraints.into_iter().collect();
        list.sort_by_key(|(id, _)| *id);
        let entities: BTreeSet<EntityId> = list
            .iter()
            .flat_map(|(_, c)| c.targets().map(|(entity, _)| entity))
            .collect();

        let mut slots = Vec::with_capacity(entities.len());
        let mut params = Vec::new();
        for entity in entities {
            let geom = geom_of(entity).ok_or(CoreError::EntityNotFound(entity))?;
            let offset = params.len();
            let kind = match geom.as_shape() {
                Some(Shape::Line(l)) => {
                    params.extend([l.a.x, l.a.y, l.b.x, l.b.y]);
                    ShapeKind::Line
                }
                Some(Shape::Circle(c)) => {
                    params.extend([c.center.x, c.center.y, c.radius]);
                    ShapeKind::Circle
                }
                Some(Shape::Arc(a)) => {
                    params.extend([a.center.x, a.center.y, a.radius, a.start_angle, a.end_angle]);
                    ShapeKind::Arc
                }
                _ => return Err(not_constrainable(entity)),
            };
            debug_assert_eq!(shape_kind(geom), Some(kind));
            slots.push(Slot {
                entity,
                kind,
                offset,
            });
        }

        let mut sys = Self {
            fixed: vec![false; params.len()],
            slots,
            params,
            equations: Vec::new(),
            rows: Vec::with_capacity(list.len()),
        };
        for (id, constraint) in list {
            let start = sys.equations.len();
            sys.push(constraint)?;
            let slots = constraint
                .entities()
                .into_iter()
                .map(|entity| sys.slot_index(entity))
                .collect::<Result<Vec<usize>, CoreError>>()?;
            sys.rows.push(Row {
                constraint: id,
                equations: start..sys.equations.len(),
                slots,
            });
        }
        Ok(sys)
    }

    /// 文書の現在の形に対する、拘束 1 件だけの方程式系（文書に入っていない拘束でもよい）。
    pub(crate) fn for_constraint(
        doc: &Document,
        constraint: &Constraint,
    ) -> Result<Self, CoreError> {
        Self::build(
            |id| doc.entity(id).map(|e| &e.geom),
            [(ConstraintId::default(), constraint)],
        )
    }

    fn slot_index(&self, entity: EntityId) -> Result<usize, CoreError> {
        self.slots
            .binary_search_by_key(&entity, |s| s.entity)
            .map_err(|_| CoreError::EntityNotFound(entity))
    }

    fn slot(&self, entity: EntityId) -> Result<Slot, CoreError> {
        self.slot_index(entity).map(|i| self.slots[i])
    }

    /// 線分の先頭のパラメータの添字。
    fn line(&self, entity: EntityId) -> Result<usize, CoreError> {
        let slot = self.slot(entity)?;
        if slot.kind == ShapeKind::Line {
            Ok(slot.offset)
        } else {
            Err(CoreError::InvalidConstraint(format!(
                "{entity:?} is not a line"
            )))
        }
    }

    /// 円・円弧の中心の先頭のパラメータの添字（半径は `+ 2`）。
    fn round(&self, entity: EntityId) -> Result<usize, CoreError> {
        let slot = self.slot(entity)?;
        if matches!(slot.kind, ShapeKind::Circle | ShapeKind::Arc) {
            Ok(slot.offset)
        } else {
            Err(CoreError::InvalidConstraint(format!(
                "{entity:?} is not a circle or an arc"
            )))
        }
    }

    fn point(&self, point: PointRef) -> Result<PointExpr, CoreError> {
        let slot = self.slot(point.entity)?;
        let o = slot.offset;
        match (slot.kind, point.kind) {
            (ShapeKind::Line, PointKind::Start)
            | (ShapeKind::Circle | ShapeKind::Arc, PointKind::Center) => Ok(PointExpr::Free(o)),
            (ShapeKind::Line, PointKind::End) => Ok(PointExpr::Free(o + 2)),
            (ShapeKind::Arc, PointKind::Start) => Ok(PointExpr::OnArc { c: o, ang: o + 3 }),
            (ShapeKind::Arc, PointKind::End) => Ok(PointExpr::OnArc { c: o, ang: o + 4 }),
            (ShapeKind::Line, PointKind::Center)
            | (ShapeKind::Circle, PointKind::Start | PointKind::End) => {
                Err(CoreError::InvalidConstraint(format!(
                    "{:?} has no {:?} point",
                    point.entity, point.kind
                )))
            }
        }
    }

    /// 拘束が参照するすべてのエンティティの大きさを数える尺度（値・点は呼び出し側が足す）。
    fn scale_of(&self, constraint: &Constraint) -> Result<Scale, CoreError> {
        let mut scale = Scale::default();
        for (entity, _) in constraint.targets() {
            let slot = self.slot(entity)?;
            let (list, item) = match slot.kind {
                ShapeKind::Line => (&mut scale.lines, slot.offset),
                ShapeKind::Circle | ShapeKind::Arc => (&mut scale.radii, slot.offset + 2),
            };
            if !list.contains(&item) {
                list.push(item);
            }
        }
        scale.value = constraint.value().map_or(0.0, f64::abs);
        Ok(scale)
    }

    /// 拘束 1 件の方程式を足す（図形の固定は固定集合へ入れる）。
    fn push(&mut self, constraint: &Constraint) -> Result<(), CoreError> {
        let mut scale = self.scale_of(constraint)?;
        let ends = |l: usize| [PointExpr::Free(l), PointExpr::Free(l + 2)];
        match *constraint {
            Constraint::Coincident { a, b } => {
                let (p, q) = (self.point(a)?, self.point(b)?);
                scale.points = vec![p, q];
                for axis in [Axis::X, Axis::Y] {
                    self.length_eq(Form::CoordDiff { p, q, axis }, &scale);
                }
            }
            Constraint::Horizontal { line } | Constraint::Vertical { line } => {
                let l = self.line(line)?;
                let [q, p] = ends(l);
                scale.points = vec![p, q];
                let axis = if matches!(constraint, Constraint::Horizontal { .. }) {
                    Axis::Y
                } else {
                    Axis::X
                };
                self.length_eq(Form::CoordDiff { p, q, axis }, &scale);
            }
            Constraint::Distance { a, b, value } => {
                let (p, q) = (self.point(a)?, self.point(b)?);
                scale.points = vec![p, q];
                self.length_eq(Form::Distance { p, q, d: value }, &scale);
            }
            Constraint::PointLineDistance {
                point,
                line,
                side,
                value,
            } => {
                let (p, l) = (self.point(point)?, self.line(line)?);
                scale.points = vec![p];
                scale.points.extend(ends(l));
                let target = Target::Const(side.sign() * value);
                self.length_eq(Form::SignedDistance { p, line: l, target }, &scale);
            }
            Constraint::Parallel { a, b } => {
                let form = Form::Cross {
                    l1: self.line(a)?,
                    l2: self.line(b)?,
                };
                self.equations.push(ConstraintEq::new(form, None));
            }
            Constraint::Perpendicular { a, b } => {
                let form = Form::Dot {
                    l1: self.line(a)?,
                    l2: self.line(b)?,
                };
                self.equations.push(ConstraintEq::new(form, None));
            }
            Constraint::Angle { a, b, value } => {
                let form = Form::Angle {
                    l1: self.line(a)?,
                    l2: self.line(b)?,
                    theta: value,
                };
                self.equations.push(ConstraintEq::new(form, None));
            }
            Constraint::EqualLength { a, b } => {
                let (l1, l2) = (self.line(a)?, self.line(b)?);
                scale.points = ends(l1).into_iter().chain(ends(l2)).collect();
                self.length_eq(Form::LengthDiff { l1, l2 }, &scale);
            }
            Constraint::EqualRadius { a, b } => {
                let form = Form::RadiusDiff {
                    r1: self.round(a)? + 2,
                    r2: self.round(b)? + 2,
                };
                self.length_eq(form, &scale);
            }
            Constraint::Radius { entity, value } => {
                let form = Form::RadiusValue {
                    r: self.round(entity)? + 2,
                    d: value,
                };
                self.length_eq(form, &scale);
            }
            Constraint::TangentLineCircle { line, circle, side } => {
                let (l, c) = (self.line(line)?, self.round(circle)?);
                let p = PointExpr::Free(c);
                scale.points = vec![p];
                scale.points.extend(ends(l));
                let target = Target::Radius {
                    r: c + 2,
                    sign: side.sign(),
                };
                self.length_eq(Form::SignedDistance { p, line: l, target }, &scale);
            }
            Constraint::TangentCircles { a, b, kind } => {
                let (c1, c2) = (self.round(a)?, self.round(b)?);
                scale.points = vec![PointExpr::Free(c1), PointExpr::Free(c2)];
                let (k1, k2) = match kind {
                    CircleTangency::External => (1.0, 1.0),
                    CircleTangency::Internal(larger) => (larger.sign(), -larger.sign()),
                };
                self.length_eq(Form::CenterDistance { c1, c2, k1, k2 }, &scale);
            }
            Constraint::FixPoint { point, at } => {
                let p = self.point(point)?;
                scale.points = vec![p];
                scale.coord = max_keep_nan(at.x.abs(), at.y.abs());
                for (axis, value) in [(Axis::X, at.x), (Axis::Y, at.y)] {
                    self.length_eq(Form::FixCoord { p, axis, value }, &scale);
                }
            }
            Constraint::FixEntity { entity } => {
                let slot = self.slot(entity)?;
                self.fixed[slot.params()].fill(true);
            }
        }
        Ok(())
    }

    fn length_eq(&mut self, form: Form, scale: &Scale) {
        self.equations
            .push(ConstraintEq::new(form, Some(scale.clone())));
    }

    /// 満たされていない拘束のうち [`ConstraintId`] 順で最初のもの。すべて満たされていれば
    /// `None`。
    ///
    /// 拘束が満たされているとは、次の両方:
    ///
    /// 1. 参照する図形がどれも退化していない（[`Slot::admissible`]。ソルバーが試行点を
    ///    受け入れる条件と同じ）。残差が 0 でも、退化した図形を参照する拘束は満たされて
    ///    いない扱いにする（図形の固定のように方程式を持たない拘束も同じ）。
    /// 2. `mcad_geom::solve::is_satisfied` と同じ判定: **今の形から計算した許容値**で割った
    ///    残差の絶対値の最大が 1 以下。残差が有限でない、許容値が有限にならない（座標が
    ///    極端に大きい）拘束は満たされていない扱いにする。
    pub(crate) fn first_unsatisfied(&self) -> Option<ConstraintId> {
        self.rows
            .iter()
            .find(|row| !self.satisfied(row))
            .map(|row| row.constraint)
    }

    fn satisfied(&self, row: &Row) -> bool {
        if !row
            .slots
            .iter()
            .all(|&i| self.slots[i].admissible(&self.params))
        {
            return false;
        }
        match max_scaled_residual(&self.params, &self.equations[row.equations.clone()]) {
            Ok(ratio) => ratio <= 1.0,
            Err(SolveError::InvalidInput) => {
                // 添字が範囲外・重複するのは方程式の組み立ての誤り（`ConstraintEq::new` が
                // 重複を除き、添字は `slots` から作るので起きない）。
                debug_assert!(false, "constraint equations have invalid parameter indices");
                false
            }
            Err(_) => false,
        }
    }

    /// 重み（タスク96 ではすべて 1。角度パラメータの重みはタスク97 で決める）。
    #[cfg_attr(not(test), expect(dead_code, reason = "求解（タスク97）で使う足場"))]
    pub(crate) fn unit_weights(&self) -> Vec<f64> {
        vec![1.0; self.params.len()]
    }

    /// 試行点 `x` を受け入れてよいか（`Problem::admissible` 用）: すべての図形が退化して
    /// いない（[`Slot::admissible`]）。方向を使う方程式の分母が 0 に近づくのを防ぐ。
    #[cfg_attr(not(test), expect(dead_code, reason = "求解（タスク97）で使う足場"))]
    pub(crate) fn admissible(&self, x: &[f64]) -> bool {
        self.slots.iter().all(|slot| slot.admissible(x))
    }

    /// パラメータ `x` から `entity` の図形を組み立てる（解いた結果の読み戻し）。
    /// `entity` がこの系に無ければ `None`。
    #[cfg_attr(not(test), expect(dead_code, reason = "求解（タスク97）で使う足場"))]
    pub(crate) fn shape(&self, entity: EntityId, x: &[f64]) -> Option<Shape> {
        let slot = self.slot(entity).ok()?;
        let o = slot.offset;
        let pt = |i: usize| Point2::new(x[i], x[i + 1]);
        Some(match slot.kind {
            ShapeKind::Line => Shape::Line(LineSeg::new(pt(o), pt(o + 2))),
            ShapeKind::Circle => Shape::Circle(Circle::new(pt(o), x[o + 2])),
            ShapeKind::Arc => Shape::Arc(Arc::new(pt(o), x[o + 2], x[o + 3], x[o + 4])),
        })
    }
}

fn not_constrainable(entity: EntityId) -> CoreError {
    CoreError::InvalidConstraint(format!(
        "{entity:?} is not a line, a circle or an arc and cannot be constrained"
    ))
}

/// `constraint` の長さ型の方程式が、文書の現在の形から計算する許容値。
///
/// 構造の検証（点どうしの距離 `d > 許容値`、内接の `s(r1 − r2) > 許容値`）と、側を決める
/// コンストラクタ（`|符号付き距離| ≤ 許容値` なら左）が使う。充足検査と同じ式を通すことで、
/// 判定の式を 2 か所に持たない。
///
/// # Errors
///
/// 参照先が無い・種類が合わないとき、長さ型の方程式を持たない拘束のとき、許容値が
/// 有限にならないとき。
pub(crate) fn length_tolerance(doc: &Document, constraint: &Constraint) -> Result<f64, CoreError> {
    let sys = System::for_constraint(doc, constraint)?;
    sys.equations
        .first()
        .and_then(|eq| match eq.kind(&sys.params) {
            kind @ ResidualKind::Length { .. } => kind.tolerance(),
            ResidualKind::Angle => None,
        })
        .ok_or_else(|| {
            CoreError::InvalidConstraint(
                "the length tolerance of the constraint is not finite".to_owned(),
            )
        })
}

#[cfg(test)]
mod tests;
