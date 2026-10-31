//! 図形のグリップの純関数のテスト（M13 タスク101）。

use std::f64::consts::{FRAC_PI_2, PI};

use mcad_core::{
    Command, DimAnnotation, DimDirection, DimLinear, Layer, Rgb, Style, TextGeom, XLineGeom,
};
use mcad_geom::Polyline;

use super::*;

/// ピック許容量なし（数値の許容量だけで判定する）。
const NO_TOL: f64 = 0.0;

fn p(x: f64, y: f64) -> Point2 {
    Point2::new(x, y)
}

fn line_geom(a: (f64, f64), b: (f64, f64)) -> EntityGeom {
    EntityGeom::Shape(Shape::Line(LineSeg::new(p(a.0, a.1), p(b.0, b.1))))
}

fn circle_geom(c: (f64, f64), r: f64) -> EntityGeom {
    EntityGeom::Shape(Shape::Circle(Circle::new(p(c.0, c.1), r)))
}

fn arc_geom(c: (f64, f64), r: f64, start: f64, end: f64) -> EntityGeom {
    EntityGeom::Shape(Shape::Arc(Arc::new(p(c.0, c.1), r, start, end)))
}

fn add(doc: &mut Document, geom: EntityGeom) -> EntityId {
    let layer = doc.current_layer();
    doc.apply(Command::AddEntity(Entity::new(
        geom,
        layer,
        Style::inherited(),
    )))
    .unwrap()
    .entities[0]
}

/// パラメータの bit 列（固定集合 (c) は f64 の完全一致で比べるので、bit で確かめる）。
fn bits(geom: &EntityGeom) -> Vec<u64> {
    let values: Vec<f64> = match geom.as_shape().unwrap() {
        Shape::Line(l) => vec![l.a.x, l.a.y, l.b.x, l.b.y],
        Shape::Circle(c) => vec![c.center.x, c.center.y, c.radius],
        Shape::Arc(a) => vec![a.center.x, a.center.y, a.radius, a.start_angle, a.end_angle],
        other => panic!("{other:?}"),
    };
    values.into_iter().map(f64::to_bits).collect()
}

/// 値が変わったパラメータの添字。
fn changed(before: &EntityGeom, after: &EntityGeom) -> Vec<usize> {
    bits(before)
        .into_iter()
        .zip(bits(after))
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, _)| i)
        .collect()
}

#[test]
fn grips_are_listed_only_for_lines_circles_and_arcs() {
    let kinds = |geom: &EntityGeom| -> Vec<ShapeGripKind> {
        shape_grips(geom)
            .into_iter()
            .map(|(kind, _)| kind)
            .collect()
    };
    assert_eq!(
        shape_grips(&line_geom((1.0, 2.0), (3.0, 4.0))),
        vec![
            (ShapeGripKind::LineStart, p(1.0, 2.0)),
            (ShapeGripKind::LineEnd, p(3.0, 4.0)),
        ]
    );
    // 円: 中心と四半円点 4 か所。
    let circle = circle_geom((0.0, 0.0), 2.0);
    assert_eq!(
        kinds(&circle),
        vec![
            ShapeGripKind::Center,
            ShapeGripKind::Radius { index: 0 },
            ShapeGripKind::Radius { index: 1 },
            ShapeGripKind::Radius { index: 2 },
            ShapeGripKind::Radius { index: 3 },
        ]
    );
    let quadrants: Vec<Point2> = shape_grips(&circle).iter().skip(1).map(|g| g.1).collect();
    for (point, expected) in
        quadrants
            .iter()
            .zip([p(2.0, 0.0), p(0.0, 2.0), p(-2.0, 0.0), p(0.0, -2.0)])
    {
        assert!(point.distance(expected) < 1e-12, "{point:?}");
    }
    // 円弧: 中心・両端・掃引の中央。掃引が 0 をまたいでも中央は弧の上。
    let grips = shape_grips(&arc_geom((0.0, 0.0), 2.0, -FRAC_PI_2, FRAC_PI_2));
    assert_eq!(
        grips.iter().map(|g| g.0).collect::<Vec<_>>(),
        vec![
            ShapeGripKind::Center,
            ShapeGripKind::ArcStart,
            ShapeGripKind::ArcEnd,
            ShapeGripKind::Radius { index: 0 },
        ]
    );
    assert!(grips[3].1.distance(p(2.0, 0.0)) < 1e-12, "{:?}", grips[3]);
    let wrapped = shape_grips(&arc_geom((0.0, 0.0), 2.0, 1.5 * PI, 0.5 * PI));
    assert!(wrapped[3].1.distance(p(2.0, 0.0)) < 1e-12, "{wrapped:?}");

    // 対象外の幾何。
    for geom in [
        EntityGeom::Shape(Shape::Point(p(0.0, 0.0))),
        EntityGeom::Shape(Shape::Polyline(Polyline::new(
            vec![p(0.0, 0.0), p(1.0, 0.0)],
            false,
        ))),
        EntityGeom::XLine(XLineGeom {
            base: p(0.0, 0.0),
            dir: mcad_geom::Vec2::new(1.0, 0.0),
        }),
        EntityGeom::Text(TextGeom {
            anchor: p(0.0, 0.0),
            content: "x".to_owned(),
            height: 3.0,
            angle: 0.0,
        }),
        EntityGeom::DimLinear(DimLinear {
            p1: p(0.0, 0.0),
            p2: p(4.0, 0.0),
            offset: 2.0,
            direction: DimDirection::Aligned,
            ext_angle: None,
            annotation: DimAnnotation::default(),
        }),
    ] {
        assert!(shape_grips(&geom).is_empty(), "{geom:?}");
    }
}

