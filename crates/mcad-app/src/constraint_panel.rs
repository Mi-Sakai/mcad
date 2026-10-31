//! 右パネル「拘束」セクション（M13 タスク100、DESIGN.md M13 設計判断6）。
//!
//! 純関数（対象の絞り込み・一覧・連結成分ごとの自由度・値の解析・記号のヒットテスト）と、
//! egui への表示（[`constraint_panel`]）を分ける。`Document` の変更はすべて
//! `Document::apply`（`SetConstraintValue`・`RemoveConstraint`）経由。
//!
//! # 自由度のキャッシュ
//!
//! `Document::degrees_of_freedom_bounded` はヤコビアンの階数計算を伴うので毎フレーム呼ばない
//! （階数の計算が約 200 ms 相当を超える成分は計算せず、その旨を出す。M13 タスク101 後半）。
//! 文書の世代番号（[`Document::generation`]）と対象図形の並びをキーに一覧・自由度を持つ
//! （世代は undo/redo でも履歴時点の値へ戻るので、同じ世代 = 同じ内容）。ファイル読込・新規は
//! 世代が `0` へ戻るため、`McadApp::reset_transient_ui_state` が [`ConstraintPanelState::reset`]
//! で捨てる。

use std::collections::HashMap;

use egui::Pos2;
use mcad_core::{Command, Constraint, ConstraintId, Document, DofOutcome, EntityId};
use mcad_geom::Shape;

use crate::constraint_symbols::{ConstraintSymbol, format_number};
use crate::constraint_tool::{ConstraintKind, ValueUnit, align_angle_sign, normalize_angle};
use crate::tool::SelectTool;
use crate::{StatusMessage, set_status};

/// 日本語の種類名（右パネルは日本語の領域。コンボ・ステータスバーの英語名は
/// [`ConstraintKind::label`]）。
pub fn kind_label_ja(kind: ConstraintKind) -> &'static str {
    match kind {
        ConstraintKind::Coincident => "一致",
        ConstraintKind::Horizontal => "水平",
        ConstraintKind::Vertical => "垂直",
        ConstraintKind::Distance => "距離",
        ConstraintKind::PointLineDistance => "点と線の距離",
        ConstraintKind::Parallel => "平行",
        ConstraintKind::Perpendicular => "直交",
        ConstraintKind::EqualLength => "等長",
        ConstraintKind::EqualRadius => "等半径",
        ConstraintKind::Radius => "半径",
        ConstraintKind::Angle => "角度",
        ConstraintKind::Tangent => "接線",
        ConstraintKind::FixPoint => "点の固定",
        ConstraintKind::FixEntity => "図形の固定",
    }
}

/// 拘束の対象になれる図形（線分・円・円弧）か。
fn is_constrainable(document: &Document, id: EntityId) -> bool {
    matches!(
        document.entity(id).and_then(|e| e.geom.as_shape()),
        Some(Shape::Line(_) | Shape::Circle(_) | Shape::Arc(_))
    )
}

