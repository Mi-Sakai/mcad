//! DXF（Drawing Exchange Format）の export/import。
//!
//! # 設計方針（DESIGN.md 3.3）
//!
//! `dxf` クレート（v0.6.1）で LINE / CIRCLE / ARC / LWPOLYLINE / POINT / TEXT と
//! レイヤーテーブルを相互変換する。未対応エンティティは読み飛ばし、無視した件数を
//! [`ImportSummary::skipped_entities`] として返す（エラーにしない）。
//!
//! TEXT（[`mcad_core::EntityGeom::Text`]）は M6 で export（タスク25）・import
//! （タスク25b。当初 M9 予定だったが 2026-07-25 のユーザー判断で M6 へ前倒し）の
//! 双方に対応した。寸法（DimLinear/DimRadial）は export も import も非対応
//! （DXF の DIMENSION はブロック参照を伴い、対応するプリミティブがない設計判断。
//! DESIGN.md M6 設計判断5、import 対応は M9 のまま）。
//!
//! # 文字列は UTF-8 で書く（ヘッダバージョン R2007）
//!
//! export のヘッダバージョンは **R2007**（[`export_dxf`] で `$ACADVER` を設定）。
//! `dxf` クレート 0.6.1 の文字列 codec はここで分岐する:
//!
//! - `$ACADVER <= R2004`: ASCII 範囲外の文字をコードポイントごとに
//!   `\U+XXXX`（4桁大文字16進）へエスケープして書き（`escape_unicode_to_ascii`）、
//!   読込側は `$ACADVER < R2007` を WINDOWS_1252 と見なして
//!   `un_escape_ascii_to_unicode` で戻す。
//! - `$ACADVER >= R2007`: UTF-8 のまま書き、読込側も `read_as_utf8()` で
//!   UTF-8 として読む（エスケープ経路を通らない）。
//!
//! 当初は R2000 を使っていたが、上記エスケープ codec に往復でデータを壊す欠陥が
//! 3 つある（非BMP文字が化ける / 末尾バックスラッシュが消える / リテラル
//! `\U+XXXX` が 1 文字に潰れる）ことが 2026-07-25 に判明したため R2007 へ
//! 引き上げた。欠陥の詳細と、逆に R14 未満へ下げられない理由（LWPOLYLINE が
//! 黙って落ちる）は [`export_dxf`] のコード内コメントにまとめてある。
//!
//! いずれの経路でも変換はクレート側が閉じて行うため、このモジュールに自前の
//! エスケープ／アンエスケープは持たない。UTF-8 で書かれること自体は
//! `tests::save_dxf_writes_cjk_text_as_utf8` が、往復一致は
//! `tests::round_trip_preserves_cjk_and_ascii_text` と
//! `tests::round_trip_preserves_pathological_text` が固定する。
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
//! # レイヤーの重ね順は保存されない
//!
//! `dxf::tables::Layer`（クレート 0.6.1 の生成コード
//! `target/debug/build/dxf-*/out/generated/tables.rs`）のフィールドは
//! `name, handle, color, line_type_name, is_layer_plotted, line_weight,
//! is_layer_on` 等のみで、重ね順／z-index に相当するフィールドが**存在しない**
//! （実測。ロックが同クレートに無いのと同じ理由）。DXF の LAYER テーブル自体に
//! 重ね順という概念がないため、これは仕様上の制約であり回避できない。
//!
//! - **export** は、デフォルトレイヤーを LAYER テーブルの先頭に固定し、残りを
//!   [`Document::layers_in_order`](mcad_core::Document::layers_in_order) の順
//!   （[`Layer::order`](mcad_core::Layer::order) 昇順、奥→手前）で続けて
//!   `drawing.add_layer` を呼ぶ（[`export_dxf`] 参照）。先頭固定は、下記
//!   import の「テーブル先頭 = デフォルトレイヤー」規則を常に成り立たせる
//!   ためで、これを崩すと**デフォルトレイヤー（削除できない特別なレイヤー）が
//!   往復で入れ替わる**実害のあるバグになる（2026-07-26 の Codex adversarial
//!   review [high] 指摘で発覚、[`export_dxf`] のコメント参照）。
//!   デフォルト以外のレイヤー間では、DXF はこの出現順を重ね順として解釈しない
//!   ため、これは**他 CAD のレイヤー一覧表示がテーブル出現順に従った場合にだけ
//!   意味を持つ best-effort** であり、保証ではない。
//! - **import** は LAYER テーブルの出現順（`enumerate()` の `index`）を
//!   そのまま `order = index as i32` として採用する（[`import_dxf`] 参照）。
//! - したがって「mcad → DXF export → 同じ mcad で import」の往復では、
//!   デフォルト以外のレイヤーは export が出現順を重ね順に揃えているため
//!   **`order`（＝見た目の重ね順）は結果的に保たれる**。一方、**デフォルト
//!   レイヤー自身の `order` は先頭固定のため往復で失われ、re-import 後は常に
//!   `order = 0` になる**（先頭固定を優先した意図的な割り切り）。また、他 CAD
//!   がテーブルを並べ替えて保存した DXF や、LAYER テーブルを手で編集した DXF を
//!   import する場合は、その CAD が採用した出現順がそのまま `order` になるため、
//!   **mcad 側の元の重ね順とは一致しない可能性がある**（ロック・線幅と同じ
//!   「往復で失われうる」仕様）。
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
//! # 未対応エンティティ・不正ジオメトリの扱い（import）
//!
//! - `EntityType` のうち Line/Circle/Arc/LwPolyline/ModelPoint/Text 以外
//!   （DIMENSION・SPLINE・ELLIPSE・INSERT など）は無視し、`skipped_entities`
//!   としてカウントする。DIMENSION の import 対応は M9 の予定（DESIGN.md M6
//!   設計判断5）。
//! - TEXT のうち位置基準（justification）が「水平 Left かつ垂直 Baseline」以外の
//!   ものも無視してカウントする。この場合、文字位置は `location`（group code 10）
//!   ではなく alignment point（group code 11）が持つが、mcad の
//!   [`mcad_core::TextGeom`] は左寄せ・ベースライン基準の 1 点しか表現できず、
//!   逆算にはフォントメトリクスが必要で io 層には無い。詳細と将来の対応方針は
//!   [`is_text_justification_supported`] の doc を参照。
//! - 対応エンティティ種別であっても、非有限座標・負半径・非正の文字高さ・空文字列
//!   など [`mcad_core::EntityGeom::validate`] が拒否するジオメトリは、
//!   **読込全体を失敗させず** 無視してカウントに含める。DXF は他の CAD
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
    LwPolyline, ModelPoint, Text as DxfText,
};
use dxf::enums::{HorizontalTextJustification, VerticalTextJustification};
use dxf::tables::Layer as DxfLayer;
use dxf::{Color, Drawing, LwPolylineVertex, Point as DxfPoint};

