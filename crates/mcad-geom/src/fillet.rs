//! 直線どうしのフィレット（[`fillet_lines`]）。DESIGN.md M7 設計判断4。
//!
//! 対象は [`LineSeg`] × [`LineSeg`] のみ（円弧を含むフィレットは M7 のスコープ外。
//! 設計判断1）。2 本の線分と半径、そして各線分上のクリック点から、4 通りある
//! 接円のうち 1 つを一意に選び、トリム済みの 2 線分と接続する弧を返す。
//!
//! # 分岐の一意化
//!
//! クリック点 `near_a` は直線 a **上** の点なので、a 自身から見た「側」を決める
//! 情報を持たない。そこで **相手側のクリック点で自分のオフセット方向を決める**:
//! a を `near_b` のある側へ、b を `near_a` のある側へそれぞれ距離 `radius` だけ
//! オフセットし、その交点をフィレット円の中心とする（設計判断4）。`near_a` /
//! `near_b` はユーザーが「残したい側」をクリックする点なので、この規則で
//! 「コーナーを削る側」の解が選ばれる。
//!
//! # 数値ロバスト性
//!
//! 平行判定は既存と同じ相対イプシロン（`|cross| <= EPS * |r| * |s|`、なす角の
//! sin 相当）を使い、接点の区間判定は無次元のパラメータ `t ∈ [0, 1]` に対して
//! [`crate::EPS`] を直接使う（[`crate::trim_extend`] と同じ流儀）。
//! オフセット方向の決定は [`LineSeg::offset`] にそのまま委ねるので、
//! 「側」の意味づけは M5 のオフセットと完全に一致する。

use std::f64::consts::PI;

use crate::primitives::wrap_2pi;
use crate::{Arc, EPS, LineSeg, OffsetError, Point2, Vec2};

/// フィレットが結果を作れない理由。
///
/// geom は GUI 非依存なので理由の列挙のみを返し、ユーザー向け文言は持たない
/// （UI 側 mcad-app が ASCII ステータスメッセージへ対応付ける）。
/// [`crate::OffsetError`] / [`crate::TrimExtendError`] と同じパターン。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilletError {
    /// 2 本の直線が平行（既存の相対 EPS 判定）でコーナーが定まらない。
    Parallel,
    /// 半径が正の有限値でない。
    NonPositiveRadius,
    /// 接点が元の線分の区間 `[0, 1]` の外に出る（この半径に対して線分が短い、
    /// あるいは線分がコーナーまで届いていない）。**線分を延長してまで接点は作らない**
    /// （M5 のオフセット退化拒否と一貫させた方針。設計判断4）。
    RadiusTooLarge,
    /// 結果が退化して確定できない。ゼロ長・非有限座標の入力、クリック点が
    /// 相手の直線上にあってオフセット方向を決められない場合（＝コーナーそのものを
    /// クリックした場合）、および残る線分が実質長さ 0 になる場合。
    Degenerate,
}

/// [`fillet_lines`] の結果。
///
/// レイヤー・スタイルの継承（新しい弧は 1 本目 `a` のものを継承する）は app 層の
/// 責務であり、ここでは幾何だけを返す（設計判断4）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FilletResult {
    /// `near_a` 側を残してコーナー側の端点を接点 `T_a` へ差し替えた `a`。
    /// 元の `a` の向き（`a.a` → `a.b`）は保存する。
    pub trimmed_a: LineSeg,
    /// `near_b` 側を残してコーナー側の端点を接点 `T_b` へ差し替えた `b`。
    pub trimmed_b: LineSeg,
    /// 2 つの接点をつなぐフィレット弧。掃引は必ず π 以下
    /// （＝コーナー側に張り出す短い方の弧）で、両端点は `T_a` / `T_b` に一致する。
    pub arc: Arc,
}

