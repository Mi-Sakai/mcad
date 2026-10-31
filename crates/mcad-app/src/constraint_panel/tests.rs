//! 右パネル「拘束」セクションの純関数のテスト（M13 タスク100）。

use std::f64::consts::{FRAC_PI_2, PI};

use egui::{Rect, vec2};
use mcad_core::{Entity, PointKind, PointRef, Style};
use mcad_geom::{LineSeg, Point2};

use super::*;
use crate::constraint_symbols::{SymbolGlyph, constraint_symbols};
use crate::viewport::Viewport;

fn p(x: f64, y: f64) -> Point2 {
    Point2::new(x, y)
}

fn line(doc: &mut Document, a: (f64, f64), b: (f64, f64)) -> EntityId {
    let layer = doc.current_layer();
    doc.apply(Command::AddEntity(Entity::new(
        Shape::Line(LineSeg::new(p(a.0, a.1), p(b.0, b.1))),
        layer,
        Style::inherited(),
    )))
    .unwrap()
    .entities[0]
}

fn constrain(doc: &mut Document, c: Constraint) -> ConstraintId {
    doc.apply(Command::AddConstraint {
        constraint: c,
        hold: None,
    })
    .unwrap()
    .constraints[0]
}

#[test]
fn rows_are_deduplicated_and_ordered_by_id() {
    let mut doc = Document::new();
    let a = line(&mut doc, (0.0, 0.0), (100.0, 0.0));
    let b = line(&mut doc, (0.0, 50.0), (100.0, 50.0));
    let c = line(&mut doc, (500.0, 0.0), (600.0, 0.0));
    let parallel = constrain(&mut doc, Constraint::Parallel { a, b });
    let horizontal = constrain(&mut doc, Constraint::Horizontal { line: b });
    let other = constrain(&mut doc, Constraint::Horizontal { line: c });
    // 平行は a と b の両方に付くが 1 回だけ。昇順（追加順）。
    assert_eq!(rows_for(&doc, &[a, b]), vec![parallel, horizontal]);
    assert_eq!(rows_for(&doc, &[b, a]), vec![parallel, horizontal]);
    assert_eq!(rows_for(&doc, &[c]), vec![other]);
    assert!(rows_for(&doc, &[]).is_empty());
}

#[test]
fn no_constraints_gives_no_rows() {
    let mut doc = Document::new();
    let a = line(&mut doc, (0.0, 0.0), (100.0, 0.0));
    assert!(rows_for(&doc, &[a]).is_empty());
    // 拘束の無い図形は 1 成分で、図形だけのパラメータ数（線分は 4）。
    let comps = dof_components(&doc, &[a]);
    assert_eq!(comps.len(), 1);
    assert_eq!(comps[0].dof, Some(4));
    assert_eq!(dof_lines(&comps), vec!["自由度: 4".to_owned()]);
}

#[test]
fn two_components_give_two_lines() {
    let mut doc = Document::new();
    let a = line(&mut doc, (0.0, 0.0), (100.0, 0.0));
    let b = line(&mut doc, (0.0, 50.0), (100.0, 50.0));
    let c = line(&mut doc, (500.0, 0.0), (600.0, 0.0));
    constrain(&mut doc, Constraint::Parallel { a, b });
    constrain(&mut doc, Constraint::Horizontal { line: c });

    // a・b は同じ成分（平行でつながる）、c は別の成分。
    let comps = dof_components(&doc, &[a, c, b]);
    assert_eq!(comps.len(), 2);
    assert_eq!(comps[0].members, vec![a, b]);
    assert_eq!(comps[1].members, vec![c]);
    let lines = dof_lines(&comps);
    assert_eq!(lines.len(), 2);
    assert!(
        lines[0].starts_with("成分 1(図形 2 個): 自由度 "),
        "{lines:?}"
    );
    assert!(
        lines[1].starts_with("成分 2(図形 1 個): 自由度 "),
        "{lines:?}"
    );
    assert_eq!(comps[0].dof, doc.degrees_of_freedom(a));
    assert_eq!(comps[1].dof, doc.degrees_of_freedom(c));

    // a だけを選べば成分は 1 つで、`自由度: n` の 1 行（選ばれていない b を含む成分の値）。
    let one = dof_components(&doc, &[a]);
    assert_eq!(one.len(), 1);
    assert_eq!(dof_lines(&one).len(), 1);
    assert_eq!(one[0].dof, doc.degrees_of_freedom(a));
    assert!(dof_lines(&one)[0].starts_with("自由度: "));
}

#[test]
fn target_entities_keeps_order_and_drops_non_shapes() {
    let mut doc = Document::new();
    let a = line(&mut doc, (0.0, 0.0), (100.0, 0.0));
    let b = line(&mut doc, (0.0, 50.0), (100.0, 50.0));
    let layer = doc.current_layer();
    let point = doc
        .apply(Command::AddEntity(Entity::new(
            Shape::Point(p(1.0, 1.0)),
            layer,
            Style::inherited(),
        )))
        .unwrap()
        .entities[0];
    assert_eq!(target_entities(&doc, &[b, point, a, b]), vec![b, a]);
}

