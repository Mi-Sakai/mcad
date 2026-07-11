//! 形状どうしの交点計算。
//!
//! [`intersect`] は各形状をプリミティブ曲線（点・線分・円・弧）へ分解し、
//! プリミティブ対ごとに交点を求めて集約・重複除去する。ポリラインは
//! 構成線分の集合として扱う。
//!
//! # 数値安定性の方針
//!
//! - 平行判定は外積を **相対** イプシロン `EPS * |r| * |s|`（角の sin 相当）で行う。
//! - 円・線分の交点は中心から直線への垂線の足を基準に計算し、判別式を
//!   直接扱う定式化よりも接線ケースの誤差を小さくする。
//! - 接線・外接・内接は距離を半径スケールの許容量と比較して 1 点に落とす。
//! - パラメータ `[0, 1]` の境界は無次元なので `EPS` を直接使う。

use crate::primitives::{Arc, Circle, LineSeg, Shape};
use crate::{EPS, Point2};

/// 2 つの形状の交点を列挙する。
///
/// 対応する組み合わせ:
/// - 線分 × 線分
/// - 線分 × 円 / 線分 × 弧
/// - 円 × 円 / 円 × 弧 / 弧 × 弧
/// - 点 × 任意（点がその形状上に載っていれば交点）
/// - ポリラインは構成線分に分解して上記を適用
///
/// 平行・同心・接線などの退化ケースでは、孤立した交点が定まらない限り
/// 空を返す（重なり区間のような無限交点は列挙しない）。重複点は除去する。
#[must_use]
pub fn intersect(a: &Shape, b: &Shape) -> Vec<Point2> {
    let pa = to_prims(a);
    let pb = to_prims(b);
    let mut out = Vec::new();
    for x in &pa {
        for y in &pb {
            prim_intersect(x, y, &mut out);
        }
    }
    dedup_points(&mut out);
    out
}

/// 交点計算のためのプリミティブ曲線。
enum Prim {
    Point(Point2),
    Seg(LineSeg),
    Circle(Circle),
    Arc(Arc),
}

/// 形状を交点計算用プリミティブ列へ分解する。
fn to_prims(shape: &Shape) -> Vec<Prim> {
    match shape {
        Shape::Point(p) => vec![Prim::Point(*p)],
        Shape::Line(s) => vec![Prim::Seg(*s)],
        Shape::Circle(c) => vec![Prim::Circle(*c)],
        Shape::Arc(a) => vec![Prim::Arc(*a)],
        Shape::Polyline(pl) => pl.segments().map(Prim::Seg).collect(),
    }
}

/// プリミティブ対の交点を `out` に追加する。
fn prim_intersect(x: &Prim, y: &Prim, out: &mut Vec<Point2>) {
    match (x, y) {
        (Prim::Seg(s1), Prim::Seg(s2)) => {
            if let Some(p) = seg_seg(s1, s2) {
                out.push(p);
            }
        }
        (Prim::Seg(s), Prim::Circle(c)) | (Prim::Circle(c), Prim::Seg(s)) => {
            seg_circle(s, c, out);
        }
        (Prim::Seg(s), Prim::Arc(a)) | (Prim::Arc(a), Prim::Seg(s)) => {
            let mut tmp = Vec::new();
            seg_circle(s, &a.circle(), &mut tmp);
            retain_on_arc(&tmp, a, out);
        }
        (Prim::Circle(c1), Prim::Circle(c2)) => {
            circle_circle(c1, c2, out);
        }
        (Prim::Circle(c), Prim::Arc(a)) | (Prim::Arc(a), Prim::Circle(c)) => {
            let mut tmp = Vec::new();
            circle_circle(c, &a.circle(), &mut tmp);
            retain_on_arc(&tmp, a, out);
        }
        (Prim::Arc(a1), Prim::Arc(a2)) => {
            let mut tmp = Vec::new();
            circle_circle(&a1.circle(), &a2.circle(), &mut tmp);
            for &p in &tmp {
                if on_arc(a1, p) && on_arc(a2, p) {
                    out.push(p);
                }
            }
        }
        // 点が絡む組み合わせ: 点が相手プリミティブ上に載っていれば交点。
        (Prim::Point(p), other) | (other, Prim::Point(p)) => {
            if prim_contains_point(other, *p) {
                out.push(*p);
            }
        }
    }
}

