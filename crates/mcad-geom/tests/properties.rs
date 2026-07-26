//! `proptest` によるプロパティテスト。
//!
//! - 線分上の点から最近点を求めると、その点自身が返る。
//! - 実際に交わる 2 円の交点は、両円の境界上にある。
//! - 交わる 2 線分の交点は、両線分上にある。
//! - 線分×円の交点は、円周上かつ線分上にある。
//! - 回転の往復不変性: `rotated(pivot, θ)→rotated(pivot, -θ)` で元に戻る。
//! - 鏡映の往復不変性: 同一軸で 2 回鏡映すると元に戻る。
//! - 通過点方式のオフセット（線分・円）は通過点を通る結果を作る。
//! - 半径不変性: `Circle`/`Arc` は回転・鏡映前後で半径が変わらない。
//! - トリムの断片は元の対象上に載り、合計長が元を超えない。
//! - 延長は固定端を保存し、新しい自由端は元の直線上かつ境界上にある。
//! - `extend_reach` は境界 AABB 内の任意の点までの距離の上界になっている。
//! - 分割: 線分・弧・開いたポリラインの往復性（2 ピースの端点が元の端点と分割点に一致）。
//! - フィレット: 弧が両直線に接し（中心から各直線までの距離＝半径）、両端点が
//!   トリム済み線分の切断端に一致し、掃引が π 以下に収まる。

