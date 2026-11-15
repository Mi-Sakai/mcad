//! `.mcad` v1〜v9 の後方互換 fixture テスト（M11 タスク65、v7 はタスク68、v8 と
//! `v7_legacy_boms.mcad` は M12 タスク81 で追加、`v8_before_col_aligns.mcad` は
//! M12 タスク93 で追加、`v8.mcad` の構築線は M12 タスク91 で追加、v9 は M13 タスク98）。
//!
//! # fixture について
//!
//! `tests/fixtures/v{1..9}.mcad` は各バージョンのスキーマが表現できる要素を
//! 一通り含む実ファイル（レイヤー2枚・複数ジオメトリ種・その版で表現可能な
//! メタデータ）。各版のスキーマの一次情報は
//! `crates/mcad-io/src/mcad_file.rs` のモジュール doc と DTO 定義
//! （`FileDocument` / `FileLayer` / `EntityGeomV5` / `EntityGeomV6` / `EntityGeomV7` /
//! `DimLinearV6` / `TableGeomV7` / `SheetMetaV7` 等）。
//!
//! `v7_legacy_boms.mcad` は版ごとの代表ではなく、「v7 以前の旧部品表を中身から推測せず
//! 表として読む」（DESIGN.md M12 設計判断1）ことを固定する追加の v7 ファイル。
//!
//! `v8_before_col_aligns.mcad` も版ごとの代表ではなく、タスク93 で `col_aligns` を
//! v8 へ足す**前**（タスク81〜92 の間）の実際のビルドが書く v8 をそのまま固定した
//! ファイル（`kind` はあるが `col_aligns` が無い。`git show <タスク93直前のコミット>:
//! crates/mcad-io/tests/fixtures/v8.mcad` の内容そのもの）。「その時期の v8 ファイルが
//! タスク93 後も読める」ことの回帰（Codex レビュー指摘）。
//!
//! `v9.mcad` は手書きせず、exporter（`to_json`）の出力を一度書き出して作った
//! （組み立て手順は `tests/fixtures/README.md`）。
//!
//! `v9_full.mcad` は v9 で表現できる要素のうち `v9.mcad` が持たないもの（文字・半径/直径/
//! 角度/座標寸法・寸法の注記全般・整列寸法・点と閉じたポリライン・非既定の寸法スタイル・
//! レイヤー・スタイル・図面メタ・接線拘束の別の側）を集めた v9（v1.0.0 の「`.mcad` 後方互換の
//! 宣言」の前提）。作り方は `v9.mcad` と同じ。1 つの `.mcad` が持てない値（用紙・様式・
//! `ArrowKind` の全種など）は、fixture を増やさず exporter の往復テストで確かめる。
//! 版 10 の拒否と v9 の未知キーの無視もここ（fixture の内容を使う通常の読込経路のテスト）。
//!
//! 「不正な JSON の拒否」「偽装 `Table` の拒否」「v6 に `direction` を混ぜた
//! ファイルの拒否」のような**壊れた入力**のテストはここへは置かない
//! （`mcad_file.rs` の `#[cfg(test)]` に残す）。fixture ディレクトリは
//! 「正規のサンプル」という位置づけを保つため。
//!
//! # v10 を追加するとき
//!
//! `.mcad` フォーマットが v10 へ上がったら:
//!
//! 1. 現行版の mcad で保存したファイル（または exporter の出力を一度書き出したもの。
//!    `tests/fixtures/README.md` の v9 の手順）を `tests/fixtures/v10.mcad` として追加する。
//!    新フィールドを含めること。
//! 2. 本ファイルに `v10_*` の名前で新しいバージョンの検証項目
//!    （新フィールドの既定値補完・ラウンドトリップ）を1ケース足す。
//! 3. `all_fixtures_declare_their_own_version_and_reexport_as_current` は
//!    `1..=FORMAT_VERSION` を回すだけなので、`FORMAT_VERSION` を上げれば自動で対象に入る。
//! 4. 旧版（このときは v9）が変わらず読めることは、既存の `v9_*` テスト（`v9_full_*` を含む）が
//!    そのまま回帰網として働く。`v9_fixture_is_exactly_what_the_exporter_writes` は
//!    「現行版の書き出しと一致」を見るので、v10 では「v9 として読めて内容が保たれる」
//!    検査へ書き換える（`v8_fixture_still_loads_and_is_upgraded_on_export` の形）。
//! 5. `version_10_file_is_rejected_by_the_regular_load_paths` は「現行より新しい版」を 10 で
//!    作っているので、版を上げたら 11 へ、文言の `supported: 1..=9` も合わせて直す。
//!    `unknown_keys_in_a_v9_file_are_ignored` と `v9_full_*` も、v10 では「v9 として読める」検査へ
//!    寄せる（v10 の網羅見本を足すかは、そのとき決める）。

use std::fs;
use std::path::PathBuf;

use mcad_core::{
    ArrowPlacement, CircleTangency, Command, Constraint, DimAnnotation, DimDirection, DimStyle,
    Document, EntityGeom, HorizontalAlign, LargerCircle, Layer, Linetype, OrdinateAxis,
    Orientation, PaperSize, PointKind, ProjectionMethod, Rgb, Scale, SheetMeta, Side,
    SizeTolerance, TableKind, TitleBlockField, TitleBlockKind, ValueStyle, WidthMm,
};
use mcad_geom::{ArrowKind, DimSymbol, Point2, Shape, Vec2};
use mcad_io::{FORMAT_VERSION, IoError, export_document, from_json, load_mcad, to_json};