/// `pts` のうち弧 `a` の角度範囲内の点だけを `out` に追加する。
fn retain_on_arc(pts: &[Point2], a: &Arc, out: &mut Vec<Point2>) {
    for &p in pts {
        if on_arc(a, p) {
            out.push(p);
        }
    }
}

/// 点 `p`（弧の台円上にある前提）が弧 `a` の角度範囲内か。
fn on_arc(a: &Arc, p: Point2) -> bool {
    let ang = (p - a.center).angle();
    a.contains_angle(ang)
}

/// プリミティブ `prim` 上に点 `p` が（許容量込みで）載っているか。
fn prim_contains_point(prim: &Prim, p: Point2) -> bool {
    let cp = match prim {
        Prim::Point(q) => *q,
        Prim::Seg(s) => s.closest_point(p),
        Prim::Circle(c) => c.closest_point(p),
        Prim::Arc(a) => a.closest_point(p),
    };
    // 点どうしの一致・点が曲線上に載る判定。座標スケールに対する相対許容量。
    let scale = 1.0 + p.to_vec2().length() + cp.to_vec2().length();
    p.distance(cp) <= EPS * scale
}

/// 線分 × 線分の真の交点（1 点）。平行・共線・区間重なりは `None`。
fn seg_seg(s1: &LineSeg, s2: &LineSeg) -> Option<Point2> {
    let r = s1.direction();
    let s = s2.direction();
    let rl = r.length();
    let sl = s.length();
    // 退化した（長さ 0 の）線分は交点計算の対象外。
    if rl <= EPS || sl <= EPS {
        return None;
    }
    let denom = r.cross(s);
    // 平行判定: 角の sin が EPS 未満（|cross| = |r||s|sin θ）。
    if denom.abs() <= EPS * rl * sl {
        return None;
    }
    let diff = s2.a - s1.a;
    let t = diff.cross(s) / denom;
    let u = diff.cross(r) / denom;
    let unit = -EPS..=1.0 + EPS;
    if unit.contains(&t) && unit.contains(&u) {
        // 数値誤差で範囲外へわずかに出た t を線分内へ寄せる。
        Some(s1.a + r * t.clamp(0.0, 1.0))
    } else {
        None
    }
}

/// 線分 × 円の交点（0〜2 点、線分区間内）を `out` に追加する。
///
/// 中心から直線への垂線の足を基準に半弦長を求める定式化。接線ケースは 1 点。
fn seg_circle(seg: &LineSeg, c: &Circle, out: &mut Vec<Point2>) {
    let d = seg.direction();
    let len = d.length();
    // 半径スケールの許容量（相対）。半径が極小でも 0 除算しないよう下限を設ける。
    let tol = EPS * c.radius.max(1.0);
    if len <= EPS {
        // 退化線分は 1 点として円周上判定。
        if (seg.a.distance(c.center) - c.radius).abs() <= tol {
            out.push(seg.a);
        }
        return;
    }
    let u = d / len; // 単位方向。
    let along_foot = (c.center - seg.a).dot(u); // 足までの符号付き距離。
    let foot = seg.a + u * along_foot;
    let dist = foot.distance(c.center); // 中心-直線 距離。
    if dist > c.radius + tol {
        return; // 交わらない。
    }
    let half2 = c.radius * c.radius - dist * dist;
    let half = if half2 <= 0.0 { 0.0 } else { half2.sqrt() };
    // half==0 なら接線（1 点）、そうでなければ 2 点。両端の符号付き距離を試す。
    let candidates: [f64; 2] = [along_foot - half, along_foot + half];
    let count = if half == 0.0 { 1 } else { 2 };
    for &along in &candidates[..count] {
        let t = along / len;
        if (-EPS..=1.0 + EPS).contains(&t) {
            out.push(seg.a + u * along);
        }
    }
}

