//! 幾何拘束のデータモデル（DESIGN.md M13 設計判断2・4、タスク96）。
//!
//! # 構成
//!
//! - [`Constraint`] は拘束 1 件。対象（[`EntityId`] / [`PointRef`]）と、作成時に決めて
//!   保存する値（距離・半径・角度、側の符号 [`Side`]、外接/内接 [`CircleTangency`]、
//!   点の固定の座標）を持つ**完全な値**。[`crate::Command::AddConstraint`] に載せる。
//! - コンストラクタ（[`Constraint::distance`] など）: 現在の形から値・側・外接/内接を決める。
//! - [`Constraint::tangent_contact`] は、接線拘束の接点と、接点が線分の区間・円弧の掃引範囲の
//!   外にあるか（表示用）。
//! - [`Constraint::mirrored`] は、図形をミラーしたときに同じ幾何関係を表す拘束
//!   （[`crate::Command::ReplaceConstraint`] に載せる）。
//! - `equations` モジュール（クレート内部）: 拘束を `mcad_geom::solve` の方程式
//!   （残差と解析ヤコビアン）へ落とし、方程式系を組み立てる。解くのは
//!   [`crate::Document::apply`]（`document/constraints.rs`）。
//! - `reshape_removals`（クレート内部）: [`crate::Command::ReshapeEntity`] で消す拘束を
//!   決める純関数（設計判断4）。
//!
//! # 対象にできる図形
//!
//! 線分・円・円弧（[`mcad_geom::Shape`] の `Line`・`Circle`・`Arc`）だけ。ポリライン・点・
//! 構築線・文字・寸法・表は対象外（M13 スコープ外）。
//!
//! # serde を付けない
//!
//! [`Constraint`] は対象を `slotmap` の生キーで指すため、[`crate::Entity`] と同じく
//! `Serialize`/`Deserialize` を導出しない。`.mcad` への保存は mcad-io が DTO で行う。

use std::f64::consts::PI;

use mcad_geom::solve::{ATOL_LEN, ResidualKind};
use mcad_geom::{Arc, EPS, LineSeg, Point2, Shape, Vec2};

use crate::{ConstraintId, CoreError, Document, EntityGeom, EntityId};

pub(crate) mod equations;

/// 図形の上の点の種類（[`PointRef`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PointKind {
    /// 始点（線分の `a`、円弧の開始角の点）。
    Start,
    /// 終点（線分の `b`、円弧の終了角の点）。
    End,
    /// 中心（円・円弧）。
    Center,
}

/// 拘束が指す「図形の上の点」。
///
/// 線分は [`PointKind::Start`]・[`PointKind::End`]、円は [`PointKind::Center`]、
/// 円弧は 3 つすべてを指せる。それ以外の組み合わせは
/// [`crate::Command::AddConstraint`] が [`CoreError::InvalidConstraint`] で拒否する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PointRef {
    /// 点を持つエンティティ。
    pub entity: EntityId,
    /// そのエンティティのどの点か。
    pub kind: PointKind,
}

impl PointRef {
    /// 対象と点の種類から作る。
    #[must_use]
    pub const fn new(entity: EntityId, kind: PointKind) -> Self {
        Self { entity, kind }
    }

    /// この点の現在のワールド座標。エンティティが無い、または図形がこの種類の点を
    /// 持たなければ `None`。
    #[must_use]
    pub fn position(&self, doc: &Document) -> Option<Point2> {
        point_of(&doc.entity(self.entity)?.geom, self.kind)
    }
}

/// 線分の向き（始点 → 終点）に対する側。値は必ず ±1 のどちらか（設計判断2「側の符号」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    /// 始点 → 終点の向きに対して左側（符号付き距離が正）。
    Left,
    /// 右側（符号付き距離が負）。
    Right,
}

impl Side {
    /// 符号付き距離に掛ける符号（左 = `+1.0`、右 = `−1.0`）。
    #[must_use]
    pub const fn sign(self) -> f64 {
        match self {
            Self::Left => 1.0,
            Self::Right => -1.0,
        }
    }
}

/// 内接する 2 円のどちらが大きいか（[`CircleTangency::Internal`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LargerCircle {
    /// 1 つ目（[`Constraint::TangentCircles`] の `a`）が大きい（`s = +1`）。
    First,
    /// 2 つ目（`b`）が大きい（`s = −1`）。
    Second,
}

impl LargerCircle {
    /// 内接の方程式 `‖c1 − c2‖ − s(r1 − r2)` の `s`。
    #[must_use]
    pub const fn sign(self) -> f64 {
        match self {
            Self::First => 1.0,
            Self::Second => -1.0,
        }
    }
}

/// 円どうしの接線の種類。作成時の形から決めて保存する（解くたびに決め直さない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CircleTangency {
    /// 外接（`‖c1 − c2‖ = r1 + r2`）。
    External,
    /// 内接（`‖c1 − c2‖ = s(r1 − r2)`）。`abs` の折れ目を避けるため、どちらが大きいかを
    /// 保存する。
    Internal(LargerCircle),
}

