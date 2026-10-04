//! [`Document`] の幾何拘束まわり（M13 タスク96・97、DESIGN.md M13 設計判断2〜4）。
//!
//! 読み取り API、拘束スロットの書き換え（墓標方式）、エンティティ削除・切り詰めに伴う
//! 拘束の削除、コマンド適用後の求解を置く。`document.rs` の `apply` / `execute` / `revert` /
//! `reapply` から呼ばれる。
//!
//! # 拘束の無い文書
//!
//! 生存している拘束の数を `live_constraints` に持ち、0 のときは走査も方程式の組み立ても
//! せずに戻る（拘束を使わない利用者 — tcad を含む — の動作と性能を変えない）。
//!
//! # 求解の流れ（タスク97、設計判断3）
//!
//! [`Document::apply`] は、最も外側のコマンドを `execute` し終えた後に 1 回だけ
//! `follow_constraints` を呼ぶ。中身は 3 段:
//!
//! 1. **何が変わったかを集める**（[`Edits::collect`]）: 逆操作記録 `Applied` を適用順に歩き、
//!    幾何を差し替えたエンティティと**適用開始時の幾何**（エンティティごとに最初に現れた
//!    `before`。`Batch` の中で何度変わっても、比べるのは開始時と最終形）、追加・値変更・
//!    差し替え・削除した拘束が参照するエンティティ、`AddConstraint` の `hold` を取り出す。
//! 2. **解く**（[`Document::solve_constraints`]。`&self` の純関数で、文書を変えない）:
//!    1 で集めたエンティティを含む**連結成分**（拘束を辺、エンティティを頂点とするグラフ）
//!    ごとに方程式系を組み、満たされていれば何もせず、満たされていなければ
//!    `mcad_geom::solve::solve` で解いて「追従する修正」の一覧を返す。
//! 3. **書き戻す**: 修正を `set_entity_geom` で入れ、`Applied::ModifyEntity` として元の記録と
//!    一緒に `Applied::Batch` へ包む（undo 1 回で全体が戻る）。2 が失敗したら元の記録を
//!    `revert` する。
//!
//! 2 を 1・3 から切り離してあるのは、グリップのドラッグ中のプレビュー（タスク101）が
//! 履歴に積まずに同じ規則で解けるようにするため。
//!
//! # 1 つの成分の解き方（[`settle`]）
//!
//! 1. 拘束が参照する図形が解く前から退化していれば [`CoreError::DegenerateShape`]。
//! 2. すべての拘束が満たされていれば**解かずに通す**（構造条件だけ確かめる）。
//! 3. 満たされていない拘束のうち、参照するパラメータがすべて止められているものがあれば、
//!    反復せずに [`CoreError::ConstraintBlockedByFixed`]。
//! 4. 解く。ソルバーが停滞・試行回数の上限で失敗したら、**同じ開始状態・同じ固定集合のまま、
//!    大きい初期減衰（`mcad_geom::solve::LAMBDA_RETRY`）で 1 回だけ解き直す**。既定の初期減衰は
//!    ほぼ 0 で、拘束を満たした形からの編集（ふつうの追従）を最小ノルムどおりに 1〜2 回で
//!    解くための値。解から遠い大きな系では 1 回目のステップが行き過ぎて収束しないことがあり、
//!    そのときだけ減衰を効かせた反復へ切り替える。解き直しも失敗したら
//!    [`CoreError::ConstraintUnsolvable`]。ただし途中の試行点を退化で棄却していたら
//!    [`CoreError::DegenerateShape`]（2 回の求解を通して最初に棄却された図形。退化した形を
//!    経由しないと残差を減らせなかった、という報告で、退化だけが原因とは限らない）。
//! 5. 解いた形を確かめる: 退化（`System::first_inadmissible`・`System::collapsed`）、
//!    構造条件（`System::first_structure_violation`）、解いた形から計算し直した許容値での
//!    充足（`System::first_unsatisfied_at`。`solve` が保証しているが、文書へ入れる形に
//!    対してもう一度確かめる）。
//!
//! `hold` のある成分は、`hold` を止めて 1〜5 を行い（4 の解き直しを含む）、失敗したら外して
//! やり直す。順は「hold あり・既定の初期減衰 → hold あり・解き直し → hold なし・既定 →
//! hold なし・解き直し」。
//!
//! # 決定性
//!
//! 成分は最小の [`ConstraintId`] の順、成分の中の方程式は [`ConstraintId`] 順、パラメータは
//! [`EntityId`] 順、返す修正は [`EntityId`] 順。乱数は使わない。

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

