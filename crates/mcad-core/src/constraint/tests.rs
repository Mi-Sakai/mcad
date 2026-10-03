//! 拘束のデータモデルのテスト: コンストラクタ（側・外接/内接・値の決め方）、接点と
//! 区間外判定、`ReshapeEntity` の削除規則（純関数）。
//!
//! 文書へ適用したときの挙動（undo/redo・拒否・連鎖削除）は `document/constraint_tests.rs`、
//! 方程式とヤコビアンは `constraint/equations/tests.rs`。

use std::f64::consts::{FRAC_PI_2, PI};

use mcad_geom::{Circle, Polyline};
use slotmap::SlotMap;

use super::*;
use crate::{Command, Entity, Style};

fn add(doc: &mut Document, shape: Shape) -> EntityId {
    let layer = doc.current_layer();
    doc.apply(Command::AddEntity(Entity::new(
        shape,
        layer,
        Style::inherited(),
    )))
    .unwrap()
    .entities[0]
}

fn line_shape(a: (f64, f64), b: (f64, f64)) -> Shape {
    Shape::Line(LineSeg::new(Point2::new(a.0, a.1), Point2::new(b.0, b.1)))
}

fn line(doc: &mut Document, a: (f64, f64), b: (f64, f64)) -> EntityId {
    add(doc, line_shape(a, b))
}

fn circle_shape(c: (f64, f64), r: f64) -> Shape {
    Shape::Circle(Circle::new(Point2::new(c.0, c.1), r))
}

fn circle(doc: &mut Document, c: (f64, f64), r: f64) -> EntityId {
    add(doc, circle_shape(c, r))
}

fn arc_shape(c: (f64, f64), r: f64, start: f64, end: f64) -> Shape {
    Shape::Arc(Arc::new(Point2::new(c.0, c.1), r, start, end))
}

fn arc(doc: &mut Document, c: (f64, f64), r: f64, start: f64, end: f64) -> EntityId {
    add(doc, arc_shape(c, r, start, end))
}

fn start(e: EntityId) -> PointRef {
    PointRef::new(e, PointKind::Start)
}

fn end(e: EntityId) -> PointRef {
    PointRef::new(e, PointKind::End)
}

fn center(e: EntityId) -> PointRef {
    PointRef::new(e, PointKind::Center)
}

fn is_invalid<T: std::fmt::Debug>(r: &Result<T, CoreError>) -> bool {
    matches!(r, Err(CoreError::InvalidConstraint(_)))
}

// ---------------------------------------------------------------------------
// 基本の読み取り
// ---------------------------------------------------------------------------

#[test]
fn side_and_larger_signs_are_plus_minus_one() {
    assert_eq!(Side::Left.sign(), 1.0);
    assert_eq!(Side::Right.sign(), -1.0);
    assert_eq!(LargerCircle::First.sign(), 1.0);
    assert_eq!(LargerCircle::Second.sign(), -1.0);
}

#[test]
fn point_positions() {
    let mut doc = Document::new();
    let l = line(&mut doc, (1.0, 2.0), (3.0, 4.0));
    let c = circle(&mut doc, (5.0, 6.0), 2.0);
    let a = arc(&mut doc, (0.0, 0.0), 2.0, 0.0, FRAC_PI_2);
    assert_eq!(start(l).position(&doc), Some(Point2::new(1.0, 2.0)));
    assert_eq!(end(l).position(&doc), Some(Point2::new(3.0, 4.0)));
    assert_eq!(center(l).position(&doc), None);
    assert_eq!(center(c).position(&doc), Some(Point2::new(5.0, 6.0)));
    assert_eq!(start(c).position(&doc), None);
    assert_eq!(start(a).position(&doc), Some(Point2::new(2.0, 0.0)));
    let e = end(a).position(&doc).unwrap();
    assert!(e.x.abs() < 1e-12 && (e.y - 2.0).abs() < 1e-12);
    assert_eq!(center(a).position(&doc), Some(Point2::ORIGIN));
}

#[test]
fn entities_and_value_accessors() {
    let mut doc = Document::new();
    let l = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    let c = circle(&mut doc, (5.0, 5.0), 5.0);
    let length = Constraint::Distance {
        a: start(l),
        b: end(l),
        value: 10.0,
    };
    // 同じ線分の 2 点を指しても、エンティティは 1 回だけ。
    assert_eq!(length.entities(), vec![l]);
    assert_eq!(length.value(), Some(10.0));
    let tangent = Constraint::TangentLineCircle {
        line: l,
        circle: c,
        side: Side::Left,
    };
    assert_eq!(tangent.entities(), vec![l, c]);
    assert!(tangent.references(l) && tangent.references(c));
    assert_eq!(tangent.value(), None);
    assert_eq!(tangent.with_value(1.0), None);
    assert_eq!(
        length.with_value(4.0),
        Some(Constraint::Distance {
            a: start(l),
            b: end(l),
            value: 4.0
        })
    );
}

