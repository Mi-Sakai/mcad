//! 拘束つきの図形に対するツール単位のテスト（M13 タスク97）。
//!
//! トリム（1 断片）・延長・フィレット・面取りが `Command::ReshapeEntity` を出して拘束を
//! 設計判断4 の対応表どおりに扱うこと、ミラーが拘束を組み直して全拘束を保つこと。

use super::*;
use mcad_core::{Constraint, PointKind, PointRef};

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

fn line(ax: f64, ay: f64, bx: f64, by: f64) -> Shape {
    Shape::Line(LineSeg::new(Point2::new(ax, ay), Point2::new(bx, by)))
}

fn pick(doc: &Document, id: EntityId, click: Point2) -> ShapePick {
    let e = doc.entity(id).unwrap();
    ShapePick {
        id,
        shape: e.geom.as_shape().unwrap().clone(),
        click,
        layer: e.layer,
        style: e.style,
    }
}

fn constrain(doc: &mut Document, c: Constraint) {
    doc.apply(Command::AddConstraint {
        constraint: c,
        hold: None,
    })
    .unwrap();
}

fn pt(entity: EntityId, kind: PointKind) -> PointRef {
    PointRef { entity, kind }
}

fn line_geom(doc: &Document, id: EntityId) -> LineSeg {
    match doc.entity(id).unwrap().geom.as_shape().unwrap() {
        Shape::Line(l) => *l,
        other => panic!("expected line, got {other:?}"),
    }
}

fn has_coincident(doc: &Document) -> bool {
    doc.constraints()
        .any(|(_, c)| matches!(c, Constraint::Coincident { .. }))
}

fn has_horizontal(doc: &Document) -> bool {
    doc.constraints()
        .any(|(_, c)| matches!(c, Constraint::Horizontal { .. }))
}

fn assert_all_satisfied(doc: &Document) {
    for (id, c) in doc.constraints() {
        assert!(
            c.is_satisfied(doc).unwrap(),
            "{id:?} {c:?} is not satisfied"
        );
    }
}

/// 水平な線分 `(0,0)-(4,0)` と、その終点で一致する縦線 `(4,0)-(4,3)` を作る。
fn horizontal_with_joint(doc: &mut Document) -> (EntityId, EntityId) {
    let a = add(doc, line(0.0, 0.0, 4.0, 0.0));
    let b = add(doc, line(4.0, 0.0, 4.0, 3.0));
    constrain(doc, Constraint::Horizontal { line: a });
    constrain(
        doc,
        Constraint::Coincident {
            a: pt(a, PointKind::End),
            b: pt(b, PointKind::Start),
        },
    );
    (a, b)
}

#[test]
fn extend_drops_the_coincidence_of_the_moved_end_and_keeps_horizontal() {
    let mut doc = Document::new();
    let (a, _b) = horizontal_with_joint(&mut doc);
    let boundary = add(&mut doc, line(6.0, -1.0, 6.0, 1.0));
    let before = doc.constraint_count();
    assert_eq!(before, 2);

    let mut tool = ExtendTool::default();
    let bp = pick(&doc, boundary, Point2::new(6.0, 0.5));
    assert_eq!(tool.on_shape_pick(bp), ToolResult::Continue);
    let target = pick(&doc, a, Point2::new(3.5, 0.0));
    let ToolResult::Commit(cmd) = tool.on_shape_pick(target) else {
        panic!("expected commit");
    };
    assert!(matches!(cmd, Command::ReshapeEntity { .. }));
    let shape_before = doc.entity(a).unwrap().geom.clone();
    let ids = doc.apply(cmd).expect("extend is accepted");

    let seg = line_geom(&doc, a);
    assert!((seg.b.x - 6.0).abs() < 1e-9 && seg.a == Point2::new(0.0, 0.0));
    assert!(!has_coincident(&doc), "moved end's coincidence is dropped");
    assert!(has_horizontal(&doc));
    assert_eq!(
        crate::removed_constraint_count(before, doc.constraint_count(), ids.constraints.len()),
        1
    );

    assert!(doc.undo());
    assert_eq!(doc.entity(a).unwrap().geom, shape_before);
    assert_eq!(doc.constraint_count(), before, "undo restores constraints");
    assert!(has_coincident(&doc));
}

