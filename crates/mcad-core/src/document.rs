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
    /// # Errors
    ///
    /// 存在しない ID への操作、デフォルト/カレント/非空レイヤーの削除などで
    /// [`CoreError`] を返す。
    pub fn apply(&mut self, cmd: Command) -> Result<(), CoreError> {
        // execute は「検証 → 変更」の順で行い、検証に失敗した場合は一切変更しない。
        let applied = self.execute(cmd)?;
        self.undo_stack.push(applied);
        self.redo_stack.clear();
        Ok(())
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
                let entity = self.take_entity(id);
                Ok(Applied::RemoveEntity { id, entity })
            }
            Command::ModifyEntity { id, new_geom } => {
                self.require_entity(id)?;
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
    fn only_entity(doc: &Document) -> EntityId {
        let mut it = doc.entities();
        let (id, _) = it.next().expect("expected exactly one entity");
        assert!(it.next().is_none(), "expected exactly one entity");
        id
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
        doc.apply(Command::AddEntity(original.clone())).unwrap();
        let id = only_entity(&doc);

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
        doc.apply(Command::AddEntity(Entity::new(
            line(0.0),
            layer,
            Style::inherited(),
        )))
        .unwrap();
        let id = only_entity(&doc);

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
        doc.apply(Command::AddEntity(Entity::new(
            line(0.0),
            layer,
            Style::inherited(),
        )))
        .unwrap();
        let id = only_entity(&doc);
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

    // --- テスト用ヘルパ ---

    /// レイヤーを 1 つ追加し、その ID を返す。
    fn add_layer_get_id(doc: &mut Document, name: &str) -> LayerId {
        let before: std::collections::HashSet<LayerId> = doc.layers().map(|(k, _)| k).collect();
        doc.apply(Command::AddLayer(Layer::new(name, Rgb::WHITE)))
            .unwrap();
        doc.layers()
            .map(|(k, _)| k)
            .find(|k| !before.contains(k))
            .expect("newly added layer")
    }
}
