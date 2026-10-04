//! [`Document::apply`] の中の求解のテスト（M13 タスク97 の完了条件、DESIGN.md M13 設計判断1・3）。
//!
//! 追従と undo/redo、`Batch`、失敗後の状態の完全一致、`AddConstraint` の `hold`、
//! `SetConstraintValue`、ロック、点の固定、接線、許容値、退化、グリップ相当の編集、自由度、
//! 拘束と無関係な編集、ミラー（`ReplaceConstraint`・`Constraint::mirrored`）。
//! データモデルと構造検証のテストは `constraint_tests.rs`。

use std::collections::BTreeSet;
use std::f64::consts::PI;

use mcad_geom::{Arc, Circle, LineSeg, Point2, Polyline, Shape, Vec2};
use proptest::prelude::*;

use super::constraint_tests::{
    Constrain, add, add_constraint, arc_geom, assert_all_satisfied, assert_close, assert_rejected,
    blocked, center, circle, circle_geom, end, geom, invalid, line, line_geom, rectangle, snapshot,
    start, translated,
};
use super::constraints::Edits;
use super::*;
use crate::{CircleTangency, LargerCircle, PointRef, Side};

// ---------------------------------------------------------------------------
// ヘルパー
// ---------------------------------------------------------------------------

/// `hold` つきの `AddConstraint`。
fn add_holding(constraint: Constraint, hold: EntityId) -> Command {
    Command::AddConstraint {
        constraint,
        hold: Some(hold),
    }
}

fn modify(id: EntityId, new_geom: EntityGeom) -> Command {
    Command::ModifyEntity { id, new_geom }
}

fn line_of(doc: &Document, id: EntityId) -> LineSeg {
    match geom(doc, id) {
        EntityGeom::Shape(Shape::Line(l)) => l,
        other => panic!("expected a line, got {other:?}"),
    }
}

fn circle_of(doc: &Document, id: EntityId) -> Circle {
    match geom(doc, id) {
        EntityGeom::Shape(Shape::Circle(c)) => c,
        other => panic!("expected a circle, got {other:?}"),
    }
}

fn arc_of(doc: &Document, id: EntityId) -> Arc {
    match geom(doc, id) {
        EntityGeom::Shape(Shape::Arc(a)) => a,
        other => panic!("expected an arc, got {other:?}"),
    }
}

fn dist(p: Point2, q: Point2) -> f64 {
    (p.x - q.x).hypot(p.y - q.y)
}

fn length(l: LineSeg) -> f64 {
    dist(l.a, l.b)
}

/// 新しいレイヤーを作り、`entity` をそこへ移してからロックする。
fn lock_on_new_layer(doc: &mut Document, entity: EntityId) -> LayerId {
    let layer = doc
        .apply(Command::AddLayer(Layer::new("locked", Rgb::WHITE)))
        .unwrap()
        .layers[0];
    doc.apply(Command::SetEntityLayer { id: entity, layer })
        .unwrap();
    let mut props = doc.layer(layer).unwrap().clone();
    props.locked = true;
    doc.apply(Command::SetLayerProps { id: layer, props })
        .unwrap();
    layer
}

/// 全エンティティの幾何（bit 単位の比較用）。
fn all_geoms(doc: &Document) -> Vec<(EntityId, EntityGeom)> {
    doc.entities().map(|(id, e)| (id, e.geom.clone())).collect()
}

/// 履歴の最後のエントリが、追従した修正を含むか（元の記録 + 追従の `Batch` に包まれる）。
fn follow_up_count(doc: &Document) -> usize {
    let Applied::Batch(records) = &doc.undo_stack.last().unwrap().applied else {
        return 0;
    };
    // 追従した修正は、先頭（元の記録）の後ろに並ぶ `ModifyEntity`。元の記録が `Batch` で
    // ないコマンドのときだけ使う。
    records.len() - 1
}

// ---------------------------------------------------------------------------
// 1. 検収 (i): 矩形の追従と undo/redo
// ---------------------------------------------------------------------------

#[test]
fn rectangle_follows_a_translated_side() {
    let mut doc = Document::new();
    let rect = rectangle(&mut doc);
    let before = all_geoms(&doc);
    let generation = doc.generation();
    let undo_depth = doc.undo_stack.len();

    // 下の辺 (0,0)-(40,0) を (3, −5) だけ動かす。
    let moved = geom(&doc, rect.sides[0]).translated(Vec2::new(3.0, -5.0));
    let ids = doc.apply(modify(rect.sides[0], moved.clone())).unwrap();
    assert_eq!(ids, NewIds::default());

    // 動かした辺はそのまま（bit 単位）。つながった辺が追従して矩形のまま。
    assert_eq!(geom(&doc, rect.sides[0]), moved);
    assert_close(
        &geom(&doc, rect.sides[1]),
        &line_geom((43.0, -5.0), (43.0, 30.0)),
        1e-7,
    );
    assert_close(
        &geom(&doc, rect.sides[2]),
        &line_geom((43.0, 30.0), (3.0, 30.0)),
        1e-7,
    );
    assert_close(
        &geom(&doc, rect.sides[3]),
        &line_geom((3.0, 30.0), (3.0, -5.0)),
        1e-7,
    );
    assert_all_satisfied(&doc);
    assert_eq!(doc.constraint_count(), 8);
    // 履歴は 1 エントリ（コマンド + 追従した 3 辺）、世代の採番は 1 回。
    assert_eq!(doc.undo_stack.len(), undo_depth + 1);
    assert_eq!(follow_up_count(&doc), 3);
    assert_eq!(doc.generation(), generation + 1);
    assert_eq!(doc.next_generation, generation + 1);
    let solved = all_geoms(&doc);

    // undo 1 回で全体が戻る。
    assert!(doc.undo());
    assert_eq!(all_geoms(&doc), before);
    assert_eq!(doc.generation(), generation);
    assert_all_satisfied(&doc);
    // redo は解き直さず、同じ形（bit 単位）を再現する。
    assert!(doc.redo());
    assert_eq!(all_geoms(&doc), solved);
    assert_eq!(doc.generation(), generation + 1);
    // もう一度往復しても同じ。
    assert!(doc.undo());
    assert_eq!(all_geoms(&doc), before);
    assert!(doc.redo());
    assert_eq!(all_geoms(&doc), solved);
}

/// 一致 4・直交 3 で組んだ矩形（[`rectangle`] と同じ 40 × 30。水平・垂直を使わない）。
fn perpendicular_rectangle(doc: &mut Document) -> [EntityId; 4] {
    let corners = [(0.0, 0.0), (40.0, 0.0), (40.0, 30.0), (0.0, 30.0)];
    let sides: [EntityId; 4] = std::array::from_fn(|i| line(doc, corners[i], corners[(i + 1) % 4]));
    for i in 0..4 {
        doc.constrain(Constraint::Coincident {
            a: end(sides[i]),
            b: start(sides[(i + 1) % 4]),
        });
    }
    for i in 0..3 {
        doc.constrain(Constraint::Perpendicular {
            a: sides[i],
            b: sides[i + 1],
        });
    }
    sides
}

#[test]
fn a_long_move_keeps_the_far_side_of_a_rectangle_in_place() {
    // 下の辺を辺に沿って 300 mm（図形の大きさの約 10 倍）動かす。最小ノルムの追従は、縦の
    // 2 辺が平行移動し、上の辺は y = 30 のまま横へ滑るだけ（矩形の高さは変わらない）。
    //
    // 向きの拘束（水平・垂直・直交）は非線形で、ソルバーの 1 回目のステップが足りないと、
    // 途中の形で縦の辺が傾き、そこからの補正で上の辺が持ち上がる。初期減衰
    // `mcad_geom::solve::LAMBDA_0` が 1e-3 のとき、上の辺は水平・垂直の矩形で 0.19 mm、
    // 直交の矩形で 2.1 mm 持ち上がった（M13 タスク97 の実測。1e-9 にして解消）。
    type Build = fn(&mut Document) -> [EntityId; 4];
    let with_axes: Build = |doc| rectangle(doc).sides;
    let builds: [(&str, Build); 2] = [
        ("horizontal/vertical", with_axes),
        ("perpendicular", perpendicular_rectangle),
    ];
    for (name, build) in builds {
        let mut doc = Document::new();
        let sides = build(&mut doc);
        let moved = geom(&doc, sides[0]).translated(Vec2::new(300.0, 0.0));
        doc.apply(modify(sides[0], moved.clone())).unwrap();
        assert_eq!(geom(&doc, sides[0]), moved, "{name}");
        for (side, expected) in [
            (sides[1], line_geom((340.0, 0.0), (340.0, 30.0))),
            (sides[2], line_geom((340.0, 30.0), (300.0, 30.0))),
            (sides[3], line_geom((300.0, 30.0), (300.0, 0.0))),
        ] {
            assert_close(&geom(&doc, side), &expected, 1e-7);
        }
        assert_all_satisfied(&doc);
    }
}

#[test]
fn solving_is_separable_from_apply_and_converges_quickly() {
    // 求解本体は `&self` の純関数（プレビュー用の足場）: execute → 解く → revert で、文書を
    // 変えずに「追従する修正」だけを得られる。
    let mut doc = Document::new();
    let rect = rectangle(&mut doc);
    let before = snapshot(&doc);
    let cmd = translated(&doc, rect.sides[0], (3.0, -5.0));

    let outcome = doc.execute(cmd.clone()).unwrap();
    let mut edits = Edits::default();
    edits.collect(&outcome.applied);
    let follow_ups = doc.solve_constraints(&edits).unwrap();
    doc.revert(&outcome.applied);
    assert_eq!(snapshot(&doc), before);

    // 追従は残りの 3 辺、EntityId 順。
    let ids: Vec<EntityId> = follow_ups.changes.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, rect.sides[1..].to_vec());
    // 一致は線形、水平・垂直は解の近くでほぼ線形なので、数回の試行で収束する（上限は 200）。
    assert!(
        (1..=6).contains(&follow_ups.trials),
        "trials {}",
        follow_ups.trials
    );

    // apply した結果と同じ形。
    doc.apply(cmd).unwrap();
    for (id, solved) in &follow_ups.changes {
        assert_eq!(&geom(&doc, *id), solved);
    }
}

// ---------------------------------------------------------------------------
// 2. Batch
// ---------------------------------------------------------------------------

