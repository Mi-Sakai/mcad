//! 拘束の方程式（残差と解析ヤコビアン）と、方程式系の組み立て
//! （DESIGN.md M13 設計判断1・2、タスク96）。
//!
//! `mcad_geom::solve` はエンティティを知らない汎用ソルバーなので、ここで
//! 「エンティティの集合 → パラメータの配列」「拘束 → [`Equation`] の列」「図形の固定 →
//! 固定集合」へ落とす。充足検査（残差の評価。[`System::first_unsatisfied`]）と、
//! [`crate::Document::apply`] の中の求解（タスク97。`document/constraints.rs`）の両方が使う。
//! 求解に渡すもの（[`System::weights`]・[`System::first_inadmissible`]）と、解いた後の検査
//! （[`System::collapsed`]・[`System::first_structure_violation`]）、解いた結果の読み戻し
//! （[`System::shape`]）もここに置く。
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
//! 座標を読まないので `C = 0`。どちらも現在のパラメータから計算する。水平・垂直・平行・直交・
//! 角度は角度型（許容値は `ATOL_ANG`。尺度を持たない）。
//!
//! # 向きの拘束（水平・垂直・平行・直交）
//!
//! 向きを区別しない（平行は反平行も満たす）ので、角度を**周期 π で畳んだ**残差にする
//! （[`Fold`]）。`α` を線分の方向角 `atan2(dy, dx)`、`φ` を 2 線分のなす角
//! `atan2(d1 × d2, d1 · d2)`（角度拘束と同じ定義）として:
//!
//! | 拘束 | 残差 | 0 になる形 |
//! |---|---|---|
//! | 水平 | `½·atan2(sin 2α, cos 2α)` | `α = 0`・`π` |
//! | 垂直 | `½·atan2(−sin 2α, −cos 2α)` | `α = ±π/2` |
//! | 平行 | `½·atan2(sin 2φ, cos 2φ)` | `φ = 0`・`π` |
//! | 直交 | `½·atan2(−sin 2φ, −cos 2φ)` | `φ = ±π/2` |
//!
//! 値は `[−π/2, π/2]` のラジアンで、解までの回転角そのもの。ヤコビアンは `∂α`・`∂φ` を
//! そのまま使う（主値化は区分的に定数の差しか生まない）。不連続になるのは解から最も遠い形
//! （平行ならちょうど直交、水平ならちょうど垂直）だけで、そこでも勾配は 0 にならない。
//! 以前の式（平行・直交は単位方向ベクトルの外積・内積、水平・垂直は端点の座標の差）は、
//! ちょうど 90° ずれた形で勾配が 0 になる・最小ノルム解が長さ 0 の線分になるため解けなかった。
//!
//! `sin 2t`・`cos 2t` は角度を経由せず成分から作る（`sin t ∝ s`・`cos t ∝ c` なら
//! `sin 2t ∝ 2sc`・`cos 2t ∝ (c − s)(c + s)`）。ちょうど水平な線分（`dy == 0.0`）・外積が
//! ちょうど 0 の 2 線分では `2sc` がちょうど 0 になり、残差もちょうど 0（`±0.0`）になる。
//!
//! # 特異点
//!
//! - ノルム（点どうしの距離・円の中心間距離・等長の線分の長さ）は、`ATOL_LEN` 未満のとき
//!   微分の方向に `(1, 0)` を使う（設計判断2）。残差そのものは 0 でも有限。
//! - **線分の方向**を使う方程式（水平・垂直・平行・直交・角度・点と線の距離・線と円の接線）は、線分の長さが
//!   `ATOL_LEN` 未満のとき残差と偏微分に **NaN** を返す（方向が定まらない）。ソルバーは
//!   開始時の NaN を `SolveError::NonFinite` で拒否し、反復中は [`System::first_inadmissible`] で
//!   その形へ入らない。充足検査（[`System::first_unsatisfied`]）は満たされていない扱いにする。
//!
//! # 退化した図形
//!
//! 長さが `ATOL_LEN` 未満の線分・半径が `ATOL_LEN` 未満の円と円弧は、ソルバーが試行点として
//! 受け入れない形（[`System::first_inadmissible`]）。充足検査も同じ条件（[`Slot::admissible`] の
//! 1 か所）を使い、**退化した図形を参照する拘束は、残差が 0 でも満たされていない扱い**にする
//! （長さ 0 の線分でも等長・一致の残差は 0 になる。それを通すと、ソルバーが初期状態と
//! して使えない文書ができる）。
//!
//! 求解ではこれに 2 つを足す（どちらも「解く前の形」との比較なので、充足検査には無い）:
//!
//! - **円弧の掃引**（[`System::first_inadmissible`]）: 開始角・終了角が動いた円弧は、掃引角
//!   `end − start` を解く前の形から連続に追い、0 以下・2π 以上へ達した試行点を棄却する
//!   （掃引が裏返った円弧 = 補弧へ飛び移らせない）。
//! - **許容値に対する退化**（[`System::collapsed`]）: 解いた結果、図形の大きさが、それを
//!   参照する長さ型の拘束の許容値以下へ落ちた形を拒否する。許容値は拘束が参照する図形の
//!   大きさの最大で決まるので、長さ 10 mm の線分と一致でつながった線分は、長さが 1e-8 mm
//!   以下になれば「両端が同じ点」と区別できない。`ATOL_LEN` だけで判定すると、線分を 1 点へ
//!   つぶす系が、長さ 1e-11 mm 程度で「収束」して受理されてしまう。角度型の拘束（水平・垂直・
//!   平行・直交・角度）は長さの物差しを持たないので、この判定には加わらない（残差が図形の
//!   大きさに依らず、図形を縮めても減らないので、つぶす経路を作らない）。