#[test]
fn angle_values_are_normalized_into_half_open_range() {
    assert_eq!(normalize_angle(0.5), 0.5);
    assert_eq!(normalize_angle(PI), PI);
    assert_eq!(normalize_angle(-PI), PI);
    assert!((normalize_angle(PI + 0.25) - (-PI + 0.25)).abs() < 1e-12);
    assert!((normalize_angle(-3.0 * PI + 0.5) - (-PI + 0.5)).abs() < 1e-12);
    assert!((normalize_angle(7.0 * PI) - PI).abs() < 1e-12);
    for a in [-20.0, -PI, -1.0, 0.0, 3.0, PI, 4.0, 100.0] {
        let n = normalize_angle(a);
        assert!(n > -PI && n <= PI, "{a} -> {n}");
    }
}

// ---------------------------------------------------------------------------
// コンストラクタ
// ---------------------------------------------------------------------------

#[test]
fn distance_constructor_uses_current_distance_and_rejects_coincident_points() {
    let mut doc = Document::new();
    let l1 = line(&mut doc, (0.0, 0.0), (3.0, 4.0));
    let l2 = line(&mut doc, (3.0, 4.0), (9.0, 4.0));
    let c = Constraint::distance(&doc, start(l1), end(l1)).unwrap();
    assert_eq!(c.value(), Some(5.0));
    assert_eq!(c.is_satisfied(&doc), Ok(true));
    // 一致している 2 点（距離 0）は拒否（一致拘束を使う）。
    assert!(is_invalid(&Constraint::distance(&doc, end(l1), start(l2))));
    // 同じ点・種類の合わない点。
    assert!(is_invalid(&Constraint::distance(&doc, end(l1), end(l1))));
    assert!(is_invalid(&Constraint::distance(&doc, center(l1), end(l2))));
}

#[test]
fn point_line_distance_constructor_picks_side_from_current_shape() {
    let mut doc = Document::new();
    let l = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    let above = circle(&mut doc, (3.0, 4.0), 1.0);
    let below = circle(&mut doc, (3.0, -2.5), 1.0);
    let on = circle(&mut doc, (30.0, 0.0), 1.0);
    let with = |point, side, value| Constraint::PointLineDistance {
        point,
        line: l,
        side,
        value,
    };
    // 始点 → 終点（+x）の向きに対して上が左。
    assert_eq!(
        Constraint::point_line_distance(&doc, center(above), l).unwrap(),
        with(center(above), Side::Left, 4.0)
    );
    assert_eq!(
        Constraint::point_line_distance(&doc, center(below), l).unwrap(),
        with(center(below), Side::Right, 2.5)
    );
    // 延長した直線上の点は左側・距離 0。
    let c = Constraint::point_line_distance(&doc, center(on), l).unwrap();
    assert_eq!(c, with(center(on), Side::Left, 0.0));
    assert_eq!(c.is_satisfied(&doc), Ok(true));
    // 線分自身の点、退化した線分は拒否。
    assert!(is_invalid(&Constraint::point_line_distance(
        &doc,
        end(l),
        l
    )));
    let dot = line(&mut doc, (1.0, 1.0), (1.0, 1.0));
    assert!(is_invalid(&Constraint::point_line_distance(
        &doc,
        center(above),
        dot
    )));
}

#[test]
fn radius_and_fix_point_constructors_capture_current_values() {
    let mut doc = Document::new();
    let a = arc(&mut doc, (1.0, 1.0), 4.0, 0.0, PI);
    let l = line(&mut doc, (0.0, 0.0), (1.0, 0.0));
    assert_eq!(
        Constraint::radius(&doc, a).unwrap(),
        Constraint::Radius {
            entity: a,
            value: 4.0
        }
    );
    assert!(is_invalid(&Constraint::radius(&doc, l)));
    let zero = circle(&mut doc, (0.0, 0.0), 0.0);
    assert!(is_invalid(&Constraint::radius(&doc, zero)));

    let fix = Constraint::fix_point(&doc, start(a)).unwrap();
    assert_eq!(
        fix,
        Constraint::FixPoint {
            point: start(a),
            at: Point2::new(5.0, 1.0)
        }
    );
    assert_eq!(fix.is_satisfied(&doc), Ok(true));
    assert!(is_invalid(&Constraint::fix_point(&doc, center(l))));
}

#[test]
fn angle_constructor_measures_signed_angle() {
    let mut doc = Document::new();
    let a = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    let b = line(&mut doc, (0.0, 0.0), (0.0, 5.0));
    let ab = Constraint::angle(&doc, a, b).unwrap();
    assert_eq!(ab.value(), Some(FRAC_PI_2));
    let ba = Constraint::angle(&doc, b, a).unwrap();
    assert_eq!(ba.value(), Some(-FRAC_PI_2));
    assert_eq!(ab.is_satisfied(&doc), Ok(true));
    assert_eq!(ba.is_satisfied(&doc), Ok(true));
    assert!(is_invalid(&Constraint::angle(&doc, a, a)));
    let dot = line(&mut doc, (1.0, 1.0), (1.0, 1.0));
    assert!(is_invalid(&Constraint::angle(&doc, a, dot)));
}

