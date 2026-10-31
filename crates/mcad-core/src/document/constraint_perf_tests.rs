//! グリップのドラッグ中の求解の実測（M13 タスク101 前半、DESIGN.md M13 設計判断3 の「演算量の
//! 予算」の根拠）。
//!
//! 通常のテストでは走らない（200 本の連鎖は release でも分単位）。release で実行する:
//!
//! ```text
//! cargo test --release -p mcad-core grip_drag_ -- --ignored --nocapture --test-threads=1
//! ```
//!
//! 数値は実行したマシンのもの。結果は DESIGN.md 判断7 の実装時追記に残す。予算のある
//! 確定・プレビューを測る（`grip_drag_solve_times`）。受理と棄却の費用の分解
//! （`grip_drag_trial_cost_breakdown`）は予算なしの `solve::solve` を直接測る。

use std::time::{Duration, Instant};

use mcad_geom::{LineSeg, Point2, Shape, Vec2};

use super::super::constraint_solve_tests::chain;
use super::super::constraint_tests::{Constrain, end, line, line_geom, rectangle, start};
use super::*;
use crate::Command;

/// 実測する編集 1 件。
struct Case {
    label: String,
    doc: Document,
    /// 動かす図形（ドラッグ中の図形）。
    id: EntityId,
    /// 動かす前の線分。始点は止め、終点を `by` だけ動かす（線分の終点グリップ）。
    from: LineSeg,
    by: Vec2,
    /// `by` の途中の 10 点もドラッグとして測るか。
    sweep: bool,
}

impl Case {
    fn edited(&self, fraction: f64) -> EntityGeom {
        EntityGeom::Shape(Shape::Line(LineSeg::new(
            self.from.a,
            self.from.b + self.by * fraction,
        )))
    }
}

fn line_of(doc: &Document, id: EntityId) -> LineSeg {
    match doc.entity(id).unwrap().geom.as_shape() {
        Some(Shape::Line(l)) => *l,
        other => panic!("{other:?}"),
    }
}

