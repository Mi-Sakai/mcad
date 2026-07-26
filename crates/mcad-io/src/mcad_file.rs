//! `.mcad`（JSON）のファイル形式と export/import。
//!
//! # 設計方針（DESIGN.md 3.3）
//!
//! [`mcad_core::Document`] は `SlotMap` の生キー（`LayerId`）と undo/redo 履歴を
//! 持つため **そのままシリアライズしない**。ファイルには次のポータブルな DTO
//! （[`FileDocument`]）だけを書く:
//!
//! - レイヤー参照は `LayerId` ではなく **`layers` 配列へのインデックス**（安定な
//!   ファイル ID）。
//! - レイヤー自体も core の [`Layer`] をそのまま流用せず、io 専用の DTO
//!   （[`FileLayer`]）として書く。core 側の型変更がファイル形式へ直接漏れないように
//!   するため。
//! - 生存中のレイヤー・エンティティのみを列挙する。undo/redo 履歴・墓標は保存しない。
//! - `version` フィールド必須（現行は `3`）。書き出しは常に v3。読込は v1・v2 を
//!   後方互換で受理し、それ以外の未知バージョンは拒否する（[`from_json`]）。
//!
//! # バージョン履歴と後方互換の変換規則
//!
//! | version | 差分 | 読込時の変換 |
//! |---|---|---|
//! | 1 | 幾何が [`Shape`] のみ。レイヤーは `order` なし | [`EntityGeom::Shape`] で包む + `order = 配列インデックス` |
//! | 2 | 幾何が [`EntityGeom`]（M6 でテキスト・寸法を追加）。レイヤーは `order` なし | `order = 配列インデックス` |
//! | 3 | レイヤーに `order`（重ね順）を追加。現行 | なし |
//!
//! `order` を持たない v1/v2 のレイヤーには **配列内のインデックスをそのまま
//! `order` として採用する**。export は常にデフォルトレイヤーを先頭に列挙してきた
//! ため、この規則で復元した重ね順は「ファイル内の元の並び」に一致し、`SlotMap` の
//! 未規定な反復順に依存しない。
//!
//! # import の再構築
//!
//! import は [`Document::new`] から [`Command`] 列で内容を再構築する
//! （`Document` の内部フィールドへ直接触らない）。手順:
//!
//! 1. ファイル全体をバリデーション（バージョン・レイヤー参照・ジオメトリ）。
//! 2. ファイルの先頭レイヤーをデフォルトレイヤーのプロパティ差し替えに割り当て、
//!    残りを `AddLayer` で追加する（`Document` はデフォルトレイヤーを必ず 1 つ持ち、
//!    export はそれを先頭に列挙するため、この対応は往復で安定する）。
//! 3. エンティティを `AddEntity` で追加する。このとき **ロックは未適用** にして
//!    おく（ロック済みレイヤーへの `AddEntity` はコアが拒否するため、ロックの
//!    適用は全エンティティ投入後に行う）。
//! 4. ロックすべきレイヤーへ `SetLayerProps` でロックを適用する。
//! 5. [`Document::clear_history`] で履歴を空にする（読込直後の Ctrl+Z で再構築
//!    手順が巻き戻るのを防ぐ）。
//!
//! # ジオメトリのバリデーション
//!
//! 非有限座標（NaN/∞）・負や非有限の半径・空ポリラインは
//! [`IoError::InvalidGeometry`] として読込を拒否する。標準 JSON は NaN/∞ を表現
//! できないが、[`import_document`] は JSON を経由せず [`FileDocument`] を直接
//! 受け取る公開 API なので、io 境界としてここで検証する。

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use mcad_core::{Command, Document, Entity, EntityGeom, Layer, LayerId, Rgb, Style};
use mcad_geom::Shape;

use crate::IoError;

/// 現行のファイルフォーマットバージョン。
///
/// - v2（M6）で [`FileEntity::geom`] を [`EntityGeom`]（テキスト・寸法を含む）へ拡張。
/// - v3 でレイヤーに `order`（重ね順）を追加（[`FileLayer`]）。
///
/// 書き出しは常に v3。v1・v2 のファイルは [`from_json`] が後方互換で読み込む
/// （[`FileDocumentV1`] / [`FileDocumentV2`] / [`FileLayerV2`] 参照）。
pub const FORMAT_VERSION: u32 = 3;