#[test]
fn tangent_line_circle_constructor_picks_side() {
    let mut doc = Document::new();
    let l = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    let side_of = |doc: &Document, c| match Constraint::tangent_line_circle(doc, l, c) {
        Ok(Constraint::TangentLineCircle { side, .. }) => side,
        other => panic!("unexpected {other:?}"),
    };
    let above = circle(&mut doc, (5.0, 3.0), 3.0);
    let below = arc(&mut doc, (5.0, -3.0), 3.0, 0.0, PI);
    let on = circle(&mut doc, (5.0, 0.0), 3.0);
    // 許容値以下のずれは「直線上」として左側。
    let nearly_on = circle(&mut doc, (5.0, -1e-12), 3.0);
    let slightly_below = circle(&mut doc, (5.0, -1e-6), 3.0);
    assert_eq!(side_of(&doc, above), Side::Left);
    assert_eq!(side_of(&doc, below), Side::Right);
    assert_eq!(side_of(&doc, on), Side::Left);
    assert_eq!(side_of(&doc, nearly_on), Side::Left);
    assert_eq!(side_of(&doc, slightly_below), Side::Right);
    // 役割の取り違え・退化した線分は拒否。
    assert!(is_invalid(&Constraint::tangent_line_circle(&doc, above, l)));
    let dot = line(&mut doc, (1.0, 1.0), (1.0, 1.0));
    assert!(is_invalid(&Constraint::tangent_line_circle(
        &doc, dot, above
    )));
}

#[test]
fn tangent_circles_constructor_picks_nearer_kind() {
    let mut doc = Document::new();
    let big = circle(&mut doc, (0.0, 0.0), 10.0);
    let kind_of = |doc: &Document, a, b| match Constraint::tangent_circles(doc, a, b) {
        Ok(Constraint::TangentCircles { kind, .. }) => kind,
        other => panic!("unexpected {other:?}"),
    };
    // 外に離れた円 → 外接。
    let outside = circle(&mut doc, (14.0, 0.0), 3.0);
    assert_eq!(kind_of(&doc, big, outside), CircleTangency::External);
    // 中に入った小さい円 → 内接（大きい方を保存。選択順で First / Second が入れ替わる）。
    let inside = circle(&mut doc, (6.0, 0.0), 3.0);
    assert_eq!(
        kind_of(&doc, big, inside),
        CircleTangency::Internal(LargerCircle::First)
    );
    assert_eq!(
        kind_of(&doc, inside, big),
        CircleTangency::Internal(LargerCircle::Second)
    );
    // 外接と内接が同じ近さ（D = max(r1, r2)）なら外接。
    let tie = circle(&mut doc, (10.0, 0.0), 3.0);
    assert_eq!(kind_of(&doc, big, tie), CircleTangency::External);
    // 等半径で内接の方が近い（同心に近い）形は拒否。
    let same = circle(&mut doc, (1.0, 0.0), 10.0);
    assert!(is_invalid(&Constraint::tangent_circles(&doc, big, same)));
    let concentric_same = circle(&mut doc, (0.0, 0.0), 10.0);
    assert!(is_invalid(&Constraint::tangent_circles(
        &doc,
        big,
        concentric_same
    )));
    // 等半径でも、離れていれば外接として作れる。
    let same_far = circle(&mut doc, (19.0, 0.0), 10.0);
    assert_eq!(kind_of(&doc, big, same_far), CircleTangency::External);
    // 同じ円どうし・線分は拒否。
    assert!(is_invalid(&Constraint::tangent_circles(&doc, big, big)));
    let l = line(&mut doc, (0.0, 0.0), (1.0, 0.0));
    assert!(is_invalid(&Constraint::tangent_circles(&doc, big, l)));
}

#[test]
fn internal_tangency_requires_marked_circle_to_be_larger() {
    let mut doc = Document::new();
    let big = circle(&mut doc, (0.0, 0.0), 10.0);
    let small = circle(&mut doc, (7.0, 0.0), 3.0);
    let same = circle(&mut doc, (0.0, 0.0), 10.0);
    let internal = |a, b, larger| Constraint::TangentCircles {
        a,
        b,
        kind: CircleTangency::Internal(larger),
    };
    assert_eq!(
        internal(big, small, LargerCircle::First).validate(&doc),
        Ok(())
    );
    assert_eq!(
        internal(small, big, LargerCircle::Second).validate(&doc),
        Ok(())
    );
    // 大小の食い違い・等半径は拒否。
    assert!(is_invalid(
        &internal(big, small, LargerCircle::Second).validate(&doc)
    ));
    assert!(is_invalid(
        &internal(big, same, LargerCircle::First).validate(&doc)
    ));
    assert!(is_invalid(
        &internal(big, same, LargerCircle::Second).validate(&doc)
    ));
}

// ---------------------------------------------------------------------------
// 接点と区間外判定（完了条件 5）
// ---------------------------------------------------------------------------

/// ±π をまたいで掃引する円弧（3π/4 → −3π/4 の CCW、掃引 π/2。左側の 1/4）。
const WRAP_START: f64 = 0.75 * PI;
const WRAP_END: f64 = -0.75 * PI;

fn assert_point(p: Point2, want: (f64, f64)) {
    assert!(
        (p.x - want.0).abs() < 1e-9 && (p.y - want.1).abs() < 1e-9,
        "{p:?} != {want:?}"
    );
}