use mcad_core::{
    Command, Document, Entity, EntityGeom, Layer, LayerId, Linetype, Rgb, Style, TextGeom, WidthMm,
};
use mcad_geom::{Arc, Circle, LineSeg, Point2, Polyline, Shape};

use crate::IoError;

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

/// DXF export の結果。
///
/// M6 で `Entity.geom` が [`mcad_core::EntityGeom`] 化され、Text・寸法（非 Shape
/// バリアント）を持てるようになった。Text（[`EntityGeom::Text`]）はタスク25で
/// DXF `TEXT` エンティティとして export されるが、寸法（`DimLinear` / `DimRadial`）は
/// DXF に対応するプリミティブがなく export 時にスキップされる。黙って消えると
/// 呼び出し側がデータロスに気づけないため、[`ImportSummary`] と対称に、スキップ件数を
/// 返してステータス表示できるようにする。
///
/// `dxf::Drawing` は `Debug` を導出しているが、対称性と将来の拡張余地のためこの構造体は
/// `Debug` を導出しない（[`ImportSummary`] と同じ扱い）。
pub struct ExportSummary {
    /// 生成した図面。
    pub drawing: Drawing,
    /// DXF 非対応でスキップしたエンティティ数（寸法。Text はタスク25で export 対応済み）。
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

/// DXF `TEXT` の位置基準（justification）が mcad の [`TextGeom`] で表現できるか判定する。
///
/// 受理するのは水平 = `Left`（group code 72 = 0）かつ垂直 = `Baseline`
/// （group code 73 = 0）の組み合わせだけ。どちらも DXF の既定値であり、
/// [`text_to_dxf_entity`] が書き出す mcad 自身の TEXT は常にこの組み合わせになる。
/// それ以外は [`dxf_entity_to_geom`] が `None` を返し、呼び出し側が
/// [`ImportSummary::skipped_entities`] に計上する。
///
/// # なぜ alignment point から逆算せず、スキップするのか
///
/// DXF TEXT の仕様では、水平 justification が `Left` 以外、または垂直
/// justification が `Baseline` 以外のとき、**実際の文字位置は alignment point
/// （group code 11 = [`DxfText::second_alignment_point`]）が持ち、`location`
/// （group code 10）は意味を持たない**。そのため `location` をそのまま
/// [`TextGeom::anchor`] へ入れると、外部 CAD が作った中央揃え・右揃え・非ベースライン
/// 揃えの TEXT は**間違った位置に配置される**（mcad 自身の export は常に
/// Left/Baseline なので、既存の往復テストではこの不整合を検出できない）。
///
/// では alignment point から `anchor` を逆算すればよい、とはならない。
/// [`TextGeom`] は「左寄せ・ベースライン基準の 1 点（`anchor`）」しか持たないため、
/// 例えば中央揃えの alignment point（描画される文字列の中央）から左端を求めるには
/// **文字列の描画幅、すなわちフォントメトリクスが必要**になる。フォント
/// （Noto Sans JP）を持つのは `mcad-app` であり、`mcad-io` はアーキテクチャ上の
/// 依存方向（app → io → core → geom）から app を参照できない。つまり io 層では
/// 原理的に正しい逆算ができない。「文字幅を係数で近似する」のは結局位置がずれる
/// ので、黙って誤配置する現状と同じ問題を残すだけである。
///
/// 誤った位置に黙って置くより、既存のスキップ機構へ乗せて件数をユーザーへ通知する
/// 方が「無警告のデータロスを防ぐ」方針（モジュール doc の「未対応エンティティ・
/// 不正ジオメトリの扱い（import）」）に合う、と判断した。
///
/// 将来対応するなら、io 層は justification と alignment point を素通しし、
/// フォントメトリクスを持つ app 層で `anchor` へ変換する構造が必要になる。
fn is_text_justification_supported(text: &DxfText) -> bool {
    matches!(
        text.horizontal_text_justification,
        HorizontalTextJustification::Left
    ) && matches!(
        text.vertical_text_justification,
        VerticalTextJustification::Baseline
    )
}

/// DXF エンティティの `specific` を [`EntityGeom`] へ変換する。
///
/// Shape 系（POINT / LINE / CIRCLE / ARC / LWPOLYLINE）に加え、TEXT を
/// [`EntityGeom::Text`] へ変換する（タスク25b、[`text_to_dxf_entity`] の逆写像）。
/// TEXT のうち位置基準が Left/Baseline 以外のものは変換せず `None` を返す
/// （理由は [`is_text_justification_supported`] の doc）。
///
/// 対応していない `EntityType`、または非有限座標・負半径・非正の文字高さなど
/// [`EntityGeom::validate`] が拒否する不正なジオメトリは `None` を返す
/// （呼び出し側で「無視」としてカウントする）。`Shape` バリアントの判定基準は
/// `.mcad` import（[`crate::mcad_file::import_document`]）と同一であり、
/// `EntityGeom::validate` が `Shape::validate` へ委譲するため divergence しない。
fn dxf_entity_to_geom(specific: &EntityType) -> Option<EntityGeom> {
    let geom: EntityGeom = match specific {
        EntityType::ModelPoint(p) => Shape::Point(from_dxf_point(&p.location)).into(),
        EntityType::Line(l) => {
            Shape::Line(LineSeg::new(from_dxf_point(&l.p1), from_dxf_point(&l.p2))).into()
        }
        EntityType::Circle(c) => {
            Shape::Circle(Circle::new(from_dxf_point(&c.center), c.radius)).into()
        }
        EntityType::Arc(a) => Shape::Arc(Arc::new(
            from_dxf_point(&a.center),
            a.radius,
            a.start_angle.to_radians(),
            a.end_angle.to_radians(),
        ))
        .into(),
        EntityType::LwPolyline(pl) => Shape::Polyline(Polyline::new(
            pl.vertices.iter().map(|v| Point2::new(v.x, v.y)).collect(),
            pl.is_closed(),
        ))
        .into(),
        // TEXT: text_to_dxf_entity の逆写像。`rotation` は度なのでラジアンへ戻す。
        // 書体名（`text_style_name`）・各種寸法比は mcad の TextGeom に対応する
        // フィールドがないため捨てる（DESIGN.md M6 設計判断5）。
        //
        // 位置基準（group code 72 / 73）が Left/Baseline 以外の TEXT は `location` が
        // 文字位置を持たないため、そのまま anchor に入れると誤配置になる。io 層では
        // 正しく逆算できないのでスキップする（is_text_justification_supported の doc）。
        EntityType::Text(t) => {
            if !is_text_justification_supported(t) {
                return None;
            }
            EntityGeom::Text(TextGeom {
                anchor: from_dxf_point(&t.location),
                content: t.value.clone(),
                height: t.text_height,
                angle: t.rotation.to_radians(),
            })
        }
        _ => return None,
    };
    if geom.validate().is_ok() {
        Some(geom)
    } else {
        None
    }
}

/// [`TextGeom`] を DXF `TEXT` エンティティへ変換する（タスク25）。
///
/// `anchor` → `location`、`height` → `text_height`、`angle`（ラジアン）→
/// `rotation`（度）をそのままマッピングする。1 書体のみのため `text_style_name`
/// は既定（`STANDARD`）のままにする（DESIGN.md M6 設計判断5）。逆写像は
/// [`dxf_entity_to_geom`]。
fn text_to_dxf_entity(text: &TextGeom) -> DxfEntity {
    let specific = EntityType::Text(DxfText {
        location: to_dxf_point(text.anchor),
        text_height: text.height,
        value: text.content.clone(),
        rotation: text.angle.to_degrees(),
        ..Default::default()
    });
    DxfEntity::new(specific)
}

/// `Document` を `dxf::Drawing` へ変換する。
///
/// 生存中のレイヤー・エンティティのみを列挙する（undo/redo 履歴は含めない）。
/// Text（[`EntityGeom::Text`]）は DXF `TEXT` エンティティとして export する
/// （タスク25）。寸法（[`EntityGeom`] の `DimLinear` / `DimRadial`）は DXF に
/// 対応するプリミティブがなくスキップし、その件数を
/// [`ExportSummary::skipped_entities`] に積む。
#[must_use]
pub fn export_dxf(doc: &Document) -> ExportSummary {
    let mut drawing = Drawing::new();

    // ヘッダバージョンは R2007。下限が上下 2 つの理由から挟まれており、**下げてはいけない**。
    //
    // ## R14 未満では LWPOLYLINE が黙って落ちる
    //
    // LWPOLYLINE（`EntityType::LwPolyline`）は AutoCAD R14 以降のエンティティで、
    // `dxf` クレートは書き出し時に `if version >= AcadVersion::R14` でガードしている
    // （生成コード `build/entity_generator.rs`）。クレート既定の R12 のままだと
    // ポリラインが **エラーにもならず消える**。
    //
    // ## R2004 以下では文字列 codec が往復でデータを壊す
    //
    // `dxf` 0.6.1 は `$ACADVER <= R2004` のとき文字列を `\U+XXXX`（4桁大文字16進）へ
    // エスケープして書き（`escape_unicode_to_ascii`）、読込時は `$ACADVER < R2007` を
    // WINDOWS_1252 と見なして `un_escape_ascii_to_unicode` でアンエスケープする。
    // この codec には往復でデータを壊す欠陥が 3 つあり、いずれも実測で確認した
    // （2026-07-25、Codex レビューの high 指摘が契機）:
    //
    // 1. **非BMP文字**: `😀`（U+1F600）は `\U+1F600` と書かれるが、デコーダが 16 進を
    //    4 桁ちょうどしか消費しないため `ὠ`（U+1F60）+ `0` の 2 文字に化ける。
    // 2. **末尾バックスラッシュ**: `path\` の末尾 `\` はエスケープ開始と誤認され、
    //    未完のシーケンスが flush されないまま捨てられて消える。
    // 3. **リテラル `\U+XXXX`**: 書き出し側がバックスラッシュを二重化しないため、
    //    ユーザーが入力した 7 文字の `\U+0041` が読込で `A` 1 文字に化ける。
    //
    // ## R2007 を選ぶ理由
    //
    // `$ACADVER >= R2007` なら書き出しは `text_as_ascii = false`、読込は
    // `read_as_utf8()` で UTF-8 経路に入り、上記エスケープを一切通らないため
    // 3 ケースすべてが往復一致する（`tests::round_trip_preserves_pathological_text`
    // で固定）。R2010 / R2013 / R2018 でも同じ UTF-8 経路だが、**UTF-8 経路に入る
    // 最小のバージョン**を採るのが保守的（読める CAD ソフトの範囲が最も広い）と
    // 判断して R2007 とする。DESIGN.md M6 設計判断5 参照。
    drawing.header.version = dxf::enums::AcadVersion::R2007;

    // `Drawing::new()` が自動追加するレイヤー（モジュール doc 参照）をすべて
    // 取り除き、`Document` のレイヤーだけで組み直す。
    while drawing.remove_layer(0).is_some() {}

    // レイヤーは「デフォルトレイヤーを先頭に固定し、残りを mcad の重ね順
    // （layers_in_order、奥→手前の昇順）で続ける」順で書き出す（タスク41の
    // Codex adversarial review [high] 指摘対応）。
    //
    // なぜ先頭固定が必要か: import 側は昔から「LAYER テーブルの先頭 = mcad の
    // デフォルトレイヤー（削除できないレイヤー）」という規則で `Document` を
    // 再構築する（下の import_dxf 参照）。これは layers_in_order 順で書いていた
    // 旧実装では、デフォルトレイヤーより order が小さい（奥の）レイヤーが
    // 存在すると破れる: そちらが先頭に来てしまい、import 時に「デフォルトレイヤー
    // が入れ替わる」（削除できないはずのレイヤーが変わる）という実害のある
    // バグになる。デフォルトを先頭へ固定すればこの規則は常に保たれる。
    //
    // 代償: デフォルトレイヤー自身の重ね順（order）は先頭に固定されるため、
    // DXF 往復では復元できない（re-import 時に index 0 → order = 0 になる）。
    // モジュール doc「レイヤーの重ね順は保存されない」が述べるとおり、DXF の
    // 重ね順往復はもともと best-effort で保証しないため、これは許容する
    // （デフォルト以外のレイヤーの相対順序は従来どおり保たれる）。
    let mut layer_names: HashMap<LayerId, String> = HashMap::new();
    let default_id = doc.default_layer();
    let ordered_ids: Vec<LayerId> = std::iter::once(default_id)
        .chain(
            doc.layers_in_order()
                .into_iter()
                .map(|(id, _)| id)
                .filter(|id| *id != default_id),
        )
        .collect();
    for id in ordered_ids {
        let layer = doc.layer(id).expect("Document invariant: layer is alive");
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

    let mut skipped_entities = 0usize;
    for (_, entity) in doc.entities() {
        let layer_name = layer_names
            .get(&entity.layer)
            .expect("Document invariant: entity's layer must be alive")
            .clone();
        // M6: Shape・Text は DXF エンティティへ変換する（タスク25で Text も対応）。
        // 寸法（DimLinear/DimRadial）は DXF に対応するプリミティブがないためスキップし
        // 件数を数える。件数は呼び出し側がステータス表示し、無警告のデータロスを防ぐ。
        let mut dxf_entity = match &entity.geom {
            EntityGeom::Shape(shape) => shape_to_dxf_entity(shape),
            EntityGeom::Text(text) => text_to_dxf_entity(text),
            // 寸法（DimLinear/DimRadial）と、将来 core へ追加される未知の幾何
            // （`EntityGeom` は `#[non_exhaustive]`）はまとめてスキップ側に回す。
            _ => {
                skipped_entities += 1;
                continue;
            }
        };
        dxf_entity.common.layer = layer_name;
        dxf_entity.common.color = style_color_to_dxf(entity.style.color);
        drawing.add_entity(dxf_entity);
    }

    ExportSummary {
        drawing,
        skipped_entities,
    }
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
/// TEXT は [`EntityGeom::Text`] として復元する（タスク25b）。ただし位置基準が
/// Left/Baseline 以外の TEXT はスキップ側に回る（理由は
/// `is_text_justification_supported` の doc）。
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
            // 線種・線幅の DXF 往復はタスク35c の担当。ここでは core の既定
            // （実線・0.35mm）で復元する。
            linetype: Linetype::default(),
            width_mm: WidthMm::DEFAULT,
            visible: layer.is_layer_on,
            // dxf::tables::Layer にロック状態のフィールドがないため常に未ロックで
            // 復元する（モジュール doc「レイヤーロックは保存されない」参照）。
            locked: false,
            // DXF に重ね順の概念はないため、LAYER テーブルの並び順を重ね順として
            // 採用する（未規定の反復順に依存しない決定的な規則）。
            order: i32::try_from(index).unwrap_or(i32::MAX),
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
        let Some(geom) = dxf_entity_to_geom(&entity.specific) else {
            skipped_entities += 1;
            continue;
        };
        let layer_id = resolve_layer(&mut doc, &mut layer_ids, &entity.common.layer)?;
        let style = Style {
            color: dxf_color_to_style(&entity.common.color),
            ..Style::inherited()
        };
        doc.apply(Command::AddEntity(Entity::new(geom, layer_id, style)))?;
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
/// 戻り値は DXF 非対応でスキップしたエンティティ数（寸法。Text はタスク25で
/// export 対応済みのためスキップされない）で、呼び出し側がステータス表示に使う
/// （[`ExportSummary`] 参照）。
///
/// # Errors
///
/// DXF への書き出し失敗時に [`IoError::Dxf`] を返す。
pub fn save_dxf(doc: &Document, path: impl AsRef<Path>) -> Result<usize, IoError> {
    let ExportSummary {
        drawing,
        skipped_entities,
    } = export_dxf(doc);
    drawing.save_file(path)?;
    Ok(skipped_entities)
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
    use dxf::entities::RadialDimension;
    use std::f64::consts::FRAC_PI_2;
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
                    // 線幅・線種の DXF 往復はタスク35c の担当。ここでは色だけを
                    // 個別指定し、線幅・線種は ByLayer のままにする。
                    ..Style::inherited()
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
        let export = export_dxf(&doc);
        assert_eq!(export.skipped_entities, 0);
        let summary = import_dxf(&export.drawing).unwrap();
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
            // full_document は Shape 系のみを含む（Text の往復は
            // round_trip_preserves_cjk_and_ascii_text が担当）。
            let os = oe.geom.as_shape().expect("Shape エンティティのはず");
            let is = ie.geom.as_shape().expect("Shape エンティティのはず");
            approx_shape(os, is);
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
        let summary = import_dxf(&export_dxf(&doc).drawing).unwrap();
        assert!(
            !summary.document.can_undo(),
            "読込直後に undo できてはならない"
        );
        assert!(!summary.document.can_redo());
    }

    #[test]
    fn round_trip_empty_document() {
        let doc = Document::new();
        let export = export_dxf(&doc);
        assert_eq!(export.skipped_entities, 0);
        let summary = import_dxf(&export.drawing).unwrap();
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

    /// 「未対応エンティティ種別はスキップして数える」機構の回帰テスト。
    ///
    /// タスク25b で TEXT が import 対応になったため、代表として引き続き未対応の
    /// RADIALDIMENSION（DXF の DIMENSION 系。ブロック参照を伴い M9 予定、
    /// DESIGN.md M6 設計判断5）を使う。
    #[test]
    fn unsupported_entity_is_skipped_and_counted() {
        let doc = full_document();
        let mut drawing = export_dxf(&doc).drawing;
        let before = doc.entity_count();

        let mut dim_entity =
            DxfEntity::new(EntityType::RadialDimension(RadialDimension::default()));
        dim_entity.common.layer = "0".to_string();
        drawing.add_entity(dim_entity);

        let summary = import_dxf(&drawing).unwrap();
        assert_eq!(summary.skipped_entities, 1);
        assert_eq!(summary.document.entity_count(), before);
    }

    /// 不正な TextGeom（空文字列・非正の文字高さ）も、Shape の不正ジオメトリと同じ
    /// 規律で読込全体を失敗させずスキップしてカウントする。
    ///
    /// `DxfText::default()` は `value` が空文字列・`text_height` が 0.0 で、
    /// [`EntityGeom::validate`] の両条件に触れる。
    #[test]
    fn invalid_text_entity_is_skipped_and_counted() {
        let mut drawing = Drawing::new();
        while drawing.remove_layer(0).is_some() {}
        drawing.add_layer(DxfLayer {
            name: "0".to_string(),
            ..Default::default()
        });

        let mut empty_text = DxfEntity::new(EntityType::Text(DxfText::default()));
        empty_text.common.layer = "0".to_string();
        drawing.add_entity(empty_text);

        // 内容はあるが文字高さが 0 の TEXT も不正。
        let mut zero_height = DxfEntity::new(EntityType::Text(DxfText {
            value: "ok".to_string(),
            text_height: 0.0,
            ..Default::default()
        }));
        zero_height.common.layer = "0".to_string();
        drawing.add_entity(zero_height);

        let summary = import_dxf(&drawing).unwrap();
        assert_eq!(summary.skipped_entities, 2);
        assert_eq!(summary.document.entity_count(), 0);
    }

    /// 位置基準（justification）が Left/Baseline 以外の TEXT は、`location`
    /// （group code 10）が文字位置を持たないため import せずスキップして計上する。
    ///
    /// 誤配置を防ぐための意図的な取りこぼしであり、逆算にフォントメトリクスが必要で
    /// io 層では実装できないという依存方向の制約が根拠（詳細は
    /// [`is_text_justification_supported`] の doc）。mcad 自身の export は常に
    /// Left/Baseline なので、この経路は**外部 CAD が作った DXF でしか通らない**。
    /// そのため往復テストでは検出できず、`dxf::Drawing` へ直接エンティティを
    /// 注入して確認する。
    ///
    /// 同じ図面に Left/Baseline の TEXT も混ぜ、そちらは従来どおり import され
    /// 位置・内容が保たれることを同時に固定する（判定が広すぎて正常な TEXT まで
    /// 落とす回帰を防ぐ）。
    #[test]
    fn text_with_unsupported_justification_is_skipped_and_counted() {
        let mut drawing = Drawing::new();
        while drawing.remove_layer(0).is_some() {}
        drawing.add_layer(DxfLayer {
            name: "0".to_string(),
            ..Default::default()
        });

        // ジオメトリとしては有効（非空の value・正の text_height）にしておき、
        // スキップの原因が justification だけであることを担保する。
        let base = DxfText {
            location: DxfPoint::new(1.0, 2.0, 0.0),
            text_height: 1.5,
            value: "label".to_string(),
            ..Default::default()
        };

        // 水平 justification が Left 以外（Center / Right）。文字位置は
        // second_alignment_point 側にあるので location は使えない。
        for horizontal in [
            HorizontalTextJustification::Center,
            HorizontalTextJustification::Right,
        ] {
            let mut entity = DxfEntity::new(EntityType::Text(DxfText {
                horizontal_text_justification: horizontal,
                second_alignment_point: DxfPoint::new(10.0, 20.0, 0.0),
                ..base.clone()
            }));
            entity.common.layer = "0".to_string();
            drawing.add_entity(entity);
        }

        // 垂直 justification が Baseline 以外（Middle）。水平が Left でもスキップ対象。
        let mut vertical_middle = DxfEntity::new(EntityType::Text(DxfText {
            vertical_text_justification: VerticalTextJustification::Middle,
            second_alignment_point: DxfPoint::new(10.0, 20.0, 0.0),
            ..base.clone()
        }));
        vertical_middle.common.layer = "0".to_string();
        drawing.add_entity(vertical_middle);

        // 既定（Left / Baseline）の TEXT は従来どおり import される。
        let mut supported = DxfEntity::new(EntityType::Text(base.clone()));
        supported.common.layer = "0".to_string();
        drawing.add_entity(supported);

        let summary = import_dxf(&drawing).unwrap();
        assert_eq!(
            summary.skipped_entities, 3,
            "Center / Right / 垂直 Middle の 3 件がスキップされるはず"
        );
        assert_eq!(
            summary.document.entity_count(),
            1,
            "Left/Baseline の 1 件だけが復元されるはず"
        );

        let (_, entity) = summary.document.entities().next().unwrap();
        let EntityGeom::Text(text) = &entity.geom else {
            panic!("Text エンティティとして復元されるはず: {:?}", entity.geom);
        };
        assert_eq!(text.content, "label");
        // alignment point ではなく location が anchor になる（Left/Baseline なので正しい）。
        approx_point(text.anchor, Point2::new(1.0, 2.0));
    }

    /// タスク25: Text は DXF `TEXT` エンティティとして export され、長さ寸法・半径寸法は
    /// DXF に対応するプリミティブがなくスキップされる（DESIGN.md M6 設計判断5・
    /// タスク分割表#25）。ここは export 側のフィールドマッピングのみを見る
    /// （往復は `round_trip_preserves_cjk_and_ascii_text`）。
    #[test]
    fn export_writes_text_entity_and_skips_dimensions() {
        use mcad_core::{DimLinear, DimRadial, EntityGeom, TextGeom};

        let mut doc = Document::new();
        let layer = doc.current_layer();
        // Shape 1 件は DXF へ書き出される。
        doc.apply(Command::AddEntity(Entity::new(
            Shape::Line(LineSeg::new(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0))),
            layer,
            Style::inherited(),
        )))
        .unwrap();
        // Text は DXF TEXT エンティティとして書き出される。
        doc.apply(Command::AddEntity(Entity::new(
            EntityGeom::Text(TextGeom {
                anchor: Point2::new(3.0, 4.0),
                content: "hi".into(),
                height: 2.5,
                angle: FRAC_PI_2,
            }),
            layer,
            Style::inherited(),
        )))
        .unwrap();
        // 長さ寸法・半径寸法は DXF 非対応でスキップされる。
        doc.apply(Command::AddEntity(Entity::new(
            EntityGeom::DimLinear(DimLinear {
                p1: Point2::new(0.0, 0.0),
                p2: Point2::new(2.0, 0.0),
                offset: 1.0,
            }),
            layer,
            Style::inherited(),
        )))
        .unwrap();
        doc.apply(Command::AddEntity(Entity::new(
            EntityGeom::DimRadial(DimRadial {
                center: Point2::new(0.0, 0.0),
                radius: 2.0,
                leader_angle: 0.0,
            }),
            layer,
            Style::inherited(),
        )))
        .unwrap();

        let export = export_dxf(&doc);
        // 2 件（長さ寸法 + 半径寸法）がスキップされ、Shape 1 件 + TEXT 1 件が図面へ入る。
        assert_eq!(export.skipped_entities, 2);
        assert_eq!(export.drawing.entities().count(), 2);

        let text_entity = export
            .drawing
            .entities()
            .find_map(|e| match &e.specific {
                EntityType::Text(t) => Some(t),
                _ => None,
            })
            .expect("TEXT entity must be present in the exported drawing");
        approx_point(from_dxf_point(&text_entity.location), Point2::new(3.0, 4.0));
        assert!((text_entity.text_height - 2.5).abs() < EPS);
        assert!((text_entity.rotation - 90.0).abs() < ANGLE_EPS);
        assert_eq!(text_entity.value, "hi");
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

    /// import: LAYER テーブルの出現順（`enumerate()` の `index`）がそのまま
    /// `order` になることの回帰テスト（タスク41、モジュール doc「レイヤーの
    /// 重ね順は保存されない」参照）。テーブル出現順と名前のアルファベット順が
    /// 一致しない並びを使い、名前順に引きずられていないことを確認する。
    #[test]
    fn import_assigns_order_from_layer_table_appearance() {
        let mut drawing = Drawing::new();
        while drawing.remove_layer(0).is_some() {}
        for name in ["zeta", "0", "alpha"] {
            drawing.add_layer(DxfLayer {
                name: name.to_string(),
                ..Default::default()
            });
        }

        let summary = import_dxf(&drawing).unwrap();
        let doc = summary.document;
        let ordered: Vec<_> = doc
            .layers_in_order()
            .into_iter()
            .map(|(_, l)| l.name.clone())
            .collect();
        assert_eq!(ordered, vec!["zeta", "0", "alpha"]);
    }

    /// export: レイヤーは「デフォルトレイヤーが先頭、残りは
    /// [`Document::layers_in_order`] の順（`order` 昇順、奥→手前）」で LAYER
    /// テーブルへ書き出される（タスク41、Codex adversarial review [high] 指摘対応で
    /// 先頭固定に変更）。挿入順（"0" → "second" → "third"）とも
    /// `layers_in_order`（"third" → "0" → "second"）とも異なる並びになることを
    /// 確認する: デフォルト "0" は `order` 上は "third" より奥（数値が小さい）だが、
    /// 先頭固定のためテーブル上は "third" より先に出る。
    #[test]
    fn export_writes_layers_in_layers_in_order_sequence() {
        let mut doc = Document::new();
        let default = doc.default_layer();
        let second = doc
            .apply(Command::AddLayer(Layer::new("second", Rgb::WHITE)))
            .unwrap()
            .layers[0];
        let third = doc
            .apply(Command::AddLayer(Layer::new("third", Rgb::WHITE)))
            .unwrap()
            .layers[0];

        // 挿入順は 0, second, third だが、重ね順は third, 0, second にする。
        let mut default_props = doc.layer(default).unwrap().clone();
        default_props.order = 1;
        doc.apply(Command::SetLayerProps {
            id: default,
            props: default_props,
        })
        .unwrap();
        let mut second_props = doc.layer(second).unwrap().clone();
        second_props.order = 2;
        doc.apply(Command::SetLayerProps {
            id: second,
            props: second_props,
        })
        .unwrap();
        let mut third_props = doc.layer(third).unwrap().clone();
        third_props.order = 0;
        doc.apply(Command::SetLayerProps {
            id: third,
            props: third_props,
        })
        .unwrap();

        let layers_in_order: Vec<String> = doc
            .layers_in_order()
            .into_iter()
            .map(|(_, l)| l.name.clone())
            .collect();
        assert_eq!(layers_in_order, vec!["third", "0", "second"]);

        // 書き出し順は「デフォルト "0" が先頭、残りは layers_in_order 順
        // （"third" → "second"、"0" を除いたもの）」。
        let export = export_dxf(&doc);
        let exported_names: Vec<String> = export.drawing.layers().map(|l| l.name.clone()).collect();
        assert_eq!(exported_names, vec!["0", "third", "second"]);
    }

    /// export → import の往復で、**デフォルトレイヤーの `order` が最小でない**
    /// （デフォルトより奥のレイヤーがある）図面でも、往復後に同じレイヤー
    /// （名前で判定）がデフォルトレイヤーのままであることを固定する
    /// （Codex adversarial review [high] 指摘の回帰テスト）。
    ///
    /// 修正前の実装は、export が `layers_in_order`（`order` 昇順）の順で
    /// LAYER テーブルを書いていたため、デフォルトより奥のレイヤーがあると
    /// そちらがテーブルの先頭に来てしまい、import は「テーブル先頭 = デフォルト」
    /// という規則で読むため、**最背面のレイヤーがデフォルトへ化け、元のデフォルトは
    /// 通常レイヤーになる**という実害のあるバグがあった
    /// （既存の `export_writes_layers_in_layers_in_order_sequence` は再 import
    /// していなかったためこの回帰を検出できなかった）。
    #[test]
    fn round_trip_preserves_default_layer_identity_when_default_is_not_backmost() {
        let mut doc = Document::new();
        let default = doc.default_layer(); // 名前は "0"

        // デフォルトより奥（order が小さい）レイヤーを作る。
        let behind = doc
            .apply(Command::AddLayer(Layer::new("behind", Rgb::WHITE)))
            .unwrap()
            .layers[0];
        let mut behind_props = doc.layer(behind).unwrap().clone();
        behind_props.order = -10;
        doc.apply(Command::SetLayerProps {
            id: behind,
            props: behind_props,
        })
        .unwrap();

        // デフォルトより手前のレイヤーも1枚。
        let front = doc
            .apply(Command::AddLayer(Layer::new("front", Rgb::WHITE)))
            .unwrap()
            .layers[0];
        let mut front_props = doc.layer(front).unwrap().clone();
        front_props.order = 10;
        doc.apply(Command::SetLayerProps {
            id: front,
            props: front_props,
        })
        .unwrap();

        // 前提: "behind" がデフォルトより奥にいる（これが回帰の引き金）。
        let layers_in_order: Vec<String> = doc
            .layers_in_order()
            .into_iter()
            .map(|(_, l)| l.name.clone())
            .collect();
        assert_eq!(layers_in_order, vec!["behind", "0", "front"]);

        let export = export_dxf(&doc);
        let summary = import_dxf(&export.drawing).unwrap();
        let mut imported = summary.document;

        // デフォルトレイヤーは名前で見て "0" のままであること
        // （import 側は削除できない特別なレイヤーとして "0" を扱う）。
        let imported_default_name = &imported.layer(imported.default_layer()).unwrap().name;
        assert_eq!(
            imported_default_name,
            &doc.layer(default).unwrap().name,
            "デフォルトレイヤーの同一性(名前)が往復で入れ替わってはならない"
        );
        assert_eq!(imported_default_name, "0");

        // 削除できないのが本当に "0" であることも確認する（"behind" は削除できる）。
        assert!(
            imported
                .apply(Command::RemoveLayer(imported.default_layer()))
                .is_err()
        );
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

    /// ヘッダバージョン（[`export_dxf`] 参照）が決める文字列 codec の実態を、
    /// 生成された .dxf ファイルの**バイト列**で固定する。
    ///
    /// 往復（`round_trip_preserves_cjk_and_ascii_text`）だけでは「エンコード・
    /// デコードが対称に働いた」ことしか分からず、ファイルに何が書かれるかは
    /// 固定されない。他の CAD ソフトが読める形になっているかを担保するため、
    /// ここでは生ファイルを直接見る。
    ///
    /// # 以前このテストが固定していたこと（R2000 時代）
    ///
    /// export が R2000 だった間は、`dxf` クレートが ASCII 範囲外の文字を
    /// `\U+XXXX`（4桁大文字16進）へエスケープしていたため（`text_as_ascii =
    /// header.version <= AcadVersion::R2004`）、このテストは
    /// 「ファイル全体が純 ASCII であること」「`寸法テスト` が
    /// `\U+5BF8\U+6CD5\U+30C6\U+30B9\U+30C8` として現れること」を固定していた。
    ///
    /// # 今固定していること（R2007）
    ///
    /// R2007 では UTF-8 のまま書かれ、エスケープ経路を通らない。この経路へ移した
    /// 理由（R2004 以下の codec が往復でデータを壊す 3 欠陥）は [`export_dxf`] の
    /// コメント、再発検知は `round_trip_preserves_pathological_text` を参照。
    /// ここでは「group code 1 の値行に元の文字列がそのまま UTF-8 で現れる」ことと
    /// 「`\U+` エスケープが使われていない」ことの両方を確認し、ヘッダを下げる
    /// 変更があれば必ず落ちるようにする。
    ///
    /// # このテストの位置づけ（何を証明していないか）
    ///
    /// 保存は `dxf` 0.6.1 が行い、検証はその**生バイト列**に対して行う。したがって
    /// 証明できるのは「このクレートがこのヘッダバージョンで期待どおりのバイトを
    /// 書く」ことだけであり、**外部の DXF リーダーがこのファイルを受理することの
    /// 証明ではない**。目的は codec 回帰（ヘッダを下げる・クレートを上げるなどで
    /// エスケープ経路へ戻る変化）の検知であって、相互運用性の保証ではない。
    /// 相互運用性は手動確認（LibreCAD 実機）で担保している
    /// （DESIGN.md M6 設計判断5）。
    #[test]
    fn save_dxf_writes_cjk_text_as_utf8() {
        let dir = std::env::temp_dir().join("mcad-io-test");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cjk_text.dxf");

        const CONTENT: &str = "寸法テスト";

        let mut doc = Document::new();
        let layer = doc.current_layer();
        doc.apply(Command::AddEntity(Entity::new(
            EntityGeom::Text(TextGeom {
                anchor: Point2::new(0.0, 0.0),
                content: CONTENT.into(),
                height: 1.0,
                angle: 0.0,
            }),
            layer,
            Style::inherited(),
        )))
        .unwrap();

        let skipped = save_dxf(&doc, &path).unwrap();
        assert_eq!(skipped, 0);

        // R2007 の DXF は UTF-8 テキストなので、そのままデコードできる。
        let text = String::from_utf8(fs::read(&path).unwrap()).unwrap();

        // group code 1（TEXT の値）の直後の行が、エスケープされていない元の文字列。
        let value_lines: Vec<&str> = text
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .windows(2)
            .filter(|w| w[0].trim() == "1")
            .map(|w| w[1])
            .collect();
        assert!(
            value_lines.contains(&CONTENT),
            "group code 1 の値行に UTF-8 のままの {CONTENT:?} が見つからない: {value_lines:?}"
        );

        // `\U+XXXX` エスケープ経路（R2004 以下）に入っていないこと。ヘッダを
        // 下げるとここで落ちる。
        assert!(
            !text.contains("\\U+"),
            "R2007 では \\U+XXXX エスケープが使われてはならない"
        );
        assert_eq!(
            export_dxf(&doc).drawing.header.version,
            dxf::enums::AcadVersion::R2007,
            "ヘッダバージョンは R2007（理由は export_dxf のコメント）"
        );

        fs::remove_file(&path).ok();
    }

    /// `dxf` 0.6.1 の R2004 以下の文字列 codec が**実際に壊した** 4 ケースの
    /// ファイル往復テスト。export のヘッダバージョンを R2007 未満へ下げると
    /// **必ずこのテストが落ちる**（それが存在理由）。
    ///
    /// R2004 以下では書き出しが `escape_unicode_to_ascii`、読込が
    /// `un_escape_ascii_to_unicode` を通る。それぞれの壊れ方（2026-07-25 実測）:
    ///
    /// 1. 非BMP文字 `😀`（U+1F600）: `\U+1F600` と書かれるが、デコーダが 16 進を
    ///    4 桁ちょうどしか消費しないため `ὠ`（U+1F60）+ `0` の 2 文字に化ける。
    /// 2. 末尾バックスラッシュ `path\`: 末尾の `\` がエスケープ開始と誤認され、
    ///    未完のシーケンスが flush されずに消える。
    /// 3. 中間のバックスラッシュ `C:\temp\a`: 上と同じ経路。単体では壊れなかったが
    ///    エスケープ開始文字を含む代表ケースとして固定する。
    /// 4. リテラル `\U+0041`（`\` + `U+0041` の 7 文字）: 書き出し側が
    ///    バックスラッシュを二重化しないため、読込で `A` 1 文字に潰れる。
    ///
    /// R2007（UTF-8 経路）ではエスケープを通らないため 4 ケースすべてが完全一致する。
    /// メモリ内往復（`export_dxf` → `import_dxf`）ではこの codec を通らないので、
    /// **必ずファイル経由**で往復させること。
    ///
    /// # このテストの位置づけ（何を証明していないか）
    ///
    /// 書き出しも読み込みも同じ `dxf` 0.6.1 が行うため、証明できるのは
    /// **同一ライブラリ内でエンコードとデコードが対称に働く**ことだけである。
    /// **外部の DXF リーダーがこのファイルを受理することの証明ではない**
    /// （対称に壊れれば往復は一致してしまう。それを補うため、ファイルに何が
    /// 書かれるかは `save_dxf_writes_cjk_text_as_utf8` が生バイトで別途固定する）。
    /// 目的は codec 回帰の検知であって相互運用性の保証ではなく、相互運用性は
    /// 手動確認（LibreCAD 実機）で担保している（DESIGN.md M6 設計判断5）。
    #[test]
    fn round_trip_preserves_pathological_text() {
        let dir = std::env::temp_dir().join("mcad-io-test");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pathological_text.dxf");

        let contents = [
            "OK 😀 done",  // 1. 非BMP（U+1F600）
            "path\\",      // 2. 末尾バックスラッシュ
            "C:\\temp\\a", // 3. 中間バックスラッシュ
            "\\U+0041",    // 4. リテラル `\U+XXXX`（7 文字。`A` に化けてはいけない）
        ];

        let mut doc = Document::new();
        let layer = doc.current_layer();
        for (i, content) in contents.iter().enumerate() {
            doc.apply(Command::AddEntity(Entity::new(
                EntityGeom::Text(TextGeom {
                    anchor: Point2::new(i as f64, 0.0),
                    content: (*content).into(),
                    height: 1.0,
                    angle: 0.0,
                }),
                layer,
                Style::inherited(),
            )))
            .unwrap();
        }

        assert_eq!(save_dxf(&doc, &path).unwrap(), 0);
        let summary = load_dxf(&path).unwrap();
        assert_eq!(summary.skipped_entities, 0);
        assert_eq!(summary.document.entity_count(), contents.len());

        for (expected, (_, entity)) in contents.iter().zip(summary.document.entities()) {
            let EntityGeom::Text(actual) = &entity.geom else {
                panic!("Text エンティティとして復元されるはず: {:?}", entity.geom);
            };
            assert_eq!(
                *expected, actual.content,
                "文字列が往復で壊れた（ヘッダバージョンを下げていないか確認すること）"
            );
        }

        fs::remove_file(&path).ok();
    }

    /// タスク25b: TEXT の往復（CJK・ASCII の両方）。
    ///
    /// **ファイル経由**で往復させるのが要点。文字列のエンコード・デコードは
    /// `dxf` クレートのコードペア書き出し・読み込みで起きるため、`export_dxf` →
    /// `import_dxf` のメモリ内往復では codec をまったく通らず、CJK が正しく
    /// 書かれ読み戻されるかを検証できない。境界ケース（非BMP・バックスラッシュ）は
    /// `round_trip_preserves_pathological_text` が担当する。
    #[test]
    fn round_trip_preserves_cjk_and_ascii_text() {
        let dir = std::env::temp_dir().join("mcad-io-test");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("text_roundtrip.dxf");

        let texts = [
            TextGeom {
                anchor: Point2::new(1.5, -2.25),
                content: "寸法テスト".into(),
                height: 3.0,
                angle: FRAC_PI_2,
            },
            TextGeom {
                anchor: Point2::new(-4.0, 8.0),
                content: "Ascii Label 123".into(),
                height: 0.5,
                angle: 0.0,
            },
        ];

        let mut doc = Document::new();
        let layer = doc.current_layer();
        for text in &texts {
            doc.apply(Command::AddEntity(Entity::new(
                EntityGeom::Text(text.clone()),
                layer,
                Style::inherited(),
            )))
            .unwrap();
        }

        assert_eq!(save_dxf(&doc, &path).unwrap(), 0);
        let summary = load_dxf(&path).unwrap();
        assert_eq!(summary.skipped_entities, 0);
        assert_eq!(summary.document.entity_count(), texts.len());

        for (expected, (_, entity)) in texts.iter().zip(summary.document.entities()) {
            let EntityGeom::Text(actual) = &entity.geom else {
                panic!("Text エンティティとして復元されるはず: {:?}", entity.geom);
            };
            assert_eq!(expected.content, actual.content);
            approx_point(expected.anchor, actual.anchor);
            assert!(
                (expected.height - actual.height).abs() < EPS,
                "height mismatch: {} vs {}",
                expected.height,
                actual.height
            );
            // angle はラジアン→度→ラジアンで往復するので丸め誤差を許容する。
            assert!(
                (expected.angle - actual.angle).abs() < ANGLE_EPS,
                "angle mismatch: {} vs {}",
                expected.angle,
                actual.angle
            );
        }

        fs::remove_file(&path).ok();
    }
}