/// `.mcad` ファイル全体を表すポータブルな DTO（現行 v3）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileDocument {
    /// フォーマットバージョン。[`FORMAT_VERSION`] 以外は読込を拒否する。
    pub version: u32,
    /// レイヤー一覧。先頭（インデックス 0）が `Document` のデフォルトレイヤーに
    /// 対応する。**配列の並びは重ね順ではない**（重ね順は [`FileLayer::order`]）。
    pub layers: Vec<FileLayer>,
    /// カレントレイヤー（`layers` へのインデックス）。
    pub current_layer: usize,
    /// エンティティ一覧。
    pub entities: Vec<FileEntity>,
}

/// `.mcad` ファイル内のレイヤー 1 枚（現行 v3）。
///
/// core の [`Layer`] とフィールドは一致するが、**ファイル形式を core の型定義から
/// 切り離すために別型として持つ**。core 側にフィールドが増えても、ここへ明示的に
/// 足さない限りファイル形式は変わらない（逆に、ここを変えるならフォーマット
/// バージョンを上げる、という対応が明確になる）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileLayer {
    /// レイヤー名。
    pub name: String,
    /// レイヤー色。
    pub color: Rgb,
    /// 表示するか。
    pub visible: bool,
    /// ロックされているか。
    pub locked: bool,
    /// 重ね順（値が大きいほど手前）。v3 で追加。
    pub order: i32,
}

impl FileLayer {
    /// core の [`Layer`] から DTO を作る。
    fn from_core(layer: &Layer) -> Self {
        Self {
            name: layer.name.clone(),
            color: layer.color,
            visible: layer.visible,
            locked: layer.locked,
            order: layer.order,
        }
    }

    /// DTO から core の [`Layer`] を作る。
    fn to_core(&self) -> Layer {
        Layer {
            name: self.name.clone(),
            color: self.color,
            visible: self.visible,
            locked: self.locked,
            order: self.order,
        }
    }
}

/// `.mcad` ファイル内のエンティティ 1 件（v2 以降で共通）。
///
/// [`mcad_core::Entity`] と違い、所属レイヤーを `LayerId` ではなく
/// [`FileDocument::layers`] へのインデックスで参照する。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileEntity {
    /// 所属レイヤー（[`FileDocument::layers`] へのインデックス）。
    pub layer: usize,
    /// 描画スタイル。
    pub style: Style,
    /// 幾何形状（v2 でテキスト・寸法を含む [`EntityGeom`] へ拡張）。
    pub geom: EntityGeom,
}

/// v2 以前のレイヤー 1 枚を表す凍結 DTO（後方互換読込専用）。
///
/// v1・v2 の時代のレイヤーは 4 フィールドで、`order` を持たなかった。この型は
/// **その当時の形のまま凍結する**（今後 core の [`Layer`] や [`FileLayer`] が
/// 変わっても追随しない）。v1 と v2 でレイヤー構造は同一だったため、両バージョンの
/// 読込がこの 1 つの型を共有する。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct FileLayerV2 {
    name: String,
    color: Rgb,
    visible: bool,
    locked: bool,
}

impl FileLayerV2 {
    /// 凍結 DTO を現行の [`FileLayer`] へ変換する。
    ///
    /// `index` は `layers` 配列内の位置で、これをそのまま `order` に採用する
    /// （モジュール doc「後方互換の変換規則」参照）。
    fn into_v3(self, index: usize) -> FileLayer {
        FileLayer {
            name: self.name,
            color: self.color,
            visible: self.visible,
            locked: self.locked,
            order: i32::try_from(index).unwrap_or(i32::MAX),
        }
    }
}

/// v1・v2 の `layers` 配列を現行の [`FileLayer`] 列へ変換する（`order = インデックス`）。
fn layers_v2_into_v3(layers: Vec<FileLayerV2>) -> Vec<FileLayer> {
    layers
        .into_iter()
        .enumerate()
        .map(|(index, layer)| layer.into_v3(index))
        .collect()
}