#[test]
fn external_tangency_contact_inside_and_outside_wrapping_sweep() {
    let mut doc = Document::new();
    let a = arc(&mut doc, (1.0, 2.0), 10.0, WRAP_START, WRAP_END);
    // 角度 π（掃引範囲の中。±π をまたぐ位置）で外接する円。
    let left = circle(&mut doc, (-12.0, 2.0), 3.0);
    // 角度 0（掃引範囲の外）で外接する円。
    let right = circle(&mut doc, (14.0, 2.0), 3.0);
    for (other, want, outside) in [(left, (-9.0, 2.0), false), (right, (11.0, 2.0), true)] {
        let c = Constraint::tangent_circles(&doc, a, other).unwrap();
        assert_eq!(
            c,
            Constraint::TangentCircles {
                a,
                b: other,
                kind: CircleTangency::External
            }
        );
        // 接点が掃引範囲の外でも、拘束は満たされる。
        assert_eq!(c.is_satisfied(&doc), Ok(true));
        let contact = c.tangent_contact(&doc).unwrap();
        assert_point(contact.point, want);
        assert_eq!(contact.first_outside, outside);
        assert!(!contact.second_outside, "a full circle is never outside");
        assert_eq!(contact.is_extended(), outside);
        // 選択順を逆にすると、外かどうかは 2 つ目（円弧）の側に出る。
        let swapped = Constraint::tangent_circles(&doc, other, a).unwrap();
        let contact = swapped.tangent_contact(&doc).unwrap();
        assert_point(contact.point, want);
        assert!(!contact.first_outside);
        assert_eq!(contact.second_outside, outside);
    }
}

#[test]
fn internal_tangency_contact_inside_and_outside_wrapping_sweep() {
    let mut doc = Document::new();
    let a = arc(&mut doc, (1.0, 2.0), 10.0, WRAP_START, WRAP_END);
    // 円弧の円の内側で、角度 π（範囲内）・角度 0（範囲外）で内接する小さい円。
    let left = circle(&mut doc, (-6.0, 2.0), 3.0);
    let right = circle(&mut doc, (8.0, 2.0), 3.0);
    for (other, want, outside) in [(left, (-9.0, 2.0), false), (right, (11.0, 2.0), true)] {
        let c = Constraint::tangent_circles(&doc, a, other).unwrap();
        assert_eq!(
            c,
            Constraint::TangentCircles {
                a,
                b: other,
                kind: CircleTangency::Internal(LargerCircle::First)
            }
        );
        assert_eq!(c.is_satisfied(&doc), Ok(true));
        let contact = c.tangent_contact(&doc).unwrap();
        assert_point(contact.point, want);
        assert_eq!(
            (contact.first_outside, contact.second_outside),
            (outside, false)
        );
        // 小さい方を先に選んだ場合（2 つ目が大きい内接）。
        let swapped = Constraint::tangent_circles(&doc, other, a).unwrap();
        assert_eq!(
            swapped,
            Constraint::TangentCircles {
                a: other,
                b: a,
                kind: CircleTangency::Internal(LargerCircle::Second)
            }
        );
        assert_eq!(swapped.is_satisfied(&doc), Ok(true));
        let contact = swapped.tangent_contact(&doc).unwrap();
        assert_point(contact.point, want);
        assert_eq!(
            (contact.first_outside, contact.second_outside),
            (false, outside)
        );
    }
    // 円弧どうしの内接: 小さい円弧（右半分だけ）は、左側の接点が掃引範囲の外。
    let small_arc = arc(&mut doc, (-6.0, 2.0), 3.0, -FRAC_PI_2, FRAC_PI_2);
    let c = Constraint::tangent_circles(&doc, a, small_arc).unwrap();
    assert_eq!(c.is_satisfied(&doc), Ok(true));
    let contact = c.tangent_contact(&doc).unwrap();
    assert_point(contact.point, (-9.0, 2.0));
    assert_eq!(
        (contact.first_outside, contact.second_outside),
        (false, true)
    );
}

