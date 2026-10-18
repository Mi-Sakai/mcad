//! 拘束記号の配置のテスト（M13 タスク99）。描画ではなく [`constraint_symbols`] の一覧で
//! 確かめる。

use std::f64::consts::{FRAC_PI_2, PI};

use mcad_core::{CircleTangency, Command, Entity, LargerCircle, PointKind, PointRef, Side, Style};
use mcad_geom::{Arc, Circle, LineSeg};

use super::*;
use crate::plot::svg::to_svg;
use crate::plot::{PlotColorMode, plot_page};

fn p(x: f64, y: f64) -> Point2 {
    Point2::new(x, y)
}

fn rect() -> Rect {
    Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))
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

fn constrain(doc: &mut Document, c: Constraint) -> ConstraintId {
    doc.apply(Command::AddConstraint {
        constraint: c,
        hold: None,
    })
    .unwrap()
    .constraints[0]
}

fn symbols(doc: &Document) -> Vec<ConstraintSymbol> {
    constraint_symbols(doc, &Viewport::new(), rect())
}

fn screen(q: Point2) -> Pos2 {
    Viewport::new().world_to_screen(rect(), q)
}

fn symbol_rect(s: &ConstraintSymbol) -> Rect {
    Rect::from_center_size(s.center, s.size)
}

#[test]
fn no_constraints_no_symbols() {
    let mut doc = Document::new();
    line(&mut doc, 0.0, 0.0, 100.0, 0.0);
    assert!(symbols(&doc).is_empty());
}

/// 横線 2 本（y = 0 と y = 80、長さ 200）の文書。
fn two_horizontal_lines() -> (Document, EntityId, EntityId) {
    let mut doc = Document::new();
    let a = line(&mut doc, -100.0, 0.0, 100.0, 0.0);
    let b = line(&mut doc, -100.0, 80.0, 100.0, 80.0);
    (doc, a, b)
}

#[test]
fn two_target_symbols_appear_on_both_targets() {
    for (make, glyph) in [
        (
            (|a, b| Constraint::Parallel { a, b }) as fn(EntityId, EntityId) -> Constraint,
            "∥",
        ),
        (|a, b| Constraint::EqualLength { a, b }, "="),
    ] {
        let (mut doc, a, b) = two_horizontal_lines();
        let id = constrain(&mut doc, make(a, b));
        let out = symbols(&doc);
        assert_eq!(out.len(), 2, "{out:?}");
        assert!(out.iter().all(|s| s.id == id));
        assert!(
            out.iter()
                .all(|s| s.glyph == SymbolGlyph::Text(glyph.to_owned()))
        );
        // それぞれの線分の中点の上に出る。
        let mid_a = screen(p(0.0, 0.0));
        let mid_b = screen(p(0.0, 80.0));
        assert!((out[0].center.x - mid_a.x).abs() < 1e-3 && out[0].center.y < mid_a.y);
        assert!((out[1].center.x - mid_b.x).abs() < 1e-3 && out[1].center.y < mid_b.y);
        assert!(out[0].center.y > mid_b.y, "a's symbol stays near a");
    }

    // 直交・角度も 2 つずつ。
    let (mut doc, a, _) = two_horizontal_lines();
    let perp_b = line(&mut doc, 200.0, -50.0, 200.0, 50.0);
    let id = constrain(&mut doc, Constraint::Perpendicular { a, b: perp_b });
    let angle = constrain(
        &mut doc,
        Constraint::Angle {
            a,
            b: perp_b,
            value: FRAC_PI_2,
        },
    );
    let out = symbols(&doc);
    assert_eq!(out.iter().filter(|s| s.id == id).count(), 2);
    let angle_syms: Vec<_> = out.iter().filter(|s| s.id == angle).collect();
    assert_eq!(angle_syms.len(), 2);
    assert_eq!(angle_syms[0].glyph, SymbolGlyph::Text("∠ 90°".to_owned()));

    // 等半径も 2 つ。
    let mut doc = Document::new();
    let c1 = add(&mut doc, Shape::Circle(Circle::new(p(0.0, 0.0), 30.0)));
    let c2 = add(&mut doc, Shape::Circle(Circle::new(p(100.0, 0.0), 30.0)));
    let id = constrain(&mut doc, Constraint::EqualRadius { a: c1, b: c2 });
    let out = symbols(&doc);
    assert_eq!(out.len(), 2);
    assert!(
        out.iter()
            .all(|s| s.id == id && s.kind == ConstraintKind::EqualRadius)
    );
}