/// v2 ファイル全体を表す DTO（後方互換読込専用）。v3 との差はレイヤーが
/// [`FileLayerV2`]（`order` なし）であること。
#[derive(Debug, Clone, PartialEq, Deserialize)]
struct FileDocumentV2 {
    version: u32,
    layers: Vec<FileLayerV2>,
    current_layer: usize,
    entities: Vec<FileEntity>,
}

impl FileDocumentV2 {
    /// v2 DTO を現行の v3 [`FileDocument`] へ変換する（バージョンは
    /// [`FORMAT_VERSION`] に更新）。
    fn into_v3(self) -> FileDocument {
        FileDocument {
            version: FORMAT_VERSION,
            layers: layers_v2_into_v3(self.layers),
            current_layer: self.current_layer,
            entities: self.entities,
        }
    }
}

/// v1 ファイル全体を表す DTO（後方互換読込専用）。v2 との差は [`FileEntityV1`] の
/// `geom` が [`Shape`] であること。[`from_json`] が `version == 1` を検出したときのみ使う。
#[derive(Debug, Clone, PartialEq, Deserialize)]
struct FileDocumentV1 {
    version: u32,
    layers: Vec<FileLayerV2>,
    current_layer: usize,
    entities: Vec<FileEntityV1>,
}

/// v1 ファイル内のエンティティ 1 件（`geom` が [`Shape`]）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
struct FileEntityV1 {
    layer: usize,
    style: Style,
    geom: Shape,
}

impl FileDocumentV1 {
    /// v1 DTO を現行の v3 [`FileDocument`] へ変換する（各 `Shape` を
    /// [`EntityGeom::Shape`] で包み、レイヤーは `order = インデックス` で復元する。
    /// バージョンは [`FORMAT_VERSION`] に更新）。
    fn into_v3(self) -> FileDocument {
        FileDocument {
            version: FORMAT_VERSION,
            layers: layers_v2_into_v3(self.layers),
            current_layer: self.current_layer,
            entities: self
                .entities
                .into_iter()
                .map(|e| FileEntity {
                    layer: e.layer,
                    style: e.style,
                    geom: EntityGeom::Shape(e.geom),
                })
                .collect(),
        }
    }
}

/// ドキュメントをポータブルな [`FileDocument`] へ変換する。
///
/// 生存中のレイヤー・エンティティのみを列挙する。undo/redo 履歴は含めない。
/// `Document` の不変条件（エンティティの所属レイヤーは必ず生存、カレントレイヤーは
/// 必ず生存）により、この変換は失敗しない。
#[must_use]
pub fn export_document(doc: &Document) -> FileDocument {
    let layers: Vec<(LayerId, Layer)> = doc.layers().map(|(id, l)| (id, l.clone())).collect();
    let layer_index = |id: LayerId| -> usize {
        layers
            .iter()
            .position(|(lid, _)| *lid == id)
            .expect("Document invariant: referenced layer must be alive")
    };

    let current_layer = layer_index(doc.current_layer());
    let entities = doc
        .entities()
        .map(|(_, e)| FileEntity {
            layer: layer_index(e.layer),
            style: e.style,
            geom: e.geom.clone(),
        })
        .collect();

    FileDocument {
        version: FORMAT_VERSION,
        layers: layers
            .iter()
            .map(|(_, l)| FileLayer::from_core(l))
            .collect(),
        current_layer,
        entities,
    }
}