/// 幾何拘束 1 件（DESIGN.md M13 設計判断2）。
///
/// # フィールドの順
///
/// 同じ種類の対象を 2 つ取る拘束（一致・点どうしの距離・平行・直交・等長・等半径・角度・
/// 円どうしの接線）の `a`・`b` の**順に意味は無い**（方程式の符号と、角度の測り始め、内接で
/// どちらが大きいかの基準になるだけ）。「先に選んだ図形を止めて、後に選んだ図形を動かす」
/// という選択順は、拘束ではなく [`crate::Command::AddConstraint`] の `hold` で渡す。
///
/// # 値の単位
///
/// 距離・半径はワールド単位、角度はラジアン（UI では mm と度で出す）。
///
/// # 方程式
///
/// 各バリアントの doc に残差（= 0 で満たされる）を書く。線分 `ℓ` の方向は始点 → 終点、
/// 符号付き距離は「線分を延長した直線から点までの距離で、左側が正」。
///
/// 向きの拘束（水平・垂直・平行・直交）は線分の向きを区別しないので、角度を**周期 π で
/// 畳んだ**ラジアンを残差にする（角度型。許容値は `ATOL_ANG`）: `α` を線分の方向角
/// `atan2(dy, dx)`、`φ` を 2 線分のなす角（[`Constraint::Angle`] と同じ定義）として、
/// 0 と π で 0 になる `½·atan2(sin 2t, cos 2t)` か、±π/2 で 0 になる
/// `½·atan2(−sin 2t, −cos 2t)`。残差は解までの回転角で、解からちょうど 90° ずれた形でも
/// 勾配が 0 にならない（その形へ拘束を足しても解ける）。
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum Constraint {
    /// 一致: 2 点の x・y の差（方程式 2 本）。
    Coincident {
        /// 1 つ目の点。
        a: PointRef,
        /// 2 つ目の点。
        b: PointRef,
    },
    /// 水平: 方向角 `α` を畳んだ `½·atan2(sin 2α, cos 2α)`（`α = 0`・`π` で 0）。
    Horizontal {
        /// 対象の線分。
        line: EntityId,
    },
    /// 垂直: 方向角 `α` を畳んだ `½·atan2(−sin 2α, −cos 2α)`（`α = ±π/2` で 0）。
    Vertical {
        /// 対象の線分。
        line: EntityId,
    },
    /// 点どうしの距離: `‖a − b‖ − value`。`value` は長さ型の許容値より大きい正の値
    /// （0 にしたいときは [`Constraint::Coincident`]）。線分の長さは同じ線分の始点・終点で表す。
    Distance {
        /// 1 つ目の点。
        a: PointRef,
        /// 2 つ目の点。
        b: PointRef,
        /// 距離（`> 0`）。
        value: f64,
    },
    /// 点と線の距離: `point` の符号付き距離 − `side.sign() × value`。`value ≥ 0`。
    PointLineDistance {
        /// 対象の点（`line` とは別のエンティティの点）。
        point: PointRef,
        /// 対象の線分（延長した直線として扱う）。
        line: EntityId,
        /// 点のある側（作成時の形から決める。直線上なら [`Side::Left`]）。
        side: Side,
        /// 距離（`≥ 0`）。
        value: f64,
    },
    /// 平行: なす角 `φ` を畳んだ `½·atan2(sin 2φ, cos 2φ)`（`φ = 0`・`π` で 0。反平行も満たす）。
    Parallel {
        /// 1 つ目の線分。
        a: EntityId,
        /// 2 つ目の線分。
        b: EntityId,
    },
    /// 直交: なす角 `φ` を畳んだ `½·atan2(−sin 2φ, −cos 2φ)`（`φ = ±π/2` で 0）。
    Perpendicular {
        /// 1 つ目の線分。
        a: EntityId,
        /// 2 つ目の線分。
        b: EntityId,
    },
    /// 等長: 線分の長さの差。
    EqualLength {
        /// 1 つ目の線分。
        a: EntityId,
        /// 2 つ目の線分。
        b: EntityId,
    },
    /// 等半径: 円・円弧の半径の差。
    EqualRadius {
        /// 1 つ目の円・円弧。
        a: EntityId,
        /// 2 つ目の円・円弧。
        b: EntityId,
    },
    /// 半径: `r − value`。`value > 0`。
    Radius {
        /// 対象の円・円弧。
        entity: EntityId,
        /// 半径（`> 0`）。
        value: f64,
    },
    /// 角度: 線分 `a` の方向から `b` の方向へのなす角 `φ = atan2(d1 × d2, d1 · d2)` と
    /// `value` の差を `(−π, π]` へ主値化したもの。`value` は `(−π, π]` に正規化して格納する。
    Angle {
        /// 1 つ目の線分。
        a: EntityId,
        /// 2 つ目の線分。
        b: EntityId,
        /// 角度 `θ`（ラジアン、`(−π, π]`）。
        value: f64,
    },
    /// 線分と円・円弧の接線: 中心の符号付き距離 − `side.sign() × r`。線分は延長した直線、
    /// 円弧はそれを含む円として扱う（接点が区間の外でも満たす。[`Constraint::tangent_contact`]）。
    TangentLineCircle {
        /// 対象の線分。
        line: EntityId,
        /// 対象の円・円弧。
        circle: EntityId,
        /// 中心のある側（作成時の形から決める。中心が直線上なら [`Side::Left`]）。
        side: Side,
    },
    /// 円・円弧どうしの接線: 外接は `‖c1 − c2‖ − (r1 + r2)`、内接は `‖c1 − c2‖ − s(r1 − r2)`。
    /// 円弧はそれを含む円として扱う。
    TangentCircles {
        /// 1 つ目の円・円弧。
        a: EntityId,
        /// 2 つ目の円・円弧。
        b: EntityId,
        /// 外接か内接か（作成時の形から決める。同じ近さなら外接）。
        kind: CircleTangency,
    },
    /// 点の固定: `point − at`（方程式 2 本）。円弧の端点も固定できるよう、固定集合ではなく
    /// 方程式で表す。
    FixPoint {
        /// 対象の点。
        point: PointRef,
        /// 固定する座標 `p₀`（作成時のワールド座標）。
        at: Point2,
    },
    /// 図形の固定: その図形の全パラメータを固定集合へ入れる（方程式ではない）。
    /// 固定した図形の幾何を変える操作は [`CoreError::EntityFixed`] で拒否される。
    FixEntity {
        /// 対象の線分・円・円弧。
        entity: EntityId,
    },
}

/// 接線拘束の接点（[`Constraint::tangent_contact`]）。
///
/// 接線は「延長した直線・円弧を含む円」どうしで定義する（設計判断2）ため、接点が線分の
/// 区間や円弧の掃引範囲の外にあっても拘束は満たされる。表示側は `*_outside` を見て、
/// 接点から図形の最寄りの端点までを破線でつなぎ、一覧に「（延長上）」と添える。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TangentContact {
    /// 接点（[`Constraint::TangentLineCircle`] は中心から直線へ下ろした垂線の足、
    /// [`Constraint::TangentCircles`] は 1 つ目の円周上の、相手に面した点）。
    pub point: Point2,
    /// 1 つ目の対象（線と円の接線では線分、円どうしでは `a`）の区間・掃引範囲の外か。
    pub first_outside: bool,
    /// 2 つ目の対象（線と円の接線では円・円弧、円どうしでは `b`）の掃引範囲の外か。
    pub second_outside: bool,
}

impl TangentContact {
    /// どちらかの対象の延長上で接しているか。
    #[must_use]
    pub const fn is_extended(&self) -> bool {
        self.first_outside || self.second_outside
    }
}