#[test]
fn degrees_are_converted_once_and_normalized() {
    let v = parse_edit_value(ValueUnit::Deg, "90").unwrap();
    assert!((v - FRAC_PI_2).abs() < 1e-12);
    // 範囲内（π ちょうど）は変えない。
    assert_eq!(parse_edit_value(ValueUnit::Deg, "180"), Some(PI));
    // 270° は −90° へ、−180° は +180° へ。
    let v = parse_edit_value(ValueUnit::Deg, " 270 ").unwrap();
    assert!((v + FRAC_PI_2).abs() < 1e-12, "{v}");
    let v = parse_edit_value(ValueUnit::Deg, "-180").unwrap();
    assert!((v - PI).abs() < 1e-12, "{v}");
    // mm はそのまま。
    assert_eq!(parse_edit_value(ValueUnit::Mm, "12.5"), Some(12.5));
    // 不正値。
    for bad in ["", "  ", "abc", "1,5", "inf", "NaN", "--1"] {
        assert_eq!(parse_edit_value(ValueUnit::Mm, bad), None, "{bad:?}");
        assert_eq!(parse_edit_value(ValueUnit::Deg, bad), None, "{bad:?}");
    }
}

#[test]
fn format_value_round_trips_through_parse() {
    assert_eq!(format_value(ValueUnit::Mm, 12.5), "12.5");
    assert_eq!(format_value(ValueUnit::Deg, FRAC_PI_2), "90");
    let text = format_value(ValueUnit::Deg, 0.5);
    let back = parse_edit_value(ValueUnit::Deg, &text).unwrap();
    assert!((back - 0.5).abs() < 1e-3);
}

#[test]
fn set_value_applies_the_converted_value_in_one_undo() {
    let mut doc = Document::new();
    let a = line(&mut doc, (0.0, 0.0), (100.0, 0.0));
    let b = line(&mut doc, (0.0, 0.0), (0.0, 100.0));
    let angle = constrain(
        &mut doc,
        Constraint::Angle {
            a,
            b,
            value: FRAC_PI_2,
        },
    );
    let before = doc.generation();
    let value = parse_edit_value(ValueUnit::Deg, "45").unwrap();
    doc.apply(Command::SetConstraintValue { id: angle, value })
        .unwrap();
    let got = doc.constraint(angle).and_then(Constraint::value).unwrap();
    assert!((got - PI / 4.0).abs() < 1e-9, "{got}");
    assert!(doc.undo());
    assert_eq!(doc.generation(), before);
    let got = doc.constraint(angle).and_then(Constraint::value).unwrap();
    assert!((got - FRAC_PI_2).abs() < 1e-9, "{got}");
}

#[test]
fn symbol_hit_uses_the_symbol_rect_and_prefers_the_topmost() {
    let mut doc = Document::new();
    let a = line(&mut doc, (0.0, 0.0), (100.0, 0.0));
    let b = line(&mut doc, (0.0, 50.0), (100.0, 50.0));
    let h = constrain(&mut doc, Constraint::Horizontal { line: a });
    let v = constrain(&mut doc, Constraint::Parallel { a, b });
    let rect = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
    let symbols = constraint_symbols(&doc, &Viewport::new(), rect);
    let sh = symbols.iter().find(|s| s.id == h).unwrap();
    assert!(matches!(&sh.glyph, SymbolGlyph::Text(t) if t == "H"));
    // 中心は当たり、矩形の外は外れ。
    assert_eq!(hit_symbol(&symbols, sh.center), Some(h));
    let inside = sh.center + vec2(sh.size.x * 0.49, sh.size.y * 0.49);
    assert_eq!(hit_symbol(&symbols, inside), Some(h));
    let outside = sh.center + vec2(sh.size.x * 0.5 + 2.0, 0.0);
    assert_ne!(hit_symbol(&symbols, outside), Some(h));
    assert_eq!(hit_symbol(&symbols, Pos2::new(799.0, 599.0)), None);
    assert!(hit_symbol(&[], sh.center).is_none());

    // 重なった 2 つの記号は、後ろ（手前に描かれる方）を優先する。
    let mut stacked = symbols.clone();
    let first = stacked.iter().position(|s| s.id == h).unwrap();
    let parallel_idx = stacked.iter().position(|s| s.id == v).unwrap();
    let center = stacked[first].center;
    stacked[parallel_idx].center = center;
    stacked[parallel_idx].size = stacked[first].size;
    let top = stacked.last().unwrap().id;
    assert_eq!(hit_symbol(&stacked, center), Some(top));
}