#[test]
fn each_grip_changes_only_its_own_parameters() {
    let cursor = p(7.25, -3.5);
    // 線分: パラメータは a.x, a.y, b.x, b.y。
    let line = line_geom((0.1, 0.2), (10.3, 0.4));
    let after = apply_shape_grip(&line, ShapeGripKind::LineStart, cursor, NO_TOL).unwrap();
    assert_eq!(changed(&line, &after), [0, 1]);
    assert_eq!(after, line_geom((7.25, -3.5), (10.3, 0.4)));
    let after = apply_shape_grip(&line, ShapeGripKind::LineEnd, cursor, NO_TOL).unwrap();
    assert_eq!(changed(&line, &after), [2, 3]);
    assert_eq!(after, line_geom((0.1, 0.2), (7.25, -3.5)));

    // 円: パラメータは c.x, c.y, r。
    let circle = circle_geom((0.5, 1.5), 2.5);
    let after = apply_shape_grip(&circle, ShapeGripKind::Center, cursor, NO_TOL).unwrap();
    assert_eq!(changed(&circle, &after), [0, 1]);
    for index in 0..4 {
        let after =
            apply_shape_grip(&circle, ShapeGripKind::Radius { index }, cursor, NO_TOL).unwrap();
        assert_eq!(changed(&circle, &after), [2]);
        let Some(Shape::Circle(c)) = after.as_shape() else {
            panic!("{after:?}");
        };
        assert_eq!(c.radius, p(0.5, 1.5).distance(cursor));
    }

    // 円弧: パラメータは c.x, c.y, r, 開始角, 終了角。
    let arc = arc_geom((0.5, 1.5), 2.5, 0.25, 2.0);
    let after = apply_shape_grip(&arc, ShapeGripKind::Center, cursor, NO_TOL).unwrap();
    assert_eq!(changed(&arc, &after), [0, 1]);
    let after = apply_shape_grip(&arc, ShapeGripKind::Radius { index: 0 }, cursor, NO_TOL).unwrap();
    assert_eq!(changed(&arc, &after), [2]);
    let d = cursor - p(0.5, 1.5);
    let after = apply_shape_grip(&arc, ShapeGripKind::ArcStart, cursor, NO_TOL).unwrap();
    assert_eq!(changed(&arc, &after), [3]);
    assert_eq!(after, arc_geom((0.5, 1.5), 2.5, d.y.atan2(d.x), 2.0));
    let after = apply_shape_grip(&arc, ShapeGripKind::ArcEnd, cursor, NO_TOL).unwrap();
    assert_eq!(changed(&arc, &after), [4]);
    assert_eq!(after, arc_geom((0.5, 1.5), 2.5, 0.25, d.y.atan2(d.x)));
}

