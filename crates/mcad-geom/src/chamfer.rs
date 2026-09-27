//! 直線どうしの面取り（[`chamfer_lines`]）。DESIGN.md M12 設計判断6。
//!
//! 対象は [`LineSeg`] × [`LineSeg`] のみ（ポリライン角・円弧を含む面取りは M12 の
//! スコープ外。設計判断10）。2 本の線分と 2 つの距離、そして各線分上のクリック点から、
//! コーナーを直線で落とした「面（bevel）」を作る。トリム済みの 2 線分と、それらを
//! つなぐ面の線分を返す。
//!
//! # [`fillet_lines`](crate::fillet_lines) との違い（意図的）
//!
//! フィレットは「残す側」を接点（オフセットした 2 直線の交点から各直線への垂線の足）
//! を基準に決める。面取りにはオフセットに相当する分岐選択が無く、接点の代わりに
//! コーナー `P`（2 直線の交点）だけがある。そこで面取りは **残す側をコーナー `P` を
//! 基準に決める**: クリック点 `near` が直線上のパラメータで `P` より手前か奥かを見て、
//! 奥側（`near` のある側）を残す。フィレットのようにクリック点が削られる区間内に
//! 入っていても、コーナー基準なので自然な側が選ばれる（フィレットは接点基準のため、
//! 削られる区間内をクリックすると意図と逆の側が残ることがある。面取りではこの非対称を
//! 避ける）。
//!
//! # 数値ロバスト性
//!
//! 平行判定・退化判定（ゼロ長線分・非有限座標）は [`fillet_lines`](crate::fillet_lines)
//! と同じ相対イプシロン方針。コーナーの交点計算は既存の
//! [`crate::line_line_intersection`]（無限直線どうしの交点、退化・平行で `None`）に
//! 委ねる。無次元パラメータ `t ∈ [0, 1]` に対する境界判定は [`crate::EPS`] を直接使い、
//! ワールド座標のまま比較する量（線分の退化、残る線分の長さ、側の判定）は
//! [`crate::point_tol`] / [`crate::rel_tol`] による相対許容量で判定する
//! （[`crate::fillet`] と同じ流儀）。

use crate::{EPS, LineSeg, Point2, line_line_intersection, point_tol, rel_tol};

/// 面取りが結果を作れない理由。
///
/// geom は GUI 非依存なので理由の列挙のみを返し、ユーザー向け文言は持たない
/// （[`crate::FilletError`] と同じパターン）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChamferError {
    /// 2 本の直線が平行（既存の相対 EPS 判定）でコーナーが定まらない。
    Parallel,
    /// 距離（`dist_a`/`dist_b` のいずれか）が正の有限値でない。
    NonPositiveDistance,
    /// 面取り点がどちらかの元の線分の区間 `[0, 1]` の外に出る（この距離に対して
    /// 線分が短い、あるいは線分がコーナーまで届いていない）。**線分を延長してまで
    /// 面取り点は作らない**（[`crate::FilletError::RadiusTooLarge`] と同じ方針）。
    DistanceTooLarge,
    /// 結果が退化して確定できない。ゼロ長・非有限座標の入力、クリック点がコーナーと
    /// 実質一致していて「残す側」を決められない場合、および残る線分が実質長さ 0 に
    /// なる場合。
    Degenerate,
}

/// [`chamfer_lines`] の結果。
///
/// レイヤー・スタイルの継承（新しい面の線分は 1 本目 `a` のものを継承する）は app 層の
/// 責務であり、ここでは幾何だけを返す（[`crate::FilletResult`] と同じ設計）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChamferResult {
    /// `near_a` 側を残してコーナー側の端点を面取り点 `C_a` へ差し替えた `a`。
    /// 元の `a` の向き（`a.a` → `a.b`）は保存する。
    pub trimmed_a: LineSeg,
    /// `near_b` 側を残してコーナー側の端点を面取り点 `C_b` へ差し替えた `b`。
    pub trimmed_b: LineSeg,
    /// 2 つの面取り点 `C_a` → `C_b` を結ぶ面の線分。
    pub bevel: LineSeg,
}