use std::collections::BTreeSet;
use std::f64::consts::TAU;
use std::ops::Range;

use mcad_geom::solve::{ATOL_LEN, Equation, ResidualKind, SolveError, max_scaled_residual};
use mcad_geom::{Arc, Circle, LineSeg, Point2, Shape};

use super::{CircleTangency, Constraint, PointKind, PointRef, ShapeKind, shape_kind};
use crate::{ConstraintId, CoreError, Document, EntityGeom, EntityId};

impl ShapeKind {
    /// この種類の図形が持つパラメータの数。
    pub(crate) const fn param_count(self) -> usize {
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
    /// `ATOL_LEN` 以上。ソルバーの試行点の棄却（[`System::first_inadmissible`]）と充足検査
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

    /// 線分（先頭の添字 `l`）の方向角 `ang(d) = atan2(d_y, d_x)` の偏微分を `sign` 倍して足す。
    /// `∂ang(d)/∂d = (−d_y, d_x) / |d|²`。線分が退化していないことは呼び出し側が確かめる。
    fn add_direction_angle(&mut self, x: &[f64], l: usize, sign: f64) {
        let (dx, dy) = dir(x, l);
        let s = dx * dx + dy * dy;
        self.add_dir(l, -sign * dy / s, sign * dx / s);
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

/// 周期 π で畳んだ角度が 0 になる向き（モジュール doc「向きの拘束」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fold {
    /// 角度が 0・π のとき 0（水平・平行）。
    Along,
    /// 角度が ±π/2 のとき 0（垂直・直交）。
    Across,
}

impl Fold {
    /// 角度 `t` を周期 π で畳んだ残差。`s`・`c` は `sin t`・`cos t` に**共通の正の係数**を
    /// 掛けたもの（線分の方向なら `dy`・`dx`、2 線分のなす角なら外積・内積）。
    ///
    /// [`Fold::Along`] は `½·atan2(sin 2t, cos 2t)`、[`Fold::Across`] は
    /// `½·atan2(−sin 2t, −cos 2t)`（= `t ∓ π/2` を畳んだもの）。どちらも `∂/∂t = 1`。
    /// `s`・`c` がともに 0（方向が定まらない）のときは呼ばない。
    fn residual(self, s: f64, c: f64) -> f64 {
        // sin 2t ∝ 2sc、cos 2t ∝ c² − s²。差は積の形で取る（c² − s² の桁落ちを避ける）。
        let (sin2, cos2) = (2.0 * s * c, (c - s) * (c + s));
        match self {
            Self::Along => 0.5 * sin2.atan2(cos2),
            Self::Across => 0.5 * (-sin2).atan2(-cos2),
        }
    }
}

/// 方程式の形。線分は先頭のパラメータの添字、円・円弧は中心の先頭の添字（半径は `+ 2`）で指す。
#[derive(Debug, Clone, Copy, PartialEq)]
enum Form {
    /// `p[axis] − q[axis]`（一致の 1 成分）。
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
    /// 線分 `l` の方向角 `α` を周期 π で畳んだ角度（水平・垂直。モジュール doc
    /// 「向きの拘束」）。
    Direction { l: usize, fold: Fold },
    /// 線分 `l1` から `l2` へのなす角 `φ` を周期 π で畳んだ角度（平行・直交）。
    Alignment { l1: usize, l2: usize, fold: Fold },
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
            Form::Direction { l, .. } => line(l, &mut raw),
            Form::Alignment { l1, l2, .. }
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

impl ConstraintEq {
    /// 長さ型の方程式が `x` の形から計算する許容値。角度型、または尺度が有限でなければ `None`。
    fn length_tolerance(&self, x: &[f64]) -> Option<f64> {
        match self.kind(x) {
            kind @ ResidualKind::Length { .. } => kind.tolerance(),
            ResidualKind::Angle => None,
        }
    }

    /// 形に依存する構造条件を `x` の形が満たすか（設計判断2「値の範囲と特異点」）:
    ///
    /// - 点どうしの距離は、値 `d` がこの方程式の長さ型の許容値より大きい（許容値以下の距離は
    ///   一致と区別できず、ノルムの微分が定まらない点が解になる）。
    /// - 内接は `s(r1 − r2)` が許容値より大きい（等半径の内接 = 同心で同じ円、大小の逆転）。
    ///
    /// それ以外の方程式は常に `true`。許容値が有限にならなければ `false`。
    /// [`Constraint::validate`]（追加・値の変更の前）と、解いた後の検査が共有する。
    fn structure_holds(&self, x: &[f64]) -> bool {
        match self.form {
            Form::Distance { d, .. } => self.length_tolerance(x).is_some_and(|tol| d > tol),
            // 外接は k1 = k2 = 1、内接は k1 = s・k2 = −s。
            Form::CenterDistance { c1, c2, k1, k2 } if k1 != k2 => self
                .length_tolerance(x)
                .is_some_and(|tol| k1 * (x[c1 + 2] - x[c2 + 2]) > tol),
            _ => true,
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
            Form::Direction { l, fold } => {
                if unit_dir(x, l).is_none() {
                    return f64::NAN;
                }
                let (dx, dy) = dir(x, l);
                fold.residual(dy, dx)
            }
            Form::Alignment { l1, l2, fold } => {
                if unit_dir(x, l1).is_none() || unit_dir(x, l2).is_none() {
                    return f64::NAN;
                }
                let ((d1x, d1y), (d2x, d2y)) = (dir(x, l1), dir(x, l2));
                fold.residual(d1x * d2y - d1y * d2x, d1x * d2x + d1y * d2y)
            }
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
            Form::Direction { l, .. } => {
                if unit_dir(x, l).is_none() {
                    g.out.fill(f64::NAN);
                    return;
                }
                // 畳んだ残差は α と区分的に定数の差しか無いので、∂α をそのまま使う。
                g.add_direction_angle(x, l, 1.0);
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
            Form::Alignment { l1, l2, .. } | Form::Angle { l1, l2, .. } => {
                if unit_dir(x, l1).is_none() || unit_dir(x, l2).is_none() {
                    g.out.fill(f64::NAN);
                    return;
                }
                // φ = ang(d2) − ang(d1)。主値化（角度は周期 2π、平行・直交は周期 π）は
                // 区分的に定数の差しか生まないので、∂φ をそのまま使う。
                g.add_direction_angle(x, l1, -1.0);
                g.add_direction_angle(x, l2, 1.0);
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
    /// 固定するパラメータ。[`System::build`] が入れるのは図形の固定の対象の全パラメータ
    /// だけで、ロックされたレイヤー・利用者が動かしたパラメータは呼び出し側が足す
    /// （`document/constraints.rs`）。
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

    /// `entity` のパラメータの置き場所。この系に無ければ `None`。
    pub(crate) fn slot_of(&self, entity: EntityId) -> Option<Slot> {
        self.slot(entity).ok()
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
            Constraint::Horizontal { line } => {
                let l = self.line(line)?;
                let fold = Fold::Along;
                self.angle_eq(Form::Direction { l, fold });
            }
            Constraint::Vertical { line } => {
                let l = self.line(line)?;
                let fold = Fold::Across;
                self.angle_eq(Form::Direction { l, fold });
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
                let (l1, l2) = (self.line(a)?, self.line(b)?);
                let fold = Fold::Along;
                self.angle_eq(Form::Alignment { l1, l2, fold });
            }
            Constraint::Perpendicular { a, b } => {
                let (l1, l2) = (self.line(a)?, self.line(b)?);
                let fold = Fold::Across;
                self.angle_eq(Form::Alignment { l1, l2, fold });
            }
            Constraint::Angle { a, b, value } => {
                let (l1, l2) = (self.line(a)?, self.line(b)?);
                let theta = value;
                self.angle_eq(Form::Angle { l1, l2, theta });
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

    /// 角度型の方程式（許容値は `ATOL_ANG`。尺度を持たない）。
    fn angle_eq(&mut self, form: Form) {
        self.equations.push(ConstraintEq::new(form, None));
    }

    /// 満たされていない拘束のうち [`ConstraintId`] 順で最初のもの。すべて満たされていれば
    /// `None`。形は組み立てたときのパラメータ（[`System::params`]）。
    ///
    /// 拘束が満たされているとは、次の両方:
    ///
    /// 1. 参照する図形がどれも退化していない（[`Slot::admissible`]。ソルバーが試行点を
    ///    受け入れる条件と同じ）。残差が 0 でも、退化した図形を参照する拘束は満たされて
    ///    いない扱いにする（図形の固定のように方程式を持たない拘束も同じ）。
    /// 2. `mcad_geom::solve::is_satisfied` と同じ判定: **その形から計算した許容値**で割った
    ///    残差の絶対値の最大が 1 以下。残差が有限でない、許容値が有限にならない（座標が
    ///    極端に大きい）拘束は満たされていない扱いにする。
    pub(crate) fn first_unsatisfied(&self) -> Option<ConstraintId> {
        self.first_unsatisfied_at(&self.params)
    }

    /// [`System::first_unsatisfied`] を、パラメータ `x` の形（解いた後の形）で判定する。
    pub(crate) fn first_unsatisfied_at(&self, x: &[f64]) -> Option<ConstraintId> {
        self.rows
            .iter()
            .find(|row| !self.satisfied(row, x))
            .map(|row| row.constraint)
    }

    /// 満たされておらず、しかも方程式が参照するパラメータがすべて `fixed` で固定されている
    /// 拘束のうち、[`ConstraintId`] 順で最初のもの。そういう拘束は、固定を外さない限り
    /// どう解いても満たせない（ソルバーの「自由なパラメータが無い」早期判定を拘束単位に
    /// 細かくしたもの。反復せずに、原因の拘束を名指しで返せる）。
    pub(crate) fn first_unsatisfied_held(&self, fixed: &[bool]) -> Option<ConstraintId> {
        self.rows
            .iter()
            .find(|row| {
                let eqs = &self.equations[row.equations.clone()];
                !eqs.is_empty()
                    && eqs.iter().all(|eq| eq.params.iter().all(|&p| fixed[p]))
                    && !self.satisfied(row, &self.params)
            })
            .map(|row| row.constraint)
    }

    fn satisfied(&self, row: &Row, x: &[f64]) -> bool {
        if !row.slots.iter().all(|&i| self.slots[i].admissible(x)) {
            return false;
        }
        match max_scaled_residual(x, &self.equations[row.equations.clone()]) {
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

    /// 形に依存する構造条件（点どうしの距離 `d` > 許容値、内接の `s(r1 − r2)` > 許容値）を
    /// `x` の形で満たさない拘束のうち、[`ConstraintId`] 順で最初のもの。
    ///
    /// [`Constraint::validate`] が追加・値の変更の前に確かめる条件と同じ式で、コマンドを
    /// 適用した後・解いた後の形にも掛ける（`ModifyEntity` や求解で、内接の 2 円が同心・
    /// 等半径になる経路を塞ぐ）。
    pub(crate) fn first_structure_violation(&self, x: &[f64]) -> Option<ConstraintId> {
        self.rows
            .iter()
            .find(|row| {
                !self.equations[row.equations.clone()]
                    .iter()
                    .all(|eq| eq.structure_holds(x))
            })
            .map(|row| row.constraint)
    }

    /// 最小ノルムの重み（設計判断1「角度パラメータは半径で重みづけする」）: 円弧の開始角・
    /// 終了角は**求解開始時の半径**（下限 `ATOL_LEN`。重みは正でなければならない）、ほかは 1。
    /// 角度の変化を弧長として、長さのパラメータと同じ物差しで測る。
    pub(crate) fn weights(&self) -> Vec<f64> {
        let mut weights = vec![1.0; self.params.len()];
        for slot in &self.slots {
            if slot.kind == ShapeKind::Arc {
                let radius = self.params[slot.offset + 2].abs().max(ATOL_LEN);
                weights[slot.offset + 3] = radius;
                weights[slot.offset + 4] = radius;
            }
        }
        weights
    }

    /// 試行点 `x` を受け入れてよいか（`Problem::admissible` 用）。受け入れられない図形の
    /// うち [`EntityId`] 順で最初のものを返す（すべて受け入れられるなら `None`）。
    ///
    /// - 図形が退化していない（[`Slot::admissible`]）。方向を使う方程式の分母が 0 に
    ///   近づくのを防ぐ。
    /// - 開始角か終了角が動いた円弧は、組み立てたときの形から連続に追った掃引角が
    ///   `(0, 2π)` に収まる（モジュール doc「退化した図形」）。角度が動いていない円弧は
    ///   確かめない（掃引 0 の円弧は幾何として正当で、求解が作ったものではない）。
    pub(crate) fn first_inadmissible(&self, x: &[f64]) -> Option<EntityId> {
        self.slots
            .iter()
            .find(|slot| !slot.admissible(x) || !self.sweep_kept(slot, x))
            .map(|slot| slot.entity)
    }

    /// 円弧 `slot` の `x` での掃引角。組み立てたときの形の掃引角（`[0, 2π)`）に、開始角・
    /// 終了角の変化を足したもの（`[0, 2π)` へ畳まないので、0 や 2π を越えたことが分かる）。
    fn sweep(&self, slot: &Slot, x: &[f64]) -> f64 {
        let (s, e) = (slot.offset + 3, slot.offset + 4);
        let base = Arc::new(Point2::ORIGIN, 1.0, self.params[s], self.params[e]).sweep();
        base + ((x[e] - self.params[e]) - (x[s] - self.params[s]))
    }

    fn sweep_kept(&self, slot: &Slot, x: &[f64]) -> bool {
        if slot.kind != ShapeKind::Arc {
            return true;
        }
        let (s, e) = (slot.offset + 3, slot.offset + 4);
        if x[s] == self.params[s] && x[e] == self.params[e] {
            return true;
        }
        let sweep = self.sweep(slot, x);
        sweep > 0.0 && sweep < TAU
    }

    /// 図形の大きさ（[`System::collapsed`] 用）: 線分は長さ、円は半径、円弧は半径・弧長・
    /// 残りの弧長（補弧）。使わない枠は先頭の値で埋める。
    fn measures(&self, slot: &Slot, x: &[f64]) -> [f64; 3] {
        let o = slot.offset;
        match slot.kind {
            ShapeKind::Line => {
                let (dx, dy) = dir(x, o);
                [dx.hypot(dy); 3]
            }
            ShapeKind::Circle => [x[o + 2]; 3],
            ShapeKind::Arc => {
                let (radius, sweep) = (x[o + 2], self.sweep(slot, x));
                [radius, radius * sweep, radius * (TAU - sweep)]
            }
        }
    }

    /// 求解が退化させた図形のうち [`EntityId`] 順で最初のもの（モジュール doc「退化した図形」）。
    ///
    /// 図形の大きさ（[`System::measures`]）が、それを参照する長さ型の拘束の許容値に対して
    /// 「組み立てたときの形では大きかったのに、`x` では許容値以下」になったら退化。
    /// 許容値はそれぞれの形から計算する。もとから許容値以下の図形（巨大な図形につないだ
    /// 微小な図形）や、求解が大きさを変えていない図形は数えない。角度型の拘束は長さの
    /// 物差しを持たないので見ない（`ATOL_LEN` の下限は [`System::first_inadmissible`] が守る）。
    pub(crate) fn collapsed(&self, x: &[f64]) -> Option<EntityId> {
        let mut found: Option<usize> = None;
        for row in &self.rows {
            let Some(eq) = self.equations[row.equations.clone()].first() else {
                continue;
            };
            let (Some(before), Some(after)) =
                (eq.length_tolerance(&self.params), eq.length_tolerance(x))
            else {
                continue;
            };
            for &i in &row.slots {
                let slot = &self.slots[i];
                let was = self.measures(slot, &self.params);
                let now = self.measures(slot, x);
                // NaN は「許容値より大きくない」に倒す。
                let collapsed = was
                    .iter()
                    .zip(&now)
                    .any(|(was, now)| *was > before && (now.is_nan() || *now <= after));
                if collapsed {
                    found = Some(found.map_or(i, |j| j.min(i)));
                }
            }
        }
        found.map(|i| self.slots[i].entity)
    }

    /// パラメータ `x` から `entity` の図形を組み立てる（解いた結果の読み戻し）。
    /// `entity` がこの系に無ければ `None`。
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
        .and_then(|eq| eq.length_tolerance(&sys.params))
        .ok_or_else(|| {
            CoreError::InvalidConstraint(
                "the length tolerance of the constraint is not finite".to_owned(),
            )
        })
}

/// `constraint` が、文書の現在の形で形に依存する構造条件を満たすか
/// （[`System::first_structure_violation`] と同じ式。[`Constraint::validate`] 用）。
///
/// # Errors
///
/// 参照先が無い・種類が合わないとき。
pub(crate) fn structure_holds(doc: &Document, constraint: &Constraint) -> Result<bool, CoreError> {
    let sys = System::for_constraint(doc, constraint)?;
    Ok(sys.first_structure_violation(&sys.params).is_none())
}

#[cfg(test)]
mod tests;
