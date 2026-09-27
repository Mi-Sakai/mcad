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
//! - 接線・外接・内接は距離を、**中心座標と半径の両方** でスケールした相対許容量
//!   （[`crate::rel_tol`]）と比較して 1 点に落とす。中心が原点から離れると
//!   「中心 → 直線」距離の丸め誤差が座標の大きさに比例して増えるため、半径だけを
//!   スケールに使うと原点から遠い円で許容量が足りなくなる。
//! - 点の一致・線分の退化は [`crate::point_tol`] で判定する。
//! - パラメータ `[0, 1]` の境界は無次元なので `EPS` を直接使う。

use crate::primitives::{Arc, Circle, LineSeg, Shape};
use crate::{Aabb, EPS, Point2, point_tol, rel_tol};

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
    p.distance(cp) <= point_tol(p, cp)
}

/// 無限直線 `a`・`b`（`LineSeg` を通る直線とみなす）どうしの交点。
///
/// 区間 `[0, 1]` へのクランプを行わない点だけが [`seg_seg`] と異なる（延長線上の
/// 交点も返す）。仮想交点スナップ（M12 タスク87、DESIGN.md M12 設計判断5）専用。
///
/// 平行・同一直線（重複判定）、またはどちらかの線分が退化（長さが
/// [`crate::point_tol`] 以下）していれば `None`。
#[must_use]
pub fn line_line_intersection(a: &LineSeg, b: &LineSeg) -> Option<Point2> {
    let r = a.direction();
    let s = b.direction();
    let rl = r.length();
    let sl = s.length();
    if rl <= point_tol(a.a, a.b) || sl <= point_tol(b.a, b.b) {
        return None;
    }
    let denom = r.cross(s);
    // 平行判定: 角の sin が EPS 未満（|cross| = |r||s|sin θ）。同一直線もここで弾く。
    if denom.abs() <= EPS * rl * sl {
        return None;
    }
    let diff = b.a - a.a;
    let t = diff.cross(s) / denom;
    Some(a.a + r * t)
}

/// 無限直線 `line`（`LineSeg` を通る直線とみなす）と円 `c` の交点（0〜2 点）。
///
/// 区間 `[0, 1]` へのクランプを行わない点だけが [`seg_circle`] と異なる（延長線上の
/// 交点も返す）。仮想交点スナップ（M12 タスク87、DESIGN.md M12 設計判断5）専用。
///
/// 線分が退化（長さが [`crate::point_tol`] 以下）、または半径が非正・非有限なら空を返す
/// （退化した直線・円は「無限直線」の前提を満たさない）。接する場合は 1 点。
#[must_use]
pub fn line_circle_intersection(line: &LineSeg, c: &Circle) -> Vec<Point2> {
    let mut out = Vec::new();
    if line.length() <= point_tol(line.a, line.b) {
        return out;
    }
    if !c.radius.is_finite() || c.radius <= 0.0 {
        return out;
    }
    let d = line.direction();
    let len = d.length();
    let tol = rel_tol(c.center.to_vec2().length() + c.radius);
    let u = d / len; // 単位方向。
    let along_foot = (c.center - line.a).dot(u);
    let foot = line.a + u * along_foot;
    let dist = foot.distance(c.center);
    if dist > c.radius + tol {
        return out; // 交わらない。
    }
    let half2 = c.radius * c.radius - dist * dist;
    let half = if half2 <= 0.0 { 0.0 } else { half2.sqrt() };
    if half == 0.0 {
        out.push(foot);
    } else {
        out.push(line.a + u * (along_foot - half));
        out.push(line.a + u * (along_foot + half));
    }
    out
}