#[test]
fn several_symbols_on_one_entity_do_not_overlap_and_follow_the_id_order() {
    let mut doc = Document::new();
    let a = line(&mut doc, -100.0, 0.0, 100.0, 0.0);
    let b = line(&mut doc, -100.0, 250.0, 100.0, 250.0);
    let h = constrain(&mut doc, Constraint::Horizontal { line: a });
    let eq = constrain(&mut doc, Constraint::EqualLength { a, b });
    let len = constrain(
        &mut doc,
        Constraint::Distance {
            a: PointRef::new(a, PointKind::Start),
            b: PointRef::new(a, PointKind::End),
            value: 200.0,
        },
    );
    let fix = constrain(&mut doc, Constraint::FixEntity { entity: a });
    let out = symbols(&doc);
    let mid_a = screen(p(0.0, 0.0));
    let on_a: Vec<_> = out
        .iter()
        .filter(|s| (s.center.x - mid_a.x).abs() < 1e-3 && s.center.y < mid_a.y)
        .filter(|s| s.center.y > screen(p(0.0, 250.0)).y + 20.0)
        .collect();
    let ids: Vec<_> = on_a.iter().map(|s| s.id).collect();
    assert_eq!(ids, vec![h, eq, len, fix], "{on_a:?}");
    // ID 順に外側へ積まれ、互いに重ならない。
    for pair in on_a.windows(2) {
        assert!(pair[1].center.y < pair[0].center.y);
        assert!(!symbol_rect(pair[0]).intersects(symbol_rect(pair[1])));
    }
    assert_eq!(on_a[2].glyph, SymbolGlyph::Text("↔ 200".to_owned()));
    assert_eq!(on_a[3].glyph, SymbolGlyph::Lock);
    // 同じ入力なら同じ配置（決定的）。
    assert_eq!(symbols(&doc), out);
}

#[test]
fn coincident_is_a_dot_on_the_point_and_fix_point_a_lock_beside_it() {
    let mut doc = Document::new();
    let a = line(&mut doc, 0.0, 0.0, 100.0, 0.0);
    let b = line(&mut doc, 100.0, 0.0, 100.0, 50.0);
    let coincident = constrain(
        &mut doc,
        Constraint::Coincident {
            a: PointRef::new(a, PointKind::End),
            b: PointRef::new(b, PointKind::Start),
        },
    );
    let fix = constrain(
        &mut doc,
        Constraint::FixPoint {
            point: PointRef::new(a, PointKind::End),
            at: p(100.0, 0.0),
        },
    );
    let out = symbols(&doc);
    let dot = out.iter().find(|s| s.id == coincident).unwrap();
    assert_eq!(dot.glyph, SymbolGlyph::Dot);
    assert_eq!(dot.center, screen(p(100.0, 0.0)));
    let lock = out.iter().find(|s| s.id == fix).unwrap();
    assert_eq!(lock.glyph, SymbolGlyph::Lock);
    let corner = screen(p(100.0, 0.0));
    assert!(
        lock.center.x > corner.x && lock.center.y < corner.y,
        "upper right"
    );
}

#[test]
fn values_are_shown_in_mm_and_degrees() {
    let mut doc = Document::new();
    let a = line(&mut doc, 0.0, 0.0, 100.0, 0.0);
    let b = line(&mut doc, 0.0, 30.0, 100.0, 30.0);
    let c = add(&mut doc, Shape::Circle(Circle::new(p(300.0, 0.0), 12.5)));
    constrain(
        &mut doc,
        Constraint::Radius {
            entity: c,
            value: 12.5,
        },
    );
    constrain(
        &mut doc,
        Constraint::PointLineDistance {
            point: PointRef::new(b, PointKind::Start),
            line: a,
            side: Side::Left,
            value: 30.0,
        },
    );
    constrain(&mut doc, Constraint::Angle { a, b, value: 0.0 });
    let texts: Vec<_> = symbols(&doc)
        .into_iter()
        .filter_map(|s| match s.glyph {
            SymbolGlyph::Text(t) => Some(t),
            _ => None,
        })
        .collect();
    assert!(texts.contains(&"R 12.5".to_owned()), "{texts:?}");
    assert!(texts.contains(&"↔ 30".to_owned()), "{texts:?}");
    assert!(texts.contains(&"∠ 0°".to_owned()), "{texts:?}");
    assert_eq!(format_number(1.0 / 3.0, 3), "0.333");
    assert_eq!(format_number(-0.0001, 3), "0");
    assert_eq!(format_number(45.000_000_1, 2), "45");
}