/// 2 本の線分 `a`・`b` を半径 `radius` の弧で丸める。
///
/// `near_a` は直線 `a` 上のクリック点、`near_b` は直線 `b` 上のクリック点で、
/// それぞれ **フィレット後に残したい側** を示す。この 2 点は
/// 「相手直線のオフセット方向（＝中心の分岐選択）」と「自分自身のトリム後に
/// 残る側」の 2 つの役割を兼ねる（設計判断4）。
///
/// 中心 `C` は、`a` を `near_b` のある側へ、`b` を `near_a` のある側へ距離
/// `radius` だけオフセットした 2 直線の交点。接点 `T_a` / `T_b` は `C` から
/// 各直線への垂線の足になる。
///
/// # Errors
///
/// - [`FilletError::NonPositiveRadius`] — `radius` が正の有限値でない。
/// - [`FilletError::Parallel`] — 2 直線が平行。
/// - [`FilletError::RadiusTooLarge`] — 接点が元の線分の区間の外に出る。
/// - [`FilletError::Degenerate`] — 入力が退化している（ゼロ長線分・非有限座標）、
///   クリック点が相手の直線上でオフセット方向を決められない、または結果の線分が
///   実質長さ 0 になる。
pub fn fillet_lines(
    a: LineSeg,
    b: LineSeg,
    radius: f64,
    near_a: Point2,
    near_b: Point2,
) -> Result<FilletResult, FilletError> {
    if !radius.is_finite() || radius <= 0.0 {
        return Err(FilletError::NonPositiveRadius);
    }
    if ![a.a, a.b, b.a, b.b, near_a, near_b]
        .iter()
        .all(|p| p.x.is_finite() && p.y.is_finite())
    {
        return Err(FilletError::Degenerate);
    }

    let (da, db) = (a.direction(), b.direction());
    let (la, lb) = (da.length(), db.length());
    if la <= EPS || lb <= EPS {
        return Err(FilletError::Degenerate);
    }
    // 平行判定: なす角の sin が EPS 未満（`intersect` の seg_seg と同じ形）。
    if da.cross(db).abs() <= EPS * la * lb {
        return Err(FilletError::Parallel);
    }

    // 相手のクリック点が自分の直線上に乗っていると「側」が決まらない
    // （＝コーナーそのものをクリックした場合）。`LineSeg::offset` は境界で
    // 左法線を選ぶが、ここで黙って片側を選ぶと解が恣意的になるので拒否する。
    if !has_definite_side(&a, near_b) || !has_definite_side(&b, near_a) {
        return Err(FilletError::Degenerate);
    }

    // オフセット方向の意味づけは M5 のオフセットへ委ねる（最近点 → `toward` の向き）。
    let a_off = a.offset(radius, near_b).map_err(offset_error)?;
    let b_off = b.offset(radius, near_a).map_err(offset_error)?;
    // オフセット後も方向は元と同じなので、交点は必ず存在する（平行は上で排除済み）。
    let center = line_line_intersection(a_off.a, da, b_off.a, db).ok_or(FilletError::Parallel)?;

    // 接点は中心から各直線への垂線の足。パラメータが区間外なら延長せず拒否する。
    let ta = param_on(&a, center, la);
    let tb = param_on(&b, center, lb);
    let unit = -EPS..=1.0 + EPS;
    if !unit.contains(&ta) || !unit.contains(&tb) {
        return Err(FilletError::RadiusTooLarge);
    }
    let tangent_a = a.a + da * ta.clamp(0.0, 1.0);
    let tangent_b = b.a + db * tb.clamp(0.0, 1.0);

    let arc = corner_arc(center, radius, tangent_a, tangent_b)?;
    let trimmed_a = trim_to_tangent(&a, tangent_a, ta, param_on(&a, near_a, la))?;
    let trimmed_b = trim_to_tangent(&b, tangent_b, tb, param_on(&b, near_b, lb))?;

    Ok(FilletResult {
        trimmed_a,
        trimmed_b,
        arc,
    })
}

/// [`LineSeg::offset`] のエラーを [`FilletError`] へ写す。
///
/// `radius` と線分の退化は呼び出し前に検査済みなので、実際にはどれも到達しない。
fn offset_error(err: OffsetError) -> FilletError {
    match err {
        OffsetError::NonPositiveDistance => FilletError::NonPositiveRadius,
        _ => FilletError::Degenerate,
    }
}

/// `p` が `seg` を含む無限直線のどちら側にあるかを確実に判定できるか。
///
/// 直線からの符号付き距離が座標スケールに対して相対的に 0 とみなせる場合は
/// 「側」が決まらない。許容量は [`crate::trim_extend`] と同じ相対形。
fn has_definite_side(seg: &LineSeg, p: Point2) -> bool {
    let d = seg.direction();
    let len = d.length();
    let offset = p - seg.a;
    let dist = offset.cross(d).abs() / len;
    dist > EPS * (1.0 + len + offset.length())
}