use mcad_geom::Shape;
use mcad_geom::solve::{self, Problem, SolveError};

use super::{Applied, Document};
use crate::constraint::equations::System;
use crate::constraint::{ReshapeBlocked, reshape_removals, shape_kind};
use crate::{Constraint, ConstraintId, CoreError, EntityGeom, EntityId};

impl Document {
    // ---- 読み取り系 ----

    /// 指定 ID の拘束を参照する。存在しない/削除済みなら `None`。
    #[must_use]
    pub fn constraint(&self, id: ConstraintId) -> Option<&Constraint> {
        self.constraints.get(id).and_then(Option::as_ref)
    }

    /// 生存している拘束を `(ID, &Constraint)` で、[`ConstraintId`] の昇順に列挙する。
    ///
    /// スロットは解放しない（墓標方式）ので、この順は追加した順でもある。
    pub fn constraints(&self) -> impl Iterator<Item = (ConstraintId, &Constraint)> {
        self.constraints
            .iter()
            .filter_map(|(k, v)| v.as_ref().map(|c| (k, c)))
    }

    /// `entity` を参照する拘束を [`ConstraintId`] の昇順に列挙する。生存している拘束を
    /// 走査する O(n)。
    pub fn constraints_of(
        &self,
        entity: EntityId,
    ) -> impl Iterator<Item = (ConstraintId, &Constraint)> {
        self.constraints()
            .filter(move |(_, c)| c.references(entity))
    }

    /// 生存している拘束の数（O(1)）。
    ///
    /// [`crate::Command::RemoveEntity`] / [`crate::Command::ReshapeEntity`] が連鎖して消した
    /// 拘束の件数は、`apply` の前後でこの値の差を取れば分かる。
    #[must_use]
    pub fn constraint_count(&self) -> usize {
        self.live_constraints
    }

    // ---- スロットの書き換え（`live_constraints` をここだけで保つ） ----

    pub(super) fn require_constraint(&self, id: ConstraintId) -> Result<&Constraint, CoreError> {
        self.constraint(id).ok_or(CoreError::ConstraintNotFound(id))
    }

    /// 新しいスロットへ拘束を入れ、発行した ID を返す。
    pub(super) fn insert_constraint(&mut self, constraint: Constraint) -> ConstraintId {
        self.live_constraints += 1;
        self.constraints.insert(Some(constraint))
    }

    /// スロット（キーは有効な前提）の値を差し替える。
    pub(super) fn set_constraint(&mut self, id: ConstraintId, value: Option<Constraint>) {
        let slot = self
            .constraints
            .get_mut(id)
            .expect("constraint slot must exist");
        match (slot.is_some(), value.is_some()) {
            (false, true) => self.live_constraints += 1,
            (true, false) => self.live_constraints -= 1,
            _ => {}
        }
        *slot = value;
    }

    /// 生存している拘束を取り出してスロットを墓標化する。生存を前提とする。
    pub(super) fn take_constraint(&mut self, id: ConstraintId) -> Constraint {
        let constraint = self
            .constraints
            .get_mut(id)
            .expect("constraint key must exist")
            .take()
            .expect("constraint must be live");
        self.live_constraints -= 1;
        constraint
    }

    /// `entity` を参照する拘束をすべて墓標化し、`(ID, 拘束)` を ID 順で返す
    /// （[`crate::Command::RemoveEntity`] の連鎖削除。undo で同じスロットへ戻す）。
    pub(super) fn take_constraints_of(
        &mut self,
        entity: EntityId,
    ) -> Vec<(ConstraintId, Constraint)> {
        if self.live_constraints == 0 {
            return Vec::new();
        }
        let ids: Vec<ConstraintId> = self.constraints_of(entity).map(|(id, _)| id).collect();
        ids.into_iter()
            .map(|id| (id, self.take_constraint(id)))
            .collect()
    }