// ---------------------------------------------------------------------------
// 図形の種類と、拘束が対象に求める役割
// ---------------------------------------------------------------------------

/// 拘束の対象にできる図形の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShapeKind {
    Line,
    Circle,
    Arc,
}

/// `geom` が拘束の対象にできる図形ならその種類。
pub(crate) fn shape_kind(geom: &EntityGeom) -> Option<ShapeKind> {
    match geom.as_shape()? {
        Shape::Line(_) => Some(ShapeKind::Line),
        Shape::Circle(_) => Some(ShapeKind::Circle),
        Shape::Arc(_) => Some(ShapeKind::Arc),
        Shape::Point(_) | Shape::Polyline(_) => None,
    }
}

/// 拘束が対象のエンティティに求める役割。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    /// 点を持つ図形（`Start`・`End` は線分・円弧、`Center` は円・円弧）。
    Point(PointKind),
    /// 線分。
    Line,
    /// 円・円弧。
    Round,
    /// 線分・円・円弧のどれか（図形の固定）。
    Any,
}

impl Role {
    /// 種類 `kind`（`None` は拘束の対象にできない図形）がこの役割を果たせるか。
    pub(crate) fn fits(self, kind: Option<ShapeKind>) -> bool {
        let Some(kind) = kind else {
            return false;
        };
        match self {
            Self::Point(PointKind::Start | PointKind::End) => {
                matches!(kind, ShapeKind::Line | ShapeKind::Arc)
            }
            Self::Point(PointKind::Center) | Self::Round => {
                matches!(kind, ShapeKind::Circle | ShapeKind::Arc)
            }
            Self::Line => kind == ShapeKind::Line,
            Self::Any => true,
        }
    }

    /// エラーメッセージ用の説明。
    fn describe(self) -> &'static str {
        match self {
            Self::Point(PointKind::Start) => "a start point (line or arc)",
            Self::Point(PointKind::End) => "an end point (line or arc)",
            Self::Point(PointKind::Center) => "a center (circle or arc)",
            Self::Line => "a line",
            Self::Round => "a circle or an arc",
            Self::Any => "a line, a circle or an arc",
        }
    }
}

/// 図形の上の点の座標。図形がその種類の点を持たなければ `None`。
pub(crate) fn point_of(geom: &EntityGeom, kind: PointKind) -> Option<Point2> {
    match (geom.as_shape()?, kind) {
        (Shape::Line(l), PointKind::Start) => Some(l.a),
        (Shape::Line(l), PointKind::End) => Some(l.b),
        (Shape::Circle(c), PointKind::Center) => Some(c.center),
        (Shape::Arc(a), PointKind::Start) => Some(a.start_point()),
        (Shape::Arc(a), PointKind::End) => Some(a.end_point()),
        (Shape::Arc(a), PointKind::Center) => Some(a.center),
        _ => None,
    }
}

fn invalid(msg: impl Into<String>) -> CoreError {
    CoreError::InvalidConstraint(msg.into())
}

/// `(−π, π]` へ正規化する。すでに範囲内なら値を変えない（丸めを持ち込まない）。
fn normalize_angle(a: f64) -> f64 {
    if a > -PI && a <= PI {
        return a;
    }
    let w = a.sin().atan2(a.cos());
    if w <= -PI { PI } else { w }
}

/// 円・円弧の中心・半径と、円弧なら掃引範囲。
#[derive(Debug, Clone, Copy)]
struct Round {
    center: Point2,
    radius: f64,
    arc: Option<Arc>,
}

impl Round {
    /// 中心から見た角度 `theta` の点が、円弧の CCW 掃引範囲の外か（円は常に範囲内）。
    fn outside(&self, dir: Vec2) -> bool {
        self.arc
            .is_some_and(|arc| !arc.contains_angle(dir.y.atan2(dir.x)))
    }
}

fn round_of(doc: &Document, id: EntityId) -> Option<Round> {
    match doc.entity(id)?.geom.as_shape()? {
        Shape::Circle(c) => Some(Round {
            center: c.center,
            radius: c.radius,
            arc: None,
        }),
        Shape::Arc(a) => Some(Round {
            center: a.center,
            radius: a.radius,
            arc: Some(*a),
        }),
        _ => None,
    }
}

fn line_of(doc: &Document, id: EntityId) -> Option<LineSeg> {
    match doc.entity(id)?.geom.as_shape()? {
        Shape::Line(l) => Some(*l),
        _ => None,
    }
}

/// 線分の長さと単位方向。長さが [`ATOL_LEN`] 未満（方向が定まらない）なら `None`。
fn unit_dir(seg: LineSeg) -> Option<(f64, Vec2)> {
    let d = seg.direction();
    let len = d.x.hypot(d.y);
    (len >= ATOL_LEN).then(|| (len, Vec2::new(d.x / len, d.y / len)))
}

/// 2 点の差のノルムと単位ベクトル。ノルムが [`ATOL_LEN`] 未満なら方向は `(1, 0)`
/// （設計判断2 のノルムの特異点の規則。方程式側と同じ）。
fn norm_dir(v: Vec2) -> (f64, Vec2) {
    let n = v.x.hypot(v.y);
    if n < ATOL_LEN {
        (n, Vec2::new(1.0, 0.0))
    } else {
        (n, Vec2::new(v.x / n, v.y / n))
    }
}

impl Constraint {
    // ---- 読み取り ----

    /// 対象のエンティティと、それぞれに求める役割（フィールド順。最大 2 つ）。
    pub(crate) fn targets(&self) -> impl Iterator<Item = (EntityId, Role)> + use<> {
        let point = |p: PointRef| (p.entity, Role::Point(p.kind));
        let pair: [Option<(EntityId, Role)>; 2] = match *self {
            Self::Coincident { a, b } | Self::Distance { a, b, .. } => {
                [Some(point(a)), Some(point(b))]
            }
            Self::Horizontal { line } | Self::Vertical { line } => [Some((line, Role::Line)), None],
            Self::PointLineDistance { point: p, line, .. } => {
                [Some(point(p)), Some((line, Role::Line))]
            }
            Self::Parallel { a, b }
            | Self::Perpendicular { a, b }
            | Self::EqualLength { a, b }
            | Self::Angle { a, b, .. } => [Some((a, Role::Line)), Some((b, Role::Line))],
            Self::EqualRadius { a, b } | Self::TangentCircles { a, b, .. } => {
                [Some((a, Role::Round)), Some((b, Role::Round))]
            }
            Self::Radius { entity, .. } => [Some((entity, Role::Round)), None],
            Self::TangentLineCircle { line, circle, .. } => {
                [Some((line, Role::Line)), Some((circle, Role::Round))]
            }
            Self::FixPoint { point: p, .. } => [Some(point(p)), None],
            Self::FixEntity { entity } => [Some((entity, Role::Any)), None],
        };
        pair.into_iter().flatten()
    }

