//! 求解の演算量の予算のテスト（M13 タスク101 後半、DESIGN.md M13 設計判断3 のタスク97 追記）。
//!
//! 予算で打ち切られた `apply` のロールバック、消費の決定性、満たされた成分が数えられないこと、
//! 普通の大きさの系で予算の有無が結果を変えないこと、複数の成分での共有、`hold` の半分の
//! 取り決め、プレビューの打ち切り、自由度の打ち切り。

use mcad_geom::{LineSeg, Shape, Vec2};

use super::super::constraint_solve_tests::chain;
use super::super::constraint_tests::{Constrain, end, line, line_geom, snapshot, start};
use super::*;
use crate::Command;

/// 解いた結果の比較用の要約（形は bit で比べる）。
#[derive(Debug, PartialEq)]
enum Outcome {
    Solved {
        changes: Vec<(EntityId, Vec<u64>)>,
        trials: usize,
        work: u64,
    },
    Failed {
        error: CoreError,
        trials: usize,
        work: u64,
    },
}

impl Outcome {
    fn of(result: Result<FollowUps, Unsettled>) -> Self {
        match result {
            Ok(follow_ups) => Self::Solved {
                changes: follow_ups
                    .changes
                    .iter()
                    .map(|(id, geom)| (*id, bits(geom)))
                    .collect(),
                trials: follow_ups.trials,
                work: follow_ups.work,
            },
            Err(failure) => Self::Failed {
                error: failure.error,
                trials: failure.trials,
                work: failure.work,
            },
        }
    }

    fn work(&self) -> u64 {
        match self {
            Self::Solved { work, .. } | Self::Failed { work, .. } => *work,
        }
    }

    fn is_budget_exceeded(&self) -> bool {
        matches!(
            self,
            Self::Failed {
                error: CoreError::ConstraintBudgetExceeded,
                ..
            }
        )
    }
}

fn bits(geom: &EntityGeom) -> Vec<u64> {
    let values: Vec<f64> = match geom.as_shape().expect("a shape") {
        Shape::Line(l) => vec![l.a.x, l.a.y, l.b.x, l.b.y],
        Shape::Circle(c) => vec![c.center.x, c.center.y, c.radius],
        Shape::Arc(a) => vec![a.center.x, a.center.y, a.radius, a.start_angle, a.end_angle],
        other => panic!("{other:?}"),
    };
    values.into_iter().map(f64::to_bits).collect()
}

fn line_of(doc: &Document, id: EntityId) -> LineSeg {
    match doc.entity(id).unwrap().geom.as_shape() {
        Some(Shape::Line(l)) => *l,
        other => panic!("{other:?}"),
    }
}

/// コマンドを適用した後の求解を `budget` で行い、適用を巻き戻して結果を返す（文書は変えない）。
fn solve_command(doc: &mut Document, cmd: Command, budget: u64) -> Outcome {
    let executed = doc.execute(cmd).unwrap();
    let mut edits = Edits::default();
    edits.collect(&executed.applied);
    let outcome = Outcome::of(doc.solve_components(&edits, None, budget));
    doc.revert(&executed.applied);
    outcome
}

/// 線分の連鎖の先頭の終点を `by` 引くコマンド。
fn pull(doc: &Document, id: EntityId, by: (f64, f64)) -> Command {
    let l = line_of(doc, id);
    Command::ModifyEntity {
        id,
        new_geom: EntityGeom::Shape(Shape::Line(LineSeg::new(l.a, l.b + Vec2::new(by.0, by.1)))),
    }
}

/// `count` 本の連鎖を 1 回の `Batch` で作る（満たされているので解かない。大きな連鎖でも速い）。
fn big_chain(doc: &mut Document, count: u32) -> Vec<EntityId> {
    let layer = doc.current_layer();
    let node = |i: u32| (10.0 * f64::from(i), if i % 2 == 1 { 5.0 } else { 0.0 });
    let adds = (0..count)
        .map(|i| {
            Command::AddEntity(crate::Entity::new(
                line_geom(node(i), node(i + 1)),
                layer,
                crate::Style::inherited(),
            ))
        })
        .collect();
    let links = doc.apply(Command::Batch(adds)).unwrap().entities;
    let mut constraints: Vec<Command> = links
        .windows(2)
        .map(|pair| Command::AddConstraint {
            constraint: Constraint::Coincident {
                a: end(pair[0]),
                b: start(pair[1]),
            },
            hold: None,
        })
        .collect();
    for &link in &links {
        constraints.push(Command::AddConstraint {
            constraint: Constraint::distance(doc, start(link), end(link)).unwrap(),
            hold: None,
        });
    }
    doc.apply(Command::Batch(constraints)).unwrap();
    links
}

