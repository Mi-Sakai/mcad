//! [`Document`] の幾何拘束まわり（M13 タスク96、DESIGN.md M13 設計判断2〜4）。
//!
//! 読み取り API、拘束スロットの書き換え（墓標方式）、エンティティ削除・切り詰めに伴う
//! 拘束の削除、コマンド適用後の充足検査を置く。`document.rs` の `execute` / `revert` /
//! `reapply` から呼ばれる。
//!
//! # 拘束の無い文書
//!
//! 生存している拘束の数を `live_constraints` に持ち、0 のときは走査も方程式の組み立ても
//! せずに戻る（拘束を使わない利用者 — tcad を含む — の動作と性能を変えない）。

use std::collections::BTreeSet;

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

    // ---- 充足検査 ----

    /// コマンド適用後の充足検査（[`Document::apply`] が最も外側で 1 回だけ呼ぶ）。
    ///
    /// `applied` から (a) 幾何を差し替えたエンティティ (b) 追加・値変更した拘束 を集め、
    /// それに関わる生存中の拘束が今の形で満たされているかを調べる。満たされていない拘束が
    /// あれば、[`ConstraintId`] 順で最初のものを [`CoreError::ConstraintUnsatisfied`] で返す
    /// （状態は変えない。巻き戻すのは呼び出し側）。
    ///
    /// 関わらない拘束は調べない: 拘束の残差と許容値は参照するエンティティの形と拘束の値だけで
    /// 決まるので、どちらも変わっていなければ判定は変わらない。同じ `Batch` の中で追加して
    /// から消えた拘束（墓標）は対象にならない。
    pub(super) fn check_constraints(&self, applied: &Applied) -> Result<(), CoreError> {
        if self.live_constraints == 0 {
            return Ok(());
        }
        let mut touched = Touched::default();
        touched.collect(applied);
        if touched.entities.is_empty() && touched.constraints.is_empty() {
            return Ok(());
        }
        let affected = self.constraints().filter(|(id, c)| {
            touched.constraints.contains(id)
                || c.targets().any(|(e, _)| touched.entities.contains(&e))
        });
        let sys = System::build(|id| self.entity(id).map(|e| &e.geom), affected)?;
        match sys.first_unsatisfied() {
            Some(id) => Err(CoreError::ConstraintUnsatisfied(id)),
            None => Ok(()),
        }
    }
}

/// 1 回の `apply` が触れたもの（充足検査の対象を絞るため）。
#[derive(Default)]
struct Touched {
    /// 幾何を差し替えたエンティティ。
    entities: BTreeSet<EntityId>,
    /// 追加・値変更した拘束。
    constraints: BTreeSet<ConstraintId>,
}

impl Touched {
    fn collect(&mut self, applied: &Applied) {
        match applied {
            // 同一幾何への差し替え（no-op のサブコマンド）も入るが、その拘束は元から
            // 満たされているので結果は変わらない（幾何の比較を省く）。
            Applied::ModifyEntity { id, .. } => {
                self.entities.insert(*id);
            }
            Applied::AddConstraint { id, .. } | Applied::SetConstraintValue { id, .. } => {
                self.constraints.insert(*id);
            }
            Applied::Batch(subs) => {
                for sub in subs {
                    self.collect(sub);
                }
            }
            // 削除はそのエンティティ・拘束に関わる拘束を残さない。追加したエンティティに
            // 拘束が付くのは AddConstraint 経由だけ。スタイル・レイヤー・図面設定は形を変えない。
            Applied::AddEntity { .. }
            | Applied::RemoveEntity { .. }
            | Applied::SetEntityStyle { .. }
            | Applied::SetEntityLayer { .. }
            | Applied::AddLayer { .. }
            | Applied::RemoveLayer { .. }
            | Applied::SetLayerProps { .. }
            | Applied::SetCurrentLayer { .. }
            | Applied::SetSheet { .. }
            | Applied::SetDimStyle { .. }
            | Applied::RemoveConstraint { .. } => {}
        }
    }
}