    /// 連鎖削除した拘束を同じスロットへ戻す（undo 方向）。
    pub(super) fn restore_constraints(&mut self, removed: &[(ConstraintId, Constraint)]) {
        for (id, constraint) in removed {
            self.set_constraint(*id, Some(constraint.clone()));
        }
    }

    /// 連鎖削除した拘束を再び墓標化する（redo 方向）。
    pub(super) fn bury_constraints(&mut self, removed: &[(ConstraintId, Constraint)]) {
        for (id, _) in removed {
            self.set_constraint(*id, None);
        }
    }

    // ---- 幾何の変更と拘束 ----

    /// エンティティ `id` の幾何を `new_geom` へ変えるときの拘束の扱いを決める
    /// （状態は変えない）。戻り値は消す拘束の ID（`reshape == false` なら常に空）。
    ///
    /// 幾何が変わらない（意味的 no-op）なら何もしない。変わるとき:
    ///
    /// - `reshape == false`（[`crate::Command::ModifyEntity`]）: 図形の固定がついていれば
    ///   [`CoreError::EntityFixed`]。幾何の種類が変わり、既存の拘束が指せなくなるなら
    ///   [`CoreError::InvalidConstraint`]（切り詰めは `ReshapeEntity` の仕事）。拘束は消さない。
    /// - `reshape == true`（[`crate::Command::ReshapeEntity`]）: 設計判断4 の規則
    ///   （`reshape_removals`）で消す拘束を決める。固定に阻まれれば [`CoreError::EntityFixed`]。
    ///   このエンティティを参照する拘束があるのに、支持する直線・円が変わる変更（移動・回転・
    ///   向きの反転・中心や半径の変更）なら [`CoreError::InvalidConstraint`]（拘束は消さない）。
    ///
    /// 判定は「その時点の状態」に対して行う（ロック・部品表の実行前チェックと同じく、
    /// [`crate::Command::Batch`] のサブコマンドごと）。
    pub(super) fn geom_change_effects(
        &self,
        id: EntityId,
        new_geom: &EntityGeom,
        reshape: bool,
    ) -> Result<Vec<ConstraintId>, CoreError> {
        if self.live_constraints == 0 {
            return Ok(Vec::new());
        }
        let Some(entity) = self.entity(id) else {
            return Err(CoreError::EntityNotFound(id));
        };
        let before = &entity.geom;
        if before == new_geom {
            return Ok(Vec::new());
        }
        if reshape {
            return reshape_removals(id, before, new_geom, self.constraints_of(id)).map_err(
                |blocked| match blocked {
                    ReshapeBlocked::Fixed(_) => CoreError::EntityFixed(id),
                    ReshapeBlocked::SupportChanged(reason) => {
                        CoreError::InvalidConstraint(format!(
                            "ReshapeEntity on the constrained {id:?} must keep its supporting \
                             line or circle ({reason}); use ModifyEntity to move it"
                        ))
                    }
                },
            );
        }
        if self
            .constraints_of(id)
            .any(|(_, c)| matches!(c, Constraint::FixEntity { .. }))
        {
            return Err(CoreError::EntityFixed(id));
        }
        let new_kind = shape_kind(new_geom);
        if shape_kind(before) != new_kind {
            for (cid, constraint) in self.constraints_of(id) {
                let fits = constraint
                    .targets()
                    .filter(|(target, _)| *target == id)
                    .all(|(_, role)| role.fits(new_kind));
                if !fits {
                    return Err(CoreError::InvalidConstraint(format!(
                        "the new geometry of {id:?} cannot be referenced by {cid:?}"
                    )));
                }
            }
        }
        Ok(Vec::new())
    }

    // ---- 自由度 ----

