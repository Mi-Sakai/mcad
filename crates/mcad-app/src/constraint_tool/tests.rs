//! 拘束ツールの単体テスト（M13 タスク99）。模擬クリック列 → 期待する `Constraint` と `hold`
//! で `apply` され、undo 1 回で戻ること。拒否・値の変換・伸びの案内。

use std::f64::consts::{FRAC_PI_2, PI};

use mcad_core::{CircleTangency, Entity, Side, Style};
use mcad_geom::{Arc, Circle, LineSeg};

use super::*;

const TOL: PickTol = PickTol {
    point: 0.5,
    shape: 0.5,
};

fn p(x: f64, y: f64) -> Point2 {
    Point2::new(x, y)
}

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

fn line(doc: &mut Document, ax: f64, ay: f64, bx: f64, by: f64) -> EntityId {
    add(doc, Shape::Line(LineSeg::new(p(ax, ay), p(bx, by))))
}

fn circle(doc: &mut Document, cx: f64, cy: f64, r: f64) -> EntityId {
    add(doc, Shape::Circle(Circle::new(p(cx, cy), r)))
}

fn pt(entity: EntityId, kind: PointKind) -> PointRef {
    PointRef::new(entity, kind)
}

/// `clicks` を順にクリックし、最後のクリックの結果を返す（途中は `Continue` を確かめる）。
fn click_all(
    doc: &Document,
    tool: &mut ConstraintTool,
    clicks: &[Point2],
    value: ValueInput,
) -> ClickOutcome {
    let (last, init) = clicks.split_last().unwrap();
    for &c in init {
        assert_eq!(
            tool.click(doc, c, TOL, value),
            ClickOutcome::Continue,
            "click {c:?}"
        );
    }
    tool.click(doc, *last, TOL, value)
}

/// `kind` で `clicks` を選び、確定したコマンドを返す。
fn commit(doc: &Document, kind: ConstraintKind, clicks: &[Point2], value: ValueInput) -> Command {
    let mut tool = ConstraintTool::default();
    tool.set_kind(kind);
    match click_all(doc, &mut tool, clicks, value) {
        ClickOutcome::Commit(cmd) => cmd,
        other => panic!("{kind:?}: expected commit, got {other:?}"),
    }
}

fn added(cmd: &Command) -> (Constraint, Option<EntityId>) {
    match cmd {
        Command::AddConstraint { constraint, hold } => (constraint.clone(), *hold),
        other => panic!("expected AddConstraint, got {other:?}"),
    }
}

fn geoms(doc: &Document) -> Vec<(EntityId, EntityGeom)> {
    doc.entities().map(|(id, e)| (id, e.geom.clone())).collect()
}

/// `apply_constraint_command` で 1 回適用し（拘束が 1 件増え、満たされている）、undo 1 回で
/// 図形と拘束が元に戻ることを確かめる。成功時のステータス文言を返す。
fn apply_then_undo(doc: &mut Document, kind: ConstraintKind, cmd: Command) -> String {
    let before = geoms(doc);
    let count = doc.constraint_count();
    let msg = apply_constraint_command(doc, kind, cmd).expect("constraint is accepted");
    assert_eq!(doc.constraint_count(), count + 1);
    for (id, c) in doc.constraints() {
        assert!(c.is_satisfied(doc).unwrap(), "{id:?} {c:?}");
    }
    assert!(doc.undo(), "one undo step");
    assert_eq!(doc.constraint_count(), count);
    assert_eq!(geoms(doc), before, "undo restores the shapes");
    msg
}

// ---------------------------------------------------------------------------
// 種類ごとの組み立てと適用
// ---------------------------------------------------------------------------

#[test]
fn coincident_picks_two_points_and_holds_the_first_entity() {
    let mut doc = Document::new();
    let a = line(&mut doc, 0.0, 0.0, 10.0, 0.0);
    let b = line(&mut doc, 10.0, 0.2, 10.0, 5.0);
    let cmd = commit(
        &doc,
        ConstraintKind::Coincident,
        &[p(10.0, 0.0), p(10.0, 0.2)],
        ValueInput::Empty,
    );
    assert_eq!(
        added(&cmd),
        (
            Constraint::Coincident {
                a: pt(a, PointKind::End),
                b: pt(b, PointKind::Start),
            },
            Some(a)
        )
    );
    let msg = apply_then_undo(&mut doc, ConstraintKind::Coincident, cmd);
    assert_eq!(msg, "Added Coincident constraint");
}