#[test]
fn batch_result_does_not_depend_on_sub_command_order() {
    // 拒否される Batch: 隣り合う 2 辺を別々の量だけ動かす。辺 0 の終点 (40, −4) と辺 1 の始点
    // (46, 0) は一致でつながっており、どちらも利用者が動かした点として止められるので、
    // 食い違ったままでは満たせない。順序に依らず同じ拒否になる。
    let rejected = |reversed: bool| {
        let mut doc = Document::new();
        let rect = rectangle(&mut doc);
        let mut subs = vec![
            translated(&doc, rect.sides[0], (0.0, -4.0)),
            translated(&doc, rect.sides[1], (6.0, 0.0)),
        ];
        if reversed {
            subs.reverse();
        }
        let before = snapshot(&doc);
        let err = doc.apply(Command::Batch(subs)).unwrap_err();
        assert_eq!(snapshot(&doc), before);
        err
    };
    assert_eq!(rejected(false), rejected(true));
    assert!(blocked(&rejected(false)));

    // 受理される Batch: 向かい合う 2 辺（一致でつながっていない）を別々に動かす。
    let accepted = |reversed: bool| {
        let mut doc = Document::new();
        let rect = rectangle(&mut doc);
        let moved = [
            geom(&doc, rect.sides[0]).translated(Vec2::new(2.0, -4.0)),
            geom(&doc, rect.sides[2]).translated(Vec2::new(2.0, 9.0)),
        ];
        let mut subs = vec![
            modify(rect.sides[0], moved[0].clone()),
            modify(rect.sides[2], moved[1].clone()),
        ];
        if reversed {
            subs.reverse();
        }
        doc.apply(Command::Batch(subs)).unwrap();
        assert_all_satisfied(&doc);
        assert_eq!(geom(&doc, rect.sides[0]), moved[0]);
        assert_eq!(geom(&doc, rect.sides[2]), moved[1]);
        // 残りの 2 辺が追従し、undo 1 回で全体が戻る。
        assert_close(
            &geom(&doc, rect.sides[1]),
            &line_geom((42.0, -4.0), (42.0, 39.0)),
            1e-7,
        );
        assert_close(
            &geom(&doc, rect.sides[3]),
            &line_geom((2.0, 39.0), (2.0, -4.0)),
            1e-7,
        );
        all_geoms(&doc)
    };
    assert_eq!(accepted(false), accepted(true));
}

#[test]
fn batch_modifying_an_entity_twice_compares_start_and_final_shape() {
    // 同じ図形を Batch の中で 2 回変えても、止めるのは「開始時と最終形で違う点」だけ。
    let mut doc = Document::new();
    let l = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    doc.constrain(Constraint::distance(&doc, start(l), end(l)).unwrap());
    // 始点を動かしてから元へ戻し、終点だけを動かす。最終形で違うのは終点だけなので、
    // 始点は自由で、長さを保つように動く。
    doc.apply(Command::Batch(vec![
        modify(l, line_geom((5.0, 5.0), (10.0, 0.0))),
        modify(l, line_geom((0.0, 0.0), (10.0, 6.0))),
    ]))
    .unwrap();
    let solved = line_of(&doc, l);
    assert_eq!(solved.b, Point2::new(10.0, 6.0));
    assert!((length(solved) - 10.0).abs() <= 1e-7);
    assert_ne!(solved.a, Point2::new(0.0, 0.0));
    assert!(doc.undo());
    assert_eq!(geom(&doc, l), line_geom((0.0, 0.0), (10.0, 0.0)));
}

#[test]
fn batch_is_rolled_back_when_the_final_solve_fails() {
    let mut doc = Document::new();
    let rect = rectangle(&mut doc);
    let extra = line(&mut doc, (100.0, 0.0), (110.0, 0.0));
    // 先のサブコマンド（無関係な線分の移動・辺 1 の移動・拘束の追加）は単独なら通るが、
    // 最後の辺 0 の回転が水平に反するので、全部が戻る。
    let rotated = geom(&doc, rect.sides[0]).rotated(Point2::new(20.0, 0.0), 0.2);
    let batch = Command::Batch(vec![
        translated(&doc, extra, (1.0, 1.0)),
        translated(&doc, rect.sides[2], (0.0, 3.0)),
        add_constraint(Constraint::Horizontal { line: extra }),
        modify(rect.sides[0], rotated),
    ]);
    assert_rejected(&mut doc, batch, blocked);
}

// ---------------------------------------------------------------------------
// 3. 失敗後の状態一致
// ---------------------------------------------------------------------------

#[test]
fn failed_solves_leave_the_document_untouched() {
    let mut doc = Document::new();
    let l = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    let m = line(&mut doc, (10.0, 0.0), (10.0, 5.0));
    let big = circle(&mut doc, (50.0, 50.0), 10.0);
    let small = circle(&mut doc, (57.0, 50.0), 3.0);
    let horizontal = doc.constrain(Constraint::Horizontal { line: l });
    let fix_l = doc.constrain(Constraint::fix_point(&doc, end(l)).unwrap());
    let fix_m = doc.constrain(Constraint::fix_point(&doc, end(m)).unwrap());
    let inner = doc.constrain(Constraint::tangent_circles(&doc, big, small).unwrap());
    assert_eq!(
        doc.constraint(inner),
        Some(&Constraint::TangentCircles {
            a: big,
            b: small,
            kind: CircleTangency::Internal(LargerCircle::First),
        })
    );
    let fixed = line(&mut doc, (0.0, 20.0), (10.0, 20.0));
    doc.constrain(Constraint::FixEntity { entity: fixed });
    // redo スタックも保たれることを見るため、undo 済みの操作を 1 つ残しておく。
    line(&mut doc, (100.0, 100.0), (110.0, 100.0));
    assert!(doc.undo());
    assert!(doc.can_redo());

    // 解の無い系: 両方とも点の固定で止まった 2 点（5 mm 離れている）を一致させる。
    assert_rejected(
        &mut doc,
        add_constraint(Constraint::Coincident {
            a: end(l),
            b: end(m),
        }),
        |e| matches!(e, CoreError::ConstraintUnsolvable(_)),
    );
    // hold をつけても同じ（止めて解けず、外して解き直しても解けない）。
    assert_rejected(
        &mut doc,
        add_holding(
            Constraint::Coincident {
                a: end(l),
                b: end(m),
            },
            l,
        ),
        |e| matches!(e, CoreError::ConstraintUnsolvable(_)),
    );
    // 自由なパラメータが無い系: 水平拘束の線分を単独で回転する。
    let rotated = geom(&doc, l).rotated(Point2::new(5.0, 0.0), 0.4);
    assert_rejected(&mut doc, modify(l, rotated), |e| {
        *e == CoreError::ConstraintBlockedByFixed(horizontal)
    });
    // 点の固定がついた点を動かす。
    let cmd = translated(&doc, m, (0.0, 1.0));
    assert_rejected(&mut doc, cmd, |e| {
        *e == CoreError::ConstraintBlockedByFixed(fix_m)
    });
    assert!(doc.constraint(fix_l).is_some());
    // 固定した図形の移動（サブコマンドの時点で拒否）。
    let cmd = translated(&doc, fixed, (1.0, 0.0));
    assert_rejected(&mut doc, cmd, |e| *e == CoreError::EntityFixed(fixed));
    // 矛盾: 水平拘束のついた線分に垂直を足す（向きの拘束どうしで、同時には満たせない）。
    assert_rejected(
        &mut doc,
        add_constraint(Constraint::Vertical { line: l }),
        |e| matches!(e, CoreError::ConstraintUnsolvable(_)),
    );
    // 退化: 線分の両端を一致させると、線分が 1 点につぶれる。
    assert_rejected(
        &mut doc,
        add_constraint(Constraint::Coincident {
            a: start(m),
            b: end(m),
        }),
        |e| *e == CoreError::DegenerateShape(m),
    );
    // 構造検証: 内接の 2 円を同心・等半径にする（残差は 0 のまま）。
    assert_rejected(
        &mut doc,
        modify(small, circle_geom((50.0, 50.0), 10.0)),
        |e| *e == CoreError::DegenerateConstraint(inner),
    );
    // Batch の中でも同じ。
    let batch = Command::Batch(vec![
        translated(&doc, big, (1.0, 0.0)),
        modify(small, circle_geom((51.0, 50.0), 10.0)),
    ]);
    assert_rejected(&mut doc, batch, |e| {
        *e == CoreError::DegenerateConstraint(inner)
    });
}

/// 線分の始点と終点。
type Ends = ((f64, f64), (f64, f64));

/// 線分 `lines` を置いた文書へ `make` の作るコマンド（拘束の追加）を適用し、文書と線分の ID を
/// 返す。同じ手順を 2 回行い、結果が bit 単位で一致すること（決定的）、すべての拘束が満たされて
/// いること、どの線分も退化していない（回るだけで、長さは減らない）ことを確かめる。
fn solved_from_a_quarter_turn(
    lines: &[Ends],
    make: impl Fn(&[EntityId]) -> Command,
) -> (Document, Vec<EntityId>) {
    let run = || {
        let mut doc = Document::new();
        let ids: Vec<EntityId> = lines.iter().map(|&(a, b)| line(&mut doc, a, b)).collect();
        let before = all_geoms(&doc);
        doc.apply(make(&ids))
            .unwrap_or_else(|e| panic!("{lines:?} must be solved: {e:?}"));
        assert_all_satisfied(&doc);
        for (&id, &(a, b)) in ids.iter().zip(lines) {
            // 回るだけで長さを保つ処理は無いので、線分は伸びる（最小ノルムの更新は端点を
            // 線分と直角に動かす）。90° の回転でも 2.5 倍には届かない。
            let original = dist(Point2::new(a.0, a.1), Point2::new(b.0, b.1));
            let solved = length(line_of(&doc, id));
            assert!(
                solved >= original && solved < 2.5 * original,
                "{lines:?}: length {original} -> {solved}"
            );
        }
        // undo 1 回で元の形へ戻る。
        let solved = all_geoms(&doc);
        assert!(doc.undo());
        assert_eq!(all_geoms(&doc), before);
        assert!(doc.redo());
        assert_eq!(all_geoms(&doc), solved);
        (doc, ids)
    };
    let (first, ids) = run();
    let (second, _) = run();
    assert_eq!(all_geoms(&first), all_geoms(&second), "{lines:?}");
    (first, ids)
}

/// 線分の向きが水平から何 rad ずれているか（`[0, π/2]`。向きは区別しない）。
fn tilt_from_horizontal(l: LineSeg) -> f64 {
    let d = l.direction();
    d.y.abs().atan2(d.x.abs())
}