/// 2 本の線分 `a`・`b` を、コーナーからそれぞれ `dist_a`・`dist_b` の位置で結ぶ面取り
/// （直線）を作る。
///
/// `near_a` は直線 `a` 上のクリック点、`near_b` は直線 `b` 上のクリック点で、
/// それぞれ **面取り後に残したい側** を示す。コーナー `P` は `a`・`b` を無限直線と
/// みなした交点。`near_a` が `P` より手前か奥かで残す側を決める（モジュール doc の
/// 「fillet との違い」参照）。
///
/// 面取り点 `C_a` は `P` から `near_a` 側へ距離 `dist_a` 進んだ、直線 `a` 上の点
/// （`C_b` も同様に `dist_b`）。
///
/// # Errors
///
/// - [`ChamferError::NonPositiveDistance`] — `dist_a`/`dist_b` のいずれかが正の有限値
///   でない。
/// - [`ChamferError::Parallel`] — 2 直線が平行。
/// - [`ChamferError::DistanceTooLarge`] — 面取り点が元の線分の区間の外に出る。
/// - [`ChamferError::Degenerate`] — 入力が退化している（ゼロ長線分・非有限座標）、
///   クリック点がコーナーと実質一致して側を決められない、または結果の線分が実質長さ
///   0 になる。
pub fn chamfer_lines(
    a: LineSeg,
    b: LineSeg,
    dist_a: f64,
    dist_b: f64,
    near_a: Point2,
    near_b: Point2,
) -> Result<ChamferResult, ChamferError> {
    if !dist_a.is_finite() || dist_a <= 0.0 || !dist_b.is_finite() || dist_b <= 0.0 {
        return Err(ChamferError::NonPositiveDistance);
    }
    if ![a.a, a.b, b.a, b.b, near_a, near_b]
        .iter()
        .all(|p| p.x.is_finite() && p.y.is_finite())
    {
        return Err(ChamferError::Degenerate);
    }

    let (da, db) = (a.direction(), b.direction());
    let (la, lb) = (da.length(), db.length());
    // 退化判定は端点の座標スケールに対して相対（geom 横断の共通基準）。
    if la <= point_tol(a.a, a.b) || lb <= point_tol(b.a, b.b) {
        return Err(ChamferError::Degenerate);
    }
    // 平行判定: なす角の sin が EPS 未満（`fillet_lines` と同じ形）。
    if da.cross(db).abs() <= EPS * la * lb {
        return Err(ChamferError::Parallel);
    }

    // コーナー P は無限直線どうしの交点。上で平行・退化は排除済みなので必ず求まる。
    let corner = line_line_intersection(&a, &b).ok_or(ChamferError::Parallel)?;

    let ta_corner = param_on(&a, corner, la);
    let tb_corner = param_on(&b, corner, lb);
    let ta_near = param_on(&a, near_a, la);
    let tb_near = param_on(&b, near_b, lb);

    let ca = chamfer_point(&a, la, ta_corner, ta_near, dist_a, corner)?;
    let cb = chamfer_point(&b, lb, tb_corner, tb_near, dist_b, corner)?;

    // 残す側はコーナー基準（モジュール doc）。`chamfer_point` で near とコーナーの
    // 一致は排除済みなので、比較は常に確定する。
    let trimmed_a = trim_to_point(&a, ca.point, ta_near < ta_corner)?;
    let trimmed_b = trim_to_point(&b, cb.point, tb_near < tb_corner)?;

    Ok(ChamferResult {
        trimmed_a,
        trimmed_b,
        bevel: LineSeg::new(ca.point, cb.point),
    })
}

/// `p` を `seg` の無限直線へ射影したパラメータ（`seg.a` が 0、`seg.b` が 1）。
///
/// `len` は `seg.direction().length()`（呼び出し側で [`crate::point_tol`] 超と検査済み）。
fn param_on(seg: &LineSeg, p: Point2, len: f64) -> f64 {
    (p - seg.a).dot(seg.direction()) / (len * len)
}

/// 面取り点の計算結果（実座標）。
struct ChamferPoint {
    point: Point2,
}

