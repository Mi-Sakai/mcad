//! `mcad-geom` — GUI 非依存の幾何プリミティブと計算。
//!
//! ワールド座標は f64（CAD は座標精度が命。f32 は大きな図面で破綻する）。
//! ここに置く型・関数はすべて純関数として実装し、egui/eframe などの
//! GUI 依存を一切持たない。
//!
//! # 公開 API の概要
//!
//! - 基本型: [`Point2`], [`Vec2`], [`Aabb`]
//! - プリミティブ: [`LineSeg`], [`Circle`], [`Arc`], [`Polyline`]
//! - それらをまとめる列挙型 [`Shape`]（mcad-core の `Entity { geom: Shape, .. }` が使う）
//! - 幾何クエリ: [`closest_point`], [`distance_to`], [`intersect`], [`circumcircle`]
//! - ジオメトリ検証: [`Shape::validate`]（非有限座標・負半径・空ポリラインなどを検出）
//!
//! すべての形状は [`Shape::aabb`]（および各プリミティブの `aabb` メソッド）で
//! 軸並行境界ボックスを算出でき、ビューポートカリング・矩形選択に使う。

mod aabb;
mod intersect;
mod point;
mod primitives;
mod vec2;

pub use aabb::Aabb;
pub use intersect::intersect;
pub use point::Point2;
pub use primitives::{
    Arc, Circle, LineSeg, Polyline, Shape, circumcircle, closest_point, distance_to,
};
pub use vec2::Vec2;

/// 幾何述語で用いる基準となる相対イプシロン。
///
/// # 値の根拠
///
/// `f64` の機械イプシロン（`f64::EPSILON` ≒ 2.22e-16）は、内積・外積のような
/// 座標の積を伴う計算では丸め誤差が桁上がりする。座標の絶対値が最大でも
/// 1e6 程度（一般的な図面のワールド寸法）だと仮定すると、積の相対誤差は
/// おおよそ `座標^2 * f64::EPSILON` ≒ 1e12 * 2.22e-16 ≒ 2e-4（絶対値）まで
/// 膨らみ得る。これを相対イプシロンに換算すると 1e-9 程度が
/// 「機械誤差より十分大きく、かつ意味のある最小フィーチャ寸法より十分小さい」
/// 安全域に収まる。
///
/// この値は以下の用途で **相対的に** 用いる（絶対値そのままでは使わない）:
/// - 平行判定: `|cross(r, s)| <= EPS * |r| * |s|`（= 角の sin が EPS 未満）
/// - 接線判定・内外判定: 半径や辺長でスケールした許容量と比較
/// - パラメータ [0, 1] 空間の境界許容: 無次元なので `EPS` を直接使用
/// - 角度（ラジアン）許容: 半径 R に対し弧長 `EPS * R` 相当なので相対的
pub const EPS: f64 = 1e-9;
