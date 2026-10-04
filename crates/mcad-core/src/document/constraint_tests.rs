//! [`Document`] に拘束を適用したときのテスト（M13 タスク96 の完了条件 6・7）。
//!
//! 追加・削除・値の変更と undo/redo、拒否されたコマンドの後の状態の完全一致、`Batch` での
//! 一括移動、`RemoveEntity` の連鎖削除、構造検証の拒否、固定図形の変更拒否、`ReshapeEntity`。
//! タスク97 で `apply` が拘束を解くようになったので、96 で「満たされていないから拒否」だった
//! 操作の多くは「解かれて追従する」に変わっている。求解そのもののテストは
//! `constraint_solve_tests.rs`。

use std::f64::consts::PI;

use mcad_geom::{Arc, Circle, LineSeg, Point2, Polyline, Shape, Vec2};

use super::*;
use crate::{CircleTangency, LargerCircle, PointKind, PointRef, Side, TextGeom};

// ---------------------------------------------------------------------------
// ヘルパー
// ---------------------------------------------------------------------------

pub(super) fn line_geom(a: (f64, f64), b: (f64, f64)) -> EntityGeom {
    EntityGeom::Shape(Shape::Line(LineSeg::new(
        Point2::new(a.0, a.1),
        Point2::new(b.0, b.1),
    )))
}

pub(super) fn circle_geom(c: (f64, f64), r: f64) -> EntityGeom {
    EntityGeom::Shape(Shape::Circle(Circle::new(Point2::new(c.0, c.1), r)))
}

pub(super) fn arc_geom(c: (f64, f64), r: f64, start: f64, end: f64) -> EntityGeom {
    EntityGeom::Shape(Shape::Arc(Arc::new(Point2::new(c.0, c.1), r, start, end)))
}

pub(super) fn add(doc: &mut Document, geom: EntityGeom) -> EntityId {
    let layer = doc.current_layer();
    doc.apply(Command::AddEntity(Entity::new(
        geom,
        layer,
        Style::inherited(),
    )))
    .unwrap()
    .entities[0]
}

pub(super) fn line(doc: &mut Document, a: (f64, f64), b: (f64, f64)) -> EntityId {
    add(doc, line_geom(a, b))
}

pub(super) fn circle(doc: &mut Document, c: (f64, f64), r: f64) -> EntityId {
    add(doc, circle_geom(c, r))
}

/// 止める図形なし（`hold: None`）の `AddConstraint`。
pub(super) fn add_constraint(constraint: Constraint) -> Command {
    Command::AddConstraint {
        constraint,
        hold: None,
    }
}

/// テスト用: 拘束を追加して発行された ID を返す（受理される前提。満たされていなければ
/// 解かれて形が変わる）。
///
/// メソッドにしてあるのは、引数の中で `Constraint::distance(&doc, ..)` のように文書を
/// 読めるようにするため（レシーバの `&mut` は 2 相借用になる）。
pub(super) trait Constrain {
    fn constrain(&mut self, constraint: Constraint) -> ConstraintId;
}

impl Constrain for Document {
    fn constrain(&mut self, constraint: Constraint) -> ConstraintId {
        let ids = self.apply(add_constraint(constraint)).unwrap();
        assert!(ids.entities.is_empty() && ids.layers.is_empty());
        assert_eq!(ids.constraints.len(), 1);
        ids.constraints[0]
    }
}

pub(super) fn start(e: EntityId) -> PointRef {
    PointRef::new(e, PointKind::Start)
}

pub(super) fn end(e: EntityId) -> PointRef {
    PointRef::new(e, PointKind::End)
}

pub(super) fn center(e: EntityId) -> PointRef {
    PointRef::new(e, PointKind::Center)
}

pub(super) fn geom(doc: &Document, id: EntityId) -> EntityGeom {
    doc.entity(id).unwrap().geom.clone()
}

pub(super) fn translated(doc: &Document, id: EntityId, delta: (f64, f64)) -> Command {
    Command::ModifyEntity {
        id,
        new_geom: geom(doc, id).translated(Vec2::new(delta.0, delta.1)),
    }
}

/// 観測可能な状態の全体（失敗したコマンドの前後で一致することを確かめる）。
#[derive(Debug, PartialEq)]
pub(super) struct Snapshot {
    pub(super) entities: Vec<(EntityId, Entity)>,
    pub(super) constraints: Vec<(ConstraintId, Constraint)>,
    constraint_count: usize,
    layers: Vec<(LayerId, Layer)>,
    current_layer: LayerId,
    undo_depth: usize,
    redo_depth: usize,
    generation: u64,
    next_generation: u64,
}

pub(super) fn snapshot(doc: &Document) -> Snapshot {
    Snapshot {
        entities: doc.entities().map(|(id, e)| (id, e.clone())).collect(),
        constraints: doc.constraints().map(|(id, c)| (id, c.clone())).collect(),
        constraint_count: doc.constraint_count(),
        layers: doc.layers().map(|(id, l)| (id, l.clone())).collect(),
        current_layer: doc.current_layer(),
        undo_depth: doc.undo_stack.len(),
        redo_depth: doc.redo_stack.len(),
        generation: doc.generation,
        next_generation: doc.next_generation,
    }
}

/// コマンドが `expected` で拒否され、状態が適用前と完全に一致することを確かめる。
pub(super) fn assert_rejected(
    doc: &mut Document,
    cmd: Command,
    expected: impl Fn(&CoreError) -> bool,
) {
    let before = snapshot(doc);
    let err = doc.apply(cmd).expect_err("command must be rejected");
    assert!(expected(&err), "unexpected error: {err:?}");
    assert_eq!(snapshot(doc), before, "state changed by a rejected command");
}

/// 止められた図形だけに関わる拘束が満たされない。
pub(super) fn blocked(err: &CoreError) -> bool {
    matches!(err, CoreError::ConstraintBlockedByFixed(_))
}

/// 図形が退化する（解く前から退化している場合を含む）。
pub(super) fn degenerate(err: &CoreError) -> bool {
    matches!(err, CoreError::DegenerateShape(_))
}

/// 2 つの図形（線分・円・円弧）のパラメータが `tol` 以内で一致する。
pub(super) fn assert_close(actual: &EntityGeom, expected: &EntityGeom, tol: f64) {
    let params = |g: &EntityGeom| -> Vec<f64> {
        match g.as_shape() {
            Some(Shape::Line(l)) => vec![l.a.x, l.a.y, l.b.x, l.b.y],
            Some(Shape::Circle(c)) => vec![c.center.x, c.center.y, c.radius],
            Some(Shape::Arc(a)) => {
                vec![a.center.x, a.center.y, a.radius, a.start_angle, a.end_angle]
            }
            _ => panic!("not a constrainable shape: {g:?}"),
        }
    };
    let (a, e) = (params(actual), params(expected));
    assert_eq!(a.len(), e.len(), "{actual:?} vs {expected:?}");
    for (x, y) in a.iter().zip(&e) {
        assert!((x - y).abs() <= tol, "{actual:?} vs {expected:?}");
    }
}

/// 文書のすべての拘束が満たされている（`apply` が受理した文書の不変条件）。
pub(super) fn assert_all_satisfied(doc: &Document) {
    for (id, constraint) in doc.constraints() {
        assert_eq!(
            constraint.is_satisfied(doc),
            Ok(true),
            "{id:?} {constraint:?}"
        );
    }
}

pub(super) fn invalid(err: &CoreError) -> bool {
    matches!(err, CoreError::InvalidConstraint(_))
}

/// 矩形（辺 4 本、反時計回り）。角の一致 4・水平 2・垂直 2 で拘束する。
pub(super) struct Rect {
    pub(super) sides: [EntityId; 4],
    pub(super) constraints: Vec<ConstraintId>,
}

pub(super) fn rectangle(doc: &mut Document) -> Rect {
    let corners = [(0.0, 0.0), (40.0, 0.0), (40.0, 30.0), (0.0, 30.0)];
    let sides: [EntityId; 4] = std::array::from_fn(|i| line(doc, corners[i], corners[(i + 1) % 4]));
    let mut constraints = Vec::new();
    for i in 0..4 {
        constraints.push(doc.constrain(Constraint::Coincident {
            a: end(sides[i]),
            b: start(sides[(i + 1) % 4]),
        }));
    }
    for (i, side) in sides.iter().enumerate() {
        let c = if i % 2 == 0 {
            Constraint::Horizontal { line: *side }
        } else {
            Constraint::Vertical { line: *side }
        };
        constraints.push(doc.constrain(c));
    }
    Rect { sides, constraints }
}

// ---------------------------------------------------------------------------
// 追加・削除と undo/redo
// ---------------------------------------------------------------------------

#[test]
fn new_document_has_no_constraints() {
    let doc = Document::new();
    assert_eq!(doc.constraint_count(), 0);
    assert_eq!(doc.constraints().count(), 0);
    assert_eq!(doc.constraint(ConstraintId::default()), None);
}