/// 選択集合のうち拘束の対象になれる図形（選択の順を保つ。重複は 1 回）。
pub fn target_entities(document: &Document, selection: &[EntityId]) -> Vec<EntityId> {
    let mut out: Vec<EntityId> = Vec::new();
    for &id in selection {
        if is_constrainable(document, id) && !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// `targets` のどれかに付いた拘束（重複なし、[`ConstraintId`] の昇順）。
/// 拘束が 1 件も無い文書では走査しない（検収 (xiii)）。
pub fn rows_for(document: &Document, targets: &[EntityId]) -> Vec<ConstraintId> {
    if document.constraint_count() == 0 || targets.is_empty() {
        return Vec::new();
    }
    document
        .constraints()
        .filter(|(_, c)| targets.iter().any(|&e| c.references(e)))
        .map(|(id, _)| id)
        .collect()
}

/// 対象図形を含む連結成分 1 つ分の自由度。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentDof {
    /// この成分に属する対象図形（`targets` の順）。
    pub members: Vec<EntityId>,
    /// 自由度。求められなければ（大きすぎて計算していない場合を含む）`None`。
    pub dof: Option<usize>,
    /// 成分が大きすぎて階数を計算していない（`dof` は `None`。M13 タスク101 後半）。
    pub too_large: bool,
}

/// `targets` を連結成分ごとにまとめ、成分ごとの自由度を求める。
///
/// 成分は拘束の `entities()` を辺にした union-find で求める（拘束の無い図形は 1 成分）。
/// 成分の並びは、各成分に属する最初の対象図形の `targets` での順。
pub fn dof_components(document: &Document, targets: &[EntityId]) -> Vec<ComponentDof> {
    if targets.is_empty() {
        return Vec::new();
    }
    let mut parent: HashMap<EntityId, EntityId> = HashMap::new();
    fn find(parent: &mut HashMap<EntityId, EntityId>, x: EntityId) -> EntityId {
        let mut root = x;
        while let Some(&p) = parent.get(&root) {
            if p == root {
                break;
            }
            root = p;
        }
        // 経路圧縮。
        let mut cur = x;
        while let Some(&p) = parent.get(&cur) {
            if p == root {
                break;
            }
            parent.insert(cur, root);
            cur = p;
        }
        root
    }
    for (_, constraint) in document.constraints() {
        let entities = constraint.entities();
        for &e in &entities {
            parent.entry(e).or_insert(e);
        }
        if let Some((&first, rest)) = entities.split_first() {
            for &other in rest {
                let a = find(&mut parent, first);
                let b = find(&mut parent, other);
                if a != b {
                    parent.insert(b, a);
                }
            }
        }
    }
    let mut groups: Vec<(EntityId, Vec<EntityId>)> = Vec::new();
    for &t in targets {
        let root = if parent.contains_key(&t) {
            find(&mut parent, t)
        } else {
            t
        };
        match groups.iter_mut().find(|(r, _)| *r == root) {
            Some((_, members)) => members.push(t),
            None => groups.push((root, vec![t])),
        }
    }
    groups
        .into_iter()
        .map(|(_, members)| {
            let (dof, too_large) = match document.degrees_of_freedom_bounded(members[0]) {
                DofOutcome::Known(dof) => (Some(dof), false),
                DofOutcome::TooLarge => (None, true),
                DofOutcome::Unknown => (None, false),
            };
            ComponentDof {
                members,
                dof,
                too_large,
            }
        })
        .collect()
}

/// 自由度の表示行。成分が 1 つなら `自由度: n`、複数なら成分ごとに 1 行。
pub fn dof_lines(components: &[ComponentDof]) -> Vec<String> {
    let value = |c: &ComponentDof| match c.dof {
        Some(n) => n.to_string(),
        None if c.too_large => "大きすぎるため計算していません".to_owned(),
        None => "不明".to_owned(),
    };
    match components {
        [] => Vec::new(),
        [only] => vec![format!("自由度: {}", value(only))],
        many => many
            .iter()
            .enumerate()
            .map(|(i, c)| {
                format!(
                    "成分 {}(図形 {} 個): 自由度 {}",
                    i + 1,
                    c.members.len(),
                    value(c)
                )
            })
            .collect(),
    }
}

/// 値の編集欄の初期表示（mm は小数 3 桁、度は小数 2 桁まで）。
pub fn format_value(unit: ValueUnit, value: f64) -> String {
    match unit {
        ValueUnit::Mm => format_number(value, 3),
        ValueUnit::Deg => format_number(value.to_degrees(), 2),
    }
}

/// 編集欄の文字列を保存単位の値（mm はワールド単位、角度はラジアン）へ解析する。
/// 不正値（空・数値でない・非有限）は `None`。度→ラジアンはここで 1 回だけ変換し、
/// `(−π, π]` へ正規化する。範囲（正など）は core が `SetConstraintValue` で確かめる。
pub fn parse_edit_value(unit: ValueUnit, text: &str) -> Option<f64> {
    let v: f64 = text.trim().parse().ok().filter(|v: &f64| v.is_finite())?;
    Some(match unit {
        ValueUnit::Mm => v,
        ValueUnit::Deg => normalize_angle(v.to_radians()),
    })
}

/// 画面上の点 `pos` にある拘束記号（記号の矩形 = `center`±`size/2`）。重なっていれば
/// 後ろ（= 手前に描かれた）記号を優先する。
pub fn hit_symbol(symbols: &[ConstraintSymbol], pos: Pos2) -> Option<ConstraintId> {
    symbols
        .iter()
        .rev()
        .find(|s| egui::Rect::from_center_size(s.center, s.size).contains(pos))
        .map(|s| s.id)
}

/// 右パネルが持つ状態（キャッシュと値の編集中バッファ）。
#[derive(Debug, Default)]
pub struct ConstraintPanelState {
    cache: Option<Cache>,
    /// 値を編集中の行。
    edit: Option<EditBuffer>,
}

/// 値の編集中バッファ。`dirty` は利用者が文字列を書き換えたか（`TextEdit::changed`）で、
/// 文字列が表示値と同じかでは判定しない（表示値と同じ文字列を打ち直して丸めた値へ
/// 確定する操作を捨てないため。Codex adversarial review（2026-10-18）指摘）。
#[derive(Debug, Clone)]
struct EditBuffer {
    id: ConstraintId,
    text: String,
    dirty: bool,
}

/// 値の欄からフォーカスが外れたときの扱い（[`decide_commit`]）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EditCommit {
    /// 確定しない（書き換えていない・`Esc`・値が変わらない）。
    Keep,
    /// この値（保存単位）で `SetConstraintValue` を出す。
    Set(f64),
    /// 不正な文字列。
    Invalid,
}