#[test]
fn one_piece_trim_drops_the_coincidence_of_the_moved_end_and_keeps_horizontal() {
    let mut doc = Document::new();
    let (a, _b) = horizontal_with_joint(&mut doc);
    let boundary = add(&mut doc, line(3.0, -1.0, 3.0, 1.0));
    let before = doc.constraint_count();

    let mut tool = TrimTool::default();
    let bp = pick(&doc, boundary, Point2::new(3.0, 0.5));
    assert_eq!(tool.on_shape_pick(bp), ToolResult::Continue);
    let target = pick(&doc, a, Point2::new(3.5, 0.0));
    let ToolResult::Commit(cmd) = tool.on_shape_pick(target) else {
        panic!("expected commit");
    };
    assert!(matches!(cmd, Command::ReshapeEntity { .. }));
    let shape_before = doc.entity(a).unwrap().geom.clone();
    doc.apply(cmd).expect("trim is accepted");

    let seg = line_geom(&doc, a);
    assert!((seg.b.x - 3.0).abs() < 1e-9);
    assert!(!has_coincident(&doc));
    assert!(has_horizontal(&doc));
    assert_all_satisfied(&doc);

    assert!(doc.undo());
    assert_eq!(doc.entity(a).unwrap().geom, shape_before);
    assert_eq!(doc.constraint_count(), before);
}

/// 角 (0,0) で一致する水平線と垂直線。`a_corner_is_start` / `b_corner_is_start` で、
/// 各線分の向き（始点が角か終点が角か）を振る。
fn corner_pair(
    doc: &mut Document,
    a_corner_is_start: bool,
    b_corner_is_start: bool,
) -> (EntityId, EntityId) {
    let a = add(
        doc,
        if a_corner_is_start {
            line(0.0, 0.0, 10.0, 0.0)
        } else {
            line(10.0, 0.0, 0.0, 0.0)
        },
    );
    let b = add(
        doc,
        if b_corner_is_start {
            line(0.0, 0.0, 0.0, 10.0)
        } else {
            line(0.0, 10.0, 0.0, 0.0)
        },
    );
    let kind = |s: bool| if s { PointKind::Start } else { PointKind::End };
    constrain(doc, Constraint::Horizontal { line: a });
    constrain(doc, Constraint::Vertical { line: b });
    constrain(doc, Constraint::Perpendicular { a, b });
    constrain(
        doc,
        Constraint::Coincident {
            a: pt(a, kind(a_corner_is_start)),
            b: pt(b, kind(b_corner_is_start)),
        },
    );
    (a, b)
}

const ORIENTATIONS: [(bool, bool); 4] =
    [(true, true), (true, false), (false, true), (false, false)];

#[test]
fn fillet_keeps_orientation_and_drops_only_the_corner_coincidence() {
    for (sa, sb) in ORIENTATIONS {
        let mut doc = Document::new();
        let (a, b) = corner_pair(&mut doc, sa, sb);
        assert_eq!(doc.constraint_count(), 4);
        let (a_before, b_before) = (line_geom(&doc, a), line_geom(&doc, b));

        let mut tool = FilletTool::default();
        tool.set_radius_input(Some(2.0));
        let pa = pick(&doc, a, Point2::new(8.0, 0.0));
        let pb = pick(&doc, b, Point2::new(0.0, 8.0));
        assert_eq!(tool.on_shape_pick(pa), ToolResult::Continue);
        let ToolResult::Commit(cmd) = tool.on_shape_pick(pb) else {
            panic!("expected commit ({sa},{sb})");
        };
        doc.apply(cmd)
            .unwrap_or_else(|e| panic!("fillet rejected ({sa},{sb}): {e}"));

        assert_eq!(doc.constraint_count(), 3, "({sa},{sb})");
        assert!(!has_coincident(&doc));
        assert_all_satisfied(&doc);
        // 向きが保たれている（far 端は動かない）。
        let far = |l: LineSeg, start_is_corner: bool| if start_is_corner { l.b } else { l.a };
        assert_eq!(far(line_geom(&doc, a), sa), far(a_before, sa));
        assert_eq!(far(line_geom(&doc, b), sb), far(b_before, sb));

        assert!(doc.undo());
        assert_eq!(doc.constraint_count(), 4);
        assert_eq!(line_geom(&doc, a), a_before);
    }
}