#[test]
fn add_satisfied_constraint_and_undo_redo_keep_id() {
    let mut doc = Document::new();
    let l1 = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    let l2 = line(&mut doc, (10.0, 0.0), (10.0, 5.0));
    let coincident = Constraint::Coincident {
        a: end(l1),
        b: start(l2),
    };
    let generation = doc.generation();
    let id = doc.constrain(coincident.clone());
    assert_eq!(doc.constraint(id), Some(&coincident));
    assert_eq!(doc.constraint_count(), 1);
    assert_ne!(doc.generation(), generation);
    assert_eq!(
        doc.constraints_of(l1).map(|(id, _)| id).collect::<Vec<_>>(),
        vec![id]
    );

    assert!(doc.undo());
    assert_eq!(doc.constraint(id), None);
    assert_eq!(doc.constraint_count(), 0);
    assert_eq!(doc.generation(), generation);

    assert!(doc.redo());
    // 墓標方式により、redo 後も同じ ID で戻る。
    assert_eq!(doc.constraint(id), Some(&coincident));
    assert_eq!(doc.constraint_count(), 1);

    // 削除と、その undo/redo。
    doc.apply(Command::RemoveConstraint(id)).unwrap();
    assert_eq!(doc.constraint(id), None);
    assert_eq!(doc.constraint_count(), 0);
    assert!(doc.undo());
    assert_eq!(doc.constraint(id), Some(&coincident));
    assert!(doc.redo());
    assert_eq!(doc.constraint(id), None);
    // 削除済み・存在しない ID の削除は拒否。
    assert_rejected(&mut doc, Command::RemoveConstraint(id), |e| {
        *e == CoreError::ConstraintNotFound(id)
    });
}

#[test]
fn constraints_are_listed_in_id_order() {
    let mut doc = Document::new();
    let rect = rectangle(&mut doc);
    let listed: Vec<ConstraintId> = doc.constraints().map(|(id, _)| id).collect();
    assert_eq!(listed, rect.constraints);
    let mut sorted = listed.clone();
    sorted.sort();
    assert_eq!(listed, sorted);
    assert_eq!(doc.constraint_count(), 8);
    // 1 辺に付く拘束: 両端の一致 2 つと、水平か垂直 1 つ。
    assert_eq!(doc.constraints_of(rect.sides[0]).count(), 3);
    // 途中を消しても順は保たれ、undo で同じ位置へ戻る。
    doc.apply(Command::RemoveConstraint(rect.constraints[2]))
        .unwrap();
    let listed: Vec<ConstraintId> = doc.constraints().map(|(id, _)| id).collect();
    assert_eq!(listed.len(), 7);
    assert!(!listed.contains(&rect.constraints[2]));
    assert!(doc.undo());
    let listed: Vec<ConstraintId> = doc.constraints().map(|(id, _)| id).collect();
    assert_eq!(listed, rect.constraints);
}

#[test]
fn unsatisfied_constraint_is_solved_on_add() {
    let mut doc = Document::new();
    let l1 = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    let l2 = line(&mut doc, (10.5, 0.5), (12.0, 5.0));
    let c = circle(&mut doc, (5.0, 4.0), 3.0);
    let shapes = |doc: &Document| (geom(doc, l1), geom(doc, l2), geom(doc, c));
    let before = shapes(&doc);
    for constraint in [
        Constraint::Coincident {
            a: end(l1),
            b: start(l2),
        },
        Constraint::Vertical { line: l2 },
        Constraint::Parallel { a: l1, b: l2 },
        Constraint::Distance {
            a: start(l1),
            b: end(l1),
            value: 9.0,
        },
        Constraint::Radius {
            entity: c,
            value: 3.5,
        },
        Constraint::TangentLineCircle {
            line: l1,
            circle: c,
            side: Side::Left,
        },
        Constraint::FixPoint {
            point: center(c),
            at: Point2::new(5.0, 4.1),
        },
        Constraint::Angle {
            a: l1,
            b: l2,
            value: 1.0,
        },
    ] {
        // 満たされていない拘束は、解かれて形が変わる（96 では拒否だった）。
        assert_eq!(constraint.is_satisfied(&doc), Ok(false), "{constraint:?}");
        let id = doc.constrain(constraint.clone());
        assert_eq!(constraint.is_satisfied(&doc), Ok(true), "{constraint:?}");
        let solved = shapes(&doc);
        assert_ne!(solved, before, "{constraint:?}");
        // undo 1 回で拘束と形がそろって戻り、redo は解き直さずに同じ形を再現する。
        assert!(doc.undo());
        assert_eq!(doc.constraint(id), None);
        assert_eq!(shapes(&doc), before, "{constraint:?}");
        assert!(doc.redo());
        assert_eq!(doc.constraint(id), Some(&constraint));
        assert_eq!(shapes(&doc), solved, "{constraint:?}");
        assert!(doc.undo());
    }
    // 満たされている拘束は、何も動かさずに入る。
    doc.constrain(Constraint::Horizontal { line: l1 });
    doc.constrain(Constraint::fix_point(&doc, center(c)).unwrap());
    doc.constrain(Constraint::radius(&doc, c).unwrap());
    assert_eq!(shapes(&doc), before);
}

#[test]
fn blocked_error_reports_first_violation_in_id_order() {
    let mut doc = Document::new();
    let l = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    let horizontal = doc.constrain(Constraint::Horizontal { line: l });
    let length = doc.constrain(Constraint::Distance {
        a: start(l),
        b: end(l),
        value: 10.0,
    });
    assert!(horizontal < length);
    // 両端を動かして両方の拘束に反する変更は、ID 順で最初の違反（水平）を返す
    // （動かした点は止められるので、解く余地が無い）。
    assert_rejected(
        &mut doc,
        Command::ModifyEntity {
            id: l,
            new_geom: line_geom((1.0, 1.0), (4.0, 5.0)),
        },
        |e| *e == CoreError::ConstraintBlockedByFixed(horizontal),
    );
    // 長さだけに反する変更は、長さの拘束を返す。
    assert_rejected(
        &mut doc,
        Command::ModifyEntity {
            id: l,
            new_geom: line_geom((1.0, 0.0), (13.0, 0.0)),
        },
        |e| *e == CoreError::ConstraintBlockedByFixed(length),
    );
    // 追加した拘束が満たせないときは、その（発行されるはずだった）拘束を返す: 水平・長さ 10 の
    // 線分に垂直を足すと、満たす形が無い。その ID は文書に残らない。
    let before = snapshot(&doc);
    let err = doc
        .apply(add_constraint(Constraint::Vertical { line: l }))
        .unwrap_err();
    assert_eq!(snapshot(&doc), before);
    let CoreError::ConstraintUnsolvable(rejected) = err else {
        panic!("unexpected {err:?}");
    };
    assert!(rejected > length);
    assert_eq!(doc.constraint(rejected), None);
}

// ---------------------------------------------------------------------------
// 拘束に反する変更の拒否と、一括移動
// ---------------------------------------------------------------------------

#[test]
fn modify_violating_own_constraint_is_rejected_with_state_intact() {
    let mut doc = Document::new();
    let rect = rectangle(&mut doc);
    // redo スタックも保たれることを見るため、undo 済みの操作を 1 つ残しておく。
    let extra = line(&mut doc, (100.0, 100.0), (110.0, 100.0));
    assert!(doc.undo());
    assert!(doc.can_redo());
    assert_eq!(doc.entity(extra), None);

    // 水平な辺を回転する: 動かした辺そのものが水平に反する（両端とも止められるので、
    // ほかの辺が追従しても満たせない）。
    let rotated = geom(&doc, rect.sides[0]).rotated(Point2::new(20.0, 0.0), 0.3);
    let horizontal = rect.constraints[4];
    assert_eq!(
        doc.constraint(horizontal),
        Some(&Constraint::Horizontal {
            line: rect.sides[0]
        })
    );
    assert_rejected(
        &mut doc,
        Command::ModifyEntity {
            id: rect.sides[0],
            new_geom: rotated,
        },
        |e| *e == CoreError::ConstraintBlockedByFixed(horizontal),
    );
    // 拘束に関わらない変更（スタイル）は通る。
    doc.apply(Command::SetEntityStyle {
        id: rect.sides[0],
        style: Style {
            linetype: Some(crate::Linetype::Dashed),
            ..Style::inherited()
        },
    })
    .unwrap();
}