    /// この拘束が参照するエンティティ（フィールド順、重複なし。1 つか 2 つ）。
    #[must_use]
    pub fn entities(&self) -> Vec<EntityId> {
        let mut out: Vec<EntityId> = Vec::with_capacity(2);
        for (entity, _) in self.targets() {
            if !out.contains(&entity) {
                out.push(entity);
            }
        }
        out
    }

    /// この拘束が `entity` を参照するか。
    #[must_use]
    pub fn references(&self, entity: EntityId) -> bool {
        self.targets().any(|(e, _)| e == entity)
    }

    /// 拘束の値（点どうしの距離・点と線の距離・半径はワールド単位、角度はラジアン）。
    /// 値を持たない拘束は `None`。[`crate::Command::SetConstraintValue`] で変えられるのは
    /// これが `Some` の拘束だけ。
    #[must_use]
    pub fn value(&self) -> Option<f64> {
        match *self {
            Self::Distance { value, .. }
            | Self::PointLineDistance { value, .. }
            | Self::Radius { value, .. }
            | Self::Angle { value, .. } => Some(value),
            _ => None,
        }
    }

    /// 値を差し替えた拘束。値を持たない拘束は `None`。検証・正規化はしない。
    pub(crate) fn with_value(&self, new_value: f64) -> Option<Self> {
        let mut out = self.clone();
        match &mut out {
            Self::Distance { value, .. }
            | Self::PointLineDistance { value, .. }
            | Self::Radius { value, .. }
            | Self::Angle { value, .. } => *value = new_value,
            _ => return None,
        }
        Some(out)
    }

    /// 格納する形へ正規化する（角度の値を `(−π, π]` へ）。
    pub(crate) fn normalized(mut self) -> Self {
        if let Self::Angle { value, .. } = &mut self {
            *value = normalize_angle(*value);
        }
        self
    }

    // ---- ミラー ----

    /// 図形の集合 `in_set` をミラー（鏡映）したとき、**ミラー後の形で同じ幾何関係を表す**
    /// 拘束を返す。何も変える必要が無ければ `self` と等しい値を返す。
    ///
    /// ミラーは向きを反転させるので、図形だけを [`crate::Command::ModifyEntity`] で鏡映すると、
    /// 側・角度の符号・円弧の端点を持つ拘束が元の関係を表さなくなる。呼び出し側は、
    /// ミラーする図形に関わる各拘束についてこれを呼び、`self` と等しくないものだけを
    /// [`crate::Command::ReplaceConstraint`] として、図形の `ModifyEntity` と同じ
    /// [`crate::Command::Batch`] に積む。
    ///
    /// 規則:
    ///
    /// - **集合内の円弧を指す点**（[`PointRef`]）は `Start` ↔ `End` を入れ替える
    ///   （[`mcad_geom::Arc::mirrored`] は CCW 掃引を保つために開始角と終了角を入れ替える。
    ///   点を物理的な点に追従させる）。相手の対象が集合内かどうかには依らない。点の固定
    ///   （[`Constraint::FixPoint`]）の点も同じ。線分は `mirrored` が始点・終点の順を保つので
    ///   入れ替えない。
    /// - **側**（点と線の距離・線と円の接線の [`Side`]）は、その拘束の 2 つの対象が**両方とも
    ///   集合内**のときだけ反転する。点と線の距離の値が 0（点が直線上）なら反転しない
    ///   （[`Constraint::point_line_distance`] と同じく `Left` のままにする）。
    /// - **角度**は、2 本の線分が両方とも集合内のときだけ符号を反転する（`(−π, π]` へ正規化。
    ///   `π` は `π` のまま）。
    /// - それ以外（水平・垂直・平行・直交・等長・等半径・半径・円どうしの接線の種類・
    ///   図形の固定）は変えない。**点の固定の座標 `at` はミラーしない**: 固定した点を動かす
    ///   操作は、平行移動と同じく拒否されるのが正しい（軸上の点なら動かないので通る）。
    ///
    /// 片方だけが集合内の拘束で側・角度を変えないのは、ミラーされなかった相手との関係が
    /// 元のまま残るため（その形で満たせなければ `apply` が解くか、拒否する）。
    ///
    /// `doc` は対象が円弧かどうかの判定にだけ使う。ミラーは図形の種類を変えないので、
    /// **ミラーの前に呼んでも後に呼んでも結果は同じ**。
    #[must_use]
    pub fn mirrored(&self, doc: &Document, in_set: impl Fn(EntityId) -> bool) -> Self {
        let point = |p: PointRef| {
            let is_arc = || {
                doc.entity(p.entity)
                    .is_some_and(|e| shape_kind(&e.geom) == Some(ShapeKind::Arc))
            };
            let kind = match p.kind {
                PointKind::Start if in_set(p.entity) && is_arc() => PointKind::End,
                PointKind::End if in_set(p.entity) && is_arc() => PointKind::Start,
                kind => kind,
            };
            PointRef::new(p.entity, kind)
        };
        let flip = |side: Side| match side {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        };
        match *self {
            Self::Coincident { a, b } => Self::Coincident {
                a: point(a),
                b: point(b),
            },
            Self::Distance { a, b, value } => Self::Distance {
                a: point(a),
                b: point(b),
                value,
            },
            Self::PointLineDistance {
                point: p,
                line,
                side,
                value,
            } => Self::PointLineDistance {
                point: point(p),
                line,
                side: if in_set(p.entity) && in_set(line) && value != 0.0 {
                    flip(side)
                } else {
                    side
                },
                value,
            },
            Self::Angle { a, b, value } if in_set(a) && in_set(b) => Self::Angle {
                a,
                b,
                value: normalize_angle(-value),
            },
            Self::TangentLineCircle { line, circle, side } if in_set(line) && in_set(circle) => {
                Self::TangentLineCircle {
                    line,
                    circle,
                    side: flip(side),
                }
            }
            Self::FixPoint { point: p, at } => Self::FixPoint {
                point: point(p),
                at,
            },
            Self::Angle { .. }
            | Self::TangentLineCircle { .. }
            | Self::Horizontal { .. }
            | Self::Vertical { .. }
            | Self::Parallel { .. }
            | Self::Perpendicular { .. }
            | Self::EqualLength { .. }
            | Self::EqualRadius { .. }
            | Self::Radius { .. }
            | Self::TangentCircles { .. }
            | Self::FixEntity { .. } => self.clone(),
        }
    }