/// `tests/fixtures/v{version}.mcad` の内容を読む。
fn fixture(version: u32) -> String {
    fixture_named(&format!("v{version}.mcad"))
}

/// `tests/fixtures/{name}` の内容を読む。
fn fixture_named(name: &str) -> String {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests/fixtures");
    path.push(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("fixture {path:?} を読めない: {e}"))
}

/// 文書内の表の列ごとの揃えを、生存エンティティの列挙順で集める（M12 タスク93）。
fn table_aligns(doc: &Document) -> Vec<Vec<HorizontalAlign>> {
    doc.entities()
        .filter_map(|(_, e)| match &e.geom {
            EntityGeom::Table(t) => Some(t.col_aligns.clone()),
            _ => None,
        })
        .collect()
}

/// 文書内の表の種類を、生存エンティティの列挙順で集める。
fn table_kinds(doc: &Document) -> Vec<TableKind> {
    doc.entities()
        .filter_map(|(_, e)| match &e.geom {
            EntityGeom::Table(t) => Some(t.kind),
            _ => None,
        })
        .collect()
}

/// エンティティの `EntityGeom` バリアント名をカウントするための小さな分類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum GeomKind {
    Shape,
    Text,
    DimLinear,
    DimRadial,
    DimDiameter,
    DimAngular,
    DimOrdinate,
    Table,
    XLine,
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
        EntityGeom::DimAngular(_) => GeomKind::DimAngular,
        EntityGeom::DimOrdinate(_) => GeomKind::DimOrdinate,
        EntityGeom::Table(_) => GeomKind::Table,
        EntityGeom::XLine(_) => GeomKind::XLine,
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
    // v7: 長さ寸法の向き（DimDirection）を追加。M11 タスク68。
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