/// 無限直線 `p + t·r` と `q + u·s` の交点。平行なら `None`。
fn line_line_intersection(p: Point2, r: Vec2, q: Point2, s: Vec2) -> Option<Point2> {
    let denom = r.cross(s);
    if denom.abs() <= EPS * r.length() * s.length() {
        return None;
    }
    Some(p + r * ((q - p).cross(s) / denom))
}

/// `p` を `seg` の無限直線へ射影したパラメータ（`seg.a` が 0、`seg.b` が 1）。
///
/// `len` は `seg.direction().length()`（呼び出し側で `EPS` 超と検査済み）。
fn param_on(seg: &LineSeg, p: Point2, len: f64) -> f64 {
    (p - seg.a).dot(seg.direction()) / (len * len)
}

/// 2 つの接点を結ぶ、掃引 π 以下の弧（＝コーナー側に張り出す短い方）。
fn corner_arc(
    center: Point2,
    radius: f64,
    tangent_a: Point2,
    tangent_b: Point2,
) -> Result<Arc, FilletError> {
    let theta_a = (tangent_a - center).angle();
    let theta_b = (tangent_b - center).angle();
    let sweep = wrap_2pi(theta_b - theta_a);
    // 2 直線が平行でなければ接点半径のなす角は (0, π) に入る。掃引 0 は保険。
    if sweep <= EPS {
        return Err(FilletError::Degenerate);
    }
    Ok(if sweep <= PI {
        Arc::new(center, radius, theta_a, theta_b)
    } else {
        Arc::new(center, radius, theta_b, theta_a)
    })
}