/// フォーカスが外れた値の欄を確定するかを決める。`value` は現在の値（保存単位）。
/// 書き換えていなければ、表示を丸めた文字列のまま抜けても確定しない（値を丸めない）。
/// 角度は向きを今の形に合わせる（[`align_angle_sign`]）。
pub fn decide_commit(
    unit: ValueUnit,
    value: f64,
    text: &str,
    dirty: bool,
    escaped: bool,
) -> EditCommit {
    if escaped || !dirty {
        return EditCommit::Keep;
    }
    let parsed = parse_edit_value(unit, text).map(|v| match unit {
        ValueUnit::Mm => v,
        ValueUnit::Deg => align_angle_sign(v, value),
    });
    match parsed {
        Some(v) if v != value => EditCommit::Set(v),
        Some(_) => EditCommit::Keep,
        None => EditCommit::Invalid,
    }
}

#[derive(Debug)]
struct Cache {
    generation: u64,
    targets: Vec<EntityId>,
    single: Option<ConstraintId>,
    rows: Vec<ConstraintId>,
    components: Vec<ComponentDof>,
}

impl ConstraintPanelState {
    /// キャッシュと編集中バッファを捨てる（新規・読込・undo/redo）。
    pub fn reset(&mut self) {
        self.cache = None;
        self.edit = None;
    }

    /// `targets` の一覧と自由度（世代と対象が同じならキャッシュを返す）。
    /// `single` が `Some` なら一覧はその拘束 1 件に絞る（拘束を選んでいるとき）。
    fn data(
        &mut self,
        document: &Document,
        targets: &[EntityId],
        single: Option<ConstraintId>,
    ) -> (&[ConstraintId], &[ComponentDof]) {
        let fresh = self.cache.as_ref().is_some_and(|c| {
            c.generation == document.generation() && c.targets == targets && c.single == single
        });
        if !fresh {
            self.cache = Some(Cache {
                generation: document.generation(),
                targets: targets.to_vec(),
                single,
                rows: match single {
                    Some(id) => vec![id],
                    None => rows_for(document, targets),
                },
                components: dof_components(document, targets),
            });
        }
        let c = self.cache.as_ref().expect("cache was just filled");
        (&c.rows, &c.components)
    }

    /// テスト用: キャッシュを作り直した回数が分かるよう、現在のキャッシュの世代。
    #[cfg(test)]
    fn cached_generation(&self) -> Option<u64> {
        self.cache.as_ref().map(|c| c.generation)
    }
}

/// 拘束 1 件を `apply(RemoveConstraint)`（undo 1 回）で消す。成功なら `true`。
pub fn remove_constraint(
    document: &mut Document,
    id: ConstraintId,
    status: &mut Option<StatusMessage>,
    now: f64,
) -> bool {
    match document.apply(Command::RemoveConstraint(id)) {
        Ok(_) => {
            set_status(status, now, "Removed constraint");
            true
        }
        Err(err) => {
            set_status(status, now, format!("Constraint remove failed: {err}"));
            false
        }
    }
}

/// 右パネル「拘束」セクションの 1 フレーム分の結果。
#[derive(Debug, Default, Clone, Copy)]
pub struct PanelOutcome {
    /// 「拘束記号を表示」が切り替わった。
    pub visibility_changed: bool,
}