    // ---- 検証 ----

    /// 構造と値を検証する（[`crate::Command::AddConstraint`] /
    /// [`crate::Command::SetConstraintValue`] の実行前チェック）。
    ///
    /// - 参照先のエンティティが存在し（無ければ [`CoreError::EntityNotFound`]）、種類が合う。
    /// - 同じ対象を 2 回指していない。
    /// - 値が有限で範囲内: 点どうしの距離は長さ型の許容値より大きい、点と線の距離は 0 以上、
    ///   半径は正、固定座標は有限。
    /// - 内接は `s(r1 − r2)` が長さ型の許容値より大きい（等半径の内接と、大小の食い違いを
    ///   拒否する）。
    ///
    /// 長さ型の許容値は、この拘束の方程式が現在の形から計算する値そのもので、判定の式を
    /// 別に持たない。形に依存する 2 つの条件（点どうしの距離・内接）は
    /// `equations::structure_holds` を通し、[`crate::Document::apply`] が解いた後の形に掛ける
    /// 検査と同じ式を使う。
    /// 現在の形が拘束を満たすかは見ない（[`Constraint::is_satisfied`]）。
    pub(crate) fn validate(&self, doc: &Document) -> Result<(), CoreError> {
        for (entity, role) in self.targets() {
            let geom = &doc
                .entity(entity)
                .ok_or(CoreError::EntityNotFound(entity))?
                .geom;
            if !role.fits(shape_kind(geom)) {
                return Err(invalid(format!(
                    "{entity:?} is not {} as the constraint requires",
                    role.describe()
                )));
            }
        }
        let distinct = |same: bool, what: &str| {
            if same {
                Err(invalid(format!(
                    "the constraint refers to the same {what} twice"
                )))
            } else {
                Ok(())
            }
        };
        let finite = |v: f64, what: &str| {
            if v.is_finite() {
                Ok(())
            } else {
                Err(invalid(format!("non-finite {what}: {v}")))
            }
        };
        match *self {
            Self::Coincident { a, b } => distinct(a == b, "point"),
            Self::Distance { a, b, value } => {
                distinct(a == b, "point")?;
                finite(value, "distance")?;
                if equations::structure_holds(doc, self)? {
                    Ok(())
                } else {
                    Err(invalid(format!(
                        "point distance must be positive (got {value}); use a coincident constraint for zero distance"
                    )))
                }
            }
            Self::PointLineDistance {
                point, line, value, ..
            } => {
                distinct(point.entity == line, "line")?;
                finite(value, "distance")?;
                if value >= 0.0 {
                    Ok(())
                } else {
                    Err(invalid(format!(
                        "point-line distance must not be negative: {value}"
                    )))
                }
            }
            Self::Parallel { a, b } | Self::Perpendicular { a, b } | Self::EqualLength { a, b } => {
                distinct(a == b, "line")
            }
            Self::Angle { a, b, value } => {
                distinct(a == b, "line")?;
                finite(value, "angle")
            }
            Self::EqualRadius { a, b } => distinct(a == b, "circle"),
            Self::Radius { value, .. } => {
                finite(value, "radius")?;
                if value > 0.0 {
                    Ok(())
                } else {
                    Err(invalid(format!("radius must be positive: {value}")))
                }
            }
            Self::TangentCircles { a, b, kind } => {
                distinct(a == b, "circle")?;
                if kind == CircleTangency::External || equations::structure_holds(doc, self)? {
                    Ok(())
                } else {
                    Err(invalid(
                        "internal tangency needs the circle marked as larger to have the larger radius",
                    ))
                }
            }
            Self::FixPoint { at, .. } => {
                finite(at.x, "fixed coordinate")?;
                finite(at.y, "fixed coordinate")
            }
            Self::Horizontal { .. }
            | Self::Vertical { .. }
            | Self::TangentLineCircle { .. }
            | Self::FixEntity { .. } => Ok(()),
        }
    }

    /// 現在の形がこの拘束を満たすか（解かずに残差だけを見る）。
    ///
    /// 判定は `mcad_geom::solve::is_satisfied` と同じ: 方程式ごとに、**その形から計算した
    /// 許容値**で割った残差の絶対値の最大が 1 以下。[`Document::apply`] が「解かずに通す」か
    /// どうかを決める判定・解いた後に確かめる判定と同じで（`apply` が受理した文書では、
    /// すべての拘束がこれを満たす）、`.mcad` 読込時の検査（設計判断5）にも使える。
    ///
    /// 次の形は、満たさないものとして `Ok(false)` を返す:
    ///
    /// - **参照する図形が退化している**（線分の長さ・円と円弧の半径が `ATOL_LEN` 未満）。
    ///   長さ 0 の線分でも等長・一致の残差は 0 になるが、ソルバーが扱えない形なので
    ///   満たされていない扱いにする。方程式を持たない [`Constraint::FixEntity`] も同じ。
    /// - 残差か許容値が有限でない（座標が極端に大きい）。
    ///
    /// # Errors
    ///
    /// 参照先が無い・種類が合わないとき（[`CoreError::EntityNotFound`] /
    /// [`CoreError::InvalidConstraint`]）。
    pub fn is_satisfied(&self, doc: &Document) -> Result<bool, CoreError> {
        let sys = equations::System::for_constraint(doc, self)?;
        Ok(sys.first_unsatisfied().is_none())
    }

    // ---- 現在の形から作るコンストラクタ ----

