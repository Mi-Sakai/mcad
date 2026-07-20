//! [`Document`]: CAD ドキュメントモデルとコマンドパターンによる undo/redo。
//!
//! # スロット墓標方式による ID 安定性
//!
//! エンティティ・レイヤーは `SlotMap<K, Option<T>>` に格納する。「削除」は
//! `slotmap` からスロットを取り除く（`SlotMap::remove`）のではなく、値を `None` に
//! する（= 墓標化する）ことで表現し、スロット自体は保持し続ける。
//!
//! こうする理由は **キーの安定性** にある。`slotmap` はスロットを解放して再挿入すると
//! 世代が繰り上がった別キーを返すため、素朴に remove/insert で undo/redo を実装すると、
//! 依存関係のあるコマンド（例: `Add A` → `Modify A` を undo→undo→redo→redo）や、
//! アプリ側が保持する選択集合の `EntityId` が壊れてしまう。墓標方式なら一度発行した
//! キーは生存/墓標を行き来しても不変なので、undo/redo は各スロットの `Option` を
//! 差し替えるだけで済み、キーの張り替え処理が一切不要になる。
//!
//! 代償として、削除済みエンティティのスロットはセッション中回収されない
//! （undo 履歴がそれらを参照しうるため妥当）。数千エンティティ規模の MVP では問題ない。

use slotmap::SlotMap;

use crate::{Command, CoreError, Entity, EntityGeom, EntityId, Layer, LayerId, Rgb};

/// 実行済みコマンドの逆操作可能な記録（内部専用）。
///
/// 公開 [`Command`] が「意図」を表すのに対し、こちらは実際に起きた変更を
/// 逆転できる形（採番されたキーや変更前の値）で保持する。キーは墓標方式により
/// 安定なので、undo/redo をまたいでも張り替え不要。
#[derive(Debug, Clone)]
enum Applied {
    AddEntity {
        id: EntityId,
        entity: Entity,
    },
    RemoveEntity {
        id: EntityId,
        entity: Entity,
    },
    ModifyEntity {
        id: EntityId,
        before: EntityGeom,
        after: EntityGeom,
    },
    AddLayer {
        id: LayerId,
        layer: Layer,
    },
    RemoveLayer {
        id: LayerId,
        layer: Layer,
    },
    SetLayerProps {
        id: LayerId,
        before: Layer,
        after: Layer,
    },
    SetCurrentLayer {
        before: LayerId,
        after: LayerId,
    },
    /// 複合コマンドの逆操作記録。サブコマンドの [`Applied`] を **適用順** に保持する。
    /// undo は逆順・redo は正順に適用することで、バッチ全体が 1 単位で戻る/やり直せる。
    Batch(Vec<Applied>),
}

/// undo/redo スタックの 1 エントリ。逆操作記録に加え、そのコマンドを適用した直後の
/// ドキュメント世代番号（[`Document::generation`]）を持つ。
///
/// 世代番号は「未保存の変更（dirty）判定」のために保持する。undo は 1 つ前の履歴時点の
/// 世代へ、redo はこのエントリが記録した世代へ [`Document::generation`] を戻す。これにより
/// 「保存 → 変更 → undo で保存時点へ戻ったら再び未保存でない」といった精密な判定が、
/// スナップショット比較なしに O(1) で行える。
#[derive(Debug, Clone)]
struct HistoryEntry {
    /// 逆操作記録（undo/redo 用）。
    applied: Applied,
    /// このコマンドを適用した直後のドキュメント世代番号。
    generation: u64,
}

/// [`Document::apply`] が新規発行した ID の一覧。
///
/// `AddEntity`/`AddLayer`（単体、および [`Command::Batch`] にネストされたもの）が
/// 発行した新規キーを、呼び出し側が「集合差分を取る」ような自前のハックなしに
/// 直接受け取れるようにするための型。含まれる ID は **適用順**。
/// それ以外のコマンド（`ModifyEntity` など）は空の `NewIds` を返す。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewIds {
    /// 新規発行されたエンティティ ID（適用順）。
    pub entities: Vec<EntityId>,
    /// 新規発行されたレイヤー ID（適用順）。
    pub layers: Vec<LayerId>,
}

/// [`Document::execute`] の実行結果（内部専用）。
///
/// 逆操作記録・新規発行 ID・実質的な状態変化の有無を 1 つにまとめて返す。
///
/// `new_ids` を `&mut` の出力引数として関数外から渡す設計を避け、戻り値へ集約している。
/// こうすることで `execute` が `Err` を返した経路では、途中まで積んだ `new_ids` も
/// ローカル変数として自動的に破棄され、「エラー時に墓標化済み ID を含む `new_ids` を
/// 誤って外部で使ってしまう」構造的リスクが型システムのレベルで消える。
///
/// `changed` は「このコマンドが実際に状態を変えたか」を [`Applied`] の構築と同時に
/// O(1) で判定した結果で、意味的 no-op（空バッチ・`before == after` の設定など）では
/// `false` になる。[`Document::apply`] はこれを見て、実体のない記録を undo 履歴へ
/// 積まないようにする（事後に `Applied` ツリーを再走査する必要はない）。
struct ExecuteOutcome {
    /// 逆操作記録（undo/redo 用）。
    applied: Applied,
    /// このコマンドが新規発行した ID（適用順）。
    new_ids: NewIds,
    /// 実質的に状態を変えたか。`false` は意味的 no-op。
    changed: bool,
}

impl ExecuteOutcome {
    /// 新規 ID を発行せず、必ず状態を変える操作（`RemoveEntity`/`RemoveLayer` など、
    /// 追加・削除のように意味的 no-op がありえないもの）の結果を作る。
    fn mutation(applied: Applied) -> Self {
        Self {
            applied,
            new_ids: NewIds::default(),
            changed: true,
        }
    }

    /// 新規 ID を発行せず、`before == after` のとき意味的 no-op になりうる操作
    /// （`ModifyEntity`/`SetLayerProps`/`SetCurrentLayer`）の結果を作る。
    fn conditional(applied: Applied, changed: bool) -> Self {
        Self {
            applied,
            new_ids: NewIds::default(),
            changed,
        }
    }
}

/// CAD ドキュメント。エンティティ・レイヤー・カレントレイヤーと undo/redo 履歴を保持する。
///
/// 内部フィールドはすべて非公開で、状態変更は [`Document::apply`] /
/// [`Document::undo`] / [`Document::redo`] のみを通じて行う。読み取りは各 getter・
/// イテレータで公開する。
pub struct Document {
    /// エンティティ格納庫。`None` は墓標（削除済みスロット）。
    entities: SlotMap<EntityId, Option<Entity>>,
    /// レイヤー格納庫。`None` は墓標（削除済みスロット）。
    layers: SlotMap<LayerId, Option<Layer>>,
    /// カレントレイヤー（新規エンティティの既定所属先）。常に生存レイヤーを指す。
    current_layer: LayerId,
    /// デフォルトレイヤー（レイヤー0相当）。削除不可。
    default_layer: LayerId,
    /// undo スタック（末尾が直近の操作）。
    undo_stack: Vec<HistoryEntry>,
    /// redo スタック（末尾が次に redo する操作）。
    redo_stack: Vec<HistoryEntry>,
    /// 現在のドキュメント内容を表す世代番号。実際に状態を変えたコマンドの適用ごとに
    /// [`Document::next_generation`] から採番した新しい番号へ更新し、undo/redo は対応する
    /// 履歴時点の世代へ戻す。no-op は変えない。保存時点の世代との比較で dirty を判定する。
    generation: u64,
    /// 次に新規コマンドを適用したときに採番する世代番号（単調増加の高水位）。
    ///
    /// undo で [`Document::generation`] を戻しても、この値は減らさない。こうすることで、
    /// undo 後に別の新規コマンドを適用しても、過去に使った世代番号を再利用せず、
    /// 「内容が違えば世代も必ず違う」不変条件を保つ（dirty の誤判定を防ぐ）。
    next_generation: u64,
}