/// 円 × 円の交点（0〜2 点）を `out` に追加する。
///
/// 同心（同一円・同心異半径）は孤立交点が無いので何も追加しない。
/// 外接・内接（接線）は 1 点。
fn circle_circle(c1: &Circle, c2: &Circle, out: &mut Vec<Point2>) {
    let between = c2.center - c1.center;
    let dist = between.length();
    let (r1, r2) = (c1.radius, c2.radius);
    let tol = EPS * (r1 + r2).max(1.0);

    if dist <= EPS {
        // 同心。同一円は無限交点、同心異半径は交点なし。いずれも列挙しない。
        return;
    }
    if dist > r1 + r2 + tol {
        return; // 離れすぎ（外部）。
    }
    if dist < (r1 - r2).abs() - tol {
        return; // 一方が他方に完全内包。
    }

    // c1 中心から交点弦の中点までの距離 a。
    let a = (dist * dist + r1 * r1 - r2 * r2) / (2.0 * dist);
    let half2 = r1 * r1 - a * a;
    let dir = between / dist; // 単位（c1→c2）。
    let mid = c1.center + dir * a;

    if half2 <= 0.0 {
        // 接線（外接・内接）: 1 点。
        out.push(mid);
        return;
    }
    let half = half2.sqrt();
    let perp = dir.perp();
    out.push(mid + perp * half);
    out.push(mid - perp * half);
}