#[test]
fn line_tangent_contact_and_segment_range() {
    let mut doc = Document::new();
    let a = arc(&mut doc, (1.0, 2.0), 10.0, WRAP_START, WRAP_END);
    // 円弧の円の左端（角度 π、範囲内）で接する垂直な線分。上向きなので中心は右側。
    let through = line(&mut doc, (-9.0, -5.0), (-9.0, 5.0));
    let c = Constraint::tangent_line_circle(&doc, through, a).unwrap();
    assert_eq!(
        c,
        Constraint::TangentLineCircle {
            line: through,
            circle: a,
            side: Side::Right
        }
    );
    assert_eq!(c.is_satisfied(&doc), Ok(true));
    let contact = c.tangent_contact(&doc).unwrap();
    assert_point(contact.point, (-9.0, 2.0));
    assert_eq!(
        (contact.first_outside, contact.second_outside),
        (false, false)
    );

    // 同じ直線上だが、接点が線分の区間の外にある線分。
    let beyond = line(&mut doc, (-9.0, 4.0), (-9.0, 9.0));
    let c = Constraint::tangent_line_circle(&doc, beyond, a).unwrap();
    assert_eq!(c.is_satisfied(&doc), Ok(true));
    let contact = c.tangent_contact(&doc).unwrap();
    assert_point(contact.point, (-9.0, 2.0));
    assert_eq!(
        (contact.first_outside, contact.second_outside),
        (true, false)
    );

    // 円の上端（角度 π/2、掃引範囲の外）で接する水平な線分。接点は線分の区間の外でもある。
    let top = line(&mut doc, (6.0, 12.0), (20.0, 12.0));
    let c = Constraint::tangent_line_circle(&doc, top, a).unwrap();
    assert_eq!(
        c,
        Constraint::TangentLineCircle {
            line: top,
            circle: a,
            side: Side::Right
        }
    );
    assert_eq!(c.is_satisfied(&doc), Ok(true));
    let contact = c.tangent_contact(&doc).unwrap();
    assert_point(contact.point, (1.0, 12.0));
    assert_eq!(
        (contact.first_outside, contact.second_outside),
        (true, true)
    );
    assert!(contact.is_extended());

    // 端点ちょうどで接する線分は区間内。
    let at_end = line(&mut doc, (-9.0, 2.0), (-9.0, 9.0));
    let contact = Constraint::tangent_line_circle(&doc, at_end, a)
        .unwrap()
        .tangent_contact(&doc)
        .unwrap();
    assert!(!contact.first_outside);

    // 円は常に範囲内。接線でない拘束・退化した線分は None。
    let full = circle(&mut doc, (1.0, 2.0), 10.0);
    let contact = Constraint::tangent_line_circle(&doc, top, full)
        .unwrap()
        .tangent_contact(&doc)
        .unwrap();
    assert_eq!(
        (contact.first_outside, contact.second_outside),
        (true, false)
    );
    assert_eq!(
        Constraint::Horizontal { line: top }.tangent_contact(&doc),
        None
    );
    let dot = line(&mut doc, (0.0, 0.0), (0.0, 0.0));
    let degenerate = Constraint::TangentLineCircle {
        line: dot,
        circle: full,
        side: Side::Left,
    };
    assert_eq!(degenerate.tangent_contact(&doc), None);
}

// ---------------------------------------------------------------------------
// ReshapeEntity の削除規則（純関数。完了条件 7 の規則部分）
// ---------------------------------------------------------------------------

/// テスト用の ID の発行元（文書を作らずに規則だけを試す）。
struct Ids {
    entities: SlotMap<EntityId, ()>,
    constraints: SlotMap<ConstraintId, ()>,
}

impl Ids {
    fn new() -> Self {
        Self {
            entities: SlotMap::with_key(),
            constraints: SlotMap::with_key(),
        }
    }

    fn entity(&mut self) -> EntityId {
        self.entities.insert(())
    }

    fn keyed(&mut self, constraints: Vec<Constraint>) -> Vec<(ConstraintId, Constraint)> {
        constraints
            .into_iter()
            .map(|c| (self.constraints.insert(()), c))
            .collect()
    }
}

fn removals(
    entity: EntityId,
    before: &Shape,
    after: &Shape,
    constraints: &[(ConstraintId, Constraint)],
) -> Result<Vec<ConstraintId>, ReshapeBlocked> {
    reshape_removals(
        entity,
        &EntityGeom::Shape(before.clone()),
        &EntityGeom::Shape(after.clone()),
        constraints.iter().map(|(id, c)| (*id, c)),
    )
}

#[test]
fn reshape_removes_constraints_on_moved_points_and_keeps_direction_ones() {
    let mut ids = Ids::new();
    let (l, other, round) = (ids.entity(), ids.entity(), ids.entity());
    let list = ids.keyed(vec![
        /* 0 */
        Constraint::Coincident {
            a: end(l),
            b: start(other),
        },
        /* 1 */
        Constraint::Coincident {
            a: start(other),
            b: start(l),
        },
        /* 2 */
        Constraint::Distance {
            a: end(l),
            b: end(other),
            value: 3.0,
        },
        /* 3 */
        Constraint::Distance {
            a: start(l),
            b: end(l),
            value: 10.0,
        },
        /* 4 */ Constraint::EqualLength { a: other, b: l },
        /* 5 */ Constraint::Horizontal { line: l },
        /* 6 */ Constraint::Vertical { line: l },
        /* 7 */ Constraint::Parallel { a: l, b: other },
        /* 8 */ Constraint::Perpendicular { a: other, b: l },
        /* 9 */
        Constraint::Angle {
            a: l,
            b: other,
            value: 0.0,
        },
        /* 10 */
        Constraint::TangentLineCircle {
            line: l,
            circle: round,
            side: Side::Left,
        },
        /* 11 */
        Constraint::PointLineDistance {
            point: center(round),
            line: l,
            side: Side::Left,
            value: 2.0,
        },
        /* 12 */
        Constraint::PointLineDistance {
            point: end(l),
            line: other,
            side: Side::Left,
            value: 2.0,
        },
        /* 13 */
        Constraint::PointLineDistance {
            point: start(l),
            line: other,
            side: Side::Left,
            value: 2.0,
        },
        /* 14 */
        Constraint::FixPoint {
            point: start(l),
            at: Point2::ORIGIN,
        },
        // 対象のエンティティを参照しない拘束は無視する。
        /* 15 */
        Constraint::Radius {
            entity: round,
            value: 2.0,
        },
        /* 16 */
        Constraint::Coincident {
            a: end(other),
            b: center(round),
        },
    ]);
    let before = line_shape((0.0, 0.0), (10.0, 0.0));
    // 終点だけを切り詰める: 終点を指す一致・距離と等長が消え、向き・直線に依存する拘束と、
    // 動いていない始点の拘束は残る。
    let after = line_shape((0.0, 0.0), (7.0, 0.0));
    let removed = removals(l, &before, &after, &list).unwrap();
    let want: Vec<ConstraintId> = [0, 2, 3, 4, 12].iter().map(|&i| list[i].0).collect();
    assert_eq!(removed, want);
    // 延長でも同じ。
    let after = line_shape((0.0, 0.0), (15.0, 0.0));
    assert_eq!(removals(l, &before, &after, &list).unwrap(), want);
    // 幾何が変わらなければ何も消さない（等長も残る）。
    assert_eq!(removals(l, &before, &before, &list), Ok(Vec::new()));
    // 許容値以下の動きは「動いていない」扱い。
    let after = line_shape((0.0, 0.0), (10.0, 1e-13));
    assert_eq!(removals(l, &before, &after, &list), Ok(Vec::new()));
}