/// 開始時の `A Aᵀ` だけで確定の予算を超える連鎖の本数（m = 1348、n ≈ 1798 で約 1.6e9）。
const OVER_APPLY_BUDGET: u32 = 450;

#[test]
fn a_budget_rejection_restores_everything() {
    let mut doc = Document::new();
    let links = big_chain(&mut doc, OVER_APPLY_BUDGET);
    let before = snapshot(&doc);
    let err = doc.apply(pull(&doc, links[0], (1.0, 0.5))).unwrap_err();
    assert_eq!(err, CoreError::ConstraintBudgetExceeded);
    assert_eq!(snapshot(&doc), before);
    // Display は英語で、大きすぎて解き切れない旨。
    assert!(err.to_string().contains("too large"), "{err}");
}

#[test]
fn work_is_deterministic_and_never_exceeds_the_budget() {
    let mut doc = Document::new();
    let links = chain(&mut doc, 8, true);
    // 上限なし・途中で切れる量・はじめから足りない量。
    let far = pull(&doc, links[0], (30.0, 40.0));
    let full = solve_command(&mut doc, far.clone(), u64::MAX);
    assert!(matches!(full, Outcome::Solved { .. }), "{full:?}");
    let used = full.work();
    assert!(used > 0);
    for budget in [u64::MAX, used, used - 1, used / 2, used / 10, 1, 0] {
        let first = solve_command(&mut doc, far.clone(), budget);
        let second = solve_command(&mut doc, far.clone(), budget);
        assert_eq!(first, second, "budget {budget}");
        assert!(first.work() <= budget, "budget {budget}: {first:?}");
        if budget >= used {
            assert_eq!(first, full, "enough budget does not change the result");
        } else {
            assert!(first.is_budget_exceeded(), "budget {budget}: {first:?}");
        }
    }
}

#[test]
fn a_satisfied_giant_component_uses_no_work() {
    let mut doc = Document::new();
    let links = big_chain(&mut doc, OVER_APPLY_BUDGET);
    // 全部をまとめて平行移動: 満たされたままなので、予算 0 でも通り、1 演算も数えない。
    let moved: Vec<Command> = links
        .iter()
        .map(|&id| Command::ModifyEntity {
            id,
            new_geom: doc
                .entity(id)
                .unwrap()
                .geom
                .translated(Vec2::new(3.0, -2.0)),
        })
        .collect();
    let outcome = solve_command(&mut doc, Command::Batch(moved.clone()), 0);
    assert_eq!(
        outcome,
        Outcome::Solved {
            changes: Vec::new(),
            trials: 0,
            work: 0,
        }
    );
    // 実際の apply でも通る。
    doc.apply(Command::Batch(moved)).unwrap();
    assert!(doc.undo());
}

/// 連鎖の編集と解の無い編集で、確定の予算の有無が結果（形の bit・試行・演算量）を変えないこと。
fn budget_does_not_change_chain_edits(count: u32, cases: &[(&str, (f64, f64), bool)]) {
    for (label, by, pin_tail) in cases {
        let mut doc = Document::new();
        let links = chain(&mut doc, count, true);
        if *pin_tail {
            doc.constrain(Constraint::FixEntity {
                entity: *links.last().unwrap(),
            });
        }
        let cmd = pull(&doc, links[0], *by);
        let unlimited = solve_command(&mut doc, cmd.clone(), u64::MAX);
        let budgeted = solve_command(&mut doc, cmd, APPLY_WORK_BUDGET);
        assert_eq!(budgeted, unlimited, "{count} links, {label}");
        assert!(
            unlimited.work() <= APPLY_WORK_BUDGET,
            "{count} links, {label}"
        );
    }
}