/// 近接する重複点を除去する（許容量内で同一とみなす）。点数は少数のため O(n²)。
fn dedup_points(pts: &mut Vec<Point2>) {
    let mut i = 0;
    while i < pts.len() {
        let pi = pts[i];
        // pi 以降で pi とほぼ一致するものを除去。
        let scale = 1.0 + pi.to_vec2().length();
        let tol = EPS * scale;
        let mut j = i + 1;
        while j < pts.len() {
            if pi.distance(pts[j]) <= tol {
                pts.swap_remove(j);
            } else {
                j += 1;
            }
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Polyline;
    use std::f64::consts::{FRAC_PI_2, PI};

    fn shape_line(ax: f64, ay: f64, bx: f64, by: f64) -> Shape {
        Shape::Line(LineSeg::new(Point2::new(ax, ay), Point2::new(bx, by)))
    }

    fn contains_approx(pts: &[Point2], p: Point2) -> bool {
        pts.iter().any(|q| q.distance(p) < 1e-6)
    }

    #[test]
    fn line_line_cross() {
        let a = shape_line(0.0, 0.0, 10.0, 10.0);
        let b = shape_line(0.0, 10.0, 10.0, 0.0);
        let pts = intersect(&a, &b);
        assert_eq!(pts.len(), 1);
        assert!(contains_approx(&pts, Point2::new(5.0, 5.0)));
    }

    #[test]
    fn line_line_parallel_none() {
        let a = shape_line(0.0, 0.0, 10.0, 0.0);
        let b = shape_line(0.0, 1.0, 10.0, 1.0);
        assert!(intersect(&a, &b).is_empty());
    }

    #[test]
    fn line_line_collinear_none() {
        let a = shape_line(0.0, 0.0, 10.0, 0.0);
        let b = shape_line(2.0, 0.0, 12.0, 0.0);
        assert!(intersect(&a, &b).is_empty());
    }

    #[test]
    fn line_line_out_of_segment_none() {
        // 交線としては (5,5) で交わるが、両線分の外なので交点なし。
        let a = shape_line(0.0, 0.0, 1.0, 1.0);
        let b = shape_line(0.0, 10.0, 1.0, 9.0);
        assert!(intersect(&a, &b).is_empty());
    }

    #[test]
    fn line_circle_two_points() {
        let line = shape_line(-10.0, 0.0, 10.0, 0.0);
        let circ = Shape::Circle(Circle::new(Point2::ORIGIN, 5.0));
        let pts = intersect(&line, &circ);
        assert_eq!(pts.len(), 2);
        assert!(contains_approx(&pts, Point2::new(5.0, 0.0)));
        assert!(contains_approx(&pts, Point2::new(-5.0, 0.0)));
    }

    #[test]
    fn line_circle_tangent_one_point() {
        // y = 5 の水平線は半径 5 の円に上端で接する。
        let line = shape_line(-10.0, 5.0, 10.0, 5.0);
        let circ = Shape::Circle(Circle::new(Point2::ORIGIN, 5.0));
        let pts = intersect(&line, &circ);
        assert_eq!(pts.len(), 1);
        assert!(contains_approx(&pts, Point2::new(0.0, 5.0)));
    }

    #[test]
    fn line_circle_miss_none() {
        let line = shape_line(-10.0, 6.0, 10.0, 6.0);
        let circ = Shape::Circle(Circle::new(Point2::ORIGIN, 5.0));
        assert!(intersect(&line, &circ).is_empty());
    }

    #[test]
    fn line_arc_filters_by_angle() {
        // 上半円（0..π）の弧。水平線 y=0 は本来 (±5,0) で円と交わるが、
        // それらは弧の端点（角 0, π）なので両方入る。
        let arc = Shape::Arc(Arc::new(Point2::ORIGIN, 5.0, 0.0, PI));
        let line = shape_line(-10.0, 0.0, 10.0, 0.0);
        let pts = intersect(&line, &arc);
        assert_eq!(pts.len(), 2);

        // 弧を第 1 象限だけ（0..π/2）にすると、(−5,0) は範囲外で除外。
        let arc2 = Shape::Arc(Arc::new(Point2::ORIGIN, 5.0, 0.0, FRAC_PI_2));
        let pts2 = intersect(&line, &arc2);
        assert_eq!(pts2.len(), 1);
        assert!(contains_approx(&pts2, Point2::new(5.0, 0.0)));
    }

    #[test]
    fn circle_circle_two_points() {
        let c1 = Shape::Circle(Circle::new(Point2::new(0.0, 0.0), 5.0));
        let c2 = Shape::Circle(Circle::new(Point2::new(8.0, 0.0), 5.0));
        let pts = intersect(&c1, &c2);
        assert_eq!(pts.len(), 2);
        // 交点は x=4、y=±3。
        assert!(contains_approx(&pts, Point2::new(4.0, 3.0)));
        assert!(contains_approx(&pts, Point2::new(4.0, -3.0)));
    }

    #[test]
    fn circle_circle_external_tangent() {
        let c1 = Shape::Circle(Circle::new(Point2::new(0.0, 0.0), 5.0));
        let c2 = Shape::Circle(Circle::new(Point2::new(10.0, 0.0), 5.0));
        let pts = intersect(&c1, &c2);
        assert_eq!(pts.len(), 1);
        assert!(contains_approx(&pts, Point2::new(5.0, 0.0)));
    }

    #[test]
    fn circle_circle_internal_tangent() {
        let c1 = Shape::Circle(Circle::new(Point2::new(0.0, 0.0), 5.0));
        let c2 = Shape::Circle(Circle::new(Point2::new(3.0, 0.0), 2.0));
        let pts = intersect(&c1, &c2);
        assert_eq!(pts.len(), 1);
        assert!(contains_approx(&pts, Point2::new(5.0, 0.0)));
    }

    #[test]
    fn circle_circle_concentric_none() {
        let c1 = Shape::Circle(Circle::new(Point2::ORIGIN, 5.0));
        let c2 = Shape::Circle(Circle::new(Point2::ORIGIN, 3.0));
        assert!(intersect(&c1, &c2).is_empty());
        // 同一円も無限交点として列挙しない。
        assert!(intersect(&c1, &c1).is_empty());
    }

    #[test]
    fn circle_circle_separate_none() {
        let c1 = Shape::Circle(Circle::new(Point2::new(0.0, 0.0), 2.0));
        let c2 = Shape::Circle(Circle::new(Point2::new(10.0, 0.0), 2.0));
        assert!(intersect(&c1, &c2).is_empty());
    }

    #[test]
    fn polyline_intersects_line() {
        let pl = Shape::Polyline(Polyline::new(
            vec![
                Point2::new(0.0, 0.0),
                Point2::new(10.0, 0.0),
                Point2::new(10.0, 10.0),
            ],
            false,
        ));
        // 縦線 x=5 は最初の水平セグメントと (5,0) で交わる。
        let line = shape_line(5.0, -5.0, 5.0, 5.0);
        let pts = intersect(&pl, &line);
        assert_eq!(pts.len(), 1);
        assert!(contains_approx(&pts, Point2::new(5.0, 0.0)));
    }

    #[test]
    fn point_on_shape() {
        let seg = shape_line(0.0, 0.0, 10.0, 0.0);
        let on = Shape::Point(Point2::new(5.0, 0.0));
        let off = Shape::Point(Point2::new(5.0, 1.0));
        assert_eq!(intersect(&on, &seg).len(), 1);
        assert!(intersect(&off, &seg).is_empty());
    }
}
