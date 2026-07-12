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
//! - 生存中のレイヤー・エンティティのみを列挙する。undo/redo 履歴・墓標は保存しない。
//! - `version` フィールド必須（現行は `1`）。未知バージョンは読込を拒否する。
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

use mcad_core::{Command, Document, Entity, Layer, LayerId, Style};
use mcad_geom::{Point2, Shape};

use crate::IoError;

/// 現行のファイルフォーマットバージョン。
pub const FORMAT_VERSION: u32 = 1;

/// `.mcad` ファイル全体を表すポータブルな DTO。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileDocument {
    /// フォーマットバージョン。[`FORMAT_VERSION`] 以外は読込を拒否する。
    pub version: u32,
    /// レイヤー一覧。先頭（インデックス 0）が `Document` のデフォルトレイヤーに
    /// 対応する。
    pub layers: Vec<Layer>,
    /// カレントレイヤー（`layers` へのインデックス）。
    pub current_layer: usize,
    /// エンティティ一覧。
    pub entities: Vec<FileEntity>,
}

/// `.mcad` ファイル内のエンティティ 1 件。
///
/// [`mcad_core::Entity`] と違い、所属レイヤーを `LayerId` ではなく
/// [`FileDocument::layers`] へのインデックスで参照する。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileEntity {
    /// 所属レイヤー（[`FileDocument::layers`] へのインデックス）。
    pub layer: usize,
    /// 描画スタイル。
    pub style: Style,
    /// 幾何形状。
    pub geom: Shape,
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
        layers: layers.into_iter().map(|(_, l)| l).collect(),
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
        if let Err(reason) = validate_shape(&entity.geom) {
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
            ..layer.clone()
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
                props: layer.clone(),
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
/// # Errors
///
/// JSON 構文・型の不一致は [`IoError::Json`]、フォーマット上の異常は
/// [`import_document`] と同じ [`IoError`] を返す。
pub fn from_json(json: &str) -> Result<Document, IoError> {
    let file: FileDocument = serde_json::from_str(json)?;
    import_document(&file)
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

/// ジオメトリが io 境界の妥当性条件を満たすか検証する。
///
/// 条件: すべての座標・半径・角度が有限、半径は非負、ポリラインは頂点 1 つ以上。
/// 半径 0 の円や長さ 0 の線分は退化しているが描画・計算を壊さないため許容する
/// （作図ツールでも同一点クリックで作れるものを io だけ拒否しない）。
///
/// `pub(crate)`: DXF import（[`crate::dxf_file`]）でも同じ判定基準を再利用する
/// ため（ロジックの重複・divergence を避ける）。
pub(crate) fn validate_shape(shape: &Shape) -> Result<(), String> {
    let finite = |p: Point2| p.x.is_finite() && p.y.is_finite();
    match shape {
        Shape::Point(p) => {
            if !finite(*p) {
                return Err("non-finite point coordinates".into());
            }
        }
        Shape::Line(l) => {
            if !finite(l.a) || !finite(l.b) {
                return Err("non-finite line coordinates".into());
            }
        }
        Shape::Circle(c) => {
            if !finite(c.center) {
                return Err("non-finite circle center".into());
            }
            if !c.radius.is_finite() || c.radius < 0.0 {
                return Err(format!("invalid circle radius: {}", c.radius));
            }
        }
        Shape::Arc(a) => {
            if !finite(a.center) {
                return Err("non-finite arc center".into());
            }
            if !a.radius.is_finite() || a.radius < 0.0 {
                return Err(format!("invalid arc radius: {}", a.radius));
            }
            if !a.start_angle.is_finite() || !a.end_angle.is_finite() {
                return Err("non-finite arc angles".into());
            }
        }
        Shape::Polyline(pl) => {
            if pl.vertices.is_empty() {
                return Err("empty polyline".into());
            }
            if !pl.vertices.iter().all(|v| finite(*v)) {
                return Err("non-finite polyline vertex".into());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcad_core::Rgb;
    use mcad_geom::{Arc, Circle, LineSeg, Polyline};

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
        let mut file = export_document(&Document::new());
        file.version = 2;
        assert!(matches!(
            import_document(&file),
            Err(IoError::UnsupportedVersion(2))
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
            geom: Shape::Point(Point2::new(0.0, 0.0)),
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
                geom: geom.clone(),
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