/// コーナー `corner`（パラメータ `t_corner`）から `near`（パラメータ `t_near`）側へ
/// 距離 `dist` 進んだ、`seg` の無限直線上の面取り点を求める。
///
/// `near` が `corner` と実質一致していて側が決まらなければ
/// [`ChamferError::Degenerate`]、結果のパラメータが区間 `[0, 1]`（境界許容込み）の
/// 外に出れば [`ChamferError::DistanceTooLarge`]。
fn chamfer_point(
    seg: &LineSeg,
    len: f64,
    t_corner: f64,
    t_near: f64,
    dist: f64,
    corner: Point2,
) -> Result<ChamferPoint, ChamferError> {
    // corner から near までのワールド距離（同一直線上なので |Δt| * len）。
    let along = (t_near - t_corner) * len;
    if along.abs() <= point_tol(corner, seg.a + seg.direction() * t_near) {
        return Err(ChamferError::Degenerate);
    }
    let sign = along.signum();
    let t = t_corner + sign * dist / len;
    let unit = -EPS..=1.0 + EPS;
    if !unit.contains(&t) {
        return Err(ChamferError::DistanceTooLarge);
    }
    let clamped = t.clamp(0.0, 1.0);
    Ok(ChamferPoint {
        point: seg.a + seg.direction() * clamped,
    })
}