#[test]
fn coincident_second_pick_skips_the_already_picked_point() {
    // 2 本の端点が同じ位置にあるとき、2 回目の同じ位置のクリックはもう一方の端点を拾う。
    let mut doc = Document::new();
    let a = line(&mut doc, 0.0, 0.0, 10.0, 0.0);
    let b = line(&mut doc, 10.0, 0.0, 10.0, 5.0);
    let cmd = commit(
        &doc,
        ConstraintKind::Coincident,
        &[p(10.0, 0.0), p(10.0, 0.0)],
        ValueInput::Empty,
    );
    assert_eq!(
        added(&cmd).0,
        Constraint::Coincident {
            a: pt(a, PointKind::End),
            b: pt(b, PointKind::Start),
        }
    );
}

#[test]
fn horizontal_and_vertical_take_one_line_without_hold() {
    let mut doc = Document::new();
    let h = line(&mut doc, 0.0, 0.0, 10.0, 0.5);
    let v = line(&mut doc, 20.0, 0.0, 20.5, 10.0);
    let cmd = commit(
        &doc,
        ConstraintKind::Horizontal,
        &[p(5.0, 0.25)],
        ValueInput::Empty,
    );
    assert_eq!(added(&cmd), (Constraint::Horizontal { line: h }, None));
    apply_then_undo(&mut doc, ConstraintKind::Horizontal, cmd);
    let cmd = commit(
        &doc,
        ConstraintKind::Vertical,
        &[p(20.25, 5.0)],
        ValueInput::Empty,
    );
    assert_eq!(added(&cmd), (Constraint::Vertical { line: v }, None));
    apply_then_undo(&mut doc, ConstraintKind::Vertical, cmd);
}

#[test]
fn distance_uses_the_current_value_or_the_typed_mm() {
    let mut doc = Document::new();
    let a = line(&mut doc, 0.0, 0.0, 3.0, 0.0);
    let b = line(&mut doc, 6.0, 4.0, 9.0, 4.0);
    let clicks = [p(0.0, 0.0), p(6.0, 4.0)];
    let cmd = commit(&doc, ConstraintKind::Distance, &clicks, ValueInput::Empty);
    let (c, hold) = added(&cmd);
    assert_eq!(hold, Some(a));
    let Constraint::Distance {
        a: pa,
        b: pb,
        value,
    } = c
    else {
        panic!("{c:?}");
    };
    assert_eq!((pa, pb), (pt(a, PointKind::Start), pt(b, PointKind::Start)));
    assert!((value - 52.0_f64.sqrt()).abs() < 1e-12, "{value}");
    apply_then_undo(&mut doc, ConstraintKind::Distance, cmd);

    let cmd = commit(
        &doc,
        ConstraintKind::Distance,
        &clicks,
        parse_value_input(" 12.5 "),
    );
    assert!(matches!(added(&cmd).0, Constraint::Distance { value, .. } if value == 12.5));
    apply_then_undo(&mut doc, ConstraintKind::Distance, cmd);
}

#[test]
fn length_of_one_line_is_its_own_two_points_and_holds_nothing() {
    // 同じ線分の始点と終点は選べる（線分の長さ）。止めると唯一の図形が動けないので hold なし。
    let mut doc = Document::new();
    let a = line(&mut doc, 0.0, 0.0, 10.0, 0.0);
    let cmd = commit(
        &doc,
        ConstraintKind::Distance,
        &[p(0.0, 0.0), p(10.0, 0.0)],
        ValueInput::Value(25.0),
    );
    assert_eq!(
        added(&cmd),
        (
            Constraint::Distance {
                a: pt(a, PointKind::Start),
                b: pt(a, PointKind::End),
                value: 25.0,
            },
            None
        )
    );
    apply_then_undo(&mut doc, ConstraintKind::Distance, cmd);
}

#[test]
fn typed_distance_works_even_when_the_points_coincide_now() {
    // コンストラクタは一致した 2 点を拒否するが、正の値を打てば作れる。
    let mut doc = Document::new();
    let a = line(&mut doc, 0.0, 0.0, 10.0, 0.0);
    let b = line(&mut doc, 10.0, 0.0, 10.0, 5.0);
    let cmd = commit(
        &doc,
        ConstraintKind::Distance,
        &[p(10.0, 0.0), p(10.0, 0.0)],
        ValueInput::Value(2.0),
    );
    assert_eq!(
        added(&cmd).0,
        Constraint::Distance {
            a: pt(a, PointKind::End),
            b: pt(b, PointKind::Start),
            value: 2.0,
        }
    );
    apply_then_undo(&mut doc, ConstraintKind::Distance, cmd);
}

