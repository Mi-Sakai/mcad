//! 出力（SVG/PDF）の中間表現（IR）とその生成（M8 タスク39、DESIGN.md M8 設計判断7）。
//!
//! # 位置づけ
//!
//! このモジュールは **egui に一切依存しない**（`egui::` を import しない）。理由は 2 つ:
//!
//! 1. タスク40 の PDF バックエンドを **同じ IR** に乗せ、バックエンドごとの分岐を
//!    作らないため（判断7「SVG と同一の中間表現を使い分岐しない」）。
//! 2. 純関数として単体テストするため（GUI コンテキスト不要）。
//!
//! app 層に置くのはフォント（Noto Sans JP）が app 層にしか無いからで、io 層へ下ろすと
//! 依存方向が逆流する（判断7）。フォントのバイト列だけを [`crate::fonts`] から借りる。
//!
//! # 座標系（バックエンドへの契約）
//!
//! IR の座標はすべて **紙 mm・原点 = 用紙左下・y-up**（[`crate::frame::FrameLayout`]・
//! ワールド座標と同じ向き）。**IR の側では y 反転をしない**:
//!
//! - SVG は y-down なので、直列化側が `y_svg = height_mm - y` を **1 箇所だけ** で適用する。
//! - PDF は元来 y-up なので、そのまま素通しできる。
//!
//! y 反転を IR に入れてしまうと PDF 側で戻す羽目になり、二重適用事故の温床になる。
//!
//! # 換算は生成側で 1 回だけ
//!
//! ワールド→紙 mm の換算（`÷ k`、`k` = [`mcad_core::Scale::world_mm_per_paper_mm`]）、
//! 線幅・破線・注記サイズの解決はすべて [`plot_page`] で終わらせる。バックエンドは
//! 幾何を書くだけで、`Document` も `Scale` も見ない。単位ごとの扱いは以下のとおり
//! （**この 3 系統の取り違えが最大の落とし穴**）:
//!
//! | 入力 | 変換 |
//! |---|---|
//! | モデル図形の座標（ワールド） | `÷ k` |
//! | [`mcad_core::TextGeom::height`]（**紙 mm**） | そのまま（anchor だけ `÷ k`） |
//! | `DimExpansion` の各値（**ワールド長**） | `÷ k`（矢先 3.0mm・文字 3.5mm になる） |
//! | [`crate::frame::FrameText::height_mm`]（**紙 mm**） | そのまま（座標も既に紙 mm） |
//! | 線幅 [`mcad_core::WidthMm`]（紙 mm） | そのまま（**1px 下限クランプなし**） |
//! | 破線パターン（紙 mm 定数） | そのまま [`PlotStroke::dash_mm`] へ |
//!
//! # 画面描画（`main.rs` の `draw_*`）との違い
//!
//! 画面専用の防御・近似は **持ち込まない**: `MIN_STROKE_PX` の 1px 下限、
//! `MIN_TEXT_PX`/`MAX_TEXT_PX` によるスキップ・頭打ち、`MIN_DASH_PERIOD_PX` の実線
//! フォールバック、`MAX_DASH_SEGMENTS`、`ARC_SEGMENTS` のポリライン近似、
//! Liang-Barsky クリップ、Point の `max(2.0)` px 下限はいずれも「画面で潰れないため」の
//! 都合であって、紙の上では誤りになる（判断5「出力側はクランプしない」）。
//!
//! **紙基準表示トグル（F9）は出力に一切影響しない**。出力は常に紙 mm 基準で、
//! 寸法注記は常に矢先 3.0mm・文字 3.5mm になる。
//!
//! 破線は幾何生成せず [`PlotStroke::dash_mm`] としてバックエンドのネイティブ機構
//! （SVG `stroke-dasharray` / PDF `d` 演算子）へ渡す。**位相はパス先頭から 0** で、
//! 画面（クリップ区間ごとに位相がリセットされる既知の近似）とは異なる — 出力の方が
//! 正確であり、この差は意図したものである。
//!
//! # 出力に含める / 含めない
//!
//! - 含む: 可視レイヤーのエンティティ（**ロックされたレイヤーは含む** — ロックは編集の
//!   禁止であって非表示ではない）、寸法、`frame_visible` が ON なら図面枠・表題欄。
//! - 含まない: グリッド・スナップマーカー・選択ハイライト・ツールプレビュー、そして
//!   **用紙縁**（`main.rs` の `draw_paper_edge`。印刷対象でない画面専用ヒントとして
//!   タスク38 が意図的に [`crate::frame::FrameLayout`] の外へ置いた）。
//! - 用紙外の図形も **クリップせずデータとして書く**（はみ出しは作図者の責任。判断7）。
//!   カリングもしない。
//!
//! # 色
//!
//! 出力色モード [`PlotColorMode`] を選択式で持つ（既定 `Monochrome`）。エンティティ色・
//! 枠/表題欄色・用紙背景色はすべてこのモードから導出する（[`plot_color`]・
//! [`frame_plot_color`]・[`PlotPage::background`]）:
//!
//! - `Monochrome`（既定）: 作図線・枠は常に黒、背景は白。
//! - `Blueprint`（青図）: 作図線・枠は常に白、背景はプルシアンブルー（[`BLUEPRINT_BACKGROUND`]）。
//! - `Color`（元の色）: [`mcad_core::Style::effective_color`] をそのまま使うが、
//!   **純白のみ黒へ再マップする**（無対策だと既定レイヤー `"0"` の白が白紙で不可視に
//!   なるため）。背景は白。
//!
//! 線色と背景色は独立の軸にせず、モード 1 つから両方を導出する（黒線/青背景のような
//! 無意味な組合せを型で排除する。DESIGN.md 7章「随時対応」の設計確定(a)）。

pub mod pdf;
pub mod svg;
pub mod text_outline;

pub use pdf::to_pdf;
pub use svg::to_svg;

use std::f64::consts::FRAC_PI_2;

use mcad_core::{Document, Entity, EntityGeom, Layer, Linetype, Rgb, TextGeom};
use mcad_geom::{Arc, Point2, Polyline, Shape, Vec2};
use serde::{Deserialize, Serialize};

use crate::dimension::{self, DimExpansion};
use crate::frame::{FrameLayout, frame_layout};
use text_outline::GlyphOutliner;

/// 出力（SVG/PDF）の色モード。図面の属性ではなく、エクスポート設定＋
/// `config.json` の既定値として持つ（印刷色は表現の選択であり図面内容ではないため、
/// `SheetMeta` には入れない。DESIGN.md 7章「随時対応」参照）。
///
/// レイヤーごと・エンティティごとの出力色オーバーライドは非対応（non-goal）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PlotColorMode {
    /// 黒（モノクロ）。作図線・枠は黒、背景は白（既定）。
    #[default]
    Monochrome,
    /// 青図（白線）。作図線・枠は白、背景はプルシアンブルー。
    Blueprint,
    /// 元の色。[`mcad_core::Style::effective_color`] をそのまま使う（純白のみ黒へ
    /// 再マップ）。枠は黒、背景は白。
    Color,
}

// ---------------------------------------------------------------------
// 紙 mm 基準の共有定数（画面描画と出力の唯一の出所）
// ---------------------------------------------------------------------

/// 破線のダッシュパターン（紙 mm 基準: ダッシュ長・ギャップ長）。
///
/// `製図規定.md` 4-1 は破線・一点鎖線・二点鎖線の「形状」（パターン長）を規定して
/// いない（線の太さのみ規定）。JIS Z 8312/ISO 128 も具体的なパターン長までは
/// 定めていないため、多くの CAD 実装が採用する一般的な値
/// （破線: 短いダッシュ + 短いギャップ）を採用する。紙 mm 基準なので、画面は
/// `k * zoom` を掛けるだけで尺度・ズームへ追従し（判断5）、出力はそのまま使える。
pub const DASH_PATTERN_MM: [f32; 2] = [3.0, 1.5];