/// [`FileDocument`] から [`Document`] を再構築する。
///
/// 再構築の手順・レイヤー対応の規則はモジュール doc を参照。読込後の undo/redo
/// 履歴は空になる。
///
/// # Errors
///
/// 未知バージョン・レイヤーなし・範囲外のレイヤー参照・不正ジオメトリで
/// [`IoError`] を返す。
pub fn import_document(file: &FileDocument) -> Result<Document, IoError> {
    if file.version != FORMAT_VERSION {
        return Err(IoError::UnsupportedVersion(file.version));
    }
    if file.layers.is_empty() {
        return Err(IoError::NoLayers);
    }
    if file.current_layer >= file.layers.len() {
        return Err(IoError::BadCurrentLayer(file.current_layer));
    }
    for (index, entity) in file.entities.iter().enumerate() {
        if entity.layer >= file.layers.len() {
            return Err(IoError::BadLayerRef {
                index,
                layer: entity.layer,
            });
        }
        if let Err(reason) = entity.geom.validate() {
            return Err(IoError::InvalidGeometry { index, reason });
        }
    }

    let mut doc = Document::new();

    // レイヤー投入。ロックは全エンティティ投入後に適用するため、ここでは一旦
    // locked = false で作る。
    let mut layer_ids: Vec<LayerId> = Vec::with_capacity(file.layers.len());
    for (index, layer) in file.layers.iter().enumerate() {
        let unlocked = Layer {
            locked: false,
            ..layer.to_core()
        };
        if index == 0 {
            let id = doc.default_layer();
            doc.apply(Command::SetLayerProps {
                id,
                props: unlocked,
            })?;
            layer_ids.push(id);
        } else {
            let new_ids = doc.apply(Command::AddLayer(unlocked))?;
            layer_ids.push(new_ids.layers[0]);
        }
    }

    doc.apply(Command::SetCurrentLayer(layer_ids[file.current_layer]))?;

    for entity in &file.entities {
        doc.apply(Command::AddEntity(Entity::new(
            entity.geom.clone(),
            layer_ids[entity.layer],
            entity.style,
        )))?;
    }

    // ロックの適用（エンティティ投入後）。
    for (index, layer) in file.layers.iter().enumerate() {
        if layer.locked {
            doc.apply(Command::SetLayerProps {
                id: layer_ids[index],
                props: layer.to_core(),
            })?;
        }
    }

    doc.clear_history();
    Ok(doc)
}

/// ドキュメントを `.mcad` の JSON 文字列（整形済み）へシリアライズする。
///
/// # Errors
///
/// シリアライズ失敗時に [`IoError::Json`] を返す（通常は起きない）。
pub fn to_json(doc: &Document) -> Result<String, IoError> {
    Ok(serde_json::to_string_pretty(&export_document(doc))?)
}

/// `.mcad` の JSON 文字列からドキュメントを再構築する。
///
/// # バージョン互換
///
/// 先頭でフォーマットバージョンだけを読み、バージョンごとに DTO を選ぶ:
///
/// - `1`: [`FileDocumentV1`]（幾何は [`Shape`]、レイヤーは `order` なし）
/// - `2`: [`FileDocumentV2`]（レイヤーは `order` なし）
/// - `3`（[`FORMAT_VERSION`]）: 現行の [`FileDocument`]
///
/// それ以外は [`IoError::UnsupportedVersion`] を返す。書き出しは常に v3。
///
/// v3 として読むファイルのレイヤーに `order` が欠けていれば [`IoError::Json`] で
/// 失敗する（`order` を暗黙の既定値で埋めない = 壊れた v3 ファイルを検出できる）。
///
/// # Errors
///
/// JSON 構文・型の不一致は [`IoError::Json`]、未知バージョンは
/// [`IoError::UnsupportedVersion`]、その他フォーマット上の異常は
/// [`import_document`] と同じ [`IoError`] を返す。
pub fn from_json(json: &str) -> Result<Document, IoError> {
    /// バージョンだけを先読みするための最小 DTO。
    #[derive(Deserialize)]
    struct VersionProbe {
        version: u32,
    }
    let probe: VersionProbe = serde_json::from_str(json)?;
    match probe.version {
        1 => {
            let v1: FileDocumentV1 = serde_json::from_str(json)?;
            import_document(&v1.into_v3())
        }
        2 => {
            let v2: FileDocumentV2 = serde_json::from_str(json)?;
            import_document(&v2.into_v3())
        }
        FORMAT_VERSION => {
            let file: FileDocument = serde_json::from_str(json)?;
            import_document(&file)
        }
        other => Err(IoError::UnsupportedVersion(other)),
    }
}

