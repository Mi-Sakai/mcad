//! `proptest` によるプロパティテスト。
//!
//! - 線分上の点から最近点を求めると、その点自身が返る。
//! - 実際に交わる 2 円の交点は、両円の境界上にある。
//! - 交わる 2 線分の交点は、両線分上にある。
//! - 線分×円の交点は、円周上かつ線分上にある。

use mcad_geom::{Arc, Circle, LineSeg, Point2, Polyline, Shape, closest_point, intersect};
use proptest::prelude::*;

/// テストで用いる許容量。intersect 内部の相対 EPS(1e-9) と数値誤差の累積を見込み、
/// 座標スケールが数百程度の範囲で十分な緩さ（1e-6）を取る。
const TOL: f64 = 1e-6;

/// 有限で常識的な座標範囲の f64。
fn coord() -> impl Strategy<Value = f64> {
    -1000.0f64..1000.0f64
}

fn point() -> impl Strategy<Value = Point2> {
    (coord(), coord()).prop_map(|(x, y)| Point2::new(x, y))
}

/// 常識的な半径。
fn radius() -> impl Strategy<Value = f64> {
    1.0f64..500.0f64
}

/// 回転角・弧の開始/終了角に使う無次元のラジアン範囲。
fn angle() -> impl Strategy<Value = f64> {
    -10.0f64..10.0f64
}

/// 変換の往復不変性テスト用に、Line/Circle/Arc/Polyline を一様に生成する。
fn any_shape() -> impl Strategy<Value = Shape> {
    prop_oneof![
        (point(), point()).prop_map(|(a, b)| Shape::Line(LineSeg::new(a, b))),
        (point(), radius()).prop_map(|(c, r)| Shape::Circle(Circle::new(c, r))),
        (point(), radius(), angle(), angle())
            .prop_map(|(c, r, s, e)| Shape::Arc(Arc::new(c, r, s, e))),
        // 小さな頂点列のポリライン（1〜4 頂点）。
        (prop::collection::vec(point(), 1..5), any::<bool>())
            .prop_map(|(v, closed)| Shape::Polyline(Polyline::new(v, closed))),
    ]
}

/// 2 点が許容量内で一致するか。
fn approx_pt(a: Point2, b: Point2) -> bool {
    a.distance(b) < TOL
}

/// 2 つの形状が許容量内で一致するか（往復不変性の判定に使う）。
///
/// `Arc` の開始/終了角は往復が浮動小数の加減算・反射で復元されるので、直接差分を
/// `TOL` で比較する（正規化はしない。本型は非正規化のまま角度を保持するため）。
fn shape_approx(a: &Shape, b: &Shape) -> bool {
    let ang = |x: f64, y: f64| (x - y).abs() < TOL;
    match (a, b) {
        (Shape::Point(p), Shape::Point(q)) => approx_pt(*p, *q),
        (Shape::Line(x), Shape::Line(y)) => approx_pt(x.a, y.a) && approx_pt(x.b, y.b),
        (Shape::Circle(x), Shape::Circle(y)) => {
            approx_pt(x.center, y.center) && (x.radius - y.radius).abs() < TOL
        }
        (Shape::Arc(x), Shape::Arc(y)) => {
            approx_pt(x.center, y.center)
                && (x.radius - y.radius).abs() < TOL
                && ang(x.start_angle, y.start_angle)
                && ang(x.end_angle, y.end_angle)
        }
        (Shape::Polyline(x), Shape::Polyline(y)) => {
            x.closed == y.closed
                && x.vertices.len() == y.vertices.len()
                && x.vertices
                    .iter()
                    .zip(&y.vertices)
                    .all(|(p, q)| approx_pt(*p, *q))
        }
        _ => false,
    }
}

/// 形状の半径（`Circle`/`Arc` のみ）。等長変換で不変であることの確認に使う。
fn shape_radius(s: &Shape) -> Option<f64> {
    match s {
        Shape::Circle(c) => Some(c.radius),
        Shape::Arc(a) => Some(a.radius),
        _ => None,
    }
}