    /// 点どうしの距離の拘束を、**現在の距離**を値にして作る。
    ///
    /// # Errors
    ///
    /// 構造が不正なとき、または 2 点が長さ型の許容値以下で一致しているとき
    /// （[`CoreError::InvalidConstraint`]。一致拘束を使う）。
    pub fn distance(doc: &Document, a: PointRef, b: PointRef) -> Result<Self, CoreError> {
        let probe = Self::Coincident { a, b };
        probe.validate(doc)?;
        let (Some(p), Some(q)) = (a.position(doc), b.position(doc)) else {
            return Err(invalid("point is not available"));
        };
        let out = Self::Distance {
            a,
            b,
            value: (p.x - q.x).hypot(p.y - q.y),
        };
        out.validate(doc)?;
        Ok(out)
    }

    /// 点と線の距離の拘束を、**現在の側と距離**で作る。
    ///
    /// 点が直線上にある（符号付き距離の絶対値が長さ型の許容値以下）なら側は
    /// [`Side::Left`]、値は 0 にする（設計判断2「側の符号は必ず ±1」）。値を変えるには
    /// 返った拘束の `value` を書き換えるか、追加後に [`crate::Command::SetConstraintValue`]
    /// を使う。
    ///
    /// # Errors
    ///
    /// 構造が不正なとき、線分が退化していて側が決まらないとき
    /// （[`CoreError::InvalidConstraint`]）。
    pub fn point_line_distance(
        doc: &Document,
        point: PointRef,
        line: EntityId,
    ) -> Result<Self, CoreError> {
        let with = |side: Side, value: f64| Self::PointLineDistance {
            point,
            line,
            side,
            value,
        };
        with(Side::Left, 0.0).validate(doc)?;
        let sd = match (point.position(doc), line_of(doc, line)) {
            (Some(p), Some(seg)) => signed_distance(seg, p),
            _ => None,
        }
        .ok_or_else(|| invalid("the line is degenerate; its side cannot be determined"))?;
        // 値は代表長さに入るので、現在の距離を入れた拘束で許容値を計算する。
        let tol = equations::length_tolerance(doc, &with(Side::Left, sd.abs()))?;
        let out = if sd.abs() <= tol {
            with(Side::Left, 0.0)
        } else if sd > 0.0 {
            with(Side::Left, sd)
        } else {
            with(Side::Right, -sd)
        };
        out.validate(doc)?;
        Ok(out)
    }

    /// 半径の拘束を、**現在の半径**を値にして作る。
    ///
    /// # Errors
    ///
    /// 構造が不正なとき、半径が 0 のとき（[`CoreError::InvalidConstraint`]）。
    pub fn radius(doc: &Document, entity: EntityId) -> Result<Self, CoreError> {
        // 対象の存在と種類を先に確かめる（値は仮）。
        Self::Radius { entity, value: 1.0 }.validate(doc)?;
        let round =
            round_of(doc, entity).ok_or_else(|| invalid("target must be a circle or an arc"))?;
        let out = Self::Radius {
            entity,
            value: round.radius,
        };
        out.validate(doc)?;
        Ok(out)
    }

    /// 角度の拘束を、**現在のなす角** `φ = atan2(d1 × d2, d1 · d2)`（`(−π, π]`）を値にして作る。
    ///
    /// # Errors
    ///
    /// 構造が不正なとき、どちらかの線分が退化していて方向が定まらないとき
    /// （[`CoreError::InvalidConstraint`]）。
    pub fn angle(doc: &Document, a: EntityId, b: EntityId) -> Result<Self, CoreError> {
        Self::Angle { a, b, value: 0.0 }.validate(doc)?;
        let dirs = match (line_of(doc, a), line_of(doc, b)) {
            (Some(la), Some(lb)) => unit_dir(la).zip(unit_dir(lb)),
            _ => None,
        };
        let Some(((_, u1), (_, u2))) = dirs else {
            return Err(invalid("a line is degenerate; its direction is undefined"));
        };
        let phi = u1.cross(u2).atan2(u1.dot(u2));
        Ok(Self::Angle {
            a,
            b,
            value: normalize_angle(phi),
        })
    }

    /// 線分と円・円弧の接線の拘束を、**現在の中心の側**で作る。
    ///
    /// 中心が直線上にある（符号付き距離の絶対値が長さ型の許容値以下）なら
    /// [`Side::Left`]（設計判断2: 中心を通る補助線を接線にする作図を拒否しない）。
    /// 現在の形が接しているかは見ない。
    ///
    /// # Errors
    ///
    /// 構造が不正なとき、線分が退化していて側が決まらないとき
    /// （[`CoreError::InvalidConstraint`]）。
    pub fn tangent_line_circle(
        doc: &Document,
        line: EntityId,
        circle: EntityId,
    ) -> Result<Self, CoreError> {
        let with = |side: Side| Self::TangentLineCircle { line, circle, side };
        let probe = with(Side::Left);
        probe.validate(doc)?;
        let sd = match (line_of(doc, line), round_of(doc, circle)) {
            (Some(seg), Some(round)) => signed_distance(seg, round.center),
            _ => None,
        }
        .ok_or_else(|| invalid("the line is degenerate; its side cannot be determined"))?;
        let tol = equations::length_tolerance(doc, &probe)?;
        Ok(with(if sd < -tol { Side::Right } else { Side::Left }))
    }

    /// 円・円弧どうしの接線の拘束を、**現在の形に近い方**（外接か内接か）で作る。
    ///
    /// 中心間距離 `D` に対して `|D − (r1 + r2)|`（外接）と `|D − |r1 − r2||`（内接）を比べ、
    /// 小さい方を選ぶ。同じなら外接。内接を選んだときは、半径の大きい方を保存する。
    /// 現在の形が接しているかは見ない。
    ///
    /// # Errors
    ///
    /// 構造が不正なとき、内接を選んだが半径の差が長さ型の許容値以下のとき
    /// （等半径の内接 = 同心で同じ円。[`CoreError::InvalidConstraint`]）。
    pub fn tangent_circles(doc: &Document, a: EntityId, b: EntityId) -> Result<Self, CoreError> {
        let with = |kind: CircleTangency| Self::TangentCircles { a, b, kind };
        with(CircleTangency::External).validate(doc)?;
        let (Some(ra), Some(rb)) = (round_of(doc, a), round_of(doc, b)) else {
            return Err(invalid("tangent targets must be circles or arcs"));
        };
        let (dist, _) = norm_dir(ra.center - rb.center);
        let external = (dist - (ra.radius + rb.radius)).abs();
        let internal = (dist - (ra.radius - rb.radius).abs()).abs();
        if external <= internal {
            return Ok(with(CircleTangency::External));
        }
        let larger = if ra.radius >= rb.radius {
            LargerCircle::First
        } else {
            LargerCircle::Second
        };
        let out = with(CircleTangency::Internal(larger));
        out.validate(doc)?;
        Ok(out)
    }