/// `hold` つきで解の無い追加（止めて解く段階も、外して解く段階も試行を使い切る）と、解ける追加。
fn budget_does_not_change_held_additions(count: u32) {
    // 文書を作り直して比べる（拒否された拘束の ID は巻き戻しの後に進むので、同じ文書では
    // エラーが指す ID が変わる）。
    let unsolvable = || {
        // 末尾が固定の連鎖の先頭を、全長の外の固定した図形へつなぐ: 解が無い。
        let mut doc = Document::new();
        let links = chain(&mut doc, count, true);
        let anchor = line(&mut doc, (0.0, 1000.0), (10.0, 1000.0));
        doc.constrain(Constraint::FixEntity { entity: anchor });
        doc.constrain(Constraint::FixEntity {
            entity: *links.last().unwrap(),
        });
        let cmd = Command::AddConstraint {
            constraint: Constraint::Coincident {
                a: start(links[0]),
                b: end(anchor),
            },
            // 固定した図形を止めるので、止めて解く段階も外して解く段階も同じ系で試行を使い切る
            // （先頭を止めると、止められた図形だけの拘束として解かずに拒否され、費用が測れない）。
            hold: Some(anchor),
        };
        (doc, cmd)
    };
    let solvable = || {
        // 解ける追加: 先頭の始点を、届く位置の線分の端へ。
        let mut doc = Document::new();
        let links = chain(&mut doc, count, true);
        let free = line(&mut doc, (-30.0, -30.0), (-25.0, -25.0));
        let cmd = Command::AddConstraint {
            constraint: Constraint::Coincident {
                a: start(links[0]),
                b: end(free),
            },
            hold: Some(free),
        };
        (doc, cmd)
    };
    let compare = |scene: &dyn Fn() -> (Document, Command), label: &str| {
        let (mut doc, cmd) = scene();
        let unlimited = solve_command(&mut doc, cmd, u64::MAX);
        let (mut doc, cmd) = scene();
        let budgeted = solve_command(&mut doc, cmd, APPLY_WORK_BUDGET);
        assert_eq!(budgeted, unlimited, "{count} links, {label}");
        assert!(unlimited.work() <= APPLY_WORK_BUDGET, "{label}");
        unlimited
    };
    let failed = compare(&unsolvable, "unsolvable");
    let Outcome::Failed { trials, .. } = failed else {
        panic!("{failed:?}");
    };
    assert!(trials > 2 * solve::MAX_TRIALS, "both stages ran: {trials}");
    let solved = compare(&solvable, "solvable");
    assert!(matches!(solved, Outcome::Solved { .. }), "{solved:?}");
}

const SMALL: (&str, (f64, f64), bool) = ("(a)", (1.0, 0.5), false);
const FAR: (&str, (f64, f64), bool) = ("(b)", (60.0, 80.0), false);
const UNLIMITED_PULL: (&str, (f64, f64), bool) = ("(c2)", (800.0, 600.0), false);

#[test]
fn the_budget_does_not_change_edits_of_small_chains() {
    // 通常のテストは debug で現実的な範囲。50 本までの全組み合わせは下の `#[ignore]`。
    let beyond = ("(c1)", (-(12.0 * 10.0 + 50.0), 0.0), true);
    budget_does_not_change_chain_edits(10, &[SMALL, FAR, beyond, UNLIMITED_PULL]);
    budget_does_not_change_chain_edits(24, &[SMALL, FAR]);
    budget_does_not_change_held_additions(10);
}

/// 線分 50 本までの連鎖で、確定の予算の有無が結果を変えないこと（完了条件）。release で
/// `cargo test --release -p mcad-core budget_does_not_change -- --ignored` で実行する。
#[test]
#[ignore = "slow in debug builds (minutes); run it with --release"]
fn the_budget_does_not_change_edits_of_chains_up_to_50_links() {
    for count in [10_u32, 24, 50] {
        let beyond = ("(c1)", (-(12.0 * f64::from(count) + 50.0), 0.0), true);
        budget_does_not_change_chain_edits(count, &[SMALL, FAR, beyond, UNLIMITED_PULL]);
        budget_does_not_change_held_additions(count);
    }
}

#[test]
fn components_share_one_budget() {
    let mut doc = Document::new();
    let first = chain(&mut doc, 6, true);
    let second = chain(&mut doc, 9, true);
    let (a, b) = (
        pull(&doc, first[0], (5.0, 7.0)),
        pull(&doc, second[0], (5.0, 7.0)),
    );
    let work_a = solve_command(&mut doc, a.clone(), u64::MAX).work();
    let work_b = solve_command(&mut doc, b.clone(), u64::MAX).work();
    assert!(work_a > 0 && work_b > 0);

    let both = Command::Batch(vec![a, b]);
    let solved = solve_command(&mut doc, both.clone(), u64::MAX);
    assert_eq!(solved.work(), work_a + work_b, "{solved:?}");
    // ちょうどの合計なら解ける。1 足りなければ（2 つ目の成分の途中で）打ち切り。各成分が
    // 自分の分だけで解ける予算でも、共有なので足りない。
    assert_eq!(
        solve_command(&mut doc, both.clone(), work_a + work_b),
        solved
    );
    let short = solve_command(&mut doc, both.clone(), work_a + work_b - 1);
    assert!(short.is_budget_exceeded(), "{short:?}");
    assert!(short.work() > work_a, "the first component completed");
    let each = solve_command(&mut doc, both, work_a.max(work_b));
    assert!(each.is_budget_exceeded(), "{each:?}");
}