#[test]
fn chamfer_keeps_orientation_and_drops_only_the_corner_coincidence() {
    for (sa, sb) in ORIENTATIONS {
        let mut doc = Document::new();
        let (a, b) = corner_pair(&mut doc, sa, sb);
        let (a_before, b_before) = (line_geom(&doc, a), line_geom(&doc, b));

        let mut tool = ChamferTool::default();
        tool.set_chamfer_distances(Some(2.0), ChamferDistInput::Empty);
        let pa = pick(&doc, a, Point2::new(8.0, 0.0));
        let pb = pick(&doc, b, Point2::new(0.0, 8.0));
        assert_eq!(tool.on_shape_pick(pa), ToolResult::Continue);
        let ToolResult::Commit(cmd) = tool.on_shape_pick(pb) else {
            panic!("expected commit ({sa},{sb})");
        };
        doc.apply(cmd)
            .unwrap_or_else(|e| panic!("chamfer rejected ({sa},{sb}): {e}"));

        assert_eq!(doc.constraint_count(), 3, "({sa},{sb})");
        assert!(!has_coincident(&doc));
        assert_all_satisfied(&doc);
        let far = |l: LineSeg, start_is_corner: bool| if start_is_corner { l.b } else { l.a };
        assert_eq!(far(line_geom(&doc, a), sa), far(a_before, sa));
        assert_eq!(far(line_geom(&doc, b), sb), far(b_before, sb));

        assert!(doc.undo());
        assert_eq!(doc.constraint_count(), 4);
        assert_eq!(line_geom(&doc, b), b_before);
    }
}

/// 線分・円・円弧・もう 1 本の線分を、接線・角度・端点の一致・水平で結んだ図形群。
fn mirror_fixture(doc: &mut Document) -> Vec<EntityId> {
    let l = add(doc, line(0.0, 2.0, 5.0, 2.0));
    let circle = add(
        doc,
        Shape::Circle(mcad_geom::Circle::new(Point2::new(2.0, 3.0), 1.0)),
    );
    let arc = add(
        doc,
        Shape::Arc(Arc::new(
            Point2::new(5.0, 4.0),
            2.0,
            -std::f64::consts::FRAC_PI_2,
            0.0,
        )),
    );
    let l2 = add(doc, line(8.0, 0.0, 10.0, 1.0));
    constrain(doc, Constraint::Horizontal { line: l });
    constrain(
        doc,
        Constraint::tangent_line_circle(doc, l, circle).unwrap(),
    );
    constrain(doc, Constraint::angle(doc, l, l2).unwrap());
    constrain(
        doc,
        Constraint::Coincident {
            a: pt(l, PointKind::End),
            b: pt(arc, PointKind::Start),
        },
    );
    vec![l, circle, arc, l2]
}

fn run_mirror(doc: &Document, ids: &[EntityId]) -> Command {
    let mut tool = SelectTool::default();
    tool.set_selection(ids.to_vec());
    assert!(tool.start_mirror());
    assert_eq!(
        tool.placement_click(doc, Point2::new(0.0, 0.0), 0.1),
        PlacementOutcome::Continue
    );
    match tool.placement_click(doc, Point2::new(1.0, 0.0), 0.1) {
        PlacementOutcome::Commit { cmd, .. } => cmd,
        other => panic!("expected commit, got {other:?}"),
    }
}