/// 一点鎖線のダッシュパターン（紙 mm 基準: 長ダッシュ・ギャップ・ドット・ギャップ）。
///
/// 中心線は破線よりも視認性を上げるため長いダッシュ（6.0mm）を使い、短い
/// ドット（0.6mm）を挟む一般的な描画慣行に合わせた（[`DASH_PATTERN_MM`] の doc 参照）。
pub const DASH_DOT_PATTERN_MM: [f32; 4] = [6.0, 1.2, 0.6, 1.2];

/// 二点鎖線のダッシュパターン（紙 mm 基準）。[`DASH_DOT_PATTERN_MM`] にドットを
/// もう1つ加えた形（長ダッシュ・ギャップ・ドット・ギャップ・ドット・ギャップ）。
pub const DASH_DOT_DOT_PATTERN_MM: [f32; 6] = [6.0, 1.2, 0.6, 1.2, 0.6, 1.2];

/// 寸法値ラベルの文字高さ（紙 mm）。製図規定 第6章の呼び 3.5。
///
/// 画面（紙基準表示 ON）は `k` 倍してワールド長へ換算し（タスク37、`main.rs` の
/// `dim_sizes`）、出力は換算後に `÷ k` して戻すため、**紙の上では常に 3.5mm** になる。
pub const DIM_TEXT_MM: f64 = 3.5;

/// 寸法の矢先の長さ（紙 mm）。[`DIM_TEXT_MM`] と同じ扱いで、紙の上では常に 3.0mm。
pub const DIM_ARROW_MM: f64 = 3.0;

/// 青図（Blueprint）モードの用紙背景色（プルシアンブルー `#003153`）。
///
/// トーンは手動スモークテストで確認して必要なら調整する（DESIGN.md 7章
/// 「随時対応」設計確定(a)）。
const BLUEPRINT_BACKGROUND: Rgb = Rgb::new(0x00, 0x31, 0x53);

/// 図面枠・表題欄の出力色をモードから導出する。
///
/// 画面の `FRAME_COLOR`（gray150）は暗い背景で見やすくするための **表示色** であって
/// 印刷色ではない。`Blueprint` では白（プルシアンブルー背景の上で見えるように）、
/// それ以外は黒で描く。
fn frame_plot_color(mode: PlotColorMode) -> Rgb {
    match mode {
        PlotColorMode::Blueprint => Rgb::WHITE,
        PlotColorMode::Monochrome | PlotColorMode::Color => Rgb::BLACK,
    }
}

/// 出力モードから用紙背景色を導出する（[`PlotPage::background`]）。
fn background_for_mode(mode: PlotColorMode) -> Rgb {
    match mode {
        PlotColorMode::Blueprint => BLUEPRINT_BACKGROUND,
        PlotColorMode::Monochrome | PlotColorMode::Color => Rgb::WHITE,
    }
}

/// 3 次ベジエで 90° の円弧を近似するときの制御点距離係数
/// （`4/3 * tan(π/8)` ≒ 0.5523、半径に対する比）。
///
/// [`arc_bezier_control_ratio`] が一般の掃引角へ拡張した式を持ち、この定数は
/// 「90° 分割のときにその式が返す値」をテストで固定するために置いている。
// 単体テスト（`circle_becomes_four_closed_cubic_segments`）専用の期待値定数で、
// 非テストビルドでは参照されない（本体は `arc_bezier_control_ratio` を使う）。
#[cfg_attr(not(test), allow(dead_code))]
const QUARTER_ARC_KAPPA: f64 = 0.552_284_749_830_793_4;

/// 線種のダッシュパターンを紙 mm 基準で返す（`Continuous` は `None` = 実線）。
///
/// [`Linetype`] は `#[non_exhaustive]` なので、将来の追加は未知の腕として
/// ワイルドカードで実線へフォールバックする（描画が壊れるより保守的な既定）。
#[must_use]
pub fn dash_pattern_mm(linetype: Linetype) -> Option<&'static [f32]> {
    match linetype {
        Linetype::Continuous => None,
        Linetype::Dashed => Some(&DASH_PATTERN_MM),
        Linetype::DashDot => Some(&DASH_DOT_PATTERN_MM),
        Linetype::DashDotDot => Some(&DASH_DOT_DOT_PATTERN_MM),
        _ => None,
    }
}

// ---------------------------------------------------------------------
// 中間表現（IR）
// ---------------------------------------------------------------------

/// パスコマンド（紙 mm 座標、y-up）。**曲線は 3 次ベジエのみ**。
///
/// 2 次ベジエ（`QuadTo`）は持たない: CFF アウトラインは元々 3 次で、TrueType 由来の
/// 2 次は生成時に厳密な次数上げで 3 次化する（`text_outline`）。円・円弧も
/// **ネイティブ弧を使わずベジエへ落とす** — PDF にネイティブ弧が無く、使うと
/// バックエンド分岐が生じてタスク40 の前提（分岐しない）を壊すため。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathCmd {
    /// 新しいサブパスの開始点へ移動する。
    MoveTo(Point2),
    /// 現在点から直線を引く。
    LineTo(Point2),
    /// 3 次ベジエ（制御点 1・制御点 2・終点）。
    CurveTo(Point2, Point2, Point2),
    /// 現在のサブパスを閉じる（SVG の `Z`）。
    ///
    /// **先頭点の複製ではない**。閉じ形状は必ずこのコマンドで表し、閉じ角の
    /// 継ぎ目（線の結合）がバックエンド側で正しく処理されるようにする。
    Close,
}

/// 線（ストローク）の指定。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlotStroke {
    /// 線幅（**紙 mm**）。[`mcad_core::WidthMm`] の検証済み値をそのまま入れるので、
    /// 常に有限の正値（0.05〜5.0mm）。**画面の 1px 下限クランプは掛かっていない**。
    pub width_mm: f32,
    /// 線色。
    pub color: Rgb,
    /// 破線パターン（**紙 mm** のダッシュ長・ギャップ長の交互列）。`None` は実線。
    ///
    /// 位相はパス先頭から 0。バックエンドはネイティブ機構（SVG `stroke-dasharray` /
    /// PDF `d` 演算子）へそのまま渡す（幾何生成しない）。
    pub dash_mm: Option<&'static [f32]>,
}

/// 1 本のパス（複数サブパスを含みうる）。
#[derive(Debug, Clone, PartialEq)]
pub struct PlotPath {
    /// パスコマンド列（紙 mm、y-up）。
    pub cmds: Vec<PathCmd>,
    /// 輪郭線。`None` なら描かない。
    pub stroke: Option<PlotStroke>,
    /// 塗り色。`None` なら塗らない。**fill-rule は nonzero**（文字アウトラインの
    /// 穴が正しく抜けるため。SVG は既定が nonzero、PDF は `f` 演算子が nonzero）。
    pub fill: Option<Rgb>,
}

impl PlotPath {
    /// 線だけのパス。
    #[must_use]
    pub fn stroked(cmds: Vec<PathCmd>, stroke: PlotStroke) -> Self {
        Self {
            cmds,
            stroke: Some(stroke),
            fill: None,
        }
    }

    /// 塗りだけのパス（矢先・点・文字アウトライン）。
    #[must_use]
    pub fn filled(cmds: Vec<PathCmd>, color: Rgb) -> Self {
        Self {
            cmds,
            stroke: None,
            fill: Some(color),
        }
    }
}

/// 出力 1 ページ分（= 用紙 1 枚）。
///
/// **用紙全面の背景塗りつぶし自体は含まない** — SVG/PDF バックエンドの責務
/// （`svg.rs` は毎回背景 rect を描く。`pdf.rs` は `background` が白のときだけ
/// 省略する。PDF のページは元来白なので、白背景なら描かなくても既存出力と
/// 同じ見た目になる）。[`background`](PlotPage::background) はモードから導出した
/// 「その背景の上に何色で線を描くべきか」を決めるための値であって、単なる
/// 演出ではない（[`PlotColorMode`]・モジュール doc「# 色」節参照）。
#[derive(Debug, Clone, PartialEq)]
pub struct PlotPage {
    /// 用紙幅（紙 mm、向き反映済み）。
    pub width_mm: f64,
    /// 用紙高さ（紙 mm、向き反映済み）。
    pub height_mm: f64,
    /// 用紙背景色。出力モードから導出する（[`background_for_mode`]）。
    pub background: Rgb,
    /// 描画するパス列。**奥→手前の描画順**（後の要素が手前）。
    pub paths: Vec<PlotPath>,
}

