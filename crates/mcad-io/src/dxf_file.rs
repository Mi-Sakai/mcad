//! DXF（Drawing Exchange Format）の export/import。
//!
//! # 設計方針（DESIGN.md 3.3）
//!
//! `dxf` クレート（v0.6.1）で LINE / CIRCLE / ARC / LWPOLYLINE / POINT と
//! レイヤーテーブルを相互変換する。未対応エンティティは読み飛ばし、無視した件数を
//! [`ImportSummary::skipped_entities`] として返す（エラーにしない）。
//!
//! [`mcad_file`](crate::mcad_file) の `.mcad`（JSON）と違い、DXF はネイティブに
//! レイヤー名参照・エンティティ種別を持つため、`.mcad` のようなポータブル DTO は
//! 経由しない。`dxf::Drawing` そのものが export 先／import 元の「ファイル形式表現」
//! を兼ねる。ただし [`Document`] の再構築は `.mcad` と同じ規律に従う: 必ず
//! [`Document::new`] から [`Command`]（`AddLayer` / `AddEntity` /
//! `SetLayerProps` / `SetCurrentLayer`）で組み立て、完了後に
//! [`Document::clear_history`] を呼ぶ（`Document` の内部フィールドへ直接触らない）。
//!
//! # レイヤー "0" の重複回避
//!
//! `dxf::Drawing::new()` は内部で `normalize()` を呼び、`$CLAYER`（既定 `"0"`）に
//! 対応するレイヤーを自動的に 1 つ追加する。一方 `mcad_core::Document::new()` の
//! デフォルトレイヤーも常に名前 `"0"` を持つため、何もせず `add_layer` すると
//! `"0"` という名前のレイヤーが 2 つできてしまう。これを避けるため、export では
//! `Drawing::new()` 直後に自動追加されたレイヤーをすべて取り除いてから、
//! `Document` のレイヤーだけを追加する。
//!
//! # カレントレイヤー
//!
//! DXF のヘッダ変数 `$CLAYER`（`Drawing::header::current_layer`）にカレント
//! レイヤー名を書き込み、import 時にその名前のレイヤーが見つかればカレントに
//! 設定する（見つからなければデフォルトレイヤーのままにする）。これは DXF の
//! 標準的な仕組みを使ったベストエフォートの復元であり、保証はしない。
//!
//! # レイヤーロックは保存されない
//!
//! `dxf::tables::Layer` にはロック状態に対応するフィールドが見当たらない
//! （AutoCAD の DXF 仕様上はグループコード 70 のフラグビットが担うが、この
//! クレートのバージョンでは公開されていない）。そのため DXF 往復ではロック状態を
//! 保存できず、import 時は常に `locked: false` として復元する。
//!
//! # 色は近似（ACI ⇔ RGB）
//!
//! mcad の [`Rgb`] は真の 24bit RGB だが、`dxf` クレートの `Color` は AutoCAD
//! Color Index（ACI, 1〜255 の索引）のみを表現でき、任意 RGB への変換 API を
//! 持たない。ここでは「ACI の標準的な基本 9 色（1〜9）」の固定パレットを持ち、
//! export 時は最も近い（二乗距離最小の）パレット色の ACI へ、import 時はその ACI
//! に対応する固定 RGB へ変換する。パレットに載っていない ACI（10〜255）を読んだ
//! 場合は中間グレーで近似する。
//!
//! これは往復の完全一致を **保証しない**（パレットに厳密に一致する色を使った
//! 場合のみ完全往復する）。任意の RGB を ACI へ丸めて DXF に書き出す方式（設計上の
//! 選択肢 (a)）と、色比較を往復テストから除外する方式（選択肢 (b)）のどちらも
//! 許容されると判断したが、他の CAD ソフトで DXF を開いたときに色がまったくの
//! `by_layer` 一色に潰れるより、基本色だけでも近い色が付く方が実用上親切だと
//! 考え、(a) 側（固定パレットによる近似）を選んだ。
//!
//! # 未対応エンティティ・不正ジオメトリの扱い
//!
//! - `EntityType` のうち Line/Circle/Arc/LwPolyline/ModelPoint 以外（TEXT や
//!   DIMENSION など）は無視し、`skipped_entities` としてカウントする。
//! - 対応エンティティ種別であっても、非有限座標・負半径など
//!   [`crate::mcad_file`] の `validate_shape` と同じ条件でジオメトリが不正な
//!   ものは、**読込全体を失敗させず** 無視してカウントに含める。DXF は他の CAD
//!   ソフトが吐き出したファイルであることも多く、壊れたエンティティ 1 件で
//!   ファイル全体の import を失敗させたくないため。ただし DXF ファイル自体が
//!   壊れている（構文が壊れている、テーブルが読めない等）場合は
//!   `dxf::DxfError` 由来の [`crate::IoError::Dxf`] として失敗する
//!   （こちらは個別エンティティの問題ではなく全体構造の異常）。
//!
//! # 未知のレイヤー名を参照するエンティティ
//!
//! DXF ファイルによっては、LAYER テーブルに列挙されていないレイヤー名を
//! エンティティが直接参照することがある（各種 CAD ソフトの出力・手書き DXF で
//! 起こりうる）。これを無視するとエンティティごと復元できなくなってしまうため、
//! 未知のレイヤー名を見つけた時点でその場に新しいレイヤーを追加し、そちらへ
//! 割り当てる（復元性を優先する）。