    /// `entity` を含む連結成分（拘束でつながった図形のまとまり）の自由度（M13 設計判断1・6。
    /// 右パネルの表示用）。
    ///
    /// 自由度 = 止められていないパラメータの数 − 拘束の方程式のヤコビアンの階数
    /// （`mcad_geom::solve::degrees_of_freedom`）。パラメータは線分 4・円 3・円弧 5。
    /// 図形の固定（[`Constraint::FixEntity`]）の対象と、ロックされたレイヤー上の図形は
    /// 止められたものとして数える。完全に拘束された成分は 0。
    ///
    /// - `entity` が存在しない、または拘束の対象にできる図形（線分・円・円弧）でなければ `None`。
    /// - `entity` を参照する拘束が無ければ、その図形だけのパラメータ数（ロックされた
    ///   レイヤー上なら 0）。
    /// - 成分のヤコビアンが有限にならない（座標が極端に大きい）ときも `None`。
    ///
    /// 生存している拘束を 1 回走査する O(n)。
    #[must_use]
    pub fn degrees_of_freedom(&self, entity: EntityId) -> Option<usize> {
        let target = self.entity(entity)?;
        let kind = shape_kind(&target.geom)?;
        let seeds = BTreeSet::from([entity]);
        let Some(component) = self.components_of(&seeds).into_iter().next() else {
            let locked = self.layer(target.layer).is_some_and(|layer| layer.locked);
            return Some(if locked { 0 } else { kind.param_count() });
        };
        let sys = self.component_system(&component, &Edits::default()).ok()?;
        let weights = sys.weights();
        let problem = Problem::new(&sys.params, &sys.fixed, &weights, &sys.equations);
        solve::degrees_of_freedom(&problem).ok()
    }

    // ---- 求解 ----

    /// コマンド適用後の求解（[`Document::apply`] が最も外側で 1 回だけ呼ぶ。モジュール doc
    /// 「求解の流れ」）。
    ///
    /// 成功したら、追従した修正を文書へ書き戻し、それを足した逆操作記録を返す（追従が無ければ
    /// `applied` をそのまま返す）。失敗したら `applied` を巻き戻して（= コマンドを適用する前の
    /// 状態へ戻して）エラーを返す。
    pub(super) fn follow_constraints(&mut self, applied: Applied) -> Result<Applied, CoreError> {
        if self.live_constraints == 0 {
            return Ok(applied);
        }
        let mut edits = Edits::default();
        edits.collect(&applied);
        let follow_ups = match self.solve_constraints(&edits) {
            Ok(follow_ups) => follow_ups,
            Err(err) => {
                self.revert(&applied);
                return Err(err);
            }
        };
        if follow_ups.changes.is_empty() {
            return Ok(applied);
        }
        // 元の記録の後ろに、追従した修正を EntityId 順で並べる。undo は逆順（追従 → 元）、
        // redo は正順（元 → 追従）に辿るので、同じエンティティが両方に現れても正しく戻る。
        let mut records = Vec::with_capacity(follow_ups.changes.len() + 1);
        records.push(applied);
        for (id, after) in follow_ups.changes {
            let before = self
                .entity(id)
                .expect("a solved entity must be live")
                .geom
                .clone();
            self.set_entity_geom(id, after.clone());
            records.push(Applied::ModifyEntity {
                id,
                before,
                after,
                removed_constraints: Vec::new(),
            });
        }
        Ok(Applied::Batch(records))
    }

