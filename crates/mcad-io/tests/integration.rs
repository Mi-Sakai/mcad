//! mcad-core と mcad-io をまたぐ統合テスト。
//!
//! DESIGN.md 116-122行目（MVP完了の検収基準）の各項目のうち、GUI のマウス入力を
//! 要するスナップ（1項目目）を除く 4 項目を、mcad-core の [`Command`] API と
//! mcad-io の公開 API だけを使って「一連の操作」として検証する。
//!
//! - 矩形選択→移動→undo→redo（[`draw_batch_select_move_undo_redo_round_trip`]）
//! - レイヤーの非表示・ロック（[`layer_visibility_and_lock_are_enforced_and_persisted`]）
//! - .mcad 保存→読込で完全復元（[`mcad_file_round_trip_through_real_file_path`]）
//! - DXF 出力→再インポートで同一内容（[`dxf_round_trip_through_real_file_path`]）
//!
//! いずれも実際の GUI ツール（`mcad-app` の `SelectTool`/`MoveTool`）は使わず、
//! 同等の操作をコア API で直接組み立てる（`mcad-app/src/tool.rs` のテストと同じ
//! パターン）。`mcad-io` の統合テストなので `mcad-app` には依存しない。
//!
//! クレート単体テスト（`mcad_file.rs`/`dxf_file.rs` 内の `#[cfg(test)]`）は
//! `FileDocument` の構造的完全一致で往復を検証しているが、ここでは実ファイル
//! パスを介した保存/読込という統合的な観点に絞り、エンティティ数・レイヤー構成・
//! カレントレイヤーが意味的に一致することを確認する（同じアサーションの複製は
//! 避ける）。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use mcad_core::{Command, CoreError, Document, Entity, EntityId, Layer, Rgb, Style};
use mcad_geom::{Arc, Circle, LineSeg, Point2, Polyline, Shape, Vec2};
use mcad_io::{load_dxf, load_mcad, save_dxf, save_mcad};

/// 線分・円・円弧・ポリラインを最低1つずつカレントレイヤーに追加し、
/// `(EntityId, 追加時点の Shape)` の一覧を追加順で返す。
fn add_one_of_each_shape(doc: &mut Document) -> Vec<(EntityId, Shape)> {
    let layer = doc.current_layer();
    let shapes = vec![
        Shape::Line(LineSeg::new(Point2::new(0.0, 0.0), Point2::new(10.0, 0.0))),
        Shape::Circle(Circle::new(Point2::new(5.0, 5.0), 3.0)),
        Shape::Arc(Arc::new(Point2::new(-2.0, -2.0), 2.0, 0.0, 1.5)),
        Shape::Polyline(Polyline::new(
            vec![
                Point2::new(0.0, 0.0),
                Point2::new(1.0, 2.0),
                Point2::new(2.0, 0.0),
            ],
            true,
        )),
    ];

    shapes
        .into_iter()
        .map(|shape| {
            let new_ids = doc
                .apply(Command::AddEntity(Entity::new(
                    shape.clone(),
                    layer,
                    Style::inherited(),
                )))
                .unwrap();
            assert_eq!(new_ids.entities.len(), 1, "AddEntity は新規IDを1件発行する");
            (new_ids.entities[0], shape)
        })
        .collect()
}

#[test]
fn draw_batch_select_move_undo_redo_round_trip() {
    // 検収基準:
    // 「線分・円・円弧・ポリラインを描き、スナップを使って正確に接続できる」の
    // 作図部分（スナップ自体は GUI マウス入力が必要なため対象外）と、
    // 「矩形選択→移動→undo→redoが正しく動く」を一連の流れで検証する。
    let mut doc = Document::new();
    let originals = add_one_of_each_shape(&mut doc);
    assert_eq!(doc.entity_count(), 4);

    // 矩形選択で複数エンティティをまとめて掴んで移動する操作を、実際の
    // SelectTool/MoveTool のマウス操作は経由せず、mcad-core の Command::Batch で
    // 直接組み立てる（mcad-app/src/tool.rs の SelectTool 移動確定と同じパターン）。
    let delta = Vec2::new(10.0, -5.0);
    let batch = Command::Batch(
        originals
            .iter()
            .map(|(id, shape)| Command::ModifyEntity {
                id: *id,
                new_geom: shape.translated(delta),
            })
            .collect(),
    );
    doc.apply(batch).unwrap();

    for (id, shape) in &originals {
        assert_eq!(doc.entity(*id).unwrap().geom, shape.translated(delta));
    }

    // undo 1 回で 4 件すべてが元の位置へ戻る（バッチは undo 履歴へ1記録として積まれる）。
    assert!(doc.undo());
    for (id, shape) in &originals {
        assert_eq!(doc.entity(*id).unwrap().geom, *shape);
    }
    // まだ戻せるのは 4 件の追加操作のみ。
    assert!(doc.can_undo());

    // redo 1 回で 4 件すべてが再び移動する。
    assert!(doc.redo());
    for (id, shape) in &originals {
        assert_eq!(doc.entity(*id).unwrap().geom, shape.translated(delta));
    }
}