use std::collections::HashMap;
use std::path::Path;

use dxf::entities::{
    Arc as DxfArc, Circle as DxfCircle, Entity as DxfEntity, EntityType, Line as DxfLine,
    LwPolyline, ModelPoint,
};
use dxf::tables::Layer as DxfLayer;
use dxf::{Color, Drawing, LwPolylineVertex, Point as DxfPoint};

use mcad_core::{Command, Document, Entity, Layer, LayerId, Rgb, Style};
use mcad_geom::{Arc, Circle, LineSeg, Point2, Polyline, Shape};

use crate::IoError;
use crate::mcad_file::validate_shape;

/// DXF ACI の標準的な基本色パレット（1〜9）と、それぞれに割り当てる近似 RGB。
///
/// 色番号の慣例的な意味づけ（AutoCAD の標準パレット）に基づく。7 は本来
/// 背景色に応じて白／黒のどちらかとして描画されるが、ここでは白として扱う。
const ACI_PALETTE: [(u8, Rgb); 9] = [
    (1, Rgb::new(255, 0, 0)),     // 赤
    (2, Rgb::new(255, 255, 0)),   // 黄
    (3, Rgb::new(0, 255, 0)),     // 緑
    (4, Rgb::new(0, 255, 255)),   // シアン
    (5, Rgb::new(0, 0, 255)),     // 青
    (6, Rgb::new(255, 0, 255)),   // マゼンタ
    (7, Rgb::new(255, 255, 255)), // 白（慣例上は白/黒）
    (8, Rgb::new(65, 65, 65)),    // 濃灰
    (9, Rgb::new(128, 128, 128)), // 灰
];

/// パレットに載っていない ACI インデックスを読んだ場合の近似 RGB（中間グレー）。
const ACI_FALLBACK_RGB: Rgb = Rgb::new(191, 191, 191);

/// DXF import の結果。
///
/// 未対応エンティティ・不正ジオメトリのエンティティを無視するのは失敗ではないため
/// `Result` の `Err` にはせず、この構造体の `skipped_entities` として返す。
///
/// `Document` が `Debug` を実装していないため、この構造体自体も `Debug` は
/// 導出しない。
pub struct ImportSummary {
    /// 再構築されたドキュメント。
    pub document: Document,
    /// 無視したエンティティ数（未対応の種別 + 不正なジオメトリ）。
    pub skipped_entities: usize,
}

/// 2 色間の二乗距離（近似色検索に使う）。
fn color_distance_sq(a: Rgb, b: Rgb) -> i32 {
    let dr = i32::from(a.r) - i32::from(b.r);
    let dg = i32::from(a.g) - i32::from(b.g);
    let db = i32::from(a.b) - i32::from(b.b);
    dr * dr + dg * dg + db * db
}

/// RGB に最も近いパレット色の ACI インデックスへ変換する。
fn rgb_to_aci(rgb: Rgb) -> Color {
    let index = ACI_PALETTE
        .iter()
        .min_by_key(|(_, palette_rgb)| color_distance_sq(*palette_rgb, rgb))
        .map(|(index, _)| *index)
        .unwrap_or(7);
    Color::from_index(index)
}

