//! 拘束の方程式のテスト（M13 タスク96 の完了条件 1〜5）。
//!
//! - 全方程式の解析ヤコビアンと中心差分の一致（proptest。円弧の端点を指す拘束を含む）
//! - 角度の残差の主値化: ±π をまたぐ回転での連続性、`θ` が 0・±π の近く、反平行
//! - 特異点: 一致した 2 点の距離、同心の円の外接（NaN を出さずに収束）
//! - 点の固定: 円弧の自由度、始点を固定して半径を変える
//! - 接線: 中心が直線上の円（左側へ解ける）
//!
//! 解くテストは `mcad_geom::solve::solve` を直接呼ぶ（文書への反映はタスク97）。

use std::f64::consts::{FRAC_PI_2, PI};

use mcad_geom::solve::{
    Problem, Solution, SolveError, check_jacobian, degrees_of_freedom, is_satisfied, solve,
};
use proptest::prelude::*;
use slotmap::SlotMap;

use super::*;
use crate::constraint::{LargerCircle, Side};
use crate::{Command, Entity, Style};

// ---------------------------------------------------------------------------
// ヘルパー
// ---------------------------------------------------------------------------

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

fn line(doc: &mut Document, a: (f64, f64), b: (f64, f64)) -> EntityId {
    add(
        doc,
        Shape::Line(LineSeg::new(Point2::new(a.0, a.1), Point2::new(b.0, b.1))),
    )
}

fn circle(doc: &mut Document, c: (f64, f64), r: f64) -> EntityId {
    add(doc, Shape::Circle(Circle::new(Point2::new(c.0, c.1), r)))
}

fn arc(doc: &mut Document, c: (f64, f64), r: f64, start: f64, end: f64) -> EntityId {
    add(
        doc,
        Shape::Arc(Arc::new(Point2::new(c.0, c.1), r, start, end)),
    )
}

/// 文書へ入れずに、拘束の列（渡した順に ID を振る）から方程式系を組み立てる。
fn system(doc: &Document, constraints: &[Constraint]) -> System {
    let mut ids: SlotMap<ConstraintId, ()> = SlotMap::with_key();
    let keyed: Vec<(ConstraintId, &Constraint)> =
        constraints.iter().map(|c| (ids.insert(()), c)).collect();
    System::build(|id| doc.entity(id).map(|e| &e.geom), keyed).unwrap()
}

fn solve_system(sys: &System) -> Result<Solution, SolveError> {
    let weights = sys.unit_weights();
    let admissible = |x: &[f64]| sys.admissible(x);
    solve(
        &Problem::new(&sys.params, &sys.fixed, &weights, &sys.equations)
            .with_admissible(&admissible),
    )
}

/// 解けて、解が有限で、解いた形が方程式を満たすことを確かめて返す。
fn solved(sys: &System) -> Vec<f64> {
    let sol = solve_system(sys).expect("system must converge");
    assert!(sol.params.iter().all(|v| v.is_finite()), "{:?}", sol.params);
    assert!(sol.objective.iter().all(|v| v.is_finite()));
    assert_eq!(is_satisfied(&sol.params, &sys.equations), Ok(true));
    sol.params
}

fn line_at(sys: &System, entity: EntityId, x: &[f64]) -> LineSeg {
    match sys.shape(entity, x) {
        Some(Shape::Line(l)) => l,
        other => panic!("expected a line, got {other:?}"),
    }
}

fn circle_at(sys: &System, entity: EntityId, x: &[f64]) -> Circle {
    match sys.shape(entity, x) {
        Some(Shape::Circle(c)) => c,
        other => panic!("expected a circle, got {other:?}"),
    }
}

fn arc_at(sys: &System, entity: EntityId, x: &[f64]) -> Arc {
    match sys.shape(entity, x) {
        Some(Shape::Arc(a)) => a,
        other => panic!("expected an arc, got {other:?}"),
    }
}

/// 線分 `a` の方向から `b` の方向へのなす角。
fn angle_between(a: LineSeg, b: LineSeg) -> f64 {
    let (d1, d2) = (a.direction(), b.direction());
    d1.cross(d2).atan2(d1.dot(d2))
}