#[test]
fn cache_is_keyed_by_generation_and_targets() {
    let mut doc = Document::new();
    let a = line(&mut doc, (0.0, 0.0), (100.0, 0.0));
    let b = line(&mut doc, (0.0, 50.0), (100.0, 50.0));
    constrain(&mut doc, Constraint::Horizontal { line: a });
    let mut state = ConstraintPanelState::default();
    let first = state.data(&doc, &[a], None).1.to_vec();
    assert_eq!(state.cached_generation(), Some(doc.generation()));
    // 世代が同じなら、文書の中身が（テストの都合で）変わらない限り同じ結果。
    assert_eq!(state.data(&doc, &[a], None).1, first.as_slice());
    // 対象が変われば作り直す。
    let rows = state.data(&doc, &[b], None).0.to_vec();
    assert!(rows.is_empty());
    // 世代が進めば作り直す。
    let horizontal_b = constrain(&mut doc, Constraint::Horizontal { line: b });
    assert_eq!(state.data(&doc, &[b], None).0, [horizontal_b].as_slice());
    assert_eq!(state.cached_generation(), Some(doc.generation()));
    // 拘束を 1 件選んでいるときは、その 1 件だけ。
    assert_eq!(
        state.data(&doc, &[b], Some(horizontal_b)).0,
        [horizontal_b].as_slice()
    );
    state.reset();
    assert_eq!(state.cached_generation(), None);
}

#[test]
fn japanese_labels_cover_every_kind() {
    for kind in ConstraintKind::ALL {
        let label = kind_label_ja(kind);
        assert!(!label.is_empty() && !label.is_ascii(), "{kind:?}: {label}");
    }
}

/// 書き換えていない欄は、表示を丸めた文字列のまま抜けても確定しない。書き換えていれば、
/// 表示値と同じ文字列でもその値で確定する（Codex adversarial review（2026-10-18）指摘）。
#[test]
fn decide_commit_uses_the_dirty_flag_not_the_displayed_text() {
    let value = 1.23449;
    let shown = format_value(ValueUnit::Mm, value);
    assert_eq!(
        decide_commit(ValueUnit::Mm, value, &shown, false, false),
        EditCommit::Keep
    );
    let typed: f64 = shown.parse().unwrap();
    assert_eq!(
        decide_commit(ValueUnit::Mm, value, &shown, true, false),
        EditCommit::Set(typed)
    );
    // Esc は書き換えていても確定しない。不正値は Invalid、同じ値は Keep。
    assert_eq!(
        decide_commit(ValueUnit::Mm, value, "5", true, true),
        EditCommit::Keep
    );
    assert_eq!(
        decide_commit(ValueUnit::Mm, value, "abc", true, false),
        EditCommit::Invalid
    );
    assert_eq!(
        decide_commit(ValueUnit::Mm, 5.0, "5", true, false),
        EditCommit::Keep
    );
    // 角度は今の向き（負）へ符号をそろえる。
    assert_eq!(
        decide_commit(ValueUnit::Deg, -0.5, "90", true, false),
        EditCommit::Set(-FRAC_PI_2)
    );
}

/// 階数の計算が重すぎる成分は計算せず、その旨を出す（M13 タスク101 後半）。
#[test]
fn huge_components_are_reported_without_computing_the_rank() {
    let mut doc = Document::new();
    let layer = doc.current_layer();
    let node = |i: u32| p(10.0 * f64::from(i), if i % 2 == 1 { 5.0 } else { 0.0 });
    let links = doc
        .apply(Command::Batch(
            (0..200)
                .map(|i| {
                    Command::AddEntity(Entity::new(
                        Shape::Line(LineSeg::new(node(i), node(i + 1))),
                        layer,
                        Style::inherited(),
                    ))
                })
                .collect(),
        ))
        .unwrap()
        .entities;
    let mut constraints: Vec<Command> = links
        .windows(2)
        .map(|pair| Command::AddConstraint {
            constraint: Constraint::Coincident {
                a: PointRef::new(pair[0], PointKind::End),
                b: PointRef::new(pair[1], PointKind::Start),
            },
            hold: None,
        })
        .collect();
    for &link in &links {
        constraints.push(Command::AddConstraint {
            constraint: Constraint::distance(
                &doc,
                PointRef::new(link, PointKind::Start),
                PointRef::new(link, PointKind::End),
            )
            .unwrap(),
            hold: None,
        });
    }
    doc.apply(Command::Batch(constraints)).unwrap();

    let comps = dof_components(&doc, &[links[0]]);
    assert_eq!(comps.len(), 1);
    assert_eq!(comps[0].dof, None);
    assert!(comps[0].too_large);
    assert_eq!(
        dof_lines(&comps),
        vec!["自由度: 大きすぎるため計算していません".to_owned()]
    );

    // 小さい成分と並ぶと、成分ごとの行で区別できる。
    let small = line(&mut doc, (0.0, 500.0), (10.0, 500.0));
    let comps = dof_components(&doc, &[links[0], small]);
    assert_eq!(comps.len(), 2);
    let lines = dof_lines(&comps);
    assert_eq!(
        lines[0],
        "成分 1(図形 1 個): 自由度 大きすぎるため計算していません"
    );
    assert_eq!(lines[1], "成分 2(図形 1 個): 自由度 4");
}