#[test]
fn mirror_keeps_every_constraint_satisfied_and_undoes_as_one_unit() {
    let mut doc = Document::new();
    let ids = mirror_fixture(&mut doc);
    let count = doc.constraint_count();
    assert_eq!(count, 4);
    let geoms: Vec<_> = ids
        .iter()
        .map(|&i| doc.entity(i).unwrap().geom.clone())
        .collect();

    let cmd = run_mirror(&doc, &ids);
    let Command::Batch(subs) = &cmd else {
        panic!("expected batch");
    };
    assert!(
        subs.iter()
            .any(|c| matches!(c, Command::ReplaceConstraint { .. })),
        "tangent / angle / arc endpoint need rebuilding"
    );
    doc.apply(cmd).expect("mirror accepted");
    assert_eq!(doc.constraint_count(), count);
    assert_all_satisfied(&doc);

    assert!(doc.undo());
    for (i, g) in ids.iter().zip(&geoms) {
        assert_eq!(&doc.entity(*i).unwrap().geom, g);
    }
    assert_all_satisfied(&doc);
}

#[test]
fn mirror_without_constraints_emits_only_modify_entity() {
    let mut doc = Document::new();
    let a = add(&mut doc, line(0.0, 2.0, 5.0, 3.0));
    let b = add(
        &mut doc,
        Shape::Circle(mcad_geom::Circle::new(Point2::new(2.0, 3.0), 1.0)),
    );
    let Command::Batch(subs) = run_mirror(&doc, &[a, b]) else {
        panic!("expected batch");
    };
    assert_eq!(subs.len(), 2);
    assert!(
        subs.iter()
            .all(|c| matches!(c, Command::ModifyEntity { .. }))
    );
}

#[test]
fn constraint_removal_note_appears_only_when_something_was_removed() {
    assert_eq!(crate::removed_constraint_count(3, 1, 0), 2);
    assert_eq!(crate::removed_constraint_count(3, 3, 0), 0);
    // 同じ apply が 1 件追加していれば、差し引き 0 でも消えていない。
    assert_eq!(crate::removed_constraint_count(3, 4, 1), 0);
    assert_eq!(crate::removed_constraint_count(3, 3, 1), 1);
    assert_eq!(
        crate::with_constraint_note("Moved 2 entities", 0),
        "Moved 2 entities"
    );
    assert_eq!(
        crate::with_constraint_note("Moved 2 entities", 2),
        "Moved 2 entities, removed 2 constraint(s)"
    );
}

#[test]
fn extend_and_trim_from_either_end_keep_the_line_direction() {
    // a→b の向きが x 正方向・負方向の両方で、始点側・終点側のどちらを動かしても受理される。
    for (reversed, click_x) in [(false, 0.5), (false, 3.5), (true, 0.5), (true, 3.5)] {
        for trim in [false, true] {
            let mut doc = Document::new();
            let seg = if reversed {
                line(4.0, 0.0, 0.0, 0.0)
            } else {
                line(0.0, 0.0, 4.0, 0.0)
            };
            let a = add(&mut doc, seg);
            constrain(&mut doc, Constraint::Horizontal { line: a });
            // 延長は線分の外（x=-2 / x=6）、トリムは線分の内側（x=1 / x=3）に境界を置く。
            let bx = match (trim, click_x < 2.0) {
                (false, true) => -2.0,
                (false, false) => 6.0,
                (true, true) => 1.0,
                (true, false) => 3.0,
            };
            let boundary = add(&mut doc, line(bx, -1.0, bx, 1.0));
            let before = line_geom(&doc, a);
            let bp = pick(&doc, boundary, Point2::new(bx, 0.5));
            let target = pick(&doc, a, Point2::new(click_x, 0.0));
            let result = if trim {
                let mut t = TrimTool::default();
                t.on_shape_pick(bp);
                t.on_shape_pick(target)
            } else {
                let mut t = ExtendTool::default();
                t.on_shape_pick(bp);
                t.on_shape_pick(target)
            };
            let ToolResult::Commit(cmd) = result else {
                panic!("expected commit (rev={reversed}, x={click_x}, trim={trim})");
            };
            doc.apply(cmd).unwrap_or_else(|e| {
                panic!("rejected (rev={reversed}, x={click_x}, trim={trim}): {e}")
            });
            let after = line_geom(&doc, a);
            let dir = |l: LineSeg| (l.b.x - l.a.x).signum();
            assert_eq!(dir(after), dir(before));
        }
    }
}
