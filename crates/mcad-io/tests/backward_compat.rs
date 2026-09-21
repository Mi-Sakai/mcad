//! `.mcad` v1〜v7 の後方互換 fixture テスト（M11 タスク65、v7 はタスク68 で追加）。
//!
//! # fixture について
//!
//! `tests/fixtures/v{1..7}.mcad` は各バージョンのスキーマが表現できる要素を
//! 一通り含む実ファイル（レイヤー2枚・複数ジオメトリ種・その版で表現可能な
//! メタデータ）。各版のスキーマの一次情報は
//! `crates/mcad-io/src/mcad_file.rs` のモジュール doc と DTO 定義
//! （`FileDocument` / `FileLayer` / `EntityGeomV5` / `EntityGeomV6` / `DimLinearV6` 等）。
//!
//! 「不正な JSON の拒否」「偽装 `Table` の拒否」「v6 に `direction` を混ぜた
//! ファイルの拒否」のような**壊れた入力**のテストはここへは置かない
//! （`mcad_file.rs` の `#[cfg(test)]` に残す）。fixture ディレクトリは
//! 「正規のサンプル」という位置づけを保つため。
//!
//! # v8 を追加するとき
//!
//! `.mcad` フォーマットが v8 へ上がったら:
//!
//! 1. 現行版の mcad で保存したファイル（または本ファイルと同じ流儀で手作りした
//!    JSON）を `tests/fixtures/v8.mcad` として追加する。新フィールドを含めること。
//! 2. 本ファイルに `v8_*` の名前で新しいバージョンの検証項目
//!    （新フィールドの既定値補完・ラウンドトリップ）を1ケース足す。
//! 3. `all_fixtures_declare_their_own_version_and_reexport_as_current` は
//!    `1..=FORMAT_VERSION` を回すだけなので、`FORMAT_VERSION` を上げれば自動で対象に入る。
//! 4. 旧版（このときは v7）が変わらず読めることは、既存の `v7_*` テストが
//!    そのまま回帰網として働く。

use std::fs;
use std::path::PathBuf;

use mcad_core::{ArrowPlacement, DimDirection, EntityGeom, Linetype};
use mcad_geom::{ArrowKind, DimSymbol};
use mcad_io::{FORMAT_VERSION, export_document, from_json};

/// `tests/fixtures/v{version}.mcad` の内容を読む。
fn fixture(version: u32) -> String {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests/fixtures");
    path.push(format!("v{version}.mcad"));
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("fixture {path:?} を読めない: {e}"))
}

/// エンティティの `EntityGeom` バリアント名をカウントするための小さな分類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum GeomKind {
    Shape,
    Text,
    DimLinear,
    DimRadial,
    DimDiameter,
    Table,
    /// `EntityGeom` が `#[non_exhaustive]` なので、テスト時点で知らないバリアントを
    /// 網羅させるための受け皿（このテストが失敗しなければ知らないバリアントは無い）。
    Unknown,
}

fn geom_kind(geom: &EntityGeom) -> GeomKind {
    match geom {
        EntityGeom::Shape(_) => GeomKind::Shape,
        EntityGeom::Text(_) => GeomKind::Text,
        EntityGeom::DimLinear(_) => GeomKind::DimLinear,
        EntityGeom::DimRadial(_) => GeomKind::DimRadial,
        EntityGeom::DimDiameter(_) => GeomKind::DimDiameter,
        EntityGeom::Table(_) => GeomKind::Table,
        _ => GeomKind::Unknown,
    }
}

// ---------------------------------------------------------------------
// v1
// ---------------------------------------------------------------------

#[test]
fn v1_fixture_loads_with_expected_content() {
    // v1: 幾何は Shape のみ、レイヤーに order なし（配列インデックスを採用）。
    let summary = from_json(&fixture(1)).expect("v1 fixture は読めるべき");
    let doc = summary.document;

    assert_eq!(doc.layer_count(), 2);
    assert_eq!(doc.entity_count(), 3);
    assert!(
        doc.entities()
            .all(|(_, e)| geom_kind(&e.geom) == GeomKind::Shape),
        "v1 の幾何はすべて Shape であるべき"
    );

    // order は配列インデックスで復元される。
    let ordered: Vec<(String, i32)> = doc
        .layers_in_order()
        .into_iter()
        .map(|(_, l)| (l.name.clone(), l.order))
        .collect();
    assert_eq!(ordered, vec![("0".to_owned(), 0), ("parts".to_owned(), 1)]);

    // v1〜v4 は矢の長さが凍結された旧既定 3.0mm へ補完される。
    assert_eq!(doc.dim_style().arrow_len_mm, 3.0);
    assert_eq!(doc.dim_style().arrow_kind, ArrowKind::ClosedFilled);

    // ラウンドトリップ: 読込 → 保存 → 再読込で内容が保たれる。
    let exported = export_document(&doc);
    assert_eq!(exported.version, FORMAT_VERSION);
    let json = mcad_io::to_json(&doc).unwrap();
    let reloaded = from_json(&json).unwrap().document;
    assert_eq!(export_document(&reloaded), exported);
}