/// 手で重ねたカーソルは数値の許容量まで一致しない。画面上で区別できない距離（ピック許容量）で
/// 拒否する。スモークの指摘: 円弧の端点を手で重ねても拒否されず、ほぼ長さ 0 や全周の円弧になった。
#[test]
fn overlaps_within_the_pick_tolerance_are_rejected() {
    let tol = 0.05;
    let arc = arc_geom((0.0, 0.0), 5.0, 0.0, FRAC_PI_2);
    // 始点 (5, 0) の少し上（掃引が 0 に近い）と少し下（掃引が全周に近い）。
    for y in [0.01, -0.01] {
        let near_start = p(5.0, y);
        assert!(apply_shape_grip(&arc, ShapeGripKind::ArcEnd, near_start, NO_TOL).is_ok());
        let reason = apply_shape_grip(&arc, ShapeGripKind::ArcEnd, near_start, tol).unwrap_err();
        assert_eq!(reason, "円弧の両端が重なります");
    }
    assert!(apply_shape_grip(&arc, ShapeGripKind::ArcStart, p(0.01, 5.0), tol).is_err());
    // 許容量の外なら通る。
    assert!(apply_shape_grip(&arc, ShapeGripKind::ArcEnd, p(5.0, 0.1), tol).is_ok());

    let line = line_geom((0.0, 0.0), (10.0, 0.0));
    assert!(apply_shape_grip(&line, ShapeGripKind::LineEnd, p(0.01, 0.01), NO_TOL).is_ok());
    assert!(apply_shape_grip(&line, ShapeGripKind::LineEnd, p(0.01, 0.01), tol).is_err());
    assert!(apply_shape_grip(&line, ShapeGripKind::LineEnd, p(0.1, 0.0), tol).is_ok());

    let circle = circle_geom((1.0, 1.0), 5.0);
    let near_center = p(1.01, 1.0);
    let radius = ShapeGripKind::Radius { index: 0 };
    assert!(apply_shape_grip(&circle, radius, near_center, NO_TOL).is_ok());
    assert!(apply_shape_grip(&circle, radius, near_center, tol).is_err());
    assert!(apply_shape_grip(&circle, ShapeGripKind::Center, near_center, tol).is_ok());

    // 円弧の中心の近くでは角度が定まらない。
    assert!(apply_shape_grip(&arc, ShapeGripKind::ArcEnd, p(0.01, 0.0), tol).is_err());
}

#[test]
fn degenerate_results_are_rejected_before_solving() {
    let line = line_geom((0.0, 0.0), (10.0, 0.0));
    assert!(apply_shape_grip(&line, ShapeGripKind::LineStart, p(10.0, 0.0), NO_TOL).is_err());
    assert!(apply_shape_grip(&line, ShapeGripKind::LineEnd, p(0.0, 0.0), NO_TOL).is_err());

    let circle = circle_geom((1.0, 1.0), 5.0);
    assert!(
        apply_shape_grip(
            &circle,
            ShapeGripKind::Radius { index: 2 },
            p(1.0, 1.0),
            NO_TOL
        )
        .is_err()
    );
    // 中心を動かすのに退化は無い。
    assert!(apply_shape_grip(&circle, ShapeGripKind::Center, p(1.0, 1.0), NO_TOL).is_ok());

    let arc = arc_geom((0.0, 0.0), 5.0, 0.0, FRAC_PI_2);
    assert!(
        apply_shape_grip(
            &arc,
            ShapeGripKind::Radius { index: 0 },
            p(0.0, 0.0),
            NO_TOL
        )
        .is_err()
    );
    assert!(apply_shape_grip(&arc, ShapeGripKind::ArcEnd, p(0.0, 0.0), NO_TOL).is_err());
    // 掃引がちょうど 0 になる: 端点どうしが重なる。
    assert!(apply_shape_grip(&arc, ShapeGripKind::ArcEnd, p(3.0, 0.0), NO_TOL).is_err());
    assert!(apply_shape_grip(&arc, ShapeGripKind::ArcStart, p(0.0, 3.0), NO_TOL).is_err());
    // 反対側へ回り込むのは許す（掃引が 180° を超える）。
    let after = apply_shape_grip(&arc, ShapeGripKind::ArcEnd, p(0.0, -3.0), NO_TOL).unwrap();
    let Some(Shape::Arc(a)) = after.as_shape() else {
        panic!("{after:?}");
    };
    assert!((a.sweep() - 1.5 * PI).abs() < 1e-12, "{a:?}");
    let after = apply_shape_grip(&arc, ShapeGripKind::ArcStart, p(0.0, -3.0), NO_TOL).unwrap();
    let Some(Shape::Arc(a)) = after.as_shape() else {
        panic!("{after:?}");
    };
    assert!((a.sweep() - PI).abs() < 1e-12, "{a:?}");

    // 図形に無いグリップ。
    assert!(apply_shape_grip(&line, ShapeGripKind::Center, p(1.0, 1.0), NO_TOL).is_err());
    assert!(apply_shape_grip(&circle, ShapeGripKind::LineEnd, p(1.0, 1.0), NO_TOL).is_err());
    assert!(apply_shape_grip(&circle, ShapeGripKind::ArcEnd, p(1.0, 2.0), NO_TOL).is_err());
}