/// 右パネル「拘束」セクション。`hovered` には、マウスが乗っている行の拘束を入れる
/// （呼び出し側が毎フレーム `None` へ戻してから呼ぶ）。
#[allow(clippy::too_many_arguments)]
pub fn constraint_panel(
    ui: &mut egui::Ui,
    document: &mut Document,
    select_tool: &SelectTool,
    state: &mut ConstraintPanelState,
    hovered: &mut Option<ConstraintId>,
    symbols_visible: &mut bool,
    status: &mut Option<StatusMessage>,
    now: f64,
) -> PanelOutcome {
    ui.separator();
    ui.heading("拘束");
    let mut outcome = PanelOutcome::default();
    if ui.checkbox(symbols_visible, "拘束記号を表示").changed() {
        outcome.visibility_changed = true;
    }

    let selected = select_tool
        .selected_constraint()
        .filter(|&id| document.constraint(id).is_some());
    let targets: Vec<EntityId> = match selected {
        Some(id) => document
            .constraint(id)
            .map(Constraint::entities)
            .unwrap_or_default(),
        None => target_entities(document, select_tool.selection()),
    };
    if targets.is_empty() {
        state.edit = None;
        ui.label("拘束の対象になる図形を選ぶと表示します");
        return outcome;
    }

    let (rows, components) = {
        let (rows, components) = state.data(document, &targets, selected);
        (rows.to_vec(), components.to_vec())
    };
    // 編集中の行が消えていたら（削除・undo）バッファを捨てる。
    if state.edit.as_ref().is_some_and(|e| !rows.contains(&e.id)) {
        state.edit = None;
    }

    if rows.is_empty() {
        ui.label("この図形に付いた拘束はありません");
    }
    let mut remove: Option<ConstraintId> = None;
    let mut set_value: Option<(ConstraintId, f64)> = None;
    for &id in &rows {
        let Some(constraint) = document.constraint(id) else {
            continue;
        };
        let kind = ConstraintKind::of(constraint);
        let current = constraint.value();
        let extended = constraint
            .tangent_contact(document)
            .is_some_and(|c| c.is_extended());
        let row = ui.horizontal_wrapped(|ui| {
            ui.label(kind_label_ja(kind));
            if let (Some(unit), Some(value)) = (kind.value_unit(), current) {
                let editing = state.edit.as_ref().filter(|e| e.id == id).cloned();
                let was_dirty = editing.as_ref().is_some_and(|e| e.dirty);
                let mut text = editing.map_or_else(|| format_value(unit, value), |e| e.text);
                let mut response =
                    ui.add(egui::TextEdit::singleline(&mut text).desired_width(64.0));
                if unit == ValueUnit::Deg {
                    response = response.on_hover_text(
                        "1本目から2本目へ反時計回りに測った角。符号は今の向きに合わせます",
                    );
                }
                ui.label(match unit {
                    ValueUnit::Mm => "mm",
                    ValueUnit::Deg => "度",
                });
                let dirty = was_dirty || response.changed();
                if response.lost_focus() {
                    let escaped = ui.input(|i| i.key_pressed(egui::Key::Escape));
                    state.edit = None;
                    match decide_commit(unit, value, &text, dirty, escaped) {
                        EditCommit::Set(v) => set_value = Some((id, v)),
                        EditCommit::Keep => {}
                        EditCommit::Invalid => {
                            set_status(status, now, "Constraint value: invalid number");
                        }
                    }
                } else if response.has_focus() || response.changed() {
                    state.edit = Some(EditBuffer { id, text, dirty });
                }
            }
            if extended {
                ui.label("(延長上)");
            }
            if ui.button("削除").clicked() {
                remove = Some(id);
            }
        });
        if ui.rect_contains_pointer(row.response.rect) {
            *hovered = Some(id);
        }
    }

    for line in dof_lines(&components) {
        ui.label(line);
    }

    // 削除ボタンのクリックで値の欄のフォーカスが外れたフレームは、保留中の値の確定を捨てて
    // 削除だけを出す（値の変更と削除が別々の undo 単位になり、undo で編集後の値の拘束が
    // 戻るのを防ぐ。Codex adversarial review（2026-10-18）指摘）。
    if remove.is_some() {
        set_value = None;
    }
    if let Some((id, value)) = set_value
        && let Err(err) = document.apply(Command::SetConstraintValue { id, value })
    {
        set_status(status, now, format!("Constraint value failed: {err}"));
    }
    if let Some(id) = remove {
        remove_constraint(document, id, status, now);
    }
    outcome
}

#[cfg(test)]
mod tests;