/// 外れ値に強い代表値。遅い編集は 1 回だけ測る（合計の時間を抑える）。
fn timed<T>(mut run: impl FnMut() -> T) -> (Duration, T) {
    let start = Instant::now();
    let mut value = run();
    let first = start.elapsed();
    if first > Duration::from_millis(500) {
        return (first, value);
    }
    let mut samples = vec![first];
    for _ in 0..4 {
        let start = Instant::now();
        value = run();
        samples.push(start.elapsed());
    }
    samples.sort();
    (samples[samples.len() / 2], value)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

/// 結果の短い表記。
fn verdict(result: &Result<bool, CoreError>) -> String {
    match result {
        Ok(_) => "ok".to_owned(),
        Err(CoreError::ConstraintBudgetExceeded) => "予算切れ".to_owned(),
        Err(CoreError::ConstraintUnsolvable(_)) => "解なし".to_owned(),
        Err(err) => format!("{err:?}"),
    }
}

/// 実測の 1 行を表示する（確定とプレビュー、それぞれの予算で）。
fn measure(mut case: Case) {
    let new_geom = case.edited(1.0);
    let before = case.doc.entity(case.id).unwrap().geom.clone();
    let before = &before;

    // 方程式数 m・未知数 n（固定されていないパラメータの数）。
    let mut edits = Edits::default();
    edits.collect(&Applied::ModifyEntity {
        id: case.id,
        before: before.clone(),
        after: new_geom.clone(),
        removed_constraints: Vec::new(),
    });
    let overlay = Some((case.id, &new_geom));
    let seeds = BTreeSet::from([case.id]);
    let (mut m, mut n) = (0, 0);
    for component in case.doc.components_of(&seeds) {
        let sys = case
            .doc
            .component_system(&component, &edits, overlay)
            .unwrap();
        m += sys.equations.len();
        n += sys.fixed.iter().filter(|fixed| !**fixed).count();
    }

    let counts = |doc: &Document, budget: u64, geom: &EntityGeom| match doc
        .solve_preview(case.id, before, geom, budget)
    {
        Ok(follow_ups) => (follow_ups.trials, follow_ups.work),
        Err(failure) => (failure.trials, failure.work),
    };

    // 確定。受理された編集は undo で戻す。undo の時間は含めない。
    let (apply_trials, apply_work) = counts(&case.doc, APPLY_WORK_BUDGET, &new_geom);
    let (mut apply_times, mut applied) = (Vec::new(), Ok(false));
    for _ in 0..5 {
        let start = Instant::now();
        let result = case.doc.apply(Command::ModifyEntity {
            id: case.id,
            new_geom: new_geom.clone(),
        });
        let spent = start.elapsed();
        apply_times.push(spent);
        applied = result.as_ref().map(|_| true).map_err(Clone::clone);
        if result.is_ok() {
            case.doc.undo();
        }
        if spent > Duration::from_millis(500) {
            break;
        }
    }
    apply_times.sort();
    let apply_time = apply_times[apply_times.len() / 2];

    // プレビュー。
    let (preview_trials, preview_work) = counts(&case.doc, PREVIEW_WORK_BUDGET, &new_geom);
    let (preview_time, previewed) = timed(|| {
        case.doc
            .preview_modify(case.id, new_geom.clone())
            .map(|_| true)
    });

    let (dof_time, dof) = timed(|| case.doc.degrees_of_freedom_bounded(case.id));
    let per_unit = |time: Duration, work: u64| {
        if work == 0 {
            f64::NAN
        } else {
            time.as_secs_f64() * 1e9 / work as f64
        }
    };
    println!(
        "| {} | {m} | {n} | {} / {apply_trials} / {apply_work:.2e} / {:.3} / {:.2} | {} / {preview_trials} / {preview_work:.2e} / {:.3} | {:.3} ({dof:?}) |",
        case.label,
        verdict(&applied),
        ms(apply_time),
        per_unit(apply_time, apply_work),
        verdict(&previewed),
        ms(preview_time),
        ms(dof_time),
    );

    // ドラッグ 1 回分: 動かす量を 10 等分した 10 位置を、それぞれ元の形からプレビューの予算で解く。
    if case.sweep {
        let (mut total, mut slowest, mut cut) = (Duration::ZERO, Duration::ZERO, 0);
        for i in 1..=10 {
            let new_geom = case.edited(f64::from(i) / 10.0);
            let start = Instant::now();
            let result = case.doc.preview_modify(case.id, new_geom);
            let spent = start.elapsed();
            total += spent;
            slowest = slowest.max(spent);
            if matches!(result, Err(CoreError::ConstraintBudgetExceeded)) {
                cut += 1;
            }
        }
        println!(
            "|   └ 10 位置のプレビュー | | | | 打ち切り {cut} 回 / 合計 {:.3} / 最大 {:.3} | |",
            ms(total),
            ms(slowest),
        );
    }
}

fn header(title: &str) {
    println!("\n### {title}");
    println!(
        "| 編集 | m | n | 確定: 結果 / 試行 / 消費 / ms / ns/消費 | プレビュー: 結果 / 試行 / 消費 / ms | dof ms (結果) |"
    );
    println!("|---|---|---|---|---|---|");
}

/// 連鎖の先頭の線分の終点を引く編集（`far_edits_of_a_long_chain_are_solved` の形）。
fn chain_case(
    links: u32,
    label: &str,
    by: (f64, f64),
    pin_tail: bool,
    sweep: bool,
    zigzag: bool,
) -> Case {
    let mut doc = Document::new();
    let ids = chain(&mut doc, links, zigzag);
    if pin_tail {
        doc.constrain(Constraint::FixEntity {
            entity: *ids.last().unwrap(),
        });
    }
    let from = line_of(&doc, ids[0]);
    Case {
        label: format!("{links} 本 {label}"),
        doc,
        id: ids[0],
        from,
        by: Vec2::new(by.0, by.1),
        sweep,
    }
}

/// 一致だけでつないだ連鎖（長さの拘束なし。`n / m` が [`chain`] と違う系。線形なので数試行）。
fn coincident_chain_case(links: u32, by: (f64, f64)) -> Case {
    let mut doc = Document::new();
    let node = |i: u32| (10.0 * f64::from(i), if i % 2 == 1 { 5.0 } else { 0.0 });
    let ids: Vec<EntityId> = (0..links)
        .map(|i| line(&mut doc, node(i), node(i + 1)))
        .collect();
    for pair in ids.windows(2) {
        doc.constrain(Constraint::Coincident {
            a: end(pair[0]),
            b: start(pair[1]),
        });
    }
    let from = line_of(&doc, ids[0]);
    Case {
        label: format!("{links} 本（一致のみ）"),
        doc,
        id: ids[0],
        from,
        by: Vec2::new(by.0, by.1),
        sweep: false,
    }
}

/// 密な `A Aᵀ` の系: 1 本の線分（ハブ）と等長の線分 `spokes` 本。どの方程式もハブを参照する。
fn star_case(spokes: u32, by: (f64, f64)) -> Case {
    let mut doc = Document::new();
    let hub = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
    for i in 0..spokes {
        let y = 10.0 + 10.0 * f64::from(i);
        let spoke = line(&mut doc, (0.0, y), (10.0, y));
        doc.constrain(Constraint::EqualLength { a: hub, b: spoke });
    }
    let from = line_of(&doc, hub);
    Case {
        label: format!("等長スター {spokes} 本"),
        doc,
        id: hub,
        from,
        by: Vec2::new(by.0, by.1),
        sweep: false,
    }
}

#[test]
#[ignore = "takes minutes even in release builds; run it with --release --nocapture"]
fn grip_drag_solve_times() {
    let small = (1.0, 0.5);
    let far = (60.0, 80.0);

    header("拘束なし");
    for (label, by) in [("(a) 小", small), ("(b) 遠", (300.0, 200.0))] {
        let mut doc = Document::new();
        let id = line(&mut doc, (0.0, 0.0), (10.0, 0.0));
        let from = line_of(&doc, id);
        measure(Case {
            label: label.to_owned(),
            doc,
            id,
            from,
            by: Vec2::new(by.0, by.1),
            sweep: false,
        });
    }
    // 拘束と無関係な図形: 全拘束の走査（索引なし）の費用だけが乗る。
    {
        let mut doc = Document::new();
        chain(&mut doc, 200, true);
        let id = line(&mut doc, (0.0, 100.0), (10.0, 100.0));
        let from = line_of(&doc, id);
        measure(Case {
            label: "無関係な線分（200 本の連鎖の隣）".to_owned(),
            doc,
            id,
            from,
            by: Vec2::new(1.0, 0.5),
            sweep: false,
        });
    }

    header("矩形（一致 4 + 水平・垂直 4）");
    let rectangle_case = |label: &str, by: (f64, f64), pin_top: bool, sweep: bool| {
        let mut doc = Document::new();
        let sides = rectangle(&mut doc).sides;
        if pin_top {
            // 上の辺を固定すると、右の辺の x は動かせない。
            doc.constrain(Constraint::FixEntity { entity: sides[2] });
        }
        let from = line_of(&doc, sides[0]);
        Case {
            label: label.to_owned(),
            doc,
            id: sides[0],
            from,
            by: Vec2::new(by.0, by.1),
            sweep,
        }
    };
    measure(rectangle_case("(a) 小", small, false, true));
    measure(rectangle_case("(b) 遠", (300.0, 200.0), false, true));
    measure(rectangle_case(
        "(c) 解けない（上の辺を固定して x へ）",
        (10.0, 0.0),
        true,
        false,
    ));

    header("密な A Aᵀ（等長スター）");
    for &spokes in &[50_u32, 100, 200, 400, 800] {
        measure(star_case(spokes, (3.0, 4.0)));
    }

    for &links in &[10_u32, 24, 50, 100, 200] {
        header(&format!(
            "一致と長さの連鎖 {links} 本（ジグザグ、先頭の終点を引く）"
        ));
        // 全長を超えて引く量（固定した末尾から届かない）。
        let beyond = -(12.0 * f64::from(links) + 50.0);
        measure(chain_case(links, "(a) 小", small, false, true, true));
        measure(chain_case(links, "(b) 遠 (60,80)", far, false, true, true));
        measure(chain_case(
            links,
            "(c1) 解なし（末尾を固定、全長の外へ）",
            (beyond, 0.0),
            true,
            false,
            true,
        ));
        measure(chain_case(
            links,
            "(c2) 上限（固定なし、(800,600)）",
            (800.0, 600.0),
            false,
            false,
            true,
        ));
        measure(chain_case(
            links,
            "(b') 直線 遠 (60,80)",
            far,
            false,
            false,
            false,
        ));
    }
}

/// 1 試行の費用を、受理された試行（行列の積 `A Aᵀ` を作り直す）と棄却された試行に分ける。
///
/// [`Document::solve_preview`] と同じ系を [`solve::solve`] へ直接渡し、受理の回数
/// （`Solution::objective` の長さ − 1）を読む。同じ系で受理と棄却の比が違う 2 つの編集から
/// 連立して `G`（`A Aᵀ` 1 回の費用）と `R`（どの試行にもかかる費用）を求める:
/// 所要時間 = (受理 + 1)・G + 試行・R。
#[test]
#[ignore = "takes about a minute even in release builds; run it with --release --nocapture"]
fn grip_drag_trial_cost_breakdown() {
    /// `solve::solve` の実測（時間の中央値 ms・試行・受理）と m・n。
    fn run(case: &Case) -> (usize, usize, f64, usize, usize) {
        let new_geom = case.edited(1.0);
        let before = case.doc.entity(case.id).unwrap().geom.clone();
        let mut edits = Edits::default();
        edits.collect(&Applied::ModifyEntity {
            id: case.id,
            before,
            after: new_geom.clone(),
            removed_constraints: Vec::new(),
        });
        let components = case.doc.components_of(&BTreeSet::from([case.id]));
        let sys = case
            .doc
            .component_system(&components[0], &edits, Some((case.id, &new_geom)))
            .unwrap();
        let weights = sys.weights();
        let problem = Problem::new(&sys.params, &sys.fixed, &weights, &sys.equations);
        let (time, solution) = timed(|| solve::solve(&problem));
        let solution = solution.unwrap_or_else(|e| panic!("{}: {e:?}", case.label));
        let n = sys.fixed.iter().filter(|fixed| !**fixed).count();
        (
            sys.equations.len(),
            n,
            ms(time),
            solution.trials,
            solution.objective.len() - 1,
        )
    }

    println!("\n### 受理と棄却の費用（長さつきの連鎖。G・R は 2 つの編集から連立）");
    println!(
        "| 本数 | m | n | (a) ms/試行/受理 | (b) ms/試行/受理 | G ms | R ms | G/(m²n/2) ns | R/(m³/3) ns |"
    );
    println!("|---|---|---|---|---|---|---|---|---|");
    for &links in &[24_u32, 50, 100, 200] {
        let (m, n, t_a, trials_a, acc_a) =
            run(&chain_case(links, "(a)", (1.0, 0.5), false, false, true));
        let (_, _, t_b, trials_b, acc_b) =
            run(&chain_case(links, "(b)", (60.0, 80.0), false, false, true));
        // (acc + 1)·G + trials·R = t の 2 元連立。
        let (a1, b1, c1) = ((acc_a + 1) as f64, trials_a as f64, t_a);
        let (a2, b2, c2) = ((acc_b + 1) as f64, trials_b as f64, t_b);
        let det = a1 * b2 - a2 * b1;
        let g = (c1 * b2 - c2 * b1) / det;
        let r = (a1 * c2 - a2 * c1) / det;
        let (mf, nf) = (m as f64, n as f64);
        println!(
            "| {links} | {m} | {n} | {t_a:.3}/{trials_a}/{acc_a} | {t_b:.3}/{trials_b}/{acc_b} | {g:.3} | {r:.3} | {:.2} | {:.2} |",
            g * 1e6 / (mf * mf * nf / 2.0),
            r * 1e6 / (mf * mf * mf / 3.0),
        );
    }

    println!("\n### 一致だけの連鎖（n/m ≈ 2。ほぼ受理だけ）");
    println!("| 本数 | m | n | ms | 試行 | 受理 | ms/(受理+1) | 同/(m²n/2) ns |");
    println!("|---|---|---|---|---|---|---|---|");
    for &links in &[50_u32, 100, 200, 400] {
        let (m, n, t, trials, accepted) = run(&coincident_chain_case(links, (3.0, 4.0)));
        let per = t / (accepted + 1) as f64;
        let (mf, nf) = (m as f64, n as f64);
        println!(
            "| {links} | {m} | {n} | {t:.3} | {trials} | {accepted} | {per:.3} | {:.2} |",
            per * 1e6 / (mf * mf * nf / 2.0),
        );
    }
}

/// 実測の前提（ジグザグの先頭の形と編集）の確認。
#[test]
fn perf_scene_shapes_are_as_documented() {
    let case = chain_case(4, "", (1.0, 0.0), false, false, true);
    assert_eq!(case.from.a, Point2::new(0.0, 0.0));
    assert_eq!(case.from.b, Point2::new(10.0, 5.0));
    assert_eq!(case.edited(1.0), line_geom((0.0, 0.0), (11.0, 5.0)));
}