#[test]
fn the_dragged_arc_end_is_drawn_on_the_circle() {
    let arc = arc_geom((1.0, 0.0), 5.0, 0.0, 1.0);
    let point = dragged_grip_point(&arc, ShapeGripKind::ArcEnd, p(1.0, 20.0));
    assert!(point.distance(p(1.0, 5.0)) < 1e-12, "{point:?}");
    // ほかのグリップはカーソルそのもの。
    assert_eq!(
        dragged_grip_point(&arc, ShapeGripKind::Radius { index: 0 }, p(1.0, 20.0)),
        p(1.0, 20.0)
    );
    assert_eq!(
        dragged_grip_point(
            &line_geom((0.0, 0.0), (1.0, 0.0)),
            ShapeGripKind::LineEnd,
            p(4.0, 4.0)
        ),
        p(4.0, 4.0)
    );
    // 中心では射影できないので、カーソルのまま。
    assert_eq!(
        dragged_grip_point(&arc, ShapeGripKind::ArcStart, p(1.0, 0.0)),
        p(1.0, 0.0)
    );
}

fn target_ids(doc: &Document, selection: &[EntityId]) -> Vec<EntityId> {
    shape_grip_targets(doc, selection)
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

#[test]
fn targets_exclude_hidden_locked_and_fixed_shapes() {
    let mut doc = Document::new();
    let plain = add(&mut doc, line_geom((0.0, 0.0), (1.0, 0.0)));
    let circle = add(&mut doc, circle_geom((5.0, 0.0), 1.0));
    let arc = add(&mut doc, arc_geom((9.0, 0.0), 1.0, 0.0, 1.0));
    let polyline = add(
        &mut doc,
        EntityGeom::Shape(Shape::Polyline(Polyline::new(
            vec![p(0.0, 5.0), p(1.0, 5.0)],
            false,
        ))),
    );
    // 選択順のまま返す。
    assert_eq!(
        target_ids(&doc, &[arc, plain, polyline, circle]),
        vec![arc, plain, circle]
    );

    // 図形の固定。
    let fix = doc
        .apply(Command::AddConstraint {
            constraint: Constraint::FixEntity { entity: circle },
            hold: None,
        })
        .unwrap()
        .constraints[0];
    assert_eq!(target_ids(&doc, &[plain, circle]), vec![plain]);
    doc.apply(Command::RemoveConstraint(fix)).unwrap();
    assert_eq!(target_ids(&doc, &[plain, circle]), vec![plain, circle]);

    // ロックされたレイヤー・非表示のレイヤー。
    let layer_of = |doc: &mut Document, id: EntityId| {
        let layer = doc
            .apply(Command::AddLayer(Layer::new("other", Rgb::WHITE)))
            .unwrap()
            .layers[0];
        doc.apply(Command::SetEntityLayer { id, layer }).unwrap();
        layer
    };
    let locked_layer = layer_of(&mut doc, plain);
    let mut props = doc.layer(locked_layer).unwrap().clone();
    props.locked = true;
    doc.apply(Command::SetLayerProps {
        id: locked_layer,
        props,
    })
    .unwrap();
    assert_eq!(target_ids(&doc, &[plain, circle]), vec![circle]);

    let hidden_layer = layer_of(&mut doc, circle);
    let mut props = doc.layer(hidden_layer).unwrap().clone();
    props.visible = false;
    doc.apply(Command::SetLayerProps {
        id: hidden_layer,
        props,
    })
    .unwrap();
    assert_eq!(target_ids(&doc, &[plain, circle]), Vec::<EntityId>::new());
}

#[test]
fn a_selection_over_the_limit_shows_no_shape_grips() {
    let mut doc = Document::new();
    let ids: Vec<EntityId> = (0..=SHAPE_GRIP_SELECTION_LIMIT)
        .map(|i| {
            let y = i as f64;
            add(&mut doc, line_geom((0.0, y), (1.0, y)))
        })
        .collect();
    assert_eq!(SHAPE_GRIP_SELECTION_LIMIT, 100);
    assert_eq!(ids.len(), 101);
    assert_eq!(target_ids(&doc, &ids[..100]).len(), 100);
    assert!(target_ids(&doc, &ids).is_empty());
    assert!(target_ids(&doc, &[]).is_empty());
}