#[test]
fn hidden_layers_hide_their_symbols() {
    let mut doc = Document::new();
    let a = line(&mut doc, -100.0, 0.0, 100.0, 0.0);
    let hidden = mcad_core::Layer::new("hidden", mcad_core::Rgb::WHITE);
    let layer = doc.apply(Command::AddLayer(hidden)).unwrap().layers[0];
    let b = add(
        &mut doc,
        Shape::Line(LineSeg::new(p(-100.0, 50.0), p(100.0, 50.0))),
    );
    doc.apply(Command::SetEntityLayer { id: b, layer }).unwrap();
    let id = constrain(&mut doc, Constraint::Parallel { a, b });
    assert_eq!(symbols(&doc).len(), 2);
    let mut props = doc.layer(layer).unwrap().clone();
    props.visible = false;
    doc.apply(Command::SetLayerProps { id: layer, props })
        .unwrap();
    let out = symbols(&doc);
    assert_eq!(out.len(), 1, "only the visible line keeps its symbol");
    assert_eq!(out[0].id, id);
}

// ---------------------------------------------------------------------------
// 接線の記号と延長の破線（DESIGN.md M13 判断2「タスク96・99 の完了条件」）
// ---------------------------------------------------------------------------

fn tangent_symbol(doc: &Document, id: ConstraintId) -> ConstraintSymbol {
    let out = symbols(doc);
    let found: Vec<_> = out.into_iter().filter(|s| s.id == id).collect();
    assert_eq!(found.len(), 1, "one T per tangent");
    assert_eq!(found[0].glyph, SymbolGlyph::Text("T".to_owned()));
    found[0].clone()
}

#[test]
fn line_circle_tangent_is_dashed_only_when_the_contact_is_off_the_segment() {
    // 円 (0, 50) r=50 と直線 y=0。接点は (0, 0)。
    let mut doc = Document::new();
    let c = add(&mut doc, Shape::Circle(Circle::new(p(0.0, 50.0), 50.0)));
    let inside = line(&mut doc, -100.0, 0.0, 100.0, 0.0);
    let outside = line(&mut doc, 100.0, 0.0, 200.0, 0.0);
    let t_in = constrain(
        &mut doc,
        Constraint::TangentLineCircle {
            line: inside,
            circle: c,
            side: Side::Left,
        },
    );
    let t_out = constrain(
        &mut doc,
        Constraint::TangentLineCircle {
            line: outside,
            circle: c,
            side: Side::Left,
        },
    );
    let s = tangent_symbol(&doc, t_in);
    assert!(s.dashes.is_empty(), "{s:?}");
    // 記号は接点の外側（円の中心の反対 = 画面の下）。
    assert!(s.center.y > screen(p(0.0, 0.0)).y);
    let s = tangent_symbol(&doc, t_out);
    assert_eq!(
        s.dashes,
        vec![[screen(p(0.0, 0.0)), screen(p(100.0, 0.0))]],
        "from the contact to the nearest end of the line"
    );
}

#[test]
fn arc_tangent_across_pi_is_dashed_outside_the_sweep() {
    // 円弧は 3π/4 → −3π/4（CCW で ±π をまたぐ 90° の掃引。左側の円弧）。
    let mut doc = Document::new();
    let arc_shape = Arc::new(p(0.0, 0.0), 50.0, 3.0 * PI / 4.0, -3.0 * PI / 4.0);
    let arc = add(&mut doc, Shape::Arc(arc_shape));
    // 左の接線 x = −50（接点 (−50, 0) は掃引の中）。下から上へ引くと中心は右側。
    let left = line(&mut doc, -50.0, -100.0, -50.0, 100.0);
    // 右の接線 x = 50（接点 (50, 0) は掃引の外）。
    let right = line(&mut doc, 50.0, -100.0, 50.0, 100.0);
    let t_left = constrain(
        &mut doc,
        Constraint::TangentLineCircle {
            line: left,
            circle: arc,
            side: Side::Right,
        },
    );
    let t_right = constrain(
        &mut doc,
        Constraint::TangentLineCircle {
            line: right,
            circle: arc,
            side: Side::Left,
        },
    );
    assert!(tangent_symbol(&doc, t_left).dashes.is_empty());
    let s = tangent_symbol(&doc, t_right);
    assert_eq!(s.dashes.len(), 1, "{s:?}");
    let [from, to] = s.dashes[0];
    assert_eq!(from, screen(p(50.0, 0.0)));
    // 最寄りの端点は始点・終点のどちらか（どちらも同じ距離。始点を選ぶ）。
    assert_eq!(to, screen(arc_shape.start_point()));
}