/// ACI インデックスを固定の近似 RGB へ変換する。
fn aci_to_rgb(index: u8) -> Rgb {
    ACI_PALETTE
        .iter()
        .find(|(i, _)| *i == index)
        .map(|(_, rgb)| *rgb)
        .unwrap_or(ACI_FALLBACK_RGB)
}

/// エンティティ個別色（[`Style::color`]）を DXF の `Color` へ変換する。
///
/// `None`（レイヤー色継承）は `Color::by_layer()` として表現する。
fn style_color_to_dxf(color: Option<Rgb>) -> Color {
    match color {
        Some(rgb) => rgb_to_aci(rgb),
        None => Color::by_layer(),
    }
}

/// DXF の `Color` をエンティティ個別色（[`Style::color`]）へ変換する。
///
/// `by_layer` はレイヤー色継承（`None`）として扱う。`by_block` / `by_entity` /
/// 消灯（負値）はこのクレートの対応範囲では意味を持たないため、同様に
/// レイヤー継承として扱う。
fn dxf_color_to_style(color: &Color) -> Option<Rgb> {
    color.index().map(aci_to_rgb)
}

fn to_dxf_point(p: Point2) -> DxfPoint {
    DxfPoint::new(p.x, p.y, 0.0)
}

fn from_dxf_point(p: &DxfPoint) -> Point2 {
    Point2::new(p.x, p.y)
}

/// [`Shape`] を対応する DXF エンティティへ変換する。
fn shape_to_dxf_entity(shape: &Shape) -> DxfEntity {
    let specific = match shape {
        Shape::Point(p) => EntityType::ModelPoint(ModelPoint::new(to_dxf_point(*p))),
        Shape::Line(l) => EntityType::Line(DxfLine {
            p1: to_dxf_point(l.a),
            p2: to_dxf_point(l.b),
            ..Default::default()
        }),
        Shape::Circle(c) => EntityType::Circle(DxfCircle {
            center: to_dxf_point(c.center),
            radius: c.radius,
            ..Default::default()
        }),
        Shape::Arc(a) => EntityType::Arc(DxfArc {
            center: to_dxf_point(a.center),
            radius: a.radius,
            start_angle: a.start_angle.to_degrees(),
            end_angle: a.end_angle.to_degrees(),
            ..Default::default()
        }),
        Shape::Polyline(pl) => {
            let mut flags = 0;
            if pl.closed {
                flags |= 1;
            }
            let vertices = pl
                .vertices
                .iter()
                .map(|v| LwPolylineVertex {
                    x: v.x,
                    y: v.y,
                    bulge: 0.0,
                    ..Default::default()
                })
                .collect();
            EntityType::LwPolyline(LwPolyline {
                flags,
                vertices,
                ..Default::default()
            })
        }
    };
    DxfEntity::new(specific)
}

/// DXF エンティティの `specific` を [`Shape`] へ変換する。
///
/// 対応していない `EntityType`、または非有限座標・負半径など
/// [`validate_shape`] が拒否する不正なジオメトリは `None` を返す
/// （呼び出し側で「無視」としてカウントする）。
fn dxf_entity_to_shape(specific: &EntityType) -> Option<Shape> {
    let shape = match specific {
        EntityType::ModelPoint(p) => Shape::Point(from_dxf_point(&p.location)),
        EntityType::Line(l) => {
            Shape::Line(LineSeg::new(from_dxf_point(&l.p1), from_dxf_point(&l.p2)))
        }
        EntityType::Circle(c) => Shape::Circle(Circle::new(from_dxf_point(&c.center), c.radius)),
        EntityType::Arc(a) => Shape::Arc(Arc::new(
            from_dxf_point(&a.center),
            a.radius,
            a.start_angle.to_radians(),
            a.end_angle.to_radians(),
        )),
        EntityType::LwPolyline(pl) => Shape::Polyline(Polyline::new(
            pl.vertices.iter().map(|v| Point2::new(v.x, v.y)).collect(),
            pl.is_closed(),
        )),
        _ => return None,
    };
    if validate_shape(&shape).is_ok() {
        Some(shape)
    } else {
        None
    }
}