#[test]
fn batch_translating_connected_shapes_is_accepted() {
    let mut doc = Document::new();
    let rect = rectangle(&mut doc);
    let before: Vec<EntityGeom> = rect.sides.iter().map(|&s| geom(&doc, s)).collect();
    let undo_depth = doc.undo_stack.len();

    // 辺を 1 本ずつ動かす Batch は、全体として矩形を保てば、何も解かずにそのまま通る
    // （サブコマンドごとには解かない）。
    let batch = Command::Batch(
        rect.sides
            .iter()
            .map(|&s| translated(&doc, s, (12.5, -7.25)))
            .collect(),
    );
    assert_eq!(doc.apply(batch).unwrap(), NewIds::default());
    assert_eq!(doc.undo_stack.len(), undo_depth + 1);
    // 追従する修正は足されない（履歴の記録は 4 つのサブコマンドだけ）。
    assert!(matches!(
        &doc.undo_stack.last().unwrap().applied,
        Applied::Batch(subs) if subs.len() == 4
    ));
    for (&s, b) in rect.sides.iter().zip(&before) {
        assert_eq!(geom(&doc, s), b.translated(Vec2::new(12.5, -7.25)));
    }
    assert_eq!(doc.constraint_count(), 8);
    assert_all_satisfied(&doc);
    // undo 1 回で全体が戻る。
    assert!(doc.undo());
    for (&s, b) in rect.sides.iter().zip(&before) {
        assert_eq!(&geom(&doc, s), b);
    }

    // 入れ子の Batch でも同じ（検査は最も外側で 1 回）。
    let nested = Command::Batch(vec![
        Command::Batch(vec![
            translated(&doc, rect.sides[0], (1.0, 1.0)),
            translated(&doc, rect.sides[1], (1.0, 1.0)),
        ]),
        translated(&doc, rect.sides[2], (1.0, 1.0)),
        translated(&doc, rect.sides[3], (1.0, 1.0)),
    ]);
    doc.apply(nested).unwrap();

    // 3 辺だけ動かす Batch では、残りの 1 辺が追従する（96 では拒否だった）。
    let moved: Vec<EntityGeom> = rect.sides[..3]
        .iter()
        .map(|&s| geom(&doc, s).translated(Vec2::new(5.0, 0.0)))
        .collect();
    let follower = geom(&doc, rect.sides[3]).translated(Vec2::new(5.0, 0.0));
    let partial = Command::Batch(
        rect.sides[..3]
            .iter()
            .map(|&s| translated(&doc, s, (5.0, 0.0)))
            .collect(),
    );
    doc.apply(partial).unwrap();
    for (&s, m) in rect.sides[..3].iter().zip(&moved) {
        assert_eq!(&geom(&doc, s), m);
    }
    assert_close(&geom(&doc, rect.sides[3]), &follower, 1e-7);
    assert_all_satisfied(&doc);
}

#[test]
fn batch_rotating_connected_shapes_is_accepted() {
    // 向きに依存しない拘束（一致・直交・平行・距離・接線・半径）でつないだ形は、
    // まとめて回転しても満たされたまま。
    let mut doc = Document::new();
    let a = line(&mut doc, (0.0, 0.0), (40.0, 0.0));
    let b = line(&mut doc, (40.0, 0.0), (40.0, 30.0));
    let c = line(&mut doc, (0.0, 30.0), (40.0, 30.0));
    let round = circle(&mut doc, (20.0, 10.0), 10.0);
    for constraint in [
        Constraint::Coincident {
            a: end(a),
            b: start(b),
        },
        Constraint::Perpendicular { a, b },
        Constraint::Parallel { a, b: c },
        Constraint::EqualLength { a, b: c },
        Constraint::Distance {
            a: start(b),
            b: end(b),
            value: 30.0,
        },
    ] {
        doc.constrain(constraint);
    }
    let tangent = Constraint::tangent_line_circle(&doc, a, round).unwrap();
    doc.constrain(tangent);
    doc.constrain(Constraint::radius(&doc, round).unwrap());
    doc.constrain(Constraint::angle(&doc, a, b).unwrap());

    let pivot = Point2::new(-13.0, 57.0);
    let all = [a, b, c, round];
    let rotate_all = |doc: &Document, angle: f64| {
        Command::Batch(
            all.iter()
                .map(|&id| Command::ModifyEntity {
                    id,
                    new_geom: geom(doc, id).rotated(pivot, angle),
                })
                .collect(),
        )
    };
    // ±π をまたぐ回転を含め、何度回しても通り、何も余計に動かない（bit 単位で回した形のまま）。
    for angle in [0.3, 2.9, 0.4, -4.0] {
        let expected: Vec<EntityGeom> = all
            .iter()
            .map(|&id| geom(&doc, id).rotated(pivot, angle))
            .collect();
        let cmd = rotate_all(&doc, angle);
        doc.apply(cmd).unwrap();
        let actual: Vec<EntityGeom> = all.iter().map(|&id| geom(&doc, id)).collect();
        assert_eq!(actual, expected, "angle {angle}");
    }
    assert_all_satisfied(&doc);
    // 1 本だけ回すと、ほかの図形が追従する（96 では拒否だった）。回した線分は動かされない。
    let rotated = geom(&doc, a).rotated(pivot, 0.3);
    doc.apply(Command::ModifyEntity {
        id: a,
        new_geom: rotated.clone(),
    })
    .unwrap();
    assert_eq!(geom(&doc, a), rotated);
    assert_all_satisfied(&doc);
}

// ---------------------------------------------------------------------------
// RemoveEntity の連鎖削除
// ---------------------------------------------------------------------------

#[test]
fn remove_entity_cascades_and_undo_restores_same_ids() {
    let mut doc = Document::new();
    let rect = rectangle(&mut doc);
    let all: Vec<(ConstraintId, Constraint)> =
        doc.constraints().map(|(id, c)| (id, c.clone())).collect();
    let side0: Vec<ConstraintId> = doc
        .constraints_of(rect.sides[0])
        .map(|(id, _)| id)
        .collect();
    assert_eq!(side0.len(), 3);

    let ids = doc.apply(Command::RemoveEntity(rect.sides[0])).unwrap();
    // 消えた拘束は NewIds に載らない（件数は constraint_count の差で分かる）。
    assert_eq!(ids, NewIds::default());
    assert_eq!(doc.constraint_count(), 5);
    assert_eq!(doc.entity(rect.sides[0]), None);
    for id in &side0 {
        assert_eq!(doc.constraint(*id), None);
    }
    assert!(doc.constraints().all(|(_, c)| !c.references(rect.sides[0])));

    assert!(doc.undo());
    assert_eq!(doc.constraint_count(), 8);
    let restored: Vec<(ConstraintId, Constraint)> =
        doc.constraints().map(|(id, c)| (id, c.clone())).collect();
    assert_eq!(restored, all);

    assert!(doc.redo());
    assert_eq!(doc.constraint_count(), 5);
    assert_eq!(doc.entity(rect.sides[0]), None);
    assert!(doc.undo());
    assert_eq!(doc.constraint_count(), 8);

    // 残った辺は、角の一致が消えたので単独で動かせる。
    doc.apply(Command::RemoveEntity(rect.sides[0])).unwrap();
    doc.apply(Command::RemoveEntity(rect.sides[2])).unwrap();
    assert_eq!(doc.constraint_count(), 2);
    let cmd = translated(&doc, rect.sides[1], (3.0, 4.0));
    doc.apply(cmd).unwrap();
}

#[test]
fn batch_failure_restores_cascaded_constraints() {
    let mut doc = Document::new();
    let rect = rectangle(&mut doc);

    // サブコマンドの失敗（存在しないエンティティ）: 先に連鎖削除した拘束も戻る。
    let ghost = {
        let id = line(&mut doc, (0.0, 0.0), (1.0, 1.0));
        doc.apply(Command::RemoveEntity(id)).unwrap();
        id
    };
    assert_rejected(
        &mut doc,
        Command::Batch(vec![
            Command::RemoveEntity(rect.sides[0]),
            Command::RemoveEntity(rect.sides[1]),
            Command::RemoveEntity(ghost),
        ]),
        |e| *e == CoreError::EntityNotFound(ghost),
    );

    // 最後の求解の失敗: 辺 0 を消した後、辺 2（水平）の両端を動かして傾ける。
    assert_rejected(
        &mut doc,
        Command::Batch(vec![
            Command::RemoveEntity(rect.sides[0]),
            Command::ModifyEntity {
                id: rect.sides[2],
                new_geom: line_geom((41.0, 30.0), (0.0, 31.0)),
            },
        ]),
        blocked,
    );

    // 同じ Batch の中で追加した拘束が、後続の RemoveEntity で消える場合。追加した拘束は
    // 求解の対象にならず、undo で全体が戻る。
    let before = snapshot(&doc);
    let ids = doc
        .apply(Command::Batch(vec![
            add_constraint(Constraint::EqualLength {
                a: rect.sides[0],
                b: rect.sides[2],
            }),
            Command::RemoveEntity(rect.sides[0]),
        ]))
        .unwrap();
    assert_eq!(ids.constraints.len(), 1);
    assert_eq!(doc.constraint(ids.constraints[0]), None);
    assert_eq!(doc.constraint_count(), 5);
    assert!(doc.undo());
    assert_eq!(snapshot(&doc).constraints, before.constraints);
    assert_eq!(snapshot(&doc).entities, before.entities);
}

// ---------------------------------------------------------------------------
// SetConstraintValue
// ---------------------------------------------------------------------------