// ---------------------------------------------------------------------
// v2
// ---------------------------------------------------------------------

#[test]
fn v2_fixture_loads_with_expected_content() {
    // v2: 幾何が EntityGeom（Shape/Text）へ拡張。レイヤーはまだ order なし。
    let summary = from_json(&fixture(2)).expect("v2 fixture は読めるべき");
    let doc = summary.document;

    assert_eq!(doc.layer_count(), 2);
    assert_eq!(doc.entity_count(), 4);

    let mut kinds: Vec<GeomKind> = doc.entities().map(|(_, e)| geom_kind(&e.geom)).collect();
    kinds.sort();
    assert_eq!(
        kinds,
        vec![
            GeomKind::Shape,
            GeomKind::Shape,
            GeomKind::Shape,
            GeomKind::Text
        ]
    );

    // Text の CJK 内容が保たれている。
    let text = doc
        .entities()
        .find_map(|(_, e)| match &e.geom {
            EntityGeom::Text(t) => Some(t.content.clone()),
            _ => None,
        })
        .expect("Text エンティティがあるはず");
    assert_eq!(text, "日本語ABC");

    // order は配列インデックス採用。
    let ordered: Vec<(String, i32)> = doc
        .layers_in_order()
        .into_iter()
        .map(|(_, l)| (l.name.clone(), l.order))
        .collect();
    assert_eq!(ordered, vec![("0".to_owned(), 0), ("parts".to_owned(), 1)]);

    assert_eq!(doc.dim_style().arrow_len_mm, 3.0);

    let exported = export_document(&doc);
    let reloaded = from_json(&mcad_io::to_json(&doc).unwrap())
        .unwrap()
        .document;
    assert_eq!(export_document(&reloaded), exported);
}

// ---------------------------------------------------------------------
// v3
// ---------------------------------------------------------------------

#[test]
fn v3_fixture_loads_with_expected_content() {
    // v3: レイヤーに order（重ね順）を追加。fixture は配列順と重ね順をわざと
    // ずらしてある（0番目が order=1、1番目が order=0）。
    let summary = from_json(&fixture(3)).expect("v3 fixture は読めるべき");
    let doc = summary.document;

    assert_eq!(doc.layer_count(), 2);
    assert_eq!(doc.entity_count(), 4);

    let ordered: Vec<(String, i32)> = doc
        .layers_in_order()
        .into_iter()
        .map(|(_, l)| (l.name.clone(), l.order))
        .collect();
    // order 昇順: parts(0) → "0"(1)。配列順とは逆転している。
    assert_eq!(ordered, vec![("parts".to_owned(), 0), ("0".to_owned(), 1)]);

    assert_eq!(doc.dim_style().arrow_len_mm, 3.0);

    let exported = export_document(&doc);
    let reloaded = from_json(&mcad_io::to_json(&doc).unwrap())
        .unwrap()
        .document;
    assert_eq!(export_document(&reloaded), exported);
}

// ---------------------------------------------------------------------
// v4
// ---------------------------------------------------------------------