/// `Document` を `dxf::Drawing` へ変換する。
///
/// 生存中のレイヤー・エンティティのみを列挙する（undo/redo 履歴は含めない）。
#[must_use]
pub fn export_dxf(doc: &Document) -> Drawing {
    let mut drawing = Drawing::new();

    // LWPOLYLINE（`EntityType::LwPolyline`）は AutoCAD R14 以降でしか書き出せない
    // （`dxf` クレートの既定バージョンは R12 で、それだと LWPOLYLINE は
    // `save_file` 時に黙って読み飛ばされる）。R2000 は LWPOLYLINE 以降の主要な
    // エンティティを広くサポートし、他の CAD ソフトとの互換性も高い一般的な
    // バージョンなのでこれを使う。
    drawing.header.version = dxf::enums::AcadVersion::R2000;

    // `Drawing::new()` が自動追加するレイヤー（モジュール doc 参照）をすべて
    // 取り除き、`Document` のレイヤーだけで組み直す。
    while drawing.remove_layer(0).is_some() {}

    let mut layer_names: HashMap<LayerId, String> = HashMap::new();
    for (id, layer) in doc.layers() {
        layer_names.insert(id, layer.name.clone());
        drawing.add_layer(DxfLayer {
            name: layer.name.clone(),
            color: rgb_to_aci(layer.color),
            is_layer_on: layer.visible,
            ..Default::default()
        });
    }

    // ベストエフォート: $CLAYER にカレントレイヤー名を書いておく（import 側で
    // 復元を試みる。モジュール doc の「カレントレイヤー」を参照）。
    if let Some(name) = layer_names.get(&doc.current_layer()) {
        drawing.header.current_layer = name.clone();
    }

    for (_, entity) in doc.entities() {
        let layer_name = layer_names
            .get(&entity.layer)
            .expect("Document invariant: entity's layer must be alive")
            .clone();
        let mut dxf_entity = shape_to_dxf_entity(&entity.geom);
        dxf_entity.common.layer = layer_name;
        dxf_entity.common.color = style_color_to_dxf(entity.style.color);
        drawing.add_entity(dxf_entity);
    }

    drawing
}

/// 未知のレイヤー名を解決する。既知なら既存 `LayerId` を返し、未知ならその場で
/// `AddLayer` して登録する（モジュール doc の「未知のレイヤー名を参照する
/// エンティティ」を参照）。
fn resolve_layer(
    doc: &mut Document,
    layer_ids: &mut HashMap<String, LayerId>,
    name: &str,
) -> Result<LayerId, IoError> {
    if let Some(id) = layer_ids.get(name) {
        return Ok(*id);
    }
    let new_ids = doc.apply(Command::AddLayer(Layer::new(name, Rgb::WHITE)))?;
    let id = new_ids.layers[0];
    layer_ids.insert(name.to_string(), id);
    Ok(id)
}

/// `dxf::Drawing` から [`Document`] を再構築する。
///
/// 再構築の手順は [`crate::mcad_file::import_document`] と同じ規律に従う:
/// [`Document::new`] から [`Command`] 列で組み立て、完了後
/// [`Document::clear_history`] を呼ぶ。未対応エンティティ・不正ジオメトリは
/// 無視して [`ImportSummary::skipped_entities`] に積む（モジュール doc 参照）。
///
/// # Errors
///
/// 再構築コマンドをコアが拒否した場合（バリデーション通過後は起きない想定の
/// 防御的経路）に [`IoError::Core`] を返す。
pub fn import_dxf(drawing: &Drawing) -> Result<ImportSummary, IoError> {
    let mut doc = Document::new();
    let mut layer_ids: HashMap<String, LayerId> = HashMap::new();

    let dxf_layers: Vec<&DxfLayer> = drawing.layers().collect();
    for (index, layer) in dxf_layers.iter().enumerate() {
        let mcad_layer = Layer {
            name: layer.name.clone(),
            color: dxf_color_to_style(&layer.color).unwrap_or(ACI_FALLBACK_RGB),
            visible: layer.is_layer_on,
            // dxf::tables::Layer にロック状態のフィールドがないため常に未ロックで
            // 復元する（モジュール doc「レイヤーロックは保存されない」参照）。
            locked: false,
        };
        if index == 0 {
            let id = doc.default_layer();
            doc.apply(Command::SetLayerProps {
                id,
                props: mcad_layer,
            })?;
            layer_ids.insert(layer.name.clone(), id);
        } else {
            let new_ids = doc.apply(Command::AddLayer(mcad_layer))?;
            layer_ids.insert(layer.name.clone(), new_ids.layers[0]);
        }
    }
    // LAYER テーブルが空の DXF（テーブルを省略したミニマルなファイル）でも、
    // `Document` は常にデフォルトレイヤー "0" を持つ。これを layer_ids に
    // 登録しておかないと、後続の resolve_layer が "0" 参照のエンティティに
    // 対して重複するレイヤーを作ってしまう。
    if dxf_layers.is_empty() {
        let default_id = doc.default_layer();
        let default_name = doc
            .layer(default_id)
            .expect("default layer exists")
            .name
            .clone();
        layer_ids.insert(default_name, default_id);
    }

    let mut skipped_entities = 0usize;
    for entity in drawing.entities() {
        let Some(shape) = dxf_entity_to_shape(&entity.specific) else {
            skipped_entities += 1;
            continue;
        };
        let layer_id = resolve_layer(&mut doc, &mut layer_ids, &entity.common.layer)?;
        let style = Style {
            color: dxf_color_to_style(&entity.common.color),
            ..Style::inherited()
        };
        doc.apply(Command::AddEntity(Entity::new(shape, layer_id, style)))?;
    }

    // ベストエフォート: $CLAYER に対応するレイヤーが見つかればカレントに設定する。
    if let Some(&id) = layer_ids.get(&drawing.header.current_layer) {
        doc.apply(Command::SetCurrentLayer(id))?;
    }

    doc.clear_history();
    Ok(ImportSummary {
        document: doc,
        skipped_entities,
    })
}