#[test]
fn set_constraint_value_noop_and_rejections() {
    let mut doc = Document::new();
    let l = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    let c = circle(&mut doc, (5.0, 4.0), 4.0);
    let length = doc.constrain(Constraint::distance(&doc, start(l), end(l)).unwrap());
    let horizontal = doc.constrain(Constraint::Horizontal { line: l });
    let radius = doc.constrain(Constraint::radius(&doc, c).unwrap());
    let offset = doc.constrain(Constraint::point_line_distance(&doc, center(c), l).unwrap());
    assert_eq!(doc.constraint(offset).unwrap().value(), Some(4.0));

    // 同じ値は意味的 no-op（履歴・世代に触れない）。
    let before = snapshot(&doc);
    assert_eq!(
        doc.apply(Command::SetConstraintValue {
            id: length,
            value: 10.0
        }),
        Ok(NewIds::default())
    );
    assert_eq!(snapshot(&doc), before);

    let set = |id, value| Command::SetConstraintValue { id, value };
    // 形を変える値は、解かれて形が変わる（96 では拒否だった）。undo 1 回で値と形が戻る。
    let shapes = (geom(&doc, l), geom(&doc, c));
    for (id, value, old) in [(length, 12.0, 10.0), (radius, 5.0, 4.0), (offset, 0.0, 4.0)] {
        doc.apply(set(id, value)).unwrap();
        assert_eq!(doc.constraint(id).unwrap().value(), Some(value));
        assert_ne!((geom(&doc, l), geom(&doc, c)), shapes);
        assert_all_satisfied(&doc);
        assert!(doc.undo());
        assert_eq!(doc.constraint(id).unwrap().value(), Some(old));
        assert_eq!((geom(&doc, l), geom(&doc, c)), shapes);
    }
    // 範囲外の値は構造の検証で拒否（点どうしの距離 0 は一致拘束を使う）。
    assert_rejected(&mut doc, set(length, 0.0), invalid);
    assert_rejected(&mut doc, set(length, 1e-13), invalid);
    assert_rejected(&mut doc, set(length, -3.0), invalid);
    assert_rejected(&mut doc, set(length, f64::NAN), invalid);
    assert_rejected(&mut doc, set(radius, 0.0), invalid);
    assert_rejected(&mut doc, set(radius, f64::INFINITY), invalid);
    assert_rejected(&mut doc, set(offset, -1.0), invalid);
    // 値を持たない拘束・存在しない拘束。
    assert_rejected(&mut doc, set(horizontal, 1.0), invalid);
    doc.apply(Command::RemoveConstraint(horizontal)).unwrap();
    assert_rejected(&mut doc, set(horizontal, 1.0), |e| {
        *e == CoreError::ConstraintNotFound(horizontal)
    });
}

#[test]
fn set_constraint_value_together_with_matching_geometry() {
    // 形と値を同じ Batch で変えれば、解かずにそのまま通る（求解は Batch の最後に 1 回）。
    let mut doc = Document::new();
    let l = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    let length = doc.constrain(Constraint::distance(&doc, start(l), end(l)).unwrap());
    let batch = |order_swapped: bool| {
        let mut subs = vec![
            Command::ModifyEntity {
                id: l,
                new_geom: line_geom((0.0, 0.0), (12.0, 0.0)),
            },
            Command::SetConstraintValue {
                id: length,
                value: 12.0,
            },
        ];
        if order_swapped {
            subs.reverse();
        }
        Command::Batch(subs)
    };
    for order_swapped in [false, true] {
        doc.apply(batch(order_swapped)).unwrap();
        assert_eq!(doc.constraint(length).unwrap().value(), Some(12.0));
        assert_eq!(geom(&doc, l), line_geom((0.0, 0.0), (12.0, 0.0)));
        assert!(doc.undo());
        assert_eq!(doc.constraint(length).unwrap().value(), Some(10.0));
        assert_eq!(geom(&doc, l), line_geom((0.0, 0.0), (10.0, 0.0)));
        assert!(doc.redo());
        assert_eq!(doc.constraint(length).unwrap().value(), Some(12.0));
        assert!(doc.undo());
    }
}

#[test]
fn angle_values_are_stored_normalized() {
    let mut doc = Document::new();
    let a = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    let b = line(&mut doc, (0.0, 0.0), (-5.0, 5.0));
    // 3π/4 を 1 周ずらして渡しても、(−π, π] に正規化して格納される。
    let id = doc.constrain(Constraint::Angle {
        a,
        b,
        value: 0.75 * PI - 2.0 * PI,
    });
    let stored = doc.constraint(id).unwrap().value().unwrap();
    assert!((stored - 0.75 * PI).abs() < 1e-12, "{stored}");
    // 範囲内の値はそのまま（丸めを持ち込まない）ので、同じ値の再設定は no-op。
    let exact = Constraint::angle(&doc, a, b).unwrap().value().unwrap();
    doc.apply(Command::SetConstraintValue { id, value: exact })
        .unwrap();
    let before = snapshot(&doc);
    doc.apply(Command::SetConstraintValue { id, value: exact })
        .unwrap();
    assert_eq!(snapshot(&doc), before);
    // −π は π として格納される。
    let c = line(&mut doc, (0.0, 3.0), (-10.0, 3.0));
    let id = doc.constrain(Constraint::Angle {
        a,
        b: c,
        value: -PI,
    });
    assert_eq!(doc.constraint(id).unwrap().value(), Some(PI));
}

// ---------------------------------------------------------------------------
// 構造の検証
// ---------------------------------------------------------------------------

#[test]
fn structural_validation_rejects_bad_targets() {
    let mut doc = Document::new();
    let l = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    let l2 = line(&mut doc, (0.0, 5.0), (10.0, 5.0));
    let c = circle(&mut doc, (5.0, 2.0), 2.0);
    let a = add(&mut doc, arc_geom((5.0, 2.0), 2.0, 0.0, PI));
    let polyline = add(
        &mut doc,
        EntityGeom::Shape(Shape::Polyline(Polyline::new(
            vec![Point2::new(0.0, 0.0), Point2::new(10.0, 0.0)],
            false,
        ))),
    );
    let point = add(&mut doc, EntityGeom::Shape(Shape::Point(Point2::ORIGIN)));
    let text = add(
        &mut doc,
        EntityGeom::Text(TextGeom {
            anchor: Point2::ORIGIN,
            height: 3.5,
            angle: 0.0,
            content: "A".to_owned(),
        }),
    );
    let removed = {
        let id = line(&mut doc, (0.0, 0.0), (1.0, 0.0));
        doc.apply(Command::RemoveEntity(id)).unwrap();
        id
    };

    // 参照先が無い。
    assert_rejected(
        &mut doc,
        add_constraint(Constraint::Horizontal { line: removed }),
        |e| *e == CoreError::EntityNotFound(removed),
    );
    assert_rejected(
        &mut doc,
        add_constraint(Constraint::Parallel { a: l, b: removed }),
        |e| *e == CoreError::EntityNotFound(removed),
    );

    let bad = [
        // 種類が合わない。
        Constraint::Horizontal { line: c },
        Constraint::Vertical { line: a },
        Constraint::Parallel { a: l, b: c },
        Constraint::Perpendicular { a, b: l },
        Constraint::EqualLength { a: l, b: polyline },
        Constraint::Angle {
            a: l,
            b: c,
            value: 0.0,
        },
        Constraint::EqualRadius { a: c, b: l },
        Constraint::Radius {
            entity: l,
            value: 1.0,
        },
        Constraint::TangentLineCircle {
            line: c,
            circle: a,
            side: Side::Left,
        },
        Constraint::TangentLineCircle {
            line: l,
            circle: l2,
            side: Side::Left,
        },
        Constraint::TangentCircles {
            a: c,
            b: l,
            kind: CircleTangency::External,
        },
        Constraint::FixEntity { entity: polyline },
        Constraint::FixEntity { entity: point },
        Constraint::FixEntity { entity: text },
        // PointKind と図形の組み合わせ。
        Constraint::Coincident {
            a: start(c),
            b: end(l),
        },
        Constraint::Coincident {
            a: end(c),
            b: end(l),
        },
        Constraint::Coincident {
            a: center(l),
            b: center(c),
        },
        Constraint::FixPoint {
            point: center(l),
            at: Point2::ORIGIN,
        },
        Constraint::FixPoint {
            point: start(polyline),
            at: Point2::ORIGIN,
        },
        Constraint::Distance {
            a: start(point),
            b: end(l),
            value: 1.0,
        },
        Constraint::PointLineDistance {
            point: center(c),
            line: a,
            side: Side::Left,
            value: 2.0,
        },
        // 同じ対象を 2 回指す。
        Constraint::Coincident {
            a: end(l),
            b: end(l),
        },
        Constraint::Distance {
            a: center(c),
            b: center(c),
            value: 1.0,
        },
        Constraint::Parallel { a: l, b: l },
        Constraint::Perpendicular { a: l, b: l },
        Constraint::EqualLength { a: l, b: l },
        Constraint::Angle {
            a: l,
            b: l,
            value: 0.0,
        },
        Constraint::EqualRadius { a: c, b: c },
        Constraint::TangentCircles {
            a,
            b: a,
            kind: CircleTangency::External,
        },
        Constraint::PointLineDistance {
            point: end(l),
            line: l,
            side: Side::Left,
            value: 0.0,
        },
        // 値。
        Constraint::Distance {
            a: start(l),
            b: end(l),
            value: 0.0,
        },
        Constraint::Distance {
            a: start(l),
            b: end(l),
            value: -10.0,
        },
        Constraint::Distance {
            a: start(l),
            b: end(l),
            value: f64::NAN,
        },
        Constraint::PointLineDistance {
            point: center(c),
            line: l,
            side: Side::Left,
            value: -2.0,
        },
        Constraint::PointLineDistance {
            point: center(c),
            line: l,
            side: Side::Left,
            value: f64::INFINITY,
        },
        Constraint::Radius {
            entity: c,
            value: 0.0,
        },
        Constraint::Radius {
            entity: c,
            value: -2.0,
        },
        Constraint::Radius {
            entity: c,
            value: f64::NAN,
        },
        Constraint::Angle {
            a: l,
            b: l2,
            value: f64::NAN,
        },
        Constraint::FixPoint {
            point: center(c),
            at: Point2::new(f64::NAN, 2.0),
        },
        Constraint::FixPoint {
            point: center(c),
            at: Point2::new(5.0, f64::INFINITY),
        },
        // 等半径の内接・大小の食い違い（c と a は同じ半径）。
        Constraint::TangentCircles {
            a: c,
            b: a,
            kind: CircleTangency::Internal(LargerCircle::First),
        },
        Constraint::TangentCircles {
            a: c,
            b: a,
            kind: CircleTangency::Internal(LargerCircle::Second),
        },
    ];
    for constraint in bad {
        let shown = format!("{constraint:?}");
        let before = snapshot(&doc);
        let err = doc.apply(add_constraint(constraint)).expect_err(&shown);
        assert!(invalid(&err), "{shown}: {err:?}");
        assert_eq!(snapshot(&doc), before, "{shown}");
    }
    assert_eq!(doc.constraint_count(), 0);

    // 同じ線分の始点・終点の距離（線分の長さ）、同じ円弧の端点どうしは許す。
    doc.constrain(Constraint::Distance {
        a: start(l),
        b: end(l),
        value: 10.0,
    });
    doc.constrain(Constraint::distance(&doc, start(a), end(a)).unwrap());
    doc.constrain(Constraint::distance(&doc, start(a), center(a)).unwrap());
}

