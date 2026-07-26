//! 分割（[`split`]）。DESIGN.md M7 設計判断 1・1a。
//!
//! 対象は [`Shape::Line`] / [`Shape::Arc`] / 開いた [`Shape::Polyline`]
//! （`closed == false`）のみ。`Shape::Circle`・閉じた `Polyline`・`Shape::Point` は
//! [`SplitError::Unsupported`] を返す（[`Shape::offset`] と同じ薄いディスパッチ）。
//!
//! # 数値ロバスト性
//!
//! 端点近接判定・（Polyline の）中間頂点近接判定はいずれも既存 geom の相対 EPS 運用
//! （`intersect.rs` の `dedup_points`/`prim_contains_point` と同じ
//! `tol = EPS * (1.0 + 座標の大きさ)` 形）に合わせる。座標スケールに依存する CAD 図面で
//! 絶対許容量を使うと、大縮尺の図面で許容が狭すぎたり小縮尺の図面で広すぎたりする
//! 既知の問題を避けるため（DESIGN.md M7 設計判断1a）。

use crate::{Arc, EPS, LineSeg, Point2, Polyline, Shape};

/// 分割が結果を作れない理由。
///
/// geom は GUI 非依存なので理由の列挙のみを返し、ユーザー向け文言は持たない
/// （UI 側 mcad-app が ASCII ステータスメッセージへ対応付ける）。
/// [`crate::OffsetError`] と同じパターン。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitError {
    /// 分割点が、分割してもピースが作れない **全体の始点／終点** の近傍にある
    /// （相対 EPS 許容量内）。`LineSeg`/`Arc` は両端点、`Polyline` は先頭・末尾頂点。
    TooCloseToEndpoint,
    /// 分割を定義できない対象の種別（`Circle`／閉じた `Polyline`／`Point`）。
    Unsupported,
}

/// 点 `p`・`q` が既存 geom の相対 EPS 許容量内で一致するか。
///
/// `intersect.rs` の `prim_contains_point`/`dedup_points` と同じ
/// `tol = EPS * (1.0 + 座標の大きさ)` 形（両点の大きさを足し合わせてスケールとする）。
fn near(p: Point2, q: Point2) -> bool {
    let scale = 1.0 + p.to_vec2().length() + q.to_vec2().length();
    p.distance(q) <= EPS * scale
}

/// 対象 `shape` を点 `at`（形状上またはその近傍）で 2 つに分割する。
///
/// `at` は各プリミティブの `closest_point` で対象上へ射影してから使う（呼び出し側が
/// 厳密に形状上の点を渡さなくても動く。app 層の `SplitTool` は事前に
/// `mcad_geom::closest_point` で射影した点を渡す設計だが、geom 側でも射影する）。
///
/// # Errors
///
/// - [`SplitError::Unsupported`] — `shape` が `Circle`／閉じた `Polyline`／`Point`。
/// - [`SplitError::TooCloseToEndpoint`] — 分割点が対象全体の始点・終点の近傍にある。
pub fn split(shape: &Shape, at: Point2) -> Result<(Shape, Shape), SplitError> {
    match shape {
        Shape::Line(seg) => split_line(seg, at),
        Shape::Arc(arc) => split_arc(arc, at),
        Shape::Polyline(pl) => split_polyline(pl, at),
        Shape::Point(_) | Shape::Circle(_) => Err(SplitError::Unsupported),
    }
}

/// 線分の分割。`at` を線分上へ射影した点で `a`→分割点／分割点→`b` の 2 本にする。
fn split_line(seg: &LineSeg, at: Point2) -> Result<(Shape, Shape), SplitError> {
    // `LineSeg::closest_point` が t を [0, 1] へクランプするので、退化線分
    // （a == b）も含めて分割点は必ず線分上（か端点）に載る。
    let sp = seg.closest_point(at);
    if near(sp, seg.a) || near(sp, seg.b) {
        return Err(SplitError::TooCloseToEndpoint);
    }
    Ok((
        Shape::Line(LineSeg::new(seg.a, sp)),
        Shape::Line(LineSeg::new(sp, seg.b)),
    ))
}