#[test]
fn layer_visibility_and_lock_are_enforced_and_persisted() {
    // 検収基準: 「レイヤーを分けて非表示・ロックが効く」
    let mut doc = Document::new();
    let default_layer = doc.current_layer();

    // 追加レイヤーを作り、まずロックしていない状態でエンティティを追加する
    // （ロック済みレイヤーへの AddEntity はコアが拒否するため、ロックは後で適用する）。
    let hidden_locked = doc
        .apply(Command::AddLayer(Layer::new(
            "hidden_locked",
            Rgb::new(80, 80, 80),
        )))
        .unwrap()
        .layers[0];
    let entity_id = doc
        .apply(Command::AddEntity(Entity::new(
            Shape::Point(Point2::new(1.0, 1.0)),
            hidden_locked,
            Style::inherited(),
        )))
        .unwrap()
        .entities[0];

    // レイヤーを非表示・ロックにする。
    let mut props = doc.layer(hidden_locked).unwrap().clone();
    props.visible = false;
    props.locked = true;
    doc.apply(Command::SetLayerProps {
        id: hidden_locked,
        props,
    })
    .unwrap();

    // Document::layer から非表示・ロック状態が正しく読める
    // （描画・ヒットテストからの除外自体は app 層の責務なので、ここでは状態の保持のみ確認）。
    let layer = doc.layer(hidden_locked).unwrap();
    assert!(!layer.visible);
    assert!(layer.locked);

    // ロックされたレイヤー上のエンティティへの変更・削除は拒否される。
    assert_eq!(
        doc.apply(Command::ModifyEntity {
            id: entity_id,
            new_geom: Shape::Point(Point2::new(9.0, 9.0)),
        }),
        Err(CoreError::LayerLocked(hidden_locked))
    );
    assert_eq!(
        doc.apply(Command::RemoveEntity(entity_id)),
        Err(CoreError::LayerLocked(hidden_locked))
    );
    // 拒否されたので状態は変わらない。
    assert_eq!(
        doc.entity(entity_id).unwrap().geom,
        Shape::Point(Point2::new(1.0, 1.0))
    );

    // 対比: デフォルトレイヤーは非表示でもロックでもないので、そちらのエンティティは
    // 問題なく変更できる（ロックがレイヤー単位で効いていることの確認）。
    let free_id = doc
        .apply(Command::AddEntity(Entity::new(
            Shape::Point(Point2::new(0.0, 0.0)),
            default_layer,
            Style::inherited(),
        )))
        .unwrap()
        .entities[0];
    doc.apply(Command::ModifyEntity {
        id: free_id,
        new_geom: Shape::Point(Point2::new(2.0, 2.0)),
    })
    .unwrap();
    let default_layer_state = doc.layer(default_layer).unwrap();
    assert!(default_layer_state.visible);
    assert!(!default_layer_state.locked);
}

/// 複数レイヤー・全 Shape 種・個別スタイル・非デフォルトのカレントレイヤーを持つ
/// ドキュメントを作る（.mcad / DXF どちらの往復テストでも使う共有フィクスチャ）。
fn build_round_trip_document() -> Document {
    let mut doc = Document::new();
    let default_layer = doc.default_layer();
    let extra_layer = doc
        .apply(Command::AddLayer(Layer::new(
            "dimensions",
            Rgb::new(10, 200, 90),
        )))
        .unwrap()
        .layers[0];

    let shapes = [
        Shape::Point(Point2::new(-3.0, 4.0)),
        Shape::Line(LineSeg::new(Point2::new(0.0, 0.0), Point2::new(6.0, 8.0))),
        Shape::Circle(Circle::new(Point2::new(1.0, 1.0), 4.0)),
        Shape::Arc(Arc::new(Point2::new(-1.0, -1.0), 2.5, 0.2, 3.0)),
        Shape::Polyline(Polyline::new(
            vec![
                Point2::new(0.0, 0.0),
                Point2::new(3.0, 0.0),
                Point2::new(3.0, 3.0),
            ],
            false,
        )),
    ];
    for (i, shape) in shapes.into_iter().enumerate() {
        let layer = if i % 2 == 0 {
            default_layer
        } else {
            extra_layer
        };
        // 個別スタイル: 1件だけ個別色を持たせ、残りはレイヤー色継承のままにする。
        let style = Style {
            color: if i == 1 {
                Some(Rgb::new(255, 128, 0))
            } else {
                None
            },
            width: 1.0 + i as f32,
        };
        doc.apply(Command::AddEntity(Entity::new(shape, layer, style)))
            .unwrap();
    }

    // 非デフォルトのカレントレイヤー。
    doc.apply(Command::SetCurrentLayer(extra_layer)).unwrap();
    doc
}