proptest! {
    /// 線分上の点から closest_point を呼ぶと、その点（に非常に近い点）が返る。
    #[test]
    fn closest_point_on_segment_returns_self(
        a in point(),
        b in point(),
        t in 0.0f64..1.0f64,
    ) {
        // 退化線分は除外（長さがほぼ 0 だと「線分上の点」が定まらない）。
        prop_assume!(a.distance(b) > 1e-3);
        let on = a.lerp(b, t);
        let shape = Shape::Line(LineSeg::new(a, b));
        let cp = closest_point(&shape, on);
        prop_assert!(cp.distance(on) < TOL, "cp={cp:?} on={on:?}");
    }

    /// 円周上の点から closest_point を呼ぶと、その点（に非常に近い点）が返る。
    #[test]
    fn closest_point_on_circle_returns_self(
        center in point(),
        radius in 1.0f64..500.0f64,
        theta in 0.0f64..std::f64::consts::TAU,
    ) {
        let circle = Circle::new(center, radius);
        let on = circle.point_at_angle(theta);
        let cp = closest_point(&Shape::Circle(circle), on);
        prop_assert!(cp.distance(on) < TOL, "cp={cp:?} on={on:?}");
    }

    /// distance_to は closest_point までの距離に一致し、常に非負。
    #[test]
    fn distance_to_matches_closest(
        a in point(),
        b in point(),
        p in point(),
    ) {
        let shape = Shape::Line(LineSeg::new(a, b));
        let d = mcad_geom::distance_to(&shape, p);
        let cp = closest_point(&shape, p);
        prop_assert!(d >= 0.0);
        prop_assert!((d - p.distance(cp)).abs() < TOL);
    }

    /// 実際に 2 点で交わる 2 円を生成し、交点が両円の境界上にあることを確認する。
    #[test]
    fn circle_circle_intersections_lie_on_both(
        c1 in point(),
        r1 in 5.0f64..300.0f64,
        // 中心間距離の比率と、2 円目の半径を、確実に 2 点交差する範囲で生成。
        dist_ratio in 0.2f64..0.9f64,
        angle in 0.0f64..std::f64::consts::TAU,
        r2_ratio in 0.5f64..1.5f64,
    ) {
        let r2 = r1 * r2_ratio;
        // 2 点交差の条件: |r1 - r2| < d < r1 + r2。
        // d を (|r1-r2|, r1+r2) の内分点として取り、境界の接線ケースを避ける。
        let lo = (r1 - r2).abs();
        let hi = r1 + r2;
        let d = lo + (hi - lo) * dist_ratio;
        // dist_ratio in [0.2,0.9] なので接線から十分離れている。
        let c2 = Point2::new(
            c1.x + d * angle.cos(),
            c1.y + d * angle.sin(),
        );
        let circle1 = Circle::new(c1, r1);
        let circle2 = Circle::new(c2, r2);
        let pts = intersect(&Shape::Circle(circle1), &Shape::Circle(circle2));
        prop_assert_eq!(pts.len(), 2, "expected 2 intersections, got {}", pts.len());
        for p in pts {
            let d1 = p.distance(c1);
            let d2 = p.distance(c2);
            // 半径に対する相対許容量。
            prop_assert!((d1 - r1).abs() < TOL * r1.max(1.0), "off circle1: {}", (d1 - r1).abs());
            prop_assert!((d2 - r2).abs() < TOL * r2.max(1.0), "off circle2: {}", (d2 - r2).abs());
        }
    }

    /// 交わるように構成した 2 線分の交点が、両線分上（最近距離ほぼ 0）にあることを確認する。
    #[test]
    fn segment_segment_intersection_on_both(
        cross in point(),
        d1a in 1.0f64..500.0f64,
        d1b in 1.0f64..500.0f64,
        d2a in 1.0f64..500.0f64,
        d2b in 1.0f64..500.0f64,
        ang1 in 0.0f64..std::f64::consts::PI,
        off in 0.3f64..2.8f64, // 2 本目の角度を 1 本目からずらす（平行回避）。
    ) {
        let ang2 = ang1 + off;
        let dir1 = Point2::new(ang1.cos(), ang1.sin()).to_vec2();
        let dir2 = Point2::new(ang2.cos(), ang2.sin()).to_vec2();
        // 交点 cross を内部に含むように両側へ伸ばした線分。
        let s1 = LineSeg::new(cross - dir1 * d1a, cross + dir1 * d1b);
        let s2 = LineSeg::new(cross - dir2 * d2a, cross + dir2 * d2b);
        let pts = intersect(&Shape::Line(s1), &Shape::Line(s2));
        prop_assert_eq!(pts.len(), 1, "expected 1 intersection, got {}", pts.len());
        let p = pts[0];
        // 交点は両線分上（最近点までの距離ほぼ 0）。
        prop_assert!(s1.closest_point(p).distance(p) < TOL);
        prop_assert!(s2.closest_point(p).distance(p) < TOL);
        // 想定した交点にも近い。
        prop_assert!(p.distance(cross) < TOL);
    }

    /// 線分×円の交点は、円周上かつ線分上にある。
    #[test]
    fn line_circle_intersections_on_both(
        center in point(),
        radius in 5.0f64..300.0f64,
        theta_a in 0.0f64..std::f64::consts::TAU,
        theta_b in 0.0f64..std::f64::consts::TAU,
    ) {
        // 円周上の 2 点を結ぶ弦を作り、外側へ延長した線分にする。
        // これで必ず 2 交点を持つ（2 点が十分離れている場合）。
        prop_assume!((theta_a - theta_b).abs() > 0.2 && (theta_a - theta_b).abs() < std::f64::consts::TAU - 0.2);
        let circle = Circle::new(center, radius);
        let pa = circle.point_at_angle(theta_a);
        let pb = circle.point_at_angle(theta_b);
        // 弦を両端で外へ延長。
        let dir = (pb - pa).normalize().unwrap();
        let s = LineSeg::new(pa - dir * 10.0, pb + dir * 10.0);
        let pts = intersect(&Shape::Line(s), &Shape::Circle(circle));
        prop_assert_eq!(pts.len(), 2, "expected 2 intersections, got {}", pts.len());
        for p in pts {
            prop_assert!((p.distance(center) - radius).abs() < TOL * radius.max(1.0));
            prop_assert!(s.closest_point(p).distance(p) < TOL);
        }
    }

    /// 線分×弧の交点は、弧の台円上かつ弧の角度範囲内、かつ線分上にある。
    #[test]
    fn line_arc_intersections_valid(
        center in point(),
        radius in 5.0f64..300.0f64,
        start in 0.0f64..std::f64::consts::TAU,
        sweep in 0.5f64..(std::f64::consts::TAU - 0.5),
        chord_t in 0.2f64..0.8f64,
    ) {
        let arc = Arc::new(center, radius, start, start + sweep);
        // 弧の内部の 1 点を通る接線でない弦を作る。弧中央付近の点を通す縦横断線。
        let mid_angle = start + sweep * chord_t;
        let on_arc = arc.circle().point_at_angle(mid_angle);
        // 中心を通る直線にすると必ず円と 2 点で交わる。
        let dir = (on_arc - center).normalize().unwrap();
        let s = LineSeg::new(center - dir * (radius + 10.0), center + dir * (radius + 10.0));
        let pts = intersect(&Shape::Line(s), &Shape::Arc(arc));
        // 交点は 0〜2 個。存在するものは全条件を満たす。
        for p in &pts {
            prop_assert!((p.distance(center) - radius).abs() < TOL * radius.max(1.0));
            prop_assert!(s.closest_point(*p).distance(*p) < TOL);
            let ang = (*p - center).angle();
            prop_assert!(arc.contains_angle(ang), "angle {ang} not in arc");
        }
        // mid_angle の点は範囲内なので、その側の交点は必ず含まれる。
        prop_assert!(pts.iter().any(|p| p.distance(on_arc) < TOL),
            "expected arc point {on_arc:?} among {pts:?}");
    }

    /// 回転の往復不変性: 任意の形状・中心・角度で `rotated(pivot, θ)` の後に
    /// `rotated(pivot, -θ)` を適用すると元の形状へ戻る。
    #[test]
    fn rotate_roundtrip_is_identity(
        shape in any_shape(),
        pivot in point(),
        theta in angle(),
    ) {
        let back = shape.rotated(pivot, theta).rotated(pivot, -theta);
        prop_assert!(shape_approx(&shape, &back), "shape={shape:?} back={back:?}");
    }

    /// 鏡映の往復不変性: 同一軸で 2 回鏡映すると元の形状へ戻る。
    #[test]
    fn mirror_twice_is_identity(
        shape in any_shape(),
        axis_a in point(),
        axis_ang in 0.0f64..std::f64::consts::TAU,
        axis_len in 1.0f64..200.0f64,
    ) {
        // 退化しない軸を、始点＋方向角＋正の長さで構成する。
        let axis_b = Point2::new(
            axis_a.x + axis_len * axis_ang.cos(),
            axis_a.y + axis_len * axis_ang.sin(),
        );
        let back = shape.mirrored(axis_a, axis_b).mirrored(axis_a, axis_b);
        prop_assert!(shape_approx(&shape, &back), "shape={shape:?} back={back:?}");
    }

    /// 通過点方式のオフセット（線分）は、通過点を通る結果を作る。
    /// `distance = distance_to(shape, p)` でオフセットすると、結果への `p` の最短距離は 0。
    ///
    /// 通過点は「セグメント内部の垂足から法線方向へ離れた点」として構成する。
    /// セグメント端で最近点がクランプされる場合、`distance_to` は垂線距離ではなく
    /// 端点距離になり、平行移動オフセットは通過点を通らない（無限直線ではなく線分の
    /// 仕様どおり）。この不変性は垂足がセグメント内部にあるときのみ成り立つため。
    #[test]
    fn line_offset_through_point_lies_on_result(
        a in point(),
        b in point(),
        t in 0.15f64..0.85f64,
        sign in prop::bool::ANY,
        dist in 0.5f64..500.0f64,
    ) {
        prop_assume!(a.distance(b) > 1e-3);
        let dir = (b - a).normalize().unwrap();
        let normal = dir.perp() * if sign { 1.0 } else { -1.0 };
        let foot = a.lerp(b, t);
        let p = foot + normal * dist;
        let shape = Shape::Line(LineSeg::new(a, b));
        let d = mcad_geom::distance_to(&shape, p);
        // 垂足がセグメント内部なので distance_to は垂線距離 dist に一致する。
        prop_assert!((d - dist).abs() < TOL, "d={d} dist={dist}");
        let off = shape.offset(d, p).expect("non-degenerate line offset");
        prop_assert!(mcad_geom::distance_to(&off, p) < TOL, "off={off:?} p={p:?}");
    }

    /// 通過点方式のオフセット（円）は、通過点を通る結果を作る（外側・内側どちらでも）。
    #[test]
    fn circle_offset_through_point_lies_on_result(
        center in point(),
        radius in radius(),
        p in point(),
    ) {
        // 中心近傍は内側オフセットで半径が消滅するため除外。
        prop_assume!(p.distance(center) > 1e-3);
        let shape = Shape::Circle(Circle::new(center, radius));
        let d = mcad_geom::distance_to(&shape, p);
        prop_assume!(d > 1e-3);
        let off = shape.offset(d, p).expect("non-degenerate circle offset");
        prop_assert!(
            mcad_geom::distance_to(&off, p) < TOL * radius.max(1.0),
            "off={off:?} p={p:?}"
        );
    }

    /// 半径不変性: `Circle`/`Arc` は回転・鏡映の前後で半径が変わらない。
    #[test]
    fn rotate_and_mirror_preserve_radius(
        center in point(),
        r in radius(),
        start in angle(),
        end in angle(),
        pivot in point(),
        theta in angle(),
        axis_a in point(),
        axis_ang in 0.0f64..std::f64::consts::TAU,
        axis_len in 1.0f64..200.0f64,
        use_arc in any::<bool>(),
    ) {
        let shape = if use_arc {
            Shape::Arc(Arc::new(center, r, start, end))
        } else {
            Shape::Circle(Circle::new(center, r))
        };
        let axis_b = Point2::new(
            axis_a.x + axis_len * axis_ang.cos(),
            axis_a.y + axis_len * axis_ang.sin(),
        );
        let rotated = shape.rotated(pivot, theta);
        let mirrored = shape.mirrored(axis_a, axis_b);
        prop_assert!((shape_radius(&rotated).unwrap() - r).abs() < TOL);
        prop_assert!((shape_radius(&mirrored).unwrap() - r).abs() < TOL);
    }
}