// ---------------------------------------------------------------------
// 生成（純関数）
// ---------------------------------------------------------------------

/// [`Document`] を出力用の中間表現へ変換する（純関数、このモジュールの唯一の入口）。
///
/// 単位・座標系・含める要素の規約はモジュール doc を参照。`mode` は出力色モード
/// （[`PlotColorMode`]）— 隠れた既定値を持つラッパは作らないため、呼び出し側が
/// 常に明示する。
#[must_use]
pub fn plot_page(document: &Document, mode: PlotColorMode) -> PlotPage {
    let sheet = document.sheet();
    let (width_mm, height_mm) = sheet.paper_extent_mm();
    let k = sheet.scale.world_mm_per_paper_mm();
    // 1 ページで 1 度だけ解析する（文字列ごとに `Face::parse` を呼ばない）。
    let outliner = GlyphOutliner::embedded();
    let mut paths = Vec::new();

    // 枠は画面と同じく最背面（`McadApp::ui` は grid → frame → entities の順に描く）。
    if sheet.frame_visible {
        push_frame(&mut paths, &frame_layout(sheet), outliner.as_ref(), mode);
    }
    for (entity, layer) in entities_in_plot_order(document) {
        push_entity(&mut paths, entity, layer, k, outliner.as_ref(), mode);
    }

    PlotPage {
        width_mm,
        height_mm,
        background: background_for_mode(mode),
        paths,
    }
}

/// ワールド座標 → 紙 mm 座標。**y 反転はしない**（IR は y-up）。
///
/// `k` は紙 1mm あたりのワールド mm。`paper_mm_per_world_mm()` を掛けるのではなく
/// `k` で割るのは、`(x * k) / k` の形（寸法注記の往復）が丸め誤差を最小にするため。
fn world_to_paper(p: Point2, k: f64) -> Point2 {
    Point2::new(p.x / k, p.y / k)
}

/// 出力する色をモードから導出する。
///
/// - `Monochrome`: 常に黒。
/// - `Blueprint`: 常に白。
/// - `Color`: **純白のみ黒へ再マップし、他の色は素通しする**（現行規則）。既定
///   レイヤー `"0"` の色は [`Rgb::WHITE`]（暗い画面背景に合わせた表示色）なので、
///   無対策だと白紙に白線を書いて何も見えなくなる。AutoCAD の ACI 7（white/black
///   の双対）と同じ慣行で、mcad の DXF export も白は ACI 7 へ落ちる。純白以外
///   （薄いグレー等）は作図者が明示的に選んだ色とみなして触らない。
fn plot_color(mode: PlotColorMode, color: Rgb) -> Rgb {
    match mode {
        PlotColorMode::Monochrome => Rgb::BLACK,
        PlotColorMode::Blueprint => Rgb::WHITE,
        PlotColorMode::Color => {
            if color == Rgb::WHITE {
                Rgb::BLACK
            } else {
                color
            }
        }
    }
}

/// 可視レイヤーのエンティティを **描画順（奥→手前）** に並べて返す。
///
/// 順序の規則は画面（`main.rs` の `entities_in_draw_order`）と同一:
/// レイヤー間は [`Layer::order`] 昇順、同一レイヤー内（および同順位レイヤーの間）は
/// [`Document::entities`] の反復順（= 追加順）のまま（安定ソート）。
///
/// 画面版との違いは **カリングをしない** ことだけ。用紙外の図形も出力へ含める
/// （判断7: クリップせずそのまま描く）ため、可視 AABB を受け取らない。
fn entities_in_plot_order(document: &Document) -> Vec<(&Entity, &Layer)> {
    let mut drawable: Vec<(&Entity, &Layer)> = document
        .entities()
        .filter_map(|(_id, entity)| {
            let layer = document.layer(entity.layer)?;
            // ロックは編集の禁止であって非表示ではないので、出力には含める。
            layer.visible.then_some((entity, layer))
        })
        .collect();
    drawable.sort_by_key(|(_, layer)| layer.order);
    drawable
}

/// エンティティ 1 つをパスへ展開して `paths` へ追加する。
fn push_entity(
    paths: &mut Vec<PlotPath>,
    entity: &Entity,
    layer: &Layer,
    k: f64,
    outliner: Option<&GlyphOutliner>,
    mode: PlotColorMode,
) {
    let color = plot_color(mode, entity.style.effective_color(layer.color));
    let width_mm = entity.style.effective_width(layer.width_mm).mm();
    let linetype = entity.style.effective_linetype(layer.linetype);

    match &entity.geom {
        EntityGeom::Shape(shape) => {
            let stroke = PlotStroke {
                width_mm,
                color,
                dash_mm: dash_pattern_mm(linetype),
            };
            push_shape(paths, shape, stroke, k);
        }
        // `TextGeom::height` は既に紙 mm なので換算しない（anchor だけ `÷ k`）。
        EntityGeom::Text(text) => push_text(paths, text, color, k, outliner),
        // 寸法は製図慣行として常に実線で描く（線種は形状エンティティのみが対象）。
        EntityGeom::DimLinear(dim) => {
            let ex = dimension::expand_linear(dim, DIM_ARROW_MM * k, DIM_TEXT_MM * k);
            push_dim(paths, &ex, color, width_mm, k, outliner);
        }
        EntityGeom::DimRadial(dim) => {
            let ex = dimension::expand_radial(dim, DIM_ARROW_MM * k, DIM_TEXT_MM * k);
            push_dim(paths, &ex, color, width_mm, k, outliner);
        }
        // `EntityGeom` は `#[non_exhaustive]`。未知の幾何は出力しない。
        _ => {}
    }
}

/// 形状エンティティをパスへ展開する。
fn push_shape(paths: &mut Vec<PlotPath>, shape: &Shape, stroke: PlotStroke, k: f64) {
    match shape {
        // 点は塗り円で表す。半径は実効線幅（紙 mm）そのもので、画面の `max(2.0)` px
        // 下限は持ち込まない（画面で潰れないための都合であって紙の上では誤り）。
        Shape::Point(p) => {
            let center = world_to_paper(*p, k);
            let cmds = circle_cmds(center, f64::from(stroke.width_mm));
            paths.push(PlotPath::filled(cmds, stroke.color));
        }
        Shape::Line(line) => {
            let cmds = vec![
                PathCmd::MoveTo(world_to_paper(line.a, k)),
                PathCmd::LineTo(world_to_paper(line.b, k)),
            ];
            paths.push(PlotPath::stroked(cmds, stroke));
        }
        Shape::Circle(circle) => {
            let cmds = circle_cmds(world_to_paper(circle.center, k), circle.radius / k);
            paths.push(PlotPath::stroked(cmds, stroke));
        }
        Shape::Arc(arc) => {
            paths.push(PlotPath::stroked(arc_cmds(arc, k), stroke));
        }
        Shape::Polyline(polyline) => {
            if let Some(cmds) = polyline_cmds(polyline, k) {
                paths.push(PlotPath::stroked(cmds, stroke));
            }
        }
    }
}

/// Text エンティティをアウトライン（塗りパス）へ展開する。
///
/// `text.height` は **紙 mm**（判断4）なので `k` を掛けない。掛けると二重換算になる。
fn push_text(
    paths: &mut Vec<PlotPath>,
    text: &TextGeom,
    color: Rgb,
    k: f64,
    outliner: Option<&GlyphOutliner>,
) {
    push_outlined_text(
        paths,
        outliner,
        &text.content,
        world_to_paper(text.anchor, k),
        text.height,
        text.angle,
        color,
    );
}