    /// 点の固定の拘束を、**現在の座標**を `p₀` にして作る。
    ///
    /// # Errors
    ///
    /// 構造が不正なとき（[`CoreError::InvalidConstraint`] / [`CoreError::EntityNotFound`]）。
    pub fn fix_point(doc: &Document, point: PointRef) -> Result<Self, CoreError> {
        let probe = Self::FixPoint {
            point,
            at: Point2::ORIGIN,
        };
        probe.validate(doc)?;
        let at = point
            .position(doc)
            .ok_or_else(|| invalid("point is not available"))?;
        Ok(Self::FixPoint { point, at })
    }

    // ---- 接点 ----

    /// 接線拘束の接点と、接点が各対象の区間・掃引範囲の外にあるか。
    ///
    /// 接線拘束でない、対象が無い・種類が合わない、線分が退化しているときは `None`。
    ///
    /// - [`Constraint::TangentLineCircle`] の接点は中心から直線へ下ろした垂線の足。
    ///   線分の区間外は、足のパラメータ `t`（始点 0・終点 1）が `[0, 1]` を
    ///   [`mcad_geom::EPS`] を超えて外れること。円弧の掃引範囲は、保存した側から決まる
    ///   接点の方向（中心から見て、中心のある側の反対）で判定する。
    /// - [`Constraint::TangentCircles`] の接点は 1 つ目の円周上で、中心どうしを結ぶ直線上の点
    ///   （外接と「1 つ目が大きい内接」では 2 つ目の中心へ向かう側、「2 つ目が大きい内接」では
    ///   反対側）。同心（中心間距離が `ATOL_LEN` 未満）のときは方向を `(1, 0)` とする。
    ///
    /// 円弧の掃引範囲の判定は CCW 掃引（[`mcad_geom::Arc::contains_angle`]）。円は常に範囲内。
    /// 拘束が満たされていない形でも、上の定義どおりの点を返す。
    #[must_use]
    pub fn tangent_contact(&self, doc: &Document) -> Option<TangentContact> {
        match *self {
            Self::TangentLineCircle { line, circle, side } => {
                let seg = line_of(doc, line)?;
                let round = round_of(doc, circle)?;
                let (len, u) = unit_dir(seg)?;
                let along = (round.center - seg.a).dot(u);
                let t = along / len;
                // 満たされた形では 中心 = 足 + s·r·n（n は左法線）なので、中心から接点へは −s·n。
                let toward_contact = u.perp() * (-side.sign());
                Some(TangentContact {
                    point: seg.a + u * along,
                    first_outside: !(-EPS..=1.0 + EPS).contains(&t),
                    second_outside: round.outside(toward_contact),
                })
            }
            Self::TangentCircles { a, b, kind } => {
                let ra = round_of(doc, a)?;
                let rb = round_of(doc, b)?;
                // a の中心から b の中心への単位ベクトル。
                let (_, u) = norm_dir(rb.center - ra.center);
                let (dir_a, dir_b) = match kind {
                    CircleTangency::External => (u, -u),
                    CircleTangency::Internal(LargerCircle::First) => (u, u),
                    CircleTangency::Internal(LargerCircle::Second) => (-u, -u),
                };
                Some(TangentContact {
                    point: ra.center + dir_a * ra.radius,
                    first_outside: ra.outside(dir_a),
                    second_outside: rb.outside(dir_b),
                })
            }
            _ => None,
        }
    }
}

/// 線分を延長した直線から点 `p` までの符号付き距離（始点 → 終点の向きに対して左が正）。
/// 線分が退化していれば `None`。
fn signed_distance(seg: LineSeg, p: Point2) -> Option<f64> {
    let (_, u) = unit_dir(seg)?;
    Some(u.cross(p - seg.a))
}

// ---------------------------------------------------------------------------
// ReshapeEntity で消す拘束（設計判断4）
// ---------------------------------------------------------------------------

/// [`reshape_removals`] が切り詰め・延長を拒否した理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReshapeBlocked {
    /// 固定の拘束（図形の固定、または動いた点の固定）がある。
    Fixed(ConstraintId),
    /// 切り詰め・延長ではない: 支持する直線・円が変わる（理由の文言つき）。
    /// 動かす・変形するには [`crate::Command::ModifyEntity`] を使う。
    SupportChanged(&'static str),
}

/// 図形の大きさ（線分の長さ、円・円弧の半径。それ以外は 0）。
fn entity_size(geom: &EntityGeom) -> f64 {
    match geom.as_shape() {
        Some(Shape::Line(l)) => {
            let d = l.direction();
            d.x.hypot(d.y)
        }
        Some(Shape::Circle(c)) => c.radius.abs(),
        Some(Shape::Arc(a)) => a.radius.abs(),
        _ => 0.0,
    }
}

/// `ReshapeEntity` の判定に使う長さ型の許容値（設計判断1）: `L` = 変更前後の図形の大きさの
/// 最大、`C` = `points`（判定に関係する点）の座標の絶対値の最大。尺度が有限でなければ `None`。
///
/// 「点が動いたか」（[`point_moved`]）と「支持する直線・円が変わったか」
/// （[`check_same_support`]）が同じ式を使う。
fn reshape_tolerance(before: &EntityGeom, after: &EntityGeom, points: &[Point2]) -> Option<f64> {
    let size = entity_size(before).max(entity_size(after));
    let coord = points
        .iter()
        .fold(0.0_f64, |m, p| m.max(p.x.abs()).max(p.y.abs()));
    (ResidualKind::Length { size, coord }).tolerance()
}

/// `value`（0 以上の距離・差）が許容値 `tol` 以内か。NaN と、許容値が定まらない場合は
/// 「以内でない」に倒す。
fn within(value: f64, tol: Option<f64>) -> bool {
    tol.is_some_and(|tol| value <= tol)
}

/// 点 `kind` が切り詰めで「位置が変わった」か。
///
/// 変更後の図形がその点を持たない場合も `true`。動いたかは [`reshape_tolerance`]
/// （`C` は変更前後のその点）を超えたかで判定する。
fn point_moved(before: &EntityGeom, after: &EntityGeom, kind: PointKind) -> bool {
    let (Some(p), Some(q)) = (point_of(before, kind), point_of(after, kind)) else {
        return true;
    };
    let moved = (p.x - q.x).hypot(p.y - q.y);
    !within(moved, reshape_tolerance(before, after, &[p, q]))
}

