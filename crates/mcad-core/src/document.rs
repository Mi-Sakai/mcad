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

use mcad_geom::Shape;

use crate::{Command, CoreError, Entity, EntityId, Layer, LayerId, Rgb};

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
        before: Shape,
        after: Shape,
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

impl NewIds {
    /// [`Applied`] を辿り、新規発行された ID を適用順で `self` へ集約する。
    /// `Batch` は再帰的に辿る。
    fn collect_from(&mut self, applied: &Applied) {
        match applied {
            Applied::AddEntity { id, .. } => self.entities.push(*id),
            Applied::AddLayer { id, .. } => self.layers.push(*id),
            Applied::Batch(subs) => {
                for sub in subs {
                    self.collect_from(sub);
                }
            }
            Applied::RemoveEntity { .. }
            | Applied::ModifyEntity { .. }
            | Applied::RemoveLayer { .. }
            | Applied::SetLayerProps { .. }
            | Applied::SetCurrentLayer { .. } => {}
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
    undo_stack: Vec<Applied>,
    /// redo スタック（末尾が次に redo する操作）。
    redo_stack: Vec<Applied>,
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
        // 空バッチは「操作なし」として扱う: 状態も履歴も一切変えず Ok(()) を返す。
        // undo できる実体がない記録を undo スタックへ積むと、Ctrl+Z 1 回が
        // 見かけ上「何も戻らない」空振りになりユーザー体験を損なうため、
        // 履歴にも redo スタックにも触れない（redo は保持したままにする）。
        if matches!(&cmd, Command::Batch(subs) if subs.is_empty()) {
            return Ok(NewIds::default());
        }
        // execute は「検証 → 変更」の順で行い、検証に失敗した場合は一切変更しない。
        // バッチもこの保証を境界全体で満たす（途中失敗時は適用済み分を巻き戻す）。
        let applied = self.execute(cmd)?;
        let mut new_ids = NewIds::default();
        new_ids.collect_from(&applied);
        self.undo_stack.push(applied);
        self.redo_stack.clear();
        Ok(new_ids)
    }

    /// 直近の操作を取り消す。取り消せた場合 `true`。
    pub fn undo(&mut self) -> bool {
        match self.undo_stack.pop() {
            Some(applied) => {
                self.revert(&applied);
                self.redo_stack.push(applied);
                true
            }
            None => false,
        }
    }

    /// 直近に取り消した操作をやり直す。やり直せた場合 `true`。
    pub fn redo(&mut self) -> bool {
        match self.redo_stack.pop() {
            Some(applied) => {
                self.reapply(&applied);
                self.undo_stack.push(applied);
                true
            }
            None => false,
        }
    }

    // ---- 内部ヘルパ ----

    /// コマンドを検証・実行し、逆操作記録を返す。検証失敗時は状態不変。
    fn execute(&mut self, cmd: Command) -> Result<Applied, CoreError> {
        match cmd {
            Command::AddEntity(entity) => {
                // 宙に浮いた layer 参照を防ぐため、所属レイヤーの存在を検証する。
                self.require_layer(entity.layer)?;
                let id = self.entities.insert(Some(entity.clone()));
                Ok(Applied::AddEntity { id, entity })
            }
            Command::RemoveEntity(id) => {
                self.require_entity(id)?;
                self.require_entity_layer_unlocked(id)?;
                let entity = self.take_entity(id);
                Ok(Applied::RemoveEntity { id, entity })
            }
            Command::ModifyEntity { id, new_geom } => {
                self.require_entity(id)?;
                self.require_entity_layer_unlocked(id)?;
                let slot = self.live_entity_mut(id);
                let before = std::mem::replace(&mut slot.geom, new_geom.clone());
                Ok(Applied::ModifyEntity {
                    id,
                    before,
                    after: new_geom,
                })
            }
            Command::AddLayer(layer) => {
                let id = self.layers.insert(Some(layer.clone()));
                Ok(Applied::AddLayer { id, layer })
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
                Ok(Applied::RemoveLayer { id, layer })
            }
            Command::SetLayerProps { id, props } => {
                self.require_layer(id)?;
                let before = self
                    .layers
                    .get_mut(id)
                    .expect("layer key checked by require_layer")
                    .replace(props.clone())
                    .expect("layer is live (checked by require_layer)");
                Ok(Applied::SetLayerProps {
                    id,
                    before,
                    after: props,
                })
            }
            Command::SetCurrentLayer(id) => {
                self.require_layer(id)?;
                let before = self.current_layer;
                self.current_layer = id;
                Ok(Applied::SetCurrentLayer { before, after: id })
            }
            Command::Batch(subs) => {
                let mut applied: Vec<Applied> = Vec::with_capacity(subs.len());
                for sub in subs {
                    match self.execute(sub) {
                        Ok(a) => applied.push(a),
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
                Ok(Applied::Batch(applied))
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

    fn require_entity(&self, id: EntityId) -> Result<(), CoreError> {
        if self.entity(id).is_some() {
            Ok(())
        } else {
            Err(CoreError::EntityNotFound(id))
        }
    }

    fn require_layer(&self, id: LayerId) -> Result<(), CoreError> {
        if self.layer(id).is_some() {
            Ok(())
        } else {
            Err(CoreError::LayerNotFound(id))
        }
    }

    /// 指定エンティティ（生存を前提とする。呼び出し前に `require_entity` 済みであること）が
    /// 所属するレイヤーがロックされていないか検証する。ロックされていれば
    /// [`CoreError::LayerLocked`] を返す（`Layer::locked` の doc comment 通り、変更・削除を
    /// 一元的に禁止する）。
    ///
    /// 所属レイヤーが（不変条件違反で）存在しない場合はここでは扱わず `Ok(())` とする。
    fn require_entity_layer_unlocked(&self, id: EntityId) -> Result<(), CoreError> {
        let layer_id = self
            .entity(id)
            .expect("entity must be live (checked by require_entity)")
            .layer;
        match self.layer(layer_id) {
            Some(layer) if layer.locked => Err(CoreError::LayerLocked(layer_id)),
            _ => Ok(()),
        }
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
    fn set_entity_geom(&mut self, id: EntityId, geom: Shape) {
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
    use mcad_geom::{LineSeg, Point2};

    fn line(x: f64) -> Shape {
        Shape::Line(LineSeg::new(Point2::new(x, 0.0), Point2::new(x, 1.0)))
    }

    /// カレントレイヤー上に、指定幾何のエンティティを追加するコマンドを作る。
    fn add_line(doc: &Document, geom: Shape) -> Command {
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

    /// [`Command::AddEntity`] を適用し、`apply` の戻り値 [`NewIds`] から新規発行された
    /// エンティティ ID を直接取り出す。以前は適用前後の集合差分を取るハックで代用していたが、
    /// `apply` が `NewIds` を返すようになったため不要になった。
    fn add_entity_get_id(doc: &mut Document, entity: Entity) -> EntityId {
        let new_ids = doc.apply(Command::AddEntity(entity)).unwrap();
        assert_eq!(new_ids.entities.len(), 1, "expected exactly one new entity");
        assert!(new_ids.layers.is_empty());
        new_ids.entities[0]
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

    /// 全 ID を安定順（挿入順）で列挙する。バッチで複数追加した要素を特定するのに使う。
    fn entity_ids(doc: &Document) -> Vec<EntityId> {
        doc.entities.iter().map(|(k, _)| k).collect()
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
        assert_eq!(new_ids.layers.len(), 1, "expected exactly one new layer");
        assert!(new_ids.entities.is_empty());
        new_ids.layers[0]
    }

    #[test]
    fn locked_layer_blocks_modify_and_remove() {
        let mut doc = Document::new();
        let layer = doc.current_layer();
        let id = add_entity_get_id(&mut doc, Entity::new(line(0.0), layer, Style::inherited()));

        // カレントレイヤーをロックする。
        let mut locked = doc.layer(layer).unwrap().clone();
        locked.locked = true;
        doc.apply(Command::SetLayerProps {
            id: layer,
            props: locked,
        })
        .unwrap();

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

        let mut locked = doc.layer(extra_layer).unwrap().clone();
        locked.locked = true;
        doc.apply(Command::SetLayerProps {
            id: extra_layer,
            props: locked,
        })
        .unwrap();

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
        assert_eq!(new_ids.entities.len(), 1);
        assert!(new_ids.layers.is_empty());
        assert!(doc.entity(new_ids.entities[0]).is_some());
    }

    #[test]
    fn apply_add_layer_returns_new_layer_id() {
        let mut doc = Document::new();
        let new_ids = doc
            .apply(Command::AddLayer(Layer::new("aux", Rgb::WHITE)))
            .unwrap();
        assert_eq!(new_ids.layers.len(), 1);
        assert!(new_ids.entities.is_empty());
        assert!(doc.layer(new_ids.layers[0]).is_some());
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
}