/// コーナー側の端点を接点へ差し替えた線分。`near` 側の端点は維持する。
///
/// `t_tangent` / `t_near` は `seg` 上のパラメータ。`near` が接点より下側
/// （`seg.a` 寄り）なら `seg.a` を残し、そうでなければ `seg.b` を残す。
fn trim_to_tangent(
    seg: &LineSeg,
    tangent: Point2,
    t_tangent: f64,
    t_near: f64,
) -> Result<LineSeg, FilletError> {
    let trimmed = if t_near < t_tangent {
        LineSeg::new(seg.a, tangent)
    } else {
        LineSeg::new(tangent, seg.b)
    };
    // 残る側が実質長さ 0 になる結果は確定しない（M5 の退化拒否規約）。
    if trimmed.length() <= EPS * seg.length() {
        return Err(FilletError::Degenerate);
    }
    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_2, FRAC_PI_3, SQRT_2};

    const TOL: f64 = 1e-9;

    fn pt(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    fn seg(ax: f64, ay: f64, bx: f64, by: f64) -> LineSeg {
        LineSeg::new(pt(ax, ay), pt(bx, by))
    }

    fn approx_pt(a: Point2, b: Point2) {
        assert!(a.distance(b) < 1e-9, "{a:?} != {b:?}");
    }

    /// 弧の共通不変条件: 両端点が接点に一致し、掃引が π 以下、半径が要求どおり。
    fn assert_arc_joins(res: &FilletResult, radius: f64, tangent_a: Point2, tangent_b: Point2) {
        assert!((res.arc.radius - radius).abs() < TOL);
        assert!(res.arc.sweep() <= PI + TOL, "sweep {}", res.arc.sweep());
        let (s, e) = (res.arc.start_point(), res.arc.end_point());
        let joins = (s.distance(tangent_a) < 1e-9 && e.distance(tangent_b) < 1e-9)
            || (s.distance(tangent_b) < 1e-9 && e.distance(tangent_a) < 1e-9);
        assert!(
            joins,
            "arc ends {s:?}/{e:?} vs tangents {tangent_a:?}/{tangent_b:?}"
        );
    }

    // --- 解析解との比較（直角・鋭角・鈍角） ---

    /// 直角コーナー（L 字、`a.b == b.a`）。接線長 = radius なので手計算できる。
    #[test]
    fn right_angle_corner_matches_analytic_solution() {
        let a = seg(0.0, 0.0, 10.0, 0.0);
        let b = seg(10.0, 0.0, 10.0, 10.0);
        let res = fillet_lines(a, b, 2.0, pt(2.0, 0.0), pt(10.0, 8.0)).unwrap();

        // 中心 (8, 2)、接点 (8, 0) と (10, 2)（接線長 = radius = 2）。
        approx_pt(res.arc.center, pt(8.0, 2.0));
        approx_pt(res.trimmed_a.a, pt(0.0, 0.0));
        approx_pt(res.trimmed_a.b, pt(8.0, 0.0));
        approx_pt(res.trimmed_b.a, pt(10.0, 2.0));
        approx_pt(res.trimmed_b.b, pt(10.0, 10.0));
        assert_arc_joins(&res, 2.0, pt(8.0, 0.0), pt(10.0, 2.0));
        assert!((res.arc.sweep() - FRAC_PI_2).abs() < TOL);

        // 弧の中点はコーナー (10, 0) 側へ張り出す（＝短い方の弧が選ばれている）。
        let mid = res
            .arc
            .circle()
            .point_at_angle(res.arc.start_angle + res.arc.sweep() / 2.0);
        approx_pt(mid, pt(8.0 + SQRT_2, 2.0 - SQRT_2));
    }

    /// 鋭角コーナー（60 度）。接線長 = r / tan(30°) = r·√3、中心距離 = r / sin(30°) = 2r。
    #[test]
    fn acute_corner_matches_analytic_solution() {
        let dir = (FRAC_PI_3.cos(), FRAC_PI_3.sin());
        let a = seg(0.0, 0.0, 10.0, 0.0);
        let b = seg(0.0, 0.0, 10.0 * dir.0, 10.0 * dir.1);
        let r = 1.0;
        let res = fillet_lines(a, b, r, pt(8.0, 0.0), pt(8.0 * dir.0, 8.0 * dir.1)).unwrap();

        let tan_len = 3.0f64.sqrt();
        let tangent_a = pt(tan_len, 0.0);
        let tangent_b = pt(tan_len * dir.0, tan_len * dir.1);
        // 中心は二等分線（30 度）上、距離 2r。
        approx_pt(
            res.arc.center,
            pt(2.0 * (PI / 6.0).cos(), 2.0 * (PI / 6.0).sin()),
        );
        approx_pt(res.trimmed_a.a, tangent_a);
        approx_pt(res.trimmed_a.b, pt(10.0, 0.0));
        approx_pt(res.trimmed_b.a, tangent_b);
        approx_pt(res.trimmed_b.b, pt(10.0 * dir.0, 10.0 * dir.1));
        assert_arc_joins(&res, r, tangent_a, tangent_b);
        // 接点半径のなす角 = π − コーナー角 = 120 度。
        assert!((res.arc.sweep() - 2.0 * FRAC_PI_3).abs() < TOL);
    }

    /// 鈍角コーナー（120 度）。接線長 = r / tan(60°) = r/√3、中心距離 = r / sin(60°)。
    #[test]
    fn obtuse_corner_matches_analytic_solution() {
        let theta = 2.0 * FRAC_PI_3;
        let dir = (theta.cos(), theta.sin());
        let a = seg(0.0, 0.0, 10.0, 0.0);
        let b = seg(0.0, 0.0, 10.0 * dir.0, 10.0 * dir.1);
        let r = 1.0;
        let res = fillet_lines(a, b, r, pt(8.0, 0.0), pt(8.0 * dir.0, 8.0 * dir.1)).unwrap();

        let tan_len = 1.0 / 3.0f64.sqrt();
        let tangent_a = pt(tan_len, 0.0);
        let tangent_b = pt(tan_len * dir.0, tan_len * dir.1);
        let center_dist = 1.0 / FRAC_PI_3.sin();
        approx_pt(
            res.arc.center,
            pt(center_dist * FRAC_PI_3.cos(), center_dist * FRAC_PI_3.sin()),
        );
        approx_pt(res.trimmed_a.a, tangent_a);
        approx_pt(res.trimmed_b.a, tangent_b);
        assert_arc_joins(&res, r, tangent_a, tangent_b);
        // π − 120 度 = 60 度。
        assert!((res.arc.sweep() - FRAC_PI_3).abs() < TOL);
    }

    // --- 分岐選択（4 象限） ---

    /// 原点で交差する十字。`near_a` / `near_b` の符号の組み合わせ 4 通りが、
    /// それぞれ対応する象限の解を選ぶ（設計判断4 の分岐一意化）。
    #[test]
    fn near_points_select_the_matching_quadrant() {
        let a = seg(-10.0, 0.0, 10.0, 0.0);
        let b = seg(0.0, -10.0, 0.0, 10.0);
        let r = 2.0;
        for (sx, sy) in [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)] {
            let res = fillet_lines(a, b, r, pt(5.0 * sx, 0.0), pt(0.0, 5.0 * sy)).unwrap();
            // 中心は near 側の象限（＝残る 2 本の交わる側）。
            approx_pt(res.arc.center, pt(2.0 * sx, 2.0 * sy));
            let tangent_a = pt(2.0 * sx, 0.0);
            let tangent_b = pt(0.0, 2.0 * sy);
            assert_arc_joins(&res, r, tangent_a, tangent_b);
            // 残るのは near 側の半分。
            let keep_a = if sx > 0.0 {
                LineSeg::new(tangent_a, pt(10.0, 0.0))
            } else {
                LineSeg::new(pt(-10.0, 0.0), tangent_a)
            };
            let keep_b = if sy > 0.0 {
                LineSeg::new(tangent_b, pt(0.0, 10.0))
            } else {
                LineSeg::new(pt(0.0, -10.0), tangent_b)
            };
            approx_pt(res.trimmed_a.a, keep_a.a);
            approx_pt(res.trimmed_a.b, keep_a.b);
            approx_pt(res.trimmed_b.a, keep_b.a);
            approx_pt(res.trimmed_b.b, keep_b.b);
        }
    }

    /// `near_a` だけを反転すると、中心が別の象限へ移る（4 分岐のうち 2 つの直接比較）。
    #[test]
    fn swapping_one_near_point_moves_the_solution() {
        let a = seg(-10.0, 0.0, 10.0, 0.0);
        let b = seg(0.0, -10.0, 0.0, 10.0);
        let right = fillet_lines(a, b, 2.0, pt(5.0, 0.0), pt(0.0, 5.0)).unwrap();
        let left = fillet_lines(a, b, 2.0, pt(-5.0, 0.0), pt(0.0, 5.0)).unwrap();
        approx_pt(right.arc.center, pt(2.0, 2.0));
        approx_pt(left.arc.center, pt(-2.0, 2.0));
        assert_ne!(right.arc.center, left.arc.center);
    }

    // --- L 字コーナーの非拒否（設計判断4 の采配役確認事項B-2） ---

    /// 端点で接続する典型的な L 字は、接線長が線分長を超えない限り
    /// `RadiusTooLarge` にならない（`[0, 1]` 判定の基準は `a.a`/`a.b` そのまま）。
    #[test]
    fn l_corner_is_not_rejected_as_too_large() {
        let a = seg(0.0, 0.0, 10.0, 0.0);
        let b = seg(10.0, 0.0, 10.0, 10.0);
        // 接線長 = radius なので、線分長 10 に対し radius < 10 なら通る。
        for r in [0.001, 0.5, 2.0, 5.0, 9.5, 9.999] {
            let res = fillet_lines(a, b, r, pt(1.0, 0.0), pt(10.0, 9.0));
            assert!(res.is_ok(), "radius {r} was rejected: {res:?}");
            let res = res.unwrap();
            approx_pt(res.arc.center, pt(10.0 - r, r));
        }
        // 逆順（b を 1 本目に）でも同じ結果になる。
        let swapped = fillet_lines(b, a, 2.0, pt(10.0, 9.0), pt(1.0, 0.0)).unwrap();
        approx_pt(swapped.arc.center, pt(8.0, 2.0));
    }

    /// L 字コーナーの向き（線分の端点定義の向き）が逆でも同じ解になる。
    #[test]
    fn l_corner_is_orientation_independent() {
        // a を b.a 側から書き始め、b を終点側から書き始めた形。
        let a = seg(10.0, 0.0, 0.0, 0.0);
        let b = seg(10.0, 10.0, 10.0, 0.0);
        let res = fillet_lines(a, b, 2.0, pt(2.0, 0.0), pt(10.0, 8.0)).unwrap();
        approx_pt(res.arc.center, pt(8.0, 2.0));
        // 元の向き（a.a → a.b）は保存される。
        approx_pt(res.trimmed_a.a, pt(8.0, 0.0));
        approx_pt(res.trimmed_a.b, pt(0.0, 0.0));
        approx_pt(res.trimmed_b.a, pt(10.0, 10.0));
        approx_pt(res.trimmed_b.b, pt(10.0, 2.0));
    }

    // --- 拒否ケース ---

    #[test]
    fn parallel_lines_are_rejected() {
        let a = seg(0.0, 0.0, 10.0, 0.0);
        let b = seg(0.0, 5.0, 10.0, 5.0);
        assert_eq!(
            fillet_lines(a, b, 1.0, pt(2.0, 0.0), pt(2.0, 5.0)),
            Err(FilletError::Parallel)
        );
        // 逆向きに平行（反平行）でも同じ。
        let c = seg(10.0, 5.0, 0.0, 5.0);
        assert_eq!(
            fillet_lines(a, c, 1.0, pt(2.0, 0.0), pt(2.0, 5.0)),
            Err(FilletError::Parallel)
        );
    }

    #[test]
    fn non_positive_radius_is_rejected() {
        let a = seg(0.0, 0.0, 10.0, 0.0);
        let b = seg(10.0, 0.0, 10.0, 10.0);
        for r in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                fillet_lines(a, b, r, pt(2.0, 0.0), pt(10.0, 8.0)),
                Err(FilletError::NonPositiveRadius),
                "radius {r}"
            );
        }
    }

    /// 接線長（直角コーナーでは radius）が線分長を超えると拒否する。
    #[test]
    fn radius_larger_than_segments_is_rejected() {
        let a = seg(0.0, 0.0, 1.0, 0.0);
        let b = seg(1.0, 0.0, 1.0, 1.0);
        assert_eq!(
            fillet_lines(a, b, 5.0, pt(0.1, 0.0), pt(1.0, 0.9)),
            Err(FilletError::RadiusTooLarge)
        );
    }

    /// コーナーが線分の外にある（＝延長しないと届かない）配置も拒否する。
    #[test]
    fn corner_outside_the_segments_is_rejected() {
        // 交点は (0,0) だが、どちらの線分もそこまで届いていない。
        let a = seg(5.0, 0.0, 10.0, 0.0);
        let b = seg(0.0, 5.0, 0.0, 10.0);
        assert_eq!(
            fillet_lines(a, b, 1.0, pt(8.0, 0.0), pt(0.0, 8.0)),
            Err(FilletError::RadiusTooLarge)
        );
    }

    #[test]
    fn degenerate_segments_are_rejected() {
        let a = seg(0.0, 0.0, 0.0, 0.0);
        let b = seg(10.0, 0.0, 10.0, 10.0);
        assert_eq!(
            fillet_lines(a, b, 1.0, pt(0.0, 0.0), pt(10.0, 8.0)),
            Err(FilletError::Degenerate)
        );
        assert_eq!(
            fillet_lines(b, a, 1.0, pt(10.0, 8.0), pt(0.0, 0.0)),
            Err(FilletError::Degenerate)
        );
    }

    #[test]
    fn non_finite_input_is_rejected() {
        let a = seg(0.0, 0.0, 10.0, 0.0);
        let b = seg(10.0, 0.0, 10.0, 10.0);
        assert_eq!(
            fillet_lines(a, b, 1.0, pt(f64::NAN, 0.0), pt(10.0, 8.0)),
            Err(FilletError::Degenerate)
        );
        let bad = seg(10.0, 0.0, f64::INFINITY, 10.0);
        assert_eq!(
            fillet_lines(a, bad, 1.0, pt(2.0, 0.0), pt(10.0, 8.0)),
            Err(FilletError::Degenerate)
        );
    }

    /// コーナー点そのものをクリックすると相手直線に対する「側」が決まらない。
    #[test]
    fn clicking_the_corner_itself_is_ambiguous() {
        let a = seg(-10.0, 0.0, 10.0, 0.0);
        let b = seg(0.0, -10.0, 0.0, 10.0);
        assert_eq!(
            fillet_lines(a, b, 1.0, pt(0.0, 0.0), pt(0.0, 5.0)),
            Err(FilletError::Degenerate)
        );
        assert_eq!(
            fillet_lines(a, b, 1.0, pt(5.0, 0.0), pt(0.0, 0.0)),
            Err(FilletError::Degenerate)
        );
    }

    /// 接点がちょうど残したい端点に重なると、残る線分が消えるので拒否する。
    #[test]
    fn vanishing_trimmed_segment_is_rejected() {
        // a は (0,0)-(10,0)、コーナーは (0,0) 側。radius = 接線長 = 10 だと
        // 接点が (10,0) に来て、near_a = (10,0) 側に残る線分が消える。
        let a = seg(0.0, 0.0, 10.0, 0.0);
        let b = seg(0.0, 0.0, 0.0, 10.0);
        assert_eq!(
            fillet_lines(a, b, 10.0, pt(10.0, 0.0), pt(0.0, 5.0)),
            Err(FilletError::Degenerate)
        );
    }
}