/// ドキュメントを DXF ファイルへ保存する。
///
/// # Errors
///
/// DXF への書き出し失敗時に [`IoError::Dxf`] を返す。
pub fn save_dxf(doc: &Document, path: impl AsRef<Path>) -> Result<(), IoError> {
    let drawing = export_dxf(doc);
    drawing.save_file(path)?;
    Ok(())
}

/// DXF ファイルからドキュメントを読み込む。
///
/// # Errors
///
/// 読込・DXF 構文解析の失敗時に [`IoError::Dxf`] を返す。未対応エンティティ・
/// 不正ジオメトリはエラーにせず [`ImportSummary::skipped_entities`] に積む。
pub fn load_dxf(path: impl AsRef<Path>) -> Result<ImportSummary, IoError> {
    let drawing = Drawing::load_file(path)?;
    import_dxf(&drawing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dxf::entities::Text as DxfText;
    use std::fs;

    const EPS: f64 = 1e-9;

    fn approx_point(a: Point2, b: Point2) {
        assert!((a.x - b.x).abs() < EPS, "x mismatch: {a:?} vs {b:?}");
        assert!((a.y - b.y).abs() < EPS, "y mismatch: {a:?} vs {b:?}");
    }

    /// ラジアン→度→ラジアン変換の丸め誤差を許容する角度比較。
    const ANGLE_EPS: f64 = 1e-6;

    fn approx_shape(expected: &Shape, actual: &Shape) {
        match (expected, actual) {
            (Shape::Point(a), Shape::Point(b)) => approx_point(*a, *b),
            (Shape::Line(a), Shape::Line(b)) => {
                approx_point(a.a, b.a);
                approx_point(a.b, b.b);
            }
            (Shape::Circle(a), Shape::Circle(b)) => {
                approx_point(a.center, b.center);
                assert!((a.radius - b.radius).abs() < EPS);
            }
            (Shape::Arc(a), Shape::Arc(b)) => {
                approx_point(a.center, b.center);
                assert!((a.radius - b.radius).abs() < EPS);
                assert!((a.start_angle - b.start_angle).abs() < ANGLE_EPS);
                assert!((a.end_angle - b.end_angle).abs() < ANGLE_EPS);
            }
            (Shape::Polyline(a), Shape::Polyline(b)) => {
                assert_eq!(a.closed, b.closed);
                assert_eq!(a.vertices.len(), b.vertices.len());
                for (va, vb) in a.vertices.iter().zip(b.vertices.iter()) {
                    approx_point(*va, *vb);
                }
            }
            _ => panic!("shape kind mismatch: {expected:?} vs {actual:?}"),
        }
    }

    /// 全 Shape 種（Polyline は開閉両方）・複数レイヤー（可視/非表示・ロック含む）・
    /// パレット上の色（entity 個別色 1 件・レイヤー色 2 件）・非デフォルトの
    /// カレントレイヤーを持つドキュメントを作る。
    ///
    /// 色はすべて [`ACI_PALETTE`] に載っている値を選ぶことで、色についても
    /// 完全往復することを確認できるようにする（パレット外の色は近似のみで、
    /// このテストの対象ではない）。
    fn full_document() -> Document {
        let mut doc = Document::new();
        let default = doc.default_layer();
        // デフォルトレイヤー "0" の色は Rgb::WHITE = ACI 7 と一致するのでそのまま。

        let second = doc
            .apply(Command::AddLayer(Layer::new(
                "second",
                Rgb::new(0, 255, 0), // ACI 3（緑）と厳密一致
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
                false, // 開いたポリライン
            )),
            Shape::Polyline(Polyline::new(
                vec![
                    Point2::new(0.0, 0.0),
                    Point2::new(1.0, 1.0),
                    Point2::new(2.0, 0.0),
                ],
                true, // 閉じたポリライン
            )),
        ];
        for (i, shape) in shapes.into_iter().enumerate() {
            let layer = if i % 2 == 0 { default } else { second };
            let style = if i == 0 {
                Style {
                    color: Some(Rgb::new(255, 0, 0)), // ACI 1（赤）と厳密一致
                    width: 2.5,
                }
            } else {
                Style::inherited()
            };
            doc.apply(Command::AddEntity(Entity::new(shape, layer, style)))
                .unwrap();
        }

        // second をカレントにし、ロック+非表示にする（ロックは DXF 往復で
        // 保存されないため、import 後は false になることを確認する側で使う）。
        doc.apply(Command::SetCurrentLayer(second)).unwrap();
        let mut props = doc.layer(second).unwrap().clone();
        props.locked = true;
        props.visible = false;
        doc.apply(Command::SetLayerProps { id: second, props })
            .unwrap();
        doc
    }

    /// レイヤー名・可視性を比較する（ロックは DXF 往復で保存されないため
    /// 比較しない。往復後は常に false になることを別途確認する）。
    fn assert_layers_match(doc: &Document, imported: &Document) {
        assert_eq!(doc.layer_count(), imported.layer_count());
        let orig_layers: Vec<_> = doc.layers().collect();
        let imp_layers: Vec<_> = imported.layers().collect();
        for ((_, ol), (_, il)) in orig_layers.iter().zip(imp_layers.iter()) {
            assert_eq!(ol.name, il.name);
            assert_eq!(ol.visible, il.visible);
            assert!(!il.locked, "DXF 往復後のレイヤーは常に unlocked");
        }
    }

    #[test]
    fn round_trip_preserves_entities_and_layers() {
        let doc = full_document();
        let drawing = export_dxf(&doc);
        let summary = import_dxf(&drawing).unwrap();
        assert_eq!(summary.skipped_entities, 0);
        let imported = summary.document;

        assert_eq!(imported.entity_count(), doc.entity_count());
        assert_layers_match(&doc, &imported);

        // レイヤー色: パレットと厳密一致する色を選んでいるので完全往復するはず。
        let orig_layers: Vec<_> = doc.layers().map(|(_, l)| l.clone()).collect();
        let imp_layers: Vec<_> = imported.layers().map(|(_, l)| l.clone()).collect();
        for (ol, il) in orig_layers.iter().zip(imp_layers.iter()) {
            assert_eq!(ol.color, il.color, "layer {}", ol.name);
        }

        // エンティティ: 挿入順が export/import を通じて保たれる前提で、位置ごとに
        // 幾何・所属レイヤー名・個別色を比較する。
        let orig_entities: Vec<_> = doc.entities().collect();
        let imp_entities: Vec<_> = imported.entities().collect();
        assert_eq!(orig_entities.len(), imp_entities.len());
        for ((_, oe), (_, ie)) in orig_entities.iter().zip(imp_entities.iter()) {
            approx_shape(&oe.geom, &ie.geom);
            let oname = &doc.layer(oe.layer).unwrap().name;
            let iname = &imported.layer(ie.layer).unwrap().name;
            assert_eq!(oname, iname);
            assert_eq!(oe.style.color, ie.style.color);
        }

        // カレントレイヤー（$CLAYER 経由のベストエフォート復元）。
        let orig_current_name = &doc.layer(doc.current_layer()).unwrap().name;
        let imp_current_name = &imported.layer(imported.current_layer()).unwrap().name;
        assert_eq!(orig_current_name, imp_current_name);
    }

    #[test]
    fn import_leaves_history_empty() {
        let doc = full_document();
        let summary = import_dxf(&export_dxf(&doc)).unwrap();
        assert!(
            !summary.document.can_undo(),
            "読込直後に undo できてはならない"
        );
        assert!(!summary.document.can_redo());
    }

    #[test]
    fn round_trip_empty_document() {
        let doc = Document::new();
        let drawing = export_dxf(&doc);
        let summary = import_dxf(&drawing).unwrap();
        assert_eq!(summary.skipped_entities, 0);
        assert_eq!(summary.document.entity_count(), 0);
        assert_eq!(summary.document.layer_count(), 1);
        assert_eq!(
            summary
                .document
                .layer(summary.document.default_layer())
                .unwrap()
                .name,
            "0"
        );
    }

    #[test]
    fn unsupported_entity_is_skipped_and_counted() {
        let doc = full_document();
        let mut drawing = export_dxf(&doc);
        let before = doc.entity_count();

        // TEXT は対応外の EntityType。無視されてカウントされることを確認する。
        let mut text_entity = DxfEntity::new(EntityType::Text(DxfText::default()));
        text_entity.common.layer = "0".to_string();
        drawing.add_entity(text_entity);

        let summary = import_dxf(&drawing).unwrap();
        assert_eq!(summary.skipped_entities, 1);
        assert_eq!(summary.document.entity_count(), before);
    }

    #[test]
    fn unknown_layer_reference_is_added_on_the_fly() {
        // LAYER テーブルには存在しないレイヤー名を直接参照するエンティティ。
        //
        // 注意: `Drawing::add_entity` はビルダー API 経由で呼ぶと内部で
        // `ensure_layer_is_present` を呼び、参照するレイヤー名を自動的に
        // LAYER テーブルへ追加してしまう（このクレートを使ってプログラム的に
        // 組み立てる分には「テーブル未登録のレイヤー参照」は起こらない）。
        // 実際の DXF ファイル読込ではテーブルとエンティティは独立して解析される
        // ため、テーブルに存在しないレイヤー名を参照するファイルがありうる。
        // これを模すため、`add_entity` でいったんデフォルトレイヤー "0" 上に
        // 追加した後、エンティティの層参照だけを "ghost" へ書き換える
        // （LAYER テーブルには "0" しか残らない）。
        let mut drawing = Drawing::new();
        while drawing.remove_layer(0).is_some() {}
        let entity = DxfEntity::new(EntityType::ModelPoint(ModelPoint::new(DxfPoint::new(
            1.0, 2.0, 0.0,
        ))));
        drawing.add_entity(entity);
        for e in drawing.entities_mut() {
            e.common.layer = "ghost".to_string();
        }

        let summary = import_dxf(&drawing).unwrap();
        assert_eq!(summary.skipped_entities, 0);
        assert_eq!(summary.document.entity_count(), 1);
        assert_eq!(summary.document.layer_count(), 2); // デフォルト "0" + "ghost"
        let (_, e) = summary.document.entities().next().unwrap();
        let layer = summary.document.layer(e.layer).unwrap();
        assert_eq!(layer.name, "ghost");
    }

    #[test]
    fn invalid_geometry_entity_is_skipped_and_counted() {
        let mut drawing = Drawing::new();
        while drawing.remove_layer(0).is_some() {}
        drawing.add_layer(DxfLayer {
            name: "0".to_string(),
            ..Default::default()
        });

        // 半径が負の CIRCLE は不正なジオメトリなので無視される。
        let mut bad_circle = DxfEntity::new(EntityType::Circle(DxfCircle {
            center: DxfPoint::origin(),
            radius: -1.0,
            ..Default::default()
        }));
        bad_circle.common.layer = "0".to_string();
        drawing.add_entity(bad_circle);

        let summary = import_dxf(&drawing).unwrap();
        assert_eq!(summary.skipped_entities, 1);
        assert_eq!(summary.document.entity_count(), 0);
    }

    #[test]
    fn save_and_load_file() {
        let dir = std::env::temp_dir().join("mcad-io-test");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("roundtrip.dxf");

        let doc = full_document();
        save_dxf(&doc, &path).unwrap();
        let summary = load_dxf(&path).unwrap();
        assert_eq!(summary.skipped_entities, 0);
        assert_eq!(summary.document.entity_count(), doc.entity_count());
        assert_layers_match(&doc, &summary.document);

        fs::remove_file(&path).ok();
    }
}