/// コーナー側の端点を面取り点へ差し替えた線分。`near` 側の端点は維持する。
///
/// `keep_start` は `near` がコーナーより下側（`seg.a` 寄り）にあるか。真なら `seg.a` を、
/// 偽なら `seg.b` を残す。[`crate::fillet`] の `trim_to_tangent` と違い面取り点ではなく
/// コーナーを基準にするので、削られる区間内をクリックしても `near` 側が残る。
fn trim_to_point(seg: &LineSeg, point: Point2, keep_start: bool) -> Result<LineSeg, ChamferError> {
    let trimmed = if keep_start {
        LineSeg::new(seg.a, point)
    } else {
        LineSeg::new(point, seg.b)
    };
    // 残る側が実質長さ 0 になる結果は確定しない（`fillet_lines` と同じ規約）。
    if trimmed.length()
        <= rel_tol(seg.length() + trimmed.a.to_vec2().length() + trimmed.b.to_vec2().length())
    {
        return Err(ChamferError::Degenerate);
    }
    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    fn seg(ax: f64, ay: f64, bx: f64, by: f64) -> LineSeg {
        LineSeg::new(pt(ax, ay), pt(bx, by))
    }

    fn approx_pt(a: Point2, b: Point2) {
        assert!(a.distance(b) < 1e-9, "{a:?} != {b:?}");
    }

    /// 直角コーナー（L 字）、等距離。面取り点はコーナーからそれぞれ dist だけ手前。
    #[test]
    fn right_angle_corner_with_equal_distances() {
        let a = seg(0.0, 0.0, 10.0, 0.0);
        let b = seg(10.0, 0.0, 10.0, 10.0);
        let res = chamfer_lines(a, b, 2.0, 2.0, pt(2.0, 0.0), pt(10.0, 8.0)).unwrap();

        approx_pt(res.trimmed_a.a, pt(0.0, 0.0));
        approx_pt(res.trimmed_a.b, pt(8.0, 0.0));
        approx_pt(res.trimmed_b.a, pt(10.0, 2.0));
        approx_pt(res.trimmed_b.b, pt(10.0, 10.0));
        approx_pt(res.bevel.a, pt(8.0, 0.0));
        approx_pt(res.bevel.b, pt(10.0, 2.0));
    }

    /// 非直交コーナー（60 度）、非対称な 2 距離。
    /// 削られる区間内（コーナーと面取り点の間）をクリックしても、残す側はコーナー基準で
    /// 決まるので結果は区間外をクリックしたときと同じ（fillet の接点基準との意図的な違い）。
    #[test]
    fn clicking_inside_the_removed_part_keeps_the_same_side() {
        let a = seg(0.0, 0.0, 10.0, 0.0);
        let b = seg(10.0, 0.0, 10.0, 10.0);
        let outside = chamfer_lines(a, b, 3.0, 3.0, pt(2.0, 0.0), pt(10.0, 8.0)).unwrap();
        let inside = chamfer_lines(a, b, 3.0, 3.0, pt(9.0, 0.0), pt(10.0, 1.0)).unwrap();
        assert_eq!(inside, outside);
        approx_pt(inside.trimmed_a.a, pt(0.0, 0.0));
        approx_pt(inside.trimmed_a.b, pt(7.0, 0.0));
        approx_pt(inside.trimmed_b.a, pt(10.0, 3.0));
        approx_pt(inside.trimmed_b.b, pt(10.0, 10.0));
    }

    #[test]
    fn oblique_corner_with_asymmetric_distances() {
        let dir = (
            std::f64::consts::FRAC_PI_3.cos(),
            std::f64::consts::FRAC_PI_3.sin(),
        );
        let a = seg(0.0, 0.0, 10.0, 0.0);
        let b = seg(0.0, 0.0, 10.0 * dir.0, 10.0 * dir.1);
        let res =
            chamfer_lines(a, b, 3.0, 1.5, pt(8.0, 0.0), pt(8.0 * dir.0, 8.0 * dir.1)).unwrap();

        approx_pt(res.trimmed_a.a, pt(3.0, 0.0));
        approx_pt(res.trimmed_a.b, pt(10.0, 0.0));
        approx_pt(res.trimmed_b.a, pt(1.5 * dir.0, 1.5 * dir.1));
        approx_pt(res.trimmed_b.b, pt(10.0 * dir.0, 10.0 * dir.1));
        approx_pt(res.bevel.a, pt(3.0, 0.0));
        approx_pt(res.bevel.b, pt(1.5 * dir.0, 1.5 * dir.1));
    }

    /// 十字交差。`near_a`/`near_b` の符号の組み合わせ 4 通りが、それぞれ対応する
    /// 象限の解を選ぶ。
    #[test]
    fn near_points_select_the_matching_quadrant() {
        let a = seg(-10.0, 0.0, 10.0, 0.0);
        let b = seg(0.0, -10.0, 0.0, 10.0);
        for (sx, sy) in [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)] {
            let res = chamfer_lines(a, b, 2.0, 2.0, pt(5.0 * sx, 0.0), pt(0.0, 5.0 * sy)).unwrap();
            let ca = pt(2.0 * sx, 0.0);
            let cb = pt(0.0, 2.0 * sy);
            approx_pt(res.bevel.a, ca);
            approx_pt(res.bevel.b, cb);
            let keep_a = if sx > 0.0 {
                LineSeg::new(ca, pt(10.0, 0.0))
            } else {
                LineSeg::new(pt(-10.0, 0.0), ca)
            };
            let keep_b = if sy > 0.0 {
                LineSeg::new(cb, pt(0.0, 10.0))
            } else {
                LineSeg::new(pt(0.0, -10.0), cb)
            };
            approx_pt(res.trimmed_a.a, keep_a.a);
            approx_pt(res.trimmed_a.b, keep_a.b);
            approx_pt(res.trimmed_b.a, keep_b.a);
            approx_pt(res.trimmed_b.b, keep_b.b);
        }
    }

    /// コーナーが線分の外にある（延長しないと届かない）配置でも、面取り点が
    /// 線分上に収まっていれば通す。
    #[test]
    fn corner_outside_the_segments_is_allowed_when_chamfer_points_fit() {
        // 交点は (0,0) だが、どちらの線分もそこまで届いていない。
        let a = seg(5.0, 0.0, 10.0, 0.0);
        let b = seg(0.0, 5.0, 0.0, 10.0);
        // near から見てコーナーは反対側（負のパラメータ方向）なので、
        // 面取り点は線分の外へ進む。コーナーからの距離を線分の始点より内側に収める。
        let res = chamfer_lines(a, b, 8.0, 8.0, pt(8.0, 0.0), pt(0.0, 8.0)).unwrap();
        approx_pt(res.bevel.a, pt(8.0, 0.0));
        approx_pt(res.bevel.b, pt(0.0, 8.0));
    }

    #[test]
    fn parallel_lines_are_rejected() {
        let a = seg(0.0, 0.0, 10.0, 0.0);
        let b = seg(0.0, 5.0, 10.0, 5.0);
        assert_eq!(
            chamfer_lines(a, b, 1.0, 1.0, pt(2.0, 0.0), pt(2.0, 5.0)),
            Err(ChamferError::Parallel)
        );
        let c = seg(10.0, 5.0, 0.0, 5.0);
        assert_eq!(
            chamfer_lines(a, c, 1.0, 1.0, pt(2.0, 0.0), pt(2.0, 5.0)),
            Err(ChamferError::Parallel)
        );
    }

    #[test]
    fn non_positive_distance_is_rejected() {
        let a = seg(0.0, 0.0, 10.0, 0.0);
        let b = seg(10.0, 0.0, 10.0, 10.0);
        for d in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                chamfer_lines(a, b, d, 1.0, pt(2.0, 0.0), pt(10.0, 8.0)),
                Err(ChamferError::NonPositiveDistance),
                "dist_a {d}"
            );
            assert_eq!(
                chamfer_lines(a, b, 1.0, d, pt(2.0, 0.0), pt(10.0, 8.0)),
                Err(ChamferError::NonPositiveDistance),
                "dist_b {d}"
            );
        }
    }

    #[test]
    fn distance_larger_than_segment_is_rejected() {
        let a = seg(0.0, 0.0, 1.0, 0.0);
        let b = seg(1.0, 0.0, 1.0, 1.0);
        assert_eq!(
            chamfer_lines(a, b, 5.0, 0.5, pt(0.1, 0.0), pt(1.0, 0.9)),
            Err(ChamferError::DistanceTooLarge)
        );
        assert_eq!(
            chamfer_lines(a, b, 0.5, 5.0, pt(0.1, 0.0), pt(1.0, 0.9)),
            Err(ChamferError::DistanceTooLarge)
        );
    }

    /// コーナーが線分の外で、かつ面取り点も線分に届かない配置は拒否する。
    #[test]
    fn corner_outside_the_segments_and_unreachable_is_rejected() {
        let a = seg(5.0, 0.0, 10.0, 0.0);
        let b = seg(0.0, 5.0, 0.0, 10.0);
        assert_eq!(
            chamfer_lines(a, b, 1.0, 1.0, pt(8.0, 0.0), pt(0.0, 8.0)),
            Err(ChamferError::DistanceTooLarge)
        );
    }

    #[test]
    fn degenerate_segments_are_rejected() {
        let a = seg(0.0, 0.0, 0.0, 0.0);
        let b = seg(10.0, 0.0, 10.0, 10.0);
        assert_eq!(
            chamfer_lines(a, b, 1.0, 1.0, pt(0.0, 0.0), pt(10.0, 8.0)),
            Err(ChamferError::Degenerate)
        );
        assert_eq!(
            chamfer_lines(b, a, 1.0, 1.0, pt(10.0, 8.0), pt(0.0, 0.0)),
            Err(ChamferError::Degenerate)
        );
    }

    #[test]
    fn non_finite_input_is_rejected() {
        let a = seg(0.0, 0.0, 10.0, 0.0);
        let b = seg(10.0, 0.0, 10.0, 10.0);
        assert_eq!(
            chamfer_lines(a, b, 1.0, 1.0, pt(f64::NAN, 0.0), pt(10.0, 8.0)),
            Err(ChamferError::Degenerate)
        );
        let bad = seg(10.0, 0.0, f64::INFINITY, 10.0);
        assert_eq!(
            chamfer_lines(a, bad, 1.0, 1.0, pt(2.0, 0.0), pt(10.0, 8.0)),
            Err(ChamferError::Degenerate)
        );
    }

    /// クリック点がコーナーそのものと実質一致すると、残す側が決まらない。
    #[test]
    fn clicking_the_corner_itself_is_ambiguous() {
        let a = seg(-10.0, 0.0, 10.0, 0.0);
        let b = seg(0.0, -10.0, 0.0, 10.0);
        assert_eq!(
            chamfer_lines(a, b, 1.0, 1.0, pt(0.0, 0.0), pt(0.0, 5.0)),
            Err(ChamferError::Degenerate)
        );
        assert_eq!(
            chamfer_lines(a, b, 1.0, 1.0, pt(5.0, 0.0), pt(0.0, 0.0)),
            Err(ChamferError::Degenerate)
        );
    }

    /// 面取り点がちょうど残したい端点に重なると、残る線分が消えるので拒否する。
    #[test]
    fn vanishing_trimmed_segment_is_rejected() {
        let a = seg(0.0, 0.0, 10.0, 0.0);
        let b = seg(0.0, 0.0, 0.0, 10.0);
        assert_eq!(
            chamfer_lines(a, b, 10.0, 1.0, pt(10.0, 0.0), pt(0.0, 5.0)),
            Err(ChamferError::Degenerate)
        );
    }
}