#[test]
fn a_hold_stage_cut_at_half_the_budget_still_solves_without_the_hold() {
    let mut doc = Document::new();
    let links = chain(&mut doc, 10, true);
    let free = line(&mut doc, (-60.0, -80.0), (-50.0, -70.0));
    // `free` を止めると連鎖の全体を遠くまで引くことになる。止めなければ `free` が近寄るだけ。
    let add = |hold| Command::AddConstraint {
        constraint: Constraint::Coincident {
            a: start(links[0]),
            b: end(free),
        },
        hold,
    };
    let held = solve_command(&mut doc, add(Some(free)), u64::MAX);
    let free_only = solve_command(&mut doc, add(None), u64::MAX);
    assert!(matches!(held, Outcome::Solved { .. }), "{held:?}");
    assert!(
        held.work() > free_only.work(),
        "held {} / free {}",
        held.work(),
        free_only.work()
    );

    // 止めて解く段階は半分で足りずに打ち切られ、残りの半分で止めずに解ける。
    let budget = 2 * free_only.work();
    let cut = solve_command(&mut doc, add(Some(free)), budget);
    let Outcome::Solved {
        changes,
        work,
        trials,
    } = &cut
    else {
        panic!("{cut:?}");
    };
    let Outcome::Solved {
        changes: expected,
        trials: free_trials,
        ..
    } = &free_only
    else {
        panic!("{free_only:?}");
    };
    assert_eq!(changes, expected, "the same shape as without the hold");
    assert!(work <= &budget);
    assert!(*work > free_only.work(), "the hold stage used its share");
    assert!(trials >= free_trials);

    // どちらの段階でも足りなければ予算切れ。
    let short = solve_command(&mut doc, add(Some(free)), free_only.work() / 2);
    assert!(short.is_budget_exceeded(), "{short:?}");
}

#[test]
fn a_preview_cut_by_the_budget_does_not_stop_the_apply() {
    // 60 本の連鎖の小さな移動: A Aᵀ を 4 回作る（1 回約 3.8e6）ので 1 フレームの予算を超える。
    // 確定の予算では解ける。100 本でも同じだが、debug で数秒かかるので 60 本にしてある。
    let mut doc = Document::new();
    let links = big_chain(&mut doc, 60);
    let cmd = pull(&doc, links[0], (1.0, 0.5));
    let Command::ModifyEntity { id, new_geom } = cmd.clone() else {
        unreachable!();
    };
    let before = snapshot(&doc);
    assert_eq!(
        doc.preview_modify(id, new_geom.clone()),
        Err(CoreError::ConstraintBudgetExceeded)
    );
    assert_eq!(snapshot(&doc), before, "the preview changed nothing");
    let geom_before = doc.entity(id).unwrap().geom.clone();
    let cut = doc
        .solve_preview(id, &geom_before, &new_geom, PREVIEW_WORK_BUDGET)
        .map(|_| ())
        .unwrap_err();
    assert!(cut.work <= PREVIEW_WORK_BUDGET, "{}", cut.work);
    doc.apply(cmd).unwrap();
    assert_eq!(line_of(&doc, id).b, {
        let Some(Shape::Line(l)) = new_geom.as_shape() else {
            unreachable!();
        };
        l.b
    });
    assert!(doc.undo());
}

#[test]
fn a_preview_inside_the_budget_is_the_same_as_the_apply() {
    // 予算の中で解き直しを通る編集（1 回目が停滞・上限）でも、確定と bit 一致。予算切れだけを
    // 解き直さないので、たどる手順は同じ。
    let mut doc = Document::new();
    let links = big_chain(&mut doc, 12);
    let Command::ModifyEntity { id, new_geom } = pull(&doc, links[0], (0.0, 400.0)) else {
        unreachable!();
    };
    let before = doc.entity(id).unwrap().geom.clone();
    let preview = Outcome::of(doc.solve_preview(id, &before, &new_geom, PREVIEW_WORK_BUDGET));
    let apply = Outcome::of(doc.solve_preview(id, &before, &new_geom, APPLY_WORK_BUDGET));
    assert_eq!(preview, apply);
    let (Outcome::Solved { trials, .. } | Outcome::Failed { trials, .. }) = apply;
    assert!(
        trials > solve::MAX_TRIALS,
        "the retry was not used: {trials}"
    );
}

#[test]
fn degrees_of_freedom_are_not_computed_for_huge_components() {
    let mut doc = Document::new();
    let huge = big_chain(&mut doc, 200);
    assert_eq!(
        doc.degrees_of_freedom_bounded(huge[0]),
        DofOutcome::TooLarge
    );
    // 小さい成分は上限なしの入口と同じ値。
    let small = chain(&mut doc, 10, true);
    assert_eq!(
        doc.degrees_of_freedom_bounded(small[0]),
        DofOutcome::Known(doc.degrees_of_freedom(small[0]).unwrap())
    );
    // 拘束の無い図形・存在しない図形。
    let free = line(&mut doc, (0.0, 500.0), (10.0, 500.0));
    assert_eq!(doc.degrees_of_freedom_bounded(free), DofOutcome::Known(4));
    assert_eq!(
        doc.degrees_of_freedom_bounded(EntityId::default()),
        DofOutcome::Unknown
    );
}