#[test]
fn point_line_distance_accepts_either_order_and_holds_the_first() {
    let mut doc = Document::new();
    let base = line(&mut doc, 0.0, 0.0, 10.0, 0.0);
    let other = line(&mut doc, 3.0, 5.0, 3.0, 8.0);
    let expected = Constraint::PointLineDistance {
        point: pt(other, PointKind::Start),
        line: base,
        side: Side::Left,
        value: 5.0,
    };
    // 点 → 線。
    let cmd = commit(
        &doc,
        ConstraintKind::PointLineDistance,
        &[p(3.0, 5.0), p(6.0, 0.0)],
        ValueInput::Empty,
    );
    assert_eq!(added(&cmd), (expected.clone(), Some(other)));
    apply_then_undo(&mut doc, ConstraintKind::PointLineDistance, cmd);
    // 線 → 点（線分の端から離れた所をクリックすれば線分になる）。
    let cmd = commit(
        &doc,
        ConstraintKind::PointLineDistance,
        &[p(6.0, 0.0), p(3.0, 5.0)],
        ValueInput::Value(7.0),
    );
    let Constraint::PointLineDistance { value, side, .. } = added(&cmd).0 else {
        panic!();
    };
    assert_eq!((value, side), (7.0, Side::Left));
    assert_eq!(added(&cmd).1, Some(base));
    apply_then_undo(&mut doc, ConstraintKind::PointLineDistance, cmd);
}

#[test]
fn two_line_kinds_hold_the_first_line() {
    let mut doc = Document::new();
    let a = line(&mut doc, 0.0, 0.0, 10.0, 0.0);
    let b = line(&mut doc, 0.0, 5.0, 10.0, 6.0);
    let clicks = [p(5.0, 0.0), p(5.0, 5.5)];
    for (kind, expected) in [
        (ConstraintKind::Parallel, Constraint::Parallel { a, b }),
        (
            ConstraintKind::Perpendicular,
            Constraint::Perpendicular { a, b },
        ),
        (
            ConstraintKind::EqualLength,
            Constraint::EqualLength { a, b },
        ),
    ] {
        let cmd = commit(&doc, kind, &clicks, ValueInput::Empty);
        assert_eq!(added(&cmd), (expected, Some(a)), "{kind:?}");
        apply_then_undo(&mut doc, kind, cmd);
    }
}

#[test]
fn angle_converts_degrees_to_radians_in_the_half_open_range() {
    let mut doc = Document::new();
    let a = line(&mut doc, 0.0, 0.0, 10.0, 0.0);
    let b = line(&mut doc, 0.0, 5.0, 10.0, 15.0);
    let clicks = [p(5.0, 0.0), p(5.0, 10.0)];
    // 空欄は現在のなす角（45°）。
    let cmd = commit(&doc, ConstraintKind::Angle, &clicks, ValueInput::Empty);
    assert_eq!(
        added(&cmd),
        (
            Constraint::Angle {
                a,
                b,
                value: PI / 4.0
            },
            Some(a)
        )
    );
    // 打った値の符号は現在の向き（+45°）へそろえる（`align_angle_sign`）。
    for (deg, rad) in [
        (90.0, FRAC_PI_2),
        (270.0, FRAC_PI_2),
        (180.0, PI),
        (-180.0, PI),
        (-30.0, PI / 6.0),
    ] {
        let cmd = commit(&doc, ConstraintKind::Angle, &clicks, ValueInput::Value(deg));
        let Constraint::Angle { value, .. } = added(&cmd).0 else {
            panic!();
        };
        assert!((value - rad).abs() < 1e-12, "{deg}° -> {value}");
    }
    let cmd = commit(
        &doc,
        ConstraintKind::Angle,
        &clicks,
        ValueInput::Value(60.0),
    );
    apply_then_undo(&mut doc, ConstraintKind::Angle, cmd);
}

/// 選ぶ順を逆にすると現在の角は −45° になり、打った `90` は −90° として扱う
/// （2026-10-18 タスク100 スモークで、順序によって線分が大きく伸びた件の対処）。
#[test]
fn typed_angle_follows_the_current_direction_in_either_pick_order() {
    let mut doc = Document::new();
    line(&mut doc, 0.0, 0.0, 10.0, 0.0);
    line(&mut doc, 0.0, 5.0, 10.0, 15.0);
    let reversed = [p(5.0, 10.0), p(5.0, 0.0)];
    let cmd = commit(&doc, ConstraintKind::Angle, &reversed, ValueInput::Empty);
    let Constraint::Angle { value, .. } = added(&cmd).0 else {
        panic!();
    };
    assert!((value + PI / 4.0).abs() < 1e-12, "{value}");
    let cmd = commit(
        &doc,
        ConstraintKind::Angle,
        &reversed,
        ValueInput::Value(90.0),
    );
    let Constraint::Angle { value, .. } = added(&cmd).0 else {
        panic!();
    };
    assert!((value + FRAC_PI_2).abs() < 1e-12, "{value}");
}

#[test]
fn align_angle_sign_keeps_magnitude_and_takes_the_current_sign() {
    assert_eq!(align_angle_sign(FRAC_PI_2, -0.5), -FRAC_PI_2);
    assert_eq!(align_angle_sign(-FRAC_PI_2, 0.5), FRAC_PI_2);
    assert_eq!(align_angle_sign(FRAC_PI_2, 0.5), FRAC_PI_2);
    // 現在が 0・π（向きが無い）、打った値が π なら打った値のまま。
    assert_eq!(align_angle_sign(-FRAC_PI_2, 0.0), -FRAC_PI_2);
    assert_eq!(align_angle_sign(-FRAC_PI_2, PI), -FRAC_PI_2);
    assert_eq!(align_angle_sign(PI, -0.5), PI);
}