impl Document {
    /// 新規ドキュメントを作る。
    ///
    /// デフォルトレイヤー（名前 `"0"`）を 1 つ自動生成し、カレントレイヤーに設定する。
    #[must_use]
    pub fn new() -> Self {
        let mut layers: SlotMap<LayerId, Option<Layer>> = SlotMap::with_key();
        let default_layer = layers.insert(Some(Layer::new("0", Rgb::WHITE)));
        Self {
            entities: SlotMap::with_key(),
            layers,
            current_layer: default_layer,
            default_layer,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            generation: 0,
            next_generation: 0,
        }
    }

    // ---- 読み取り系 ----

    /// カレントレイヤーの ID。
    #[must_use]
    pub fn current_layer(&self) -> LayerId {
        self.current_layer
    }

    /// デフォルトレイヤー（削除不可）の ID。
    #[must_use]
    pub fn default_layer(&self) -> LayerId {
        self.default_layer
    }

    /// 指定 ID のエンティティを参照する。存在しない/削除済みなら `None`。
    #[must_use]
    pub fn entity(&self, id: EntityId) -> Option<&Entity> {
        self.entities.get(id).and_then(Option::as_ref)
    }

    /// 指定 ID のレイヤーを参照する。存在しない/削除済みなら `None`。
    #[must_use]
    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.layers.get(id).and_then(Option::as_ref)
    }

    /// 生存しているエンティティを `(ID, &Entity)` で列挙する（順序は未規定）。
    pub fn entities(&self) -> impl Iterator<Item = (EntityId, &Entity)> {
        self.entities
            .iter()
            .filter_map(|(k, v)| v.as_ref().map(|e| (k, e)))
    }

    /// 生存しているレイヤーを `(ID, &Layer)` で列挙する（順序は未規定）。
    pub fn layers(&self) -> impl Iterator<Item = (LayerId, &Layer)> {
        self.layers
            .iter()
            .filter_map(|(k, v)| v.as_ref().map(|l| (k, l)))
    }

    /// 生存エンティティ数。
    #[must_use]
    pub fn entity_count(&self) -> usize {
        self.entities().count()
    }

    /// 生存レイヤー数。
    #[must_use]
    pub fn layer_count(&self) -> usize {
        self.layers().count()
    }

    /// undo できる操作があるか。
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    /// redo できる操作があるか。
    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// 現在のドキュメント内容を表す世代番号。
    ///
    /// 実際に状態を変えたコマンドの適用ごとに単調増加した新しい番号が割り当てられ、
    /// undo/redo は対応する履歴時点の世代へ戻す。意味的 no-op（空バッチ・`before == after`
    /// の設定など）は世代を変えない。アプリ側は保存成功時の世代を記録し、現在の世代と
    /// 比較することで「未保存の変更（dirty）」を正確に判定する（保存 → 変更 → undo で
    /// 保存時点へ厳密に戻ったら再び未保存でない、まで表現できる）。
    ///
    /// [`Document::clear_history`] は世代を初期値へリセットするため、ファイル読込・新規
    /// 作成の直後は必ず新しい基準点（`0`）になる。
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// undo/redo 履歴を空にする。ドキュメントの内容自体は変更しない。
    ///
    /// ファイル読込直後など「ここより前へは戻れない」基準点を作るために使う。
    /// mcad-io の import は `Command` 列でドキュメントを再構築するため、これを
    /// 呼ばないと読込直後の Ctrl+Z で再構築手順が 1 つずつ巻き戻ってしまう。
    ///
    /// 履歴を消すのと同時に、世代番号（[`Document::generation`] /
    /// [`Document::next_generation`]）も初期状態へリセットする。これにより読込・新規作成の
    /// 直後は世代が新しい基準点（`0`）になり、アプリ側が保存済み世代をここへ合わせれば
    /// 「読込直後は未保存の変更なし」が保たれる。
    pub fn clear_history(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.generation = 0;
        self.next_generation = 0;
    }

    // ---- 変更系 ----

    /// コマンドを適用する。
    ///
    /// 成功時は逆操作を undo 履歴へ積み、redo スタックをクリアする。
    /// 失敗時は [`CoreError`] を返し、**ドキュメントの状態は変更しない**。
    ///
    /// 戻り値の [`NewIds`] には、このコマンド（`Batch` の場合はネストされた
    /// サブコマンドを含む）が新規発行した `EntityId`/`LayerId` が適用順で入る。
    /// `AddEntity`/`AddLayer` を含まないコマンドでは空になる。
    ///
    /// # Errors
    ///
    /// 存在しない ID への操作、デフォルト/カレント/非空レイヤーの削除、
    /// ロックされたレイヤー上のエンティティへの変更・削除などで [`CoreError`] を返す。
    pub fn apply(&mut self, cmd: Command) -> Result<NewIds, CoreError> {
        // 原子性（検証→変更、失敗時のロールバック）の保証内容は [`Command::Batch`] の
        // doc（「# 原子性」節）を参照。no-op 判定の仕組みは [`ExecuteOutcome`] の doc を参照。
        let ExecuteOutcome {
            applied,
            new_ids,
            changed,
        } = self.execute(cmd)?;
        // 実質 no-op（changed == false）は状態も履歴も一切変えない。undo できる実体が
        // ない記録を undo スタックへ積むと、Ctrl+Z 1 回が見かけ上「何も戻らない」空振り
        // になりユーザー体験を損なうため、undo/redo スタックのどちらにも触れない
        // （redo は保持したままにする）。
        if !changed {
            return Ok(NewIds::default());
        }
        // 実変更があったので新しい世代を採番する。`next_generation` は単調増加の高水位で、
        // undo で世代を戻しても減らさないため、過去に使った番号を再利用しない（内容が違えば
        // 世代も必ず違う、という dirty 判定の前提を保つ）。採番した世代を履歴エントリに
        // 記録し、redo 時に同じ世代へ復元できるようにする。
        self.next_generation += 1;
        self.generation = self.next_generation;
        self.undo_stack.push(HistoryEntry {
            applied,
            generation: self.generation,
        });
        self.redo_stack.clear();
        Ok(new_ids)
    }

    /// 直近の操作を取り消す。取り消せた場合 `true`。
    pub fn undo(&mut self) -> bool {
        match self.undo_stack.pop() {
            Some(entry) => {
                self.revert(&entry.applied);
                // 世代は「undo 後に到達する内容」の世代へ戻す。それは undo スタックに残った
                // 直前エントリが記録した世代であり、履歴が空になる（最古の状態へ戻る）場合は
                // 初期世代 `0`。これにより、保存時点の内容へ厳密に戻れば世代も保存時と一致する。
                self.generation = self.undo_stack.last().map_or(0, |e| e.generation);
                self.redo_stack.push(entry);
                true
            }
            None => false,
        }
    }

    /// 直近に取り消した操作をやり直す。やり直せた場合 `true`。
    pub fn redo(&mut self) -> bool {
        match self.redo_stack.pop() {
            Some(entry) => {
                self.reapply(&entry.applied);
                // 同じ操作をやり直すだけなので新規採番はせず、元の適用時に記録した世代を
                // そのまま復元する（`next_generation` は増やさない）。
                self.generation = entry.generation;
                self.undo_stack.push(entry);
                true
            }
            None => false,
        }
    }

    // ---- 内部ヘルパ ----

    /// コマンドを検証・実行し、逆操作記録・新規発行 ID・状態変化の有無を
    /// [`ExecuteOutcome`] にまとめて返す。検証失敗時は状態不変。
    ///
    /// 新規発行した `EntityId`/`LayerId` と「実際に状態を変えたか」（`changed`）は、
    /// 変更を起こすのと同時に O(1) で組み立てる（`Batch` はサブの結果を再帰的に集約する）。
    /// 戻り値へ集約しているため、`Err` を返した経路では途中まで積んだ `new_ids` も
    /// 呼び出し元のローカル変数として自動的に破棄される（[`ExecuteOutcome`] の doc 参照）。
    fn execute(&mut self, cmd: Command) -> Result<ExecuteOutcome, CoreError> {
        match cmd {
            Command::AddEntity(entity) => {
                // NaN/∞座標・負半径などの不正ジオメトリはそもそもドキュメントへ
                // 入れない（DESIGN.md M4 タスク15）。
                entity.geom.validate().map_err(CoreError::InvalidGeometry)?;
                // 宙に浮いた layer 参照を防ぐため所属レイヤーの存在を検証し、
                // ロック済みレイヤーへの追加も Modify/Remove と対称に拒否する。
                self.require_editable_layer(entity.layer)?;
                let id = self.entities.insert(Some(entity.clone()));
                // 追加は常に状態を変えるので changed = true。
                Ok(ExecuteOutcome {
                    applied: Applied::AddEntity { id, entity },
                    new_ids: NewIds {
                        entities: vec![id],
                        layers: Vec::new(),
                    },
                    changed: true,
                })
            }
            Command::RemoveEntity(id) => {
                self.require_editable_entity(id)?;
                let entity = self.take_entity(id);
                Ok(ExecuteOutcome::mutation(Applied::RemoveEntity {
                    id,
                    entity,
                }))
            }
            Command::ModifyEntity { id, new_geom } => {
                // NaN/∞座標・負半径などの不正ジオメトリへの差し替えは拒否する
                // （DESIGN.md M4 タスク15）。
                new_geom.validate().map_err(CoreError::InvalidGeometry)?;
                self.require_editable_entity(id)?;
                let slot = self.live_entity_mut(id);
                let before = std::mem::replace(&mut slot.geom, new_geom.clone());
                // 同一幾何への差し替えは意味的 no-op（履歴を汚さない）。
                let changed = before != new_geom;
                Ok(ExecuteOutcome::conditional(
                    Applied::ModifyEntity {
                        id,
                        before,
                        after: new_geom,
                    },
                    changed,
                ))
            }
            Command::AddLayer(layer) => {
                let id = self.layers.insert(Some(layer.clone()));
                // 追加は常に状態を変えるので changed = true。
                Ok(ExecuteOutcome {
                    applied: Applied::AddLayer { id, layer },
                    new_ids: NewIds {
                        entities: Vec::new(),
                        layers: vec![id],
                    },
                    changed: true,
                })
            }
            Command::RemoveLayer(id) => {
                self.require_layer(id)?;
                if id == self.default_layer {
                    return Err(CoreError::CannotDeleteDefaultLayer);
                }
                if id == self.current_layer {
                    return Err(CoreError::CannotDeleteCurrentLayer);
                }
                if self.entities().any(|(_, e)| e.layer == id) {
                    return Err(CoreError::LayerNotEmpty(id));
                }
                let layer = self.take_layer(id);
                Ok(ExecuteOutcome::mutation(Applied::RemoveLayer { id, layer }))
            }
            Command::SetLayerProps { id, props } => {
                self.require_layer(id)?;
                let before = self
                    .layers
                    .get_mut(id)
                    .expect("layer key checked by require_layer")
                    .replace(props.clone())
                    .expect("layer is live (checked by require_layer)");
                // 同一プロパティの設定は意味的 no-op（履歴を汚さない）。
                let changed = before != props;
                Ok(ExecuteOutcome::conditional(
                    Applied::SetLayerProps {
                        id,
                        before,
                        after: props,
                    },
                    changed,
                ))
            }
            Command::SetCurrentLayer(id) => {
                self.require_layer(id)?;
                let before = self.current_layer;
                self.current_layer = id;
                // 現在と同じレイヤーの指定は意味的 no-op（履歴を汚さない）。
                Ok(ExecuteOutcome::conditional(
                    Applied::SetCurrentLayer { before, after: id },
                    before != id,
                ))
            }
            Command::Batch(subs) => {
                let mut applied: Vec<Applied> = Vec::with_capacity(subs.len());
                let mut new_ids = NewIds::default();
                // バッチは 1 つでも実変更を含めば changed。空バッチ・全 no-op サブの
                // バッチ（ネストした空バッチ含む）は changed = false のままとなり、
                // apply が undo 履歴へ積まない。
                let mut changed = false;
                for sub in subs {
                    match self.execute(sub) {
                        Ok(outcome) => {
                            applied.push(outcome.applied);
                            new_ids.entities.extend(outcome.new_ids.entities);
                            new_ids.layers.extend(outcome.new_ids.layers);
                            changed |= outcome.changed;
                        }
                        Err(err) => {
                            // 原子性: これまでに適用したサブコマンドを逆順で巻き戻し、
                            // 呼び出し前の状態へ完全復帰させてからエラーを返す。
                            // revert は undo と同じ経路なので部分適用は残らない。
                            for done in applied.iter().rev() {
                                self.revert(done);
                            }
                            return Err(err);
                        }
                    }
                }
                Ok(ExecuteOutcome {
                    applied: Applied::Batch(applied),
                    new_ids,
                    changed,
                })
            }
        }
    }

    /// [`Applied`] を逆転（undo 方向）に適用する。履歴は一貫しているものとして扱う。
    fn revert(&mut self, applied: &Applied) {
        match applied {
            Applied::AddEntity { id, .. } => self.set_entity(*id, None),
            Applied::RemoveEntity { id, entity } => self.set_entity(*id, Some(entity.clone())),
            Applied::ModifyEntity { id, before, .. } => self.set_entity_geom(*id, before.clone()),
            Applied::AddLayer { id, .. } => self.set_layer(*id, None),
            Applied::RemoveLayer { id, layer } => self.set_layer(*id, Some(layer.clone())),
            Applied::SetLayerProps { id, before, .. } => self.set_layer(*id, Some(before.clone())),
            Applied::SetCurrentLayer { before, .. } => self.current_layer = *before,
            // バッチは逆順に各サブ逆操作を適用する（依存関係を正しく巻き戻すため）。
            Applied::Batch(applied) => {
                for a in applied.iter().rev() {
                    self.revert(a);
                }
            }
        }
    }

    /// [`Applied`] を順方向（redo 方向）に再適用する。履歴は一貫しているものとして扱う。
    fn reapply(&mut self, applied: &Applied) {
        match applied {
            Applied::AddEntity { id, entity } => self.set_entity(*id, Some(entity.clone())),
            Applied::RemoveEntity { id, .. } => self.set_entity(*id, None),
            Applied::ModifyEntity { id, after, .. } => self.set_entity_geom(*id, after.clone()),
            Applied::AddLayer { id, layer } => self.set_layer(*id, Some(layer.clone())),
            Applied::RemoveLayer { id, .. } => self.set_layer(*id, None),
            Applied::SetLayerProps { id, after, .. } => self.set_layer(*id, Some(after.clone())),
            Applied::SetCurrentLayer { after, .. } => self.current_layer = *after,
            // バッチは正順に各サブ操作を再適用する（execute と同じ順序）。
            Applied::Batch(applied) => {
                for a in applied.iter() {
                    self.reapply(a);
                }
            }
        }
    }

    fn require_layer(&self, id: LayerId) -> Result<(), CoreError> {
        if self.layer(id).is_some() {
            Ok(())
        } else {
            Err(CoreError::LayerNotFound(id))
        }
    }

    /// レイヤーがロックされていれば [`CoreError::LayerLocked`] を返す判定ロジック本体。
    ///
    /// `execute` 経由で新規コマンドを適用する経路（`AddEntity`（[`Document::require_editable_layer`]
    /// 経由）・`RemoveEntity`・`ModifyEntity`（いずれも [`Document::require_editable_entity`]
    /// 経由）を介した [`Document::ensure_layer_unlocked`]）がこの規則を通す。ロック判定
    /// ロジックをここへ一元化することで、レイヤーごとのロック挙動の食い違いや、将来の
    /// コマンド追加時のチェック漏れのリスクを下げる（ただし各コマンドアームが手動で
    /// このヘルパー経由で呼ぶ規約に依っており、型やマクロによる強制はない）。
    ///
    /// `revert`/`reapply`（undo/redo 経路）はこのチェックを一切通らない。ロック中の
    /// レイヤーに対しても undo/redo は常に成功させる、という意図した設計判断である。
    ///
    /// `layer` が `None`（対象レイヤーが存在しない）の場合はここでは咎めず `Ok(())` と
    /// する。存在確認を行うかどうかは呼び出し側の責務であり、
    /// [`Document::ensure_layer_unlocked`] と [`Document::require_editable_layer`] とで
    /// 扱いが異なる。
    fn check_layer_lock(id: LayerId, layer: Option<&Layer>) -> Result<(), CoreError> {
        match layer {
            Some(layer) if layer.locked => Err(CoreError::LayerLocked(id)),
            _ => Ok(()),
        }
    }

    /// 指定レイヤーがロックされていれば [`CoreError::LayerLocked`] を返す。
    ///
    /// 対象レイヤーが（不変条件違反で）存在しない場合はここでは咎めず `Ok(())` とする。
    /// ロック判定ロジック自体は [`Document::check_layer_lock`] に一元化されている。
    fn ensure_layer_unlocked(&self, id: LayerId) -> Result<(), CoreError> {
        Self::check_layer_lock(id, self.layer(id))
    }

    /// レイヤーが存在し、かつロックされていないことを一度のルックアップで検証する。
    ///
    /// エンティティ側の [`Document::require_editable_entity`] と対になるヘルパー。
    /// 存在確認とロック確認を別々のヘルパー呼び出しで行うと同じレイヤー ID を 2 回
    /// ルックアップすることになるため、1 回にまとめている。`AddEntity` がこれを使う。
    ///
    /// ロック判定ロジック自体は [`Document::check_layer_lock`] に一元化されている。
    /// 適用範囲（どの経路がこのチェックを通るか）は [`Document::check_layer_lock`] を
    /// 参照。
    fn require_editable_layer(&self, id: LayerId) -> Result<(), CoreError> {
        let layer = self.layer(id).ok_or(CoreError::LayerNotFound(id))?;
        Self::check_layer_lock(id, Some(layer))
    }

    /// エンティティが生存し、かつ所属レイヤーがロックされていないことを一度に検証する。
    ///
    /// 「存在確認」と「ロック確認」を 1 つのヘルパーにまとめている。ただし呼び出し元
    /// （`RemoveEntity`/`ModifyEntity`）はこの後 `take_entity`/`live_entity_mut` で
    /// 改めて対象スロットを参照するため、エンティティスロットへの参照そのものは合計
    /// 2 回になる（抑えているのは検証ロジックの重複であり、参照回数そのものではない）。
    ///
    /// ロック判定の適用範囲は [`Document::ensure_layer_unlocked`] を参照。
    fn require_editable_entity(&self, id: EntityId) -> Result<(), CoreError> {
        let layer_id = self.entity(id).ok_or(CoreError::EntityNotFound(id))?.layer;
        self.ensure_layer_unlocked(layer_id)
    }

    /// 生存エンティティを取り出してスロットを墓標化する。生存を前提とする。
    fn take_entity(&mut self, id: EntityId) -> Entity {
        self.entities
            .get_mut(id)
            .expect("entity key must exist")
            .take()
            .expect("entity must be live")
    }

    /// 生存レイヤーを取り出してスロットを墓標化する。生存を前提とする。
    fn take_layer(&mut self, id: LayerId) -> Layer {
        self.layers
            .get_mut(id)
            .expect("layer key must exist")
            .take()
            .expect("layer must be live")
    }

    /// 生存エンティティへの可変参照。生存を前提とする。
    fn live_entity_mut(&mut self, id: EntityId) -> &mut Entity {
        self.entities
            .get_mut(id)
            .expect("entity key must exist")
            .as_mut()
            .expect("entity must be live")
    }

    /// スロット（キーは有効な前提）の値を差し替える。
    fn set_entity(&mut self, id: EntityId, value: Option<Entity>) {
        *self
            .entities
            .get_mut(id)
            .expect("entity slot must exist during undo/redo") = value;
    }

    /// スロット（キーは有効な前提）の値を差し替える。
    fn set_layer(&mut self, id: LayerId, value: Option<Layer>) {
        *self
            .layers
            .get_mut(id)
            .expect("layer slot must exist during undo/redo") = value;
    }

    /// 生存エンティティの幾何のみ差し替える。生存を前提とする。
    fn set_entity_geom(&mut self, id: EntityId, geom: EntityGeom) {
        self.live_entity_mut(id).geom = geom;
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Style;
    use mcad_geom::{LineSeg, Point2, Shape};

    fn line(x: f64) -> EntityGeom {
        EntityGeom::Shape(Shape::Line(LineSeg::new(
            Point2::new(x, 0.0),
            Point2::new(x, 1.0),
        )))
    }

    /// カレントレイヤー上に、指定幾何のエンティティを追加するコマンドを作る。
    fn add_line(doc: &Document, geom: EntityGeom) -> Command {
        Command::AddEntity(Entity::new(geom, doc.current_layer(), Style::inherited()))
    }

    /// 直近に追加された（唯一の）エンティティの ID を取り出す。
    ///
    /// これは「新規発行 ID を知るには前後の集合差分を取るしかない」場合の
    /// 汎用ヘルパではなく、undo/redo 後に「同一キーが復元されているか」を検証する
    /// 用途に限定して使う（新規追加直後の ID 取得には [`add_entity_get_id`] を使う）。
    fn only_entity(doc: &Document) -> EntityId {
        let mut it = doc.entities();
        let (id, _) = it.next().expect("expected exactly one entity");
        assert!(it.next().is_none(), "expected exactly one entity");
        id
    }

    /// [`NewIds`] の一方のフィールド（`one`）にちょうど 1 件だけ入っており、もう一方
    /// （`other`）が空であることを検証したうえで、その 1 件を返す。`add_entity_get_id`/
    /// `add_layer_get_id` はどちらも「新規追加コマンドを 1 件適用し、`NewIds` から
    /// 新規発行された ID を 1 つだけ取り出す」という同じ形なので、この共通処理へまとめる。
    fn assert_single_new_id<T: Copy, U>(one: &[T], other: &[U]) -> T {
        assert_eq!(one.len(), 1, "expected exactly one new id");
        assert!(
            other.is_empty(),
            "expected the other NewIds field to stay empty"
        );
        one[0]
    }

    /// [`Command::AddEntity`] を適用し、`apply` の戻り値 [`NewIds`] から新規発行された
    /// エンティティ ID を直接取り出す。以前は適用前後の集合差分を取るハックで代用していたが、
    /// `apply` が `NewIds` を返すようになったため不要になった。
    fn add_entity_get_id(doc: &mut Document, entity: Entity) -> EntityId {
        let new_ids = doc.apply(Command::AddEntity(entity)).unwrap();
        assert_single_new_id(&new_ids.entities, &new_ids.layers)
    }

    #[test]
    fn new_document_has_single_default_layer_as_current() {
        let doc = Document::new();
        assert_eq!(doc.layer_count(), 1);
        assert_eq!(doc.entity_count(), 0);
        assert_eq!(doc.current_layer(), doc.default_layer());
        assert!(doc.layer(doc.current_layer()).is_some());
        assert!(!doc.can_undo());
        assert!(!doc.can_redo());
    }

    #[test]
    fn add_entity_undo_redo_roundtrip() {
        let mut doc = Document::new();
        let cmd = add_line(&doc, line(0.0));
        doc.apply(cmd).unwrap();
        assert_eq!(doc.entity_count(), 1);

        assert!(doc.undo());
        assert_eq!(doc.entity_count(), 0);

        assert!(doc.redo());
        assert_eq!(doc.entity_count(), 1);
    }

    #[test]
    fn remove_entity_undo_fully_restores() {
        let mut doc = Document::new();
        let layer = doc.current_layer();
        let style = Style {
            color: Some(Rgb::new(10, 20, 30)),
            width: 2.5,
        };
        let original = Entity::new(line(3.0), layer, style);
        let id = add_entity_get_id(&mut doc, original.clone());

        doc.apply(Command::RemoveEntity(id)).unwrap();
        assert_eq!(doc.entity_count(), 0);

        assert!(doc.undo());
        assert_eq!(doc.entity_count(), 1);
        let restored = only_entity(&doc);
        // 墓標方式によりキーも保存される。
        assert_eq!(restored, id);
        // geom / layer / style がすべて完全復元される。
        assert_eq!(*doc.entity(restored).unwrap(), original);
    }

    #[test]
    fn modify_entity_undo_redo_swaps_geometry() {
        let mut doc = Document::new();
        let layer = doc.current_layer();
        let id = add_entity_get_id(&mut doc, Entity::new(line(0.0), layer, Style::inherited()));

        doc.apply(Command::ModifyEntity {
            id,
            new_geom: line(5.0),
        })
        .unwrap();
        assert_eq!(doc.entity(id).unwrap().geom, line(5.0));

        assert!(doc.undo());
        assert_eq!(doc.entity(id).unwrap().geom, line(0.0));

        assert!(doc.redo());
        assert_eq!(doc.entity(id).unwrap().geom, line(5.0));
    }

    #[test]
    fn missing_entity_ops_error_without_mutating() {
        let mut doc = Document::new();
        let ghost = EntityId::default();

        assert_eq!(
            doc.apply(Command::RemoveEntity(ghost)),
            Err(CoreError::EntityNotFound(ghost))
        );
        assert_eq!(
            doc.apply(Command::ModifyEntity {
                id: ghost,
                new_geom: line(1.0),
            }),
            Err(CoreError::EntityNotFound(ghost))
        );
        // 失敗しても履歴は積まれない。
        assert!(!doc.can_undo());
        assert_eq!(doc.entity_count(), 0);
    }

    #[test]
    fn add_entity_with_unknown_layer_errors() {
        let mut doc = Document::new();
        let ghost = LayerId::default();
        assert_eq!(
            doc.apply(Command::AddEntity(Entity::new(
                line(0.0),
                ghost,
                Style::inherited(),
            ))),
            Err(CoreError::LayerNotFound(ghost))
        );
        assert_eq!(doc.entity_count(), 0);
        assert!(!doc.can_undo());
    }

    #[test]
    fn default_layer_cannot_be_deleted() {
        let mut doc = Document::new();
        assert_eq!(
            doc.apply(Command::RemoveLayer(doc.default_layer())),
            Err(CoreError::CannotDeleteDefaultLayer)
        );
        assert_eq!(doc.layer_count(), 1);
    }

    #[test]
    fn current_layer_cannot_be_deleted() {
        let mut doc = Document::new();
        let extra = add_layer_get_id(&mut doc, "aux");
        doc.apply(Command::SetCurrentLayer(extra)).unwrap();
        assert_eq!(
            doc.apply(Command::RemoveLayer(extra)),
            Err(CoreError::CannotDeleteCurrentLayer)
        );
        assert!(doc.layer(extra).is_some());
    }

    #[test]
    fn non_empty_layer_cannot_be_deleted() {
        let mut doc = Document::new();
        let extra = add_layer_get_id(&mut doc, "aux");
        doc.apply(Command::AddEntity(Entity::new(
            line(0.0),
            extra,
            Style::inherited(),
        )))
        .unwrap();
        assert_eq!(
            doc.apply(Command::RemoveLayer(extra)),
            Err(CoreError::LayerNotEmpty(extra))
        );
    }

    #[test]
    fn layer_add_remove_undo_redo() {
        let mut doc = Document::new();
        let extra = add_layer_get_id(&mut doc, "aux");
        assert_eq!(doc.layer_count(), 2);

        doc.apply(Command::RemoveLayer(extra)).unwrap();
        assert_eq!(doc.layer_count(), 1);
        assert!(doc.layer(extra).is_none());

        assert!(doc.undo());
        assert_eq!(doc.layer_count(), 2);
        assert_eq!(doc.layer(extra).unwrap().name, "aux");

        assert!(doc.redo());
        assert!(doc.layer(extra).is_none());
    }

    #[test]
    fn set_layer_props_undo_redo() {
        let mut doc = Document::new();
        let id = doc.default_layer();
        let before = doc.layer(id).unwrap().clone();

        let mut after = before.clone();
        after.visible = false;
        after.color = Rgb::BLACK;
        doc.apply(Command::SetLayerProps {
            id,
            props: after.clone(),
        })
        .unwrap();
        assert_eq!(*doc.layer(id).unwrap(), after);

        assert!(doc.undo());
        assert_eq!(*doc.layer(id).unwrap(), before);

        assert!(doc.redo());
        assert_eq!(*doc.layer(id).unwrap(), after);
    }

    #[test]
    fn set_current_layer_undo_redo() {
        let mut doc = Document::new();
        let default = doc.default_layer();
        let extra = add_layer_get_id(&mut doc, "aux");

        doc.apply(Command::SetCurrentLayer(extra)).unwrap();
        assert_eq!(doc.current_layer(), extra);

        assert!(doc.undo());
        assert_eq!(doc.current_layer(), default);

        assert!(doc.redo());
        assert_eq!(doc.current_layer(), extra);
    }

    #[test]
    fn new_command_clears_redo_stack() {
        let mut doc = Document::new();
        doc.apply(Command::AddEntity(Entity::new(
            line(0.0),
            doc.current_layer(),
            Style::inherited(),
        )))
        .unwrap();
        assert!(doc.undo());
        assert!(doc.can_redo());

        // 新規コマンドで redo スタックはクリアされる。
        doc.apply(Command::AddEntity(Entity::new(
            line(1.0),
            doc.current_layer(),
            Style::inherited(),
        )))
        .unwrap();
        assert!(!doc.can_redo());
        // redo しても取り消した最初の追加は復活しない。
        assert!(!doc.redo());
        assert_eq!(doc.entity_count(), 1);
    }

    #[test]
    fn dependent_commands_survive_undo_redo_roundtrip() {
        // Add A → Modify A を undo→undo→redo→redo しても、キー安定性のおかげで
        // 依存する Modify が正しく対象を見つけられる。
        let mut doc = Document::new();
        let layer = doc.current_layer();
        let id = add_entity_get_id(&mut doc, Entity::new(line(0.0), layer, Style::inherited()));
        doc.apply(Command::ModifyEntity {
            id,
            new_geom: line(9.0),
        })
        .unwrap();

        assert!(doc.undo()); // modify 取り消し → line(0)
        assert!(doc.undo()); // add 取り消し → 0 件
        assert_eq!(doc.entity_count(), 0);

        assert!(doc.redo()); // add やり直し
        assert!(doc.redo()); // modify やり直し
        let restored = only_entity(&doc);
        assert_eq!(restored, id); // 同一キーが復元される
        assert_eq!(doc.entity(restored).unwrap().geom, line(9.0));

        // これ以上 redo/undo は片側だけ。往復の一貫性を最終確認。
        assert!(!doc.redo());
        assert!(doc.undo());
        assert!(doc.undo());
        assert!(!doc.undo());
        assert_eq!(doc.entity_count(), 0);
    }

    /// 生存エンティティの ID を安定順（挿入順）で列挙する。バッチで複数追加した要素を
    /// 特定するのに使う。公開の [`Document::entities`] 経由なので、墓標（削除済み）
    /// スロットが混入することはない。
    fn entity_ids(doc: &Document) -> Vec<EntityId> {
        doc.entities().map(|(k, _)| k).collect()
    }

    #[test]
    fn batch_modify_undo_redo_moves_all_entities_as_one_unit() {
        // 検収シナリオ: 矩形選択→複数移動→undo 1 回で全戻り→redo 1 回で全再移動。
        let mut doc = Document::new();
        let layer = doc.current_layer();
        for x in [0.0, 1.0, 2.0] {
            doc.apply(Command::AddEntity(Entity::new(
                line(x),
                layer,
                Style::inherited(),
            )))
            .unwrap();
        }
        let ids = entity_ids(&doc);
        assert_eq!(ids.len(), 3);

        // 各エンティティを +10 相当の別幾何へまとめて移動するバッチ。
        let batch = Command::Batch(
            ids.iter()
                .enumerate()
                .map(|(i, &id)| Command::ModifyEntity {
                    id,
                    new_geom: line(10.0 + i as f64),
                })
                .collect(),
        );
        doc.apply(batch).unwrap();
        for (i, &id) in ids.iter().enumerate() {
            assert_eq!(doc.entity(id).unwrap().geom, line(10.0 + i as f64));
        }

        // undo 1 回で全エンティティが元に戻る。
        assert!(doc.undo());
        for (i, &id) in ids.iter().enumerate() {
            assert_eq!(doc.entity(id).unwrap().geom, line(i as f64));
        }
        // バッチは 1 単位なので、もう戻す元操作はエンティティ追加の 3 件のみ。
        assert!(doc.can_undo());

        // redo 1 回で全エンティティが再び移動する。
        assert!(doc.redo());
        for (i, &id) in ids.iter().enumerate() {
            assert_eq!(doc.entity(id).unwrap().geom, line(10.0 + i as f64));
        }
    }

    #[test]
    fn batch_add_undo_redo_is_single_unit() {
        let mut doc = Document::new();
        let layer = doc.current_layer();
        let batch = Command::Batch(vec![
            Command::AddEntity(Entity::new(line(0.0), layer, Style::inherited())),
            Command::AddEntity(Entity::new(line(1.0), layer, Style::inherited())),
            Command::AddEntity(Entity::new(line(2.0), layer, Style::inherited())),
        ]);
        doc.apply(batch).unwrap();
        assert_eq!(doc.entity_count(), 3);

        // undo 1 回で 3 件すべて消える。
        assert!(doc.undo());
        assert_eq!(doc.entity_count(), 0);
        // 空ドキュメントに戻るので、これ以上 undo するものはない。
        assert!(!doc.can_undo());

        // redo 1 回で 3 件すべて復活する。
        assert!(doc.redo());
        assert_eq!(doc.entity_count(), 3);
    }

    #[test]
    fn batch_is_atomic_on_sub_command_failure() {
        // 2 番目のサブコマンドが失敗するバッチ。1 番目の効果も残ってはならない。
        let mut doc = Document::new();
        let layer = doc.current_layer();
        let id = add_entity_get_id(&mut doc, Entity::new(line(0.0), layer, Style::inherited()));
        let ghost = EntityId::default();

        let can_undo_before = doc.can_undo();
        let batch = Command::Batch(vec![
            // 1 番目は成功する変更。
            Command::ModifyEntity {
                id,
                new_geom: line(7.0),
            },
            // 2 番目は存在しない ID なので失敗する。
            Command::ModifyEntity {
                id: ghost,
                new_geom: line(8.0),
            },
        ]);
        assert_eq!(doc.apply(batch), Err(CoreError::EntityNotFound(ghost)));

        // 1 番目の変更も巻き戻り、幾何は完全に元通り。
        assert_eq!(doc.entity(id).unwrap().geom, line(0.0));
        // 失敗したバッチは履歴に積まれない（undo 可能性は変化しない）。
        assert_eq!(doc.can_undo(), can_undo_before);
        assert_eq!(doc.entity_count(), 1);
    }

    #[test]
    fn empty_batch_is_noop() {
        let mut doc = Document::new();
        doc.apply(Command::AddEntity(Entity::new(
            line(0.0),
            doc.current_layer(),
            Style::inherited(),
        )))
        .unwrap();
        let can_undo_before = doc.can_undo();
        let count_before = doc.entity_count();

        // 空バッチは Ok(NewIds::default()) を返し、状態も undo 可能性も変えない。
        assert_eq!(doc.apply(Command::Batch(vec![])), Ok(NewIds::default()));
        assert_eq!(doc.can_undo(), can_undo_before);
        assert_eq!(doc.entity_count(), count_before);
        // 直近操作（エンティティ追加）を undo できる状態が保たれている。
        assert!(doc.undo());
        assert_eq!(doc.entity_count(), 0);
    }

    #[test]
    fn nested_empty_batch_is_noop() {
        // 回帰: ネストした空バッチ（サブ数は 0 でない）が履歴を汚さないこと。
        // 以前はトップレベルの空バッチしか特判しておらず、Batch([Batch([])]) は
        // execute を通って undo_stack に積まれ、can_undo が偽陽性になっていた。
        let mut doc = Document::new();
        let cmd = add_line(&doc, line(0.0));
        doc.apply(cmd).unwrap();

        let can_undo_before = doc.can_undo();
        let undo_len_before = doc.undo_stack.len();
        let redo_len_before = doc.redo_stack.len();

        // 1 段ネストした空バッチ。
        assert_eq!(
            doc.apply(Command::Batch(vec![Command::Batch(vec![])])),
            Ok(NewIds::default())
        );
        assert_eq!(doc.can_undo(), can_undo_before);
        assert_eq!(doc.undo_stack.len(), undo_len_before);
        assert_eq!(doc.redo_stack.len(), redo_len_before);

        // さらに深くネストした空バッチも同様に no-op。
        assert_eq!(
            doc.apply(Command::Batch(vec![Command::Batch(vec![Command::Batch(
                vec![]
            )])])),
            Ok(NewIds::default())
        );
        assert_eq!(doc.can_undo(), can_undo_before);
        assert_eq!(doc.undo_stack.len(), undo_len_before);
        assert_eq!(doc.redo_stack.len(), redo_len_before);

        // 直近の実操作（エンティティ追加）は依然として undo できる。
        assert!(doc.undo());
        assert_eq!(doc.entity_count(), 0);
    }

    #[test]
    fn set_current_layer_to_same_layer_is_noop() {
        // 回帰: 現在と同じレイヤーを SetCurrentLayer しても履歴を汚さないこと。
        // 以前は before == after でも Applied::SetCurrentLayer が undo_stack に積まれ、
        // can_undo が false→true に変化し、次の Ctrl+Z が空振りになっていた。
        let mut doc = Document::new();
        let current = doc.current_layer();

        let can_undo_before = doc.can_undo();
        let undo_len_before = doc.undo_stack.len();
        let redo_len_before = doc.redo_stack.len();

        assert_eq!(
            doc.apply(Command::SetCurrentLayer(current)),
            Ok(NewIds::default())
        );
        assert_eq!(doc.current_layer(), current);
        assert_eq!(doc.can_undo(), can_undo_before);
        assert_eq!(doc.undo_stack.len(), undo_len_before);
        assert_eq!(doc.redo_stack.len(), redo_len_before);
    }

    #[test]
    fn set_layer_props_with_identical_props_is_noop() {
        // 回帰: 現在と全く同じ props を設定しても before == after なので履歴を汚さない。
        let mut doc = Document::new();
        let id = doc.default_layer();
        let same = doc.layer(id).unwrap().clone();

        let can_undo_before = doc.can_undo();
        let undo_len_before = doc.undo_stack.len();
        let redo_len_before = doc.redo_stack.len();

        assert_eq!(
            doc.apply(Command::SetLayerProps { id, props: same }),
            Ok(NewIds::default())
        );
        assert_eq!(doc.can_undo(), can_undo_before);
        assert_eq!(doc.undo_stack.len(), undo_len_before);
        assert_eq!(doc.redo_stack.len(), redo_len_before);
    }

    #[test]
    fn modify_entity_with_identical_geometry_is_noop() {
        // 回帰: 現在と全く同じ幾何を設定しても before == after なので履歴を汚さない。
        let mut doc = Document::new();
        let layer = doc.current_layer();
        let id = add_entity_get_id(&mut doc, Entity::new(line(0.0), layer, Style::inherited()));

        let can_undo_before = doc.can_undo();
        let undo_len_before = doc.undo_stack.len();
        let redo_len_before = doc.redo_stack.len();

        assert_eq!(
            doc.apply(Command::ModifyEntity {
                id,
                new_geom: line(0.0),
            }),
            Ok(NewIds::default())
        );
        assert_eq!(doc.entity(id).unwrap().geom, line(0.0));
        assert_eq!(doc.can_undo(), can_undo_before);
        assert_eq!(doc.undo_stack.len(), undo_len_before);
        assert_eq!(doc.redo_stack.len(), redo_len_before);
    }

    #[test]
    fn noop_command_preserves_redo_stack() {
        // 意味的 no-op は redo スタックを保持する（クリアしない）こと。
        let mut doc = Document::new();
        let current = doc.current_layer();
        doc.apply(add_line(&doc, line(0.0))).unwrap();
        assert!(doc.undo()); // redo スタックに 1 件積まれる
        assert!(doc.can_redo());

        // no-op な SetCurrentLayer は redo を消さない。
        assert_eq!(
            doc.apply(Command::SetCurrentLayer(current)),
            Ok(NewIds::default())
        );
        assert!(doc.can_redo());
        assert!(doc.redo());
        assert_eq!(doc.entity_count(), 1);
    }

    #[test]
    fn batch_and_single_commands_interleave_consistently() {
        let mut doc = Document::new();
        let layer = doc.current_layer();

        // 単一: A 追加
        let a = add_entity_get_id(&mut doc, Entity::new(line(0.0), layer, Style::inherited()));

        // バッチ: B, C 追加
        doc.apply(Command::Batch(vec![
            Command::AddEntity(Entity::new(line(1.0), layer, Style::inherited())),
            Command::AddEntity(Entity::new(line(2.0), layer, Style::inherited())),
        ]))
        .unwrap();
        assert_eq!(doc.entity_count(), 3);

        // 単一: A を移動
        doc.apply(Command::ModifyEntity {
            id: a,
            new_geom: line(5.0),
        })
        .unwrap();
        assert_eq!(doc.entity(a).unwrap().geom, line(5.0));

        // undo 3 回で完全に空へ戻る（単一・バッチ・単一の順で 1 単位ずつ）。
        assert!(doc.undo()); // A 移動を取り消し
        assert_eq!(doc.entity(a).unwrap().geom, line(0.0));
        assert!(doc.undo()); // バッチ（B,C 追加）を取り消し
        assert_eq!(doc.entity_count(), 1);
        assert!(doc.undo()); // A 追加を取り消し
        assert_eq!(doc.entity_count(), 0);
        assert!(!doc.can_undo());

        // redo 3 回で完全に再現する。
        assert!(doc.redo()); // A 追加
        assert!(doc.redo()); // バッチ
        assert!(doc.redo()); // A 移動
        assert_eq!(doc.entity_count(), 3);
        assert_eq!(doc.entity(a).unwrap().geom, line(5.0));
        assert!(!doc.can_redo());
    }

    // --- テスト用ヘルパ ---

    /// レイヤーを 1 つ追加し、その ID を返す。
    ///
    /// 以前は適用前後の集合差分を取るハックで新規 ID を特定していたが、`apply` が
    /// [`NewIds`] を返すようになったため、戻り値から直接取り出せばよい。
    fn add_layer_get_id(doc: &mut Document, name: &str) -> LayerId {
        let new_ids = doc
            .apply(Command::AddLayer(Layer::new(name, Rgb::WHITE)))
            .unwrap();
        assert_single_new_id(&new_ids.layers, &new_ids.entities)
    }

    /// 指定レイヤーをロック状態にする（`SetLayerProps` で `locked = true` を適用する）。
    fn lock_layer(doc: &mut Document, id: LayerId) {
        let mut props = doc.layer(id).unwrap().clone();
        props.locked = true;
        doc.apply(Command::SetLayerProps { id, props }).unwrap();
    }

    #[test]
    fn locked_layer_blocks_modify_and_remove() {
        let mut doc = Document::new();
        let layer = doc.current_layer();
        let id = add_entity_get_id(&mut doc, Entity::new(line(0.0), layer, Style::inherited()));

        // カレントレイヤーをロックする。
        lock_layer(&mut doc, layer);

        // ModifyEntity はロックされたレイヤーのエンティティに対して拒否される。
        assert_eq!(
            doc.apply(Command::ModifyEntity {
                id,
                new_geom: line(9.0),
            }),
            Err(CoreError::LayerLocked(layer))
        );
        assert_eq!(doc.entity(id).unwrap().geom, line(0.0));

        // RemoveEntity も同様に拒否される。
        assert_eq!(
            doc.apply(Command::RemoveEntity(id)),
            Err(CoreError::LayerLocked(layer))
        );
        assert_eq!(doc.entity_count(), 1);
        assert!(doc.entity(id).is_some());
    }

    #[test]
    fn locked_layer_blocks_add_entity() {
        // ロック済みレイヤーへの新規追加は、Modify/Remove と対称に拒否される。
        let mut doc = Document::new();
        let layer = doc.current_layer();

        lock_layer(&mut doc, layer);

        let can_undo_before = doc.can_undo();
        assert_eq!(
            doc.apply(Command::AddEntity(Entity::new(
                line(0.0),
                layer,
                Style::inherited(),
            ))),
            Err(CoreError::LayerLocked(layer))
        );
        // 状態も履歴も変わらない。
        assert_eq!(doc.entity_count(), 0);
        assert_eq!(doc.can_undo(), can_undo_before);
    }

    #[test]
    fn locked_layer_in_batch_rolls_back_whole_batch() {
        // ロックされていないレイヤーの b と、ロックされた別レイヤーの a を用意し、
        // 「1 番目は成功するはずの変更・2 番目はロックで失敗する変更」を 1 バッチにする。
        // バッチはアトミックなので、1 番目の効果も残らず完全に巻き戻るはず。
        let mut doc = Document::new();
        let default_layer = doc.current_layer();
        let extra_layer = add_layer_get_id(&mut doc, "aux");

        let b = add_entity_get_id(
            &mut doc,
            Entity::new(line(0.0), default_layer, Style::inherited()),
        );
        let a = add_entity_get_id(
            &mut doc,
            Entity::new(line(1.0), extra_layer, Style::inherited()),
        );

        lock_layer(&mut doc, extra_layer);

        let can_undo_before = doc.can_undo();
        let batch = Command::Batch(vec![
            // 1 番目: ロックされていない default_layer 上の b への変更（単体なら成功する）。
            Command::ModifyEntity {
                id: b,
                new_geom: line(20.0),
            },
            // 2 番目: ロックされた extra_layer 上の a への変更（失敗する）。
            Command::ModifyEntity {
                id: a,
                new_geom: line(30.0),
            },
        ]);
        assert_eq!(doc.apply(batch), Err(CoreError::LayerLocked(extra_layer)));

        // バッチ全体が巻き戻り、b の変更も残らない。
        assert_eq!(doc.entity(b).unwrap().geom, line(0.0));
        assert_eq!(doc.entity(a).unwrap().geom, line(1.0));
        assert_eq!(doc.can_undo(), can_undo_before);
    }

    #[test]
    fn apply_add_entity_returns_new_entity_id() {
        let mut doc = Document::new();
        let layer = doc.current_layer();
        let new_ids = doc
            .apply(Command::AddEntity(Entity::new(
                line(0.0),
                layer,
                Style::inherited(),
            )))
            .unwrap();
        let id = assert_single_new_id(&new_ids.entities, &new_ids.layers);
        assert!(doc.entity(id).is_some());
    }

    #[test]
    fn apply_add_layer_returns_new_layer_id() {
        let mut doc = Document::new();
        let new_ids = doc
            .apply(Command::AddLayer(Layer::new("aux", Rgb::WHITE)))
            .unwrap();
        let id = assert_single_new_id(&new_ids.layers, &new_ids.entities);
        assert!(doc.layer(id).is_some());
    }

    #[test]
    fn apply_batch_collects_new_entity_ids_in_order() {
        let mut doc = Document::new();
        let layer = doc.current_layer();
        let batch = Command::Batch(vec![
            Command::AddEntity(Entity::new(line(0.0), layer, Style::inherited())),
            Command::AddEntity(Entity::new(line(1.0), layer, Style::inherited())),
            Command::AddEntity(Entity::new(line(2.0), layer, Style::inherited())),
        ]);
        let new_ids = doc.apply(batch).unwrap();
        assert_eq!(new_ids.entities.len(), 3);
        assert!(new_ids.layers.is_empty());
        // 適用順で並んでいることを、それぞれの幾何から確認する。
        for (i, &id) in new_ids.entities.iter().enumerate() {
            assert_eq!(doc.entity(id).unwrap().geom, line(i as f64));
        }
    }

    #[test]
    fn apply_modify_and_remove_return_empty_new_ids() {
        let mut doc = Document::new();
        let layer = doc.current_layer();
        let id = add_entity_get_id(&mut doc, Entity::new(line(0.0), layer, Style::inherited()));

        let new_ids = doc
            .apply(Command::ModifyEntity {
                id,
                new_geom: line(5.0),
            })
            .unwrap();
        assert_eq!(new_ids, NewIds::default());

        let new_ids = doc.apply(Command::RemoveEntity(id)).unwrap();
        assert_eq!(new_ids, NewIds::default());
    }

    /// NaN 座標を持つ、幾何として不正な線分。
    fn invalid_line() -> EntityGeom {
        EntityGeom::Shape(Shape::Line(LineSeg::new(
            Point2::new(f64::NAN, 0.0),
            Point2::new(1.0, 1.0),
        )))
    }

    #[test]
    fn add_entity_rejects_invalid_geometry() {
        // DESIGN.md M4 タスク15: NaN/∞座標などの不正ジオメトリは AddEntity で拒否される。
        let mut doc = Document::new();
        let entities_before = doc.entity_count();

        let result = doc.apply(add_line(&doc, invalid_line()));
        assert!(
            matches!(result, Err(CoreError::InvalidGeometry(_))),
            "expected InvalidGeometry, got {result:?}"
        );
        // 状態が変化していないこと(部分適用がない)。
        assert_eq!(doc.entity_count(), entities_before);
        assert!(!doc.can_undo());
    }

    #[test]
    fn modify_entity_rejects_invalid_geometry() {
        // DESIGN.md M4 タスク15: NaN/∞座標などの不正ジオメトリへの差し替えは
        // ModifyEntity で拒否される。
        let mut doc = Document::new();
        let layer = doc.current_layer();
        let id = add_entity_get_id(&mut doc, Entity::new(line(0.0), layer, Style::inherited()));
        let undo_len_before = doc.undo_stack.len();

        let result = doc.apply(Command::ModifyEntity {
            id,
            new_geom: invalid_line(),
        });
        assert!(
            matches!(result, Err(CoreError::InvalidGeometry(_))),
            "expected InvalidGeometry, got {result:?}"
        );
        // 元の幾何が保たれ、履歴も汚れていないこと(部分適用がない)。
        assert_eq!(doc.entity(id).unwrap().geom, line(0.0));
        assert_eq!(doc.undo_stack.len(), undo_len_before);
    }

    // --- 世代カウンタ（dirty 判定の基盤、DESIGN.md M4 タスク14）---

    #[test]
    fn new_document_generation_is_zero() {
        // 新規ドキュメントの世代は初期基準点 0。
        let doc = Document::new();
        assert_eq!(doc.generation(), 0);
    }

    #[test]
    fn applied_change_bumps_generation() {
        // 実変更のあるコマンドは世代を単調増加させる。
        let mut doc = Document::new();
        assert_eq!(doc.generation(), 0);
        doc.apply(add_line(&doc, line(0.0))).unwrap();
        let g1 = doc.generation();
        assert!(g1 > 0);
        doc.apply(add_line(&doc, line(1.0))).unwrap();
        assert!(doc.generation() > g1);
    }

    #[test]
    fn save_change_undo_returns_to_saved_generation() {
        // シナリオ1: 保存 → 変更 → undo で保存時点へ戻ったら、世代が保存時の世代と一致する
        // （= dirty でない）。
        let mut doc = Document::new();
        doc.apply(add_line(&doc, line(0.0))).unwrap();
        // 「保存した」瞬間の世代を記録する（アプリ側の saved_generation 相当）。
        let saved_generation = doc.generation();

        // 1 操作して未保存の変更が生じる（世代が保存時と食い違う）。
        doc.apply(add_line(&doc, line(1.0))).unwrap();
        assert_ne!(doc.generation(), saved_generation);

        // undo で保存時点の内容へ厳密に戻ると、世代も保存時と一致する。
        assert!(doc.undo());
        assert_eq!(doc.generation(), saved_generation);
    }

    #[test]
    fn change_undo_redo_restores_changed_generation() {
        // シナリオ2: 変更 → undo → redo で、世代が変更後の世代へ戻る（= 再び dirty）。
        let mut doc = Document::new();
        doc.apply(add_line(&doc, line(0.0))).unwrap();
        let saved_generation = doc.generation();

        doc.apply(add_line(&doc, line(1.0))).unwrap();
        let changed_generation = doc.generation();
        assert_ne!(changed_generation, saved_generation);

        assert!(doc.undo());
        assert_eq!(doc.generation(), saved_generation);

        // redo は元の適用時に採番した世代をそのまま復元する（新規採番はしない）。
        assert!(doc.redo());
        assert_eq!(doc.generation(), changed_generation);
    }

    #[test]
    fn noop_command_does_not_change_generation() {
        // シナリオ3: 意味的 no-op はドキュメントを変えないので世代も変えない。
        let mut doc = Document::new();
        let current = doc.current_layer();
        doc.apply(add_line(&doc, line(0.0))).unwrap();
        let before = doc.generation();

        // 現在と同じレイヤーの SetCurrentLayer は no-op。
        doc.apply(Command::SetCurrentLayer(current)).unwrap();
        assert_eq!(doc.generation(), before);

        // 空バッチも no-op。
        doc.apply(Command::Batch(vec![])).unwrap();
        assert_eq!(doc.generation(), before);

        // 同一幾何への差し替えも no-op。
        let id = only_entity(&doc);
        doc.apply(Command::ModifyEntity {
            id,
            new_geom: line(0.0),
        })
        .unwrap();
        assert_eq!(doc.generation(), before);
    }

    #[test]
    fn clear_history_resets_generation_and_stacks() {
        // シナリオ4: clear_history 後は世代が初期化され、undo/redo スタックが空になる。
        let mut doc = Document::new();
        doc.apply(add_line(&doc, line(0.0))).unwrap();
        doc.apply(add_line(&doc, line(1.0))).unwrap();
        assert!(doc.undo()); // redo スタックへ 1 件積む
        assert!(doc.generation() > 0);
        assert!(doc.can_undo());
        assert!(doc.can_redo());

        doc.clear_history();

        assert_eq!(doc.generation(), 0);
        assert!(!doc.can_undo());
        assert!(!doc.can_redo());
        assert!(doc.undo_stack.is_empty());
        assert!(doc.redo_stack.is_empty());
    }

    #[test]
    fn undo_to_empty_history_returns_to_initial_generation() {
        // 履歴が空になるまで undo すると、世代は初期基準点 0 に戻る。
        let mut doc = Document::new();
        doc.apply(add_line(&doc, line(0.0))).unwrap();
        assert!(doc.generation() > 0);

        assert!(doc.undo());
        assert_eq!(doc.generation(), 0);
        assert!(!doc.can_undo());
    }

    #[test]
    fn new_command_after_undo_does_not_reuse_generation() {
        // undo 後に別の新規コマンドを適用しても、過去に使った世代番号を再利用しない。
        // これにより「保存時点の内容と違うのに世代が偶然一致して dirty を見落とす」事故を防ぐ。
        let mut doc = Document::new();
        doc.apply(add_line(&doc, line(0.0))).unwrap();
        let saved_generation = doc.generation();
        doc.apply(add_line(&doc, line(1.0))).unwrap();
        let branched_generation = doc.generation();

        // 保存時点まで undo し、別の内容の新規コマンドを適用する。
        assert!(doc.undo());
        assert_eq!(doc.generation(), saved_generation);
        doc.apply(add_line(&doc, line(2.0))).unwrap();

        // 内容が違う（line(1.0) ではなく line(2.0)）ので、世代も分岐前とは異なる。
        assert_ne!(doc.generation(), branched_generation);
        assert_ne!(doc.generation(), saved_generation);
    }
}