#[test]
fn circle_tangents_external_and_internal_mark_the_arc_outside_its_sweep() {
    let mut doc = Document::new();
    // 外接: 円 (0,0) r=50 と、中心 (80,0) r=30 の円弧。接点 (50, 0)。
    let big = add(&mut doc, Shape::Circle(Circle::new(p(0.0, 0.0), 50.0)));
    // 円弧の掃引は 3π/4 → −3π/4（±π をまたぐ。接点の方向 π を含む）。
    let arc_in = add(
        &mut doc,
        Shape::Arc(Arc::new(
            p(80.0, 0.0),
            30.0,
            3.0 * PI / 4.0,
            -3.0 * PI / 4.0,
        )),
    );
    // 同じ円で、掃引が −π/4 → π/4（接点の方向 π を含まない）。
    let arc_out = add(
        &mut doc,
        Shape::Arc(Arc::new(p(80.0, 0.0), 30.0, -PI / 4.0, PI / 4.0)),
    );
    let ext_in = constrain(
        &mut doc,
        Constraint::TangentCircles {
            a: big,
            b: arc_in,
            kind: CircleTangency::External,
        },
    );
    let ext_out = constrain(
        &mut doc,
        Constraint::TangentCircles {
            a: big,
            b: arc_out,
            kind: CircleTangency::External,
        },
    );
    assert!(tangent_symbol(&doc, ext_in).dashes.is_empty());
    let s = tangent_symbol(&doc, ext_out);
    assert_eq!(s.dashes.len(), 1);
    assert_eq!(s.dashes[0][0], screen(p(50.0, 0.0)));

    // 内接: 円 (0,0) r=50 の内側に中心 (30,0) r=20 の円弧。接点 (50, 0)（方向 0）。
    let small_in = add(
        &mut doc,
        Shape::Arc(Arc::new(p(30.0, 0.0), 20.0, -PI / 2.0, PI / 2.0)),
    );
    let small_out = add(
        &mut doc,
        Shape::Arc(Arc::new(p(30.0, 0.0), 20.0, PI / 2.0, 3.0 * PI / 2.0)),
    );
    let int_in = constrain(
        &mut doc,
        Constraint::TangentCircles {
            a: big,
            b: small_in,
            kind: CircleTangency::Internal(LargerCircle::First),
        },
    );
    let int_out = constrain(
        &mut doc,
        Constraint::TangentCircles {
            a: big,
            b: small_out,
            kind: CircleTangency::Internal(LargerCircle::First),
        },
    );
    assert!(tangent_symbol(&doc, int_in).dashes.is_empty());
    let s = tangent_symbol(&doc, int_out);
    assert_eq!(s.dashes.len(), 1, "{s:?}");
    assert_eq!(s.dashes[0][0], screen(p(50.0, 0.0)));

    // 1 つ目の対象（円弧）の外: 円弧どうしの外接で a が掃引外。
    let a_arc = add(
        &mut doc,
        Shape::Arc(Arc::new(p(0.0, 200.0), 50.0, PI / 2.0, PI)),
    );
    let b_circle = add(&mut doc, Shape::Circle(Circle::new(p(80.0, 200.0), 30.0)));
    let first_out = constrain(
        &mut doc,
        Constraint::TangentCircles {
            a: a_arc,
            b: b_circle,
            kind: CircleTangency::External,
        },
    );
    let s = tangent_symbol(&doc, first_out);
    assert_eq!(s.dashes.len(), 1, "{s:?}");
    // 接点 (50, 200) から円弧の最寄りの端点（始点 (0, 250)・終点 (−50, 200) のうち始点）。
    assert_eq!(s.dashes[0], [screen(p(50.0, 200.0)), screen(p(0.0, 250.0))]);
}

// ---------------------------------------------------------------------------
// 出力に出ない（検収 (viii)）
// ---------------------------------------------------------------------------

#[test]
fn svg_output_is_the_same_with_and_without_constraints() {
    let build = |with_constraints: bool| {
        let mut doc = Document::new();
        let a = line(&mut doc, 0.0, 0.0, 100.0, 0.0);
        let b = line(&mut doc, 100.0, 0.0, 100.0, 60.0);
        let c = add(&mut doc, Shape::Circle(Circle::new(p(50.0, 30.0), 20.0)));
        if with_constraints {
            // 図形が既に満たしている拘束だけを付ける（解いて形が変わると比較にならない）。
            constrain(&mut doc, Constraint::Horizontal { line: a });
            constrain(&mut doc, Constraint::Vertical { line: b });
            constrain(
                &mut doc,
                Constraint::Coincident {
                    a: PointRef::new(a, PointKind::End),
                    b: PointRef::new(b, PointKind::Start),
                },
            );
            constrain(&mut doc, Constraint::Perpendicular { a, b });
            constrain(
                &mut doc,
                Constraint::Radius {
                    entity: c,
                    value: 20.0,
                },
            );
            constrain(&mut doc, Constraint::FixEntity { entity: c });
            assert_eq!(doc.constraint_count(), 6);
            assert!(!symbols(&doc).is_empty());
        }
        doc
    };
    let with = build(true);
    let without = build(false);
    for mode in [
        PlotColorMode::Monochrome,
        PlotColorMode::Blueprint,
        PlotColorMode::Color,
    ] {
        assert_eq!(
            to_svg(&plot_page(&with, mode)),
            to_svg(&plot_page(&without, mode)),
            "{mode:?}"
        );
    }
}