#[test]
fn reshape_is_blocked_by_fixed_entity_and_moved_fixed_point() {
    let mut ids = Ids::new();
    let (l, other) = (ids.entity(), ids.entity());
    let before = line_shape((0.0, 0.0), (10.0, 0.0));
    let shorter_end = line_shape((0.0, 0.0), (7.0, 0.0));
    let shorter_start = line_shape((2.0, 0.0), (10.0, 0.0));

    // 動いた点の固定は拒否、動いていない点の固定は残る。
    let list = ids.keyed(vec![
        Constraint::Coincident {
            a: end(l),
            b: start(other),
        },
        Constraint::FixPoint {
            point: end(l),
            at: Point2::new(10.0, 0.0),
        },
    ]);
    assert_eq!(
        removals(l, &before, &shorter_end, &list),
        Err(ReshapeBlocked::Fixed(list[1].0))
    );
    assert_eq!(removals(l, &before, &shorter_start, &list), Ok(Vec::new()));

    // 図形の固定は、どこを切り詰めても拒否。別の図形の固定は関係しない。
    let list = ids.keyed(vec![
        Constraint::FixEntity { entity: other },
        Constraint::FixEntity { entity: l },
    ]);
    assert_eq!(
        removals(l, &before, &shorter_end, &list),
        Err(ReshapeBlocked::Fixed(list[1].0))
    );
    assert_eq!(
        removals(l, &before, &shorter_start, &list),
        Err(ReshapeBlocked::Fixed(list[1].0))
    );
    // 幾何が変わらなければ固定があっても拒否しない。
    assert_eq!(removals(l, &before, &before, &list), Ok(Vec::new()));
}

#[test]
fn reshape_circle_to_arc_keeps_center_and_radius_constraints() {
    let mut ids = Ids::new();
    let (c, l, other) = (ids.entity(), ids.entity(), ids.entity());
    let list = ids.keyed(vec![
        Constraint::Coincident {
            a: center(c),
            b: start(l),
        },
        Constraint::FixPoint {
            point: center(c),
            at: Point2::new(1.0, 2.0),
        },
        Constraint::Radius {
            entity: c,
            value: 5.0,
        },
        Constraint::EqualRadius { a: c, b: other },
        Constraint::TangentCircles {
            a: other,
            b: c,
            kind: CircleTangency::External,
        },
        Constraint::TangentLineCircle {
            line: l,
            circle: c,
            side: Side::Right,
        },
    ]);
    let before = circle_shape((1.0, 2.0), 5.0);
    let after = arc_shape((1.0, 2.0), 5.0, 0.3, 2.0);
    assert_eq!(removals(c, &before, &after, &list), Ok(Vec::new()));
}

