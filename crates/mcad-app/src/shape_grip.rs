//! 図形のグリップ編集（M13 タスク101、DESIGN.md M13 設計判断7）。
//!
//! 選択中の線分・円・円弧に出すグリップの一覧（[`shape_grips`]）、グリップを動かした後の
//! 形（[`apply_shape_grip`]）、グリップを出す対象の絞り込み（[`shape_grip_targets`]）を置く。
//! ドラッグの状態とプレビューのキャッシュは [`crate::tool::SelectTool`]、当たり判定と描画は
//! `main.rs`（寸法のグリップと同じ枠組み）。
//!
//! # 各グリップが動かすもの
//!
//! 求解の固定集合 (c)（設計判断3。点の単位・f64 の完全一致で「値が変わったパラメータ」を
//! 止める）に直結するので、**グリップが変えるパラメータ以外は bit 単位で同じまま**にする。
//! 線分の片端を動かしたとき、もう片端が自由なまま残って拘束に従って動くのはこのため。
//!
//! | グリップ | 動かすもの |
//! |---|---|
//! | 線分の始点・終点 | その端点だけ |
//! | 円・円弧の中心 | 中心だけ（半径・角度は不変） |
//! | 半径（円は四半円点 4 か所、円弧は掃引の中央 1 か所） | 半径だけ（カーソルと中心の距離） |
//! | 円弧の始点・終点 | その角度だけ（カーソルの方向。カーソルは円周へ射影する） |

use std::collections::BTreeSet;
use std::f64::consts::TAU;

use mcad_core::{Constraint, Document, Entity, EntityGeom, EntityId};
use mcad_geom::{Arc, Circle, EPS, LineSeg, Point2, Shape, point_tol};

use crate::tool::layer_visible;

/// 選択がこの数を超えたら図形のグリップを出さない。矩形選択で大量に選んだとき、毎フレームの
/// 描画と押下時の当たり判定に図形ごとのグリップ列挙が乗るのを避ける（拘束の無い図面の
/// 操作・性能を M12 から変えない。M13 検収基準 (xiii)）。寸法のグリップは対象外。
pub const SHAPE_GRIP_SELECTION_LIMIT: usize = 100;

/// 図形 1 件が持つグリップの識別子。列挙順は [`shape_grips`] の順で、グリップどうしが
/// 重なったときのタイブレーク（先に定義したものが勝つ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeGripKind {
    /// 線分の始点。
    LineStart,
    /// 線分の終点。
    LineEnd,
    /// 円・円弧の中心。
    Center,
    /// 円弧の始点。
    ArcStart,
    /// 円弧の終点。
    ArcEnd,
    /// 半径のグリップ。`index` は位置の区別で、円は 0°/90°/180°/270° の 0〜3、円弧は 0。
    Radius { index: u8 },
}

/// `geom` のグリップの一覧（種別と現在のワールド座標）。線分・円・円弧のほかは空
/// （ポリライン・点・構築線・文字・寸法・表は図形のグリップの対象外）。
#[must_use]
pub fn shape_grips(geom: &EntityGeom) -> Vec<(ShapeGripKind, Point2)> {
    match geom.as_shape() {
        Some(Shape::Line(l)) => vec![
            (ShapeGripKind::LineStart, l.a),
            (ShapeGripKind::LineEnd, l.b),
        ],
        Some(Shape::Circle(c)) => {
            let mut grips = vec![(ShapeGripKind::Center, c.center)];
            grips.extend(
                (0_u8..)
                    .zip(c.quadrant_points())
                    .map(|(index, point)| (ShapeGripKind::Radius { index }, point)),
            );
            grips
        }
        Some(Shape::Arc(a)) => vec![
            (ShapeGripKind::Center, a.center),
            (ShapeGripKind::ArcStart, a.start_point()),
            (ShapeGripKind::ArcEnd, a.end_point()),
            (
                ShapeGripKind::Radius { index: 0 },
                a.circle().point_at_angle(a.start_angle + a.sweep() / 2.0),
            ),
        ],
        _ => Vec::new(),
    }
}