    /// 文書の現在の形（コマンドを適用した後の形）と `edits` から、拘束を満たすために
    /// **追従させる修正**を求める（モジュール doc「求解の流れ」の 2）。文書は変えない。
    ///
    /// 返す修正は、解いた形が今の形と違うエンティティだけで、[`EntityId`] 順。関わる成分が
    /// すべて満たされていれば空。
    ///
    /// # Errors
    ///
    /// 解けない成分が 1 つでもあれば、最初の成分（最小の [`ConstraintId`] の順）のエラー
    /// （モジュール doc「1 つの成分の解き方」）。
    pub(super) fn solve_constraints(&self, edits: &Edits) -> Result<FollowUps, CoreError> {
        let mut follow_ups = FollowUps::default();
        if self.live_constraints == 0 {
            return Ok(follow_ups);
        }
        let seeds: BTreeSet<EntityId> = edits
            .geoms
            .keys()
            .chain(&edits.constrained)
            .copied()
            .filter(|id| self.entity(*id).is_some())
            .collect();
        if seeds.is_empty() {
            return Ok(follow_ups);
        }
        // 同じ apply の中で消えた拘束の hold は無視する。
        let holds: BTreeSet<EntityId> = edits
            .holds
            .iter()
            .filter(|(constraint, _)| self.constraint(*constraint).is_some())
            .map(|(_, entity)| *entity)
            .collect();
        for component in self.components_of(&seeds) {
            let sys = self.component_system(&component, edits)?;
            let mut held = sys.fixed.clone();
            let mut any_hold = false;
            for slot in holds.iter().filter_map(|entity| sys.slot_of(*entity)) {
                held[slot.params()].fill(true);
                any_hold = true;
            }
            let settled = if any_hold {
                // 先に選んだ図形を止めて解き、満たせないときに限り止めずに解き直す。
                match settle(&sys, &held) {
                    Ok(settled) => settled,
                    Err(with_hold) => {
                        let mut settled = settle(&sys, &sys.fixed).map_err(|f| f.error)?;
                        settled.trials += with_hold.trials;
                        settled
                    }
                }
            } else {
                settle(&sys, &sys.fixed).map_err(|f| f.error)?
            };
            follow_ups.trials += settled.trials;
            let Some(x) = settled.params else {
                continue;
            };
            for slot in &sys.slots {
                if x[slot.params()] == sys.params[slot.params()] {
                    continue;
                }
                let shape = sys
                    .shape(slot.entity, &x)
                    .expect("the slot belongs to the system");
                follow_ups
                    .changes
                    .push((slot.entity, EntityGeom::Shape(shape)));
            }
        }
        // 成分はエンティティを共有しないので、並べ替えるだけで EntityId 順になる。
        follow_ups.changes.sort_by_key(|(id, _)| *id);
        Ok(follow_ups)
    }

