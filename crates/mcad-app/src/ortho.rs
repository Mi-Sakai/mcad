//! 直交モード（ortho）の拘束ロジック（v0.7.1、番号外の操作性改善。アイソメ-2で
//! 任意の軸集合へ一般化）。
//!
//! [`Document`]/`egui` に依存しない純関数のみを持つ。`origin`（直前に確定した点）
//! から見て `raw`（素のカーソル位置）の方向が、与えられた軸集合のうちどれに
//! 最も近いかを判定し、該当する軸上へ `raw` を投影した点を返す。
//!
//! # 適用ツール・優先関係
//!
//! `Line`/`Polyline`/`Arc` の3ツールのみが `Tool::ortho_origin()` を override して
//! この関数を使う（`tool.rs` 参照）。`main.rs` の `handle_tool_input` は「スナップ
//! 優先」で合成する: スナップ候補が見つかった場合はそれを最終値として使い、
//! スナップが `None`（候補なし）のときだけ ortho を適用する（`resolve_click_point`）。
//! グリッドモードが `Isometric` のときは [`ISO_AXES`]、`Rectangular` のときは
//! [`RECT_AXES`]（＝[`constrain`]）を使う（DESIGN.md 7章「アイソメ図・アクソメ図の
//! 作図補助」アイソメ-2 設計確定）。

use mcad_geom::Point2;

/// `√3 / 2`（30°・150° 軸の x 成分。`f64::sqrt` は const でないため数値リテラル）。
const SQRT3_OVER_2: f64 = 0.866_025_403_784_438_6;

/// 直交モード（矩形グリッド）の拘束軸: 水平（0°）・垂直（90°）の単位方向ベクトル `(x, y)`。
///
/// 軸を角度ではなく**単位ベクトルで持つ**のは、`(PI / 2.0).cos()` が厳密な `0.0` に
/// ならない丸め誤差を射影に持ち込まないため（これにより [`constrain`] は v0.7.1 の
/// 実装とビット単位で一致する）。
pub const RECT_AXES: [(f64, f64); 2] = [(1.0, 0.0), (0.0, 1.0)];

/// アイソメ拘束の3軸: 30°・90°・150° の単位方向ベクトル。
pub const ISO_AXES: [(f64, f64); 3] = [(SQRT3_OVER_2, 0.5), (0.0, 1.0), (-SQRT3_OVER_2, 0.5)];

/// `origin` から見て `raw` を、`axes`（単位方向ベクトル。向きは正負を区別しない）の
/// うち最も近い軸へ直交投影した点を返す。
///
/// 軸との近さは `atan2(|cross|, |dot|)`（`[0, π/2]` の角度差。軸は直線であり向きを
/// 持たないため正負両方向を同一視する）で比較する。同値（角度差が複数の軸で等しい）の
/// 場合は `axes` の先頭側（配列内で先に現れる軸）へ倒す。`raw == origin`（方向が
/// 定義できない退化ケース）はそのまま返す。
#[must_use]
pub fn constrain_to_axes(origin: Point2, raw: Point2, axes: &[(f64, f64)]) -> Point2 {
    let dx = raw.x - origin.x;
    let dy = raw.y - origin.y;
    if dx == 0.0 && dy == 0.0 {
        return raw;
    }

    // 同値は `axes` の先頭側を残す。境界（例: 矩形の対角線、等測の 60°・180° 方向）は
    // 浮動小数点の丸めで角度差がわずかにずれうるので、`TIE_EPSILON` 以内の差は同値と
    // みなして先頭側を保つ（更新条件は `diff + ε < best`）。
    const TIE_EPSILON: f64 = 1e-12;
    let mut best = axes[0];
    let mut best_diff = f64::INFINITY;
    for &(ux, uy) in axes {
        let dot = dx * ux + dy * uy;
        let cross = dx * uy - dy * ux;
        let diff = cross.abs().atan2(dot.abs());
        if diff + TIE_EPSILON < best_diff {
            best_diff = diff;
            best = (ux, uy);
        }
    }

    // 選ばれた軸（原点を通る直線）上へ `raw` を直交射影する。単位方向ベクトルとの
    // 内積が符号付き射影長になる（軸の負側に落ちれば内積は負になり、自然に軸の
    // 反対側へ投影される）。
    let (ux, uy) = best;
    let dot = dx * ux + dy * uy;
    Point2::new(origin.x + dot * ux, origin.y + dot * uy)
}