#[test]
fn constraints_ignore_layer_lock() {
    // 拘束の追加・削除・値の変更はエンティティを変えないので、ロックを見ない。
    let mut doc = Document::new();
    let l = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    let layer = doc.current_layer();
    let mut props = doc.layer(layer).unwrap().clone();
    props.locked = true;
    doc.apply(Command::SetLayerProps { id: layer, props })
        .unwrap();
    let horizontal = doc.constrain(Constraint::Horizontal { line: l });
    let length = doc.constrain(Constraint::distance(&doc, start(l), end(l)).unwrap());
    doc.apply(Command::SetConstraintValue {
        id: length,
        value: 10.0 + 1e-10,
    })
    .unwrap();
    doc.apply(Command::RemoveConstraint(horizontal)).unwrap();
    // エンティティの変更は従来どおりロックで拒否される。
    assert_rejected(&mut doc, Command::RemoveEntity(l), |e| {
        *e == CoreError::LayerLocked(layer)
    });
}

// ---------------------------------------------------------------------------
// 固定された図形
// ---------------------------------------------------------------------------

#[test]
fn fixed_entity_cannot_change_geometry() {
    let mut doc = Document::new();
    let l = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    let other = line(&mut doc, (10.0, 0.0), (10.0, 5.0));
    let fix = doc.constrain(Constraint::FixEntity { entity: l });
    doc.constrain(Constraint::Coincident {
        a: end(l),
        b: start(other),
    });
    let fixed = |e: &CoreError| *e == CoreError::EntityFixed(l);

    let moved = line_geom((0.0, 1.0), (10.0, 1.0));
    assert_rejected(
        &mut doc,
        Command::ModifyEntity {
            id: l,
            new_geom: moved.clone(),
        },
        fixed,
    );
    assert_rejected(
        &mut doc,
        Command::ReshapeEntity {
            id: l,
            new_geom: line_geom((0.0, 0.0), (7.0, 0.0)),
        },
        fixed,
    );
    // Batch の中でも、固定がついている時点の変更は拒否。
    let batch = Command::Batch(vec![
        translated(&doc, other, (0.0, 1.0)),
        Command::ModifyEntity {
            id: l,
            new_geom: moved.clone(),
        },
    ]);
    assert_rejected(&mut doc, batch, fixed);

    // 同一幾何への差し替えは no-op のまま（エラーにしない）。
    let before = snapshot(&doc);
    let same = geom(&doc, l);
    assert_eq!(
        doc.apply(Command::ModifyEntity {
            id: l,
            new_geom: same.clone()
        }),
        Ok(NewIds::default())
    );
    assert_eq!(
        doc.apply(Command::ReshapeEntity {
            id: l,
            new_geom: same
        }),
        Ok(NewIds::default())
    );
    assert_eq!(snapshot(&doc), before);

    // 形を変えない変更（スタイル）は通る。固定した図形と一致でつながった図形をまるごと
    // 動かすと、一致の両側が止められるので拒否（エラーは求解のもの）。
    doc.apply(Command::SetEntityStyle {
        id: l,
        style: Style {
            linetype: Some(crate::Linetype::Dashed),
            ..Style::inherited()
        },
    })
    .unwrap();
    let cmd = translated(&doc, other, (1.0, 0.0));
    assert_rejected(&mut doc, cmd, blocked);
    // 一致していない側の端点だけを動かすのは通る（固定した図形は動かない）。
    doc.apply(Command::ModifyEntity {
        id: other,
        new_geom: line_geom((10.0, 0.0), (12.0, 7.0)),
    })
    .unwrap();
    assert_eq!(geom(&doc, l), line_geom((0.0, 0.0), (10.0, 0.0)));
    assert!(doc.undo());

    // 固定を外してから動かす Batch は通る（判定はその時点の状態）。
    doc.apply(Command::Batch(vec![
        Command::RemoveConstraint(fix),
        Command::ModifyEntity {
            id: l,
            new_geom: moved.clone(),
        },
        translated(&doc, other, (0.0, 1.0)),
    ]))
    .unwrap();
    assert_eq!(geom(&doc, l), moved);
    assert!(doc.undo());
    assert_eq!(
        doc.constraint(fix),
        Some(&Constraint::FixEntity { entity: l })
    );

    // 固定した図形の削除は通り、拘束も一緒に消える。
    doc.apply(Command::RemoveEntity(l)).unwrap();
    assert_eq!(doc.constraint_count(), 0);
    assert!(doc.undo());
    assert_eq!(doc.constraint_count(), 2);
}

#[test]
fn fixed_point_blocks_moving_that_point() {
    let mut doc = Document::new();
    let l = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    let fix = doc.constrain(Constraint::fix_point(&doc, start(l)).unwrap());
    // 固定した点を動かす変更は、点の固定の方程式が満たされず、動かした点は止められるので拒否。
    assert_rejected(
        &mut doc,
        Command::ModifyEntity {
            id: l,
            new_geom: line_geom((1.0, 0.0), (10.0, 0.0)),
        },
        |e| *e == CoreError::ConstraintBlockedByFixed(fix),
    );
    // 固定した点を保つ変更（もう一方の端点を動かす）は通る。
    doc.apply(Command::ModifyEntity {
        id: l,
        new_geom: line_geom((0.0, 0.0), (3.0, 8.0)),
    })
    .unwrap();
}

#[test]
fn modify_changing_kind_is_rejected_when_constraints_cannot_refer() {
    let mut doc = Document::new();
    let l = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    let c = circle(&mut doc, (5.0, 3.0), 3.0);
    doc.constrain(Constraint::Horizontal { line: l });
    doc.constrain(Constraint::radius(&doc, c).unwrap());
    doc.constrain(Constraint::tangent_line_circle(&doc, l, c).unwrap());

    // 線分 → 円・ポリライン: 水平と接線が指せなくなる。
    assert_rejected(
        &mut doc,
        Command::ModifyEntity {
            id: l,
            new_geom: circle_geom((0.0, 0.0), 1.0),
        },
        invalid,
    );
    assert_rejected(
        &mut doc,
        Command::ModifyEntity {
            id: l,
            new_geom: EntityGeom::Shape(Shape::Polyline(Polyline::new(
                vec![Point2::new(0.0, 0.0), Point2::new(10.0, 0.0)],
                false,
            ))),
        },
        invalid,
    );
    // 円 → 線分: 半径と接線（円の側）が指せなくなる。
    assert_rejected(
        &mut doc,
        Command::ModifyEntity {
            id: c,
            new_geom: line_geom((0.0, 0.0), (1.0, 0.0)),
        },
        invalid,
    );
    // 円 → 円弧は、半径・接線が指せるままなので通る（形が拘束を満たす限り）。
    doc.apply(Command::ModifyEntity {
        id: c,
        new_geom: arc_geom((5.0, 3.0), 3.0, 0.0, PI),
    })
    .unwrap();
    // 円弧の端点を指す拘束を付けた後は、円へ戻せない。
    let other = line(&mut doc, (8.0, 3.0), (8.0, 9.0));
    doc.constrain(Constraint::Coincident {
        a: start(c),
        b: start(other),
    });
    assert_rejected(
        &mut doc,
        Command::ModifyEntity {
            id: c,
            new_geom: circle_geom((5.0, 3.0), 3.0),
        },
        invalid,
    );
}

// ---------------------------------------------------------------------------
// ReshapeEntity（完了条件 7）
// ---------------------------------------------------------------------------