#[test]
fn v7_legacy_boms_fixture_loads_as_plain_tables() {
    // v0.11.0 以前の旧 5 列部品表と v0.11.1 の様式A/C 7 列部品表を 1 つずつ持つ v7。
    // どちらも見出しは部品表の並びそのものだが、**中身から推測せず表として読む**
    // （DESIGN.md M12 設計判断1）。推測していたら部品表が 2 つになり読込が失敗する。
    let summary = from_json(&fixture_named("v7_legacy_boms.mcad"))
        .expect("旧部品表を 2 つ持つ v7 は読めるべき");
    let doc = summary.document;

    assert_eq!(doc.entity_count(), 3);
    assert_eq!(table_kinds(&doc), vec![TableKind::Table, TableKind::Table]);
    assert_eq!(doc.bom(), None);
    assert!(doc.sheet().use_bom, "v7 以前は「部品表を使う」= ON で読む");

    // 表の中身は変わらない（見出し行 = 最下段がそのまま残る）。
    let headers: Vec<Vec<String>> = doc
        .entities()
        .filter_map(|(_, e)| match &e.geom {
            EntityGeom::Table(t) => Some(
                (0..t.cols())
                    .map(|c| t.cell(t.rows() - 1, c).unwrap().to_owned())
                    .collect(),
            ),
            _ => None,
        })
        .collect();
    assert_eq!(
        headers,
        vec![
            vec!["品番", "名称", "個数", "材質", "備考"],
            vec!["部番", "品番", "名称", "個数", "材質", "質量[kg]", "備考"],
        ]
    );

    // v8 で保存し直すと "kind": "Table" が書かれ、再読込しても表のまま。
    let json = mcad_io::to_json(&doc).unwrap();
    assert_eq!(json.matches(r#""kind": "Table""#).count(), 2, "{json}");
    assert!(json.contains(r#""use_bom": true"#), "{json}");
    let reloaded = from_json(&json).unwrap().document;
    assert_eq!(export_document(&reloaded), export_document(&doc));
}

// ---------------------------------------------------------------------
// v8
// ---------------------------------------------------------------------

#[test]
fn v8_fixture_loads_with_expected_content() {
    // v8: 表の種類（TableKind）と「部品表を使う」（SheetMeta::use_bom）。M12 タスク81。
    // 表の列ごとの揃え（TableGeom::col_aligns）が M12 タスク93 で相乗りした。
    let summary = from_json(&fixture(8)).expect("v8 fixture は読めるべき");
    let doc = summary.document;

    assert_eq!(doc.layer_count(), 2);
    assert_eq!(doc.entity_count(), 5);
    assert!(!doc.sheet().use_bom, "use_bom = false はそのまま読む");
    assert_eq!(table_kinds(&doc), vec![TableKind::Bom, TableKind::Table]);

    // 列ごとの揃え（M12 タスク93、v8 に相乗り）はファイルの値をそのまま読む。部品表は
    // 部品表プリセットの既定（部番・個数・材質・質量 = 中央、品番・名称・備考 = 左）。
    assert_eq!(
        table_aligns(&doc),
        vec![
            vec![
                HorizontalAlign::Center,
                HorizontalAlign::Left,
                HorizontalAlign::Left,
                HorizontalAlign::Center,
                HorizontalAlign::Center,
                HorizontalAlign::Center,
                HorizontalAlign::Left,
            ],
            vec![HorizontalAlign::Center, HorizontalAlign::Right],
        ]
    );

    // 部品表はロック済みレイヤー上にあっても読める（ロックはエンティティ投入後に適用）。
    let bom = doc.bom().expect("部品表が 1 つあるはず");
    let bom_layer = doc.layer(doc.entity(bom).unwrap().layer).unwrap();
    assert_eq!(bom_layer.name, "tables");
    assert!(bom_layer.locked);

    // v7 までの要素（回転寸法・補助線の傾き）も v8 のまま読める。
    let dim = doc
        .entities()
        .find_map(|(_, e)| match &e.geom {
            EntityGeom::DimLinear(d) => Some(d.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(dim.direction, DimDirection::Rotated(0.0));
    assert_eq!(dim.ext_angle, Some(1.2));

    // 構築線（M12 タスク91、v8 に相乗り）は基点と単位方向をファイルの値のまま読む。
    // スタイルの線種上書きも他の図形と同じく保たれる。
    let (xline, style) = doc
        .entities()
        .find_map(|(_, e)| match &e.geom {
            EntityGeom::XLine(x) => Some((*x, e.style)),
            _ => None,
        })
        .expect("構築線が 1 本あるはず");
    assert_eq!(xline.base, Point2::new(10.0, 20.0));
    assert_eq!(xline.dir, Vec2::new(0.6, 0.8));
    assert_eq!(style.linetype, Some(Linetype::DashDot));
}

#[test]
fn v8_fixture_still_loads_and_is_upgraded_on_export() {
    // v9 から、v8 fixture は「現行版の書き出しと一致」ではなく「v8 として読めて内容が
    // 保たれる」ことを見る。書き出しは v9 になり、違いは `version` と空の `constraints`
    // だけ（数値は文字列を解析して比べる。f32 の線幅を `to_value` 経由にすると f64 へ
    // 広げた誤差が出るため）。
    let text = fixture(8);
    let summary = from_json(&text).expect("v8 fixture は読めるべき");
    assert_eq!(
        summary.document.constraint_count(),
        0,
        "v8 は拘束を持たない"
    );
    let written = mcad_io::to_json(&summary.document).unwrap();
    let mut expected: serde_json::Value = serde_json::from_str(&text).unwrap();
    expected["version"] = serde_json::json!(9);
    expected["constraints"] = serde_json::json!([]);
    let actual: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(actual, expected, "書き出し: {written}");
}

#[test]
fn v8_before_col_aligns_fixture_is_backfilled_as_left() {
    // タスク93 より前のビルドが書いた v8（`kind` はあるが `col_aligns` が無い）は
    // 拒否されず、列数ぶんの左揃えで補って読める（Codex レビュー指摘）。
    let text = fixture_named("v8_before_col_aligns.mcad");
    assert!(
        !text.contains("col_aligns"),
        "テスト前提: この fixture は col_aligns を持たない: {text}"
    );
    let doc = from_json(&text)
        .expect("col_aligns を欠く旧い v8 fixture は読めるべき")
        .document;
    assert_eq!(doc.layer_count(), 2);
    assert_eq!(doc.entity_count(), 4);
    assert_eq!(table_kinds(&doc), vec![TableKind::Bom, TableKind::Table]);
    for aligns in table_aligns(&doc) {
        assert!(
            aligns.iter().all(|a| *a == HorizontalAlign::Left),
            "col_aligns 欠落時はすべて左揃えで読むべき: {aligns:?}"
        );
    }

    // 書き出すと col_aligns 付きの現行版（v9）になる（値そのものは補完した左揃えのまま）。
    let written = mcad_io::to_json(&doc).unwrap();
    let written: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(written["version"], 9);
    let table_entities: Vec<&serde_json::Value> = written["entities"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["geom"].get("Table").is_some())
        .collect();
    assert_eq!(table_entities.len(), 2, "表は 2 つのはず: {written}");
    for entity in table_entities {
        let table = &entity["geom"]["Table"];
        let cols = table["col_widths_mm"].as_array().unwrap().len();
        assert_eq!(
            table["col_aligns"],
            serde_json::Value::Array(vec![serde_json::Value::String("Left".into()); cols]),
            "書き出しは col_aligns を列数ぶん明示的に持つべき: {written}"
        );
    }
}

// ---------------------------------------------------------------------
// v9
// ---------------------------------------------------------------------

#[test]
fn v9_fixture_loads_with_expected_content() {
    // v9(現行): 幾何拘束（M13 タスク98）。エンティティは `entities` 配列の添字で指す。
    // fixture は v8.mcad の 5 要素（部品表・表・構築線・長さ寸法など）に、線分 5・円 3・
    // 円弧 1・ポリライン 1 と、全 15 種の拘束 20 件を足したもの。
    let summary = from_json(&fixture(9)).expect("v9 fixture は読めるべき");
    let doc = summary.document;

    assert_eq!(doc.layer_count(), 2);
    assert_eq!(doc.entity_count(), 15);
    assert_eq!(doc.constraint_count(), 20);
    assert!(!doc.can_undo(), "読込操作は undo できない");

    // v8 までの要素も v9 のまま読める。
    assert!(!doc.sheet().use_bom);
    assert_eq!(table_kinds(&doc), vec![TableKind::Bom, TableKind::Table]);
    assert!(doc.bom().is_some());

    // 15 種すべてが 1 件以上ある（種類の数え上げは網羅 match なので、新しい種類を
    // 足すとここがコンパイルエラーになる）。
    let mut seen = [false; 15];
    for (_, c) in doc.constraints() {
        let slot = match c {
            Constraint::Coincident { .. } => 0,
            Constraint::Horizontal { .. } => 1,
            Constraint::Vertical { .. } => 2,
            Constraint::Distance { .. } => 3,
            Constraint::PointLineDistance { .. } => 4,
            Constraint::Parallel { .. } => 5,
            Constraint::Perpendicular { .. } => 6,
            Constraint::EqualLength { .. } => 7,
            Constraint::EqualRadius { .. } => 8,
            Constraint::Radius { .. } => 9,
            Constraint::Angle { .. } => 10,
            Constraint::TangentLineCircle { .. } => 11,
            Constraint::TangentCircles { .. } => 12,
            Constraint::FixPoint { .. } => 13,
            Constraint::FixEntity { .. } => 14,
        };
        seen[slot] = true;
    }
    assert_eq!(seen, [true; 15]);

    // 値・側・外接/内接・点の種類・固定座標がファイルのとおりに戻る。
    let all: Vec<&Constraint> = doc.constraints().map(|(_, c)| c).collect();
    assert!(all.iter().any(|c| matches!(
        c,
        Constraint::Distance { a, b, value }
            if a.kind == PointKind::Start && b.kind == PointKind::End && *value == 100.0
    )));
    assert!(all.iter().any(|c| matches!(
        c,
        Constraint::Angle { value, .. } if *value == std::f64::consts::FRAC_PI_2
    )));
    assert!(all.iter().any(|c| matches!(
        c,
        Constraint::PointLineDistance { side: Side::Right, value, .. } if *value == 30.0
    )));
    assert!(all.iter().any(|c| matches!(
        c,
        Constraint::TangentCircles {
            kind: CircleTangency::External,
            ..
        }
    )));
    assert!(all.iter().any(|c| matches!(
        c,
        Constraint::TangentCircles {
            kind: CircleTangency::Internal(LargerCircle::First),
            ..
        }
    )));
    assert!(all.iter().any(|c| matches!(
        c,
        Constraint::FixPoint { point, at }
            if point.kind == PointKind::End && *at == Point2::new(230.0, 90.0)
    )));
    // どの拘束も、生きたエンティティを指している（添字 → EntityId の対応が正しい）。
    for c in &all {
        for id in c.entities() {
            assert!(doc.entity(id).is_some(), "{c:?}");
        }
        assert_eq!(c.is_satisfied(&doc), Ok(true), "{c:?}");
    }
}

#[test]
fn v9_fixture_is_exactly_what_the_exporter_writes() {
    // v9 往復の無損失性を最も強い形で固定する: fixture を読んで書き出した JSON が、
    // fixture そのものと（キーの並び・空白を除いて）一致する。
    let text = fixture(9);
    let doc = from_json(&text).unwrap().document;
    let written = mcad_io::to_json(&doc).unwrap();
    let expected: serde_json::Value = serde_json::from_str(&text).unwrap();
    let actual: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(actual, expected, "書き出し: {written}");
}

// ---------------------------------------------------------------------
// v9_full（v9 で表現できる要素の網羅。v9.mcad が持たないもの）
// ---------------------------------------------------------------------

#[test]
fn v9_full_fixture_loads_with_expected_content() {
    // v9.mcad が持たない要素（文字・半径/直径/角度/座標寸法・寸法の注記全般・整列寸法・
    // 点と閉じたポリライン・非既定の DimStyle・非既定のレイヤー/スタイル・Custom 様式の図面
    // メタ・接線拘束の Right / Internal(Second)）を 1 枚に集めた v9。
    let summary = from_json(&fixture_named("v9_full.mcad")).expect("v9_full は読めるべき");
    let doc = summary.document;

    assert_eq!(summary.clamped_widths, 0);
    assert_eq!(doc.layer_count(), 4);
    assert_eq!(doc.entity_count(), 23);
    assert_eq!(doc.constraint_count(), 2);
    assert!(!doc.can_undo(), "読込操作は undo できない");

    // 種類ごとの件数。
    let count = |kind: GeomKind| {
        doc.entities()
            .filter(|(_, e)| geom_kind(&e.geom) == kind)
            .count()
    };
    assert_eq!(count(GeomKind::Shape), 9);
    assert_eq!(count(GeomKind::Text), 1);
    assert_eq!(count(GeomKind::DimLinear), 5);
    assert_eq!(count(GeomKind::DimRadial), 3);
    assert_eq!(count(GeomKind::DimDiameter), 2);
    assert_eq!(count(GeomKind::DimAngular), 1);
    assert_eq!(count(GeomKind::DimOrdinate), 2);
    assert_eq!(count(GeomKind::Table), 0);
    assert_eq!(count(GeomKind::XLine), 0);
    assert_eq!(count(GeomKind::Unknown), 0);

    // レイヤー: 非表示と、実線以外の 3 線種。重ね順の昇順で並ぶ。
    let layers: Vec<(String, bool, Linetype, i32)> = doc
        .layers_in_order()
        .into_iter()
        .map(|(_, l)| (l.name.clone(), l.visible, l.linetype, l.order))
        .collect();
    assert_eq!(
        layers,
        vec![
            ("0".to_owned(), true, Linetype::Continuous, 0),
            ("dashed".to_owned(), true, Linetype::Dashed, 1),
            ("chain".to_owned(), true, Linetype::DashDot, 2),
            ("hidden".to_owned(), false, Linetype::DashDotDot, 3),
        ]
    );

    // Shape: 点・閉じたポリライン・線・円弧・円。
    let shapes: Vec<&Shape> = doc
        .entities()
        .filter_map(|(_, e)| match &e.geom {
            EntityGeom::Shape(s) => Some(s),
            _ => None,
        })
        .collect();
    assert!(
        shapes
            .iter()
            .any(|s| matches!(s, Shape::Point(p) if *p == Point2::new(5.0, 5.0)))
    );
    assert!(
        shapes
            .iter()
            .any(|s| matches!(s, Shape::Polyline(pl) if pl.closed && pl.vertices.len() == 4))
    );
    assert!(shapes.iter().any(|s| matches!(s, Shape::Arc(_))));

    // 明示スタイル: 色・線幅・線種（Dashed / DashDotDot / Continuous / DashDot）。
    let styles: Vec<_> = doc.entities().map(|(_, e)| e.style).collect();
    assert!(styles.iter().any(|s| s.color == Some(Rgb::new(200, 30, 30))
        && s.width_mm == Some(WidthMm::new(0.5).unwrap())
        && s.linetype == Some(Linetype::Dashed)));
    assert!(styles.iter().any(|s| s.color.is_none()
        && s.width_mm.is_none()
        && s.linetype == Some(Linetype::DashDotDot)));
    assert!(styles.iter().any(|s| s.color == Some(Rgb::new(0, 0, 0))
        && s.width_mm == Some(WidthMm::new(0.13).unwrap())
        && s.linetype == Some(Linetype::Continuous)));
    assert!(
        styles
            .iter()
            .any(|s| s.width_mm == Some(WidthMm::new(2.0).unwrap())
                && s.linetype == Some(Linetype::DashDot))
    );

    // 文字（CJK 混じり）。
    let text = doc
        .entities()
        .find_map(|(_, e)| match &e.geom {
            EntityGeom::Text(t) => Some(t.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(text.content, "寸法 ABC");
    assert_eq!((text.height, text.angle), (5.0, 0.5));
    assert_eq!(text.anchor, Point2::new(10.0, 40.0));

    // 寸法の注記（全寸法を通した集計）。
    let annotations: Vec<&DimAnnotation> = doc
        .entities()
        .filter_map(|(_, e)| match &e.geom {
            EntityGeom::DimLinear(d) => Some(&d.annotation),
            EntityGeom::DimRadial(d) => Some(&d.annotation),
            EntityGeom::DimDiameter(d) => Some(&d.annotation),
            EntityGeom::DimAngular(d) => Some(&d.annotation),
            EntityGeom::DimOrdinate(d) => Some(&d.annotation),
            _ => None,
        })
        .collect();
    assert_eq!(annotations.len(), 13);
    // 寸法補助記号 8 種すべて。
    for symbol in [
        DimSymbol::Diameter,
        DimSymbol::SphereDiameter,
        DimSymbol::Square,
        DimSymbol::Radius,
        DimSymbol::SphereRadius,
        DimSymbol::ControlRadius,
        DimSymbol::Chamfer,
        DimSymbol::Thickness,
    ] {
        assert!(
            annotations.iter().any(|a| a.symbol == Some(symbol)),
            "{symbol:?} が無い"
        );
    }
    // サイズ公差 3 種。
    assert!(
        annotations
            .iter()
            .any(|a| a.tolerance == Some(SizeTolerance::Symmetric(0.05)))
    );
    assert!(annotations.iter().any(|a| a.tolerance
        == Some(SizeTolerance::Deviations {
            upper: 0.1,
            lower: -0.05
        })));
    assert!(annotations.iter().any(|a| matches!(
        &a.tolerance,
        Some(SizeTolerance::Fit(f)) if f.as_str() == "H7"
    )));
    // 矢の配置 3 値・値の表記 3 値。
    for placement in [
        ArrowPlacement::Auto,
        ArrowPlacement::Inside,
        ArrowPlacement::Outside,
    ] {
        assert!(annotations.iter().any(|a| a.arrow_placement == placement));
    }
    for style in [
        ValueStyle::Plain,
        ValueStyle::Reference,
        ValueStyle::TheoreticallyExact,
    ] {
        assert!(annotations.iter().any(|a| a.value_style == style));
    }

    // 1 本目の長さ寸法（整列・補助線の傾きなし・全フィールドが埋まった注記）。
    let linears: Vec<_> = doc
        .entities()
        .filter_map(|(_, e)| match &e.geom {
            EntityGeom::DimLinear(d) => Some(d.clone()),
            _ => None,
        })
        .collect();
    let first = &linears[0];
    assert_eq!(first.direction, DimDirection::Aligned);
    assert_eq!(first.ext_angle, None);
    assert_eq!(first.offset, -10.0);
    assert_eq!(
        first.annotation,
        DimAnnotation {
            symbol: Some(DimSymbol::Diameter),
            tolerance: Some(SizeTolerance::Symmetric(0.05)),
            decimals_override: Some(1),
            text_anchor: Some(Point2::new(20.0, -14.0)),
            arrow_placement: ArrowPlacement::Outside,
            value_override: None,
            text_rotation: Some(0.0),
            value_style: ValueStyle::Reference,
            prefix: Some("2×".to_owned()),
            suffix: Some("-M6".to_owned()),
        }
    );
    // 2 本目: 回転寸法・補助線の傾き・表示値の上書き。
    let second = &linears[1];
    assert_eq!(
        second.direction,
        DimDirection::Rotated(std::f64::consts::FRAC_PI_2)
    );
    assert_eq!(second.ext_angle, Some(1.4));
    assert_eq!(second.annotation.value_override.as_deref(), Some("30-35"));
    assert_eq!(
        linears
            .iter()
            .filter(|d| d.direction == DimDirection::Aligned)
            .count(),
        4
    );

    // 座標寸法: X・Y の両軸。値は符号つき（X は -30）。
    let ordinates: Vec<_> = doc
        .entities()
        .filter_map(|(_, e)| match &e.geom {
            EntityGeom::DimOrdinate(d) => Some(d.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(ordinates.len(), 2);
    let x = ordinates
        .iter()
        .find(|d| d.axis == OrdinateAxis::X)
        .unwrap();
    let y = ordinates
        .iter()
        .find(|d| d.axis == OrdinateAxis::Y)
        .unwrap();
    assert_eq!(x.measured_value(), -30.0);
    assert_eq!(y.measured_value(), 10.0);
    assert_eq!(y.annotation.prefix.as_deref(), Some("Y="));

    // 角度寸法: 頂点 (0,100) から (30,100) と (20,120) の方向のなす角（atan2(20, 20) = π/4）。
    let angular = doc
        .entities()
        .find_map(|(_, e)| match &e.geom {
            EntityGeom::DimAngular(d) => Some(d.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(angular.arc_radius, 18.0);
    assert!((angular.measured_value() - std::f64::consts::FRAC_PI_4).abs() < 1e-9);
    assert_eq!(angular.annotation.decimals_override, Some(1));

    // 半径寸法の値と記号。
    let radial_symbols: Vec<_> = doc
        .entities()
        .filter_map(|(_, e)| match &e.geom {
            EntityGeom::DimRadial(d) => Some((d.radius, d.annotation.symbol)),
            _ => None,
        })
        .collect();
    assert_eq!(
        radial_symbols,
        vec![
            (8.0, Some(DimSymbol::Radius)),
            (9.0, Some(DimSymbol::SphereRadius)),
            (10.0, Some(DimSymbol::ControlRadius)),
        ]
    );

    // 寸法スタイル: すべて既定と異なる値。矢先は ClosedFilled 以外。
    assert_eq!(
        *doc.dim_style(),
        DimStyle {
            text_height_mm: 2.5,
            arrow_len_mm: 4.0,
            decimals: 3,
            trim_trailing_zeros: false,
            ext_gap_mm: 0.8,
            ext_overshoot_mm: 1.5,
            text_gap_mm: 1.0,
            tolerance_scale: 0.6,
            arrow_kind: ArrowKind::Open30,
        }
    );

    // 図面メタ: v9.mcad と違う側の値（A4・縦・Custom 様式・枠なし・部品表を使う・第一角法）。
    let sheet = doc.sheet();
    assert_eq!(sheet.paper, PaperSize::A4);
    assert_eq!(sheet.orientation, Orientation::Portrait);
    assert_eq!(sheet.scale, Scale::new(2, 1).unwrap());
    assert!(!sheet.frame_visible);
    assert!(sheet.use_bom);
    assert_eq!(sheet.fields.projection, ProjectionMethod::FirstAngle);
    assert_eq!(sheet.fields.drawing_number, "MCAD-FULL-001");
    let TitleBlockKind::Custom(template) = &sheet.title_block else {
        panic!("様式は Custom のはず: {:?}", sheet.title_block);
    };
    assert_eq!(template.width_mm, 90.0);
    let binds: Vec<Option<TitleBlockField>> = template
        .rows
        .iter()
        .flat_map(|r| r.cells.iter().map(|c| c.bind))
        .collect();
    assert_eq!(
        binds,
        vec![
            Some(TitleBlockField::DrawingTitle),
            Some(TitleBlockField::DrawingNumber),
            Some(TitleBlockField::Author),
            Some(TitleBlockField::Date),
            Some(TitleBlockField::Scale),
            Some(TitleBlockField::Projection),
            Some(TitleBlockField::PaperSize),
            Some(TitleBlockField::Revision),
            None,
        ]
    );

    // 接線拘束: 線の右側にある円（Right）と、2 つ目が大きい内接（Internal(Second)）。
    let all: Vec<&Constraint> = doc.constraints().map(|(_, c)| c).collect();
    assert!(all.iter().any(|c| matches!(
        c,
        Constraint::TangentLineCircle {
            side: Side::Right,
            ..
        }
    )));
    assert!(all.iter().any(|c| matches!(
        c,
        Constraint::TangentCircles {
            kind: CircleTangency::Internal(LargerCircle::Second),
            ..
        }
    )));
    for c in &all {
        assert_eq!(c.is_satisfied(&doc), Ok(true), "{c:?}");
    }

    // ラウンドトリップ: 読込 → 保存 → 再読込で内容が保たれる。
    let exported = export_document(&doc);
    assert_eq!(exported.version, FORMAT_VERSION);
    let reloaded = from_json(&to_json(&doc).unwrap()).unwrap().document;
    assert_eq!(export_document(&reloaded), exported);
}

#[test]
fn v9_full_fixture_is_exactly_what_the_exporter_writes() {
    // `v9_fixture_is_exactly_what_the_exporter_writes` と同じ最も強い形の固定。
    let text = fixture_named("v9_full.mcad");
    let doc = from_json(&text).unwrap().document;
    let written = to_json(&doc).unwrap();
    let expected: serde_json::Value = serde_json::from_str(&text).unwrap();
    let actual: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(actual, expected, "書き出し: {written}");
}

/// 文書の書き出しを読み戻した文書（書き出し → 読込の 1 往復）。
fn round_trip(doc: &Document) -> Document {
    from_json(&to_json(doc).unwrap()).unwrap().document
}

#[test]
fn sheet_variants_round_trip_through_the_exporter() {
    // 1 つの `.mcad` は sheet を 1 つしか持てないので、用紙・向き・様式の取りうる値は
    // fixture では 1 組しか固定できない（v9.mcad = A3・横・様式A、v9_full.mcad = A4・縦・
    // Custom）。残りの値は、exporter の出力を読み戻す往復で網羅する（Custom は
    // v9_full.mcad が固定している）。
    let papers = [
        PaperSize::A0,
        PaperSize::A1,
        PaperSize::A2,
        PaperSize::A3,
        PaperSize::A4,
    ];
    let kinds = [TitleBlockKind::A, TitleBlockKind::B, TitleBlockKind::C];
    let mut checked = 0;
    for paper in papers {
        for orientation in [Orientation::Landscape, Orientation::Portrait] {
            for title_block in &kinds {
                for frame_visible in [false, true] {
                    for use_bom in [false, true] {
                        for projection in
                            [ProjectionMethod::ThirdAngle, ProjectionMethod::FirstAngle]
                        {
                            let mut sheet = SheetMeta {
                                paper,
                                orientation,
                                title_block: title_block.clone(),
                                frame_visible,
                                use_bom,
                                ..SheetMeta::default()
                            };
                            sheet.fields.projection = projection;
                            let mut doc = Document::new();
                            doc.apply(Command::SetSheet(sheet.clone())).unwrap();
                            let reloaded = round_trip(&doc);
                            assert_eq!(*reloaded.sheet(), sheet);
                            assert_eq!(to_json(&reloaded).unwrap(), to_json(&doc).unwrap());
                            checked += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(checked, 5 * 2 * 3 * 2 * 2 * 2);
}

#[test]
fn dim_style_arrow_kinds_round_trip_through_the_exporter() {
    // v9_full.mcad は ArrowKind を 1 つ（Open30）しか持てない。7 種すべてを往復で確かめる。
    for arrow_kind in [
        ArrowKind::ClosedFilled,
        ArrowKind::ClosedBlank,
        ArrowKind::Open30,
        ArrowKind::Open90,
        ArrowKind::Oblique,
        ArrowKind::Dot,
        ArrowKind::None,
    ] {
        let style = DimStyle {
            arrow_kind,
            ..DimStyle::DEFAULT
        };
        let mut doc = Document::new();
        doc.apply(Command::SetDimStyle(style)).unwrap();
        assert_eq!(*round_trip(&doc).dim_style(), style, "{arrow_kind:?}");
    }
}

#[test]
fn layer_linetypes_visibility_and_order_round_trip_through_the_exporter() {
    // レイヤーの線種 4 種 × 表示/非表示を往復で確かめる（fixture が持つ組み合わせに依存しない）。
    let mut doc = Document::new();
    let mut expected = vec![(
        "0".to_owned(),
        true,
        Linetype::Continuous,
        doc.layer(doc.default_layer()).unwrap().order,
    )];
    for (i, linetype) in [
        Linetype::Continuous,
        Linetype::Dashed,
        Linetype::DashDot,
        Linetype::DashDotDot,
    ]
    .into_iter()
    .enumerate()
    {
        for visible in [true, false] {
            let name = format!("{linetype:?}-{visible}");
            let mut layer = Layer::new(name.clone(), Rgb::new(10 * i as u8, 20, 30));
            layer.linetype = linetype;
            layer.visible = visible;
            layer.order = 10 + expected.len() as i32;
            expected.push((name, visible, linetype, layer.order));
            doc.apply(Command::AddLayer(layer)).unwrap();
        }
    }
    let actual: Vec<(String, bool, Linetype, i32)> = round_trip(&doc)
        .layers_in_order()
        .into_iter()
        .map(|(_, l)| (l.name.clone(), l.visible, l.linetype, l.order))
        .collect();
    assert_eq!(actual, expected);
}

// ---------------------------------------------------------------------
// 将来の版・未知のキー
// ---------------------------------------------------------------------

#[test]
fn version_10_file_is_rejected_by_the_regular_load_paths() {
    // 現行より新しい版は、アプリが使う通常の読込経路（文字列の `from_json` と、ファイルの
    // `load_mcad`）で拒否する。内容が現行版と同一で `version` だけが 10 のファイルでも拒否される
    // （新しい版のファイルを、現行版のスキーマとして読み違えない）。
    let mut value: serde_json::Value =
        serde_json::from_str(&fixture_named("v9_full.mcad")).unwrap();
    value["version"] = serde_json::json!(10);
    let text = serde_json::to_string_pretty(&value).unwrap();

    // 文言の版番号は意図的にリテラルで書く（FORMAT_VERSION を上げたら、この期待が落ちて
    // 「読める版の範囲」が変わったことに気づける）。
    let expected = "unsupported format version: 10 (supported: 1..=9)";

    let err = from_json(&text).err().expect("version 10 は拒否されるべき");
    assert!(matches!(err, IoError::UnsupportedVersion(10)), "{err:?}");
    assert_eq!(err.to_string(), expected);

    let path = std::env::temp_dir().join(format!(
        "mcad_backward_compat_v10_{}.mcad",
        std::process::id()
    ));
    fs::write(&path, &text).unwrap();
    let result = load_mcad(&path);
    fs::remove_file(&path).unwrap();
    let err = result.err().expect("version 10 は拒否されるべき");
    assert!(matches!(err, IoError::UnsupportedVersion(10)), "{err:?}");
    assert_eq!(err.to_string(), expected);
}

#[test]
fn unknown_keys_in_a_v9_file_are_ignored() {
    // 現行版（v9）の DTO は `deny_unknown_fields` を付けていないので、知らないキーは黙って
    // 無視して読める。この挙動を固定する（凍結 DTO の v4〜v7 は別: 未知キーの一部を拒否する。
    // それは `mcad_file.rs` のテストが固定している）。
    for name in ["v9.mcad", "v9_full.mcad"] {
        let original = fixture_named(name);
        let baseline = to_json(&from_json(&original).unwrap().document).unwrap();

        let mut value: serde_json::Value = serde_json::from_str(&original).unwrap();
        let unknown = serde_json::json!({"future": [1, 2, {"nested": true}]});
        value["unknown_top_level"] = unknown.clone();
        value["layers"][0]["unknown_in_layer"] = unknown.clone();
        value["sheet"]["unknown_in_sheet"] = unknown.clone();
        value["sheet"]["fields"]["unknown_in_fields"] = unknown.clone();
        value["dim_style"]["unknown_in_dim_style"] = unknown.clone();
        value["entities"][0]["unknown_in_entity"] = unknown.clone();
        value["entities"][0]["style"]["unknown_in_style"] = unknown.clone();
        // 幾何（`geom` → `Shape` → 図形の構造体）にも混ぜる。1 つ目のエンティティは Shape。
        let geom = value["entities"][0]["geom"].as_object_mut().unwrap();
        let shape = geom["Shape"].as_object_mut().unwrap();
        let (_, body) = shape.iter_mut().next().unwrap();
        body.as_object_mut()
            .unwrap()
            .insert("unknown_in_shape".to_owned(), unknown.clone());
        let mixed = serde_json::to_string_pretty(&value).unwrap();
        assert!(mixed.contains("unknown_in_entity"));

        let loaded = from_json(&mixed)
            .unwrap_or_else(|e| panic!("{name}: 未知のキーを混ぜても読めるべき: {e}"))
            .document;
        assert_eq!(
            to_json(&loaded).unwrap(),
            baseline,
            "{name}: 未知のキーを無視した結果は、混ぜる前と同じ内容のはず"
        );
    }
}

#[test]
fn fixtures_before_v9_have_no_constraints() {
    // v8 以前は拘束を持たない。読むと空で、書き出すと `constraints` が空配列で付く。
    for version in 1..9 {
        let doc = from_json(&fixture(version))
            .unwrap_or_else(|e| panic!("v{version} fixture が読めない: {e}"))
            .document;
        assert_eq!(doc.constraint_count(), 0, "v{version}");
        assert!(export_document(&doc).constraints.is_empty(), "v{version}");
    }
    for name in ["v7_legacy_boms.mcad", "v8_before_col_aligns.mcad"] {
        let doc = from_json(&fixture_named(name)).unwrap().document;
        assert_eq!(doc.constraint_count(), 0, "{name}");
    }
}

// ---------------------------------------------------------------------
// バージョン横断
// ---------------------------------------------------------------------

#[test]
fn fixtures_before_v8_read_tables_as_table_and_use_bom_as_on() {
    // v7 以前は種類・「部品表を使う」・列ごとの揃えを持たないので、表はすべて Table、
    // 「部品表を使う」は ON（DESIGN.md M12 設計判断1）、揃えはすべての列で左
    // （DESIGN.md M12 設計判断11）。
    for version in 1..8 {
        let doc = from_json(&fixture(version))
            .unwrap_or_else(|e| panic!("v{version} fixture が読めない: {e}"))
            .document;
        assert!(doc.sheet().use_bom, "v{version}: use_bom は ON で読むべき");
        assert!(
            table_kinds(&doc).iter().all(|k| *k == TableKind::Table),
            "v{version}: 表はすべて Table で読むべき"
        );
        assert_eq!(doc.bom(), None, "v{version}: 部品表は無いはず");
        for aligns in table_aligns(&doc) {
            assert!(
                aligns.iter().all(|a| *a == HorizontalAlign::Left),
                "v{version}: 揃えはすべて左で読むべき: {aligns:?}"
            );
        }
    }
    // 旧部品表を 2 つ持つ v7 も同じ（v0.11.1 以前の部品表の見え方を変えない）。
    let doc = from_json(&fixture_named("v7_legacy_boms.mcad"))
        .unwrap()
        .document;
    let aligns = table_aligns(&doc);
    assert_eq!(aligns.len(), 2);
    for (aligns, cols) in aligns.iter().zip([5, 7]) {
        assert_eq!(*aligns, vec![HorizontalAlign::Left; cols]);
    }
}

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
    // 版ごとの代表ではない追加の fixture も、自称する版で読めて現行版へ書き出される。
    for (name, declared) in [
        ("v7_legacy_boms.mcad", 7),
        ("v8_before_col_aligns.mcad", 8),
        ("v9_full.mcad", 9),
    ] {
        let text = fixture_named(name);
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["version"], declared, "{name} が自称する版");
        let summary = from_json(&text).unwrap_or_else(|e| panic!("{name} が読めない: {e}"));
        assert_eq!(
            export_document(&summary.document).version,
            FORMAT_VERSION,
            "{name} の再書き出しは現行版であるべき"
        );
    }
}