/// グリップ `kind` をカーソル `cursor`（スナップ適用済み）へ動かした後の形（モジュール doc の
/// 表）。プレビューと確定が同じこの関数を通る。`tol` はピック許容量（ワールド単位）。
///
/// # Errors
///
/// 退化する形はソルバーへ渡す前に理由つきで拒否する: 線分の両端が重なる、半径が 0 に
/// 近い（カーソルが中心にある）、円弧の両端が重なる。「重なる」は画面上で区別できない距離
/// （`tol` 未満）で判定する。手で動かすカーソルが数値の許容量まで一致することはまず無く、
/// 数値の許容量だけで判定すると、見た目で重ねても拒否されず極端に短い図形や全周に近い円弧が
/// できるため。掃引は開始角から終了角への反時計回りなので、端点が反対側へ回り込むのは
/// 許す。`geom` が `kind` のグリップを持たない組合せも拒否する（ドラッグ中に図形が変わった
/// 場合の防御）。
pub fn apply_shape_grip(
    geom: &EntityGeom,
    kind: ShapeGripKind,
    cursor: Point2,
    tol: f64,
) -> Result<EntityGeom, String> {
    let shape = match (geom.as_shape(), kind) {
        (Some(Shape::Line(l)), ShapeGripKind::LineStart) => line(cursor, l.b, tol)?,
        (Some(Shape::Line(l)), ShapeGripKind::LineEnd) => line(l.a, cursor, tol)?,
        (Some(Shape::Circle(c)), ShapeGripKind::Center) => {
            Shape::Circle(Circle::new(cursor, c.radius))
        }
        (Some(Shape::Circle(c)), ShapeGripKind::Radius { .. }) => {
            Shape::Circle(Circle::new(c.center, radius_to(c.center, cursor, tol)?))
        }
        (Some(Shape::Arc(a)), ShapeGripKind::Center) => {
            Shape::Arc(Arc::new(cursor, a.radius, a.start_angle, a.end_angle))
        }
        (Some(Shape::Arc(a)), ShapeGripKind::Radius { .. }) => Shape::Arc(Arc::new(
            a.center,
            radius_to(a.center, cursor, tol)?,
            a.start_angle,
            a.end_angle,
        )),
        (Some(Shape::Arc(a)), ShapeGripKind::ArcStart) => arc(
            Arc::new(
                a.center,
                a.radius,
                angle_to(a.center, cursor, tol)?,
                a.end_angle,
            ),
            tol,
        )?,
        (Some(Shape::Arc(a)), ShapeGripKind::ArcEnd) => arc(
            Arc::new(
                a.center,
                a.radius,
                a.start_angle,
                angle_to(a.center, cursor, tol)?,
            ),
            tol,
        )?,
        _ => return Err("この図形にはこのグリップがありません".to_owned()),
    };
    let new_geom = EntityGeom::Shape(shape);
    new_geom.validate()?;
    Ok(new_geom)
}

fn line(a: Point2, b: Point2, tol: f64) -> Result<Shape, String> {
    if a.distance(b) < tol.max(point_tol(a, b)) {
        return Err("線分の両端が重なります".to_owned());
    }
    Ok(Shape::Line(LineSeg::new(a, b)))
}

fn radius_to(center: Point2, cursor: Point2, tol: f64) -> Result<f64, String> {
    let radius = center.distance(cursor);
    if radius < tol.max(point_tol(center, cursor)) {
        return Err("半径が 0 になります".to_owned());
    }
    Ok(radius)
}

fn angle_to(center: Point2, cursor: Point2, tol: f64) -> Result<f64, String> {
    if center.distance(cursor) < tol.max(point_tol(center, cursor)) {
        return Err("円弧の中心では角度が決まりません".to_owned());
    }
    let d = cursor - center;
    Ok(d.y.atan2(d.x))
}

fn arc(a: Arc, tol: f64) -> Result<Shape, String> {
    // 角度の許容量は `Arc::contains_angle` と同じ `EPS`。`TAU` 側も、数値の上では掃引 0 と
    // 区別できない。弦の長さで見るので、掃引が 0 に近い側も全周に近い側も同じに拒否する。
    if !(EPS..=TAU - EPS).contains(&a.sweep()) || a.start_point().distance(a.end_point()) < tol {
        return Err("円弧の両端が重なります".to_owned());
    }
    Ok(Shape::Arc(a))
}

/// ドラッグ中のグリップを描く位置。ふつうはカーソルそのもので、円弧の始点・終点だけ円周へ
/// 射影する（確定する形の端点と揃える）。
#[must_use]
pub fn dragged_grip_point(geom: &EntityGeom, kind: ShapeGripKind, cursor: Point2) -> Point2 {
    if let (Some(Shape::Arc(a)), ShapeGripKind::ArcStart | ShapeGripKind::ArcEnd) =
        (geom.as_shape(), kind)
        && let Ok(angle) = angle_to(a.center, cursor, 0.0)
    {
        return a.circle().point_at_angle(angle);
    }
    cursor
}

/// `selection` のうち図形のグリップを出すもの（選択順）: 線分・円・円弧で、可視で、ロック
/// されていないレイヤーにあり、図形の固定（[`Constraint::FixEntity`]）が付いていないもの。
/// 選択が [`SHAPE_GRIP_SELECTION_LIMIT`] を超えたら空。
#[must_use]
pub fn shape_grip_targets<'a>(
    document: &'a Document,
    selection: &[EntityId],
) -> Vec<(EntityId, &'a Entity)> {
    if selection.is_empty() || selection.len() > SHAPE_GRIP_SELECTION_LIMIT {
        return Vec::new();
    }
    // 図形ごとに `constraints_of` を呼ぶと 図形数 × 拘束数 になるので、先に 1 回だけ走査する。
    let fixed: BTreeSet<EntityId> = document
        .constraints()
        .filter_map(|(_, constraint)| match constraint {
            Constraint::FixEntity { entity } => Some(*entity),
            _ => None,
        })
        .collect();
    selection
        .iter()
        .filter(|id| !fixed.contains(id))
        .filter_map(|&id| document.entity(id).map(|entity| (id, entity)))
        .filter(|(_, entity)| {
            matches!(
                entity.geom.as_shape(),
                Some(Shape::Line(_) | Shape::Circle(_) | Shape::Arc(_))
            ) && layer_visible(document, entity)
                && !document.layer(entity.layer).is_some_and(|l| l.locked)
        })
        .collect()
}

#[cfg(test)]
mod app_tests;
#[cfg(test)]
mod tests;