/// 切り詰める線分 `l` と、それに付けた拘束。
struct ReshapeScene {
    l: EntityId,
    /// 切り詰めで消えるはずの拘束（終点の一致・終点の距離・等長）。
    removed: Vec<ConstraintId>,
    /// 残るはずの拘束（水平・平行・接線・半径・始点の距離）。
    kept: Vec<ConstraintId>,
}

fn reshape_scene(doc: &mut Document) -> ReshapeScene {
    let l = line(doc, (0.0, 0.0), (10.0, 0.0));
    let corner = line(doc, (10.0, 0.0), (10.0, 5.0));
    let twin = line(doc, (0.0, 3.0), (10.0, 3.0));
    let round = circle(doc, (5.0, -2.0), 2.0);
    let removed = vec![
        doc.constrain(Constraint::Coincident {
            a: end(l),
            b: start(corner),
        }),
        doc.constrain(Constraint::distance(doc, end(l), end(twin)).unwrap()),
        doc.constrain(Constraint::EqualLength { a: l, b: twin }),
    ];
    let kept = vec![
        doc.constrain(Constraint::Horizontal { line: l }),
        doc.constrain(Constraint::Parallel { a: l, b: twin }),
        doc.constrain(Constraint::tangent_line_circle(doc, l, round).unwrap()),
        doc.constrain(Constraint::radius(doc, round).unwrap()),
        doc.constrain(Constraint::distance(doc, start(l), start(twin)).unwrap()),
    ];
    ReshapeScene { l, removed, kept }
}

#[test]
fn reshape_removes_moved_point_constraints_and_keeps_the_rest() {
    let mut doc = Document::new();
    let scene = reshape_scene(&mut doc);
    assert_eq!(doc.constraint_count(), 8);
    let all: Vec<(ConstraintId, Constraint)> =
        doc.constraints().map(|(id, c)| (id, c.clone())).collect();
    let original = geom(&doc, scene.l);
    let shorter = line_geom((0.0, 0.0), (7.0, 0.0));

    // 同じ変更を ModifyEntity で行うと「動かした」扱いで、拘束は 1 件も消えず、ほかの図形が
    // 追従する（96 では終点の一致に反して拒否だった）。
    let everything = snapshot(&doc).entities;
    doc.apply(Command::ModifyEntity {
        id: scene.l,
        new_geom: shorter.clone(),
    })
    .unwrap();
    // 止められるのは動かした終点だけ（始点は自由なので、最小ノルムの解では始点も少し動く）。
    assert_eq!(
        end(scene.l).position(&doc),
        Some(Point2::new(7.0, 0.0)),
        "{:?}",
        geom(&doc, scene.l)
    );
    assert_eq!(doc.constraint_count(), 8);
    assert_all_satisfied(&doc);
    assert!(doc.undo());
    assert_eq!(snapshot(&doc).entities, everything);

    let ids = doc
        .apply(Command::ReshapeEntity {
            id: scene.l,
            new_geom: shorter.clone(),
        })
        .unwrap();
    assert_eq!(ids, NewIds::default());
    assert_eq!(geom(&doc, scene.l), shorter);
    // 動いた端点の一致・点の距離と等長が消え、水平・平行・接線・半径・動いていない点の
    // 距離が残る。消えた件数は constraint_count の差で分かる。
    assert_eq!(doc.constraint_count(), 5);
    for id in &scene.removed {
        assert_eq!(doc.constraint(*id), None);
    }
    let kept: Vec<ConstraintId> = doc.constraints().map(|(id, _)| id).collect();
    assert_eq!(kept, scene.kept);

    // undo 1 回で図形と拘束がそろって戻り、redo で再び消える（ID は同じ）。
    assert!(doc.undo());
    assert_eq!(geom(&doc, scene.l), original);
    let restored: Vec<(ConstraintId, Constraint)> =
        doc.constraints().map(|(id, c)| (id, c.clone())).collect();
    assert_eq!(restored, all);
    assert!(doc.redo());
    assert_eq!(geom(&doc, scene.l), shorter);
    assert_eq!(doc.constraint_count(), 5);
    assert!(doc.undo());

    // 延長でも同じ規則。
    doc.apply(Command::ReshapeEntity {
        id: scene.l,
        new_geom: line_geom((0.0, 0.0), (25.0, 0.0)),
    })
    .unwrap();
    assert_eq!(doc.constraint_count(), 5);
    assert!(doc.undo());

    // 同一幾何への切り詰めは no-op で、拘束も消さない。
    let before = snapshot(&doc);
    doc.apply(Command::ReshapeEntity {
        id: scene.l,
        new_geom: original,
    })
    .unwrap();
    assert_eq!(snapshot(&doc), before);
}

#[test]
fn reshape_moving_a_constrained_entity_is_rejected_and_removes_nothing() {
    // ReshapeEntity は切り詰め・延長だけ。拘束のついた図形を動かす使い方は、端点の拘束を
    // 消さずに丸ごと拒否する（消してしまうと、水平・平行だけが残って検査を通る）。
    let mut doc = Document::new();
    let scene = reshape_scene(&mut doc);
    assert_eq!(doc.constraint_count(), 8);
    let original = geom(&doc, scene.l);
    let moved = [
        ("translate", original.translated(Vec2::new(0.0, 4.0))),
        (
            "translate along and across",
            original.translated(Vec2::new(3.0, 1.0)),
        ),
        ("rotate", original.rotated(Point2::new(0.0, 0.0), 0.3)),
        (
            "rotate half turn",
            original.rotated(Point2::new(5.0, 0.0), PI),
        ),
        ("reverse", line_geom((10.0, 0.0), (0.0, 0.0))),
        ("reverse and trim", line_geom((7.0, 0.0), (0.0, 0.0))),
        ("to a circle", circle_geom((5.0, 0.0), 5.0)),
    ];
    for (what, new_geom) in moved {
        let before = snapshot(&doc);
        let err = doc
            .apply(Command::ReshapeEntity {
                id: scene.l,
                new_geom,
            })
            .expect_err(what);
        assert!(invalid(&err), "{what}: {err:?}");
        let CoreError::InvalidConstraint(message) = &err else {
            unreachable!()
        };
        assert!(message.contains("ModifyEntity"), "{what}: {message}");
        assert_eq!(snapshot(&doc), before, "{what}");
        assert_eq!(doc.constraint_count(), 8, "{what}");
    }
    // 同じ直線上の切り詰め・延長は従来どおり通る。
    for new_geom in [
        line_geom((0.0, 0.0), (7.0, 0.0)),
        line_geom((0.0, 0.0), (25.0, 0.0)),
        line_geom((-3.0, 0.0), (4.0, 0.0)),
    ] {
        doc.apply(Command::ReshapeEntity {
            id: scene.l,
            new_geom,
        })
        .unwrap();
        assert!(doc.constraint_count() < 8);
        assert!(doc.undo());
        assert_eq!(doc.constraint_count(), 8);
    }

    // 円弧: 中心の移動・半径の変更・円へ戻す変更は拒否、開始角・終了角の変更は通る。
    let arc_id = add(&mut doc, arc_geom((50.0, 50.0), 5.0, 0.3, 2.0));
    doc.constrain(Constraint::radius(&doc, arc_id).unwrap());
    doc.constrain(Constraint::fix_point(&doc, center(arc_id)).unwrap());
    for (what, new_geom) in [
        ("move center", arc_geom((50.0, 51.0), 5.0, 0.3, 2.0)),
        ("change radius", arc_geom((50.0, 50.0), 6.0, 0.3, 2.0)),
        ("back to a circle", circle_geom((50.0, 50.0), 5.0)),
    ] {
        let before = snapshot(&doc);
        let err = doc
            .apply(Command::ReshapeEntity {
                id: arc_id,
                new_geom,
            })
            .expect_err(what);
        assert!(invalid(&err), "{what}: {err:?}");
        assert_eq!(snapshot(&doc), before, "{what}");
    }
    doc.apply(Command::ReshapeEntity {
        id: arc_id,
        new_geom: arc_geom((50.0, 50.0), 5.0, 1.0, 1.5),
    })
    .unwrap();
    assert_eq!(doc.constraint_count(), 10);
}

#[test]
fn reshape_of_unconstrained_entity_accepts_any_change() {
    // 拘束のある文書でも、対象を参照する拘束が無ければ ReshapeEntity は検証なし
    // （ModifyEntity と同じ）。
    let mut doc = Document::new();
    let scene = reshape_scene(&mut doc);
    let free = line(&mut doc, (0.0, 20.0), (10.0, 20.0));
    for new_geom in [
        line_geom((0.0, 25.0), (10.0, 25.0)),
        line_geom((10.0, 25.0), (0.0, 25.0)),
        line_geom((1.0, 2.0), (3.0, 9.0)),
        line_geom((4.0, 4.0), (4.0, 4.0)),
        circle_geom((0.0, 20.0), 3.0),
        arc_geom((1.0, 20.0), 2.0, 0.0, 1.0),
    ] {
        doc.apply(Command::ReshapeEntity {
            id: free,
            new_geom: new_geom.clone(),
        })
        .unwrap();
        assert_eq!(geom(&doc, free), new_geom);
        assert_eq!(doc.constraint_count(), 8);
    }
    // 拘束をすべて外した図形も同じ。
    let ids: Vec<ConstraintId> = doc.constraints_of(scene.l).map(|(id, _)| id).collect();
    doc.apply(Command::Batch(
        ids.into_iter().map(Command::RemoveConstraint).collect(),
    ))
    .unwrap();
    doc.apply(Command::ReshapeEntity {
        id: scene.l,
        new_geom: line_geom((0.0, -9.0), (-10.0, -9.0)),
    })
    .unwrap();
}