use mcad_geom::{
    Aabb, Arc, Circle, LineSeg, Point2, Polyline, Shape, closest_point, extend, extend_reach,
    fillet_lines, intersect, split, trim,
};
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

    /// トリムが成功したら、断片は 1〜2 個で、いずれも元の線分上に載り、
    /// 合計長は元の長さを超えない。
    #[test]
    fn trim_line_pieces_lie_on_original(
        a in point(),
        b in point(),
        b1 in point(),
        b2 in point(),
        click in point(),
    ) {
        prop_assume!(a.distance(b) > 1.0);
        let seg = LineSeg::new(a, b);
        let boundary = Shape::Line(LineSeg::new(b1, b2));
        let Ok(res) = trim(&Shape::Line(seg), &boundary, click) else { return Ok(()); };

        prop_assert!(!res.retained.is_empty() && res.retained.len() <= 2,
            "unexpected piece count {}", res.retained.len());
        let mut total = 0.0;
        for piece in &res.retained {
            let Shape::Line(p) = piece else {
                prop_assert!(false, "trim of a line produced {piece:?}");
                unreachable!()
            };
            prop_assert!(seg.closest_point(p.a).distance(p.a) < TOL);
            prop_assert!(seg.closest_point(p.b).distance(p.b) < TOL);
            total += p.length();
        }
        prop_assert!(total <= seg.length() + TOL, "total={total} original={}", seg.length());
    }

    /// 延長が成功したら、固定端は保存され、結果は元の直線上で元より短くならず、
    /// 新しい自由端は境界線分上にある（平行に近い病的配置は除外）。
    #[test]
    fn extend_line_keeps_fixed_end_and_reaches_boundary(
        a in point(),
        b in point(),
        b1 in point(),
        b2 in point(),
        click in point(),
    ) {
        prop_assume!(a.distance(b) > 1.0);
        prop_assume!(b1.distance(b2) > 1.0);
        let seg = LineSeg::new(a, b);
        let bseg = LineSeg::new(b1, b2);
        // 交点の条件数が悪い（ほぼ平行な）配置は除外する。sin(角) > 0.1 ≒ 5.7 度以上。
        let (d1, d2) = (seg.direction(), bseg.direction());
        prop_assume!(d1.cross(d2).abs() > 0.1 * d1.length() * d2.length());

        let Ok(result) = extend(&Shape::Line(seg), &Shape::Line(bseg), click) else { return Ok(()); };
        let Shape::Line(res) = result else {
            prop_assert!(false, "extend of a line produced {result:?}");
            unreachable!()
        };

        // 自由端はクリックに近い方（`extend` の契約）。
        let extend_b = click.distance_squared(b) <= click.distance_squared(a);
        let (fixed_before, fixed_after, new_free) = if extend_b {
            (a, res.a, res.b)
        } else {
            (b, res.b, res.a)
        };
        prop_assert!(fixed_after.distance(fixed_before) < TOL);
        prop_assert!(res.length() >= seg.length() - TOL);
        // 新しい自由端は元の直線上（無限直線への距離ほぼ 0）かつ境界線分上。
        let along = (new_free - a).cross(d1).abs() / d1.length();
        prop_assert!(along < TOL * (1.0 + res.length()), "off the target line: {along}");
        let off_boundary = bseg.closest_point(new_free).distance(new_free);
        prop_assert!(off_boundary < TOL * (1.0 + res.length()), "off the boundary: {off_boundary}");
    }

    /// `extend_reach` は境界 AABB 内の任意の点までの距離の上界になっている。
    #[test]
    fn extend_reach_bounds_every_point_in_aabb(
        free in point(),
        c1 in point(),
        c2 in point(),
        u in 0.0f64..=1.0f64,
        v in 0.0f64..=1.0f64,
    ) {
        let bb = Aabb::from_corners(c1, c2);
        let inside = Point2::new(bb.min.x + bb.width() * u, bb.min.y + bb.height() * v);
        prop_assert!(free.distance(inside) <= extend_reach(free, bb) + TOL);
    }

    /// 分割（線分）の往復性: ランダムな線分をランダムな内部点で分割すると、
    /// 2 ピースの端点が元の端点と分割点に一致する。
    #[test]
    fn split_line_roundtrip(
        a in point(),
        b in point(),
        t in 0.05f64..0.95f64,
    ) {
        prop_assume!(a.distance(b) > 10.0);
        let seg = LineSeg::new(a, b);
        let at = a.lerp(b, t);
        let Ok((piece1, piece2)) = split(&Shape::Line(seg), at) else { return Ok(()); };
        let (Shape::Line(l1), Shape::Line(l2)) = (piece1, piece2) else {
            prop_assert!(false, "split of a line produced non-line pieces");
            unreachable!()
        };
        prop_assert!(l1.a.distance(a) < TOL, "l1.a={:?} a={:?}", l1.a, a);
        prop_assert!(l1.b.distance(at) < TOL, "l1.b={:?} at={:?}", l1.b, at);
        prop_assert!(l2.a.distance(at) < TOL, "l2.a={:?} at={:?}", l2.a, at);
        prop_assert!(l2.b.distance(b) < TOL, "l2.b={:?} b={:?}", l2.b, b);
    }

    /// 分割（弧）の往復性: ランダムな弧をランダムな内部角で分割すると、
    /// 2 ピースの端点が元の端点と分割点に一致する。
    #[test]
    fn split_arc_roundtrip(
        center in point(),
        radius in radius(),
        start in angle(),
        sweep in 0.2f64..(std::f64::consts::TAU - 0.2),
        frac in 0.05f64..0.95f64,
    ) {
        let arc = Arc::new(center, radius, start, start + sweep);
        let theta = start + sweep * frac;
        let at = arc.circle().point_at_angle(theta);
        let Ok((piece1, piece2)) = split(&Shape::Arc(arc), at) else { return Ok(()); };
        let (Shape::Arc(a1), Shape::Arc(a2)) = (piece1, piece2) else {
            prop_assert!(false, "split of an arc produced non-arc pieces");
            unreachable!()
        };
        prop_assert!(a1.start_point().distance(arc.start_point()) < TOL);
        prop_assert!(a1.end_point().distance(at) < TOL);
        prop_assert!(a2.start_point().distance(at) < TOL);
        prop_assert!(a2.end_point().distance(arc.end_point()) < TOL);
    }

    /// 分割（開いたポリライン）の往復性: `piece1.vertices` の末尾を除いた列と
    /// `piece2.vertices` を連結すると、「分割点を挿入した元の頂点列」に一致する
    /// （本テストは中間頂点近傍を避けるように配置するので、常に新規頂点挿入の
    /// ケースになる。挿入せず既存頂点で割るケースはユニットテストで別途固定）。
    /// 両ピースとも頂点 2 以上。
    #[test]
    fn split_polyline_roundtrip(
        verts in prop::collection::vec(point(), 3..6),
        seg_idx_frac in 0.0f64..1.0f64,
        t in 0.1f64..0.9f64,
    ) {
        // 隣接頂点間が近すぎない（分割点が中間頂点近傍に落ちない）配置に限定する。
        prop_assume!(verts.windows(2).all(|w| w[0].distance(w[1]) > 5.0));
        let pl = Polyline::new(verts.clone(), false);
        let n = verts.len();
        let seg_count = n - 1;
        let i = ((seg_idx_frac * seg_count as f64) as usize).min(seg_count - 1);
        let at = verts[i].lerp(verts[i + 1], t);

        let Ok((piece1, piece2)) = split(&Shape::Polyline(pl), at) else { return Ok(()); };
        let (Shape::Polyline(pl1), Shape::Polyline(pl2)) = (piece1, piece2) else {
            prop_assert!(false, "split of a polyline produced non-polyline pieces");
            unreachable!()
        };
        prop_assert!(!pl1.closed && !pl2.closed);
        prop_assert!(pl1.vertices.len() >= 2, "piece1 too short: {:?}", pl1.vertices);
        prop_assert!(pl2.vertices.len() >= 2, "piece2 too short: {:?}", pl2.vertices);
        // 全体の端点は保存される。
        prop_assert!(pl1.vertices[0].distance(verts[0]) < TOL);
        prop_assert!(pl2.vertices.last().unwrap().distance(*verts.last().unwrap()) < TOL);
        // 分割点で両ピースが接続する。
        let joint = *pl1.vertices.last().unwrap();
        prop_assert!(joint.distance(pl2.vertices[0]) < TOL);
        prop_assert!(joint.distance(at) < TOL, "joint={joint:?} at={at:?}");
        // 連結すると「分割点を挿入した元の頂点列」に一致する（t in [0.1, 0.9] かつ
        // 隣接頂点間距離 > 5 なので分割点は常に既存頂点近傍にならず、新規挿入になる）。
        let mut expected: Vec<Point2> = verts[..=i].to_vec();
        expected.push(at);
        expected.extend_from_slice(&verts[i + 1..]);
        let mut rejoined: Vec<Point2> = pl1.vertices[..pl1.vertices.len() - 1].to_vec();
        rejoined.extend_from_slice(&pl2.vertices);
        prop_assert_eq!(rejoined.len(), expected.len());
        for (r, e) in rejoined.iter().zip(&expected) {
            prop_assert!(r.distance(*e) < TOL, "r={r:?} e={e:?}");
        }
    }
}