#[test]
fn reshape_arc_removes_only_moved_endpoint_constraints() {
    let mut ids = Ids::new();
    let (a, l) = (ids.entity(), ids.entity());
    let list = ids.keyed(vec![
        /* 0 */
        Constraint::Coincident {
            a: start(a),
            b: start(l),
        },
        /* 1 */
        Constraint::Coincident {
            a: end(a),
            b: end(l),
        },
        /* 2 */
        Constraint::Coincident {
            a: center(a),
            b: start(l),
        },
        /* 3 */ Constraint::Radius {
            entity: a,
            value: 5.0,
        },
        /* 4 */
        Constraint::Distance {
            a: start(a),
            b: end(a),
            value: 4.0,
        },
    ]);
    let before = arc_shape((1.0, 2.0), 5.0, 0.3, 2.0);
    // 終了角だけを変える: 終点の拘束（と、終点を含む弦の距離）が消える。
    let after = arc_shape((1.0, 2.0), 5.0, 0.3, 1.5);
    assert_eq!(
        removals(a, &before, &after, &list),
        Ok(vec![list[1].0, list[4].0])
    );
    // 開始角・終了角の両方を変えても、中心と半径が同じなら切り詰め・延長。
    let after = arc_shape((1.0, 2.0), 5.0, -0.5, 2.5);
    assert_eq!(
        removals(a, &before, &after, &list),
        Ok(vec![list[0].0, list[1].0, list[4].0])
    );
    // 動いた端点に点の固定があれば拒否、動いていない端点・中心の固定は残る。
    let fixed = ids.keyed(vec![
        Constraint::FixPoint {
            point: center(a),
            at: Point2::new(1.0, 2.0),
        },
        Constraint::FixPoint {
            point: end(a),
            at: Point2::ORIGIN,
        },
    ]);
    let shorter_end = arc_shape((1.0, 2.0), 5.0, 0.3, 1.5);
    let shorter_start = arc_shape((1.0, 2.0), 5.0, 0.8, 2.0);
    assert_eq!(
        removals(a, &before, &shorter_end, &fixed),
        Err(ReshapeBlocked::Fixed(fixed[1].0))
    );
    assert_eq!(removals(a, &before, &shorter_start, &fixed), Ok(Vec::new()));
}

fn support_changed<T>(r: &Result<T, ReshapeBlocked>) -> bool {
    matches!(r, Err(ReshapeBlocked::SupportChanged(_)))
}

#[test]
fn reshape_of_constrained_line_must_stay_on_its_line() {
    // 拘束のついた線分は、元の線分を延長した直線の上での切り詰め・延長だけを受け付ける。
    // 移動を ReshapeEntity で行うと、端点の拘束だけが消えて向きの拘束が残ってしまう。
    let mut ids = Ids::new();
    let (l, other) = (ids.entity(), ids.entity());
    let list = ids.keyed(vec![
        Constraint::Coincident {
            a: end(l),
            b: start(other),
        },
        Constraint::Horizontal { line: l },
        Constraint::Parallel { a: l, b: other },
    ]);
    let before = line_shape((0.0, 0.0), (10.0, 0.0));
    for (what, after) in [
        ("translate across", line_shape((0.0, 5.0), (10.0, 5.0))),
        ("translate slightly", line_shape((0.0, 1e-6), (10.0, 1e-6))),
        ("rotate about start", line_shape((0.0, 0.0), (8.0, 6.0))),
        ("rotate about middle", line_shape((5.0, -5.0), (5.0, 5.0))),
        (
            "move one end off the line",
            line_shape((0.0, 0.0), (7.0, 0.5)),
        ),
        ("reverse", line_shape((10.0, 0.0), (0.0, 0.0))),
        ("reverse and trim", line_shape((7.0, 0.0), (2.0, 0.0))),
        ("collapse to a point", line_shape((3.0, 0.0), (3.0, 0.0))),
        ("to a circle", circle_shape((5.0, 0.0), 5.0)),
        ("to an arc", arc_shape((5.0, 0.0), 5.0, 0.0, PI)),
        (
            "to a polyline",
            Shape::Polyline(Polyline::new(
                vec![Point2::new(0.0, 0.0), Point2::new(7.0, 0.0)],
                false,
            )),
        ),
    ] {
        let got = removals(l, &before, &after, &list);
        assert!(support_changed(&got), "{what}: {got:?}");
    }
    // 同じ直線上なら、片側・両側の切り詰め、延長、ずらした区間のどれも通る。
    for (after, removed) in [
        (line_shape((0.0, 0.0), (7.0, 0.0)), vec![list[0].0]),
        (line_shape((2.0, 0.0), (10.0, 0.0)), vec![]),
        (line_shape((-5.0, 0.0), (25.0, 0.0)), vec![list[0].0]),
        (line_shape((20.0, 0.0), (30.0, 0.0)), vec![list[0].0]),
        // 許容値（RTOL × 長さ 10 = 1e-8）以内のずれは直線上とみなす。
        (line_shape((0.0, 0.0), (7.0, 1e-9)), vec![list[0].0]),
    ] {
        assert_eq!(
            removals(l, &before, &after, &list),
            Ok(removed),
            "{after:?}"
        );
    }
    // 斜めの線分でも同じ。
    let slanted = line_shape((1.0, 1.0), (4.0, 5.0));
    assert_eq!(
        removals(l, &slanted, &line_shape((1.0, 1.0), (2.5, 3.0)), &list),
        Ok(vec![list[0].0])
    );
    assert!(support_changed(&removals(
        l,
        &slanted,
        &line_shape((1.0, 1.0), (4.0, 4.0)),
        &list
    )));
    // 元の線分が退化していて直線が定まらなければ拒否。
    let dot = line_shape((1.0, 1.0), (1.0, 1.0));
    assert!(support_changed(&removals(
        l,
        &dot,
        &line_shape((1.0, 1.0), (5.0, 1.0)),
        &list
    )));
    // 幾何が変わらなければ、何も確かめず何も消さない。
    assert_eq!(removals(l, &dot, &dot, &list), Ok(Vec::new()));
}