/// 無限直線 `line`（`LineSeg` を通る直線とみなす）と有限の形状 `shape` の交点。
///
/// 直線側は区間 `[0, 1]` へクランプせず、**`shape` 側だけ**実体の範囲（線分の区間・
/// 円弧の掃引範囲）で絞る。構築線（M12 タスク91、DESIGN.md M12 設計判断9）の交点
/// スナップ専用。ポリラインは構成線分ごとに求める。点は直線上に（[`crate::point_tol`]
/// 以内で）載っていれば交点とする。
///
/// 平行・同一直線の線分（重なり区間のような無限交点）は [`intersect`] と同じく
/// 列挙しない。`line` が退化（長さが [`crate::point_tol`] 以下）していれば空を返す。
/// 重複点は除去する。
#[must_use]
pub fn line_shape_intersection(line: &LineSeg, shape: &Shape) -> Vec<Point2> {
    let mut out = Vec::new();
    if line.length() <= point_tol(line.a, line.b) {
        return out;
    }
    for prim in to_prims(shape) {
        match prim {
            Prim::Point(p) => {
                // 無限直線への射影（`LineSeg::closest_point` は区間へクランプするため
                // 使えない）。
                let d = line.direction();
                let t = (p - line.a).dot(d) / d.length_squared();
                let foot = line.a + d * t;
                if p.distance(foot) <= point_tol(p, foot) {
                    out.push(p);
                }
            }
            Prim::Seg(s) => {
                if let Some(p) = line_seg(line, &s) {
                    out.push(p);
                }
            }
            Prim::Circle(c) => out.extend(line_circle_intersection(line, &c)),
            Prim::Arc(a) => {
                let tmp = line_circle_intersection(line, &a.circle());
                retain_on_arc(&tmp, &a, &mut out);
            }
        }
    }
    dedup_points(&mut out);
    out
}

/// 無限直線 `line`（`LineSeg` を通る直線とみなす）を矩形 `rect` で切り取った線分。
///
/// 構築線（M12 タスク91、DESIGN.md M12 設計判断9）の描画・出力・範囲判定の共通部品。
/// 画面描画は表示範囲、SVG/PDF は用紙の範囲、カリングとスナップの前段絞り込みは
/// 「`Some` かどうか」だけを使う。
///
/// - 矩形は境界を含む閉領域として扱う。辺の上を通る直線・角だけに触れる直線も
///   `Some` を返す（角だけに触れる場合は長さ 0 の線分）。
/// - 返す線分の向きは `line` と同じ（`a` → `b` の向きに進む）。端点は矩形の境界上にある
///   （丸め誤差の範囲で）。
/// - 矩形と交わらない、`line` が退化している（長さが [`crate::point_tol`] 以下）、
///   `line` か `rect` に非有限値が含まれる場合は `None`。
///
/// 実装は Liang–Barsky のパラメータ切り取りで、直線のパラメータ範囲を `(-∞, ∞)` から
/// 始める（線分の切り取りとの違いはこれだけ）。
#[must_use]
pub fn clip_line_to_aabb(line: &LineSeg, rect: &Aabb) -> Option<LineSeg> {
    let finite = |p: Point2| p.x.is_finite() && p.y.is_finite();
    if !(finite(line.a) && finite(line.b) && finite(rect.min) && finite(rect.max)) {
        return None;
    }
    let d = line.direction();
    if d.length() <= point_tol(line.a, line.b) {
        return None;
    }
    let mut t0 = f64::NEG_INFINITY;
    let mut t1 = f64::INFINITY;
    for (origin, delta, lo, hi) in [
        (line.a.x, d.x, rect.min.x, rect.max.x),
        (line.a.y, d.y, rect.min.y, rect.max.y),
    ] {
        if delta == 0.0 {
            // 軸に平行: その軸の範囲内を通るかだけで決まる（0 除算を避ける）。
            if origin < lo || origin > hi {
                return None;
            }
        } else {
            let ta = (lo - origin) / delta;
            let tb = (hi - origin) / delta;
            t0 = t0.max(ta.min(tb));
            t1 = t1.min(ta.max(tb));
        }
    }
    // 退化していないので少なくとも一方の軸が有限の範囲を与える。極端に小さい成分の
    // 除算で ±∞ が出ても、交わらない場合は t0 > t1 になって弾かれる。
    if !(t0 <= t1 && t0.is_finite() && t1.is_finite()) {
        return None;
    }
    Some(LineSeg::new(line.a + d * t0, line.a + d * t1))
}