/// 弧の分割。`at` を弧上へ射影した点の角度で `start`→分割点／分割点→`end` の 2 本にする。
///
/// `Arc::closest_point` は範囲内なら射影角、範囲外（中心近傍も含む）なら近い方の端点を
/// 返すので、端点近傍判定は射影後の点が始点・終点と一致するかで判定できる
/// （範囲外にずれたクリックは自然に `TooCloseToEndpoint` として拒否される）。
fn split_arc(arc: &Arc, at: Point2) -> Result<(Shape, Shape), SplitError> {
    if !arc.radius.is_finite() {
        return Err(SplitError::TooCloseToEndpoint);
    }
    let sp = arc.closest_point(at);
    let start = arc.start_point();
    let end = arc.end_point();
    if near(sp, start) || near(sp, end) {
        return Err(SplitError::TooCloseToEndpoint);
    }
    let theta = (sp - arc.center).angle();
    Ok((
        Shape::Arc(Arc::new(arc.center, arc.radius, arc.start_angle, theta)),
        Shape::Arc(Arc::new(arc.center, arc.radius, theta, arc.end_angle)),
    ))
}

/// 開いたポリラインの分割。DESIGN.md M7 設計判断1a。
///
/// 1. 各セグメントへ `closest_point` して最も近いセグメント `i` を特定する。
/// 2. 分割点が全体の始点 `v_0` または終点 `v_n` の近傍なら
///    [`SplitError::TooCloseToEndpoint`]。
/// 3. 分割点がセグメント `i` の中間頂点（`v_i` または `v_{i+1}`）の近傍なら、
///    新しい点を挿入せず **その頂点で** 分割する。
/// 4. それ以外（セグメント内部）なら分割点を新しい頂点として挿入する。
///
/// 閉じたポリライン（`closed == true`）は 1 点で割っても "2 本の独立したポリライン"
/// にならないため [`SplitError::Unsupported`]。頂点 2 未満（線分を構成できない）も
/// 同様に扱う。
fn split_polyline(pl: &Polyline, at: Point2) -> Result<(Shape, Shape), SplitError> {
    if pl.closed {
        return Err(SplitError::Unsupported);
    }
    let n = pl.vertices.len();
    if n < 2 {
        return Err(SplitError::Unsupported);
    }

    // 最も近いセグメントとその上の射影点を探す（`segments()` は開いた場合 n-1 本を返す）。
    let mut segs = pl.segments();
    let first = segs.next().expect("n >= 2 なので少なくとも1セグメントある");
    let mut best_i = 0usize;
    let mut best_sp = first.closest_point(at);
    let mut best_d2 = at.distance_squared(best_sp);
    for (offset, seg) in segs.enumerate() {
        let sp = seg.closest_point(at);
        let d2 = at.distance_squared(sp);
        if d2 < best_d2 {
            best_i = offset + 1;
            best_sp = sp;
            best_d2 = d2;
        }
    }
    let i = best_i;
    let sp = best_sp;

    let v0 = pl.vertices[0];
    let vn = pl.vertices[n - 1];
    if near(sp, v0) || near(sp, vn) {
        return Err(SplitError::TooCloseToEndpoint);
    }

    let seg_a = pl.vertices[i];
    let seg_b = pl.vertices[i + 1];

    // 中間頂点ちょうどでの分割: 新規頂点を挿入せず、その頂点で素直に割る。
    // v0/vn 近傍は上で既に排除済みなので、ここで一致しうるのは中間頂点のみ
    // （i == 0 の seg_a は v0、最終セグメントの seg_b は vn と一致するため既に弾かれている）。
    if near(sp, seg_a) {
        let piece1: Vec<Point2> = pl.vertices[..=i].to_vec();
        let piece2: Vec<Point2> = pl.vertices[i..].to_vec();
        return Ok((
            Shape::Polyline(Polyline::new(piece1, false)),
            Shape::Polyline(Polyline::new(piece2, false)),
        ));
    }
    if near(sp, seg_b) {
        let piece1: Vec<Point2> = pl.vertices[..=(i + 1)].to_vec();
        let piece2: Vec<Point2> = pl.vertices[(i + 1)..].to_vec();
        return Ok((
            Shape::Polyline(Polyline::new(piece1, false)),
            Shape::Polyline(Polyline::new(piece2, false)),
        ));
    }

    // セグメント内部: 分割点を新しい頂点として両ピースへ挿入する。
    let mut piece1: Vec<Point2> = pl.vertices[..=i].to_vec();
    piece1.push(sp);
    let mut piece2: Vec<Point2> = Vec::with_capacity(n - i);
    piece2.push(sp);
    piece2.extend_from_slice(&pl.vertices[i + 1..]);

    Ok((
        Shape::Polyline(Polyline::new(piece1, false)),
        Shape::Polyline(Polyline::new(piece2, false)),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_2, PI, TAU};

    fn p(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    // -----------------------------------------------------------------
    // Line
    // -----------------------------------------------------------------

    #[test]
    fn split_line_at_midpoint() {
        let seg = LineSeg::new(p(0.0, 0.0), p(10.0, 0.0));
        let (piece1, piece2) = split(&Shape::Line(seg), p(5.0, 0.0)).expect("split");
        let Shape::Line(l1) = piece1 else {
            panic!("expected line")
        };
        let Shape::Line(l2) = piece2 else {
            panic!("expected line")
        };
        assert!(l1.a.distance(seg.a) < 1e-9);
        assert!(l1.b.distance(p(5.0, 0.0)) < 1e-9);
        assert!(l2.a.distance(p(5.0, 0.0)) < 1e-9);
        assert!(l2.b.distance(seg.b) < 1e-9);
    }

    #[test]
    fn split_line_near_start_rejected() {
        let seg = LineSeg::new(p(0.0, 0.0), p(10.0, 0.0));
        assert_eq!(
            split(&Shape::Line(seg), p(0.0, 0.0)),
            Err(SplitError::TooCloseToEndpoint)
        );
    }

    #[test]
    fn split_line_near_end_rejected() {
        let seg = LineSeg::new(p(0.0, 0.0), p(10.0, 0.0));
        assert_eq!(
            split(&Shape::Line(seg), p(10.0, 0.0)),
            Err(SplitError::TooCloseToEndpoint)
        );
    }

    // -----------------------------------------------------------------
    // Arc
    // -----------------------------------------------------------------

    #[test]
    fn split_arc_at_midpoint() {
        let arc = Arc::new(p(0.0, 0.0), 5.0, 0.0, PI);
        let (piece1, piece2) = split(&Shape::Arc(arc), p(0.0, 5.0)).expect("split");
        let Shape::Arc(a1) = piece1 else {
            panic!("expected arc")
        };
        let Shape::Arc(a2) = piece2 else {
            panic!("expected arc")
        };
        assert!((a1.start_angle - 0.0).abs() < 1e-9);
        assert!((a1.end_angle - FRAC_PI_2).abs() < 1e-6);
        assert!((a2.start_angle - FRAC_PI_2).abs() < 1e-6);
        assert!((a2.end_angle - PI).abs() < 1e-9);
        // 往復性: 両端点が元の弧の端点・分割点に一致する。
        assert!(a1.start_point().distance(arc.start_point()) < 1e-6);
        assert!(a1.end_point().distance(a2.start_point()) < 1e-6);
        assert!(a2.end_point().distance(arc.end_point()) < 1e-6);
    }

    #[test]
    fn split_arc_near_start_rejected() {
        let arc = Arc::new(p(0.0, 0.0), 5.0, 0.0, PI);
        assert_eq!(
            split(&Shape::Arc(arc), arc.start_point()),
            Err(SplitError::TooCloseToEndpoint)
        );
    }

    #[test]
    fn split_arc_near_end_rejected() {
        let arc = Arc::new(p(0.0, 0.0), 5.0, 0.0, PI);
        assert_eq!(
            split(&Shape::Arc(arc), arc.end_point()),
            Err(SplitError::TooCloseToEndpoint)
        );
    }

    #[test]
    fn split_arc_almost_full_circle() {
        // 掃引がほぼ 2π（全周近く）でも中間点で正しく分割できる。
        let arc = Arc::new(p(0.0, 0.0), 5.0, 0.0, TAU - 0.1);
        let mid = arc.circle().point_at_angle(PI);
        let (piece1, piece2) = split(&Shape::Arc(arc), mid).expect("split");
        let Shape::Arc(a1) = piece1 else {
            panic!("expected arc")
        };
        let Shape::Arc(a2) = piece2 else {
            panic!("expected arc")
        };
        assert!(a1.sweep() > 0.0 && a1.sweep() < TAU);
        assert!(a2.sweep() > 0.0 && a2.sweep() < TAU);
    }

    // -----------------------------------------------------------------
    // Polyline
    // -----------------------------------------------------------------

    fn open_polyline() -> Polyline {
        Polyline::new(vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0)], false)
    }

    #[test]
    fn split_polyline_inside_segment_inserts_vertex() {
        let pl = open_polyline();
        let (piece1, piece2) = split(&Shape::Polyline(pl.clone()), p(5.0, 0.0)).expect("split");
        let Shape::Polyline(pl1) = piece1 else {
            panic!("expected polyline")
        };
        let Shape::Polyline(pl2) = piece2 else {
            panic!("expected polyline")
        };
        assert!(!pl1.closed && !pl2.closed);
        assert_eq!(pl1.vertices, vec![p(0.0, 0.0), p(5.0, 0.0)]);
        assert_eq!(pl2.vertices, vec![p(5.0, 0.0), p(10.0, 0.0), p(10.0, 10.0)]);
    }

    #[test]
    fn split_polyline_at_middle_vertex_does_not_duplicate() {
        let pl = open_polyline();
        // 中間頂点 (10, 0) のごく近傍をクリック。
        let (piece1, piece2) = split(&Shape::Polyline(pl.clone()), p(10.0, 0.0)).expect("split");
        let Shape::Polyline(pl1) = piece1 else {
            panic!("expected polyline")
        };
        let Shape::Polyline(pl2) = piece2 else {
            panic!("expected polyline")
        };
        // 新規頂点を挿入せず、既存の中間頂点で素直に割れる(重複なし)。
        assert_eq!(pl1.vertices, vec![p(0.0, 0.0), p(10.0, 0.0)]);
        assert_eq!(pl2.vertices, vec![p(10.0, 0.0), p(10.0, 10.0)]);
    }

    #[test]
    fn split_polyline_near_overall_start_rejected() {
        let pl = open_polyline();
        assert_eq!(
            split(&Shape::Polyline(pl), p(0.0, 0.0)),
            Err(SplitError::TooCloseToEndpoint)
        );
    }

    #[test]
    fn split_polyline_near_overall_end_rejected() {
        let pl = open_polyline();
        assert_eq!(
            split(&Shape::Polyline(pl), p(10.0, 10.0)),
            Err(SplitError::TooCloseToEndpoint)
        );
    }

    #[test]
    fn split_closed_polyline_unsupported() {
        let pl = Polyline::new(vec![p(0.0, 0.0), p(10.0, 0.0), p(5.0, 10.0)], true);
        assert_eq!(
            split(&Shape::Polyline(pl), p(5.0, 0.0)),
            Err(SplitError::Unsupported)
        );
    }

    // -----------------------------------------------------------------
    // Circle / Point
    // -----------------------------------------------------------------

    #[test]
    fn split_circle_unsupported() {
        let c = crate::Circle::new(p(0.0, 0.0), 5.0);
        assert_eq!(
            split(&Shape::Circle(c), p(5.0, 0.0)),
            Err(SplitError::Unsupported)
        );
    }

    #[test]
    fn split_point_unsupported() {
        assert_eq!(
            split(&Shape::Point(p(0.0, 0.0)), p(0.0, 0.0)),
            Err(SplitError::Unsupported)
        );
    }
}