#[test]
fn reshape_of_constrained_round_must_keep_center_and_radius() {
    let mut ids = Ids::new();
    let (c, l) = (ids.entity(), ids.entity());
    let list = ids.keyed(vec![
        Constraint::Coincident {
            a: center(c),
            b: start(l),
        },
        Constraint::Radius {
            entity: c,
            value: 5.0,
        },
    ]);
    let circle = circle_shape((1.0, 2.0), 5.0);
    let arc = arc_shape((1.0, 2.0), 5.0, 0.3, 2.0);
    for (what, before, after) in [
        (
            "arc: move center",
            &arc,
            arc_shape((1.5, 2.0), 5.0, 0.3, 2.0),
        ),
        (
            "arc: change radius",
            &arc,
            arc_shape((1.0, 2.0), 4.0, 0.3, 2.0),
        ),
        (
            "arc: rotate about a point",
            &arc,
            arc_shape((2.0, 1.0), 5.0, 1.3, 3.0),
        ),
        ("arc to circle", &arc, circle_shape((1.0, 2.0), 5.0)),
        ("arc to line", &arc, line_shape((0.0, 0.0), (1.0, 0.0))),
        ("circle: move", &circle, circle_shape((1.0, 3.0), 5.0)),
        (
            "circle: change radius",
            &circle,
            circle_shape((1.0, 2.0), 6.0),
        ),
        (
            "circle to arc elsewhere",
            &circle,
            arc_shape((0.0, 2.0), 5.0, 0.3, 2.0),
        ),
        (
            "circle to arc, other radius",
            &circle,
            arc_shape((1.0, 2.0), 5.5, 0.3, 2.0),
        ),
    ] {
        let got = removals(c, before, &after, &list);
        assert!(support_changed(&got), "{what}: {got:?}");
    }
    // 円 → 円弧、円弧 → 円弧で中心と半径が同じなら通る（許容値以内の差も）。
    assert_eq!(removals(c, &circle, &arc, &list), Ok(Vec::new()));
    assert_eq!(
        removals(c, &arc, &arc_shape((1.0, 2.0), 5.0, 1.0, 1.5), &list),
        Ok(Vec::new())
    );
    assert_eq!(
        removals(
            c,
            &circle,
            &arc_shape((1.0, 2.0 + 1e-9), 5.0 - 1e-9, 0.3, 2.0),
            &list
        ),
        Ok(Vec::new())
    );
}

#[test]
fn reshape_of_unconstrained_entity_is_not_checked() {
    // 対象を参照する拘束が無ければ、どんな変更でも何も確かめず何も消さない
    // （ModifyEntity と同じ）。ほかの図形の拘束は関係しない。
    let mut ids = Ids::new();
    let (l, other, round) = (ids.entity(), ids.entity(), ids.entity());
    let unrelated = ids.keyed(vec![
        Constraint::Horizontal { line: other },
        Constraint::FixEntity { entity: other },
        Constraint::Coincident {
            a: end(other),
            b: center(round),
        },
    ]);
    let before = line_shape((0.0, 0.0), (10.0, 0.0));
    for after in [
        line_shape((0.0, 5.0), (10.0, 5.0)),
        line_shape((10.0, 0.0), (0.0, 0.0)),
        line_shape((3.0, 3.0), (3.0, 3.0)),
        circle_shape((0.0, 0.0), 1.0),
    ] {
        assert_eq!(removals(l, &before, &after, &unrelated), Ok(Vec::new()));
        assert_eq!(removals(l, &before, &after, &[]), Ok(Vec::new()));
    }
}

#[test]
fn reshape_support_check_comes_before_fixed_check() {
    // 固定された図形の移動は「切り詰めではない」として拒否し、同一直線上の切り詰めは
    // 「固定されている」として拒否する。どちらも拘束は消さない。
    let mut ids = Ids::new();
    let l = ids.entity();
    let list = ids.keyed(vec![Constraint::FixEntity { entity: l }]);
    let before = line_shape((0.0, 0.0), (10.0, 0.0));
    assert!(support_changed(&removals(
        l,
        &before,
        &line_shape((0.0, 1.0), (10.0, 1.0)),
        &list
    )));
    assert_eq!(
        removals(l, &before, &line_shape((0.0, 0.0), (7.0, 0.0)), &list),
        Err(ReshapeBlocked::Fixed(list[0].0))
    );
}

#[test]
fn moved_point_tolerance_scales_with_entity_size() {
    // 長さ 1e5 の線分では 1e-5 のずれは許容値（RTOL × L = 1e-4）以下、長さ 10 では超える。
    let big_before = EntityGeom::Shape(line_shape((0.0, 0.0), (1e5, 0.0)));
    let big_after = EntityGeom::Shape(line_shape((0.0, 0.0), (1e5, 1e-5)));
    assert!(!point_moved(&big_before, &big_after, PointKind::End));
    let small_before = EntityGeom::Shape(line_shape((0.0, 0.0), (10.0, 0.0)));
    let small_after = EntityGeom::Shape(line_shape((0.0, 0.0), (10.0, 1e-5)));
    assert!(point_moved(&small_before, &small_after, PointKind::End));
    assert!(!point_moved(&small_before, &small_after, PointKind::Start));
    // 変更後の図形が持たない点は「動いた」。
    assert!(point_moved(&small_before, &small_after, PointKind::Center));
}