/// `origin` から見て `raw` を水平軸または垂直軸へ投影した点を返す（矩形グリッド用、
/// [`RECT_AXES`] の薄い wrapper）。
///
/// `|raw.x - origin.x| >= |raw.y - origin.y|` なら水平寄りとみなし `y = origin.y`
/// （同値はこちらへ倒す）、それ以外は垂直寄りとみなし `x = origin.x` とする。
#[must_use]
pub fn constrain(origin: Point2, raw: Point2) -> Point2 {
    constrain_to_axes(origin, raw, &RECT_AXES)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(a: Point2, b: Point2) {
        assert!(
            (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9,
            "left={a:?} right={b:?}"
        );
    }

    #[test]
    fn horizontal_leaning_snaps_y_to_origin() {
        // dx=10, dy=1 -> 水平寄り。
        let origin = Point2::new(0.0, 0.0);
        let raw = Point2::new(10.0, 1.0);
        assert_eq!(constrain(origin, raw), Point2::new(10.0, 0.0));
    }

    #[test]
    fn vertical_leaning_snaps_x_to_origin() {
        // dx=1, dy=10 -> 垂直寄り。
        let origin = Point2::new(0.0, 0.0);
        let raw = Point2::new(1.0, 10.0);
        assert_eq!(constrain(origin, raw), Point2::new(0.0, 10.0));
    }

    #[test]
    fn exact_diagonal_boundary_leans_horizontal() {
        // dx == dy の境界は水平（>=）に倒す仕様を固定する。
        let origin = Point2::new(2.0, 3.0);
        let raw = Point2::new(7.0, 8.0);
        assert_eq!(constrain(origin, raw), Point2::new(7.0, 3.0));
    }

    #[test]
    fn raw_equal_to_origin_is_degenerate_but_does_not_panic() {
        let origin = Point2::new(5.0, -3.0);
        assert_eq!(constrain(origin, origin), origin);
    }

    #[test]
    fn negative_x_direction_projects_correctly() {
        // origin より raw.x が小さい（水平寄り、負方向）。
        let origin = Point2::new(0.0, 0.0);
        let raw = Point2::new(-10.0, 1.0);
        assert_eq!(constrain(origin, raw), Point2::new(-10.0, 0.0));
    }

    #[test]
    fn negative_y_direction_projects_correctly() {
        // origin より raw.y が小さい（垂直寄り、負方向）。
        let origin = Point2::new(0.0, 0.0);
        let raw = Point2::new(1.0, -10.0);
        assert_eq!(constrain(origin, raw), Point2::new(0.0, -10.0));
    }

    #[test]
    fn all_quadrants_project_to_correct_axis() {
        let origin = Point2::new(1.0, 1.0);
        // 第1象限、水平寄り。
        assert_eq!(
            constrain(origin, Point2::new(5.0, 2.0)),
            Point2::new(5.0, 1.0)
        );
        // 第2象限、垂直寄り。
        assert_eq!(
            constrain(origin, Point2::new(-3.0, 6.0)),
            Point2::new(1.0, 6.0)
        );
        // 第3象限、水平寄り。
        assert_eq!(
            constrain(origin, Point2::new(-9.0, 0.5)),
            Point2::new(-9.0, 1.0)
        );
        // 第4象限、垂直寄り。
        assert_eq!(
            constrain(origin, Point2::new(1.5, -9.0)),
            Point2::new(1.0, -9.0)
        );
    }

    /// 与えられた方向（度）に origin から distance だけ離れた raw 点を作るヘルパー。
    fn point_at_degrees(origin: Point2, degrees: f64, distance: f64) -> Point2 {
        let rad = degrees.to_radians();
        Point2::new(
            origin.x + distance * rad.cos(),
            origin.y + distance * rad.sin(),
        )
    }

    /// `actual`（origin 起点）が `expected_deg` 方向の半直線上に乗っている
    /// （直交射影は入力方向とのなす角に応じて長さが変わるため、方向のみを
    /// 検証する。退化点 = origin 自身は許容しない）ことを確認する。
    fn assert_direction(origin: Point2, actual: Point2, expected_deg: f64) {
        let vx = actual.x - origin.x;
        let vy = actual.y - origin.y;
        assert!(
            vx.hypot(vy) > 1e-9,
            "actual point coincides with origin: {actual:?}"
        );
        let rad = expected_deg.to_radians();
        let (ux, uy) = (rad.cos(), rad.sin());
        let cross = vx * uy - vy * ux; // 0 なら平行。
        let dot = vx * ux + vy * uy; // 正なら同じ向き。
        assert!(
            cross.abs() < 1e-9 && dot > 0.0,
            "expected direction {expected_deg}°, got vector ({vx}, {vy})"
        );
    }

    #[test]
    fn iso_axes_truth_table() {
        let origin = Point2::new(0.0, 0.0);
        let dist = 10.0;
        // (入力方向の度数, 期待される投影後の方向の度数)。0°/60°/120°/180° は
        // 隣接軸からの角度差が同値になる境界で、先頭側（配列で先に現れる軸、
        // かつ内積の符号で決まる正負の側）へ倒れる。
        let cases: &[(f64, f64)] = &[
            (0.0, 30.0),    // 境界: 330°(30°軸の負側)と30°の中間 -> 30°へ
            (30.0, 30.0),   // 軸ちょうど
            (60.0, 30.0),   // 境界: 30°と90°の中間 -> 先頭側の30°へ倒す
            (90.0, 90.0),   // 軸ちょうど
            (120.0, 90.0),  // 境界: 90°と150°の中間 -> 先頭側の90°へ倒す
            (150.0, 150.0), // 軸ちょうど
            (180.0, 210.0), // 境界: 150°(の正側)と210°(30°軸の負側)の中間 -> 30°軸側へ
            (210.0, 210.0), // 30°軸の負側ちょうど
        ];
        for &(input_deg, expected_deg) in cases {
            let raw = point_at_degrees(origin, input_deg, dist);
            let actual = constrain_to_axes(origin, raw, &ISO_AXES);
            assert_direction(origin, actual, expected_deg);
        }
    }

    #[test]
    fn iso_axes_boundary_leans_to_first_axis() {
        // 60° ちょうどは 30° 軸と 90° 軸から等距離（境界）。先頭側の 30° 軸へ倒す。
        let origin = Point2::new(2.0, -1.0);
        let raw = point_at_degrees(origin, 60.0, 5.0);
        assert_direction(origin, constrain_to_axes(origin, raw, &ISO_AXES), 30.0);
    }

    #[test]
    fn iso_axes_negative_direction_projects_to_opposite_side() {
        // 210° 方向（30°軸の負側）は、大きさを保ったまま同じ方向へ射影される
        // （軸ちょうどの入力は射影で長さが変わらない）。
        let origin = Point2::new(-4.0, 6.0);
        let raw = point_at_degrees(origin, 210.0, 8.0);
        let expected = point_at_degrees(origin, 210.0, 8.0);
        assert_close(constrain_to_axes(origin, raw, &ISO_AXES), expected);
    }

    #[test]
    fn iso_axes_raw_equal_to_origin_is_degenerate_but_does_not_panic() {
        let origin = Point2::new(1.0, 2.0);
        assert_eq!(constrain_to_axes(origin, origin, &ISO_AXES), origin);
    }
}