/// ドキュメントを `.mcad` ファイルへ保存する。
///
/// # Errors
///
/// シリアライズ失敗（[`IoError::Json`]）・書き込み失敗（[`IoError::File`]）を返す。
pub fn save_mcad(doc: &Document, path: impl AsRef<Path>) -> Result<(), IoError> {
    fs::write(path, to_json(doc)?)?;
    Ok(())
}

/// `.mcad` ファイルからドキュメントを読み込む。
///
/// # Errors
///
/// 読み取り失敗（[`IoError::File`]）・JSON/フォーマット異常（[`from_json`] と同じ）を返す。
pub fn load_mcad(path: impl AsRef<Path>) -> Result<Document, IoError> {
    from_json(&fs::read_to_string(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcad_core::Rgb;
    use mcad_geom::{Arc, Circle, LineSeg, Point2, Polyline};

    /// レイヤー2枚（1枚はロック+非表示+色付き）・全 Shape 種・個別スタイル・
    /// 非デフォルトのカレントレイヤーを持つドキュメントを作る。
    fn full_document() -> Document {
        let mut doc = Document::new();
        let default = doc.default_layer();

        let second = doc
            .apply(Command::AddLayer(Layer::new(
                "second",
                Rgb::new(200, 40, 40),
            )))
            .unwrap()
            .layers[0];

        let shapes = [
            Shape::Point(Point2::new(1.0, 2.0)),
            Shape::Line(LineSeg::new(Point2::new(0.0, 0.0), Point2::new(3.0, 4.0))),
            Shape::Circle(Circle::new(Point2::new(-1.0, 5.0), 2.5)),
            Shape::Arc(Arc::new(Point2::new(2.0, 2.0), 1.5, 0.3, 2.8)),
            Shape::Polyline(Polyline::new(
                vec![
                    Point2::new(0.0, 0.0),
                    Point2::new(1.0, 1.0),
                    Point2::new(2.0, 0.0),
                ],
                true,
            )),
        ];
        for (i, shape) in shapes.into_iter().enumerate() {
            let layer = if i % 2 == 0 { default } else { second };
            let style = if i == 0 {
                Style {
                    color: Some(Rgb::new(10, 20, 30)),
                    width: 2.5,
                }
            } else {
                Style::inherited()
            };
            doc.apply(Command::AddEntity(Entity::new(shape, layer, style)))
                .unwrap();
        }

        // second をカレントにし、ロック+非表示にする。
        doc.apply(Command::SetCurrentLayer(second)).unwrap();
        let mut props = doc.layer(second).unwrap().clone();
        props.locked = true;
        props.visible = false;
        // 重ね順も既定値から動かしておき、往復テストが order を含めて検証するようにする。
        props.order = 7;
        doc.apply(Command::SetLayerProps { id: second, props })
            .unwrap();
        doc
    }

    #[test]
    fn round_trip_empty_document() {
        let doc = Document::new();
        let exported = export_document(&doc);
        assert_eq!(exported.version, FORMAT_VERSION);
        assert_eq!(exported.entities.len(), 0);
        assert_eq!(exported.layers.len(), 1);
        assert_eq!(exported.current_layer, 0);

        let imported = import_document(&exported).unwrap();
        assert_eq!(export_document(&imported), exported);
    }

    #[test]
    fn round_trip_preserves_semantic_content() {
        // export → import → export が意味的に一致する（SlotMap の実キー一致は
        // 要求しない。FileDocument 上の比較がその「意味的同値」にあたる）。
        let doc = full_document();
        let exported = export_document(&doc);
        let imported = import_document(&exported).unwrap();
        assert_eq!(export_document(&imported), exported);

        // 内容の抜き取り確認: エンティティ数・レイヤー数・カレントが保たれている。
        assert_eq!(imported.entity_count(), doc.entity_count());
        assert_eq!(imported.layer_count(), doc.layer_count());
        let cur = imported.layer(imported.current_layer()).unwrap();
        assert_eq!(cur.name, "second");
        assert!(cur.locked);
        assert!(!cur.visible);
    }

    #[test]
    fn json_string_round_trip() {
        let doc = full_document();
        let json = to_json(&doc).unwrap();
        let imported = from_json(&json).unwrap();
        assert_eq!(export_document(&imported), export_document(&doc));
    }

    #[test]
    fn import_leaves_history_empty() {
        let imported = import_document(&export_document(&full_document())).unwrap();
        assert!(!imported.can_undo(), "読込直後に undo できてはならない");
        assert!(!imported.can_redo());
    }

    #[test]
    fn locked_layer_entities_survive_import() {
        // ロック済みレイヤー上のエンティティも import で復元される
        // （ロックの適用がエンティティ投入より後であることの検証）。
        let doc = full_document();
        let locked_entities = doc
            .entities()
            .filter(|(_, e)| doc.layer(e.layer).is_some_and(|l| l.locked))
            .count();
        assert!(
            locked_entities > 0,
            "テスト前提: ロック層にエンティティがある"
        );

        let imported = import_document(&export_document(&doc)).unwrap();
        let imported_locked = imported
            .entities()
            .filter(|(_, e)| imported.layer(e.layer).is_some_and(|l| l.locked))
            .count();
        assert_eq!(imported_locked, locked_entities);
    }

    #[test]
    fn unsupported_version_fails() {
        // 現行は v3。未知の将来バージョン（4）は拒否する。
        let mut file = export_document(&Document::new());
        file.version = 4;
        assert!(matches!(
            import_document(&file),
            Err(IoError::UnsupportedVersion(4))
        ));
    }

    #[test]
    fn v3_round_trip_preserves_layer_order() {
        // v3 の往復で重ね順が保たれる（配列順ではなく order フィールドが根拠）。
        let mut doc = Document::new();
        let default = doc.default_layer();

        // デフォルトレイヤーを一番手前（order = 10）へ、後から足す 2 枚をその奥へ置く。
        // → 配列順（デフォルトが先頭）と重ね順が一致しない状態を作る。
        let mut props = doc.layer(default).unwrap().clone();
        props.order = 10;
        doc.apply(Command::SetLayerProps { id: default, props })
            .unwrap();
        for (name, order) in [("mid", 5), ("back", -3)] {
            let mut layer = Layer::new(name, Rgb::new(1, 2, 3));
            layer.order = order;
            doc.apply(Command::AddLayer(layer)).unwrap();
        }

        let expected: Vec<(String, i32)> = doc
            .layers_in_order()
            .into_iter()
            .map(|(_, l)| (l.name.clone(), l.order))
            .collect();
        assert_eq!(
            expected,
            vec![
                ("back".to_owned(), -3),
                ("mid".to_owned(), 5),
                ("0".to_owned(), 10),
            ]
        );

        let loaded = from_json(&to_json(&doc).unwrap()).unwrap();
        let actual: Vec<(String, i32)> = loaded
            .layers_in_order()
            .into_iter()
            .map(|(_, l)| (l.name.clone(), l.order))
            .collect();
        assert_eq!(actual, expected);
        // デフォルトレイヤーの対応（先頭 = デフォルト）も往復で保たれる。
        assert_eq!(loaded.layer(loaded.default_layer()).unwrap().name, "0");
        assert_eq!(export_document(&loaded), export_document(&doc));
    }

    #[test]
    fn v2_file_is_read_with_backward_compat() {
        // v0.7.x 以前の v2 ファイル。レイヤーに order キーが無い。
        // 幾何は v2 以降の EntityGeom（"Shape" ラッパーあり）。
        let v2_json = r#"{
          "version": 2,
          "layers": [
            {"name": "0", "color": {"r": 255, "g": 255, "b": 255}, "visible": true, "locked": false},
            {"name": "middle", "color": {"r": 10, "g": 20, "b": 30}, "visible": false, "locked": true},
            {"name": "top", "color": {"r": 40, "g": 50, "b": 60}, "visible": true, "locked": false}
          ],
          "current_layer": 2,
          "entities": [
            {"layer": 1, "style": {"color": null, "width": 1.0},
             "geom": {"Shape": {"Line": {"a": {"x": 0.0, "y": 0.0}, "b": {"x": 3.0, "y": 4.0}}}}}
          ]
        }"#;

        let doc = from_json(v2_json).expect("v2 ファイルは後方互換で読み込めるべき");

        // order は配列インデックスで復元される（並びはファイル内の元の順序）。
        let ordered: Vec<(String, i32)> = doc
            .layers_in_order()
            .into_iter()
            .map(|(_, l)| (l.name.clone(), l.order))
            .collect();
        assert_eq!(
            ordered,
            vec![
                ("0".to_owned(), 0),
                ("middle".to_owned(), 1),
                ("top".to_owned(), 2),
            ]
        );

        // 他のプロパティ・カレントレイヤー・エンティティも従来どおり復元される。
        let (_, middle) = doc.layers().find(|(_, l)| l.name == "middle").unwrap();
        assert!(middle.locked && !middle.visible);
        assert_eq!(doc.layer(doc.current_layer()).unwrap().name, "top");
        assert_eq!(doc.entity_count(), 1);

        // 読込後の書き出しは常に v3。
        assert_eq!(export_document(&doc).version, FORMAT_VERSION);
    }

    #[test]
    fn v1_file_layer_order_falls_back_to_array_index() {
        // v1 もレイヤーは order なし。v2 と同じ「order = 配列インデックス」規則を通る。
        let v1_json = r#"{
          "version": 1,
          "layers": [
            {"name": "0", "color": {"r": 255, "g": 255, "b": 255}, "visible": true, "locked": false},
            {"name": "front", "color": {"r": 1, "g": 2, "b": 3}, "visible": true, "locked": false}
          ],
          "current_layer": 0,
          "entities": []
        }"#;

        let doc = from_json(v1_json).unwrap();
        let ordered: Vec<(String, i32)> = doc
            .layers_in_order()
            .into_iter()
            .map(|(_, l)| (l.name.clone(), l.order))
            .collect();
        assert_eq!(ordered, vec![("0".to_owned(), 0), ("front".to_owned(), 1)]);
    }

    #[test]
    fn v3_file_without_layer_order_is_rejected() {
        // v3 と自称するファイルのレイヤーに order が無いのは壊れたファイル。
        // 暗黙の既定値で埋めず、JSON エラーとして弾く（core の Layer に
        // serde(default) を付けない理由そのもの）。
        let broken = r#"{
          "version": 3,
          "layers": [
            {"name": "0", "color": {"r": 255, "g": 255, "b": 255}, "visible": true, "locked": false}
          ],
          "current_layer": 0,
          "entities": []
        }"#;
        assert!(matches!(from_json(broken), Err(IoError::Json(_))));
    }

    #[test]
    fn v1_file_is_read_with_backward_compat() {
        // v0.5.0 以前の v1 ファイル。幾何は Shape が直下に来る（v2 の "Shape" ラッパーなし）。
        let v1_json = r#"{
          "version": 1,
          "layers": [
            {"name": "0", "color": {"r": 255, "g": 255, "b": 255}, "visible": true, "locked": false}
          ],
          "current_layer": 0,
          "entities": [
            {"layer": 0, "style": {"color": null, "width": 1.0},
             "geom": {"Line": {"a": {"x": 0.0, "y": 0.0}, "b": {"x": 3.0, "y": 4.0}}}}
          ]
        }"#;

        let doc = from_json(v1_json).expect("v1 ファイルは後方互換で読み込めるべき");
        assert_eq!(doc.entity_count(), 1);
        let (_, entity) = doc.entities().next().unwrap();
        // v1 の Shape は EntityGeom::Shape へ包まれて受理される。
        assert_eq!(
            entity.geom,
            EntityGeom::Shape(Shape::Line(LineSeg::new(
                Point2::new(0.0, 0.0),
                Point2::new(3.0, 4.0),
            )))
        );

        // 読込後の書き出しは常に v2。往復しても内容が保たれる。
        let reexported = export_document(&doc);
        assert_eq!(reexported.version, FORMAT_VERSION);
        let reimported = import_document(&reexported).unwrap();
        assert_eq!(export_document(&reimported), reexported);
    }

    #[test]
    fn text_entity_round_trips_with_cjk_content() {
        use mcad_core::TextGeom;
        // M6: v2 フォーマットで Text エンティティ（CJK 文字列含む）が JSON 往復で保たれる。
        let mut doc = Document::new();
        let layer = doc.current_layer();
        let text = EntityGeom::Text(TextGeom {
            anchor: Point2::new(1.5, -2.5),
            content: "日本語ABC 123".to_owned(),
            height: 3.5,
            angle: 0.75,
        });
        doc.apply(Command::AddEntity(Entity::new(
            text.clone(),
            layer,
            Style::inherited(),
        )))
        .unwrap();

        // JSON 文字列を経由しても意味的に一致する（UTF-8 の CJK がそのまま保たれる）。
        let json = to_json(&doc).unwrap();
        assert!(json.contains("日本語"), "JSON に CJK 文字列が含まれるべき");
        let loaded = from_json(&json).unwrap();
        assert_eq!(export_document(&loaded), export_document(&doc));

        // 幾何が Text として復元されている。
        let (_, entity) = loaded.entities().next().unwrap();
        assert_eq!(entity.geom, text);
    }

    #[test]
    fn unknown_version_via_json_is_rejected() {
        // from_json はバージョン先読みで未知バージョンを弾く（v1/v2 以外）。
        let json = r#"{"version": 99, "layers": [], "current_layer": 0, "entities": []}"#;
        assert!(matches!(
            from_json(json),
            Err(IoError::UnsupportedVersion(99))
        ));
    }

    #[test]
    fn no_layers_fails() {
        let mut file = export_document(&Document::new());
        file.layers.clear();
        assert!(matches!(import_document(&file), Err(IoError::NoLayers)));
    }

    #[test]
    fn bad_current_layer_fails() {
        let mut file = export_document(&Document::new());
        file.current_layer = 5;
        assert!(matches!(
            import_document(&file),
            Err(IoError::BadCurrentLayer(5))
        ));
    }

    #[test]
    fn bad_layer_ref_fails() {
        let mut file = export_document(&Document::new());
        file.entities.push(FileEntity {
            layer: 9,
            style: Style::inherited(),
            geom: EntityGeom::Shape(Shape::Point(Point2::new(0.0, 0.0))),
        });
        assert!(matches!(
            import_document(&file),
            Err(IoError::BadLayerRef { index: 0, layer: 9 })
        ));
    }

    #[test]
    fn invalid_geometry_fails() {
        let cases: Vec<Shape> = vec![
            Shape::Point(Point2::new(f64::NAN, 0.0)),
            Shape::Circle(Circle::new(Point2::new(0.0, 0.0), -1.0)),
            Shape::Circle(Circle::new(Point2::new(0.0, 0.0), f64::INFINITY)),
            Shape::Arc(Arc::new(Point2::new(0.0, 0.0), 1.0, f64::NAN, 1.0)),
            Shape::Polyline(Polyline::new(vec![], false)),
        ];
        for geom in cases {
            let mut file = export_document(&Document::new());
            file.entities.push(FileEntity {
                layer: 0,
                style: Style::inherited(),
                geom: EntityGeom::Shape(geom.clone()),
            });
            assert!(
                matches!(
                    import_document(&file),
                    Err(IoError::InvalidGeometry { index: 0, .. })
                ),
                "should reject: {geom:?}"
            );
        }
    }

    #[test]
    fn malformed_json_fails() {
        assert!(matches!(from_json("{ not json"), Err(IoError::Json(_))));
        // 型は正しい JSON だが必須フィールドがない。
        assert!(matches!(from_json("{}"), Err(IoError::Json(_))));
    }

    #[test]
    fn save_and_load_file() {
        let dir = std::env::temp_dir().join("mcad-io-test");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("roundtrip.mcad");

        let doc = full_document();
        save_mcad(&doc, &path).unwrap();
        let loaded = load_mcad(&path).unwrap();
        assert_eq!(export_document(&loaded), export_document(&doc));

        fs::remove_file(&path).ok();
    }
}