    /// `seeds` のどれかを含む連結成分（拘束を辺、エンティティを頂点とするグラフ）を、成分ごとの
    /// 拘束の ID 一覧（[`ConstraintId`] 順）で返す。成分は最小の [`ConstraintId`] の順。
    /// 拘束に参照されていない `seeds` は成分を作らない。
    ///
    /// 生存している拘束を 1 回走査する（索引は持たない）。
    fn components_of(&self, seeds: &BTreeSet<EntityId>) -> Vec<Vec<ConstraintId>> {
        if self.live_constraints == 0 || seeds.is_empty() {
            return Vec::new();
        }
        // 素集合森（union-find）。頂点は、拘束に参照されるエンティティへ現れた順に振る番号。
        fn find(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        let mut index: BTreeMap<EntityId, usize> = BTreeMap::new();
        let mut parent: Vec<usize> = Vec::new();
        // 拘束ごとの、最初の対象の頂点番号（拘束は必ず 1 つ以上の対象を持つ）。
        let mut anchors: Vec<(ConstraintId, usize)> = Vec::with_capacity(self.live_constraints);
        for (id, constraint) in self.constraints() {
            let mut anchor: Option<usize> = None;
            for (entity, _) in constraint.targets() {
                let vertex = *index.entry(entity).or_insert_with(|| {
                    parent.push(parent.len());
                    parent.len() - 1
                });
                match anchor {
                    None => anchor = Some(vertex),
                    Some(first) => {
                        let (a, b) = (find(&mut parent, first), find(&mut parent, vertex));
                        // 番号の小さい方を根にする（結果は根の選び方に依らないが、決定的にする）。
                        parent[a.max(b)] = a.min(b);
                    }
                }
            }
            if let Some(anchor) = anchor {
                anchors.push((id, anchor));
            }
        }
        let wanted: BTreeSet<usize> = seeds
            .iter()
            .filter_map(|entity| index.get(entity).copied())
            .map(|vertex| find(&mut parent, vertex))
            .collect();
        if wanted.is_empty() {
            return Vec::new();
        }
        // `constraints()` は ConstraintId 順なので、根が最初に現れた順 = 最小の ID の順。
        let mut order: Vec<usize> = Vec::new();
        let mut groups: BTreeMap<usize, Vec<ConstraintId>> = BTreeMap::new();
        for (id, anchor) in anchors {
            let root = find(&mut parent, anchor);
            if !wanted.contains(&root) {
                continue;
            }
            groups
                .entry(root)
                .or_insert_with(|| {
                    order.push(root);
                    Vec::new()
                })
                .push(id);
        }
        order
            .into_iter()
            .filter_map(|root| groups.remove(&root))
            .collect()
    }

    /// 1 つの成分の方程式系を組み、固定集合（設計判断3）を入れる:
    ///
    /// - (a) 図形の固定の対象（`System::build` が入れる）。
    /// - (b) ロックされたレイヤー上のエンティティの全パラメータ。
    /// - (c) そのコマンドで値が変わったパラメータ（[`moved_params`]）。
    ///
    /// `hold` はここでは入れない（外して解き直すため、呼び出し側が重ねる）。
    fn component_system(
        &self,
        constraints: &[ConstraintId],
        edits: &Edits,
    ) -> Result<System, CoreError> {
        let mut sys = System::build(
            |id| self.entity(id).map(|e| &e.geom),
            constraints
                .iter()
                .filter_map(|&id| self.constraint(id).map(|c| (id, c))),
        )?;
        for i in 0..sys.slots.len() {
            let slot = sys.slots[i];
            let entity = self
                .entity(slot.entity)
                .expect("a constrained entity must be live");
            if self.layer(entity.layer).is_some_and(|layer| layer.locked) {
                sys.fixed[slot.params()].fill(true);
                continue;
            }
            // `None` は「この apply で追加された」= 自由。
            if let Some(Some(before)) = edits.geoms.get(&slot.entity) {
                let moved = moved_params(before, &entity.geom);
                for (fixed, moved) in sys.fixed[slot.params()].iter_mut().zip(moved) {
                    *fixed |= moved;
                }
            }
        }
        Ok(sys)
    }
}

/// 1 回の `apply` が変えたもの（求解の入力。モジュール doc「求解の流れ」の 1）。
#[derive(Debug, Default)]
pub(super) struct Edits {
    /// 幾何を差し替えたエンティティと、**適用開始時**の幾何。その `apply` で追加された
    /// エンティティは `None`（追加されたばかりの図形は自由で、何も止めない）。
    geoms: BTreeMap<EntityId, Option<EntityGeom>>,
    /// 追加・値変更・差し替え・削除した拘束（連鎖削除を含む）が参照するエンティティ。
    constrained: BTreeSet<EntityId>,
    /// [`crate::Command::AddConstraint`] の `hold`（追加した拘束の ID つき）。
    holds: Vec<(ConstraintId, EntityId)>,
}

impl Edits {
    /// 逆操作記録を適用順に歩いて集める。
    pub(super) fn collect(&mut self, applied: &Applied) {
        match applied {
            Applied::AddEntity { id, .. } => {
                self.geoms.insert(*id, None);
            }
            // 同一幾何への差し替え（no-op のサブコマンド）も入るが、何も止めず、その成分は
            // 元から満たされているので結果は変わらない。
            Applied::ModifyEntity {
                id,
                before,
                removed_constraints,
                ..
            } => {
                // 最初に現れた `before` が開始時の形。追加されたエンティティは `None` のまま。
                self.geoms
                    .entry(*id)
                    .or_insert_with(|| Some(before.clone()));
                self.touch_all(removed_constraints);
            }
            Applied::RemoveEntity {
                removed_constraints,
                ..
            } => self.touch_all(removed_constraints),
            Applied::AddConstraint {
                id,
                constraint,
                hold,
            } => {
                self.touch(constraint);
                if let Some(entity) = hold {
                    self.holds.push((*id, *entity));
                }
            }
            // 差し替え（ReplaceConstraint）は対象が変わりうるので、前後の両方を数える。
            Applied::SetConstraintValue { before, after, .. } => {
                self.touch(before);
                self.touch(after);
            }
            Applied::RemoveConstraint { constraint, .. } => self.touch(constraint),
            Applied::Batch(subs) => {
                for sub in subs {
                    self.collect(sub);
                }
            }
            // スタイル・レイヤー・図面設定は形も拘束も変えない（レイヤーのロックは、解くときの
            // 文書の状態から読む）。
            Applied::SetEntityStyle { .. }
            | Applied::SetEntityLayer { .. }
            | Applied::AddLayer { .. }
            | Applied::RemoveLayer { .. }
            | Applied::SetLayerProps { .. }
            | Applied::SetCurrentLayer { .. }
            | Applied::SetSheet { .. }
            | Applied::SetDimStyle { .. } => {}
        }
    }