#[test]
fn reshape_solves_the_remaining_constraints() {
    // 支持する直線の判定（許容値は変更前後の大きい方の長さで決まる）は通るが、残った拘束が
    // 変更後の形（短くなって許容値が厳しくなる）では満たされない場合。切り詰めた終点を
    // 止めたまま、残った拘束を解く（96 では拒否だった）。
    let mut doc = Document::new();
    let long = line(&mut doc, (0.0, 0.0), (1000.0, 0.0));
    let post = line(&mut doc, (1000.0, 0.0), (1000.0, 5.0));
    let horizontal = doc.constrain(Constraint::Horizontal { line: long });
    doc.constrain(Constraint::Coincident {
        a: end(long),
        b: start(post),
    });
    // 直線からのずれ 5e-7 は、長さ 1000 の許容値（1e-6）以内だが、長さ 10 の線分では
    // 5e-8 rad の傾きになり、水平の許容値（1e-9 rad）を超える。
    let trimmed = Command::ReshapeEntity {
        id: long,
        new_geom: line_geom((0.0, 0.0), (10.0, 5e-7)),
    };
    doc.apply(trimmed.clone()).unwrap();
    // 終点の一致は消え、水平は始点が追従して保たれる。
    assert_eq!(doc.constraint_count(), 1);
    assert_all_satisfied(&doc);
    let EntityGeom::Shape(Shape::Line(solved)) = geom(&doc, long) else {
        panic!("expected a line");
    };
    assert_eq!(solved.b, Point2::new(10.0, 5e-7));
    // 始点は、止まった終点のまわりに 5e-8 rad 回る（線分と直角に動くので、x も 2.5e-14 動く）。
    assert!(solved.a.x.abs() <= 1e-12, "{solved:?}");
    assert!((solved.a.y - 5e-7).abs() <= 1e-8, "{solved:?}");
    // undo 1 回で、切り詰め・消えた拘束・追従がそろって戻る。
    assert!(doc.undo());
    assert_eq!(geom(&doc, long), line_geom((0.0, 0.0), (1000.0, 0.0)));
    assert_eq!(doc.constraint_count(), 2);

    // 始点が点の固定で止まっていれば追従できず、拒否される。消しかけた拘束も戻る。
    doc.constrain(Constraint::fix_point(&doc, start(long)).unwrap());
    assert_rejected(&mut doc, trimmed, |e| {
        *e == CoreError::ConstraintUnsolvable(horizontal)
    });
    assert_eq!(doc.constraint_count(), 3);

    // Batch の途中で失敗した場合も、先の ReshapeEntity が消した拘束が戻る。
    let scene = reshape_scene(&mut doc);
    let ghost = {
        let id = line(&mut doc, (0.0, 0.0), (1.0, 1.0));
        doc.apply(Command::RemoveEntity(id)).unwrap();
        id
    };
    assert_rejected(
        &mut doc,
        Command::Batch(vec![
            Command::ReshapeEntity {
                id: scene.l,
                new_geom: line_geom((0.0, 0.0), (7.0, 0.0)),
            },
            Command::RemoveEntity(ghost),
        ]),
        |e| *e == CoreError::EntityNotFound(ghost),
    );
}

#[test]
fn reshape_is_rejected_by_fixed_point_and_fixed_entity() {
    let mut doc = Document::new();
    let scene = reshape_scene(&mut doc);
    let fixed = |e: &CoreError| *e == CoreError::EntityFixed(scene.l);
    let shorter_end = line_geom((0.0, 0.0), (7.0, 0.0));
    let shorter_start = line_geom((2.0, 0.0), (10.0, 0.0));

    // 動いた点の固定は拒否。
    let fix_end = doc.constrain(Constraint::fix_point(&doc, end(scene.l)).unwrap());
    assert_rejected(
        &mut doc,
        Command::ReshapeEntity {
            id: scene.l,
            new_geom: shorter_end.clone(),
        },
        fixed,
    );
    // 固定していない側の切り詰めは通り、点の固定は残る。
    doc.apply(Command::ReshapeEntity {
        id: scene.l,
        new_geom: shorter_start.clone(),
    })
    .unwrap();
    assert!(doc.constraint(fix_end).is_some());
    assert!(doc.undo());

    // 図形の固定は、どちら側の切り詰めも拒否。
    doc.apply(Command::RemoveConstraint(fix_end)).unwrap();
    doc.constrain(Constraint::FixEntity { entity: scene.l });
    for new_geom in [shorter_end, shorter_start] {
        assert_rejected(
            &mut doc,
            Command::ReshapeEntity {
                id: scene.l,
                new_geom,
            },
            fixed,
        );
    }
}

#[test]
fn reshape_circle_to_arc_keeps_center_constraints() {
    let mut doc = Document::new();
    let c = circle(&mut doc, (5.0, 3.0), 3.0);
    let spoke = line(&mut doc, (5.0, 3.0), (20.0, 3.0));
    let floor = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    let ids = vec![
        doc.constrain(Constraint::Coincident {
            a: center(c),
            b: start(spoke),
        }),
        doc.constrain(Constraint::fix_point(&doc, center(c)).unwrap()),
        doc.constrain(Constraint::radius(&doc, c).unwrap()),
        doc.constrain(Constraint::tangent_line_circle(&doc, floor, c).unwrap()),
    ];
    // 1 断片のトリムで円が円弧になる。中心・半径・接線の拘束はすべて残る。
    let trimmed = arc_geom((5.0, 3.0), 3.0, 0.5, 2.5);
    doc.apply(Command::ReshapeEntity {
        id: c,
        new_geom: trimmed.clone(),
    })
    .unwrap();
    assert_eq!(geom(&doc, c), trimmed);
    assert_eq!(doc.constraints().map(|(id, _)| id).collect::<Vec<_>>(), ids);
    // 接点（円の下端）は円弧の掃引範囲の外になるが、拘束は満たされたまま。
    let tangent = doc.constraint(ids[3]).unwrap();
    assert_eq!(tangent.is_satisfied(&doc), Ok(true));
    let contact = tangent.tangent_contact(&doc).unwrap();
    assert!(contact.second_outside && !contact.first_outside);

    // 円弧の端点に一致を付けてから、その端点を切り詰めると一致だけが消える。
    let tail = {
        let p = start(c).position(&doc).unwrap();
        line(&mut doc, (p.x, p.y), (p.x + 4.0, p.y))
    };
    let joint = doc.constrain(Constraint::Coincident {
        a: start(c),
        b: start(tail),
    });
    doc.apply(Command::ReshapeEntity {
        id: c,
        new_geom: arc_geom((5.0, 3.0), 3.0, 1.0, 2.5),
    })
    .unwrap();
    assert_eq!(doc.constraint(joint), None);
    assert_eq!(doc.constraint_count(), 4);
}

#[test]
fn reshape_without_constraints_matches_modify() {
    // 拘束の無いエンティティに対しては ModifyEntity と同じ結果になる。
    let run = |reshape: bool| {
        let mut doc = Document::new();
        let l = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
        // 別の図形には拘束がある（拘束のある文書でも、対象に無ければ同じ）。
        let other = line(&mut doc, (0.0, 5.0), (10.0, 5.0));
        doc.constrain(Constraint::Horizontal { line: other });
        let new_geom = line_geom((0.0, 0.0), (4.0, 3.0));
        let cmd = if reshape {
            Command::ReshapeEntity { id: l, new_geom }
        } else {
            Command::ModifyEntity { id: l, new_geom }
        };
        assert_eq!(doc.apply(cmd), Ok(NewIds::default()));
        let applied = snapshot(&doc);
        assert!(doc.undo());
        (applied, snapshot(&doc))
    };
    assert_eq!(run(true), run(false));

    // 検証（不正な幾何・ロック・存在しない ID）も ModifyEntity と同じ。
    let mut doc = Document::new();
    let l = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    assert_rejected(
        &mut doc,
        Command::ReshapeEntity {
            id: l,
            new_geom: line_geom((0.0, 0.0), (f64::NAN, 0.0)),
        },
        |e| matches!(e, CoreError::InvalidGeometry(_)),
    );
    let layer = doc.current_layer();
    let mut props = doc.layer(layer).unwrap().clone();
    props.locked = true;
    doc.apply(Command::SetLayerProps { id: layer, props })
        .unwrap();
    assert_rejected(
        &mut doc,
        Command::ReshapeEntity {
            id: l,
            new_geom: line_geom((0.0, 0.0), (5.0, 0.0)),
        },
        |e| *e == CoreError::LayerLocked(layer),
    );
}

// ---------------------------------------------------------------------------
// 退化した図形（ソルバーが受け入れない形を参照する拘束は拒否）
// ---------------------------------------------------------------------------