#[test]
fn direction_constraints_solve_from_a_quarter_turn_away() {
    // 解から最も遠い形（ちょうど 90° ずれた形）へ向きの拘束を足す。直交モードで描いた線で
    // ふつうに当たる。以前の式では、平行・直交は勾配が 0 で停滞し、水平・垂直は線分が
    // 1 点へつぶれて拒否されていた。
    const FLAT: Ends = ((0.0, 0.0), (10.0, 0.0));
    const UPRIGHT: Ends = ((3.0, 2.0), (3.0, 9.0));
    const FLAT_ABOVE: Ends = ((0.0, 5.0), (10.0, 5.0));
    const FLAT_BACK: Ends = ((10.0, 5.0), (0.0, 5.0));
    let seg = |l: Ends| LineSeg::new(Point2::new(l.0.0, l.0.1), Point2::new(l.1.0, l.1.1));
    let mid = |l: LineSeg| Point2::new(0.5 * (l.a.x + l.b.x), 0.5 * (l.a.y + l.b.y));

    // ちょうど直交した 2 線分へ平行、ちょうど平行（同じ向き・反平行）な 2 線分へ直交。
    type Make = fn(EntityId, EntityId) -> Constraint;
    let parallel: Make = |a, b| Constraint::Parallel { a, b };
    let perpendicular: Make = |a, b| Constraint::Perpendicular { a, b };
    for (pair, make) in [
        ([FLAT, UPRIGHT], parallel),
        ([FLAT, FLAT_ABOVE], perpendicular),
        ([FLAT, FLAT_BACK], perpendicular),
    ] {
        // hold なし: 両方が回る（最小ノルム）。どちらも中点のまわりに回る。
        let (doc, ids) =
            solved_from_a_quarter_turn(&pair, |ids| add_constraint(make(ids[0], ids[1])));
        for (&id, &original) in ids.iter().zip(&pair) {
            assert_ne!(line_of(&doc, id), seg(original));
            assert!(dist(mid(line_of(&doc, id)), mid(seg(original))) <= 1e-9);
        }
        // hold つき: hold した方は動かず（bit 単位）、他方だけが 90° 回る。対象の順（a・b）と
        // hold の組み合わせをすべて試す。
        for held in 0..2 {
            for swap in [false, true] {
                let (doc, ids) = solved_from_a_quarter_turn(&pair, |ids| {
                    let (a, b) = if swap {
                        (ids[1], ids[0])
                    } else {
                        (ids[0], ids[1])
                    };
                    add_holding(make(a, b), ids[held])
                });
                let turned = 1 - held;
                assert_eq!(line_of(&doc, ids[held]), seg(pair[held]));
                // 元の向きは軸に沿っているので、回った線分は元と直交する向きになる。
                let (before, after) = (seg(pair[turned]), line_of(&doc, ids[turned]));
                let quarter = (tilt_from_horizontal(after) - tilt_from_horizontal(before)).abs();
                assert!((quarter - PI / 2.0).abs() <= 1e-9, "turned by {quarter}");
                assert!(dist(mid(after), mid(before)) <= 1e-9);
            }
        }
    }

    // ちょうど水平な線分へ垂直、ちょうど垂直な線分へ水平（線分の向きはどちら向きでも）。
    let vertical: fn(EntityId) -> Constraint = |line| Constraint::Vertical { line };
    let horizontal: fn(EntityId) -> Constraint = |line| Constraint::Horizontal { line };
    for (original, make, solved_tilt) in [
        (FLAT, vertical, PI / 2.0),
        (FLAT_BACK, vertical, PI / 2.0),
        (UPRIGHT, horizontal, 0.0),
        ((UPRIGHT.1, UPRIGHT.0), horizontal, 0.0),
    ] {
        let (doc, ids) =
            solved_from_a_quarter_turn(&[original], |ids| add_constraint(make(ids[0])));
        let solved = line_of(&doc, ids[0]);
        assert!((tilt_from_horizontal(solved) - solved_tilt).abs() <= 1e-9);
        assert!(dist(mid(solved), mid(seg(original))) <= 1e-9);
        // 対象が 1 つだけの拘束で、その線分を hold にしても解ける（止めたままでは満たせない
        // ので hold を外して解き直す。結果は hold なしと同じ）。
        let (held, held_ids) =
            solved_from_a_quarter_turn(&[original], |ids| add_holding(make(ids[0]), ids[0]));
        assert_eq!(geom(&held, held_ids[0]), geom(&doc, ids[0]));
    }
}

// ---------------------------------------------------------------------------
// 4. AddConstraint の hold（検収 iv）
// ---------------------------------------------------------------------------

#[test]
fn add_constraint_moves_the_shape_that_is_not_held() {
    let mut doc = Document::new();
    let l1 = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    let l2 = line(&mut doc, (12.0, 1.0), (12.0, 6.0));
    let (g1, g2) = (geom(&doc, l1), geom(&doc, l2));
    let coincident = Constraint::Coincident {
        a: end(l1),
        b: start(l2),
    };

    // 先に選んだ l1 を止める: l1 は動かず（bit 単位）、l2 の始点が寄ってくる。
    doc.apply(add_holding(coincident.clone(), l1)).unwrap();
    assert_eq!(geom(&doc, l1), g1);
    let moved = line_of(&doc, l2);
    assert!(dist(moved.a, Point2::new(10.0, 0.0)) <= 1e-7);
    // 拘束が参照しない端点は動かない。
    assert_eq!(moved.b, Point2::new(12.0, 6.0));
    assert_all_satisfied(&doc);
    assert!(doc.undo());
    assert_eq!((geom(&doc, l1), geom(&doc, l2)), (g1.clone(), g2.clone()));

    // l2 を止めれば、l1 が動く。フィールド順（a・b）は関係ない。
    doc.apply(add_holding(coincident.clone(), l2)).unwrap();
    assert_eq!(geom(&doc, l2), g2);
    let moved = line_of(&doc, l1);
    assert!(dist(moved.b, Point2::new(12.0, 1.0)) <= 1e-7);
    assert_eq!(moved.a, Point2::new(0.0, 0.0));
    assert!(doc.undo());

    // hold: None は最小ノルム（2 点が中間で出会う）。
    doc.apply(add_constraint(coincident.clone())).unwrap();
    let (m1, m2) = (line_of(&doc, l1), line_of(&doc, l2));
    assert!(dist(m1.b, Point2::new(11.0, 0.5)) <= 1e-7, "{m1:?}");
    assert!(dist(m2.a, Point2::new(11.0, 0.5)) <= 1e-7, "{m2:?}");
    assert!(doc.undo());

    // すでに満たされている拘束は、hold があっても何も動かさない。
    let l3 = line(&mut doc, (10.0, 0.0), (20.0, 5.0));
    let before = all_geoms(&doc);
    doc.apply(add_holding(
        Constraint::Coincident {
            a: end(l1),
            b: start(l3),
        },
        l3,
    ))
    .unwrap();
    assert_eq!(all_geoms(&doc), before);
    assert_eq!(follow_up_count(&doc), 0);

    // 拘束の対象でない図形は hold にできない。
    assert_rejected(&mut doc, add_holding(coincident, l3), invalid);
    // 存在しない図形も同じ（対象でない）。
    let ghost = {
        let id = line(&mut doc, (0.0, 0.0), (1.0, 1.0));
        doc.apply(Command::RemoveEntity(id)).unwrap();
        id
    };
    assert_rejected(
        &mut doc,
        add_holding(Constraint::Horizontal { line: l1 }, ghost),
        invalid,
    );
}

#[test]
fn add_constraint_releases_the_hold_when_it_cannot_be_kept() {
    // a は始点を固定した水平な線分（終点は y = 0 の上を動ける）。b は終点を固定した長さ 10 の
    // 線分（始点は (40, 5) を中心とする半径 10 の円の上を動ける）。a の終点と b の始点を
    // 一致させるとき、a を止めたままでは満たせない（a の終点は b の円から 20 以上離れている）。
    let mut doc = Document::new();
    let a = line(&mut doc, (0.0, 0.0), (25.0, 0.0));
    let b = line(&mut doc, (30.0, 5.0), (40.0, 5.0));
    doc.constrain(Constraint::fix_point(&doc, start(a)).unwrap());
    doc.constrain(Constraint::Horizontal { line: a });
    doc.constrain(Constraint::fix_point(&doc, end(b)).unwrap());
    doc.constrain(Constraint::distance(&doc, start(b), end(b)).unwrap());
    let (ga, gb) = (geom(&doc, a), geom(&doc, b));
    let coincident = Constraint::Coincident {
        a: end(a),
        b: start(b),
    };

    doc.apply(add_holding(coincident, a)).unwrap();
    // hold を外して解き直され、両方が動いて受理される。
    assert_ne!(geom(&doc, a), ga);
    assert_ne!(geom(&doc, b), gb);
    assert_all_satisfied(&doc);
    let (sa, sb) = (line_of(&doc, a), line_of(&doc, b));
    assert!(dist(sa.b, sb.a) <= 1e-7);
    assert!(sa.b.y.abs() <= 1e-7 && (length(sb) - 10.0).abs() <= 1e-7);
    // undo 1 回で戻る。
    assert!(doc.undo());
    assert_eq!((geom(&doc, a), geom(&doc, b)), (ga, gb));
}

#[test]
fn batch_of_add_constraints_holds_every_first_pick() {
    // 1 つの Batch に複数の AddConstraint があるときは、それぞれの hold をまとめて止める。
    let mut doc = Document::new();
    let base = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    let left = line(&mut doc, (-1.0, 1.0), (0.0, 8.0));
    let right = line(&mut doc, (11.0, 1.0), (10.0, 8.0));
    let g = geom(&doc, base);
    doc.apply(Command::Batch(vec![
        add_holding(
            Constraint::Coincident {
                a: start(base),
                b: start(left),
            },
            base,
        ),
        add_holding(
            Constraint::Coincident {
                a: start(right),
                b: end(base),
            },
            base,
        ),
    ]))
    .unwrap();
    assert_eq!(geom(&doc, base), g);
    assert!(dist(line_of(&doc, left).a, Point2::new(0.0, 0.0)) <= 1e-7);
    assert!(dist(line_of(&doc, right).a, Point2::new(10.0, 0.0)) <= 1e-7);
    assert_all_satisfied(&doc);
    assert!(doc.undo());
    assert_eq!(doc.constraint_count(), 0);
}

// ---------------------------------------------------------------------------
// 5. SetConstraintValue（検収 ii）
// ---------------------------------------------------------------------------

#[test]
fn set_constraint_value_reshapes_and_undoes_in_one_step() {
    let mut doc = Document::new();
    let a = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    let b = line(&mut doc, (0.0, 0.0), (6.0, 8.0));
    let c = circle(&mut doc, (20.0, 20.0), 4.0);
    doc.constrain(Constraint::Coincident {
        a: start(a),
        b: start(b),
    });
    let length_id = doc.constrain(Constraint::distance(&doc, start(a), end(a)).unwrap());
    let radius_id = doc.constrain(Constraint::radius(&doc, c).unwrap());
    let angle_id = doc.constrain(Constraint::angle(&doc, a, b).unwrap());
    let before = all_geoms(&doc);
    let undo_depth = doc.undo_stack.len();
    let set = |id, value| Command::SetConstraintValue { id, value };

    // 距離: 線分 a の長さが 15 になる。
    doc.apply(set(length_id, 15.0)).unwrap();
    assert!((length(line_of(&doc, a)) - 15.0).abs() <= 1e-7);
    assert_all_satisfied(&doc);
    assert_eq!(doc.undo_stack.len(), undo_depth + 1);
    assert!(doc.undo());
    assert_eq!(all_geoms(&doc), before);
    assert_eq!(doc.constraint(length_id).unwrap().value(), Some(10.0));

    // 半径: 中心は動かず、半径だけが変わる（最小ノルム）。
    doc.apply(set(radius_id, 6.5)).unwrap();
    let solved = circle_of(&doc, c);
    assert_eq!(solved.center, Point2::new(20.0, 20.0));
    assert!((solved.radius - 6.5).abs() <= 1e-7);
    assert!(doc.undo());
    assert_eq!(all_geoms(&doc), before);

    // 角度: a から b へのなす角が新しい値になる（始点の一致・a の長さは保たれる）。
    let theta = doc.constraint(angle_id).unwrap().value().unwrap();
    doc.apply(set(angle_id, theta + 0.3)).unwrap();
    let (sa, sb) = (line_of(&doc, a), line_of(&doc, b));
    let (da, db) = (sa.direction(), sb.direction());
    let phi = da.cross(db).atan2(da.dot(db));
    assert!((phi - (theta + 0.3)).abs() <= 1e-8, "{phi}");
    assert_all_satisfied(&doc);
    assert_ne!(all_geoms(&doc), before);
    let solved = all_geoms(&doc);
    assert!(doc.undo());
    assert_eq!(all_geoms(&doc), before);
    assert!(doc.redo());
    assert_eq!(all_geoms(&doc), solved);
}

// ---------------------------------------------------------------------------
// 6. ロック（検収 v）
// ---------------------------------------------------------------------------

