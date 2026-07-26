//! 直交モード（ortho）の拘束ロジック（v0.7.1、番号外の操作性改善）。
//!
//! [`Document`]/`egui` に依存しない純関数のみを持つ。`origin`（直前に確定した点）
//! から見て `raw`（素のカーソル位置）が「より水平に近い」か「より垂直に近い」かを
//! 判定し、該当する軸上へ `raw` を投影した点を返す。
//!
//! # 適用ツール・優先関係
//!
//! `Line`/`Polyline`/`Arc` の3ツールのみが `Tool::ortho_origin()` を override して
//! この関数を使う（`tool.rs` 参照）。`main.rs` の `handle_tool_input` は「スナップ
//! 優先」で合成する: スナップ候補が見つかった場合はそれを最終値として使い、
//! スナップが `None`（候補なし）のときだけ ortho を適用する（`resolve_click_point`）。

use mcad_geom::Point2;

/// `origin` から見て `raw` を水平軸または垂直軸へ投影した点を返す。
///
/// `|raw.x - origin.x| >= |raw.y - origin.y|` なら水平寄りとみなし `y = origin.y`
/// （同値はこちらへ倒す）、それ以外は垂直寄りとみなし `x = origin.x` とする。
#[must_use]
pub fn constrain(origin: Point2, raw: Point2) -> Point2 {
    let dx = (raw.x - origin.x).abs();
    let dy = (raw.y - origin.y).abs();
    if dx >= dy {
        Point2::new(raw.x, origin.y)
    } else {
        Point2::new(origin.x, raw.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