/// 無限直線 `line` × 線分 `seg` の交点。[`seg_seg`] から直線側（`t`）の区間判定を
/// 外したもの。平行・共線・`seg` の退化は `None`。
fn line_seg(line: &LineSeg, seg: &LineSeg) -> Option<Point2> {
    let r = line.direction();
    let s = seg.direction();
    let rl = r.length();
    let sl = s.length();
    if rl <= point_tol(line.a, line.b) || sl <= point_tol(seg.a, seg.b) {
        return None;
    }
    let denom = r.cross(s);
    if denom.abs() <= EPS * rl * sl {
        return None;
    }
    let diff = seg.a - line.a;
    let u = diff.cross(r) / denom;
    if (-EPS..=1.0 + EPS).contains(&u) {
        // 線分側のパラメータで位置を決める（数値誤差で区間外へ僅かに出た u は寄せる）。
        Some(seg.a + s * u.clamp(0.0, 1.0))
    } else {
        None
    }
}

/// 線分 × 線分の真の交点（1 点）。平行・共線・区間重なりは `None`。
fn seg_seg(s1: &LineSeg, s2: &LineSeg) -> Option<Point2> {
    let r = s1.direction();
    let s = s2.direction();
    let rl = r.length();
    let sl = s.length();
    // 退化した線分（長さが端点の座標スケールに対する相対許容量以下）は交点計算の対象外。
    if rl <= point_tol(s1.a, s1.b) || sl <= point_tol(s2.a, s2.b) {
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
    // 中心座標と半径の両方でスケールした相対許容量（原点から遠い円でも足りるように）。
    let tol = rel_tol(c.center.to_vec2().length() + c.radius);
    if len <= point_tol(seg.a, seg.b) {
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
    // 中心座標と半径の両方でスケールした相対許容量（`seg_circle` と同じ理由）。
    let tol = rel_tol(c1.center.to_vec2().length() + c2.center.to_vec2().length() + r1 + r2);

    if dist <= point_tol(c1.center, c2.center) {
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
        // pi 以降で pi とほぼ一致するものを除去。許容量は両点の座標スケールから決める。
        let mut j = i + 1;
        while j < pts.len() {
            if pi.distance(pts[j]) <= point_tol(pi, pts[j]) {
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
    fn circle_circle_external_tangent_far_from_origin() {
        // モデリング許容量が座標スケールに比例することの固定（lib.rs の `rel_tol` doc
        // 「平行移動に対する非不変性」参照）: 原点から 1e6 離れた場所では、その場所の
        // 許容量（≒2.8e-3）未満の隙間（ここでは 1e-5）は「実質接している」ものとして
        // 接線 1 点へ併合される。これは丸め誤差の吸収ではなく **意図した挙動** —
        // 設計範囲の端では許容量 ≈ 1µm 相当で、製図上の最小有意寸法を常に下回る。
        // 旧・半径のみスケールの許容量（`EPS * (r1 + r2)` = 1e-8）では同じ隙間が
        // 「離れすぎ」（交点 0 個）になり、判定が図形の置かれた絶対座標に過敏だった。
        let base = 1.0e6;
        let gap = 1.0e-5; // 旧許容量より大きく、この場所の許容量（≒2.8e-3）より小さいずれ。
        let c1 = Shape::Circle(Circle::new(Point2::new(base, base), 5.0));
        let c2 = Shape::Circle(Circle::new(Point2::new(base + 10.0 + gap, base), 5.0));
        let pts = intersect(&c1, &c2);
        assert_eq!(pts.len(), 1, "外接が接線として 1 点に落ちるべき: {pts:?}");
        // 接点は両円の中点付近（gap の半分だけ理想位置からずれる）。
        assert!(pts[0].distance(Point2::new(base + 5.0, base)) < 1.0e-3);
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

    // --- line_line_intersection / line_circle_intersection（M12 タスク87、仮想交点用） ---

    #[test]
    fn line_line_intersection_typical_crossing() {
        let a = LineSeg::new(Point2::new(0.0, 0.0), Point2::new(10.0, 10.0));
        let b = LineSeg::new(Point2::new(0.0, 10.0), Point2::new(10.0, 0.0));
        let p = line_line_intersection(&a, &b).unwrap();
        assert!(p.distance(Point2::new(5.0, 5.0)) < 1e-9);
    }

    #[test]
    fn line_line_intersection_finds_extension_point() {
        // 両線分の区間の外（延長線上）で交わる。seg_seg なら None だが、
        // 無限直線とみなす line_line_intersection は交点を返す。
        let a = LineSeg::new(Point2::new(0.0, 0.0), Point2::new(1.0, 1.0));
        let b = LineSeg::new(Point2::new(0.0, 10.0), Point2::new(1.0, 9.0));
        let p = line_line_intersection(&a, &b).unwrap();
        assert!(p.distance(Point2::new(5.0, 5.0)) < 1e-9);
    }

    #[test]
    fn line_line_intersection_parallel_is_none() {
        let a = LineSeg::new(Point2::new(0.0, 0.0), Point2::new(10.0, 0.0));
        let b = LineSeg::new(Point2::new(0.0, 1.0), Point2::new(10.0, 1.0));
        assert_eq!(line_line_intersection(&a, &b), None);
    }

    #[test]
    fn line_line_intersection_collinear_is_none() {
        let a = LineSeg::new(Point2::new(0.0, 0.0), Point2::new(10.0, 0.0));
        let b = LineSeg::new(Point2::new(20.0, 0.0), Point2::new(30.0, 0.0));
        assert_eq!(line_line_intersection(&a, &b), None);
    }

    #[test]
    fn line_line_intersection_degenerate_segment_is_none() {
        let degenerate = LineSeg::new(Point2::new(1.0, 1.0), Point2::new(1.0, 1.0));
        let normal = LineSeg::new(Point2::new(0.0, 0.0), Point2::new(10.0, 10.0));
        assert_eq!(line_line_intersection(&degenerate, &normal), None);
        assert_eq!(line_line_intersection(&normal, &degenerate), None);
    }

    #[test]
    fn line_circle_intersection_two_points_on_extension() {
        // 線分区間は円から離れているが、延長した直線は円と2点で交わる。
        let line = LineSeg::new(Point2::new(-10.0, 0.0), Point2::new(-8.0, 0.0));
        let circ = Circle::new(Point2::ORIGIN, 5.0);
        let pts = line_circle_intersection(&line, &circ);
        assert_eq!(pts.len(), 2);
        assert!(contains_approx(&pts, Point2::new(5.0, 0.0)));
        assert!(contains_approx(&pts, Point2::new(-5.0, 0.0)));
    }

    #[test]
    fn line_circle_intersection_tangent_one_point() {
        let line = LineSeg::new(Point2::new(-10.0, 5.0), Point2::new(10.0, 5.0));
        let circ = Circle::new(Point2::ORIGIN, 5.0);
        let pts = line_circle_intersection(&line, &circ);
        assert_eq!(pts.len(), 1);
        assert!(contains_approx(&pts, Point2::new(0.0, 5.0)));
    }

    #[test]
    fn line_circle_intersection_miss_is_empty() {
        let line = LineSeg::new(Point2::new(-10.0, 6.0), Point2::new(10.0, 6.0));
        let circ = Circle::new(Point2::ORIGIN, 5.0);
        assert!(line_circle_intersection(&line, &circ).is_empty());
    }

    #[test]
    fn line_circle_intersection_degenerate_line_is_empty() {
        let line = LineSeg::new(Point2::new(1.0, 1.0), Point2::new(1.0, 1.0));
        let circ = Circle::new(Point2::ORIGIN, 5.0);
        assert!(line_circle_intersection(&line, &circ).is_empty());
    }

    #[test]
    fn line_circle_intersection_non_positive_radius_is_empty() {
        let line = LineSeg::new(Point2::new(-10.0, 0.0), Point2::new(10.0, 0.0));
        assert!(line_circle_intersection(&line, &Circle::new(Point2::ORIGIN, 0.0)).is_empty());
        assert!(line_circle_intersection(&line, &Circle::new(Point2::ORIGIN, -1.0)).is_empty());
    }

    // --- line_shape_intersection（M12 タスク91、構築線用） ---

    #[test]
    fn line_shape_intersection_finds_segment_crossing_outside_line_interval() {
        // 直線側の定義区間（0..1）の遥か外でも、線分側の区間内なら交点になる。
        let line = LineSeg::new(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0));
        let seg = shape_line(100.0, -5.0, 100.0, 5.0);
        let pts = line_shape_intersection(&line, &seg);
        assert_eq!(pts.len(), 1);
        assert!(contains_approx(&pts, Point2::new(100.0, 0.0)));
    }

    #[test]
    fn line_shape_intersection_rejects_crossing_outside_segment() {
        // 直線と線分の延長は (100, 0) で交わるが、線分の区間外なので交点なし。
        let line = LineSeg::new(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0));
        let seg = shape_line(100.0, 1.0, 100.0, 5.0);
        assert!(line_shape_intersection(&line, &seg).is_empty());
    }

    #[test]
    fn line_shape_intersection_parallel_and_collinear_are_empty() {
        let line = LineSeg::new(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0));
        assert!(line_shape_intersection(&line, &shape_line(0.0, 1.0, 5.0, 1.0)).is_empty());
        assert!(line_shape_intersection(&line, &shape_line(3.0, 0.0, 5.0, 0.0)).is_empty());
    }

    #[test]
    fn line_shape_intersection_circle_arc_polyline_and_point() {
        let line = LineSeg::new(Point2::new(-1.0, 0.0), Point2::new(0.0, 0.0));
        let circle = Shape::Circle(Circle::new(Point2::new(20.0, 0.0), 5.0));
        let pts = line_shape_intersection(&line, &circle);
        assert_eq!(pts.len(), 2);
        assert!(contains_approx(&pts, Point2::new(15.0, 0.0)));
        assert!(contains_approx(&pts, Point2::new(25.0, 0.0)));

        // 上半円の弧（0..π）は両端 (15,0)・(25,0) を含む。第 1 象限だけなら (25,0) のみ。
        let quarter = Shape::Arc(Arc::new(Point2::new(20.0, 0.0), 5.0, 0.0, FRAC_PI_2));
        let pts = line_shape_intersection(&line, &quarter);
        assert_eq!(pts.len(), 1);
        assert!(contains_approx(&pts, Point2::new(25.0, 0.0)));

        let pl = Shape::Polyline(Polyline::new(
            vec![
                Point2::new(30.0, -1.0),
                Point2::new(30.0, 1.0),
                Point2::new(32.0, 1.0),
                Point2::new(32.0, -1.0),
            ],
            false,
        ));
        let pts = line_shape_intersection(&line, &pl);
        assert_eq!(pts.len(), 2);
        assert!(contains_approx(&pts, Point2::new(30.0, 0.0)));
        assert!(contains_approx(&pts, Point2::new(32.0, 0.0)));

        let on = Shape::Point(Point2::new(-50.0, 0.0));
        let off = Shape::Point(Point2::new(-50.0, 0.5));
        assert_eq!(line_shape_intersection(&line, &on).len(), 1);
        assert!(line_shape_intersection(&line, &off).is_empty());
    }

    // --- clip_line_to_aabb（M12 タスク91、構築線の描画・出力の切り取り） ---

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Aabb {
        Aabb::from_corners(Point2::new(x0, y0), Point2::new(x1, y1))
    }

    fn seg_approx(s: &LineSeg, a: Point2, b: Point2) -> bool {
        s.a.distance(a) < 1e-9 && s.b.distance(b) < 1e-9
    }

    #[test]
    fn clip_line_horizontal_and_vertical_span_the_rect() {
        let r = rect(0.0, 0.0, 10.0, 5.0);
        // 基点が矩形の外（左）でも、直線が横切れば矩形の幅いっぱいの線分になる。
        let h = LineSeg::new(Point2::new(-100.0, 2.0), Point2::new(-99.0, 2.0));
        let s = clip_line_to_aabb(&h, &r).unwrap();
        assert!(
            seg_approx(&s, Point2::new(0.0, 2.0), Point2::new(10.0, 2.0)),
            "{s:?}"
        );
        // 向きは入力の直線に従う（-x 向きなら右端 → 左端）。
        let h_rev = LineSeg::new(Point2::new(3.0, 2.0), Point2::new(2.0, 2.0));
        let s = clip_line_to_aabb(&h_rev, &r).unwrap();
        assert!(
            seg_approx(&s, Point2::new(10.0, 2.0), Point2::new(0.0, 2.0)),
            "{s:?}"
        );
        let v = LineSeg::new(Point2::new(4.0, 50.0), Point2::new(4.0, 51.0));
        let s = clip_line_to_aabb(&v, &r).unwrap();
        assert!(
            seg_approx(&s, Point2::new(4.0, 0.0), Point2::new(4.0, 5.0)),
            "{s:?}"
        );
    }

    #[test]
    fn clip_line_diagonal_and_miss() {
        let r = rect(0.0, 0.0, 10.0, 10.0);
        // 対角線はちょうど 2 つの角を通る。
        let diag = LineSeg::new(Point2::new(20.0, 20.0), Point2::new(21.0, 21.0));
        let s = clip_line_to_aabb(&diag, &r).unwrap();
        assert!(
            seg_approx(&s, Point2::new(0.0, 0.0), Point2::new(10.0, 10.0)),
            "{s:?}"
        );
        // 斜めの直線が矩形の 2 辺を横切る。
        let slope = LineSeg::new(Point2::new(0.0, 5.0), Point2::new(2.0, 6.0));
        let s = clip_line_to_aabb(&slope, &r).unwrap();
        assert!(
            seg_approx(&s, Point2::new(0.0, 5.0), Point2::new(10.0, 10.0)),
            "{s:?}"
        );
        // 矩形の外を通る直線は None（軸平行・斜めとも）。
        let above = LineSeg::new(Point2::new(0.0, 11.0), Point2::new(1.0, 11.0));
        assert_eq!(clip_line_to_aabb(&above, &r), None);
        let far = LineSeg::new(Point2::new(0.0, 25.0), Point2::new(1.0, 24.0));
        assert_eq!(clip_line_to_aabb(&far, &r), None);
    }

    #[test]
    fn clip_line_touching_boundary_is_closed() {
        let r = rect(0.0, 0.0, 10.0, 10.0);
        // 辺の上を通る直線は辺そのもの。
        let edge = LineSeg::new(Point2::new(0.0, 10.0), Point2::new(1.0, 10.0));
        let s = clip_line_to_aabb(&edge, &r).unwrap();
        assert!(
            seg_approx(&s, Point2::new(0.0, 10.0), Point2::new(10.0, 10.0)),
            "{s:?}"
        );
        // 角 (10, 0) だけに触れる直線は長さ 0 の線分。
        let corner = LineSeg::new(Point2::new(10.0, 0.0), Point2::new(11.0, 1.0));
        let s = clip_line_to_aabb(&corner, &r).unwrap();
        assert!(
            seg_approx(&s, Point2::new(10.0, 0.0), Point2::new(10.0, 0.0)),
            "{s:?}"
        );
    }

    #[test]
    fn clip_line_degenerate_or_non_finite_is_none() {
        let r = rect(0.0, 0.0, 10.0, 10.0);
        let degenerate = LineSeg::new(Point2::new(5.0, 5.0), Point2::new(5.0, 5.0));
        assert_eq!(clip_line_to_aabb(&degenerate, &r), None);
        let nan = LineSeg::new(Point2::new(f64::NAN, 5.0), Point2::new(1.0, 5.0));
        assert_eq!(clip_line_to_aabb(&nan, &r), None);
        let inf_rect = rect(0.0, 0.0, f64::INFINITY, 10.0);
        let h = LineSeg::new(Point2::new(0.0, 5.0), Point2::new(1.0, 5.0));
        assert_eq!(clip_line_to_aabb(&h, &inf_rect), None);
    }

    #[test]
    fn clip_line_nearly_axis_parallel_stays_finite() {
        // 回転の丸めで x 成分が極小になった「ほぼ鉛直」の直線。
        let r = rect(-10.0, -10.0, 10.0, 10.0);
        let almost = LineSeg::new(Point2::new(1.0, 0.0), Point2::new(1.0 + 1e-17, 1.0));
        let s = clip_line_to_aabb(&almost, &r).unwrap();
        assert!(
            seg_approx(&s, Point2::new(1.0, -10.0), Point2::new(1.0, 10.0)),
            "{s:?}"
        );
    }

    #[test]
    fn line_shape_intersection_degenerate_line_is_empty() {
        let line = LineSeg::new(Point2::new(1.0, 1.0), Point2::new(1.0, 1.0));
        assert!(line_shape_intersection(&line, &shape_line(0.0, 0.0, 2.0, 2.0)).is_empty());
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