#[test]
fn round_kinds_take_circles_and_arcs() {
    let mut doc = Document::new();
    let c1 = circle(&mut doc, 0.0, 0.0, 5.0);
    let arc = add(&mut doc, Shape::Arc(Arc::new(p(20.0, 0.0), 3.0, 0.0, PI)));
    let cmd = commit(
        &doc,
        ConstraintKind::EqualRadius,
        &[p(5.0, 0.0), p(20.0, 3.0)],
        ValueInput::Empty,
    );
    assert_eq!(
        added(&cmd),
        (Constraint::EqualRadius { a: c1, b: arc }, Some(c1))
    );
    apply_then_undo(&mut doc, ConstraintKind::EqualRadius, cmd);

    let cmd = commit(
        &doc,
        ConstraintKind::Radius,
        &[p(-5.0, 0.0)],
        ValueInput::Empty,
    );
    assert_eq!(
        added(&cmd),
        (
            Constraint::Radius {
                entity: c1,
                value: 5.0
            },
            None
        )
    );
    let cmd = commit(
        &doc,
        ConstraintKind::Radius,
        &[p(20.0, 3.0)],
        ValueInput::Value(7.0),
    );
    assert_eq!(
        added(&cmd),
        (
            Constraint::Radius {
                entity: arc,
                value: 7.0
            },
            None
        )
    );
    apply_then_undo(&mut doc, ConstraintKind::Radius, cmd);
}

#[test]
fn tangent_dispatches_on_the_target_types() {
    let mut doc = Document::new();
    let l = line(&mut doc, -10.0, 0.0, 10.0, 0.0);
    let c1 = circle(&mut doc, 0.0, 5.0, 5.0);
    let c2 = circle(&mut doc, 20.0, 5.0, 4.0);
    // 線 → 円: 線と円の接線、hold は線。
    let cmd = commit(
        &doc,
        ConstraintKind::Tangent,
        &[p(-8.0, 0.0), p(0.0, 10.0)],
        ValueInput::Empty,
    );
    assert_eq!(
        added(&cmd),
        (
            Constraint::TangentLineCircle {
                line: l,
                circle: c1,
                side: Side::Left
            },
            Some(l)
        )
    );
    apply_then_undo(&mut doc, ConstraintKind::Tangent, cmd);
    // 円 → 線: 同じ拘束で、hold は円。
    let cmd = commit(
        &doc,
        ConstraintKind::Tangent,
        &[p(0.0, 10.0), p(-8.0, 0.0)],
        ValueInput::Empty,
    );
    assert_eq!(added(&cmd).1, Some(c1));
    assert!(matches!(
        added(&cmd).0,
        Constraint::TangentLineCircle { .. }
    ));
    // 円 → 円: 円どうしの接線（近い方 = 外接）。
    let cmd = commit(
        &doc,
        ConstraintKind::Tangent,
        &[p(0.0, 10.0), p(24.0, 5.0)],
        ValueInput::Empty,
    );
    assert_eq!(
        added(&cmd),
        (
            Constraint::TangentCircles {
                a: c1,
                b: c2,
                kind: CircleTangency::External
            },
            Some(c1)
        )
    );
    apply_then_undo(&mut doc, ConstraintKind::Tangent, cmd);
}

#[test]
fn tangent_after_a_line_does_not_accept_another_line() {
    let mut doc = Document::new();
    line(&mut doc, -10.0, 0.0, 10.0, 0.0);
    line(&mut doc, -10.0, 5.0, 10.0, 5.0);
    let mut tool = ConstraintTool::default();
    tool.set_kind(ConstraintKind::Tangent);
    assert_eq!(
        tool.click(&doc, p(0.0, 0.0), TOL, ValueInput::Empty),
        ClickOutcome::Continue
    );
    assert_eq!(
        tool.click(&doc, p(0.0, 5.0), TOL, ValueInput::Empty),
        ClickOutcome::Rejected("Tangent: click a circle or an arc".to_owned())
    );
    assert_eq!(tool.picks().len(), 1, "the first pick is kept");
}