#[test]
fn v4_fixture_loads_with_expected_content() {
    // v4: 図面メタデータ(sheet)・レイヤーの linetype/width_mm・寸法(annotation なし)を追加。
    let summary = from_json(&fixture(4)).expect("v4 fixture は読めるべき");
    let doc = summary.document;

    assert_eq!(doc.layer_count(), 2);
    assert_eq!(doc.entity_count(), 3);
    assert_eq!(summary.clamped_widths, 0);

    // sheet メタデータが保たれている。
    assert_eq!(doc.sheet().fields.drawing_number, "MCAD-V4-001");
    assert_eq!(doc.sheet().fields.author, "almaz");
    assert!(doc.sheet().frame_visible);

    // レイヤーの線種・線幅。
    let (_, parts) = doc.layers().find(|(_, l)| l.name == "parts").unwrap();
    assert_eq!(parts.linetype, Linetype::DashDot);
    assert_eq!(parts.width_mm.mm(), 0.7);

    // 寸法は annotation キーを持たないので無注記へ補完される。
    let dim = doc
        .entities()
        .find_map(|(_, e)| match &e.geom {
            EntityGeom::DimLinear(d) => Some(d.clone()),
            _ => None,
        })
        .expect("DimLinear があるはず");
    assert!(dim.annotation.is_unannotated());

    // v1〜v4 は矢の長さが凍結された旧既定 3.0mm。
    assert_eq!(doc.dim_style().arrow_len_mm, 3.0);
    assert_eq!(doc.dim_style().arrow_kind, ArrowKind::ClosedFilled);

    let exported = export_document(&doc);
    assert_eq!(exported.version, FORMAT_VERSION);
    let reloaded = from_json(&mcad_io::to_json(&doc).unwrap())
        .unwrap()
        .document;
    assert_eq!(export_document(&reloaded), exported);
}

// ---------------------------------------------------------------------
// v5
// ---------------------------------------------------------------------