/// 寸法の展開結果（線分・矢先・文字）をパスへ展開する。
///
/// [`DimExpansion`] の座標・長さ・文字高さは **すべてワールド長** なので `÷ k` して
/// 紙 mm へ戻す。呼び出し側が `DIM_ARROW_MM * k` / `DIM_TEXT_MM * k` を渡しているため、
/// 結果は尺度によらず常に矢先 3.0mm・文字 3.5mm になる。
fn push_dim(
    paths: &mut Vec<PlotPath>,
    ex: &DimExpansion,
    color: Rgb,
    width_mm: f32,
    k: f64,
    outliner: Option<&GlyphOutliner>,
) {
    let stroke = PlotStroke {
        width_mm,
        color,
        dash_mm: None,
    };
    for seg in &ex.segments {
        let cmds = vec![
            PathCmd::MoveTo(world_to_paper(seg[0], k)),
            PathCmd::LineTo(world_to_paper(seg[1], k)),
        ];
        paths.push(PlotPath::stroked(cmds, stroke));
    }
    for tri in &ex.arrows {
        let cmds = vec![
            PathCmd::MoveTo(world_to_paper(tri[0], k)),
            PathCmd::LineTo(world_to_paper(tri[1], k)),
            PathCmd::LineTo(world_to_paper(tri[2], k)),
            PathCmd::Close,
        ];
        paths.push(PlotPath::filled(cmds, color));
    }
    push_outlined_text(
        paths,
        outliner,
        &ex.text.content,
        world_to_paper(ex.text.anchor, k),
        // ワールド高さ → 紙 mm（`TextGeom::height` と違いここは換算が要る）。
        ex.text.height / k,
        ex.text.angle,
        color,
    );
}

/// 図面枠・表題欄をパスへ展開する。
///
/// [`FrameLayout`] は **既に紙 mm**（原点=用紙左下、y-up）なので座標変換は不要。
/// 欄文字もアウトライン化してパスにする（`FrameText::height_mm` も紙 mm なので換算不要）。
fn push_frame(
    paths: &mut Vec<PlotPath>,
    layout: &FrameLayout,
    outliner: Option<&GlyphOutliner>,
    mode: PlotColorMode,
) {
    let color = frame_plot_color(mode);
    for line in &layout.lines {
        let stroke = PlotStroke {
            width_mm: line.width_mm,
            color,
            dash_mm: None,
        };
        let cmds = vec![PathCmd::MoveTo(line.a), PathCmd::LineTo(line.b)];
        paths.push(PlotPath::stroked(cmds, stroke));
    }
    for text in &layout.texts {
        push_outlined_text(
            paths,
            outliner,
            &text.content,
            text.anchor_mm,
            text.height_mm,
            0.0,
            color,
        );
    }
}

/// 文字列を紙 mm でアウトライン化して塗りパスとして追加する（空なら何も足さない）。
///
/// `anchor`・`height_mm` は **紙 mm**。単位換算は呼び出し側の責務にしてある
/// （3 系統の高さの取り違えを型ではなく呼び出し規約で防ぐ。モジュール doc の表を参照）。
fn push_outlined_text(
    paths: &mut Vec<PlotPath>,
    outliner: Option<&GlyphOutliner>,
    content: &str,
    anchor: Point2,
    height_mm: f64,
    angle: f64,
    color: Rgb,
) {
    let Some(outliner) = outliner else {
        return;
    };
    let cmds = outliner.outline(content, anchor, height_mm, angle);
    if !cmds.is_empty() {
        paths.push(PlotPath::filled(cmds, color));
    }
}

// ---------------------------------------------------------------------
// 円・円弧の 3 次ベジエ近似
// ---------------------------------------------------------------------

/// 円周上の点（角 `theta`、CCW）。
fn circle_point(center: Point2, radius: f64, theta: f64) -> Point2 {
    Point2::new(
        center.x + radius * theta.cos(),
        center.y + radius * theta.sin(),
    )
}

/// 掃引角 `sweep` の円弧を 1 本の 3 次ベジエで近似するときの制御点距離（半径に対する比）。
///
/// `4/3 * tan(sweep/4)`。90° で [`QUARTER_ARC_KAPPA`]（≒ 0.5523）になる標準的な式。
fn arc_bezier_control_ratio(sweep: f64) -> f64 {
    4.0 / 3.0 * (sweep / 4.0).tan()
}

/// 円弧 1 セグメント（掃引 90° 以下が前提）を 3 次ベジエとして `cmds` へ足す。
/// 始点は既に現在点になっていること。
fn push_arc_bezier(cmds: &mut Vec<PathCmd>, center: Point2, radius: f64, a0: f64, a1: f64) {
    let alpha = arc_bezier_control_ratio(a1 - a0) * radius;
    let p0 = circle_point(center, radius, a0);
    let p3 = circle_point(center, radius, a1);
    // 各端点での接線方向（CCW）。
    let t0 = Vec2::new(-a0.sin(), a0.cos()) * alpha;
    let t1 = Vec2::new(-a1.sin(), a1.cos()) * alpha;
    cmds.push(PathCmd::CurveTo(p0 + t0, p3 - t1, p3));
}

/// 円を 4 分割の 3 次ベジエ + [`PathCmd::Close`] で表す。
///
/// # 近似精度
///
/// 90° 分割の最大半径誤差は実測 `2.73e-4 × r`（画面の `ARC_SEGMENTS = 64` ポリライン
/// 近似のサジッタ誤差 `1.20e-3 × r` より 4.4 倍高精度）。絶対値が製図上の最小有意寸法
/// 0.01mm を下回るのは **紙 r ≤ 36.7mm** の範囲なので、A4 いっぱいの大円（紙 r = 100mm）
/// では 0.027mm になる。それでも最も細い線幅 0.13mm の 1/5 程度で目視できず、
/// **寸法値は数値として別に出力される**ため寸法精度へは一切波及しない
/// （これは描画のなめらかさの問題であって計測精度の問題ではない）。
/// より高精度が要るなら 45° 分割（誤差 `4e-6 × r` 程度）へ落とせばよいが、
/// パスデータが倍になるだけの効果しかないので M8 では 90° 分割で確定とする。
fn circle_cmds(center: Point2, radius: f64) -> Vec<PathCmd> {
    let mut cmds = vec![PathCmd::MoveTo(circle_point(center, radius, 0.0))];
    for i in 0..4 {
        let a0 = FRAC_PI_2 * f64::from(i);
        push_arc_bezier(&mut cmds, center, radius, a0, a0 + FRAC_PI_2);
    }
    cmds.push(PathCmd::Close);
    cmds
}

/// 円弧を 90° 以下のセグメントへ等分割した 3 次ベジエ列にする（紙 mm へ換算済み）。
fn arc_cmds(arc: &Arc, k: f64) -> Vec<PathCmd> {
    // `÷ k` はワールド原点まわりの一様拡大縮小なので、円は円のまま・角度は不変。
    let center = world_to_paper(arc.center, k);
    let radius = arc.radius / k;
    let sweep = arc.sweep();
    let n = arc_segment_count(sweep);
    let step = sweep / f64::from(n);

    let mut cmds = vec![PathCmd::MoveTo(circle_point(
        center,
        radius,
        arc.start_angle,
    ))];
    for i in 0..n {
        let a0 = arc.start_angle + step * f64::from(i);
        push_arc_bezier(&mut cmds, center, radius, a0, a0 + step);
    }
    cmds
}

/// 掃引角を 90° 以下へ収めるのに必要な分割数（1〜4）。
///
/// [`Arc::sweep`] の値域は `[0, TAU)` なので 4 分割で必ず足りる。非有限・退化
/// （掃引 0）は 1 分割として扱う（退化弧は潰れた曲線として出るだけで害がない）。
fn arc_segment_count(sweep: f64) -> u32 {
    if !sweep.is_finite() || sweep <= 0.0 {
        return 1;
    }
    // `ceil` の結果は高々 4（sweep < TAU）。念のため上下ともクランプする。
    ((sweep / FRAC_PI_2).ceil() as u32).clamp(1, 4)
}