#[test]
fn locked_layer_entities_are_never_moved_by_the_solver() {
    let mut doc = Document::new();
    let free = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    let held = line(&mut doc, (10.0, 0.0), (10.0, 5.0));
    let joint = doc.constrain(Constraint::Coincident {
        a: end(free),
        b: start(held),
    });
    let round = circle(&mut doc, (30.0, 0.0), 4.0);
    let radius = doc.constrain(Constraint::radius(&doc, round).unwrap());
    lock_on_new_layer(&mut doc, held);
    let layer = lock_on_new_layer(&mut doc, round);
    let locked = (geom(&doc, held), geom(&doc, round));

    // ロックされた図形を動かさずに済む編集は通る。
    doc.apply(modify(free, line_geom((-3.0, 4.0), (10.0, 0.0))))
        .unwrap();
    assert_eq!((geom(&doc, held), geom(&doc, round)), locked);
    // ロックされた図形を動かさないと満たせない編集は拒否。
    let cmd = translated(&doc, free, (1.0, 0.0));
    assert_rejected(&mut doc, cmd, |e| {
        *e == CoreError::ConstraintBlockedByFixed(joint)
    });
    assert_rejected(
        &mut doc,
        Command::SetConstraintValue {
            id: radius,
            value: 5.0,
        },
        |e| *e == CoreError::ConstraintBlockedByFixed(radius),
    );
    // ロックされた図形そのものの変更は、従来どおりサブコマンドの時点で拒否。
    let cmd = translated(&doc, round, (1.0, 0.0));
    assert_rejected(&mut doc, cmd, |e| *e == CoreError::LayerLocked(layer));

    // 拘束の追加では、ロックされていない側だけが動く（hold が無くても）。
    let other = line(&mut doc, (12.0, 7.0), (20.0, 7.0));
    doc.apply(add_constraint(Constraint::Coincident {
        a: end(held),
        b: start(other),
    }))
    .unwrap();
    assert_eq!((geom(&doc, held), geom(&doc, round)), locked);
    assert!(dist(line_of(&doc, other).a, Point2::new(10.0, 5.0)) <= 1e-7);
    // 動かしたい側を hold で止めると、ロックと両立しないので hold が外れる。
    assert!(doc.undo());
    doc.apply(add_holding(
        Constraint::Coincident {
            a: end(held),
            b: start(other),
        },
        other,
    ))
    .unwrap();
    assert_eq!((geom(&doc, held), geom(&doc, round)), locked);
    assert!(dist(line_of(&doc, other).a, Point2::new(10.0, 5.0)) <= 1e-7);
    assert_all_satisfied(&doc);
}

// ---------------------------------------------------------------------------
// 7. 点の固定 / 8. 接線
// ---------------------------------------------------------------------------

#[test]
fn radius_change_keeps_the_fixed_arc_start_in_place() {
    let mut doc = Document::new();
    let a = add(&mut doc, arc_geom((2.0, 1.0), 5.0, 0.3, 2.0));
    let before = arc_of(&doc, a);
    let fix = Constraint::fix_point(&doc, start(a)).unwrap();
    doc.constrain(fix);
    let radius = doc.constrain(Constraint::radius(&doc, a).unwrap());

    doc.apply(Command::SetConstraintValue {
        id: radius,
        value: 8.0,
    })
    .unwrap();
    let after = arc_of(&doc, a);
    assert!((after.radius - 8.0).abs() <= 1e-7);
    // 始点は動かず、中心と終点が動く。
    assert!(dist(after.start_point(), before.start_point()) <= 1e-7);
    assert!(dist(after.center, before.center) > 1.0);
    assert!(dist(after.end_point(), before.end_point()) > 1.0);
    assert_all_satisfied(&doc);
    assert!(doc.undo());
    assert_eq!(arc_of(&doc, a), before);
}

#[test]
fn tangent_with_center_on_the_line_solves_to_the_left_side() {
    for hold_line in [true, false] {
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
        let cmd = if hold_line {
            add_holding(tangent, l)
        } else {
            add_constraint(tangent)
        };
        doc.apply(cmd).unwrap();
        let (seg, round) = (line_of(&doc, l), circle_of(&doc, c));
        let d = seg.direction();
        let signed = d.cross(round.center - seg.a) / d.length();
        // 中心から直線までの距離 = 半径で、中心は左側（符号付き距離が正）。
        assert!((signed - round.radius).abs() <= 1e-7, "signed {signed}");
        assert!(signed > 1.0, "signed {signed}");
        assert_all_satisfied(&doc);
        if hold_line {
            assert_eq!(geom(&doc, l), line_geom((-10.0, 0.0), (10.0, 0.0)));
        }
    }
}

// ---------------------------------------------------------------------------
// 9. 許容値（設計判断1）
// ---------------------------------------------------------------------------

/// 原点を `origin` に、大きさを `scale` 倍にした同じ形で、一連の編集の受理・拒否を記録する。
fn tolerance_scenario(origin: f64, scale: f64) -> Vec<String> {
    let at = |x: f64, y: f64| (origin + x * scale, origin + y * scale);
    let mut doc = Document::new();
    let corners = [at(0.0, 0.0), at(40.0, 0.0), at(40.0, 30.0), at(0.0, 30.0)];
    let sides: [EntityId; 4] =
        std::array::from_fn(|i| line(&mut doc, corners[i], corners[(i + 1) % 4]));
    let mut outcomes = Vec::new();
    let mut record = |doc: &mut Document, what: &str, cmd: Command| {
        let before = snapshot(doc);
        let outcome = match doc.apply(cmd) {
            Ok(_) => {
                assert_all_satisfied(doc);
                "ok".to_owned()
            }
            Err(err) => {
                assert_eq!(snapshot(doc), before, "{what}");
                // 種別だけを比べる（ID は同じ順で発行されるが、文言には座標が入りうる）。
                format!("{err:?}")
                    .split('(')
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            }
        };
        outcomes.push(format!("{what}: {outcome}"));
    };
    for i in 0..4 {
        record(
            &mut doc,
            "coincident",
            add_constraint(Constraint::Coincident {
                a: end(sides[i]),
                b: start(sides[(i + 1) % 4]),
            }),
        );
    }
    for (i, side) in sides.iter().enumerate() {
        let c = if i % 2 == 0 {
            Constraint::Horizontal { line: *side }
        } else {
            Constraint::Vertical { line: *side }
        };
        record(&mut doc, "axis", add_constraint(c));
    }
    // 追従（解ける）。
    let cmd = translated(&doc, sides[0], (3.0 * scale, -5.0 * scale));
    record(&mut doc, "translate a side", cmd);
    // 辺の長さを値で変える（解ける）。
    let width = Constraint::distance(&doc, start(sides[0]), end(sides[0])).unwrap();
    record(&mut doc, "width", add_constraint(width));
    let width_id = doc.constraints().last().unwrap().0;
    record(
        &mut doc,
        "set width",
        Command::SetConstraintValue {
            id: width_id,
            value: 55.0 * scale,
        },
    );
    // 水平な辺の両端を動かして傾ける。許容値（1e-9 × 長さ 55·scale）の半分のずれは
    // 「満たされたまま」で通り、4 倍のずれは拒否される。
    let tol = 1e-9 * 55.0 * scale;
    for (what, tilt) in [
        ("tilt within tolerance", 0.5),
        ("tilt beyond tolerance", 4.0),
    ] {
        let l = line_of(&doc, sides[0]);
        let shift = 0.25 * scale;
        let tilted = LineSeg::new(
            Point2::new(l.a.x + shift, l.a.y),
            Point2::new(l.b.x + shift, l.b.y + tilt * tol),
        );
        // 傾けた辺につながる辺も一緒に動かさないと一致で拒否されるので、一致を外した
        // 単独の水平線で試す。
        let probe = add(&mut doc, EntityGeom::Shape(Shape::Line(l)));
        record(
            &mut doc,
            "probe horizontal",
            add_constraint(Constraint::Horizontal { line: probe }),
        );
        record(
            &mut doc,
            what,
            modify(probe, EntityGeom::Shape(Shape::Line(tilted))),
        );
    }
    // 満たせない編集（水平な辺の回転、矛盾する拘束）。
    let pivot = line_of(&doc, sides[0]).a;
    let rotated = geom(&doc, sides[0]).rotated(pivot, 0.3);
    // 回転は始点を動かさないので、始点も動かす。
    let rotated = rotated.translated(Vec2::new(scale, 0.0));
    record(
        &mut doc,
        "rotate a horizontal side",
        modify(sides[0], rotated),
    );
    record(
        &mut doc,
        "contradiction",
        add_constraint(Constraint::Vertical { line: sides[0] }),
    );
    // 円と接線。
    let c = circle(&mut doc, at(20.0, 60.0), 7.0 * scale);
    let tangent = Constraint::tangent_line_circle(&doc, sides[2], c).unwrap();
    record(&mut doc, "tangent", add_holding(tangent, sides[2]));
    record(
        &mut doc,
        "radius",
        add_constraint(Constraint::Radius {
            entity: c,
            value: 9.0 * scale,
        }),
    );
    outcomes
}

#[test]
fn decisions_do_not_depend_on_the_distance_from_the_origin() {
    let near = tolerance_scenario(0.0, 1.0);
    let far = tolerance_scenario(1e6, 1.0);
    assert_eq!(near, far);
    // 受理と拒否の両方を含むシナリオであること。
    let count = |needle: &str| near.iter().filter(|o| o.ends_with(needle)).count();
    assert_eq!(count(": ok"), near.len() - 3, "{near:#?}");
    assert!(near.contains(&"tilt within tolerance: ok".to_owned()));
    assert!(near.contains(&"tilt beyond tolerance: ConstraintBlockedByFixed".to_owned()));
    assert!(near.contains(&"rotate a horizontal side: ConstraintBlockedByFixed".to_owned()));
    assert!(
        near.iter()
            .any(|o| o.starts_with("contradiction: ") && !o.ends_with(": ok")),
        "{near:#?}"
    );
}

#[test]
fn tiny_and_huge_shapes_both_solve() {
    let unit = tolerance_scenario(0.0, 1.0);
    assert_eq!(tolerance_scenario(0.0, 1e-3), unit);
    assert_eq!(tolerance_scenario(0.0, 1e5), unit);
}

#[test]
fn stretching_a_shape_does_not_count_as_satisfying() {
    // a の終点と b の始点はどちらも点の固定で止まっていて、1e-6 mm 離れている。長さ 10 mm の
    // 線分に対する許容値は 1e-8 mm なので、一致は満たせない。a の始点・b の終点は自由で、
    // 線分を 100 倍に伸ばせば許容値（代表長さに比例）が 1e-6 mm になり、残差を減らさずに
    // 「満たした」ことにできてしまう — その経路が受理されないこと。
    let gap = 1e-6;
    let build = |a_start: f64| {
        let mut doc = Document::new();
        let a = line(&mut doc, (a_start, 0.0), (10.0, 0.0));
        let b = line(&mut doc, (10.0 + gap, 0.0), (20.0, 0.0));
        (doc, a, b)
    };
    let (mut doc, a, b) = build(0.0);
    doc.constrain(Constraint::fix_point(&doc, end(a)).unwrap());
    doc.constrain(Constraint::fix_point(&doc, start(b)).unwrap());
    let coincident = Constraint::Coincident {
        a: end(a),
        b: start(b),
    };
    assert_rejected(&mut doc, add_constraint(coincident.clone()), |e| {
        matches!(e, CoreError::ConstraintUnsolvable(_))
    });
    assert_rejected(&mut doc, add_holding(coincident.clone(), a), |e| {
        matches!(e, CoreError::ConstraintUnsolvable(_))
    });

    // 対照: はじめから長い（代表長さ 2000 mm、許容値 2e-6 mm）線分なら、同じ食い違いは
    // 許容値の内で、解かずに受理される。伸ばす経路が「受理されうる形」へ通じていたことの確認。
    let (mut doc, a, b) = build(-1990.0);
    doc.constrain(Constraint::fix_point(&doc, end(a)).unwrap());
    doc.constrain(Constraint::fix_point(&doc, start(b)).unwrap());
    let before = all_geoms(&doc);
    doc.apply(add_constraint(Constraint::Coincident {
        a: end(a),
        b: start(b),
    }))
    .unwrap();
    assert_eq!(all_geoms(&doc), before);
}