/// `before` → `after` が切り詰め・延長（支持する直線・円が変わらない変更）かを確かめる。
///
/// - 線分 → 線分: 新しい両端点が、元の線分を延長した直線の上にあり（直線からの距離が
///   [`reshape_tolerance`] 以内。`C` は 4 つの端点）、向きが反転していない（元の方向との
///   内積が正。側の符号と角度の符号が向きに依存するため）。元の線分が退化していて直線が
///   定まらなければ拒否。
/// - 円 → 円弧、円弧 → 円弧: 中心と半径が変わらない（中心の移動・半径の差が
///   [`reshape_tolerance`] 以内。`C` は 2 つの中心）。
/// - それ以外の組み合わせ（円 → 円、円弧 → 円、種類の変わる変更、拘束の対象にできない
///   図形）は拒否。
fn check_same_support(before: &EntityGeom, after: &EntityGeom) -> Result<(), &'static str> {
    match (before.as_shape(), after.as_shape()) {
        (Some(Shape::Line(old)), Some(Shape::Line(new))) => {
            let (_, u) = unit_dir(*old).ok_or("the original line is degenerate")?;
            let tol = reshape_tolerance(before, after, &[old.a, old.b, new.a, new.b]);
            let off_line = |p: Point2| u.cross(p - old.a).abs();
            if !within(off_line(new.a), tol) || !within(off_line(new.b), tol) {
                return Err("the new end points are off the original line");
            }
            // NaN も「反転」に倒す。
            if u.dot(new.direction()) > 0.0 {
                Ok(())
            } else {
                Err("the line direction is reversed or the line collapses to a point")
            }
        }
        (Some(Shape::Circle(_)), Some(Shape::Arc(_)))
        | (Some(Shape::Arc(_)), Some(Shape::Arc(_))) => {
            let (Some(c0), Some(c1)) = (
                point_of(before, PointKind::Center),
                point_of(after, PointKind::Center),
            ) else {
                return Err("the center is not available");
            };
            let tol = reshape_tolerance(before, after, &[c0, c1]);
            if !within((c0.x - c1.x).hypot(c0.y - c1.y), tol) {
                return Err("the center moves");
            }
            if !within((entity_size(before) - entity_size(after)).abs(), tol) {
                return Err("the radius changes");
            }
            Ok(())
        }
        _ => Err("the kind of geometry is not a trim or an extension of the original"),
    }
}

/// [`crate::Command::ReshapeEntity`] で `entity` の幾何を `before` から `after` へ
/// 切り詰め・延長するとき、消す拘束の ID を `constraints` の順で返す（設計判断4）。
///
/// `constraints` のうち `entity` を参照しないものは無視する。規則:
///
/// - 幾何が変わらない、または `entity` を参照する拘束が 1 件も無ければ、何も確かめず
///   何も消さない（拘束の無い図形では `ModifyEntity` と同じ）。
/// - `entity` を参照する拘束があるときは、**支持する直線・円が変わらない変更だけ**を
///   受け付ける（[`check_same_support`]）。平行移動・回転・向きの反転・中心や半径の変更は
///   [`ReshapeBlocked::SupportChanged`] で拒否し、拘束は消さない（`ReshapeEntity` を
///   移動に使うと、端点の拘束だけが黙って消えて向きの拘束が残るため）。
/// - **図形の固定**がある、または**点の固定**の点が動いたなら拒否する
///   （[`ReshapeBlocked::Fixed`]。最初に見つかった固定の拘束を返す）。
/// - 動いた点（[`point_moved`]）を指す一致・点どうしの距離・点と線の距離（点の側）は消す。
/// - 等長は、その線分の端点のどちらかが動いたら消す（切り詰め・延長は必ず端点を動かす。
///   端点が動いていなければ長さも変わっていないので残す）。
/// - 向き・無限直線・半径に依存する拘束（水平・垂直・平行・直交・角度・点と線の距離の
///   「線」側・接線・半径・等半径）は残す。支持する直線・円が変わっていないので、
///   満たされたままになる。
///
/// 受け付ける変更（線分 → 線分、円 → 円弧、円弧 → 円弧）では、既存の拘束が指す点・役割は
/// 変更後の図形にも必ずある（円は `Start`・`End` を指されていない）ので、「種類が変わって
/// 指せなくなった拘束」は生じない。
///
/// 2 つの対象がどちらも `entity` を指す拘束（同じ線分の始点・終点の距離など）は、
/// どちらか一方でも消す条件に当たれば消す。
pub(crate) fn reshape_removals<'a>(
    entity: EntityId,
    before: &EntityGeom,
    after: &EntityGeom,
    constraints: impl IntoIterator<Item = (ConstraintId, &'a Constraint)>,
) -> Result<Vec<ConstraintId>, ReshapeBlocked> {
    if before == after {
        return Ok(Vec::new());
    }
    let own: Vec<(ConstraintId, &Constraint)> = constraints
        .into_iter()
        .filter(|(_, c)| c.references(entity))
        .collect();
    if own.is_empty() {
        return Ok(Vec::new());
    }
    check_same_support(before, after).map_err(ReshapeBlocked::SupportChanged)?;

    let after_kind = shape_kind(after);
    let end_moved = || {
        point_moved(before, after, PointKind::Start) || point_moved(before, after, PointKind::End)
    };
    let mut removed = Vec::new();
    for (id, constraint) in own {
        let mut remove = false;
        for (target, role) in constraint.targets() {
            if target != entity {
                continue;
            }
            debug_assert!(
                role.fits(after_kind),
                "check_same_support admits only changes that keep every referenced role"
            );
            remove |= match role {
                Role::Any => return Err(ReshapeBlocked::Fixed(id)),
                Role::Point(kind) => {
                    let moved = point_moved(before, after, kind);
                    if moved && matches!(constraint, Constraint::FixPoint { .. }) {
                        return Err(ReshapeBlocked::Fixed(id));
                    }
                    moved
                }
                Role::Line => matches!(constraint, Constraint::EqualLength { .. }) && end_moved(),
                Role::Round => false,
            };
        }
        if remove {
            removed.push(id);
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests;