fn wrap_pi(a: f64) -> f64 {
    a.sin().atan2(a.cos())
}

/// すべての方程式について、添字に重複が無く、解析ヤコビアンが中心差分と合うこと。
fn assert_jacobians(sys: &System) -> Result<(), TestCaseError> {
    for eq in &sys.equations {
        let ps = eq.params();
        for (k, p) in ps.iter().enumerate() {
            prop_assert!(!ps[..k].contains(p), "duplicate parameter in {eq:?}");
        }
        let r = check_jacobian(eq, &sys.params, 1e-6, 1e-5);
        prop_assert!(r.is_ok(), "{r:?} for {eq:?} at {:?}", sys.params);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// proptest の生成器（座標は ±1000、線分の長さ 5 以上、半径 1 以上）
// ---------------------------------------------------------------------------

fn pt() -> impl Strategy<Value = Point2> {
    (-1000.0f64..1000.0, -1000.0f64..1000.0).prop_map(|(x, y)| Point2::new(x, y))
}

fn line_shape() -> impl Strategy<Value = Shape> {
    (pt(), 5.0f64..1000.0, -PI..PI).prop_map(|(a, len, t)| {
        Shape::Line(LineSeg::new(
            a,
            Point2::new(a.x + len * t.cos(), a.y + len * t.sin()),
        ))
    })
}

fn circle_shape() -> impl Strategy<Value = Shape> {
    (pt(), 1.0f64..1000.0).prop_map(|(c, r)| Shape::Circle(Circle::new(c, r)))
}

fn arc_shape() -> impl Strategy<Value = Shape> {
    (pt(), 1.0f64..1000.0, -10.0f64..10.0, -10.0f64..10.0)
        .prop_map(|(c, r, s, e)| Shape::Arc(Arc::new(c, r, s, e)))
}

fn round_shape() -> impl Strategy<Value = Shape> {
    prop_oneof![circle_shape(), arc_shape()]
}

/// 点を持つ図形と、その点の種類（円弧の端点・中心を含む）。
fn holder() -> impl Strategy<Value = (Shape, PointKind)> {
    prop_oneof![
        (
            line_shape(),
            prop_oneof![Just(PointKind::Start), Just(PointKind::End)]
        ),
        (circle_shape(), Just(PointKind::Center)),
        (
            arc_shape(),
            prop_oneof![
                Just(PointKind::Start),
                Just(PointKind::End),
                Just(PointKind::Center)
            ]
        ),
    ]
}

fn side() -> impl Strategy<Value = Side> {
    prop_oneof![Just(Side::Left), Just(Side::Right)]
}

fn distance_between(doc: &Document, a: PointRef, b: PointRef) -> f64 {
    let (p, q) = (a.position(doc).unwrap(), b.position(doc).unwrap());
    (p.x - q.x).hypot(p.y - q.y)
}

proptest! {
    /// 一致・点どうしの距離・点の固定（点は線分の端点・円の中心・円弧の端点と中心）。
    #[test]
    fn jacobian_point_constraints(
        (s1, k1) in holder(),
        (s2, k2) in holder(),
        d in 1.0f64..1000.0,
        at in pt(),
    ) {
        let mut doc = Document::new();
        let a = PointRef::new(add(&mut doc, s1), k1);
        let b = PointRef::new(add(&mut doc, s2), k2);
        // ノルムが 0 に近い点の近くでは中心差分が食い違って当然なので避ける。
        prop_assume!(distance_between(&doc, a, b) >= 5.0);
        let sys = system(&doc, &[
            Constraint::Coincident { a, b },
            Constraint::Distance { a, b, value: d },
            Constraint::FixPoint { point: a, at },
            Constraint::FixPoint { point: b, at },
        ]);
        prop_assert_eq!(sys.equations.len(), 7);
        assert_jacobians(&sys)?;
    }

    /// 同じ円弧の 2 点を指す拘束（中心・半径のパラメータを両方の点が共有する）。
    #[test]
    fn jacobian_points_on_the_same_arc(shape in arc_shape(), d in 1.0f64..1000.0) {
        let mut doc = Document::new();
        let e = add(&mut doc, shape);
        let (start, end, center) = (
            PointRef::new(e, PointKind::Start),
            PointRef::new(e, PointKind::End),
            PointRef::new(e, PointKind::Center),
        );
        prop_assume!(distance_between(&doc, start, end) >= 5.0);
        prop_assume!(distance_between(&doc, start, center) >= 5.0);
        let sys = system(&doc, &[
            Constraint::Distance { a: start, b: end, value: d },
            Constraint::Distance { a: start, b: center, value: d },
            Constraint::Distance { a: center, b: end, value: d },
            Constraint::Coincident { a: start, b: end },
            Constraint::Coincident { a: end, b: center },
        ]);
        assert_jacobians(&sys)?;
    }

    /// 同じ線分の始点・終点の距離（線分の長さ）。
    #[test]
    fn jacobian_line_length(shape in line_shape(), d in 1.0f64..1000.0) {
        let mut doc = Document::new();
        let e = add(&mut doc, shape);
        let sys = system(&doc, &[Constraint::Distance {
            a: PointRef::new(e, PointKind::Start),
            b: PointRef::new(e, PointKind::End),
            value: d,
        }]);
        assert_jacobians(&sys)?;
    }

    /// 線分どうしの拘束（水平・垂直・平行・直交・等長・角度）。
    #[test]
    fn jacobian_line_constraints(s1 in line_shape(), s2 in line_shape(), theta in -PI..PI) {
        let mut doc = Document::new();
        let a = add(&mut doc, s1);
        let b = add(&mut doc, s2);
        let angle = Constraint::Angle { a, b, value: theta };
        // 主値化の不連続点（φ − θ = ±π）の近くは避ける。
        let probe = system(&doc, std::slice::from_ref(&angle));
        prop_assume!(probe.equations[0].residual(&probe.params).abs() < 3.0);
        let sys = system(&doc, &[
            Constraint::Horizontal { line: a },
            Constraint::Vertical { line: b },
            Constraint::Parallel { a, b },
            Constraint::Perpendicular { a, b },
            Constraint::EqualLength { a, b },
            angle,
        ]);
        prop_assert_eq!(sys.equations.len(), 6);
        assert_jacobians(&sys)?;
    }

    /// 点と線の距離（点は円弧の端点を含む）と、線分と円・円弧の接線。
    #[test]
    fn jacobian_signed_distance(
        (holder_shape, kind) in holder(),
        line in line_shape(),
        round in round_shape(),
        d in 0.0f64..1000.0,
        s in side(),
    ) {
        let mut doc = Document::new();
        let point = PointRef::new(add(&mut doc, holder_shape), kind);
        let line = add(&mut doc, line);
        let circle = add(&mut doc, round);
        let sys = system(&doc, &[
            Constraint::PointLineDistance { point, line, side: s, value: d },
            Constraint::TangentLineCircle { line, circle, side: s },
        ]);
        assert_jacobians(&sys)?;
    }

    /// 円・円弧どうしの接線（外接・両方の内接）と、半径・等半径。
    #[test]
    fn jacobian_round_constraints(s1 in round_shape(), s2 in round_shape(), d in 1.0f64..1000.0) {
        let mut doc = Document::new();
        let a = add(&mut doc, s1);
        let b = add(&mut doc, s2);
        let center = |e| PointRef::new(e, PointKind::Center);
        prop_assume!(distance_between(&doc, center(a), center(b)) >= 5.0);
        let tangent = |kind| Constraint::TangentCircles { a, b, kind };
        let sys = system(&doc, &[
            tangent(CircleTangency::External),
            tangent(CircleTangency::Internal(LargerCircle::First)),
            tangent(CircleTangency::Internal(LargerCircle::Second)),
            Constraint::EqualRadius { a, b },
            Constraint::Radius { entity: a, value: d },
        ]);
        assert_jacobians(&sys)?;
    }

    /// 角度の残差とヤコビアン: `θ` が 0・±π の近く、線分が反平行に近い形。
    #[test]
    fn jacobian_angle_near_special_values(
        base in line_shape(),
        len in 5.0f64..1000.0,
        origin in pt(),
        // θ と φ（実際のなす角）を、0 の近くどうし、または ±π の近くどうし（同じ側・反対側の
        // 両方。反対側は φ − θ がほぼ ±2π になり、主値化しないと残差が跳ぶ組）に置く。
        // 不連続点（φ − θ = ±π）からは離れている。
        (theta_center, phi_center) in prop_oneof![
            Just((0.0, 0.0)),
            Just((PI, PI)),
            Just((PI, -PI)),
            Just((-PI, PI)),
            Just((-PI, -PI)),
        ],
        phi_off in -0.2f64..0.2,
        theta_off in -0.2f64..0.2,
    ) {
        let Shape::Line(l1) = base else { unreachable!() };
        let dir1 = l1.direction().angle();
        let phi = phi_center + phi_off;
        let l2 = LineSeg::new(
            origin,
            Point2::new(origin.x + len * (dir1 + phi).cos(), origin.y + len * (dir1 + phi).sin()),
        );
        let theta = theta_center + theta_off;
        prop_assert!(wrap_pi(phi - theta).abs() < 0.5);
        let mut doc = Document::new();
        let a = add(&mut doc, Shape::Line(l1));
        let b = add(&mut doc, Shape::Line(l2));
        let sys = system(&doc, &[Constraint::Angle { a, b, value: theta }]);
        // 残差は主値化した差そのもの（2π の跳びが無い）。
        let residual = sys.equations[0].residual(&sys.params);
        prop_assert!((residual - wrap_pi(phi - theta)).abs() < 1e-9, "{residual} vs {phi} - {theta}");
        assert_jacobians(&sys)?;
    }
}

// ---------------------------------------------------------------------------
// パラメータの並びと方程式の並び
// ---------------------------------------------------------------------------

#[test]
fn layout_is_ordered_by_entity_and_constraint_id() {
    let mut doc = Document::new();
    let l = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    let c = circle(&mut doc, (5.0, 5.0), 2.0);
    let a = arc(&mut doc, (1.0, 2.0), 3.0, 0.5, 2.5);
    let constraints = [
        Constraint::Radius {
            entity: a,
            value: 3.0,
        },
        Constraint::FixEntity { entity: c },
        Constraint::Coincident {
            a: PointRef::new(l, PointKind::End),
            b: PointRef::new(a, PointKind::Start),
        },
    ];
    let sys = system(&doc, &constraints);
    // エンティティは EntityId の昇順（追加した順）で、線分 4・円 3・円弧 5。
    assert_eq!(
        sys.slots
            .iter()
            .map(|s| (s.entity, s.kind, s.offset))
            .collect::<Vec<_>>(),
        vec![
            (l, ShapeKind::Line, 0),
            (c, ShapeKind::Circle, 4),
            (a, ShapeKind::Arc, 7)
        ]
    );
    assert_eq!(
        sys.params,
        vec![0.0, 0.0, 10.0, 0.0, 5.0, 5.0, 2.0, 1.0, 2.0, 3.0, 0.5, 2.5]
    );
    // 図形の固定は方程式を持たず、固定集合へ入る。
    let fixed: Vec<usize> = (0..sys.fixed.len()).filter(|&i| sys.fixed[i]).collect();
    assert_eq!(fixed, vec![4, 5, 6]);
    assert_eq!(
        sys.rows
            .iter()
            .map(|r| r.equations.clone())
            .collect::<Vec<_>>(),
        vec![0..1, 1..1, 1..3]
    );
    // 渡す順を変えても、ConstraintId の昇順に並ぶ（決定的）。
    let mut ids: SlotMap<ConstraintId, ()> = SlotMap::with_key();
    let keyed: Vec<(ConstraintId, &Constraint)> =
        constraints.iter().map(|c| (ids.insert(()), c)).collect();
    let reversed: Vec<(ConstraintId, &Constraint)> = keyed.iter().rev().copied().collect();
    let geom_of = |id| doc.entity(id).map(|e| &e.geom);
    assert_eq!(
        System::build(geom_of, keyed).unwrap(),
        System::build(geom_of, reversed).unwrap()
    );
}

#[test]
fn length_scale_uses_every_referenced_entity_and_the_value() {
    // 一致拘束そのものは長さを持たないが、点を持つ図形の大きさが L に入る。
    let mut doc = Document::new();
    let l = line(&mut doc, (0.0, 0.0), (300.0, 400.0));
    let c = circle(&mut doc, (-7.0, 2.0), 20.0);
    let coincident = Constraint::Coincident {
        a: PointRef::new(l, PointKind::Start),
        b: PointRef::new(c, PointKind::Center),
    };
    let sys = system(&doc, &[coincident]);
    assert_eq!(
        sys.equations[0].kind(&sys.params),
        ResidualKind::Length {
            size: 500.0,
            coord: 7.0
        }
    );
    // 値が図形より大きければ値が L になる。半径だけを見る拘束は座標を読まない（C = 0）。
    let sys = system(
        &doc,
        &[Constraint::Radius {
            entity: c,
            value: 1000.0,
        }],
    );
    assert_eq!(
        sys.equations[0].kind(&sys.params),
        ResidualKind::Length {
            size: 1000.0,
            coord: 0.0
        }
    );
    // 点の固定は p₀ の座標も C に入る。
    let sys = system(
        &doc,
        &[Constraint::FixPoint {
            point: PointRef::new(c, PointKind::Center),
            at: Point2::new(3.0, -9000.0),
        }],
    );
    assert_eq!(
        sys.equations[1].kind(&sys.params),
        ResidualKind::Length {
            size: 20.0,
            coord: 9000.0
        }
    );
    // 平行・直交・角度は角度型。
    let l2 = line(&mut doc, (0.0, 0.0), (1.0, 0.0));
    let sys = system(
        &doc,
        &[
            Constraint::Parallel { a: l, b: l2 },
            Constraint::Perpendicular { a: l, b: l2 },
            Constraint::Angle {
                a: l,
                b: l2,
                value: 0.0,
            },
        ],
    );
    assert!(
        sys.equations
            .iter()
            .all(|eq| eq.kind(&sys.params) == ResidualKind::Angle)
    );
}

#[test]
fn degenerate_line_makes_direction_equations_non_finite() {
    // 方向を使う方程式は、線分が退化していると NaN（満たされていない扱い）。
    let mut doc = Document::new();
    let dot = line(&mut doc, (1.0, 1.0), (1.0, 1.0));
    let other = line(&mut doc, (0.0, 0.0), (5.0, 0.0));
    let c = circle(&mut doc, (0.0, 3.0), 3.0);
    for constraint in [
        Constraint::Parallel { a: dot, b: other },
        Constraint::Perpendicular { a: other, b: dot },
        Constraint::Angle {
            a: dot,
            b: other,
            value: 0.0,
        },
        Constraint::TangentLineCircle {
            line: dot,
            circle: c,
            side: Side::Left,
        },
        Constraint::PointLineDistance {
            point: PointRef::new(c, PointKind::Center),
            line: dot,
            side: Side::Left,
            value: 1.0,
        },
    ] {
        let sys = system(&doc, std::slice::from_ref(&constraint));
        assert!(
            sys.equations[0].residual(&sys.params).is_nan(),
            "{constraint:?}"
        );
        assert!(sys.first_unsatisfied().is_some(), "{constraint:?}");
        assert_eq!(constraint.is_satisfied(&doc), Ok(false));
        assert_eq!(solve_system(&sys), Err(SolveError::NonFinite));
        assert!(!sys.admissible(&sys.params));
    }
    // 方向を使わない拘束（水平・等長）は退化した線分でも有限。
    let sys = system(
        &doc,
        &[
            Constraint::Horizontal { line: dot },
            Constraint::EqualLength { a: dot, b: other },
        ],
    );
    assert!(
        sys.equations
            .iter()
            .all(|eq| eq.residual(&sys.params).is_finite())
    );
    let mut grad = [0.0; 8];
    sys.equations[1].gradient(&sys.params, &mut grad);
    assert!(grad.iter().all(|g| g.is_finite()));
}

// ---------------------------------------------------------------------------
// 角度（完了条件 2）
// ---------------------------------------------------------------------------

/// 原点から長さ 10 で方向 `t` の線分。
fn ray(doc: &mut Document, origin: (f64, f64), t: f64) -> EntityId {
    line(
        doc,
        origin,
        (origin.0 + 10.0 * t.cos(), origin.1 + 10.0 * t.sin()),
    )
}

#[test]
fn angle_residual_is_continuous_across_pi() {
    // 2 本目を、なす角 φ が +π をまたぐように回す（φ = π − 0.1 … π + 0.1）。θ を π の近くに
    // 置くと、φ を直接 θ と引いた残差は φ が −π 側へ折り返した瞬間に 2π 跳ぶ。
    for theta in [PI - 0.05, PI, -PI + 0.05] {
        let steps = 200;
        let mut previous: Option<f64> = None;
        for i in 0..=steps {
            let phi = PI - 0.1 + 0.2 * f64::from(i) / f64::from(steps);
            let mut doc = Document::new();
            let a = ray(&mut doc, (3.0, -2.0), 0.4);
            let b = ray(&mut doc, (-5.0, 7.0), 0.4 + phi);
            let sys = system(&doc, &[Constraint::Angle { a, b, value: theta }]);
            let residual = sys.equations[0].residual(&sys.params);
            assert!(
                (residual - wrap_pi(phi - theta)).abs() < 1e-9,
                "theta {theta}, phi {phi}: {residual}"
            );
            assert!(residual.abs() < 0.2, "residual jumped: {residual}");
            if let Some(prev) = previous {
                assert!(
                    (residual - prev).abs() < 2.0 * 0.2 / f64::from(steps),
                    "discontinuity between steps: {prev} -> {residual}"
                );
            }
            previous = Some(residual);
            check_jacobian(&sys.equations[0], &sys.params, 1e-6, 1e-5)
                .unwrap_or_else(|m| panic!("theta {theta}, phi {phi}: {m:?}"));
        }
    }
}

#[test]
fn angle_converges_for_theta_near_zero_and_pi() {
    // 現在のなす角 φ から 0.3 rad 離れた θ（0・±π の近くを含む）へ解ける。
    for theta in [0.0, 1e-3, -1e-3, PI, PI - 1e-3, -PI + 1e-3, FRAC_PI_2] {
        for off in [0.3, -0.3] {
            let mut doc = Document::new();
            let a = ray(&mut doc, (1.0, 1.0), -0.7);
            let b = ray(&mut doc, (4.0, -2.0), -0.7 + theta + off);
            let sys = system(&doc, &[Constraint::Angle { a, b, value: theta }]);
            let x = solved(&sys);
            let phi = angle_between(line_at(&sys, a, &x), line_at(&sys, b, &x));
            assert!(
                wrap_pi(phi - theta).abs() <= 1e-9,
                "theta {theta}, off {off}: phi {phi}"
            );
            // 0.3 rad だけ動く局所解で、反対回りへ飛び移らない。
            let before =
                angle_between(line_at(&sys, a, &sys.params), line_at(&sys, b, &sys.params));
            assert!((wrap_pi(phi - before).abs() - 0.3).abs() < 1e-6);
        }
    }
}

#[test]
fn angle_with_antiparallel_lines() {
    // 反平行（φ = π）。atan2 は ±π のどちらも返しうるが、θ = π なら残差は 0。
    let mut doc = Document::new();
    let a = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    let b = line(&mut doc, (8.0, 3.0), (-2.0, 3.0));
    let c = Constraint::angle(&doc, a, b).unwrap();
    assert_eq!(c, Constraint::Angle { a, b, value: PI });
    assert_eq!(c.is_satisfied(&doc), Ok(true));
    // 逆順（b → a）は −π ではなく π に正規化される。
    assert_eq!(Constraint::angle(&doc, b, a).unwrap().value(), Some(PI));
    // 反平行は平行拘束も満たす。
    assert_eq!(Constraint::Parallel { a, b }.is_satisfied(&doc), Ok(true));
    let sys = system(&doc, std::slice::from_ref(&c));
    assert_eq!(sys.equations[0].residual(&sys.params), 0.0);
    check_jacobian(&sys.equations[0], &sys.params, 1e-6, 1e-5).unwrap();
    // 反平行の形から θ = π − 0.2・−π + 0.2 へ、どちらも 0.2 rad の回転で解ける。
    for theta in [PI - 0.2, -PI + 0.2] {
        let sys = system(&doc, &[Constraint::Angle { a, b, value: theta }]);
        assert!((sys.equations[0].residual(&sys.params).abs() - 0.2).abs() < 1e-12);
        let x = solved(&sys);
        let phi = angle_between(line_at(&sys, a, &x), line_at(&sys, b, &x));
        assert!(wrap_pi(phi - theta).abs() <= 1e-9);
    }
}

// ---------------------------------------------------------------------------
// 特異点（完了条件 3）
// ---------------------------------------------------------------------------

#[test]
fn distance_between_coincident_points_solves_without_nan() {
    // 原点で一致する 2 点（L 以外の尺度がすべて 0）と、離れた位置で一致する 2 点。
    for at in [(0.0, 0.0), (120.0, -45.0)] {
        let mut doc = Document::new();
        let l1 = line(&mut doc, (at.0 - 10.0, at.1), at);
        let l2 = line(&mut doc, at, (at.0, at.1 + 10.0));
        let (a, b) = (
            PointRef::new(l1, PointKind::End),
            PointRef::new(l2, PointKind::Start),
        );
        let sys = system(&doc, &[Constraint::Distance { a, b, value: 5.0 }]);
        let mut grad = [0.0; 4];
        sys.equations[0].gradient(&sys.params, &mut grad);
        // ノルムが 0 の点では方向 (1, 0) を使う（0 除算にならない）。
        assert_eq!(grad, [1.0, 0.0, -1.0, 0.0]);
        let x = solved(&sys);
        let (p, q) = (line_at(&sys, l1, &x).b, line_at(&sys, l2, &x).a);
        assert!(((p.x - q.x).hypot(p.y - q.y) - 5.0).abs() <= 1e-8);
        // x 方向へ離れる（決定的）。
        assert!(p.x > q.x && (p.y - q.y).abs() < 1e-12);
    }
}

#[test]
fn external_tangency_of_concentric_circles_solves_without_nan() {
    let mut doc = Document::new();
    let a = circle(&mut doc, (0.0, 0.0), 3.0);
    let b = circle(&mut doc, (0.0, 0.0), 2.0);
    let sys = system(
        &doc,
        &[Constraint::TangentCircles {
            a,
            b,
            kind: CircleTangency::External,
        }],
    );
    let x = solved(&sys);
    let (ca, cb) = (circle_at(&sys, a, &x), circle_at(&sys, b, &x));
    let dist = (ca.center.x - cb.center.x).hypot(ca.center.y - cb.center.y);
    assert!((dist - (ca.radius + cb.radius)).abs() <= 1e-8);
    assert!(ca.radius > 0.0 && cb.radius > 0.0);
}

// ---------------------------------------------------------------------------
// 点の固定（完了条件 4）
// ---------------------------------------------------------------------------

#[test]
fn fixed_point_on_arc_leaves_three_degrees_of_freedom() {
    let mut doc = Document::new();
    let a = arc(&mut doc, (2.0, 1.0), 5.0, 0.3, 2.0);
    for kind in [PointKind::Start, PointKind::End, PointKind::Center] {
        let fix = Constraint::fix_point(&doc, PointRef::new(a, kind)).unwrap();
        let sys = system(&doc, &[fix]);
        let weights = sys.unit_weights();
        let problem = Problem::new(&sys.params, &sys.fixed, &weights, &sys.equations);
        assert_eq!(degrees_of_freedom(&problem), Ok(3), "{kind:?}");
    }
    // 図形の固定は全パラメータを固定集合へ入れる（自由度 0）。
    let sys = system(&doc, &[Constraint::FixEntity { entity: a }]);
    let weights = sys.unit_weights();
    let problem = Problem::new(&sys.params, &sys.fixed, &weights, &sys.equations);
    assert_eq!(degrees_of_freedom(&problem), Ok(0));
}

#[test]
fn radius_change_keeps_fixed_arc_start_in_place() {
    let mut doc = Document::new();
    let a = arc(&mut doc, (2.0, 1.0), 5.0, 0.3, 2.0);
    let fix = Constraint::fix_point(&doc, PointRef::new(a, PointKind::Start)).unwrap();
    let Constraint::FixPoint { at, .. } = fix else {
        panic!("expected FixPoint");
    };
    let sys = system(
        &doc,
        &[
            fix,
            Constraint::Radius {
                entity: a,
                value: 8.0,
            },
        ],
    );
    let before = arc_at(&sys, a, &sys.params);
    let x = solved(&sys);
    let after = arc_at(&sys, a, &x);
    assert!((after.radius - 8.0).abs() <= 1e-8);
    // 始点は動かず、中心と終点が動く。
    let start = after.start_point();
    assert!((start.x - at.x).hypot(start.y - at.y) <= 1e-8);
    assert!((after.center.x - before.center.x).hypot(after.center.y - before.center.y) > 1.0);
    let (e0, e1) = (before.end_point(), after.end_point());
    assert!((e0.x - e1.x).hypot(e0.y - e1.y) > 1.0);
}

// ---------------------------------------------------------------------------
// 接線（完了条件 5 のうち解く部分）
// ---------------------------------------------------------------------------

#[test]
fn tangent_with_center_on_line_solves_to_the_left_side() {
    // 中心が直線上: コンストラクタは左側（+1）を選び、解くと中心が左側へ半径だけ離れる。
    for fix_line in [true, false] {
        let mut doc = Document::new();
        let l = line(&mut doc, (-10.0, 0.0), (10.0, 0.0));
        let c = circle(&mut doc, (2.0, 0.0), 5.0);
        let tangent = Constraint::tangent_line_circle(&doc, l, c).unwrap();
        assert_eq!(
            tangent,
            Constraint::TangentLineCircle {
                line: l,
                circle: c,
                side: Side::Left
            }
        );
        assert_eq!(tangent.is_satisfied(&doc), Ok(false));
        let mut constraints = vec![tangent];
        if fix_line {
            constraints.push(Constraint::FixEntity { entity: l });
        }
        let sys = system(&doc, &constraints);
        let x = solved(&sys);
        let (seg, round) = (line_at(&sys, l, &x), circle_at(&sys, c, &x));
        let d = seg.direction();
        let signed = d.cross(round.center - seg.a) / d.length();
        // 符号付き距離が +半径（始点 → 終点の向きに対して左側）。
        assert!((signed - round.radius).abs() <= 1e-8, "signed {signed}");
        assert!(signed > 0.0 && round.radius > 0.0);
        if fix_line {
            assert_eq!(seg, line_at(&sys, l, &sys.params));
        }
    }
    // 向きを逆にした線分では、左側は反対（y が負の側）になる。
    let mut doc = Document::new();
    let l = line(&mut doc, (10.0, 0.0), (-10.0, 0.0));
    let c = circle(&mut doc, (2.0, 0.0), 5.0);
    let tangent = Constraint::tangent_line_circle(&doc, l, c).unwrap();
    let sys = system(&doc, &[tangent, Constraint::FixEntity { entity: l }]);
    let x = solved(&sys);
    assert!((circle_at(&sys, c, &x).center.y + circle_at(&sys, c, &x).radius).abs() <= 1e-8);
}