#[test]
fn v5_fixture_loads_with_expected_content() {
    // v5: 寸法注記(annotation)・文書単位の dim_style を永続化。arrow_kind キーは
    // まだ持たないので ClosedFilled へ補完される。
    let summary = from_json(&fixture(5)).expect("v5 fixture は読めるべき");
    let doc = summary.document;

    assert_eq!(doc.layer_count(), 2);
    assert_eq!(doc.entity_count(), 4);

    let mut kinds: Vec<GeomKind> = doc.entities().map(|(_, e)| geom_kind(&e.geom)).collect();
    kinds.sort();
    assert_eq!(
        kinds,
        vec![
            GeomKind::Shape,
            GeomKind::DimLinear,
            GeomKind::DimRadial,
            GeomKind::DimDiameter,
        ]
    );

    // dim_style はファイルの値をそのまま持つ(v1〜v4 の凍結既定ではない)。
    assert_eq!(doc.dim_style().text_height_mm, 4.0);
    assert_eq!(doc.dim_style().arrow_len_mm, 3.0);
    // arrow_kind キーが無いので ClosedFilled へ既定値補完。
    assert_eq!(doc.dim_style().arrow_kind, ArrowKind::ClosedFilled);

    // 注記の内容を抜き取り確認。
    let linear = doc
        .entities()
        .find_map(|(_, e)| match &e.geom {
            EntityGeom::DimLinear(d) => Some(d.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(linear.annotation.symbol, Some(DimSymbol::Diameter));
    assert_eq!(linear.annotation.decimals_override, Some(1));
    assert_eq!(linear.annotation.arrow_placement, ArrowPlacement::Outside);

    let diameter = doc
        .entities()
        .find_map(|(_, e)| match &e.geom {
            EntityGeom::DimDiameter(d) => Some(d.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(diameter.annotation.value_override, Some("5-10".to_owned()));
    assert_eq!(diameter.annotation.arrow_placement, ArrowPlacement::Inside);

    let exported = export_document(&doc);
    assert_eq!(exported.version, FORMAT_VERSION);
    let reloaded = from_json(&mcad_io::to_json(&doc).unwrap())
        .unwrap()
        .document;
    assert_eq!(export_document(&reloaded), exported);
}

// ---------------------------------------------------------------------
// v6
// ---------------------------------------------------------------------

#[test]
fn v6_fixture_loads_with_expected_content() {
    // v6: 汎用テーブル・矢先種別を追加。
    let summary = from_json(&fixture(6)).expect("v6 fixture は読めるべき");
    let doc = summary.document;

    assert_eq!(doc.layer_count(), 2);
    assert_eq!(doc.entity_count(), 3);

    let mut kinds: Vec<GeomKind> = doc.entities().map(|(_, e)| geom_kind(&e.geom)).collect();
    kinds.sort();
    assert_eq!(
        kinds,
        vec![GeomKind::Shape, GeomKind::DimLinear, GeomKind::Table]
    );

    // arrow_kind はファイルの値をそのまま持つ(ClosedFilled 既定ではない)。
    assert_eq!(doc.dim_style().arrow_kind, ArrowKind::Open90);

    // 表の内容を抜き取り確認。
    let table = doc
        .entities()
        .find_map(|(_, e)| match &e.geom {
            EntityGeom::Table(t) => Some(t.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(table.col_widths_mm.len(), 2);
    assert_eq!(table.row_heights_mm.len(), 2);
    assert_eq!(table.cells, vec!["No.", "部品名", "1", "ブラケット"]);

    // v6 の長さ寸法は向きを持たないので Aligned へ補完される（M11 タスク68）。
    let dim = doc
        .entities()
        .find_map(|(_, e)| match &e.geom {
            EntityGeom::DimLinear(d) => Some(d.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(dim.direction, DimDirection::Aligned);

    let exported = export_document(&doc);
    assert_eq!(exported.version, FORMAT_VERSION);
    let reloaded = from_json(&mcad_io::to_json(&doc).unwrap())
        .unwrap()
        .document;
    assert_eq!(export_document(&reloaded), exported);
}

// ---------------------------------------------------------------------
// v7
// ---------------------------------------------------------------------

#[test]
fn v7_fixture_loads_with_expected_content() {
    // v7(現行): 長さ寸法の向き（DimDirection）を追加。M11 タスク68。
    let summary = from_json(&fixture(7)).expect("v7 fixture は読めるべき");
    let doc = summary.document;

    assert_eq!(doc.layer_count(), 2);
    assert_eq!(doc.entity_count(), 4);

    let mut kinds: Vec<GeomKind> = doc.entities().map(|(_, e)| geom_kind(&e.geom)).collect();
    kinds.sort();
    assert_eq!(
        kinds,
        vec![
            GeomKind::Shape,
            GeomKind::DimLinear,
            GeomKind::DimLinear,
            GeomKind::DimLinear,
        ]
    );

    // 同じ斜辺 (0,0)-(120,30) に 3 通りの向きで寸法が付いている。値はそれぞれ
    // 実距離・水平投影・鉛直投影になる（fixture の座標から計算し直した期待値）。
    let mut measured: Vec<(DimDirection, f64)> = doc
        .entities()
        .filter_map(|(_, e)| match &e.geom {
            EntityGeom::DimLinear(d) => Some((d.direction, d.measured_value())),
            _ => None,
        })
        .collect();
    measured.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    assert_eq!(
        measured[0].0,
        DimDirection::Rotated(std::f64::consts::FRAC_PI_2)
    );
    assert!(
        (measured[0].1 - 30.0).abs() < 1e-9,
        "鉛直投影: {measured:?}"
    );
    assert_eq!(measured[1].0, DimDirection::Rotated(0.0));
    assert!(
        (measured[1].1 - 120.0).abs() < 1e-9,
        "水平投影: {measured:?}"
    );
    assert_eq!(measured[2].0, DimDirection::Aligned);
    assert!(
        (measured[2].1 - 123.693_168_768_529_82).abs() < 1e-9,
        "実距離: {measured:?}"
    );

    // この fixture は M11 タスク78（補助線の傾き）より前に保存した v7 なので
    // `ext_angle` キーを持たない。`#[serde(default)]` で「傾き無し」に補完され、
    // **既に v7 で保存されたファイルがそのまま読める**ことの回帰になる。
    assert!(
        doc.entities().all(|(_, e)| match &e.geom {
            EntityGeom::DimLinear(d) => d.ext_angle.is_none(),
            _ => true,
        }),
        "ext_angle を持たない v7 は傾き無しとして読むべき"
    );

    let exported = export_document(&doc);
    assert_eq!(exported.version, FORMAT_VERSION);
    let reloaded = from_json(&mcad_io::to_json(&doc).unwrap())
        .unwrap()
        .document;
    assert_eq!(export_document(&reloaded), exported);
}

// ---------------------------------------------------------------------
// バージョン横断
// ---------------------------------------------------------------------

#[test]
fn all_fixtures_declare_their_own_version_and_reexport_as_current() {
    // 各 fixture が自称するバージョン番号どおりに解釈され、再書き出しは常に現行版になる。
    for version in 1..=FORMAT_VERSION {
        let summary = from_json(&fixture(version))
            .unwrap_or_else(|e| panic!("v{version} fixture が読めない: {e}"));
        assert_eq!(
            export_document(&summary.document).version,
            FORMAT_VERSION,
            "v{version} の再書き出しは現行版であるべき"
        );
    }
}