/// 矩形と、その 1 辺に接する円、1 辺に長さ拘束。乱数で選んだ編集を順に適用する。
fn proptest_scene() -> (Document, Vec<EntityId>) {
    let mut doc = Document::new();
    let rect = rectangle(&mut doc);
    let round = circle(&mut doc, (20.0, 37.0), 7.0);
    doc.constrain(Constraint::tangent_line_circle(&doc, rect.sides[2], round).unwrap());
    doc.constrain(Constraint::distance(&doc, start(rect.sides[0]), end(rect.sides[0])).unwrap());
    let free = line(&mut doc, (60.0, 0.0), (70.0, 10.0));
    let mut ids = rect.sides.to_vec();
    ids.extend([round, free]);
    (doc, ids)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// `apply` が受理した形は全拘束が `is_satisfied` を満たし（= 保存して読み込むと充足検査を
    /// 通る）、拒否したら状態は変わらない。
    #[test]
    fn accepted_edits_keep_every_constraint_satisfied(
        edits in proptest::collection::vec(
            (0usize..6, 0u8..4, -20.0f64..20.0, -20.0f64..20.0, -1.0f64..1.0),
            1..8,
        ),
    ) {
        let (mut doc, ids) = proptest_scene();
        for (which, kind, dx, dy, angle) in edits {
            let id = ids[which];
            let current = geom(&doc, id);
            let new_geom = match (kind, current.as_shape()) {
                // まるごと移動。
                (0, _) => current.translated(Vec2::new(dx, dy)),
                // 回転。
                (1, _) => current.rotated(Point2::new(dx, dy), angle),
                // 片方の端点（円は半径）だけを動かす。
                (2, Some(Shape::Line(l))) => {
                    line_geom((l.a.x, l.a.y), (l.b.x + dx, l.b.y + dy))
                }
                (_, Some(Shape::Line(l))) => {
                    line_geom((l.a.x + dx, l.a.y + dy), (l.b.x, l.b.y))
                }
                (_, Some(Shape::Circle(c))) => {
                    circle_geom((c.center.x, c.center.y), c.radius + dx.abs())
                }
                _ => current.clone(),
            };
            let before = snapshot(&doc);
            match doc.apply(modify(id, new_geom.clone())) {
                Ok(_) => {
                    assert_all_satisfied(&doc);
                    // 利用者が動かした点はそのまま残る。線分をまるごと平行移動したときは、
                    // 両端とも動いているので全体がそのまま（円は半径が自由なまま）。
                    let whole_line = kind == 0
                        && (dx != 0.0 || dy != 0.0)
                        && matches!(current.as_shape(), Some(Shape::Line(_)));
                    if whole_line {
                        prop_assert_eq!(geom(&doc, id), new_geom);
                    }
                }
                Err(_) => prop_assert_eq!(snapshot(&doc), before),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 10. 退化
// ---------------------------------------------------------------------------

#[test]
fn edits_that_collapse_a_shape_are_rejected() {
    // 線分の長さ: 固定した線分の端点に、別の線分の両端を一致させる（1 点につぶれる）。
    let mut doc = Document::new();
    let anchor = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    let m = line(&mut doc, (10.0, 0.0), (10.0, 5.0));
    doc.constrain(Constraint::FixEntity { entity: anchor });
    doc.constrain(Constraint::Coincident {
        a: start(m),
        b: end(anchor),
    });
    assert_rejected(
        &mut doc,
        add_constraint(Constraint::Coincident {
            a: end(m),
            b: end(anchor),
        }),
        |e| *e == CoreError::DegenerateShape(m),
    );
    // 自由な線分の両端を一致させる（最小ノルム解は中点へつぶれる）。
    let flat = line(&mut doc, (0.0, 10.0), (8.0, 10.0));
    assert_rejected(
        &mut doc,
        add_constraint(Constraint::Coincident {
            a: start(flat),
            b: end(flat),
        }),
        |e| *e == CoreError::DegenerateShape(flat),
    );
    // 水平な線分に垂直を足しても、つぶれない（向きの拘束は角度で測るので、線分は回る。
    // 以前は端点の x の差を残差にしていて、最小ノルム解が長さ 0 になった）。
    doc.apply(add_constraint(Constraint::Vertical { line: flat }))
        .unwrap();
    assert!(length(line_of(&doc, flat)) >= 8.0);
    assert_all_satisfied(&doc);

    // 半径: 中心が直線上に固定された円を、その直線に接させる（半径が 0 になる）。
    let round = circle(&mut doc, (5.0, 0.0), 3.0);
    doc.constrain(Constraint::fix_point(&doc, center(round)).unwrap());
    let tangent = Constraint::tangent_line_circle(&doc, anchor, round).unwrap();
    assert_rejected(&mut doc, add_constraint(tangent), |e| {
        *e == CoreError::DegenerateShape(round)
    });

    // 円弧の掃引: 始点と終点を一致させる（掃引が 0 になるか、半径が 0 になる）。
    let a = add(&mut doc, arc_geom((30.0, 30.0), 5.0, 0.2, 1.0));
    assert_rejected(
        &mut doc,
        add_constraint(Constraint::Coincident {
            a: start(a),
            b: end(a),
        }),
        |e| *e == CoreError::DegenerateShape(a),
    );
    // 優弧でも同じ（掃引が 2π へ達する）。
    let major = add(&mut doc, arc_geom((60.0, 30.0), 5.0, 0.2, 6.0));
    assert_rejected(
        &mut doc,
        add_constraint(Constraint::Coincident {
            a: start(major),
            b: end(major),
        }),
        |e| *e == CoreError::DegenerateShape(major),
    );
}

#[test]
fn arc_sweep_is_not_flipped_through_zero() {
    // 掃引 0.2 rad の円弧の終点を、始点の「手前」側にある点へ一致させる。掃引が 0 を越えて
    // 補弧（ほぼ 1 周の円弧）へ飛び移る形は受け入れない: 受理されるなら、掃引は連続に追った
    // 範囲 (0, 2π) の中で、元の掃引から大きく離れていない。
    let mut doc = Document::new();
    let a = add(&mut doc, arc_geom((0.0, 0.0), 10.0, 1.0, 1.2));
    doc.constrain(Constraint::fix_point(&doc, center(a)).unwrap());
    doc.constrain(Constraint::fix_point(&doc, start(a)).unwrap());
    doc.constrain(Constraint::radius(&doc, a).unwrap());
    // 終点の行き先: 角度 0.7（始点 1.0 より 0.3 rad 手前）の円周上の点を固定した線分の端点。
    let target = line(
        &mut doc,
        (10.0 * 0.7f64.cos(), 10.0 * 0.7f64.sin()),
        (30.0, 30.0),
    );
    doc.constrain(Constraint::FixEntity { entity: target });
    let before = snapshot(&doc);
    let result = doc.apply(add_constraint(Constraint::Coincident {
        a: end(a),
        b: start(target),
    }));
    // 中心・始点・半径が止まっているので、終了角を 1.2 → 0.7 へ下げる（掃引 −0.3）しか
    // 満たす形が無い。掃引が 0 以下になるので退化として拒否される。
    assert_eq!(result, Err(CoreError::DegenerateShape(a)));
    assert_eq!(snapshot(&doc), before);

    // 掃引が正のまま届く行き先（角度 2.0）なら通る。
    let reachable = line(
        &mut doc,
        (10.0 * 2.0f64.cos(), 10.0 * 2.0f64.sin()),
        (-30.0, 30.0),
    );
    doc.constrain(Constraint::FixEntity { entity: reachable });
    doc.apply(add_constraint(Constraint::Coincident {
        a: end(a),
        b: start(reachable),
    }))
    .unwrap();
    let solved = arc_of(&doc, a);
    assert!((solved.sweep() - 1.0).abs() <= 1e-7, "{solved:?}");
    assert_all_satisfied(&doc);
}

#[test]
fn internal_tangency_cannot_become_concentric_and_equal() {
    let mut doc = Document::new();
    let big = circle(&mut doc, (0.0, 0.0), 10.0);
    let small = circle(&mut doc, (7.0, 0.0), 3.0);
    let inner = doc.constrain(Constraint::tangent_circles(&doc, big, small).unwrap());
    let rejected = |e: &CoreError| *e == CoreError::DegenerateConstraint(inner);
    // 同心・等半径（残差は 0）。
    assert_rejected(
        &mut doc,
        modify(small, circle_geom((0.0, 0.0), 10.0)),
        rejected,
    );
    // 両方を動かして同じ円にする Batch も同じ。
    let batch = Command::Batch(vec![
        modify(big, circle_geom((3.0, 1.0), 5.0)),
        modify(small, circle_geom((3.0, 1.0), 5.0)),
    ]);
    assert_rejected(&mut doc, batch, rejected);
    // 大小が逆転する向きの変更: 外側の円を半径 1 にし、内側の円は中心だけを動かす（半径 3 は
    // 変えていないので自由）。満たすには内側の円の半径を負へ向かわせるしかなく、途中の試行点が
    // 退化で棄却されて行き詰まる（「退化した形を経由しないと進めない」経路の報告）。
    let batch = Command::Batch(vec![
        modify(big, circle_geom((7.0, 0.0), 1.0)),
        modify(small, circle_geom((5.0, 0.0), 3.0)),
    ]);
    assert_rejected(&mut doc, batch, |e| *e == CoreError::DegenerateShape(small));
    // 構造条件を保つ変更は通る。
    doc.apply(modify(small, circle_geom((0.0, 6.0), 4.0)))
        .unwrap();
    assert_all_satisfied(&doc);

    // 解いた結果として同じ円になる場合: 外側の円を固定し、内側の円の半径の値を外側と同じに
    // 変える。満たす形は同心・等半径しか無いので拒否される。
    doc.constrain(Constraint::FixEntity { entity: big });
    let radius = doc.constrain(Constraint::radius(&doc, small).unwrap());
    let before = snapshot(&doc);
    let err = doc
        .apply(Command::SetConstraintValue {
            id: radius,
            value: 10.0,
        })
        .unwrap_err();
    assert_eq!(snapshot(&doc), before);
    assert_eq!(err, CoreError::DegenerateConstraint(inner));
}

// ---------------------------------------------------------------------------
// 11. グリップ相当
// ---------------------------------------------------------------------------

#[test]
fn dragging_one_end_keeps_the_length_by_moving_the_other_end() {
    let mut doc = Document::new();
    let l = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    doc.constrain(Constraint::distance(&doc, start(l), end(l)).unwrap());

    // 終点だけを (13, 4) へ動かす。始点は bit 単位で同じなので自由で、長さ 10 を保つように動く。
    doc.apply(modify(l, line_geom((0.0, 0.0), (13.0, 4.0))))
        .unwrap();
    let solved = line_of(&doc, l);
    assert_eq!(solved.b, Point2::new(13.0, 4.0));
    assert!((length(solved) - 10.0).abs() <= 1e-7, "{solved:?}");
    // 最小ノルム: 始点は、元の位置から終点へ向かう直線に沿って動く。
    let along = Vec2::new(13.0, 4.0);
    assert!(along.cross(solved.a - Point2::new(0.0, 0.0)).abs() <= 1e-6);
    assert!(doc.undo());
    assert_eq!(geom(&doc, l), line_geom((0.0, 0.0), (10.0, 0.0)));

    // 始点だけを動かせば、終点が動く。
    doc.apply(modify(l, line_geom((0.0, -6.0), (10.0, 0.0))))
        .unwrap();
    let solved = line_of(&doc, l);
    assert_eq!(solved.a, Point2::new(0.0, -6.0));
    assert!((length(solved) - 10.0).abs() <= 1e-7);
    assert_ne!(solved.b, Point2::new(10.0, 0.0));
    assert!(doc.undo());

    // 水平に動かした点は y が bit 単位で同じでも、点として止められる（x だけでなく）。
    doc.constrain(Constraint::Horizontal { line: l });
    doc.apply(modify(l, line_geom((0.0, 0.0), (14.0, 0.0))))
        .unwrap();
    assert_eq!(line_of(&doc, l).b, Point2::new(14.0, 0.0));
    assert_close(&geom(&doc, l), &line_geom((4.0, 0.0), (14.0, 0.0)), 1e-7);

    // 円弧: 半径だけを変えると、中心・角度は自由のまま。半径拘束があれば拒否。
    let a = add(&mut doc, arc_geom((0.0, 20.0), 5.0, 0.0, 1.0));
    let radius = doc.constrain(Constraint::radius(&doc, a).unwrap());
    assert_rejected(
        &mut doc,
        modify(a, arc_geom((0.0, 20.0), 6.0, 0.0, 1.0)),
        |e| *e == CoreError::ConstraintBlockedByFixed(radius),
    );
    // 中心を動かすのは通る（半径は変わっていない）。
    doc.apply(modify(a, arc_geom((1.0, 21.0), 5.0, 0.0, 1.0)))
        .unwrap();
}

/// 一致と長さでつないだ線分 `count` 本の連鎖（一致 `count − 1`・各線分の長さ `count`）。
/// 線分は x 方向へ 10 mm ずつ進み、`zigzag` なら 1 本おきに y = 5 へ上がる。
fn chain(doc: &mut Document, count: u32, zigzag: bool) -> Vec<EntityId> {
    let node = |i: u32| {
        let y = if zigzag && i % 2 == 1 { 5.0 } else { 0.0 };
        (10.0 * f64::from(i), y)
    };
    let links: Vec<EntityId> = (0..count)
        .map(|i| line(doc, node(i), node(i + 1)))
        .collect();
    for pair in links.windows(2) {
        doc.constrain(Constraint::Coincident {
            a: end(pair[0]),
            b: start(pair[1]),
        });
    }
    for &link in &links {
        let length = Constraint::distance(doc, start(link), end(link)).unwrap();
        doc.constrain(length);
    }
    links
}

/// 連鎖の先頭の線分を編集する（`drag_end_only` なら終点だけを引く。そうでなければ平行移動）。
/// [`Document::apply`] が受理し、残りが追従して全拘束を満たし、undo 1 回で戻ることを確かめる。
fn apply_chain_edit(count: u32, zigzag: bool, drag_end_only: bool, by: (f64, f64)) {
    let (mut doc, links, cmd, edited) = chain_edit(count, zigzag, drag_end_only, by);
    let before = all_geoms(&doc);
    doc.apply(cmd)
        .unwrap_or_else(|e| panic!("{count} links, zigzag {zigzag}, by {by:?}: {e:?}"));
    assert_all_satisfied(&doc);
    assert_eq!(doc.constraint_count(), 2 * links.len() - 1);
    // 利用者が動かした点はそのまま（終点だけを引いたときの始点は、長さを保つために動く）。
    let solved = line_of(&doc, links[0]);
    assert_eq!(solved.b, edited.b);
    if !drag_end_only {
        assert_eq!(solved.a, edited.a);
    }
    // 残りの線分が追従している（動かした線分のほかにも形が変わる）。
    assert!(follow_up_count(&doc) > 0);
    assert!(doc.undo());
    assert_eq!(all_geoms(&doc), before);
}

/// [`apply_chain_edit`] と同じ編集の、ソルバーの試行回数の合計（文書は変えない。
/// `apply` は試行回数を返さないので、同じ手順を分けて行って読む）。
fn chain_edit_trials(count: u32, zigzag: bool, drag_end_only: bool, by: (f64, f64)) -> usize {
    let (mut doc, _, cmd, _) = chain_edit(count, zigzag, drag_end_only, by);
    let outcome = doc.execute(cmd).unwrap();
    let mut edits = Edits::default();
    edits.collect(&outcome.applied);
    let trials = doc
        .solve_constraints(&edits)
        .unwrap_or_else(|e| panic!("{count} links, zigzag {zigzag}, by {by:?}: {e:?}"))
        .trials;
    doc.revert(&outcome.applied);
    trials
}

/// 連鎖と、その先頭の線分の編集（文書・連鎖・コマンド・編集後の先頭の線分）。
fn chain_edit(
    count: u32,
    zigzag: bool,
    drag_end_only: bool,
    by: (f64, f64),
) -> (Document, Vec<EntityId>, Command, LineSeg) {
    let mut doc = Document::new();
    let links = chain(&mut doc, count, zigzag);
    let first = line_of(&doc, links[0]);
    let shift = Vec2::new(by.0, by.1);
    let edited = if drag_end_only {
        LineSeg::new(first.a, first.b + shift)
    } else {
        LineSeg::new(first.a + shift, first.b + shift)
    };
    let cmd = modify(links[0], EntityGeom::Shape(Shape::Line(edited)));
    (doc, links, cmd, edited)
}

#[test]
fn far_edits_of_a_long_chain_are_solved() {
    // 解から遠い大きな系。既定の初期減衰はほぼ 0 で、1 回目のステップが行き過ぎる。それで
    // 停滞・試行回数の上限に達したら、同じ開始状態を大きい初期減衰
    // （`mcad_geom::solve::LAMBDA_RETRY`）で解き直す。
    use mcad_geom::solve::MAX_TRIALS;

    // 既定の初期減衰のまま解ける事例（解き直しに入らない。実測: 25 回・54 回）: ジグザグの
    // 連鎖の先頭の終点を引く、直線の連鎖の先頭を平行移動する。
    for (zigzag, drag_end_only) in [(true, true), (false, false)] {
        apply_chain_edit(20, zigzag, drag_end_only, (30.0, 40.0));
        let trials = chain_edit_trials(20, zigzag, drag_end_only, (30.0, 40.0));
        assert!(trials <= MAX_TRIALS, "zigzag {zigzag}: {trials}");
    }

    // 直線に並べた 24 本の連鎖の先頭を (90, 120) 平行移動する: 既定の初期減衰では上限に
    // 達し、解き直しで余裕を持って解ける（実測: 200 回で打ち切り → 解き直し 120 回）。
    // 試行回数の合計が上限を超えていることで、解き直しを通ったことを確かめる。
    apply_chain_edit(24, false, false, (90.0, 120.0));
    let trials = chain_edit_trials(24, false, false, (90.0, 120.0));
    assert!(trials > MAX_TRIALS, "solved without the retry: {trials}");
    assert!(trials < 2 * MAX_TRIALS - 40, "no margin left: {trials}");
}

/// 初期減衰を 1e-9 へ下げたときに拒否へ変わった事例（Codex adversarial review の指摘）:
/// ジグザグの 50 本の連鎖の先頭の終点を (0, 100) 引く。解き直しで受理される。
///
/// 通常のテストから外してある: 試行回数の上限の境目にあり（実測: 引く量 99 mm は既定の
/// 初期減衰のまま 196 回、100 mm は既定で打ち切り → 解き直しがちょうど上限の 200 回目に
/// 収束、101 mm は解き直しも打ち切り）、数学関数の丸めが違う環境では拒否になりうる。
/// 400 試行ぶんで、debug ビルドでは約 15 秒かかる。`cargo test -- --ignored` で実行する。
#[test]
#[ignore = "slow in debug builds (about 15 s) and sits exactly on the solver's trial limit"]
fn chain_edit_on_the_trial_limit_is_solved_by_the_retry() {
    apply_chain_edit(50, true, true, (0.0, 100.0));
}

// ---------------------------------------------------------------------------
// 12. 自由度（検収 x）
// ---------------------------------------------------------------------------

#[test]
fn degrees_of_freedom_of_connected_components() {
    let mut doc = Document::new();
    // 拘束の無い図形はパラメータ数。
    let l = line(&mut doc, (100.0, 0.0), (110.0, 0.0));
    let c = circle(&mut doc, (100.0, 20.0), 3.0);
    let a = add(&mut doc, arc_geom((100.0, 40.0), 5.0, 0.3, 2.0));
    assert_eq!(doc.degrees_of_freedom(l), Some(4));
    assert_eq!(doc.degrees_of_freedom(c), Some(3));
    assert_eq!(doc.degrees_of_freedom(a), Some(5));
    // 拘束の対象にできない図形・存在しない図形は None。
    let polyline = add(
        &mut doc,
        EntityGeom::Shape(Shape::Polyline(Polyline::new(
            vec![Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)],
            false,
        ))),
    );
    assert_eq!(doc.degrees_of_freedom(polyline), None);
    doc.apply(Command::RemoveEntity(polyline)).unwrap();
    assert_eq!(doc.degrees_of_freedom(polyline), None);

    // 矩形: 16 パラメータ − 12 本 = 4（位置 2・幅・高さ）。どの辺で聞いても同じ。
    let rect = rectangle(&mut doc);
    for side in rect.sides {
        assert_eq!(doc.degrees_of_freedom(side), Some(4));
    }
    // 拘束のある文書でも、拘束の無い図形はパラメータ数のまま。
    assert_eq!(doc.degrees_of_freedom(l), Some(4));
    // 角を固定すると 2、幅と高さを決めると 0（完全に拘束）。
    doc.constrain(Constraint::fix_point(&doc, start(rect.sides[0])).unwrap());
    assert_eq!(doc.degrees_of_freedom(rect.sides[2]), Some(2));
    doc.constrain(Constraint::distance(&doc, start(rect.sides[0]), end(rect.sides[0])).unwrap());
    assert_eq!(doc.degrees_of_freedom(rect.sides[2]), Some(1));
    let height = doc
        .constrain(Constraint::distance(&doc, start(rect.sides[1]), end(rect.sides[1])).unwrap());
    for side in rect.sides {
        assert_eq!(doc.degrees_of_freedom(side), Some(0));
    }
    // 矛盾しない重複（向かいの辺にも長さ）を足しても 0 のまま。
    doc.constrain(Constraint::distance(&doc, start(rect.sides[2]), end(rect.sides[2])).unwrap());
    assert_eq!(doc.degrees_of_freedom(rect.sides[0]), Some(0));
    doc.apply(Command::RemoveConstraint(height)).unwrap();
    assert_eq!(doc.degrees_of_freedom(rect.sides[0]), Some(1));

    // 点の固定を付けた円弧は 5 − 2 = 3。
    doc.constrain(Constraint::fix_point(&doc, start(a)).unwrap());
    assert_eq!(doc.degrees_of_freedom(a), Some(3));
    // 図形の固定は 0。
    doc.constrain(Constraint::FixEntity { entity: c });
    assert_eq!(doc.degrees_of_freedom(c), Some(0));

    // 一致でつないだ 2 線分は 4 + 4 − 2 = 6。片方をロックされたレイヤーへ移すと、
    // その 4 パラメータが止まって 4 − 2 = 2。
    let m = line(&mut doc, (110.0, 0.0), (120.0, 10.0));
    doc.constrain(Constraint::Coincident {
        a: end(l),
        b: start(m),
    });
    assert_eq!(doc.degrees_of_freedom(l), Some(6));
    assert_eq!(doc.degrees_of_freedom(m), Some(6));
    lock_on_new_layer(&mut doc, m);
    assert_eq!(doc.degrees_of_freedom(l), Some(2));
    // 拘束の無い図形も、ロックされたレイヤーの上なら 0。
    let lonely = line(&mut doc, (0.0, 100.0), (5.0, 100.0));
    assert_eq!(doc.degrees_of_freedom(lonely), Some(4));
    lock_on_new_layer(&mut doc, lonely);
    assert_eq!(doc.degrees_of_freedom(lonely), Some(0));
}

// ---------------------------------------------------------------------------
// 13. 拘束と無関係な編集
// ---------------------------------------------------------------------------

#[test]
fn edits_outside_a_component_leave_it_bit_identical() {
    let mut doc = Document::new();
    let rect = rectangle(&mut doc);
    // 別の成分（一致でつないだ 2 線分）と、拘束の無い線分。
    let p = line(&mut doc, (100.0, 0.1), (110.0, 0.3));
    let q = line(&mut doc, (110.0, 0.3), (120.0, 7.7));
    doc.constrain(Constraint::Coincident {
        a: end(p),
        b: start(q),
    });
    let free = line(&mut doc, (0.0, 100.0), (10.0, 101.0));
    let others = |doc: &Document, except: &[EntityId]| -> Vec<(EntityId, EntityGeom)> {
        all_geoms(doc)
            .into_iter()
            .filter(|(id, _)| !except.contains(id))
            .collect()
    };

    // 拘束の無い線分の編集（種類の変更を含む）: ほかは bit 単位で変わらず、追従も無い。
    let before = others(&doc, &[free]);
    for new_geom in [
        line_geom((1.0, 99.0), (30.0, 80.0)),
        circle_geom((5.0, 100.0), 2.0),
        line_geom((0.0, 100.0), (10.0, 101.0)),
    ] {
        doc.apply(modify(free, new_geom.clone())).unwrap();
        assert_eq!(geom(&doc, free), new_geom);
        assert_eq!(others(&doc, &[free]), before);
        assert!(matches!(
            &doc.undo_stack.last().unwrap().applied,
            Applied::ModifyEntity { .. }
        ));
    }
    // 追加・削除・スタイル変更も同じ。
    let extra = line(&mut doc, (5.0, 5.0), (6.0, 6.0));
    doc.apply(Command::RemoveEntity(extra)).unwrap();
    assert_eq!(others(&doc, &[free]), before);

    // ある成分の編集は、別の成分を動かさない。
    let before = others(&doc, &[p, q]);
    doc.apply(translated(&doc, p, (0.0, 4.0))).unwrap();
    assert_eq!(others(&doc, &[p, q]), before);
    assert!(dist(line_of(&doc, q).a, line_of(&doc, p).b) <= 1e-7);
    // 成分の中でも、拘束が参照しないパラメータ（q の終点）は動かない。
    assert_eq!(line_of(&doc, q).b, Point2::new(120.0, 7.7));
    // 矩形は最初から最後まで動いていない。
    assert_eq!(line_of(&doc, rect.sides[0]).a, Point2::new(0.0, 0.0));
}

// ---------------------------------------------------------------------------
// ReplaceConstraint
// ---------------------------------------------------------------------------

#[test]
fn replace_constraint_keeps_the_id_and_solves() {
    let mut doc = Document::new();
    let l = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    let c = circle(&mut doc, (5.0, 3.0), 3.0);
    let horizontal = doc.constrain(Constraint::Horizontal { line: l });
    let tangent = doc.constrain(Constraint::tangent_line_circle(&doc, l, c).unwrap());
    doc.constrain(Constraint::radius(&doc, c).unwrap());
    doc.constrain(Constraint::fix_point(&doc, start(l)).unwrap());
    let replace = |id, constraint| Command::ReplaceConstraint { id, constraint };

    // 同じ拘束への差し替えは意味的 no-op（履歴・世代に触れない）。
    let before = snapshot(&doc);
    let same = doc.constraint(tangent).unwrap().clone();
    assert_eq!(doc.apply(replace(tangent, same)), Ok(NewIds::default()));
    assert_eq!(snapshot(&doc), before);

    // 存在しない ID。
    let ghost = {
        let id = doc.constrain(Constraint::Horizontal { line: l });
        doc.apply(Command::RemoveConstraint(id)).unwrap();
        id
    };
    assert_rejected(
        &mut doc,
        replace(ghost, Constraint::Horizontal { line: l }),
        |e| *e == CoreError::ConstraintNotFound(ghost),
    );
    // 構造の検証は AddConstraint と同じ。
    assert_rejected(
        &mut doc,
        replace(horizontal, Constraint::Horizontal { line: c }),
        invalid,
    );
    assert_rejected(
        &mut doc,
        replace(
            horizontal,
            Constraint::Distance {
                a: start(l),
                b: end(l),
                value: 0.0,
            },
        ),
        invalid,
    );

    // 側を反対にすると満たされなくなり、解かれる（円が直線の反対側へ移る）。ID は同じ。
    let before_geoms = all_geoms(&doc);
    let flipped = Constraint::TangentLineCircle {
        line: l,
        circle: c,
        side: Side::Right,
    };
    let ids = doc.apply(replace(tangent, flipped.clone())).unwrap();
    assert_eq!(ids, NewIds::default());
    assert_eq!(doc.constraint(tangent), Some(&flipped));
    assert_eq!(doc.constraint_count(), 4);
    assert_all_satisfied(&doc);
    let seg = line_of(&doc, l);
    let round = circle_of(&doc, c);
    assert!(seg.direction().cross(round.center - seg.a) < 0.0);
    // undo 1 回で拘束と形が戻り、redo で同じ状態。
    let solved = snapshot(&doc);
    assert!(doc.undo());
    assert_eq!(all_geoms(&doc), before_geoms);
    assert_eq!(
        doc.constraint(tangent),
        Some(&Constraint::TangentLineCircle {
            line: l,
            circle: c,
            side: Side::Left
        })
    );
    assert!(doc.redo());
    assert_eq!(snapshot(&doc).entities, solved.entities);
    assert_eq!(snapshot(&doc).constraints, solved.constraints);
    assert!(doc.undo());

    // 種類・対象の違う拘束へも差し替えられる（水平 → 長さ 7）。
    doc.apply(replace(
        horizontal,
        Constraint::Distance {
            a: start(l),
            b: end(l),
            value: 7.0,
        },
    ))
    .unwrap();
    assert!((length(line_of(&doc, l)) - 7.0).abs() <= 1e-7);
    assert_all_satisfied(&doc);
    assert!(doc.undo());

    // 満たせない差し替えは拒否され、状態は変わらない（終点も固定してから、水平を垂直へ）。
    doc.constrain(Constraint::fix_point(&doc, end(l)).unwrap());
    assert_rejected(
        &mut doc,
        replace(horizontal, Constraint::Vertical { line: l }),
        |e| !invalid(e),
    );
    // 角度は AddConstraint と同じく正規化して格納する。
    let m = line(&mut doc, (0.0, 20.0), (0.0, 30.0));
    let angle = doc.constrain(Constraint::angle(&doc, l, m).unwrap());
    let value = doc.constraint(angle).unwrap().value().unwrap();
    doc.apply(replace(
        angle,
        Constraint::Angle {
            a: l,
            b: m,
            value: value + 4.0 * PI,
        },
    ))
    .unwrap();
    let stored = doc.constraint(angle).unwrap().value().unwrap();
    assert!((stored - value).abs() <= 1e-12, "{stored}");
}

// ---------------------------------------------------------------------------
// ミラー
// ---------------------------------------------------------------------------

/// ミラーで壊れる拘束（側・角度の符号・円弧の端点）をひととおり含む図形群。
struct MirrorScene {
    entities: Vec<EntityId>,
    arc: EntityId,
    line: EntityId,
    slanted: EntityId,
    round: EntityId,
}

fn mirror_scene(doc: &mut Document) -> MirrorScene {
    let l = line(doc, (0.0, 0.0), (10.0, 0.0));
    let round = circle(doc, (5.0, 3.0), 3.0);
    let slanted = line(doc, (2.0, 6.0), (8.0, 9.0));
    // 始点 (10, 0) が線分の終点に一致する円弧（中心 (10, 5)、−π/2 から 0.3 まで）。
    let arc = add(doc, arc_geom((10.0, 5.0), 5.0, -PI / 2.0, 0.3));
    doc.constrain(Constraint::tangent_line_circle(doc, l, round).unwrap());
    doc.constrain(Constraint::radius(doc, round).unwrap());
    doc.constrain(Constraint::point_line_distance(doc, start(slanted), l).unwrap());
    doc.constrain(Constraint::angle(doc, l, slanted).unwrap());
    doc.constrain(Constraint::Coincident {
        a: end(l),
        b: start(arc),
    });
    doc.constrain(Constraint::distance(doc, end(arc), end(slanted)).unwrap());
    assert_all_satisfied(doc);
    MirrorScene {
        entities: vec![l, round, slanted, arc],
        arc,
        line: l,
        slanted,
        round,
    }
}

const AXIS: (Point2, Point2) = (Point2::new(20.0, -5.0), Point2::new(23.0, 8.0));

/// `entities` をミラーする Batch。`replace` なら、変わる拘束の `ReplaceConstraint` も積む。
fn mirror_batch(doc: &Document, entities: &[EntityId], replace: bool) -> Command {
    let set: BTreeSet<EntityId> = entities.iter().copied().collect();
    let mut subs: Vec<Command> = entities
        .iter()
        .map(|&id| modify(id, geom(doc, id).mirrored(AXIS.0, AXIS.1)))
        .collect();
    if replace {
        for (id, constraint) in doc.constraints() {
            let mirrored = constraint.mirrored(doc, |e| set.contains(&e));
            if mirrored != *constraint {
                subs.push(Command::ReplaceConstraint {
                    id,
                    constraint: mirrored,
                });
            }
        }
    }
    Command::Batch(subs)
}

#[test]
fn mirroring_with_replaced_constraints_is_accepted_bit_for_bit() {
    let mut doc = Document::new();
    let scene = mirror_scene(&mut doc);
    let before = snapshot(&doc);
    let expected: Vec<EntityGeom> = scene
        .entities
        .iter()
        .map(|&id| geom(&doc, id).mirrored(AXIS.0, AXIS.1))
        .collect();

    let batch = mirror_batch(&doc, &scene.entities, true);
    // 側 2 件・角度 1 件・円弧の端点 2 件が差し替えの対象。
    let Command::Batch(subs) = &batch else {
        unreachable!()
    };
    let replaced = subs
        .iter()
        .filter(|c| matches!(c, Command::ReplaceConstraint { .. }))
        .count();
    assert_eq!(replaced, 5);

    doc.apply(batch).unwrap();
    // 幾何は `mirrored` の結果そのまま（ソルバーは何も動かさない）。
    let actual: Vec<EntityGeom> = scene.entities.iter().map(|&id| geom(&doc, id)).collect();
    assert_eq!(actual, expected);
    assert_all_satisfied(&doc);
    assert_eq!(doc.constraint_count(), 6);
    let mirrored = snapshot(&doc);

    // undo 1 回で図形と拘束がそろって戻り、redo で同じ状態。
    assert!(doc.undo());
    assert_eq!(snapshot(&doc).entities, before.entities);
    assert_eq!(snapshot(&doc).constraints, before.constraints);
    assert!(doc.redo());
    assert_eq!(snapshot(&doc).entities, mirrored.entities);
    assert_eq!(snapshot(&doc).constraints, mirrored.constraints);

    // もう一度ミラーすると元の拘束へ戻る（幾何は丸めの分だけ違いうる）。
    let back = mirror_batch(&doc, &scene.entities, true);
    doc.apply(back).unwrap();
    assert_eq!(snapshot(&doc).constraints, before.constraints);
    assert_all_satisfied(&doc);
    for ((id, original), now) in before.entities.iter().zip(snapshot(&doc).entities) {
        assert_eq!(*id, now.0);
        if original
            .geom
            .as_shape()
            .is_some_and(|s| !matches!(s, Shape::Arc(_)))
        {
            assert_close(&now.1.geom, &original.geom, 1e-9);
        }
    }
}

#[test]
fn mirroring_without_replacing_constraints_is_rejected() {
    let mut doc = Document::new();
    let scene = mirror_scene(&mut doc);
    let batch = mirror_batch(&doc, &scene.entities, false);
    // すべての点が「利用者が動かした」として止められるので、側・角度・円弧の端点が
    // 食い違ったままでは満たせない。
    assert_rejected(&mut doc, batch, blocked);
}

#[test]
fn mirroring_shapes_without_sides_or_angles_needs_no_replacement() {
    // 線分・円だけで、側も角度も持たない拘束の図形群。
    let mut doc = Document::new();
    let a = line(&mut doc, (0.0, 0.0), (10.0, 4.0));
    let b = line(&mut doc, (10.0, 4.0), (14.0, 12.0));
    let c = line(&mut doc, (2.0, 8.0), (12.0, 12.0));
    let big = circle(&mut doc, (30.0, 0.0), 6.0);
    let small = circle(&mut doc, (40.0, 0.0), 4.0);
    doc.constrain(Constraint::Coincident {
        a: end(a),
        b: start(b),
    });
    doc.constrain(Constraint::Parallel { a, b: c });
    doc.constrain(Constraint::EqualLength { a, b: c });
    doc.constrain(Constraint::distance(&doc, start(a), start(c)).unwrap());
    doc.constrain(Constraint::radius(&doc, big).unwrap());
    doc.constrain(Constraint::tangent_circles(&doc, big, small).unwrap());
    doc.constrain(Constraint::Coincident {
        a: center(big),
        b: start(a),
    });
    // 中心を一致させるために円が動いているかもしれないので、ここで形を読む。
    assert_all_satisfied(&doc);
    let entities = [a, b, c, big, small];
    let set: BTreeSet<EntityId> = entities.iter().copied().collect();
    for (_, constraint) in doc.constraints() {
        assert_eq!(&constraint.mirrored(&doc, |e| set.contains(&e)), constraint);
    }
    let expected: Vec<EntityGeom> = entities
        .iter()
        .map(|&id| geom(&doc, id).mirrored(AXIS.0, AXIS.1))
        .collect();
    let batch = mirror_batch(&doc, &entities, false);
    doc.apply(batch).unwrap();
    let actual: Vec<EntityGeom> = entities.iter().map(|&id| geom(&doc, id)).collect();
    assert_eq!(actual, expected);
    assert_all_satisfied(&doc);
}

#[test]
fn constraint_mirrored_follows_the_rules() {
    let mut doc = Document::new();
    let scene = mirror_scene(&mut doc);
    let (l, slanted, round, arc) = (scene.line, scene.slanted, scene.round, scene.arc);
    let outsider = line(&mut doc, (10.0, 0.0), (15.0, -5.0));
    let all = |_: EntityId| true;
    let none = |_: EntityId| false;
    let only = |wanted: EntityId| move |e: EntityId| e == wanted;

    // 何も集合に入っていなければ、どの拘束も変わらない。
    for (_, constraint) in doc.constraints() {
        assert_eq!(&constraint.mirrored(&doc, none), constraint);
    }

    // 側: 両方が集合内のときだけ反転する。
    let tangent = Constraint::TangentLineCircle {
        line: l,
        circle: round,
        side: Side::Left,
    };
    assert_eq!(
        tangent.mirrored(&doc, all),
        Constraint::TangentLineCircle {
            line: l,
            circle: round,
            side: Side::Right
        }
    );
    assert_eq!(tangent.mirrored(&doc, only(l)), tangent);
    assert_eq!(tangent.mirrored(&doc, only(round)), tangent);
    let offset = Constraint::PointLineDistance {
        point: start(slanted),
        line: l,
        side: Side::Right,
        value: 6.0,
    };
    assert_eq!(
        offset.mirrored(&doc, all),
        Constraint::PointLineDistance {
            point: start(slanted),
            line: l,
            side: Side::Left,
            value: 6.0
        }
    );
    assert_eq!(offset.mirrored(&doc, only(slanted)), offset);
    assert_eq!(offset.mirrored(&doc, only(l)), offset);
    // 点が直線上（値 0）なら、側は Left のまま。
    let on_line = Constraint::PointLineDistance {
        point: start(outsider),
        line: l,
        side: Side::Left,
        value: 0.0,
    };
    assert_eq!(on_line.mirrored(&doc, all), on_line);

    // 角度: 両方が集合内のときだけ符号を反転する。π は π のまま。
    let angle = |value: f64| Constraint::Angle {
        a: l,
        b: slanted,
        value,
    };
    assert_eq!(angle(0.4).mirrored(&doc, all), angle(-0.4));
    assert_eq!(angle(-2.0).mirrored(&doc, all), angle(2.0));
    assert_eq!(angle(PI).mirrored(&doc, all), angle(PI));
    assert_eq!(angle(0.0).mirrored(&doc, all), angle(0.0));
    assert_eq!(angle(0.4).mirrored(&doc, only(l)), angle(0.4));
    assert_eq!(angle(0.4).mirrored(&doc, only(slanted)), angle(0.4));

    // 集合内の円弧の端点は、相手が集合外でも入れ替わる。中心と、線分の端点は変わらない。
    let joint = Constraint::Coincident {
        a: start(outsider),
        b: start(arc),
    };
    assert_eq!(
        joint.mirrored(&doc, only(arc)),
        Constraint::Coincident {
            a: start(outsider),
            b: end(arc)
        }
    );
    assert_eq!(joint.mirrored(&doc, only(outsider)), joint);
    let chord = Constraint::Distance {
        a: start(arc),
        b: end(arc),
        value: 3.0,
    };
    assert_eq!(
        chord.mirrored(&doc, all),
        Constraint::Distance {
            a: end(arc),
            b: start(arc),
            value: 3.0
        }
    );
    let spoke = Constraint::Coincident {
        a: center(arc),
        b: end(l),
    };
    assert_eq!(spoke.mirrored(&doc, all), spoke);
    let arc_offset = Constraint::PointLineDistance {
        point: end(arc),
        line: l,
        side: Side::Left,
        value: 2.0,
    };
    // 円弧だけが集合内: 端点は入れ替わるが、側は変わらない。
    assert_eq!(
        arc_offset.mirrored(&doc, only(arc)),
        Constraint::PointLineDistance {
            point: start(arc),
            line: l,
            side: Side::Left,
            value: 2.0
        }
    );

    // 点の固定: 座標はミラーしない。線分・円の点はそのまま、円弧の端点は種類だけ入れ替わる。
    let at = Point2::new(1.5, -2.5);
    let fix = |point: PointRef| Constraint::FixPoint { point, at };
    assert_eq!(fix(end(l)).mirrored(&doc, all), fix(end(l)));
    assert_eq!(fix(center(round)).mirrored(&doc, all), fix(center(round)));
    assert_eq!(fix(center(arc)).mirrored(&doc, all), fix(center(arc)));
    assert_eq!(fix(start(arc)).mirrored(&doc, all), fix(end(arc)));

    // そのほかは変わらない。
    for constraint in [
        Constraint::Horizontal { line: l },
        Constraint::Vertical { line: l },
        Constraint::Parallel { a: l, b: slanted },
        Constraint::Perpendicular { a: l, b: slanted },
        Constraint::EqualLength { a: l, b: slanted },
        Constraint::EqualRadius { a: round, b: arc },
        Constraint::Radius {
            entity: arc,
            value: 5.0,
        },
        Constraint::TangentCircles {
            a: round,
            b: arc,
            kind: CircleTangency::Internal(LargerCircle::Second),
        },
        Constraint::FixEntity { entity: arc },
    ] {
        assert_eq!(constraint.mirrored(&doc, all), constraint);
    }

    // ミラーの前に呼んでも後に呼んでも同じ結果（図形の種類は変わらない）。
    let entities: BTreeSet<EntityId> = scene.entities.iter().copied().collect();
    let in_set = |e: EntityId| entities.contains(&e);
    let before: Vec<Constraint> = doc
        .constraints()
        .map(|(_, c)| c.mirrored(&doc, in_set))
        .collect();
    let original: Vec<Constraint> = doc.constraints().map(|(_, c)| c.clone()).collect();
    let batch = mirror_batch(&doc, &scene.entities, true);
    doc.apply(batch).unwrap();
    let after: Vec<Constraint> = original.iter().map(|c| c.mirrored(&doc, in_set)).collect();
    assert_eq!(before, after);
    // 適用した結果の拘束は、ミラーした拘束そのもの。
    let stored: Vec<Constraint> = doc.constraints().map(|(_, c)| c.clone()).collect();
    assert_eq!(stored, before);
}

#[test]
fn mirroring_a_fixed_point_is_rejected_unless_it_lies_on_the_axis() {
    // 点の固定の座標はミラーされないので、固定した点を動かすミラーは拒否される。
    let mut doc = Document::new();
    let l = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    let fix = doc.constrain(Constraint::fix_point(&doc, start(l)).unwrap());
    let batch = mirror_batch(&doc, &[l], true);
    assert_rejected(&mut doc, batch, |e| {
        *e == CoreError::ConstraintBlockedByFixed(fix)
    });
    // 軸の上にある点なら動かないので通る。
    let mut doc = Document::new();
    let l = line(&mut doc, (20.0, -5.0), (10.0, 0.0));
    doc.constrain(Constraint::fix_point(&doc, start(l)).unwrap());
    let batch = mirror_batch(&doc, &[l], true);
    doc.apply(batch).unwrap();
    assert_all_satisfied(&doc);
    assert_eq!(line_of(&doc, l).a, Point2::new(20.0, -5.0));
}