#[test]
fn fix_point_and_fix_entity() {
    let mut doc = Document::new();
    let a = line(&mut doc, 0.0, 0.0, 10.0, 0.0);
    let cmd = commit(
        &doc,
        ConstraintKind::FixPoint,
        &[p(10.0, 0.1)],
        ValueInput::Empty,
    );
    assert_eq!(
        added(&cmd),
        (
            Constraint::FixPoint {
                point: pt(a, PointKind::End),
                at: p(10.0, 0.0)
            },
            None
        )
    );
    apply_then_undo(&mut doc, ConstraintKind::FixPoint, cmd);
    let cmd = commit(
        &doc,
        ConstraintKind::FixEntity,
        &[p(5.0, 0.0)],
        ValueInput::Empty,
    );
    assert_eq!(added(&cmd), (Constraint::FixEntity { entity: a }, None));
    apply_then_undo(&mut doc, ConstraintKind::FixEntity, cmd);
}

#[test]
fn arc_points_are_start_end_and_center() {
    let mut doc = Document::new();
    let arc = add(
        &mut doc,
        Shape::Arc(Arc::new(p(0.0, 0.0), 5.0, 0.0, FRAC_PI_2)),
    );
    for (click, kind) in [
        (p(5.0, 0.0), PointKind::Start),
        (p(0.0, 5.0), PointKind::End),
        (p(0.0, 0.0), PointKind::Center),
    ] {
        let cmd = commit(&doc, ConstraintKind::FixPoint, &[click], ValueInput::Empty);
        assert!(
            matches!(added(&cmd).0, Constraint::FixPoint { point, .. } if point == pt(arc, kind)),
            "{kind:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 拒否
// ---------------------------------------------------------------------------

#[test]
fn the_same_entity_twice_is_rejected() {
    let mut doc = Document::new();
    line(&mut doc, 0.0, 0.0, 10.0, 0.0);
    let mut tool = ConstraintTool::default();
    tool.set_kind(ConstraintKind::Parallel);
    assert_eq!(
        tool.click(&doc, p(2.0, 0.0), TOL, ValueInput::Empty),
        ClickOutcome::Continue
    );
    assert_eq!(
        tool.click(&doc, p(8.0, 0.0), TOL, ValueInput::Empty),
        ClickOutcome::Rejected("Parallel: already picked; pick a different entity".to_owned())
    );
    // ホバーもしない。
    tool.update_hover(&doc, Some(p(8.0, 0.0)), TOL);
    assert_eq!(tool.hover(), None);

    // 一致・距離は同じ点だけを拒否する。
    let mut tool = ConstraintTool::default();
    tool.set_kind(ConstraintKind::Coincident);
    assert_eq!(
        tool.click(&doc, p(0.0, 0.0), TOL, ValueInput::Empty),
        ClickOutcome::Continue
    );
    assert_eq!(
        tool.click(&doc, p(0.0, 0.0), TOL, ValueInput::Empty),
        ClickOutcome::Rejected("Coincident: already picked; pick a different point".to_owned())
    );
}

#[test]
fn point_line_distance_rejects_the_points_own_line() {
    let mut doc = Document::new();
    line(&mut doc, 0.0, 0.0, 10.0, 0.0);
    let mut tool = ConstraintTool::default();
    tool.set_kind(ConstraintKind::PointLineDistance);
    assert_eq!(
        tool.click(&doc, p(0.0, 0.0), TOL, ValueInput::Empty),
        ClickOutcome::Continue
    );
    assert!(matches!(
        tool.click(&doc, p(5.0, 0.0), TOL, ValueInput::Empty),
        ClickOutcome::Rejected(_)
    ));
}

#[test]
fn wrong_shape_types_are_not_targets() {
    let mut doc = Document::new();
    circle(&mut doc, 0.0, 0.0, 5.0);
    line(&mut doc, 20.0, 0.0, 30.0, 0.0);
    // ポリライン・点は対象外。
    add(
        &mut doc,
        Shape::Polyline(mcad_geom::Polyline::new(
            vec![p(40.0, 0.0), p(50.0, 0.0), p(50.0, 10.0)],
            false,
        )),
    );
    add(&mut doc, Shape::Point(p(60.0, 0.0)));
    for (kind, click, msg) in [
        (
            ConstraintKind::Horizontal,
            p(5.0, 0.0),
            "Horizontal: click a line",
        ),
        (
            ConstraintKind::Radius,
            p(25.0, 0.0),
            "Radius: click a circle or an arc",
        ),
        (
            ConstraintKind::Horizontal,
            p(45.0, 0.0),
            "Horizontal: click a line",
        ),
        (
            ConstraintKind::FixPoint,
            p(40.0, 0.0),
            "Fix point: click an endpoint or a center",
        ),
        (
            ConstraintKind::FixPoint,
            p(60.0, 0.0),
            "Fix point: click an endpoint or a center",
        ),
        (
            ConstraintKind::FixEntity,
            p(45.0, 0.0),
            "Fix entity: click a line, a circle or an arc",
        ),
    ] {
        let mut tool = ConstraintTool::default();
        tool.set_kind(kind);
        assert_eq!(
            tool.click(&doc, click, TOL, ValueInput::Empty),
            ClickOutcome::Rejected(msg.to_owned()),
            "{kind:?} at {click:?}"
        );
        assert!(tool.picks().is_empty());
    }
}

#[test]
fn zero_or_negative_distance_values_are_rejected_with_a_coincident_hint() {
    let mut doc = Document::new();
    line(&mut doc, 0.0, 0.0, 10.0, 0.0);
    let mut tool = ConstraintTool::default();
    tool.set_kind(ConstraintKind::Distance);
    assert_eq!(
        tool.click(&doc, p(0.0, 0.0), TOL, ValueInput::Value(0.0)),
        ClickOutcome::Continue
    );
    for v in [0.0, -3.0] {
        let ClickOutcome::Rejected(msg) = tool.click(&doc, p(10.0, 0.0), TOL, ValueInput::Value(v))
        else {
            panic!("value {v} must be rejected");
        };
        assert!(msg.contains("Coincident"), "{msg}");
        assert_eq!(tool.picks().len(), 1, "the first point is kept");
    }
    // 現在の距離が 0（一致した 2 点）で空欄のときは core が拒否し、その文言も一致を案内する。
    let mut doc = Document::new();
    line(&mut doc, 0.0, 0.0, 10.0, 0.0);
    line(&mut doc, 10.0, 0.0, 10.0, 5.0);
    let mut tool = ConstraintTool::default();
    tool.set_kind(ConstraintKind::Distance);
    let ClickOutcome::Rejected(msg) = click_all(
        &doc,
        &mut tool,
        &[p(10.0, 0.0), p(10.0, 0.0)],
        ValueInput::Empty,
    ) else {
        panic!("coincident points with an empty value must be rejected");
    };
    assert!(msg.starts_with("Constraint failed: "), "{msg}");
    assert!(msg.contains("coincident"), "{msg}");
}

#[test]
fn other_out_of_range_values_are_rejected() {
    let mut doc = Document::new();
    line(&mut doc, 0.0, 0.0, 10.0, 0.0);
    line(&mut doc, 0.0, 5.0, 10.0, 5.0);
    circle(&mut doc, 30.0, 0.0, 5.0);
    let rejected = |kind, clicks: &[Point2], v| {
        let mut tool = ConstraintTool::default();
        tool.set_kind(kind);
        matches!(
            click_all(&doc, &mut tool, clicks, ValueInput::Value(v)),
            ClickOutcome::Rejected(_)
        )
    };
    assert!(rejected(ConstraintKind::Radius, &[p(35.0, 0.0)], 0.0));
    assert!(rejected(ConstraintKind::Radius, &[p(35.0, 0.0)], -1.0));
    assert!(rejected(
        ConstraintKind::PointLineDistance,
        &[p(0.0, 5.0), p(5.0, 0.0)],
        -1.0
    ));
    // 点と線の距離の 0 は受け付ける。
    assert!(!rejected(
        ConstraintKind::PointLineDistance,
        &[p(0.0, 5.0), p(5.0, 0.0)],
        0.0
    ));
}

#[test]
fn invalid_value_text_rejects_the_confirming_click_only() {
    let mut doc = Document::new();
    line(&mut doc, 0.0, 0.0, 10.0, 0.0);
    line(&mut doc, 0.0, 5.0, 10.0, 15.0);
    let invalid = parse_value_input("12mm");
    assert_eq!(invalid, ValueInput::Invalid);
    let mut tool = ConstraintTool::default();
    tool.set_kind(ConstraintKind::Angle);
    // 確定でないクリックは値を見ない。
    assert_eq!(
        tool.click(&doc, p(5.0, 0.0), TOL, invalid),
        ClickOutcome::Continue
    );
    assert_eq!(
        tool.click(&doc, p(5.0, 10.0), TOL, invalid),
        ClickOutcome::Rejected("Angle: the value is not a number (deg)".to_owned())
    );
    assert_eq!(tool.picks().len(), 1);
    // 直して打ち直せば通る。
    assert!(matches!(
        tool.click(&doc, p(5.0, 10.0), TOL, parse_value_input("30")),
        ClickOutcome::Commit(_)
    ));
    // 値を持たない種類は Value 欄を見ない。
    let cmd = commit(
        &doc,
        ConstraintKind::Parallel,
        &[p(5.0, 0.0), p(5.0, 10.0)],
        invalid,
    );
    assert!(matches!(added(&cmd).0, Constraint::Parallel { .. }));
}

#[test]
fn parse_value_input_distinguishes_empty_values_and_garbage() {
    assert_eq!(parse_value_input(""), ValueInput::Empty);
    assert_eq!(parse_value_input("   "), ValueInput::Empty);
    assert_eq!(parse_value_input("2.5"), ValueInput::Value(2.5));
    assert_eq!(parse_value_input("-30"), ValueInput::Value(-30.0));
    assert_eq!(parse_value_input("abc"), ValueInput::Invalid);
    assert_eq!(parse_value_input("inf"), ValueInput::Invalid);
    assert_eq!(parse_value_input("NaN"), ValueInput::Invalid);
}

#[test]
fn a_failed_apply_reports_the_error_and_keeps_the_document() {
    // 水平と垂直を同じ線分に付けるのは矛盾 → 拒否され、文書は変わらない。
    let mut doc = Document::new();
    let a = line(&mut doc, 0.0, 0.0, 10.0, 0.0);
    doc.apply(Command::AddConstraint {
        constraint: Constraint::Horizontal { line: a },
        hold: None,
    })
    .unwrap();
    let before = geoms(&doc);
    let generation = doc.generation();
    let mut tool = ConstraintTool::default();
    tool.set_kind(ConstraintKind::Vertical);
    let ClickOutcome::Commit(cmd) = tool.click(&doc, p(5.0, 0.0), TOL, ValueInput::Empty) else {
        panic!();
    };
    let err = apply_constraint_command(&mut doc, ConstraintKind::Vertical, cmd).unwrap_err();
    assert!(err.starts_with("Constraint failed: "), "{err}");
    assert_eq!(geoms(&doc), before);
    assert_eq!(doc.generation(), generation);
    assert_eq!(doc.constraint_count(), 1);
}

#[test]
fn locked_layers_can_be_picked() {
    let mut doc = Document::new();
    let a = line(&mut doc, 0.0, 0.0, 10.0, 0.0);
    let layer = doc.current_layer();
    let mut props = doc.layer(layer).unwrap().clone();
    props.locked = true;
    doc.apply(Command::SetLayerProps { id: layer, props })
        .unwrap();
    let cmd = commit(
        &doc,
        ConstraintKind::Horizontal,
        &[p(5.0, 0.0)],
        ValueInput::Empty,
    );
    assert_eq!(added(&cmd).0, Constraint::Horizontal { line: a });
    // 既に水平なので解かずに通る。
    apply_then_undo(&mut doc, ConstraintKind::Horizontal, cmd);
}

#[test]
fn hidden_layers_are_not_picked() {
    let mut doc = Document::new();
    line(&mut doc, 0.0, 0.0, 10.0, 0.0);
    let layer = doc.current_layer();
    let mut props = doc.layer(layer).unwrap().clone();
    props.visible = false;
    doc.apply(Command::SetLayerProps { id: layer, props })
        .unwrap();
    let mut tool = ConstraintTool::default();
    tool.set_kind(ConstraintKind::Horizontal);
    assert!(matches!(
        tool.click(&doc, p(5.0, 0.0), TOL, ValueInput::Empty),
        ClickOutcome::Rejected(_)
    ));
}

// ---------------------------------------------------------------------------
// 状態（Esc・種類の変更・案内）
// ---------------------------------------------------------------------------

#[test]
fn escape_clears_the_picks_first_then_leaves_the_tool() {
    let mut doc = Document::new();
    line(&mut doc, 0.0, 0.0, 10.0, 0.0);
    let mut tool = ConstraintTool::default();
    tool.set_kind(ConstraintKind::Parallel);
    tool.click(&doc, p(5.0, 0.0), TOL, ValueInput::Empty);
    assert_eq!(tool.picks().len(), 1);
    assert!(tool.cancel(), "first Esc stays in the tool");
    assert!(tool.picks().is_empty());
    assert!(!tool.cancel(), "second Esc leaves the tool");
}

#[test]
fn changing_the_kind_drops_the_picks_and_success_resets_them() {
    let mut doc = Document::new();
    line(&mut doc, 0.0, 0.0, 10.0, 0.0);
    line(&mut doc, 0.0, 5.0, 10.0, 5.0);
    let mut tool = ConstraintTool::default();
    tool.set_kind(ConstraintKind::Parallel);
    tool.click(&doc, p(5.0, 0.0), TOL, ValueInput::Empty);
    tool.set_kind(ConstraintKind::Parallel);
    assert_eq!(tool.picks().len(), 1, "same kind keeps the picks");
    assert_eq!(tool.prompt(), "Parallel: click a line (2/2)");
    tool.set_kind(ConstraintKind::EqualLength);
    assert!(tool.picks().is_empty());
    assert_eq!(tool.prompt(), "Equal length: click a line (1/2)");

    tool.click(&doc, p(5.0, 0.0), TOL, ValueInput::Empty);
    let ClickOutcome::Commit(cmd) = tool.click(&doc, p(5.0, 5.0), TOL, ValueInput::Empty) else {
        panic!();
    };
    // 確定前は選んだ対象を変えない（失敗したら 2 本目だけ選び直せる）。
    assert_eq!(tool.picks().len(), 1);
    apply_constraint_command(&mut doc, ConstraintKind::EqualLength, cmd).unwrap();
    tool.on_committed();
    assert!(tool.picks().is_empty());
    assert_eq!(tool.kind(), ConstraintKind::EqualLength);
}

#[test]
fn hover_shows_the_next_admissible_target() {
    let mut doc = Document::new();
    let a = line(&mut doc, 0.0, 0.0, 10.0, 0.0);
    let mut tool = ConstraintTool::default();
    tool.set_kind(ConstraintKind::PointLineDistance);
    // 点と線の両方を受け付ける段階では、端点の近くは点、中ほどは線分。
    tool.update_hover(&doc, Some(p(0.2, 0.0)), TOL);
    assert_eq!(tool.hover(), Some(Target::Point(pt(a, PointKind::Start))));
    tool.update_hover(&doc, Some(p(5.0, 0.1)), TOL);
    assert_eq!(tool.hover(), Some(Target::Line(a)));
    tool.update_hover(&doc, Some(p(5.0, 3.0)), TOL);
    assert_eq!(tool.hover(), None);
    tool.update_hover(&doc, None, TOL);
    assert_eq!(tool.hover(), None);
}

// ---------------------------------------------------------------------------
// 伸びの案内（判断3 の既知の制約 (b)）
// ---------------------------------------------------------------------------

#[test]
fn rotating_a_line_into_place_reports_the_stretch() {
    // ちょうど水平な長さ 10 の線分へ垂直を付けると、最小ノルム解で長さが変わる（約 21.43）。
    let mut doc = Document::new();
    line(&mut doc, 0.0, 0.0, 10.0, 0.0);
    let cmd = commit(
        &doc,
        ConstraintKind::Vertical,
        &[p(5.0, 0.0)],
        ValueInput::Empty,
    );
    let msg = apply_constraint_command(&mut doc, ConstraintKind::Vertical, cmd).unwrap();
    assert_eq!(
        msg,
        "Added Vertical constraint; a line changed length (add a Distance constraint to keep it)"
    );
}

#[test]
fn constraints_already_satisfied_or_length_kept_have_no_stretch_note() {
    let mut doc = Document::new();
    let a = line(&mut doc, 0.0, 0.0, 10.0, 0.0);
    line(&mut doc, 0.0, 5.0, 10.0, 5.5);
    // 既に水平: 動かない。
    let cmd = commit(
        &doc,
        ConstraintKind::Horizontal,
        &[p(5.0, 0.0)],
        ValueInput::Empty,
    );
    let msg = apply_constraint_command(&mut doc, ConstraintKind::Horizontal, cmd).unwrap();
    assert_eq!(msg, "Added Horizontal constraint");
    // 長さを距離拘束で止めてから平行を付けると、長さは保たれ案内は出ない。
    let b_start = p(0.0, 5.0);
    let b_end = p(10.0, 5.5);
    let cmd = commit(
        &doc,
        ConstraintKind::Distance,
        &[b_start, b_end],
        ValueInput::Empty,
    );
    apply_constraint_command(&mut doc, ConstraintKind::Distance, cmd).unwrap();
    let cmd = commit(
        &doc,
        ConstraintKind::Parallel,
        &[p(5.0, 0.0), p(5.0, 5.25)],
        ValueInput::Empty,
    );
    assert_eq!(added(&cmd).1, Some(a));
    let msg = apply_constraint_command(&mut doc, ConstraintKind::Parallel, cmd).unwrap();
    assert_eq!(msg, "Added Parallel constraint");
    // 伸びの案内の対象でない種類（半径など）は長さを比べない。
    assert!(!ConstraintKind::Radius.may_stretch());
    assert!(!ConstraintKind::Distance.may_stretch());
}

#[test]
fn every_kind_has_a_label_and_the_right_target_count() {
    assert_eq!(ConstraintKind::ALL.len(), 14);
    for kind in ConstraintKind::ALL {
        assert!(!kind.label().is_empty());
        assert!(kind.label().is_ascii(), "the top panel is an English area");
    }
    let single: Vec<_> = ConstraintKind::ALL
        .into_iter()
        .filter(|k| k.target_count() == 1)
        .collect();
    assert_eq!(
        single,
        vec![
            ConstraintKind::Horizontal,
            ConstraintKind::Vertical,
            ConstraintKind::Radius,
            ConstraintKind::FixPoint,
            ConstraintKind::FixEntity,
        ]
    );
    let valued: Vec<_> = ConstraintKind::ALL
        .into_iter()
        .filter_map(|k| k.value_unit().map(|u| (k, u)))
        .collect();
    assert_eq!(
        valued,
        vec![
            (ConstraintKind::Distance, ValueUnit::Mm),
            (ConstraintKind::PointLineDistance, ValueUnit::Mm),
            (ConstraintKind::Radius, ValueUnit::Mm),
            (ConstraintKind::Angle, ValueUnit::Deg),
        ]
    );
}