/// ポリラインをパスコマンド列にする。閉ポリラインは **先頭点を複製せず**
/// [`PathCmd::Close`] で閉じる。頂点が無ければ `None`。
fn polyline_cmds(polyline: &Polyline, k: f64) -> Option<Vec<PathCmd>> {
    let (first, rest) = polyline.vertices.split_first()?;
    let mut cmds = vec![PathCmd::MoveTo(world_to_paper(*first, k))];
    cmds.extend(rest.iter().map(|p| PathCmd::LineTo(world_to_paper(*p, k))));
    if polyline.closed && polyline.vertices.len() >= 2 {
        cmds.push(PathCmd::Close);
    }
    Some(cmds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{FRAME_BORDER_WIDTH_MM, FRAME_DIVIDER_WIDTH_MM};
    use mcad_core::{Command, DimLinear, DimRadial, Layer, Scale, SheetMeta, Style, WidthMm};
    use mcad_geom::{Circle, LineSeg};
    use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

    const T: f64 = 1e-9;

    // ---- テスト用ヘルパー ----

    /// 尺度 `num:den` の空ドキュメント（A4 横・枠非表示）。
    fn document_with_scale(num: u32, den: u32) -> Document {
        let mut document = Document::new();
        let sheet = SheetMeta {
            scale: Scale::new(num, den).unwrap(),
            ..document.sheet().clone()
        };
        document.apply(Command::SetSheet(sheet)).unwrap();
        document
    }

    /// カレントレイヤーへ幾何を追加する（スタイルは全 ByLayer）。
    fn add(document: &mut Document, geom: impl Into<EntityGeom>) {
        add_styled(document, geom, Style::inherited());
    }

    /// カレントレイヤーへスタイル付きで幾何を追加する。
    fn add_styled(document: &mut Document, geom: impl Into<EntityGeom>, style: Style) {
        let layer = document.current_layer();
        document
            .apply(Command::AddEntity(Entity::new(geom, layer, style)))
            .unwrap();
    }

    /// パスコマンド列に現れる点（制御点を含む）をすべて集める。
    fn cmd_points(cmds: &[PathCmd]) -> Vec<Point2> {
        let mut points = Vec::new();
        for cmd in cmds {
            match cmd {
                PathCmd::MoveTo(p) | PathCmd::LineTo(p) => points.push(*p),
                PathCmd::CurveTo(c1, c2, p) => points.extend([*c1, *c2, *p]),
                PathCmd::Close => {}
            }
        }
        points
    }

    /// パスの外接矩形の `(幅, 高さ)`。
    fn bbox_size(cmds: &[PathCmd]) -> (f64, f64) {
        let points = cmd_points(cmds);
        let min_x = points.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
        let max_x = points.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max);
        let min_y = points.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
        let max_y = points.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max);
        (max_x - min_x, max_y - min_y)
    }

    /// 線だけのパス（ストロークあり・塗りなし）を順序どおり取り出す。
    fn stroked(page: &PlotPage) -> Vec<&PlotPath> {
        page.paths.iter().filter(|p| p.stroke.is_some()).collect()
    }

    /// 塗りだけのパス（塗りあり・ストロークなし）を順序どおり取り出す。
    fn filled(page: &PlotPage) -> Vec<&PlotPath> {
        page.paths.iter().filter(|p| p.fill.is_some()).collect()
    }

    /// 線分パス（`MoveTo`+`LineTo`）の 2 端点。
    fn endpoints(path: &PlotPath) -> (Point2, Point2) {
        match path.cmds.as_slice() {
            [PathCmd::MoveTo(a), PathCmd::LineTo(b)] => (*a, *b),
            other => panic!("expected a 2-command line path, got {other:?}"),
        }
    }

    fn close_to(a: f64, b: f64) {
        assert!((a - b).abs() < T, "{a} != {b}");
    }

    fn point_close_to(p: Point2, x: f64, y: f64) {
        assert!(
            (p.x - x).abs() < T && (p.y - y).abs() < T,
            "{p:?} != ({x}, {y})"
        );
    }

    // ---- 1: 座標系（1:1、y-up のまま） ----

    #[test]
    fn line_at_one_to_one_keeps_world_coordinates_and_y_up() {
        let mut document = document_with_scale(1, 1);
        add(
            &mut document,
            Shape::Line(LineSeg::new(
                Point2::new(0.0, 0.0),
                Point2::new(100.0, 50.0),
            )),
        );
        let page = plot_page(&document, PlotColorMode::Color);

        // 用紙は A4 横。
        close_to(page.width_mm, 297.0);
        close_to(page.height_mm, 210.0);

        let lines = stroked(&page);
        assert_eq!(lines.len(), 1);
        let (a, b) = endpoints(lines[0]);
        point_close_to(a, 0.0, 0.0);
        // **y 反転は IR では行わない**（`210 - 50` ではなく `50`）。反転は SVG 直列化の責務。
        point_close_to(b, 100.0, 50.0);
    }

    // ---- 2: 尺度によるモデル座標の換算 ----

    #[test]
    fn model_coordinates_are_divided_by_scale_factor() {
        // 1:2（k = 2、縮小図）: ワールド 100 → 紙 50mm。
        let mut half = document_with_scale(1, 2);
        add(
            &mut half,
            Shape::Line(LineSeg::new(Point2::ORIGIN, Point2::new(100.0, 0.0))),
        );
        let (_, b) = endpoints(stroked(&plot_page(&half, PlotColorMode::Color))[0]);
        point_close_to(b, 50.0, 0.0);

        // 2:1（k = 0.5、拡大図）: ワールド 10 → 紙 20mm。
        let mut double = document_with_scale(2, 1);
        add(
            &mut double,
            Shape::Line(LineSeg::new(Point2::ORIGIN, Point2::new(10.0, 0.0))),
        );
        let (_, b) = endpoints(stroked(&plot_page(&double, PlotColorMode::Color))[0]);
        point_close_to(b, 20.0, 0.0);
    }

    // ---- 3: 注記サイズと破線パターンは尺度不変（紙 mm 固定） ----

    /// 矢先三角形 `[先端, 後端+半幅, 後端−半幅]` の長さ（先端から後端まで）。
    fn arrow_length(path: &PlotPath) -> f64 {
        let points = cmd_points(&path.cmds);
        assert_eq!(points.len(), 3, "arrow must be a triangle");
        points[0].distance(points[1].midpoint(points[2]))
    }

    #[test]
    fn dimension_annotation_sizes_and_dash_pattern_are_scale_invariant() {
        let outliner = GlyphOutliner::embedded().unwrap();
        let mut text_sizes = Vec::new();

        for (num, den) in [(1, 2), (2, 1)] {
            let mut document = document_with_scale(num, den);
            // 破線の形状（`dash_mm` の尺度不変を同時に確認する）。
            add_styled(
                &mut document,
                Shape::Line(LineSeg::new(Point2::ORIGIN, Point2::new(40.0, 0.0))),
                Style {
                    linetype: Some(Linetype::Dashed),
                    ..Style::inherited()
                },
            );
            add(
                &mut document,
                EntityGeom::DimLinear(DimLinear {
                    p1: Point2::ORIGIN,
                    p2: Point2::new(40.0, 0.0),
                    offset: 10.0,
                }),
            );
            let page = plot_page(&document, PlotColorMode::Color);

            // 破線パターンは紙 mm 定数のまま（尺度でスケールしない）。
            let dashed: Vec<&PlotPath> = stroked(&page)
                .into_iter()
                .filter(|p| p.stroke.unwrap().dash_mm.is_some())
                .collect();
            assert_eq!(dashed.len(), 1);
            assert_eq!(
                dashed[0].stroke.unwrap().dash_mm,
                Some(&DASH_PATTERN_MM[..])
            );

            // 矢先は常に紙 3.0mm。
            let arrows: Vec<&PlotPath> = filled(&page)
                .into_iter()
                .filter(|p| p.cmds.len() == 4)
                .collect();
            assert_eq!(arrows.len(), 2);
            for arrow in arrows {
                close_to(arrow_length(arrow), DIM_ARROW_MM);
            }

            // 寸法線・補助線の 3 本は実線のまま。
            let solid = stroked(&page)
                .into_iter()
                .filter(|p| p.stroke.unwrap().dash_mm.is_none())
                .count();
            assert_eq!(solid, 3);

            // 値ラベル（アウトライン）の外接矩形サイズを控える。
            let label = filled(&page)
                .into_iter()
                .find(|p| p.cmds.len() > 4)
                .expect("dimension label outline");
            text_sizes.push(bbox_size(&label.cmds));
        }

        // 尺度が違っても紙の上の文字サイズは同一。
        close_to(text_sizes[0].0, text_sizes[1].0);
        close_to(text_sizes[0].1, text_sizes[1].1);
        // かつ「紙 3.5mm で書いた '40.00'」と同じ大きさ（＝ ÷k の戻しが効いている）。
        let reference = bbox_size(&outliner.outline("40.00", Point2::ORIGIN, DIM_TEXT_MM, 0.0));
        close_to(text_sizes[0].0, reference.0);
        close_to(text_sizes[0].1, reference.1);
    }

    #[test]
    fn radial_dimension_uses_the_same_paper_sized_annotations() {
        let mut document = document_with_scale(1, 2);
        add(
            &mut document,
            EntityGeom::DimRadial(DimRadial {
                center: Point2::ORIGIN,
                radius: 20.0,
                leader_angle: 0.0,
            }),
        );
        let page = plot_page(&document, PlotColorMode::Color);
        // 引出線 1 本 + 矢先 1 つ + ラベル 1 つ。
        assert_eq!(stroked(&page).len(), 1);
        let arrows: Vec<&PlotPath> = filled(&page)
            .into_iter()
            .filter(|p| p.cmds.len() == 4)
            .collect();
        assert_eq!(arrows.len(), 1);
        close_to(arrow_length(arrows[0]), DIM_ARROW_MM);
        // 円周点 (20,0) は紙では (10,0)。
        let (center, rim) = endpoints(stroked(&page)[0]);
        point_close_to(center, 0.0, 0.0);
        point_close_to(rim, 10.0, 0.0);
    }

    // ---- 4: Text の height は紙 mm（二重換算防止） ----

    #[test]
    fn text_entity_height_stays_paper_mm_while_anchor_is_scaled() {
        let outliner = GlyphOutliner::embedded().unwrap();
        let mut document = document_with_scale(1, 2); // k = 2
        add(
            &mut document,
            EntityGeom::Text(TextGeom {
                anchor: Point2::new(100.0, 40.0),
                content: "AB".to_owned(),
                height: 3.5,
                angle: 0.0,
            }),
        );
        let page = plot_page(&document, PlotColorMode::Color);
        let outlines = filled(&page);
        assert_eq!(outlines.len(), 1);

        // anchor は ÷k、height は素通し（×k も ÷k もしない）。
        let expected = outliner.outline("AB", Point2::new(50.0, 20.0), 3.5, 0.0);
        assert!(!expected.is_empty());
        assert_eq!(outlines[0].cmds, expected);
        // 塗りのみ（アウトラインは fill-rule nonzero で塗る）。
        assert!(outlines[0].stroke.is_none());
    }

    // ---- 5: 線幅はクランプしない ----

    #[test]
    fn line_width_is_not_clamped_for_output() {
        let mut document = document_with_scale(1, 1);
        add_styled(
            &mut document,
            Shape::Line(LineSeg::new(Point2::ORIGIN, Point2::new(1.0, 0.0))),
            Style {
                width_mm: Some(WidthMm::new(0.05).unwrap()),
                ..Style::inherited()
            },
        );
        let page = plot_page(&document, PlotColorMode::Color);
        let stroke = stroked(&page)[0].stroke.unwrap();
        // 画面の 1px 下限（`MIN_STROKE_PX`）は出力には掛からない。
        assert_eq!(stroke.width_mm, 0.05);
    }

    // ---- 6: 円・円弧の 3 次ベジエ近似 ----

    #[test]
    fn circle_becomes_four_closed_cubic_segments() {
        let mut document = document_with_scale(1, 1);
        add(
            &mut document,
            Shape::Circle(Circle::new(Point2::new(5.0, 7.0), 10.0)),
        );
        let page = plot_page(&document, PlotColorMode::Color);
        let cmds = &stroked(&page)[0].cmds;

        // MoveTo + CurveTo×4 + Close。
        assert_eq!(cmds.len(), 6);
        assert!(matches!(cmds[0], PathCmd::MoveTo(_)));
        assert_eq!(cmds[5], PathCmd::Close);
        assert_eq!(
            cmds[1..5]
                .iter()
                .filter(|c| matches!(c, PathCmd::CurveTo(..)))
                .count(),
            4
        );

        let center = Point2::new(5.0, 7.0);
        let PathCmd::MoveTo(start) = cmds[0] else {
            unreachable!()
        };
        point_close_to(start, 15.0, 7.0);

        // 各セグメントの端点は円周上、制御点距離は 0.5523r。
        let mut prev = start;
        for cmd in &cmds[1..5] {
            let PathCmd::CurveTo(c1, c2, end) = *cmd else {
                unreachable!()
            };
            close_to(end.distance(center), 10.0);
            close_to(prev.distance(c1), QUARTER_ARC_KAPPA * 10.0);
            close_to(end.distance(c2), QUARTER_ARC_KAPPA * 10.0);
            prev = end;
        }
    }

    #[test]
    fn arc_splits_sweep_into_at_most_ninety_degree_segments() {
        // 分割数は掃引角で決まる（90° 以下は 1、超えたら増える）。
        assert_eq!(arc_segment_count(FRAC_PI_2), 1);
        assert_eq!(arc_segment_count(FRAC_PI_2 + 0.01), 2);
        assert_eq!(arc_segment_count(PI), 2);
        assert_eq!(arc_segment_count(PI + 0.01), 3);
        assert_eq!(arc_segment_count(TAU - 0.01), 4);
        // 退化・非有限は 1 分割へフォールバックする。
        assert_eq!(arc_segment_count(0.0), 1);
        assert_eq!(arc_segment_count(f64::NAN), 1);

        let mut document = document_with_scale(1, 1);
        // 掃引 270°（0 → 3π/2）。
        add(
            &mut document,
            Shape::Arc(Arc::new(Point2::ORIGIN, 4.0, 0.0, 3.0 * FRAC_PI_2)),
        );
        let page = plot_page(&document, PlotColorMode::Color);
        let cmds = &stroked(&page)[0].cmds;
        // MoveTo + CurveTo×3（円弧は閉じない）。
        assert_eq!(cmds.len(), 4);
        assert!(!cmds.contains(&PathCmd::Close));
        for cmd in &cmds[1..] {
            let PathCmd::CurveTo(_, _, end) = *cmd else {
                unreachable!()
            };
            close_to(end.distance(Point2::ORIGIN), 4.0);
        }
        let PathCmd::CurveTo(_, _, last) = cmds[3] else {
            unreachable!()
        };
        point_close_to(last, 0.0, -4.0);
    }

    #[test]
    fn arc_radius_and_center_follow_the_scale() {
        let mut document = document_with_scale(1, 2); // k = 2
        add(
            &mut document,
            Shape::Arc(Arc::new(Point2::new(20.0, 0.0), 8.0, 0.0, FRAC_PI_4)),
        );
        let page = plot_page(&document, PlotColorMode::Color);
        let cmds = &stroked(&page)[0].cmds;
        assert_eq!(cmds.len(), 2); // 45° は 1 セグメント。
        let PathCmd::MoveTo(start) = cmds[0] else {
            unreachable!()
        };
        // 中心 (10,0)・半径 4 の紙 mm 円弧。開始角 0 の点は (14, 0)。
        point_close_to(start, 14.0, 0.0);
        let PathCmd::CurveTo(_, _, end) = cmds[1] else {
            unreachable!()
        };
        close_to(end.distance(Point2::new(10.0, 0.0)), 4.0);
    }

    // ---- 7: 閉ポリラインは Close で閉じる ----

    #[test]
    fn closed_polyline_uses_close_instead_of_duplicating_the_first_vertex() {
        let mut document = document_with_scale(1, 1);
        let vertices = vec![
            Point2::new(0.0, 0.0),
            Point2::new(10.0, 0.0),
            Point2::new(10.0, 5.0),
        ];
        add(
            &mut document,
            Shape::Polyline(Polyline::new(vertices.clone(), true)),
        );
        let page = plot_page(&document, PlotColorMode::Color);
        let cmds = &stroked(&page)[0].cmds;

        assert_eq!(cmds.len(), 4); // MoveTo + LineTo×2 + Close（先頭点の複製なし）。
        assert_eq!(cmds[3], PathCmd::Close);
        assert_eq!(cmd_points(cmds).len(), 3);

        // 開ポリラインは Close を持たない。
        let mut open_doc = document_with_scale(1, 1);
        add(
            &mut open_doc,
            Shape::Polyline(Polyline::new(vertices, false)),
        );
        let open_page = plot_page(&open_doc, PlotColorMode::Color);
        let open_cmds = &stroked(&open_page)[0].cmds;
        assert_eq!(open_cmds.len(), 3);
        assert!(!open_cmds.contains(&PathCmd::Close));
    }

    // ---- 8: 図面枠 ----

    #[test]
    fn frame_is_emitted_only_when_visible_and_drawn_in_black() {
        let mut document = document_with_scale(1, 1);
        assert!(!document.sheet().frame_visible);
        assert!(plot_page(&document, PlotColorMode::Color).paths.is_empty());

        let sheet = SheetMeta {
            frame_visible: true,
            ..document.sheet().clone()
        };
        document.apply(Command::SetSheet(sheet)).unwrap();
        let page = plot_page(&document, PlotColorMode::Color);

        // 罫線 15 本（輪郭4 + 表題欄外枠4 + 区切り7。様式B・A4横）。
        let lines = stroked(&page);
        assert_eq!(lines.len(), 15);
        let border = lines
            .iter()
            .filter(|p| p.stroke.unwrap().width_mm == FRAME_BORDER_WIDTH_MM)
            .count();
        let divider = lines
            .iter()
            .filter(|p| p.stroke.unwrap().width_mm == FRAME_DIVIDER_WIDTH_MM)
            .count();
        assert_eq!((border, divider), (8, 7));
        for line in &lines {
            let stroke = line.stroke.unwrap();
            // 画面の gray150 ではなく黒。破線にはしない。
            assert_eq!(stroke.color, Rgb::BLACK);
            assert!(stroke.dash_mm.is_none());
        }

        // 輪郭は用紙左下から 10mm 内側（紙 mm のまま無変換で載る）。
        let corner = lines.iter().any(|p| {
            let (a, b) = endpoints(p);
            [a, b]
                .iter()
                .any(|p| (p.x - 10.0).abs() < T && (p.y - 10.0).abs() < T)
        });
        assert!(corner, "outline corner (10, 10) should be present");

        // 欄文字（既定 fields では尺度・投影法・用紙サイズの 3 つ）もアウトライン化する。
        let texts = filled(&page);
        assert_eq!(texts.len(), 3);
        for text in texts {
            assert_eq!(text.fill, Some(Rgb::BLACK));
            assert!(!text.cmds.is_empty());
        }
    }

    #[test]
    fn frame_is_behind_entities() {
        let mut document = document_with_scale(1, 1);
        let sheet = SheetMeta {
            frame_visible: true,
            ..document.sheet().clone()
        };
        document.apply(Command::SetSheet(sheet)).unwrap();
        add(
            &mut document,
            Shape::Line(LineSeg::new(Point2::ORIGIN, Point2::new(1.0, 0.0))),
        );
        let page = plot_page(&document, PlotColorMode::Color);
        // 画面（grid → frame → entities）と同じく、枠は最背面 = 配列の前方。
        let last = stroked(&page).pop().unwrap();
        let (a, b) = endpoints(last);
        point_close_to(a, 0.0, 0.0);
        point_close_to(b, 1.0, 0.0);
    }

    // ---- 9: レイヤーの可視・ロック・重ね順 ----

    /// `order` 付きレイヤーを足す。
    fn add_layer(document: &mut Document, name: &str, order: i32) -> mcad_core::LayerId {
        let mut layer = Layer::new(name, Rgb::WHITE);
        layer.order = order;
        document.apply(Command::AddLayer(layer)).unwrap().layers[0]
    }

    /// レイヤー上に x 位置で識別できる線を 1 本足す。
    fn add_line_on(document: &mut Document, layer: mcad_core::LayerId, x: f64) {
        let geom = Shape::Line(LineSeg::new(Point2::new(x, 0.0), Point2::new(x, 1.0)));
        document
            .apply(Command::AddEntity(Entity::new(
                geom,
                layer,
                Style::inherited(),
            )))
            .unwrap();
    }

    #[test]
    fn hidden_layers_are_excluded_locked_layers_are_included_and_order_is_ascending() {
        let mut document = document_with_scale(1, 1);
        let base = document.default_layer(); // order = 0
        let front = add_layer(&mut document, "front", 5);
        let back = add_layer(&mut document, "back", -5);
        let hidden = add_layer(&mut document, "hidden", 1);
        let locked = add_layer(&mut document, "locked", 2);

        // 追加順はレイヤー順とあえてずらす。
        add_line_on(&mut document, base, 0.0);
        add_line_on(&mut document, front, 1.0);
        add_line_on(&mut document, back, 2.0);
        add_line_on(&mut document, hidden, 3.0);
        add_line_on(&mut document, locked, 4.0);
        add_line_on(&mut document, base, 5.0);

        // 非表示・ロックを設定する（ロックはエンティティ追加後に掛ける）。
        for (id, visible, is_locked) in [(hidden, false, false), (locked, true, true)] {
            let mut props = document.layer(id).unwrap().clone();
            props.visible = visible;
            props.locked = is_locked;
            document
                .apply(Command::SetLayerProps { id, props })
                .unwrap();
        }

        let page = plot_page(&document, PlotColorMode::Color);
        let xs: Vec<f64> = stroked(&page).iter().map(|p| endpoints(p).0.x).collect();
        // order 昇順（back(-5) → base(0) は追加順 → locked(2) → front(5)）。
        // 非表示レイヤー（x = 3）だけが落ち、ロックレイヤー（x = 4）は残る。
        assert_eq!(xs, vec![2.0, 0.0, 5.0, 4.0, 1.0]);
    }

    // ---- 10: 色 ----

    #[test]
    fn color_mode_remaps_only_pure_white_to_black_and_passes_through_others() {
        // Color モードは現行規則（純白のみ黒へ再マップ）そのまま。
        assert_eq!(plot_color(PlotColorMode::Color, Rgb::WHITE), Rgb::BLACK);
        assert_eq!(plot_color(PlotColorMode::Color, Rgb::BLACK), Rgb::BLACK);
        // 純白以外は 1 成分違うだけでも素通し（作図者が選んだ色として尊重する）。
        let near_white = Rgb::new(254, 255, 255);
        assert_eq!(plot_color(PlotColorMode::Color, near_white), near_white);

        let mut document = document_with_scale(1, 1);
        add(
            &mut document,
            Shape::Line(LineSeg::new(Point2::ORIGIN, Point2::new(1.0, 0.0))),
        );
        add_styled(
            &mut document,
            Shape::Line(LineSeg::new(Point2::ORIGIN, Point2::new(2.0, 0.0))),
            Style {
                color: Some(Rgb::new(200, 30, 40)),
                ..Style::inherited()
            },
        );
        let page = plot_page(&document, PlotColorMode::Color);
        let lines = stroked(&page);
        assert_eq!(lines[0].stroke.unwrap().color, Rgb::BLACK);
        assert_eq!(lines[1].stroke.unwrap().color, Rgb::new(200, 30, 40));
    }

    /// モード × 色のマトリクス: `plot_color` の全 9 組（3 モード × 3 色）を固定する。
    #[test]
    fn plot_color_matrix_across_modes_and_colors() {
        let sample_colors = [Rgb::WHITE, Rgb::BLACK, Rgb::new(200, 30, 40)];

        for color in sample_colors {
            assert_eq!(plot_color(PlotColorMode::Monochrome, color), Rgb::BLACK);
            assert_eq!(plot_color(PlotColorMode::Blueprint, color), Rgb::WHITE);
        }
        assert_eq!(plot_color(PlotColorMode::Color, Rgb::WHITE), Rgb::BLACK);
        assert_eq!(plot_color(PlotColorMode::Color, Rgb::BLACK), Rgb::BLACK);
        assert_eq!(
            plot_color(PlotColorMode::Color, Rgb::new(200, 30, 40)),
            Rgb::new(200, 30, 40)
        );
    }

    /// `frame_plot_color`・`background_for_mode` の全モードの組合せを固定する。
    #[test]
    fn frame_color_and_background_follow_the_mode() {
        assert_eq!(frame_plot_color(PlotColorMode::Monochrome), Rgb::BLACK);
        assert_eq!(frame_plot_color(PlotColorMode::Blueprint), Rgb::WHITE);
        assert_eq!(frame_plot_color(PlotColorMode::Color), Rgb::BLACK);

        assert_eq!(background_for_mode(PlotColorMode::Monochrome), Rgb::WHITE);
        assert_eq!(
            background_for_mode(PlotColorMode::Blueprint),
            BLUEPRINT_BACKGROUND
        );
        assert_eq!(background_for_mode(PlotColorMode::Color), Rgb::WHITE);
    }

    /// `PlotColorMode` の既定値は `Monochrome`。
    #[test]
    fn plot_color_mode_default_is_monochrome() {
        assert_eq!(PlotColorMode::default(), PlotColorMode::Monochrome);
    }

    /// 青図モードでは作図線・矢先・文字（寸法値ラベル）・枠（罫線・欄文字）がすべて白になり、
    /// 用紙背景がプルシアンブルーになる（SVG rect・PDF 全面 rect の両方は svg.rs/pdf.rs 側の
    /// テストで確認する。ここは IR レベルでの色・背景の確認）。
    #[test]
    fn blueprint_mode_makes_lines_and_frame_white_with_prussian_blue_background() {
        let mut document = document_with_scale(1, 1);
        let sheet = SheetMeta {
            frame_visible: true,
            ..document.sheet().clone()
        };
        document.apply(Command::SetSheet(sheet)).unwrap();
        add(
            &mut document,
            Shape::Line(LineSeg::new(Point2::ORIGIN, Point2::new(10.0, 0.0))),
        );
        add(
            &mut document,
            EntityGeom::DimLinear(DimLinear {
                p1: Point2::ORIGIN,
                p2: Point2::new(10.0, 0.0),
                offset: 5.0,
            }),
        );
        let page = plot_page(&document, PlotColorMode::Blueprint);

        assert_eq!(page.background, BLUEPRINT_BACKGROUND);
        for path in &page.paths {
            if let Some(stroke) = path.stroke {
                assert_eq!(stroke.color, Rgb::WHITE, "stroke must be white");
            }
            if let Some(fill) = path.fill {
                assert_eq!(fill, Rgb::WHITE, "fill must be white");
            }
        }
        // 枠（罫線+欄文字）・作図線・寸法（線+矢先+文字）がいずれも存在すること。
        assert!(!stroked(&page).is_empty());
        assert!(!filled(&page).is_empty());
    }

    /// Monochrome・Color モードでは（既定）背景が白であることを確認する
    /// （PDF が背景 rect を出さないことは pdf.rs 側で確認する）。
    #[test]
    fn monochrome_and_color_modes_use_white_background() {
        let document = document_with_scale(1, 1);
        assert_eq!(
            plot_page(&document, PlotColorMode::Monochrome).background,
            Rgb::WHITE
        );
        assert_eq!(
            plot_page(&document, PlotColorMode::Color).background,
            Rgb::WHITE
        );
    }

    // ---- 11: グリフ ----

    #[test]
    fn embedded_font_parses() {
        assert!(GlyphOutliner::embedded().is_some());
    }

    #[test]
    fn ascii_and_cjk_glyphs_produce_non_empty_outlines() {
        let outliner = GlyphOutliner::embedded().unwrap();
        for content in ["A", "40.00", "日", "第三角法"] {
            let cmds = outliner.outline(content, Point2::ORIGIN, 3.5, 0.0);
            assert!(!cmds.is_empty(), "{content} should produce an outline");
            assert!(matches!(cmds[0], PathCmd::MoveTo(_)));
            // CFF アウトラインなので閉じた輪郭になる。
            assert!(cmds.contains(&PathCmd::Close));
        }
        // 空文字列・非正の高さは何も生まない。
        assert!(outliner.outline("", Point2::ORIGIN, 3.5, 0.0).is_empty());
        assert!(outliner.outline("A", Point2::ORIGIN, 0.0, 0.0).is_empty());
    }

    #[test]
    fn space_has_no_outline_but_still_advances_the_pen() {
        let outliner = GlyphOutliner::embedded().unwrap();
        // 空白そのものは輪郭を持たない。
        assert!(outliner.outline(" ", Point2::ORIGIN, 3.5, 0.0).is_empty());

        // それでも後続の文字は右へ進む（語間が消えない）。
        let tight = outliner.outline("AA", Point2::ORIGIN, 3.5, 0.0);
        let spaced = outliner.outline("A A", Point2::ORIGIN, 3.5, 0.0);
        let max_x = |cmds: &[PathCmd]| {
            cmd_points(cmds)
                .iter()
                .map(|p| p.x)
                .fold(f64::NEG_INFINITY, f64::max)
        };
        assert!(
            max_x(&spaced) > max_x(&tight),
            "space must advance the pen: {} vs {}",
            max_x(&spaced),
            max_x(&tight)
        );
        // 輪郭の数（= グリフ 2 つ分）は変わらない。
        assert_eq!(
            tight.iter().filter(|c| **c == PathCmd::Close).count(),
            spaced.iter().filter(|c| **c == PathCmd::Close).count()
        );
    }

    #[test]
    fn rotation_is_baked_around_the_anchor() {
        let outliner = GlyphOutliner::embedded().unwrap();
        let anchor = Point2::new(20.0, 5.0);
        let flat = outliner.outline("R", anchor, 3.5, 0.0);
        let turned = outliner.outline("R", anchor, 3.5, FRAC_PI_2);
        assert_eq!(flat.len(), turned.len());

        // ワールド CCW +π/2: (dx, dy) → (−dy, dx)。**符号反転（画面用の a = −θ）はしない。**
        for (a, b) in cmd_points(&flat).iter().zip(cmd_points(&turned).iter()) {
            let d = *a - anchor;
            close_to(b.x, anchor.x - d.y);
            close_to(b.y, anchor.y + d.x);
        }
    }

    #[test]
    fn missing_glyphs_are_skipped_but_still_advance() {
        let outliner = GlyphOutliner::embedded().unwrap();
        // 私用領域の文字はフォントに無い（tofu も出さずに送りだけ進める）。
        let missing = outliner.outline("\u{E000}", Point2::ORIGIN, 3.5, 0.0);
        assert!(missing.is_empty());
        let after_missing = outliner.outline("\u{E000}A", Point2::ORIGIN, 3.5, 0.0);
        let plain = outliner.outline("A", Point2::ORIGIN, 3.5, 0.0);
        let min_x = |cmds: &[PathCmd]| {
            cmd_points(cmds)
                .iter()
                .map(|p| p.x)
                .fold(f64::INFINITY, f64::min)
        };
        assert!(min_x(&after_missing) > min_x(&plain));
    }

    // ---- 12: 点エンティティ ----

    #[test]
    fn point_entity_becomes_a_filled_circle_of_line_width_radius() {
        let mut document = document_with_scale(1, 2); // k = 2
        add(&mut document, Shape::Point(Point2::new(10.0, 20.0)));
        let page = plot_page(&document, PlotColorMode::Color);

        assert!(stroked(&page).is_empty());
        let dots = filled(&page);
        assert_eq!(dots.len(), 1);
        assert_eq!(dots[0].fill, Some(Rgb::BLACK));
        // 円（MoveTo + CurveTo×4 + Close）。
        assert_eq!(dots[0].cmds.len(), 6);

        // 中心は ÷k、半径は実効線幅（既定 0.35mm）。画面の 2px 下限は持ち込まない。
        let center = Point2::new(5.0, 10.0);
        let PathCmd::MoveTo(start) = dots[0].cmds[0] else {
            unreachable!()
        };
        close_to(start.distance(center), f64::from(WidthMm::DEFAULT.mm()));
    }
}