    fn touch(&mut self, constraint: &Constraint) {
        self.constrained
            .extend(constraint.targets().map(|(entity, _)| entity));
    }

    fn touch_all(&mut self, constraints: &[(ConstraintId, Constraint)]) {
        for (_, constraint) in constraints {
            self.touch(constraint);
        }
    }
}

/// [`Document::solve_constraints`] の結果。
#[derive(Debug, Default)]
pub(super) struct FollowUps {
    /// 追従する修正（解いた形が今の形と違うエンティティと、その新しい幾何。[`EntityId`] 順）。
    pub(super) changes: Vec<(EntityId, EntityGeom)>,
    /// ソルバーの試行回数の合計（解かずに通した成分は 0）。失敗して解き直した求解
    /// （大きい初期減衰での解き直し、`hold` を外した解き直し）の試行も含む。性能の観測用。
    pub(super) trials: usize,
}

/// 1 つの成分を解いた結果。
struct Settled {
    /// 解いたパラメータ。解かずに通したなら `None`。
    params: Option<Vec<f64>>,
    /// ソルバーの試行回数（失敗して解き直した求解のぶんを含む）。
    trials: usize,
}

/// 1 つの成分を満たせなかった。
struct Unsettled {
    error: CoreError,
    /// 失敗するまでに使ったソルバーの試行回数（解く前に拒否したなら 0）。
    trials: usize,
}

impl From<CoreError> for Unsettled {
    /// 解かずに拒否した場合。
    fn from(error: CoreError) -> Self {
        Self { error, trials: 0 }
    }
}

/// 図形のパラメータ（`equations` の並び。線分 4・円 3・円弧 5）のうち、`before` から `after`
/// へ値が変わったもの（設計判断3 の固定集合 (c)。使わない枠は `false`）。
///
/// **f64 の完全一致**で比べ、**点の単位**で止める: 線分は始点・終点、円と円弧は中心を
/// (x, y) の組として、どちらかの座標が違えば両方を止める（水平に動かした点の y は bit 単位で
/// 同じだが、利用者が置いた点であることに変わりはない）。半径・開始角・終了角は個別。
/// 許容値では比べない: グリップで片方の端点だけを動かしたとき、もう片方が bit 単位で同じで
/// あること（= 自由なまま）が、長さ拘束に従って反対側が動く前提になる。
///
/// 種類が変わった（円 → 円弧など）、またはどちらかが拘束の対象にできない図形なら、
/// すべて止める。
fn moved_params(before: &EntityGeom, after: &EntityGeom) -> [bool; 5] {
    match (before.as_shape(), after.as_shape()) {
        (Some(Shape::Line(p)), Some(Shape::Line(q))) => {
            let (a, b) = (p.a != q.a, p.b != q.b);
            [a, a, b, b, false]
        }
        (Some(Shape::Circle(p)), Some(Shape::Circle(q))) => {
            let center = p.center != q.center;
            [center, center, p.radius != q.radius, false, false]
        }
        (Some(Shape::Arc(p)), Some(Shape::Arc(q))) => {
            let center = p.center != q.center;
            [
                center,
                center,
                p.radius != q.radius,
                p.start_angle != q.start_angle,
                p.end_angle != q.end_angle,
            ]
        }
        _ => [true; 5],
    }
}

/// 1 つの成分を、固定集合 `fixed` のもとで満たす（モジュール doc「1 つの成分の解き方」）。
fn settle(sys: &System, fixed: &[bool]) -> Result<Settled, Unsettled> {
    // 1. 拘束が参照する図形が、解く前から退化している（`solve` は開始時の形に
    //    `admissible` を呼ばないので、ここで確かめる）。
    if let Some(slot) = sys.slots.iter().find(|slot| !slot.admissible(&sys.params)) {
        return Err(CoreError::DegenerateShape(slot.entity).into());
    }
    // 2. すべて満たされていれば解かない（何も動かさない）。
    let Some(first) = sys.first_unsatisfied() else {
        return match sys.first_structure_violation(&sys.params) {
            Some(id) => Err(CoreError::DegenerateConstraint(id).into()),
            None => Ok(Settled {
                params: None,
                trials: 0,
            }),
        };
    };
    // 3. 止められたパラメータだけに関わる拘束が満たされていない。
    if let Some(id) = sys.first_unsatisfied_held(fixed) {
        return Err(CoreError::ConstraintBlockedByFixed(id).into());
    }
    // 4. 解く。
    let weights = sys.weights();
    // 退化で棄却した最初の図形（解き直しをまたいで保つ）と、試行点の数。失敗した求解は
    // 試行回数を返さないので、フックが呼ばれた回数で数える（ガウス消去に失敗した試行と
    // 非有限の試行点はフックへ来ないので入らない）。
    let rejected: Cell<Option<EntityId>> = Cell::new(None);
    let trial_points: Cell<usize> = Cell::new(0);
    let admissible = |x: &[f64]| {
        trial_points.set(trial_points.get() + 1);
        match sys.first_inadmissible(x) {
            None => true,
            Some(entity) => {
                if rejected.get().is_none() {
                    rejected.set(Some(entity));
                }
                false
            }
        }
    };
    let problem =
        Problem::new(&sys.params, fixed, &weights, &sys.equations).with_admissible(&admissible);
    // 既定の初期減衰で停滞・試行回数の上限に達したときだけ、同じ入力を大きい初期減衰で
    // 1 回だけ解き直す。ほかの失敗（自由なパラメータが無い・非有限）は初期減衰に依らない。
    let mut wasted = 0;
    let solution = solve::solve(&problem).or_else(|err| match err {
        SolveError::Stalled | SolveError::TrialLimit => {
            wasted = trial_points.get();
            solve::solve(&problem.with_initial_damping(solve::LAMBDA_RETRY))
        }
        other => Err(other),
    });
    let solution = solution.map_err(|err| Unsettled {
        error: match err {
            // 3 で拘束単位に確かめているので、ここへは来ないはず（来ても同じ意味）。
            SolveError::NoFreeParameters => CoreError::ConstraintBlockedByFixed(first),
            SolveError::Stalled | SolveError::TrialLimit => match rejected.get() {
                Some(entity) => CoreError::DegenerateShape(entity),
                None => CoreError::ConstraintUnsolvable(first),
            },
            // 座標が極端に大きく、残差・ヤコビアン・許容値が有限にならない。
            SolveError::NonFinite | SolveError::InvalidTolerance { .. } => {
                CoreError::ConstraintUnsolvable(first)
            }
            SolveError::InvalidInput => {
                debug_assert!(false, "the constraint system is malformed");
                CoreError::ConstraintUnsolvable(first)
            }
        },
        trials: trial_points.get(),
    })?;
    // 5. 解いた形を確かめる。
    let trials = wasted + solution.trials;
    let unsettled = |error: CoreError| Unsettled { error, trials };
    let x = solution.params;
    if let Some(entity) = sys.first_inadmissible(&x).or_else(|| sys.collapsed(&x)) {
        return Err(unsettled(CoreError::DegenerateShape(entity)));
    }
    if let Some(id) = sys.first_structure_violation(&x) {
        return Err(unsettled(CoreError::DegenerateConstraint(id)));
    }
    if let Some(id) = sys.first_unsatisfied_at(&x) {
        return Err(unsettled(CoreError::ConstraintUnsolvable(id)));
    }
    Ok(Settled {
        params: Some(x),
        trials,
    })
}