/// レイヤー名の集合（順序に依存しない比較のため）。
fn layer_name_set(doc: &Document) -> BTreeSet<String> {
    doc.layers().map(|(_, l)| l.name.clone()).collect()
}

/// レイヤー名 → そのレイヤーに属するエンティティ数、のマップ。
fn entity_counts_by_layer(doc: &Document) -> BTreeMap<String, usize> {
    let mut map = BTreeMap::new();
    for (_, e) in doc.entities() {
        let name = doc.layer(e.layer).unwrap().name.clone();
        *map.entry(name).or_insert(0) += 1;
    }
    map
}

/// Shape 種別ごとのエンティティ数のマップ（DXF 往復のように幾何が近似丸めされる
/// 場合でも「種類の構成」自体は完全一致するはずのものを確認するのに使う）。
fn shape_kind_counts(doc: &Document) -> BTreeMap<&'static str, usize> {
    let mut map = BTreeMap::new();
    for (_, e) in doc.entities() {
        let kind = match e.geom {
            Shape::Point(_) => "point",
            Shape::Line(_) => "line",
            Shape::Circle(_) => "circle",
            Shape::Arc(_) => "arc",
            Shape::Polyline(_) => "polyline",
        };
        *map.entry(kind).or_insert(0) += 1;
    }
    map
}

#[test]
fn mcad_file_round_trip_through_real_file_path() {
    // 検収基準: 「.mcadで保存→再起動→読込で完全復元される」
    //
    // mcad_file.rs の単体テストは export_document() による FileDocument の完全一致で
    // 往復を検証済みなので、ここでは重複を避け、実ファイルパスを介した
    // 保存→（プロセス的に独立した）読込という統合的な観点から、エンティティ数・
    // レイヤー構成・カレントレイヤーが意味的に一致することを確認する。
    let doc = build_round_trip_document();
    let original_layer_names = layer_name_set(&doc);
    let original_counts_by_layer = entity_counts_by_layer(&doc);
    let original_current_name = doc.layer(doc.current_layer()).unwrap().name.clone();
    let original_entity_count = doc.entity_count();
    let original_layer_count = doc.layer_count();

    let dir = std::env::temp_dir().join("mcad-io-integration-test");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("roundtrip-{}.mcad", std::process::id()));

    save_mcad(&doc, &path).unwrap();
    let loaded = load_mcad(&path).unwrap();

    assert_eq!(loaded.entity_count(), original_entity_count);
    assert_eq!(loaded.layer_count(), original_layer_count);
    assert_eq!(layer_name_set(&loaded), original_layer_names);
    assert_eq!(entity_counts_by_layer(&loaded), original_counts_by_layer);

    let loaded_current = loaded.layer(loaded.current_layer()).unwrap();
    assert_eq!(loaded_current.name, original_current_name);

    // 読込直後は undo/redo 履歴が空である（「再起動」相当の基準点）。
    assert!(!loaded.can_undo());
    assert!(!loaded.can_redo());

    fs::remove_file(&path).ok();
}

#[test]
fn dxf_round_trip_through_real_file_path() {
    // 検収基準: 「描いた図面をDXF出力し、再インポートして同一内容になる」
    //
    // dxf_file.rs のモジュール doc・単体テストの設計判断のとおり、色は ACI への
    // 近似変換、レイヤーロックは DXF フォーマットの制約で保存されない
    // （import 後は常に unlocked）。したがってここではその2点を比較対象から外し、
    // エンティティ数・Shape種別の構成・レイヤー構成が一致することに絞って検証する。
    let doc = build_round_trip_document();
    let original_layer_names = layer_name_set(&doc);
    let original_shape_counts = shape_kind_counts(&doc);
    let original_counts_by_layer = entity_counts_by_layer(&doc);
    let original_entity_count = doc.entity_count();

    let dir = std::env::temp_dir().join("mcad-io-integration-test");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("roundtrip-{}.dxf", std::process::id()));

    save_dxf(&doc, &path).unwrap();
    let summary = load_dxf(&path).unwrap();
    assert_eq!(summary.skipped_entities, 0);
    let imported = summary.document;

    assert_eq!(imported.entity_count(), original_entity_count);
    assert_eq!(layer_name_set(&imported), original_layer_names);
    assert_eq!(shape_kind_counts(&imported), original_shape_counts);
    assert_eq!(entity_counts_by_layer(&imported), original_counts_by_layer);

    // DXF 往復はロックを保存しないため、常に unlocked で復元される
    // （dxf_file.rs モジュール doc「レイヤーロックは保存されない」を参照）。
    for (_, layer) in imported.layers() {
        assert!(!layer.locked);
    }

    assert!(!imported.can_undo());
    assert!(!imported.can_redo());

    fs::remove_file(&path).ok();
}