/// `mcad_geom::solve::ATOL_LEN`（1e-12）を下回る長さ・半径。
const BELOW_ATOL: f64 = 1e-13;
const _: () = assert!(BELOW_ATOL < mcad_geom::solve::ATOL_LEN);

#[test]
fn degenerate_lines_do_not_satisfy_constraints() {
    // 長さ 0・ATOL_LEN 未満の線分でも、等長・一致の残差は 0 になる（水平・垂直は向きが
    // 定まらず NaN）。通すとソルバー（`admissible` が退化した形を棄却する）が初期状態として
    // 使えない文書になる。
    let mut doc = Document::new();
    let dot = line(&mut doc, (3.0, 3.0), (3.0, 3.0));
    let tiny = line(&mut doc, (3.0, 3.0), (3.0 + BELOW_ATOL, 3.0));
    let dot2 = line(&mut doc, (8.0, 1.0), (8.0, 1.0));
    let normal = line(&mut doc, (3.0, 3.0), (13.0, 3.0));

    let mut rejected = Vec::new();
    for degenerate in [dot, tiny] {
        rejected.extend([
            Constraint::Horizontal { line: degenerate },
            Constraint::Vertical { line: degenerate },
            Constraint::EqualLength {
                a: degenerate,
                b: dot2,
            },
            Constraint::EqualLength {
                a: dot2,
                b: degenerate,
            },
            Constraint::Coincident {
                a: start(degenerate),
                b: start(normal),
            },
            Constraint::Coincident {
                a: start(normal),
                b: start(degenerate),
            },
            Constraint::Coincident {
                a: start(degenerate),
                b: end(degenerate),
            },
            Constraint::FixPoint {
                point: start(degenerate),
                at: Point2::new(3.0, 3.0),
            },
            Constraint::FixEntity { entity: degenerate },
        ]);
    }
    for constraint in rejected {
        // 構造は正しく、残差だけなら満たされている（退化の判定が効いていることの確認は
        // この下の「退化していない線分の同じ拘束は入る」と対で行う）。
        assert_eq!(constraint.validate(&doc), Ok(()), "{constraint:?}");
        assert_eq!(constraint.is_satisfied(&doc), Ok(false), "{constraint:?}");
        let shown = format!("{constraint:?}");
        let before = snapshot(&doc);
        let err = doc.apply(add_constraint(constraint)).expect_err(&shown);
        assert!(degenerate(&err), "{shown}: {err:?}");
        assert_eq!(snapshot(&doc), before, "{shown}");
    }
    // 退化していない線分の同じ拘束は入る。
    let horizontal = doc.constrain(Constraint::Horizontal { line: normal });
    // ATOL_LEN 以上なら、短い線分でも退化ではない。
    let short = line(&mut doc, (0.0, -5.0), (1e-6, -5.0));
    let fix = doc.constrain(Constraint::FixEntity { entity: short });
    doc.constrain(Constraint::Horizontal { line: short });
    assert!(horizontal < fix);

    // 水平のついた線分を点へつぶす変更は拒否され、状態は変わらない。
    let collapse = |new_geom: EntityGeom, reshape: bool| {
        if reshape {
            Command::ReshapeEntity {
                id: normal,
                new_geom,
            }
        } else {
            Command::ModifyEntity {
                id: normal,
                new_geom,
            }
        }
    };
    assert!(doc.constraint(horizontal).is_some());
    let degenerate_normal = |e: &CoreError| *e == CoreError::DegenerateShape(normal);
    // ModifyEntity: 長さ 0・ATOL_LEN 未満とも、退化として拒否。
    assert_rejected(
        &mut doc,
        collapse(line_geom((5.0, 3.0), (5.0, 3.0)), false),
        degenerate_normal,
    );
    assert_rejected(
        &mut doc,
        collapse(line_geom((5.0, 3.0), (5.0 + BELOW_ATOL, 3.0)), false),
        degenerate_normal,
    );
    // ReshapeEntity: 向きを保ったまま ATOL_LEN 未満へ切り詰めると退化として拒否。
    // 長さ 0 は向きが定まらないので、切り詰めの判定で拒否。
    assert_rejected(
        &mut doc,
        collapse(line_geom((5.0, 3.0), (5.0 + BELOW_ATOL, 3.0)), true),
        degenerate_normal,
    );
    assert_rejected(
        &mut doc,
        collapse(line_geom((5.0, 3.0), (5.0, 3.0)), true),
        invalid,
    );
    // 退化しない切り詰めは通る。
    doc.apply(collapse(line_geom((5.0, 3.0), (5.5, 3.0)), true))
        .unwrap();
}

#[test]
fn degenerate_rounds_do_not_satisfy_constraints() {
    let mut doc = Document::new();
    let zero = circle(&mut doc, (3.0, 3.0), 0.0);
    let tiny = circle(&mut doc, (3.0, 3.0), BELOW_ATOL);
    let zero_arc = add(&mut doc, arc_geom((3.0, 3.0), 0.0, 0.0, 1.0));
    let spoke = line(&mut doc, (3.0, 3.0), (13.0, 3.0));
    let good = circle(&mut doc, (3.0, 3.0), 2.0);

    let mut rejected = Vec::new();
    for degenerate in [zero, tiny, zero_arc] {
        rejected.extend([
            Constraint::Coincident {
                a: center(degenerate),
                b: start(spoke),
            },
            Constraint::Coincident {
                a: center(good),
                b: center(degenerate),
            },
            Constraint::FixPoint {
                point: center(degenerate),
                at: Point2::new(3.0, 3.0),
            },
            Constraint::FixEntity { entity: degenerate },
        ]);
    }
    rejected.extend([
        Constraint::EqualRadius {
            a: zero,
            b: zero_arc,
        },
        Constraint::EqualRadius { a: tiny, b: zero },
        // 半径 0 の円弧は始点・終点・中心が一致する。
        Constraint::Coincident {
            a: start(zero_arc),
            b: start(spoke),
        },
    ]);
    for constraint in rejected {
        assert_eq!(constraint.validate(&doc), Ok(()), "{constraint:?}");
        assert_eq!(constraint.is_satisfied(&doc), Ok(false), "{constraint:?}");
        assert_rejected(&mut doc, add_constraint(constraint), degenerate);
    }

    // 中心に一致のついた円を、半径 0・ATOL_LEN 未満へつぶす変更は拒否。
    doc.constrain(Constraint::Coincident {
        a: center(good),
        b: start(spoke),
    });
    for radius in [0.0, BELOW_ATOL] {
        assert_rejected(
            &mut doc,
            Command::ModifyEntity {
                id: good,
                new_geom: circle_geom((3.0, 3.0), radius),
            },
            |e| *e == CoreError::DegenerateShape(good),
        );
    }
    // 退化しない半径の変更は通る（一致は中心だけを見る）。
    doc.apply(Command::ModifyEntity {
        id: good,
        new_geom: circle_geom((3.0, 3.0), 1e-6),
    })
    .unwrap();
}

// ---------------------------------------------------------------------------
// 拘束の無い文書・接線の受理
// ---------------------------------------------------------------------------

#[test]
fn documents_without_constraints_skip_all_constraint_work() {
    // 拘束を一度も使わない文書では、拘束のスロットも検査も発生しない。
    let mut doc = Document::new();
    let l = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    doc.apply(Command::ModifyEntity {
        id: l,
        new_geom: circle_geom((0.0, 0.0), 1.0),
    })
    .unwrap();
    doc.apply(Command::RemoveEntity(l)).unwrap();
    assert!(doc.constraints.is_empty());
    assert_eq!(doc.live_constraints, 0);

    // 拘束をすべて消した後も、種類の変更などの制限は残らない。
    let l = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    let id = doc.constrain(Constraint::Horizontal { line: l });
    doc.apply(Command::RemoveConstraint(id)).unwrap();
    assert_eq!(doc.live_constraints, 0);
    doc.apply(Command::ModifyEntity {
        id: l,
        new_geom: line_geom((0.0, 0.0), (3.0, 4.0)),
    })
    .unwrap();
}

#[test]
fn tangent_with_contact_outside_sweep_is_accepted() {
    // 接点が円弧の掃引範囲・線分の区間の外でも、接線拘束は満たされたものとして入る。
    let mut doc = Document::new();
    // ±π をまたぐ掃引（左側の 1/4）。
    let a = add(&mut doc, arc_geom((1.0, 2.0), 10.0, 0.75 * PI, -0.75 * PI));
    let outer = circle(&mut doc, (14.0, 2.0), 3.0);
    let inner = circle(&mut doc, (8.0, 2.0), 3.0);
    let top = line(&mut doc, (6.0, 12.0), (20.0, 12.0));
    for constraint in [
        Constraint::tangent_circles(&doc, a, outer).unwrap(),
        Constraint::tangent_circles(&doc, a, inner).unwrap(),
        Constraint::tangent_line_circle(&doc, top, a).unwrap(),
    ] {
        let id = doc.constrain(constraint);
        let contact = doc.constraint(id).unwrap().tangent_contact(&doc).unwrap();
        assert!(contact.is_extended(), "{:?}", doc.constraint(id));
    }
    assert_eq!(doc.constraint_count(), 3);
}