/// 点から無限直線（`seg` を含む）への距離。
fn distance_to_line(seg: &LineSeg, p: Point2) -> f64 {
    let d = seg.direction();
    (p - seg.a).cross(d).abs() / d.length()
}

proptest! {
    /// フィレットが成功したら、弧は両直線に接し（中心から各直線への距離＝半径）、
    /// 掃引は π 以下、両端点はトリム済み線分の「切断された側」の端点に一致し、
    /// 各トリム結果は元の線分上に載って `near` 側の端点を保存する。
    #[test]
    fn fillet_arc_is_tangent_to_both_lines(
        a1 in point(),
        a2 in point(),
        b1 in point(),
        b2 in point(),
        r in 0.5f64..200.0f64,
        near_a in point(),
        near_b in point(),
    ) {
        let a = LineSeg::new(a1, a2);
        let b = LineSeg::new(b1, b2);
        prop_assume!(a.length() > 1.0 && b.length() > 1.0);
        // 交点の条件数が悪い（ほぼ平行な）配置は除外する。sin(角) > 0.1 ≒ 5.7 度以上。
        let (da, db) = (a.direction(), b.direction());
        prop_assume!(da.cross(db).abs() > 0.1 * da.length() * db.length());

        // クリック点は各直線上へ射影して渡す（app 層が渡す形に合わせる）。
        let near_a = a.closest_point(near_a);
        let near_b = b.closest_point(near_b);
        let Ok(res) = fillet_lines(a, b, r, near_a, near_b) else { return Ok(()); };

        // 接している: 中心から両無限直線までの距離が半径に一致する。
        prop_assert!((distance_to_line(&a, res.arc.center) - r).abs() < TOL * (1.0 + r),
            "not tangent to a: {}", distance_to_line(&a, res.arc.center));
        prop_assert!((distance_to_line(&b, res.arc.center) - r).abs() < TOL * (1.0 + r),
            "not tangent to b: {}", distance_to_line(&b, res.arc.center));
        prop_assert!((res.arc.radius - r).abs() < TOL);
        prop_assert!(res.arc.sweep() <= std::f64::consts::PI + TOL,
            "sweep {}", res.arc.sweep());

        for (orig, trimmed) in [(a, res.trimmed_a), (b, res.trimmed_b)] {
            // 元の線分上に載っている。
            prop_assert!(orig.closest_point(trimmed.a).distance(trimmed.a) < TOL);
            prop_assert!(orig.closest_point(trimmed.b).distance(trimmed.b) < TOL);
            prop_assert!(trimmed.length() <= orig.length() + TOL);
            // 片方の端点は元のまま（`near` 側）、もう片方（接点）が弧の端点に一致する。
            prop_assert!(approx_pt(trimmed.a, orig.a) || approx_pt(trimmed.b, orig.b),
                "neither endpoint preserved: {trimmed:?} vs {orig:?}");
            let ends = [res.arc.start_point(), res.arc.end_point()];
            prop_assert!(
                [trimmed.a, trimmed.b].iter().any(|p| ends.iter().any(|e| approx_pt(*p, *e))),
                "{trimmed:?} does not meet the arc ends {ends:?}");
        }
    }
}
