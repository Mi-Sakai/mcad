//! `mcad-app` — eguiアプリ本体（バイナリ）。
//!
//! M2タスク4（Viewport+描画）: 座標変換・ズーム/パン・グリッド表示・
//! エンティティ描画（カリング付き）を実装する。
//! M2タスク5（Toolフレームワーク+作図ツール）: Point/Line/Circle/Arc/Polyline の
//! 各ツール（`tool.rs`）を統合し、キーボードショートカットで切り替え、
//! クリック/Enter/Escで確定・キャンセルできるようにする。後続タスクで
//! 選択・編集ツールとスナップエンジンを追加する。

mod config;
mod dimension;
mod fonts;
mod frame;
mod ortho;
mod plot;
mod snap;
mod tool;
mod viewport;

use std::path::{Path, PathBuf};

use egui::{Color32, Key, Pos2, Rect, Stroke};

use mcad_core::{
    ArrowPlacement, Command, DimAnnotation, DimDiameter, DimKind, DimLinear, DimRadial, DimStyle,
    Document, Entity, EntityGeom, EntityId, FitClass, Layer, LayerId, Linetype, MAX_DIM_DECIMALS,
    Orientation, PaperSize, ProjectionMethod, Rgb, Scale, SheetMeta, SizeTolerance, Style,
    TextGeom, TitleBlockFields, TitleBlockKind, WidthMm,
};
use mcad_geom::{Aabb, Arc, DimSymbol, Point2, Polyline, Shape};
use mcad_io::{ImportSummary, LoadSummary, load_dxf, load_mcad, save_dxf, save_mcad};

use frame::{frame_layout, paper_to_world, parse_scale_input};

// 破線パターンの紙 mm 定数は「画面と出力の唯一の出所」として `plot` に置く（M8 タスク39）。
// 画面側（`dash_pattern_px`）はここから参照するだけで、挙動は変えない。
// 寸法注記の紙 mm サイズは定数ではなく文書の [`DimStyle`] が唯一の出所（M9 タスク49-3、
// [`dim_sizes`] 参照）。
use plot::dash_pattern_mm;

use tool::{
    ArcTool, CircleTool, DimDiameterTool, DimLinearTool, DimRadialTool, DragPreview, ExtendTool,
    FilletTool, InputEvent, LineTool, OffsetOutcome, PlacementKind, PlacementOutcome,
    PlacementPreview, PointTool, PolylineTool, SelectTool, SplitTool, TextTool, Tool, ToolCtx,
    ToolResult, TrimTool, layer_visible,
};
use viewport::Viewport;

/// 円弧をポリライン近似する際の分割数（固定値。将来ズーム適応分割は不要）。
const ARC_SEGMENTS: usize = 64;

/// ホイール1ノッチあたりのズーム倍率。
const WHEEL_ZOOM_SPEED: f64 = 0.0015;

/// グリッドの目標スクリーン間隔（ピクセル）。
const GRID_TARGET_PX: f64 = 48.0;

/// グリッド間隔(mm)を表示用に整形する。`{:.6}`で固定小数化してから末尾の余分な
/// '0'と'.'を落とす(10f64.powf由来の浮動小数アーティファクトを丸めて確実に短い
/// 表記にする)。
fn format_grid_step(step: f64) -> String {
    let s = format!("{step:.6}");
    let s = s.trim_end_matches('0');
    s.trim_end_matches('.').to_owned()
}

/// 選択ヒットテストのピック許容量（スクリーンピクセル）。ワールド単位へは
/// `PICK_TOLERANCE_PX / viewport.zoom` で変換する。
const PICK_TOLERANCE_PX: f64 = 6.0;

/// スナップの探索半径（スクリーンピクセル）。ワールド単位へは
/// `SNAP_RADIUS_PX / viewport.zoom` で変換する。ピック許容量より少し大きめにして、
/// 作図時に候補点へ吸い付きやすくする。
const SNAP_RADIUS_PX: f64 = 12.0;

/// スナップマーカーの色（作図色・選択色と区別しやすい明るい緑）。
const SNAP_MARKER_COLOR: Color32 = Color32::from_rgb(60, 255, 120);
/// スナップマーカーの基準サイズ（スクリーンピクセル。中心からの半幅相当）。
const SNAP_MARKER_SIZE: f32 = 6.0;

/// ステータスメッセージ（通常の操作フィードバック）の表示時間（秒）。経過後は自動で
/// 消える。旧値は5.0秒で、手動スモークテストで「読む前に消える」との指摘を受けて
/// 延長した。
const STATUS_MESSAGE_SECS: f64 = 10.0;

/// ステータスメッセージ（ファイル入出力の結果通知）の表示時間（秒）。
///
/// 開く/保存/DXFインポート・エクスポートの成否は、通常の操作フィードバック
/// （[`STATUS_MESSAGE_SECS`]）より発生頻度が低く、かつ「N entity(ies) skipped」の
/// ようなデータロスに関わる件数を含むことがあるため、確認する時間を長めに確保する。
const STATUS_MESSAGE_SECS_IMPORTANT: f64 = 15.0;

/// `.mcad` ファイルの拡張子（ファイルダイアログのフィルタ・拡張子補完の両方で使う）。
const MCAD_EXTENSION: &str = "mcad";

/// DXF ファイルの拡張子（ファイルダイアログのフィルタ・拡張子補完の両方で使う）。
const DXF_EXTENSION: &str = "dxf";

/// パス未定のドキュメントを「名前を付けて保存」する際の初期ファイル名。
const DEFAULT_FILE_NAME: &str = "Untitled.mcad";

/// DXF エクスポートダイアログの初期ファイル名（`current_path` が未定のとき）。
const DEFAULT_DXF_FILE_NAME: &str = "Untitled.dxf";

/// SVG ファイルの拡張子（ファイルダイアログのフィルタ・拡張子補完の両方で使う）。
const SVG_EXTENSION: &str = "svg";

/// SVG エクスポートダイアログの初期ファイル名（`current_path` が未定のとき）。
const DEFAULT_SVG_FILE_NAME: &str = "Untitled.svg";

/// PDF ファイルの拡張子（ファイルダイアログのフィルタ・拡張子補完の両方で使う）。
const PDF_EXTENSION: &str = "pdf";

/// PDF エクスポートダイアログの初期ファイル名（`current_path` が未定のとき）。
const DEFAULT_PDF_FILE_NAME: &str = "Untitled.pdf";

/// DXF importで生成した文書に割り当てる `saved_generation` の番兵値。
///
/// `load_dxf`（内部で `clear_history()` を呼ぶ）が返す `Document` の世代は必ず `0`
/// になる。もし `.mcad` の `open_document` のように `saved_generation` をその世代へ
/// 合わせると dirty 判定（[`McadApp::is_dirty`]）が偽になってしまうが、DXF import は
/// 設計判断上「必ず未保存」として扱う必要がある（DESIGN.md 6章 設計判断1: DXFは
/// `.mcad` と混同しない。Ctrl+S を押すと元の DXF を上書きせず「名前を付けて保存」へ
/// 誘導する）。そのため `document.generation()`（常に0）とは一致し得ない `u64::MAX`
/// を「まだ一度もこの文書を保存していない」ことを表す番兵として使い、
/// 常に dirty=true になるようにする。
const DXF_IMPORT_SAVED_GENERATION_SENTINEL: u64 = u64::MAX;

/// ファイルパス未定のドキュメントをウィンドウタイトル/ステータスバーへ表示する際の
/// ラベル。
const UNTITLED_LABEL: &str = "Untitled";

/// 新規レイヤーへ作成順に巡回で割り当てる色のパレット。
///
/// デフォルトレイヤー（白想定）と区別しやすい彩度のある色を並べる。
const LAYER_COLOR_PALETTE: [Rgb; 6] = [
    Rgb::new(230, 80, 80),
    Rgb::new(80, 200, 120),
    Rgb::new(90, 140, 255),
    Rgb::new(230, 200, 60),
    Rgb::new(200, 90, 220),
    Rgb::new(70, 210, 210),
];

/// 新規文書（起動時・Ctrl+N）に用意する、デフォルトレイヤー `"0"` 以外の既定レイヤー。
///
/// `(レイヤー名, 色, 線種, 線幅mm)` を **奥から手前の順** に並べる。重ね順
/// （[`Layer::order`]）はこの配列の添字 + 1 を割り当てる（`"0"` が `order = 0` なので、
/// 既定レイヤーは常に `"0"` より手前に載る）。将来ハッチング等を実装したときは
/// **この配列へ1行足すだけ**で既定セットを拡張できる。
///
/// 製図規定 4-1 準拠（M9 タスク53）: 中心線（一点鎖線 0.13mm）→ 破線（破線 0.35mm）
/// → 外形線（実線 0.35mm）→ 寸法線（実線 0.18mm）→ 文字（実線 0.18mm、規定に明記の
/// 行は無いため寸法線と同じ細線幅を流用）の順で奥から手前へ積む。文字を最前面に
/// 置くのは図形に隠れないようにする一般的な運用。色は [`LAYER_COLOR_PALETTE`] から
/// 重複しないよう割り当てる（`文字` は改名前の `"Text"` 時代から `palette[3]` を
/// 維持し、他の4層は残りの添字を使う）。[`fresh_document`] がこの配列を基に
/// カレントレイヤーを `外形線` へ設定する。
const DEFAULT_EXTRA_LAYERS: [(&str, Rgb, Linetype, f32); 5] = [
    ("中心線", LAYER_COLOR_PALETTE[0], Linetype::DashDot, 0.13),
    ("破線", LAYER_COLOR_PALETTE[1], Linetype::Dashed, 0.35),
    ("外形線", LAYER_COLOR_PALETTE[2], Linetype::Continuous, 0.35),
    ("寸法線", LAYER_COLOR_PALETTE[4], Linetype::Continuous, 0.18),
    ("文字", LAYER_COLOR_PALETTE[3], Linetype::Continuous, 0.18),
];

/// [`DEFAULT_EXTRA_LAYERS`] のうち、新規文書のカレントレイヤーにする名前。
///
/// 作図の既定は輪郭線であるべきなので `外形線` を選ぶ（[`fresh_document`]）。
const DEFAULT_CURRENT_LAYER_NAME: &str = "外形線";

/// 寸法ツール（[`ToolKind::DimLinear`]/`DimRadial`/`DimDiameter`）が新規エンティティを
/// 割り当てる先のレイヤー名。[`layer_named`] で存在確認したうえで使う
/// （M9 タスク53: 該当名のレイヤーが無ければカレントレイヤーへフォールバックし、
/// 読込図面で勝手にレイヤーを増やさない）。
const DIM_LAYER_NAME: &str = "寸法線";

/// 文字ツール（`commit_text`）が新規エンティティを割り当てる先のレイヤー名。
/// [`DIM_LAYER_NAME`] と同様、存在すれば使い、無ければカレントレイヤーへ
/// フォールバックする。
const TEXT_LAYER_NAME: &str = "文字";

/// 文書中から名前が完全一致するレイヤーを探す（表示・ロック状態は問わない）。
///
/// 作図ツールの自動割当（[`DIM_LAYER_NAME`]/[`TEXT_LAYER_NAME`]）が使う。**新規に
/// レイヤーを作らない** — 該当名が無ければ呼び出し側がカレントレイヤーへ
/// フォールバックする（読込図面でレイヤーが勝手に増えないための設計。
/// [`fresh_document`] の doc 参照）。同名レイヤーが複数存在する場合は
/// [`Document::layers`] の反復順で最初に見つかったものを返す（重複禁止は
/// 不変条件ではないため、決定的な仕様として明記する）。
fn layer_named(document: &Document, name: &str) -> Option<LayerId> {
    document
        .layers()
        .find(|(_, layer)| layer.name == name)
        .map(|(id, _)| id)
}

/// ステータスメッセージの文字色（エラー通知が主用途なので警告寄りの赤）。
const STATUS_MESSAGE_COLOR: Color32 = Color32::from_rgb(255, 120, 120);

/// 図面枠（輪郭・表題欄の罫線・欄文字）の色。グリッドより明るく、通常のエンティティ
/// 色よりは控えめな中間グレーで、印刷対象と分かる程度に主張する（タスク38）。
const FRAME_COLOR: Color32 = Color32::from_gray(150);

/// 用紙縁（用紙の外形矩形）の色。**印刷対象ではない画面専用ヒント**であることが
/// 一目で分かるよう、グリッドと同じ流儀の控えめなグレーにする（タスク38）。
const PAPER_EDGE_COLOR: Color32 = Color32::from_gray(90);

/// 線幅の画面 px 下限（DESIGN.md M8 設計判断5・6）。
///
/// 紙 mm の線幅がズームアウトで極小になっても線が消えないための下限。**出力
/// （SVG/PDF/DXF）側はこの下限を適用しない**（判断5「出力側はクランプしない」）。
/// これはタスク35a/35bまで使っていた暫定固定値 `PROVISIONAL_STROKE_PX` の後継で、
/// 「常にこの太さで描く」から「この太さを下限に、紙 mm 線幅を反映する」へ役割が
/// 変わったため改名した（[`resolve_stroke_px`]）。
const MIN_STROKE_PX: f32 = 1.0;

/// 線種のダッシュ周期（紙 mm 換算後の合計 px）がこれ未満なら実線として描く。
///
/// ズームアウトでダッシュパターンが潰れると、1 本の線に対して大量の極小
/// シェイプ（[`egui::Shape::Line`] 断片）が生成され描画コストだけが増え、
/// かつ人の目にも実線と区別できない。2px は「隣接するダッシュとギャップを
/// 視覚的に見分けられる最小限」の目安（一般的なディスプレイの実効解像度で
/// 1px 未満の要素は潰れて見える点を踏まえ、ダッシュ+ギャップの1周期として
/// 2px を境界にした）。DESIGN.md M8 設計判断5 検収(c)。
const MIN_DASH_PERIOD_PX: f32 = 2.0;

/// v3 `.mcad` の線幅移行規則（DESIGN.md M8 設計判断6）と旧描画 `max(width_px, 1.0)`
/// が厳密一致することを保証する基準ズーム。判断6 検収(b)が要求する名前付き定数。
///
/// 旧描画は「常に 1px」（ズーム非依存）だったため、`resolve_stroke_px` が同じ結果を
/// 返すのは `width_mm * k * zoom` がちょうど 1px 相当になるズームでしかない。
/// 旧 `.mcad` の実効表示幅換算（`width_mm = 0.35mm * max(width_px, 1.0)`）は
/// `0.35mm` を基準単位にしているため、逆数のこのズームで換算が打ち消し合う。
///
/// **回帰テスト専用の定数**（実行時の描画ロジックはズームに応じて連続的に px を
/// 解決するだけで、この基準ズームを特別扱いしない）なので `#[cfg(test)]`。
#[cfg(test)]
const LEGACY_WIDTH_PX_REFERENCE_ZOOM: f64 = 1.0 / 0.35;

/// 線幅（紙 mm）とワールド→画面の換算率から、画面 px の線幅を解決する
/// （DESIGN.md M8 設計判断5: `px = max(1px, width_mm * k * zoom)`）。
///
/// `k` は [`mcad_core::Scale::world_mm_per_paper_mm`]（紙 1mm あたりのワールド mm）。
/// ワールド量として f64 で計算し、egui へ渡す境界でのみ f32 化する
/// （AGENTS.md のワールド座標 f64 規約）。GUI 非依存の純関数なので単体テストできる。
fn resolve_stroke_px(width_mm: f32, k: f64, zoom: f64) -> f32 {
    let raw_px = f64::from(width_mm) * k * zoom;
    raw_px.max(f64::from(MIN_STROKE_PX)) as f32
}

/// 紙基準表示（タスク36b で「線幅表示」として導入、タスク37 で寸法注記・Text も含む
/// 「紙基準表示」へ拡張、AutoCAD の LWDISPLAY 相当）の ON/OFF を反映して画面 px の
/// 線幅を決める。OFF なら `width_mm`・`k`・`zoom` に関わらず常に [`MIN_STROKE_PX`]
/// （タスク36 以前と同じ固定 1px）、ON なら [`resolve_stroke_px`] をそのまま使う
/// （紙 mm 基準・ズーム比例）。
///
/// 線種のダッシュピッチ（[`dash_pattern_px`]）はこのフラグの影響を受けない
/// （常に紙 mm 基準のまま。DESIGN.md M8 設計判断5 実装時追記）。分岐をこの関数へ
/// 集約し、`draw_entities` 側で形状・寸法（`DimLinear`/`DimRadial`）の両方に
/// 同じ規則を適用する。GUI 非依存の純関数なので単体テストできる。
fn resolve_stroke_px_with_toggle(paper_display: bool, width_mm: f32, k: f64, zoom: f64) -> f32 {
    if paper_display {
        resolve_stroke_px(width_mm, k, zoom)
    } else {
        MIN_STROKE_PX
    }
}

/// 線種のダッシュパターンを画面 px 単位（ダッシュ長配列・ギャップ長配列）へ換算する。
///
/// [`egui::Shape::dashed_line_many_with_offset`] の引数形（ダッシュ長とギャップ長を
/// 別配列で持ち、要素ごとに交互のダッシュ/ギャップを表す）に合わせて偶数長の紙 mm
/// パターンを分割する。周期（合計 px）が [`MIN_DASH_PERIOD_PX`] 未満、または `k*zoom`
/// が非有限・非正なら `None`（実線へフォールバックする防御。判断5 検収(c)）。
fn dash_pattern_px(linetype: Linetype, k: f64, zoom: f64) -> Option<(Vec<f32>, Vec<f32>)> {
    let pattern_mm = dash_pattern_mm(linetype)?;
    let scale = (k * zoom) as f32;
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let pattern_px: Vec<f32> = pattern_mm.iter().map(|mm| mm * scale).collect();
    let period: f32 = pattern_px.iter().sum();
    if !period.is_finite() || period < MIN_DASH_PERIOD_PX {
        return None;
    }
    let dash_lengths: Vec<f32> = pattern_px.iter().step_by(2).copied().collect();
    let gap_lengths: Vec<f32> = pattern_px.iter().skip(1).step_by(2).copied().collect();
    Some((dash_lengths, gap_lengths))
}

/// 生成するダッシュ片数の上限（1本の線あたり）。超える場合は実線へフォールバックする。
///
/// クリップ後のビューポート矩形は通常でも数千px規模（一般的なウィンドウで対角
/// 2000〜3000px程度）で、[`MIN_DASH_PERIOD_PX`]（2px）が周期の下限なので、
/// クリップ矩形1枚分のダッシュ片数はおよそ「対角px ÷ 2px」= 高々 1500〜2000 程度に
/// 収まる。多頂点ポリライン（クリップ区間が複数に分かれる）や極端に横長/縦長の
/// ウィンドウでも安全マージンを持たせるため、一桁上の 20000 を上限とする
/// （Codex adversarial review 指摘: クリップ前は高ズームでスクリーン座標が
/// `1e7`px規模に達しうり、周期が数px ならダッシュ片数が数百万個になりフリーズする。
/// クリップで大部分は解消するが、極端に多い頂点を持つポリラインが繰り返し
/// クリップ矩形の境界をまたぐ病的なケースへの保険として、この上限を残す）。
const MAX_DASH_SEGMENTS: usize = 20_000;

/// スクリーン座標の線分 `(a, b)` を矩形 `clip` でクリップする（Liang-Barsky）。
///
/// 交差する部分がなければ `None`。パラメトリック媒介変数 `t ∈ [0, 1]`
/// （`a` が `t=0`、`b` が `t=1`）で計算し、4辺それぞれの半平面制約で
/// `[t0, t1]` を絞り込む。GUI 非依存の純関数で単体テストできる。
fn clip_segment_to_rect(a: Pos2, b: Pos2, clip: Rect) -> Option<(Pos2, Pos2)> {
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    let dx = f64::from(b.x - a.x);
    let dy = f64::from(b.y - a.y);
    // (p, q): p < 0 は「左/下境界を超えて外へ向かう」方向、p > 0 は「右/上境界へ
    // 向かう」方向。p == 0 は境界と平行（q < 0 なら完全に外側）。
    let checks = [
        (-dx, f64::from(a.x - clip.min.x)),
        (dx, f64::from(clip.max.x - a.x)),
        (-dy, f64::from(a.y - clip.min.y)),
        (dy, f64::from(clip.max.y - a.y)),
    ];
    for (p, q) in checks {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                if r > t1 {
                    return None;
                }
                if r > t0 {
                    t0 = r;
                }
            } else {
                if r < t0 {
                    return None;
                }
                if r < t1 {
                    t1 = r;
                }
            }
        }
    }
    if t0 > t1 {
        return None;
    }
    let lerp = |t: f64| -> Pos2 {
        Pos2::new(
            (f64::from(a.x) + t * dx) as f32,
            (f64::from(a.y) + t * dy) as f32,
        )
    };
    Some((lerp(t0), lerp(t1)))
}

/// 2点がほぼ同一位置とみなせるか（クリップ後の区間連結判定用の許容誤差）。
fn points_nearly_equal(a: Pos2, b: Pos2) -> bool {
    (a.x - b.x).abs() < 1e-3 && (a.y - b.y).abs() < 1e-3
}

/// 開いた点列を矩形 `clip` でクリップし、可視な区間ごとの点列（複数本になりうる）へ
/// 分割する（GUI 非依存の純関数、単体テストできる）。
///
/// 各線分を個別にクリップし、前の線分のクリップ後終点と今の線分のクリップ後始点が
/// 一致する（＝間に不可視区間を挟まない）場合は同じ区間として連結する。画面外へ
/// 出て別の場所で再び入る折れ線は、複数の独立した区間として返る。
///
/// **ダッシュの位相は区間ごとにリセットする**（各区間の描画は
/// `dashed_line_many_with_offset` の `dash_offset = 0` から開始する。呼び出し側
/// [`stroke_polyline`] 参照）。既存の実装も1エンティティ全体の位相を常に 0 起点で
/// 描いており（形状ごとに独立、パン・ズームでは連続しない）、クリップ区間の切れ目で
/// 同じ扱いを踏襲するだけなので新しい種類の見た目の破綻ではない。不可視区間の
/// 長さを跨いで位相を連続させる代替案は、クリップ前の全長（高ズームで最大 `1e7`px
/// 規模）を経由する累積長計算を要し、シェイプ数ではなく計算コストの点で今回の
/// 問題を作り込む方向とも言えるため採らない。
fn clip_polyline_runs(points: &[Pos2], clip: Rect) -> Vec<Vec<Pos2>> {
    let mut runs: Vec<Vec<Pos2>> = Vec::new();
    for window in points.windows(2) {
        let (a, b) = (window[0], window[1]);
        let Some((ca, cb)) = clip_segment_to_rect(a, b, clip) else {
            continue;
        };
        let continues = runs
            .last()
            .and_then(|run| run.last())
            .is_some_and(|&last| points_nearly_equal(last, ca));
        if continues {
            runs.last_mut().expect("just checked non-empty").push(cb);
        } else {
            runs.push(vec![ca, cb]);
        }
    }
    runs
}

/// 折れ線（複数区間）の総延長 [px]。
fn total_path_length(runs: &[Vec<Pos2>]) -> f32 {
    runs.iter()
        .map(|run| run.windows(2).map(|w| (w[1] - w[0]).length()).sum::<f32>())
        .sum()
}

/// 線種を反映して開いた/閉じた点列を描く（[`draw_shape`] が扱う全形状で共通に使う）。
///
/// `Continuous`、またはダッシュ周期が潰れる場合（[`dash_pattern_px`]）は通常の
/// 折れ線として描く。ダッシュ化する場合は、点列を `clip_rect`（キャンバスのスクリーン
/// 矩形）へ線幅+1周期分のマージンを足した矩形でクリップしてから
/// [`egui::Shape::dashed_line_many_with_offset`] へ渡す（Codex adversarial review
/// 指摘: クリップしないと、画面外まで伸びる線を高ズームで見たときスクリーン座標の
/// 全長が周期の桁違いに大きくなり、生成シェイプ数が爆発してフリーズする。マージンは
/// クリップ境界ちょうどでダッシュの端が不自然に切り詰められて見えるのを避けるため）。
/// クリップ後もなお生成予定のダッシュ片数が [`MAX_DASH_SEGMENTS`] を超える場合は
/// 保険として実線へフォールバックする。
fn stroke_polyline(
    painter: &egui::Painter,
    points: Vec<Pos2>,
    stroke: Stroke,
    linetype: Linetype,
    k: f64,
    zoom: f64,
    clip_rect: Rect,
) {
    if points.len() < 2 {
        return;
    }
    match dash_pattern_px(linetype, k, zoom) {
        None => {
            painter.line(points, stroke);
        }
        Some((dash_lengths, gap_lengths)) => {
            let period: f32 = dash_lengths.iter().chain(&gap_lengths).sum();
            if period <= 0.0 || !period.is_finite() {
                painter.line(points, stroke);
                return;
            }
            let margin = stroke.width + period;
            let runs = clip_polyline_runs(&points, clip_rect.expand(margin));
            if runs.is_empty() {
                // 画面外（マージン込みでも交差しない）。何も描かない。
                return;
            }
            let estimated_dashes = total_path_length(&runs) / period;
            if !estimated_dashes.is_finite() || estimated_dashes > MAX_DASH_SEGMENTS as f32 {
                // 保険: クリップ後もなお片数が過大なら実線へ後退する
                // （多頂点ポリラインがクリップ境界を繰り返し跨ぐ病的ケース）。
                painter.line(points, stroke);
                return;
            }
            let mut shapes = Vec::new();
            for run in &runs {
                if run.len() < 2 {
                    continue;
                }
                egui::Shape::dashed_line_many_with_offset(
                    run,
                    stroke,
                    &dash_lengths,
                    &gap_lengths,
                    0.0,
                    &mut shapes,
                );
            }
            painter.extend(shapes);
        }
    }
}

/// 選択エンティティのハイライト色（確定済みエンティティ色とは別の強調色）。
const SELECTION_COLOR: Color32 = Color32::from_rgb(80, 200, 255);
/// 選択ハイライトの線の太さ。
const SELECTION_WIDTH: f32 = 2.5;
/// 矩形選択プレビューの塗り色（半透明）。
const RECT_FILL_COLOR: Color32 = Color32::from_rgba_premultiplied(30, 60, 90, 60);
/// 矩形選択プレビューの枠線色。
const RECT_OUTLINE_COLOR: Color32 = Color32::from_rgb(80, 160, 255);
/// オフセット結果ゴーストのプレビュー色（暖色。作図ツールのプレビューと同系統で、
/// 選択ハイライト（寒色）と区別しやすい。オフセット中は元＝ハイライト、結果＝この色）。
const OFFSET_PREVIEW_COLOR: Color32 = Color32::from_rgb(255, 200, 60);

/// Text ツールの入力中プレビュー色（作図プレビューと同系統の暖色）。
const TEXT_PREVIEW_COLOR: Color32 = Color32::from_rgb(255, 200, 60);

/// Text 描画時の最小フォントサイズ（px, ワールド高さ×zoom）。これ未満は判読不能として
/// 描画しない（極小ガリー生成のコストと 0 近傍を避ける）。
const MIN_TEXT_PX: f64 = 1.0;
/// Text 描画時の最大フォントサイズ（px）。過大ズームでフォントアトラスが肥大するのを
/// 防ぐため、描画サイズをここで頭打ちにする（ワールド固定サイズの近似上限）。
const MAX_TEXT_PX: f64 = 4096.0;

/// 寸法の矢先の長さ（紙基準表示 OFF 時の画面固定ピクセル）。ワールド長へは
/// `DIM_ARROW_PX / zoom` で換算する。OFF のときは注釈（矢印・文字）を縮尺に関わらず
/// 読める大きさに保つため、ピック許容量と同じくスクリーン固定 px をズームで割る
/// （DESIGN.md M6 設計判断2 の展開は純関数側、大きさは app 側）。ON 時は
/// [`DimStyle::arrow_len_mm`] を使う（タスク37 / M9 タスク49-3、[`dim_sizes`] 参照）。
///
/// **これは画面固定 px であって紙 mm ではない**ので、[`DimStyle`] へ寄せない
/// （スタイルは紙の上の大きさを決める設定で、画面固定モードはその外側にある）。
const DIM_ARROW_PX: f64 = 12.0;
/// 寸法値ラベルの文字高さ（紙基準表示 OFF 時の画面固定ピクセル）。ワールド高さへは
/// `DIM_TEXT_PX / zoom`。ON 時は [`DimStyle::text_height_mm`] を使う
/// （[`DIM_ARROW_PX`] と同じ扱い。[`dim_sizes`] 参照）。
const DIM_TEXT_PX: f64 = 14.0;

/// 未保存確認モーダルの状態（OS の閉じるボタン / Ctrl+N / Ctrl+O の3経路で共有）。
///
/// いずれの経路も、ネイティブの確認ダイアログ（`rfd::MessageDialog`）を同期表示すると
/// メインウィンドウの裏に隠れてユーザーが気づけない、あるいは（閉じるボタン経路では）
/// イベントループが止まり「応答なし」になる問題があったため、egui 内製の
/// 非ブロッキングモーダルへ統一した。その状態遷移をこの enum で管理し、閉じるボタン
/// 経路では「破棄して終了」時の無限クローズループも防ぐ（[`McadApp::ui`] の
/// close 検知ロジック参照）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfirmState {
    /// 通常。確認モーダルは出ていない。
    Idle,
    /// OS のクローズ確認モーダルを表示中（OS のクローズはキャンセル済み）。
    ConfirmingClose,
    /// Ctrl+N（新規文書）の確認モーダルを表示中。
    ConfirmingNew,
    /// Ctrl+O（ファイルを開く）の確認モーダルを表示中。
    ConfirmingOpen,
    /// Ctrl+Shift+O（DXFを開く）の確認モーダルを表示中。
    ConfirmingOpenDxf,
    /// Recent メニュー経由（「開く」）の確認モーダルを表示中。対象パスは
    /// [`McadApp::pending_recent_path`] 側に持つ（M8 タスク41-2）。
    ConfirmingOpenRecent,
    /// ユーザーが破棄を選択済み。以降の close 要求はキャンセルせず通す。
    Closing,
}

impl ConfirmState {
    /// この状態で確認モーダルを描くべきなら `(本文, 破棄ボタンのラベル)` を返す。
    /// `Idle`（モーダルなし）・`Closing`（破棄確定済みで再描画不要）では `None`。
    fn prompt(self) -> Option<(&'static str, &'static str)> {
        match self {
            ConfirmState::ConfirmingClose => {
                Some(("Discard unsaved changes and quit?", "Discard and quit"))
            }
            ConfirmState::ConfirmingNew => Some((
                "Discard unsaved changes and start a new document?",
                "Discard and continue",
            )),
            ConfirmState::ConfirmingOpen => Some((
                "Discard unsaved changes and open another file?",
                "Discard and continue",
            )),
            ConfirmState::ConfirmingOpenDxf => Some((
                "Discard unsaved changes and import a DXF file?",
                "Discard and continue",
            )),
            ConfirmState::ConfirmingOpenRecent => Some((
                "Discard unsaved changes and open a recent file?",
                "Discard and continue",
            )),
            ConfirmState::Idle | ConfirmState::Closing => None,
        }
    }
}

/// 現在アクティブなツールの種類。`Select` は選択・編集モード（[`SelectTool`]）で、
/// 作図ツール（Point/Line/…）とは別経路で処理する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolKind {
    Select,
    Point,
    Line,
    Circle,
    Arc,
    Polyline,
    Text,
    DimLinear,
    DimRadial,
    DimDiameter,
    Trim,
    Extend,
    Fillet,
    Split,
}

impl ToolKind {
    /// この種類に対応する作図ツールの新しいインスタンスを作る。
    /// `Select` は作図ツールを持たず（選択・編集は [`McadApp::select_tool`] が
    /// 別経路で担う）、`None` を返す。
    fn spawn(self) -> Option<Box<dyn Tool>> {
        match self {
            ToolKind::Select => None,
            ToolKind::Point => Some(Box::new(PointTool::default())),
            ToolKind::Line => Some(Box::new(LineTool::default())),
            ToolKind::Circle => Some(Box::new(CircleTool::default())),
            ToolKind::Arc => Some(Box::new(ArcTool::default())),
            ToolKind::Polyline => Some(Box::new(PolylineTool::default())),
            ToolKind::Text => Some(Box::new(TextTool::default())),
            ToolKind::DimLinear => Some(Box::new(DimLinearTool::default())),
            ToolKind::DimRadial => Some(Box::new(DimRadialTool::default())),
            ToolKind::DimDiameter => Some(Box::new(DimDiameterTool::default())),
            ToolKind::Trim => Some(Box::new(TrimTool::default())),
            ToolKind::Extend => Some(Box::new(ExtendTool::default())),
            ToolKind::Fillet => Some(Box::new(FilletTool::default())),
            ToolKind::Split => Some(Box::new(SplitTool::default())),
        }
    }

    /// このツールが既存エンティティのヒットテスト結果（境界・フィレット1本目など）を
    /// スナップショットとして抱え込むか。抱え込むツールは undo/redo・ファイル操作の後で
    /// 作り直し、消えたエンティティの形状を持ち越さないようにする
    /// （DESIGN.md M7 設計判断6 の共通規約）。
    ///
    /// `Split` は含まない: `SplitTool` は `ShapePick` を状態として跨いで保持しない
    /// （単一状態で、ヒットした形状は `on_shape_pick` の引数から確定コマンドを作るその場で
    /// しか使わない）ため、undo/redo・ファイル操作をまたいで持ち越される古いスナップショットが
    /// 存在しない。
    fn caches_picked_shapes(self) -> bool {
        matches!(self, ToolKind::Trim | ToolKind::Extend | ToolKind::Fillet)
    }

    /// `Document::apply` が失敗した（レイヤーロック等）ときも、このツールをそのまま
    /// （`spawn()` で作り直さず）使い続けるか。
    ///
    /// M7 の4ツール（トリム・延長・フィレット・分割）はDESIGN.md M7設計判断6が「失敗時は
    /// 状態据え置きで、同じ境界のまま別対象を試せる／1本目は選び直さず2本目だけ選び直せる」
    /// という再挑戦フローを規定している。他ツールと同じ「失敗時は spawn() で作り直す」を
    /// 適用すると、`TrimTool`/`ExtendTool` の `WaitingTarget { boundary }` や `FilletTool` の
    /// `WaitingSecondLine` が失われ、この規約が崩れる（Codex adversarial review
    /// 2026-07-26 指摘）。作り直さない代わりに、選択集合の載せ替えフラグを次の確定へ
    /// 誤って持ち越さないよう [`Tool::on_commit_failed`] でクリアする必要がある。
    fn keeps_state_on_commit_failure(self) -> bool {
        matches!(
            self,
            ToolKind::Trim | ToolKind::Extend | ToolKind::Fillet | ToolKind::Split
        )
    }

    /// ステータス表示用のラベル。
    fn label(self) -> &'static str {
        match self {
            ToolKind::Select => "Select",
            ToolKind::Point => "Point",
            ToolKind::Line => "Line",
            ToolKind::Circle => "Circle",
            ToolKind::Arc => "Arc",
            ToolKind::Polyline => "Polyline",
            ToolKind::Text => "Text",
            ToolKind::DimLinear => "Linear Dim",
            ToolKind::DimRadial => "Radial Dim",
            ToolKind::DimDiameter => "Diameter Dim",
            ToolKind::Trim => "Trim",
            ToolKind::Extend => "Extend",
            ToolKind::Fillet => "Fillet",
            ToolKind::Split => "Split",
        }
    }
}

struct McadApp {
    document: Document,
    viewport: Viewport,
    /// 現在アクティブなツールの種類（UI表示・キー切替用）。
    tool_kind: ToolKind,
    /// 現在アクティブな作図ツール本体。`tool_kind == Select` のとき `None`。
    tool: Option<Box<dyn Tool>>,
    /// 選択・編集ツール。選択集合を所有し、`tool_kind == Select` のとき有効。
    /// 選択集合はアプリ UI 状態でありドキュメント履歴には積まない（[`SelectTool`] の doc 参照）。
    select_tool: SelectTool,
    /// アプリ設定（トグル・既定用紙/尺度/様式・最近使ったファイル）。実行時の唯一の
    /// 置き場（M8 タスク41）。スナップ有効/無効（`F3`）・直交モード（`F8`、v0.7.1。
    /// `Line`/`Polyline`/`Arc` の作図中、スナップ候補が無い場所でだけ直前の点から
    /// 水平・垂直な方向へ拘束する。`ortho.rs`、`resolve_click_point` 参照）・
    /// 紙基準表示（`F9`、タスク37。旧: 線幅表示、タスク36b）の3トグルもここに含む。
    /// 紙基準表示 OFF のときは全エンティティの線幅を [`MIN_STROKE_PX`] 固定（ズーム
    /// 非依存）で描き、寸法注記（矢印・文字）と線幅はスクリーン固定 px
    /// （[`DIM_ARROW_PX`]/[`DIM_TEXT_PX`]）のまま。ON のときは線幅がタスク36 の紙 mm 基準
    /// `resolve_stroke_px`（ズームに比例して太くなる）、寸法注記が文書スタイルの紙 mm
    /// （[`DimStyle::arrow_len_mm`]/[`DimStyle::text_height_mm`]）を `k` 倍したワールド長に
    /// なる（`dim_sizes`）。
    /// タスク38 の図面枠実線・タスク39/40 の SVG/PDF 出力が紙 mm 基準を前提とするため、
    /// 画面と出力の一致確認に ON が要る（DESIGN.md M8 設計判断5 実装時追記・判断4
    /// 実装時追記）。Text エンティティの表示（`height * k`）はこのトグルの影響を受けない
    /// （判断4 実装時追記、常に紙 mm 解釈）。
    config: config::Config,
    /// config.json の保存先。`None` なら保存不能（起動時に警告済み。以後の保存は
    /// 黙ってスキップする）。
    config_path: Option<PathBuf>,
    /// 直近フレームで作図ツールのカーソルがスナップした先。マーカー描画に使う。
    /// スナップしていない・作図ツール非アクティブ・スナップ無効のときは `None`。
    snap_marker: Option<snap::SnapResult>,
    /// ステータスバーに一時表示するメッセージ（主にコア操作のエラー通知）。
    /// メッセージごとに保持している表示時間（[`StatusMessage::duration_secs`]、通常は
    /// [`STATUS_MESSAGE_SECS`]、ファイル入出力の結果は [`STATUS_MESSAGE_SECS_IMPORTANT`]）
    /// 経過で自動的に消える。
    status: Option<StatusMessage>,
    /// 現在開いている `.mcad` ファイルのパス。`None` は「一度も保存/読込していない
    /// (Untitled)」を意味する。新規文書（Ctrl+N）で `None` に戻る。
    current_path: Option<PathBuf>,
    /// 最後に保存/読込/新規作成した時点のドキュメント世代番号
    /// （[`Document::generation`]）。
    ///
    /// 未保存の変更（dirty）判定は [`McadApp::is_dirty`] が
    /// `document.generation() != saved_generation` で行う。世代はコアが「実際に状態を
    /// 変えたコマンドの適用」ごとに単調増加し、undo/redo は対応する履歴時点の世代へ戻す。
    /// このため「保存 → 1 操作 → undo で保存時点の内容へ厳密に戻る」と世代が再び
    /// `saved_generation` に一致し、`*` 表示が消える（redo でやり直せば再び dirty）。
    /// no-op なコマンドは世代を変えないので dirty 状態にも影響しない。
    saved_generation: u64,
    /// 未保存確認モーダルの状態（OS の閉じるボタン / Ctrl+N / Ctrl+O 共通、
    /// [`ConfirmState`] の doc 参照）。
    confirm_state: ConfirmState,
    /// ファイル読込直後、次フレームで図面全体へズームフィットする必要があるか。
    ///
    /// M4タスク13: 読込ショートカット処理（`ui()` 前半）の時点ではまだキャンバスの
    /// スクリーン矩形が確定していないため、フィット計算をその場で行えない。
    /// `open_document`/`apply_imported_dxf` は読込直後にこのフラグだけを立て、
    /// `CentralPanel` 内でスクリーン矩形が確定した直後（`ui()` 後半）にこのフラグを
    /// 見て `Viewport::fit_to_aabb` を呼び、`false` へ戻す。エンティティ0件の読込
    /// （空の`.mcad`/DXF）はフィット対象がないため、このフラグは立てず代わりに
    /// その場で `Viewport::new()` の既定ビューへ戻す。
    pending_zoom_fit: bool,
    /// このセッション中に最後にファイルダイアログ（開く/名前を付けて保存/DXFインポート/
    /// DXFエクスポート）でユーザーがパスを確定したときの、そのディレクトリ。
    ///
    /// ダイアログを開く際の初期ディレクトリに使う（[`McadApp::dialog_start_dir`]）。
    /// アプリ再起動をまたぐ永続化はロードマップM8「設定保存」の範囲であり、ここでは
    /// 行わない。
    last_dialog_dir: Option<PathBuf>,
    /// Recent メニュー経由の「開く」で未保存確認中のパス。
    /// `ConfirmState::ConfirmingOpenRecent` へ入るとき必ず上書きするので、古い値が
    /// 残っても無害（M8 タスク41-2）。
    pending_recent_path: Option<PathBuf>,
    /// オフセット距離入力欄の文字列（設計判断5）。空・0・非数なら通過点方式へ
    /// フォールバックし、正の有限値なら距離固定＋クリックは側の決定のみに使う
    /// （[`parse_offset_distance`]）。欄はオフセットモード中のみ上部パネルに表示するが、
    /// 文字列自体はセッション中保持し、`O` 再押下での等間隔連続オフセットに使い回せる
    /// （新規・読込では [`McadApp::reset_transient_ui_state`] でクリア）。
    offset_distance_input: String,
    /// フィレット半径入力欄の文字列（M7 タスク32、設計判断6）。オフセット距離欄と同じ
    /// 「正の有限値のみ受理」規則（[`parse_fillet_radius`]）で、空欄・不正値のままフィレットを
    /// 確定しようとすると ASCII の理由付きで拒否される（通過点方式のようなフォールバックは
    /// 半径には存在しないため）。欄はフィレットツール中のみ上部パネルへ表示するが、文字列
    /// 自体はセッション中保持し、同じ半径での連続フィレットに使い回せる（新規・読込では
    /// [`McadApp::reset_transient_ui_state`] でクリア）。
    fillet_radius_input: String,
    /// Text ツールの文字列入力欄（M6 タスク23）。アンカー確定後に上部パネルへ表示し、
    /// Enter で `AddEntity` 確定。CJK（IME 入力）可。確定・キャンセルのたびにクリアする。
    text_content_input: String,
    /// Text ツールの高さ入力欄（ワールド単位）。**前回値を保持**して次のテキストの既定に
    /// 使うため、確定してもクリアしない（DESIGN.md M6 設計判断6）。
    text_height_input: String,
    /// 直前フレームで Text 入力欄を表示していたか。false→true の遷移フレームで文字列欄へ
    /// フォーカスを移すのに使う（アンカー確定直後すぐタイプできるように）。
    text_field_shown: bool,
    /// 表題欄編集ダイアログの作業コピー。`Some` の間はモーダルが開いている
    /// （[`McadApp::modal_open`]、M8タスク38）。「表題欄を編集…」ボタンで開き、
    /// OK で `Command::SetSheet` 1回（ダイアログセッション全体が undo 1単位）、
    /// キャンセル/モーダル外クリックで破棄する。
    sheet_dialog: Option<TitleBlockDialogState>,
    /// 図面セクションの尺度コンボで「カスタム」が選ばれているか（M8タスク38）。
    /// プリセットに一致しない尺度（ファイル読込等）では、この値に関わらず表示上は
    /// カスタム扱いになる（`sheet_panel` 参照）。
    scale_custom_selected: bool,
    /// 尺度カスタム入力欄の文字列（`N:M` 形式）。セッション中保持する
    /// （[`McadApp::reset_transient_ui_state`] でクリア）。
    scale_custom_input: String,
    /// 尺度カスタム入力の直近の拒否理由（インライン赤字表示用）。適用成功・
    /// プリセット選択・入力欄変更のたびにクリアする。
    scale_input_error: Option<String>,
    /// Alt(+Shift)修飾キージェスチャ（ズーム/パン）の開始時カーソル位置
    /// （DESIGN.md 7章「ズーム・パンの追加操作手段」設計判断(c)(d)）。
    ///
    /// `modifier_view_gesture` が `Some` を返した瞬間（直前フレームが `None` だった、
    /// またはズーム↔パンが切り替わった）にその時点の hover 位置で上書きし、
    /// ジェスチャ継続中は固定する。条件を満たさなくなったフレームで `None` へ戻す。
    /// [`McadApp::reset_transient_ui_state`] でも破棄する（読込後に押しっぱなしの Alt が
    /// 旧アンカーを引きずらないため）。
    alt_zoom_anchor: Option<Pos2>,
    /// 直前フレームで有効だった Alt(+Shift)修飾キージェスチャの種類。
    ///
    /// `alt_zoom_anchor` はアンカー座標だけを持ち、ジェスチャの種類（ズーム/パン）を
    /// 区別しない。ジェスチャ中の Shift 切替（ズーム↔パン移行）を検知してアンカーを
    /// 再取得する（DESIGN.md 設計判断(d)）ために、直前フレームの種類をここに保持する。
    /// `alt_zoom_anchor` と常に一体で扱い、[`McadApp::reset_transient_ui_state`] でも
    /// 一緒に破棄する。
    alt_view_gesture: Option<ViewGesture>,
    /// 寸法パネル(M9タスク50-2)の入力欄が「どの選択集合を対象に開いているか」
    /// （選択中の寸法 ID をソート済みで保持）。`dim_panel` は毎フレーム冒頭でこれと
    /// 現在の選択を比較し、異なれば入力欄一式を新しい選択の共通値へ再同期する
    /// （`sync_dim_edit_state`）。これが無いと、寸法Aの入力途中に選択をBへ変えて
    /// 「確定」を押した際、A用の値がBへ誤って適用されてしまう
    /// （Codex adversarial review 2026-09-04 指摘、M9タスク50 差し戻し対応）。
    dim_edit_target: Vec<EntityId>,
    /// 右パネル「寸法」セクション(M9タスク50)の公差編集状態。`None` は「選択中の
    /// 公差種別をそのまま表示」(`sheet_panel` の `scale_custom_selected` と同じ流儀)。
    /// 種別コンボで値の要る種別(対称/上下偏差/はめあい)を選ぶと、確定/選択解除まで
    /// この状態を保つ。
    dim_tol_editing: Option<DimTolKindUi>,
    /// 対称公差(± v)の数値入力欄。
    dim_tol_symmetric_input: String,
    /// 上下偏差の上側入力欄。
    dim_tol_upper_input: String,
    /// 上下偏差の下側入力欄。
    dim_tol_lower_input: String,
    /// はめあい記号の入力欄。
    dim_tol_fit_input: String,
    /// 公差入力の直近の拒否理由(インライン赤字表示用)。
    dim_tol_input_error: Option<String>,
    /// 桁数の編集状態が「スタイルに従う」チェックを外して明示値へ入っているか。
    /// `all_follow_style`(選択中の注記から算出する値)だけを見てチェック表示を決めると、
    /// チェックを外した直後(まだ `decimals_override` を適用していない)フレームで
    /// `all_follow_style` が再び `true` のままなので即座にチェックが戻ってしまう。
    /// この永続フラグを併用することで「外した」という操作そのものを状態として保持する
    /// (Codex adversarial review 2026-09-04 差し戻し対応A)。`sync_dim_edit_state` と
    /// `reset_transient_ui_state` でリセットする。
    dim_decimals_editing: bool,
    /// 桁数上書きの入力欄(「スタイルに従う」チェックを外したときに使う)。
    dim_decimals_input: String,
    /// 桁数入力の直近の拒否理由。
    dim_decimals_input_error: Option<String>,
    /// 表示値上書き(非比例寸法)の入力欄。
    dim_value_override_input: String,
    /// 寸法スタイルダイアログの作業コピー。`Some` の間はモーダルが開いている
    /// (表題欄編集ダイアログと同じ流儀。M9タスク50-3)。
    dim_style_dialog: Option<DimStyleDialogState>,
}

/// Text ツールの高さ入力欄の既定値（ワールド単位）。既定ビュー（zoom=1）で読める大きさ。
const DEFAULT_TEXT_HEIGHT: &str = "20";

/// ステータスバー2行目のキーバインド凡例。項目ごとに分けて `ui.horizontal_wrapped` へ
/// 渡し、利用可能幅に応じて自動で折り返す（以前は1本の長い文字列で、標準的な
/// ウィンドウ幅だと右端が画面外に切れていた）。
const KEYBIND_LEGEND: &[&str] = &[
    "S=Select",
    "1=Point",
    "L=Line",
    "C=Circle",
    "A=Arc",
    "P=Polyline",
    "T=Text",
    "D=Linear Dim",
    "Shift+D=Radial Dim",
    "G=Diameter Dim",
    "X=Trim",
    "E=Extend",
    "F=Fillet",
    "B=Split",
    "M=Move",
    "R=Rotate",
    "Shift+M=Mirror",
    "O=Offset",
    "Ctrl+D=Duplicate",
    "Del=Delete",
    "Esc=Cancel",
    "F3=Snap",
    "F8=Ortho",
    "F9=Paper View",
    "Ctrl+Z=Undo",
    "Ctrl+Y=Redo",
    "Ctrl+N=New",
    "Ctrl+O=Open",
    "Ctrl+S=Save",
    "Ctrl+Shift+S=Save As",
    "Ctrl+Shift+O=Import DXF",
    "Ctrl+E=Export DXF",
    "Ctrl+Shift+E=Export SVG",
    "Ctrl+P=Export PDF",
    "Home=Zoom Fit",
];

/// ステータスバーに一時表示するメッセージ。
struct StatusMessage {
    /// 表示する文言。
    text: String,
    /// 表示を開始した時刻（`egui::InputState::time`、秒）。
    shown_at: f64,
    /// このメッセージの表示時間（秒）。[`set_status`]（通常）と
    /// [`set_status_important`]（ファイル入出力の結果）で異なる値を使う。
    duration_secs: f64,
}

/// ステータスメッセージを設定する（既存の表示は上書き）。表示時間は通常の操作
/// フィードバック向けの [`STATUS_MESSAGE_SECS`]。
fn set_status(status: &mut Option<StatusMessage>, now: f64, text: impl Into<String>) {
    set_status_with_duration(status, now, text, STATUS_MESSAGE_SECS);
}

/// ステータスメッセージを設定する（既存の表示は上書き）。ファイル入出力の結果通知
/// （開く/保存/DXFインポート・エクスポートの成否）専用で、[`STATUS_MESSAGE_SECS_IMPORTANT`]
/// のぶん通常より長く表示する。
fn set_status_important(status: &mut Option<StatusMessage>, now: f64, text: impl Into<String>) {
    set_status_with_duration(status, now, text, STATUS_MESSAGE_SECS_IMPORTANT);
}

/// [`set_status`] / [`set_status_important`] の共通実装。
fn set_status_with_duration(
    status: &mut Option<StatusMessage>,
    now: f64,
    text: impl Into<String>,
    duration_secs: f64,
) {
    *status = Some(StatusMessage {
        text: text.into(),
        shown_at: now,
        duration_secs,
    });
}

/// `.mcad` 読込成功時のステータス文言（ステータスバーは未日本語化領域のため英語）。
///
/// 旧バージョン（v1〜v3）の線幅移行で上限へ丸めた件数があれば必ず添える。黙って値を
/// 変えたことに気づけるようにするためで、DXF import の skipped 件数表示と同じ流儀
/// （DESIGN.md M8 設計判断6 の規則3）。
fn open_status(clamped_widths: usize) -> String {
    if clamped_widths > 0 {
        format!(
            "Opened file: {clamped_widths} entity(ies) had their legacy line width \
             clamped to {max} mm",
            max = WidthMm::MAX_MM
        )
    } else {
        "Opened file".to_string()
    }
}

/// 新規文書（起動直後・Ctrl+N）用のドキュメントを作る。
///
/// [`Document::new()`] の `"0"` 1枚だけの文書に、[`DEFAULT_EXTRA_LAYERS`] の既定
/// レイヤーを [`Command::AddLayer`] で重ね順つきで追加し、履歴を消して返す。
///
/// # なぜ core（`Document::new()`）ではなく app 側に置くのか
///
/// `Document::new()` は `.mcad` / DXF の読込時にも「再構築の出発点」として使われる
/// （読込はデフォルトレイヤーを [`Command::SetLayerProps`] で上書きし、残りを
/// ファイル内容から [`Command::AddLayer`] する）。既定レイヤーを core へ入れると
/// **既存ファイルを読むたびにファイルに存在しない `文字` 等のレイヤーが生えてしまう**。
/// そのため「新規文書のテンプレート」という UI 上の判断は app 側だけに置き、
/// 読込経路（[`McadApp::open_document`] / [`McadApp::apply_imported_dxf`]）からは
/// 一切呼ばない。
///
/// [`Command::AddLayer`] は検証を持たない（core の `execute` 参照）ので、この経路で
/// 失敗することはない。
///
/// `sheet` は新規文書の図面メタデータ（既定用紙/尺度/様式。M8 タスク41で
/// [`config::Config::default_sheet_meta`] から渡される）。[`Command::SetSheet`] で
/// 適用する — 「ドキュメントの変更は必ず Command 経由」の不変条件（AGENTS.md）を
/// レイヤー追加と同様に守る。標準様式 A/B/C は常に妥当なテンプレートなので
/// （[`SheetMeta::validate`]）、この経路で `SetSheet` が失敗することはない。
fn fresh_document(sheet: SheetMeta) -> Document {
    let mut document = Document::new();
    let mut default_current_layer = None;
    for (index, (name, color, linetype, width_mm)) in DEFAULT_EXTRA_LAYERS.iter().enumerate() {
        let mut layer = Layer::new(*name, *color);
        layer.linetype = *linetype;
        layer.width_mm = WidthMm::new(*width_mm).expect("DEFAULT_EXTRA_LAYERS widths are valid");
        // "0" が order = 0。既定レイヤーはその手前に配列順で積み上げる。
        layer.order = i32::try_from(index).expect("DEFAULT_EXTRA_LAYERS is tiny") + 1;
        let new_ids = document
            .apply(Command::AddLayer(layer))
            .expect("AddLayer on a fresh document cannot fail");
        if *name == DEFAULT_CURRENT_LAYER_NAME {
            default_current_layer = new_ids.layers.first().copied();
        }
    }
    if let Some(layer_id) = default_current_layer {
        document
            .apply(Command::SetCurrentLayer(layer_id))
            .expect("DEFAULT_CURRENT_LAYER_NAME is always added above");
    }
    document
        .apply(Command::SetSheet(sheet))
        .expect("standard title block templates are always valid");
    // 既定レイヤー・カレントレイヤー・図面メタデータの適用自体を Ctrl+Z で巻き戻せて
    // はいけない（読込と同じ扱い）。呼び出し側は clear_history 後の世代（0）を
    // saved_generation の基準点にする。
    document.clear_history();
    document
}

impl McadApp {
    /// 空文書（起動直後の画面）を持つアプリを作る。
    ///
    /// # 起動時サンプルを廃止した経緯
    ///
    /// M3第4段（ファイル操作のapp統合）の Codex レビュー指摘は、「実用的な新規文書/
    /// 読込フローを作る際にはこれを開発用サンプルとして分離または削除すること」だった。
    /// 当時の対応は、**Ctrl+N（新規文書）は必ず [`Document::new()`] のみの真に空の
    /// ドキュメントを作る**（[`McadApp::new_document`] 参照）一方、アプリ起動時
    /// （本関数）はサンプル（線分・円・円弧・ポリライン）を残す、というものだった。
    /// 理由: ここで作るのは「新規文書」ではなく「起動直後の画面」であり、目視確認
    /// （`cargo run -p mcad-app` で起動して形状・スナップ・レイヤー等が一目で見える）
    /// 用の実利があると判断したため。
    ///
    /// M4設計判断2（DESIGN.md 6章「M4: 入出力の一貫性」）でこの判断は覆った:
    /// 作図ツール一式（Point/Line/Circle/Arc/Polyline）が揃った今、起動時サンプルによる
    /// 目視確認という役目は終わったとみなし、起動も Ctrl+N と同じ真に空の
    /// [`Document::new()`] にする。サンプル生成コード自体は削除せず、複数種の
    /// エンティティを要するテストのためのヘルパー（`tests::sample_document`）へ移した。
    ///
    /// # 既定レイヤーセットの導入（レイヤー重ね順の設計 §5）
    ///
    /// 上記の「Ctrl+N も起動時も `Document::new()` のみ」という判断のうち、
    /// **エンティティは一切追加しない**という部分は変わらないが、**レイヤーについては
    /// 覆した**: 起動時・Ctrl+N ともに [`fresh_document`] を使い、`"0"` に加えて
    /// [`DEFAULT_EXTRA_LAYERS`]（M9 タスク53以降は規定4-1準拠の `中心線`/`破線`/
    /// `外形線`/`寸法線`/`文字` の5層）を持つ文書を作る。文字を最前面に置く
    /// といった典型的な運用を初期状態で満たすためで、既定セットを core ではなく app 側に
    /// 置いた理由（ファイル読込時にレイヤーが増えるのを避ける）は [`fresh_document`] の
    /// doc を参照。**読込経路は従来どおり `Document::new()` ベースで再構築する。**
    /// 設定ファイルの実 IO を伴わない既定構成で作る。IO を伴う起動経路は
    /// [`McadApp::with_config`]（`main()` が使う）。既存のテスト・[`Default`] 相当の
    /// 用途はすべてこちら経由のまま（`config::Startup::default()` は既定 [`config::Config`]
    /// を持ち、`config_path` は `None`）。
    fn new() -> Self {
        Self::with_config(config::Startup::default())
    }

    /// 起動時に読み込んだ設定 `startup` を使ってアプリを作る（`main()` 専用の実 IO
    /// 経路）。設定の読込・保存先解決は事前に [`config::load_startup`] が済ませている
    /// ため、ここでは結果を配線するだけでよい。
    fn with_config(startup: config::Startup) -> Self {
        let document = fresh_document(startup.config.default_sheet_meta());
        let mut app = Self {
            // 起動直後（空文書）の世代を保存済み基準点とし、未保存扱いにしない。
            saved_generation: document.generation(),
            document,
            viewport: Viewport::new(),
            tool_kind: ToolKind::Select,
            tool: None,
            select_tool: SelectTool::default(),
            config: startup.config,
            config_path: startup.path,
            snap_marker: None,
            status: None,
            current_path: None,
            confirm_state: ConfirmState::Idle,
            pending_zoom_fit: false,
            last_dialog_dir: None,
            pending_recent_path: None,
            offset_distance_input: String::new(),
            fillet_radius_input: String::new(),
            text_content_input: String::new(),
            text_height_input: DEFAULT_TEXT_HEIGHT.to_owned(),
            text_field_shown: false,
            sheet_dialog: None,
            scale_custom_selected: false,
            scale_custom_input: String::new(),
            scale_input_error: None,
            alt_zoom_anchor: None,
            alt_view_gesture: None,
            dim_edit_target: Vec::new(),
            dim_tol_editing: None,
            dim_tol_symmetric_input: String::new(),
            dim_tol_upper_input: String::new(),
            dim_tol_lower_input: String::new(),
            dim_tol_fit_input: String::new(),
            dim_tol_input_error: None,
            dim_decimals_editing: false,
            dim_decimals_input: String::new(),
            dim_decimals_input_error: None,
            dim_value_override_input: String::new(),
            dim_style_dialog: None,
        };
        if let Some(warning) = startup.warning {
            // 起動時はまだ egui の InputState が無いため now=0.0（[`STATUS_MESSAGE_SECS_IMPORTANT`]
            // 経過後の消去判定は最初のフレームの `now` との比較になる。0.0 起点でも
            // 実用上問題ない — 起動直後の1フレーム目は必ず本物の `now` が極めて小さい値）。
            set_status_important(&mut app.status, 0.0, warning);
        }
        app
    }

    /// モーダル（未保存確認 / 表題欄編集ダイアログ）が開いているか。
    ///
    /// キャンバス入力・ショートカット（F3/F8/F9/Home 含む）のゲートをこの1関数へ
    /// 集約する（M8タスク38。旧 `confirm_state != ConfirmState::Idle` 直書きの集約先）。
    fn modal_open(&self) -> bool {
        self.confirm_state != ConfirmState::Idle
            || self.sheet_dialog.is_some()
            || self.dim_style_dialog.is_some()
    }

    /// 選択集合・進行中の作図ツール・スナップマーカーをリセットする。
    ///
    /// 新規文書・読込の直後に呼ぶ。読込前のドキュメントを参照していた選択
    /// `EntityId` や作図ツールの途中状態を持ち越すと、死んだ ID や不整合な
    /// プレビューが残ってしまうため、ツール種別を `Select` へ戻し選択を空にする。
    fn reset_transient_ui_state(&mut self) {
        self.tool_kind = ToolKind::Select;
        self.tool = None;
        self.select_tool.clear_selection();
        self.select_tool.cancel_placement();
        self.select_tool.cancel_offset();
        self.select_tool.cancel_text_drag();
        // 読込前の距離・半径入力は持ち越さない（別図面では意味が変わるため）。
        self.offset_distance_input.clear();
        self.fillet_radius_input.clear();
        // Text 入力欄も初期化する（文字列はクリア、高さは既定へ戻す）。
        self.text_content_input.clear();
        self.text_height_input = DEFAULT_TEXT_HEIGHT.to_owned();
        self.text_field_shown = false;
        self.snap_marker = None;
        // 表題欄編集ダイアログ・尺度カスタム入力も別図面へ持ち越さない。
        self.sheet_dialog = None;
        self.scale_custom_selected = false;
        self.scale_custom_input.clear();
        self.scale_input_error = None;
        self.alt_zoom_anchor = None;
        self.alt_view_gesture = None;
        // 寸法パネル・寸法スタイルダイアログの編集中状態も別図面へ持ち越さない。
        self.dim_edit_target.clear();
        self.dim_tol_editing = None;
        self.dim_tol_symmetric_input.clear();
        self.dim_tol_upper_input.clear();
        self.dim_tol_lower_input.clear();
        self.dim_tol_fit_input.clear();
        self.dim_tol_input_error = None;
        self.dim_decimals_editing = false;
        self.dim_decimals_input.clear();
        self.dim_decimals_input_error = None;
        self.dim_value_override_input.clear();
        self.dim_style_dialog = None;
    }

    /// undo/redo が成功した直後の UI 状態の後始末。
    ///
    /// undo/redo はエンティティを削除・復活させるため、選択集合から死んだ ID を
    /// 取り除く（[`SelectTool::retain_alive`]）。加えて、配置モード（Ctrl+D 複製の
    /// 基準点確定後〜配置先クリック前）が進行中に選択が変わると、生き残った部分集合
    /// だけの複製が無警告で確定してしまう。これを防ぐため配置モードも解除する
    /// （DESIGN.md 設計判断2: 選択の意図が崩れたら配置は畳む）。
    fn after_history_change(&mut self) {
        self.select_tool.retain_alive(&self.document);
        self.reset_picked_shape_tool();
        self.select_tool.cancel_placement();
        // オフセットモードも解除する（対象が undo/redo で消えたり、選択の意図が崩れたら
        // 宙ぶらりんのオフセットを残さない。設計判断2 と同じ思想）。
        self.select_tool.cancel_offset();
        self.select_tool.cancel_text_drag();
        self.snap_marker = None;
        // 右パネル「寸法」の入力バッファは選択 ID 集合を鮮度キーにしているが、undo/redo は
        // 選択を変えずに注記の中身だけを変える。そのままだと undo で戻した値がバッファに
        // 残り、「確定」で undo を打ち消す新コマンドになる（M9 タスク50 Codex 指摘）。
        // 対象集合を空にして、次フレームの `sync_dim_edit_state` に再同期させる。
        self.dim_edit_target.clear();
    }

    /// ファイル操作（新規・開く・インポート・保存・名前を付けて保存・エクスポート）の
    /// 入口で呼ぶ。進行中の配置モード（Ctrl+D 複製）を、操作の成否やネイティブダイアログ
    /// のキャンセルに関係なく解除する。
    ///
    /// 未保存確認モーダル経由の解除（`confirm_state != Idle` の分岐）や
    /// `reset_transient_ui_state` はモーダルを出す/ドキュメントを置き換える経路しか
    /// カバーせず、保存系（`confirm_state` 不変）やキャンセルされたファイル選択
    /// （`reset_transient_ui_state` に到達しない）では配置モードが武装したまま残る。
    /// その後のキャンバスクリックで意図しない複製が確定するのを防ぐ。
    fn cancel_placement_for_file_op(&mut self) {
        self.select_tool.cancel_placement();
        self.select_tool.cancel_offset();
        self.select_tool.cancel_text_drag();
        self.reset_picked_shape_tool();
        self.snap_marker = None;
    }

    /// ヒットテスト結果をスナップショットとして抱えるツール（トリム・延長・フィレット）を
    /// 初期状態へ戻す（[`ToolKind::caches_picked_shapes`]）。
    ///
    /// undo/redo とファイル操作は、境界やフィレットの 1 本目として選んだエンティティを
    /// 消したり別図面へ差し替えたりしうる。これらはエンティティ ID ではなく形状の
    /// スナップショットとして保持されるため放置すると「もう存在しない線で切る／丸める」
    /// 状態が残る。ツール種別自体は変えず（ユーザーが選んだモードは尊重する）、
    /// インスタンスだけ作り直して初期状態へ戻す。他のツール（Line の連続線分など）は
    /// 既存挙動を保つため触らない。
    fn reset_picked_shape_tool(&mut self) {
        if self.tool_kind.caches_picked_shapes() {
            self.tool = self.tool_kind.spawn();
        }
    }

    /// ファイルダイアログを開く際の初期ディレクトリを決める。
    ///
    /// 優先順位: このセッション中に最後にダイアログで確定したディレクトリ
    /// （[`McadApp::last_dialog_dir`]）→ 現在開いているファイルの親ディレクトリ
    /// （[`McadApp::current_path`]）→ 「最近使ったファイル」先頭の親ディレクトリ
    /// （[`config::Config::recent_files`]、M8 タスク41-2）→ どちらもなければ `None`
    /// （rfd の既定に任せる）。
    fn dialog_start_dir(&self) -> Option<PathBuf> {
        self.last_dialog_dir
            .clone()
            .or_else(|| {
                self.current_path
                    .as_deref()
                    .and_then(Path::parent)
                    .map(Path::to_path_buf)
            })
            .or_else(|| {
                self.config
                    .recent_files
                    .first()
                    .and_then(|p| p.parent())
                    .map(Path::to_path_buf)
            })
    }

    /// ファイルダイアログでユーザーが確定したパスから、その親ディレクトリを
    /// [`McadApp::last_dialog_dir`] へ記憶する。キャンセル時は呼ばない。
    fn remember_dialog_dir(&mut self, path: &Path) {
        if let Some(dir) = path.parent() {
            self.last_dialog_dir = Some(dir.to_path_buf());
        }
    }

    /// ズームフィットを要求する。呼び出し元はファイル読込（`.mcad`/DXF共通）直後
    /// （M4タスク13: 起動状態とズームフィット）と、`Home` キーの手動ズームフィット
    /// （M8タスク42。同じ判定・計算を再利用し、フィットロジックを二重化しない）。
    ///
    /// 呼び出し時点ではキャンバスのスクリーン矩形がまだ確定していない（読込直後）か、
    /// あるいは確定済みでもこの関数自体はそれを持たない（`Home` キー押下時）ため、
    /// フィット計算をその場では行えない。ドキュメントにエンティティが1件以上あれば
    /// [`McadApp::pending_zoom_fit`] を立てて次フレームの `CentralPanel`
    /// （スクリーン矩形確定後）へ計算を委ねる。エンティティが0件（空文書）なら
    /// フィット対象がないため、その場で [`Viewport::new`] の既定ビューへリセットする
    /// （DESIGN.md 6章タスク13: 「空文書は既定ビューへリセット」）。
    fn request_zoom_fit(&mut self) {
        // 枠 ON のときは、エンティティが0件でも用紙矩形がフィット対象になる
        // （`fit_target_aabb` の doc 参照）。
        if self.document.entity_count() > 0 || self.document.sheet().frame_visible {
            self.pending_zoom_fit = true;
        } else {
            self.viewport = Viewport::new();
            self.pending_zoom_fit = false;
        }
    }

    /// ドキュメントに未保存の変更があるか。
    ///
    /// 現在の世代（[`Document::generation`]）が、最後に保存/読込/新規作成した時点の
    /// 世代（[`McadApp::saved_generation`]）と一致しなければ dirty。undo で保存時点の
    /// 内容へ厳密に戻れば世代も一致し dirty でなくなる（[`McadApp::saved_generation`] の
    /// doc 参照）。
    fn is_dirty(&self) -> bool {
        self.document.generation() != self.saved_generation
    }

    /// Ctrl+N: 未保存の変更があれば確認モーダルを出し、なければ即座に新規文書へ置き換える。
    fn request_new_document(&mut self, now: f64) {
        self.cancel_placement_for_file_op();
        if self.is_dirty() {
            self.confirm_state = ConfirmState::ConfirmingNew;
        } else {
            self.new_document(now);
        }
    }

    /// Ctrl+O: 未保存の変更があれば確認モーダルを出し、なければ即座にファイル選択へ進む。
    fn request_open_document(&mut self, now: f64) {
        self.cancel_placement_for_file_op();
        if self.is_dirty() {
            self.confirm_state = ConfirmState::ConfirmingOpen;
        } else {
            self.open_document(now);
        }
    }

    /// Ctrl+Shift+O: 未保存の変更があれば確認モーダルを出し、なければ即座に
    /// DXF ファイル選択へ進む。
    fn request_open_dxf(&mut self, now: f64) {
        self.cancel_placement_for_file_op();
        if self.is_dirty() {
            self.confirm_state = ConfirmState::ConfirmingOpenDxf;
        } else {
            self.open_dxf(now);
        }
    }

    /// Recent メニュー: 未保存の変更があれば確認モーダルを出し、なければ即座に
    /// `path` を読み込む。ネイティブダイアログは一切開かない（M8 タスク41-2）。
    fn request_open_recent(&mut self, path: PathBuf, now: f64) {
        self.cancel_placement_for_file_op();
        if self.is_dirty() {
            self.pending_recent_path = Some(path);
            self.confirm_state = ConfirmState::ConfirmingOpenRecent;
        } else {
            self.open_document_at(&path, now);
        }
    }

    /// Ctrl+D: 選択集合の複製配置モードへ入る。選択が空なら ASCII ステータスメッセージを
    /// 出して何もしない。非空なら基準点クリック待ちに入り、以降のキャンバス入力は
    /// [`handle_select_input`] の配置モード経路が受け取る（DESIGN.md 設計判断2）。
    fn request_duplicate(&mut self, now: f64) {
        if self.select_tool.start_duplicate() {
            set_status(&mut self.status, now, "Duplicate: click base point");
        } else {
            set_status(&mut self.status, now, "Select entities to duplicate");
        }
    }

    /// Text ツールの確定（`AddEntity`）。アンカー（クリック済み）と入力欄の文字列・高さから
    /// [`EntityGeom::Text`] を組み、検証してからカレントレイヤーへ追加する。角度は 0 固定
    /// （向きは回転ツールで変える。DESIGN.md M6 設計判断6）。
    ///
    /// 成功したら文字列欄をクリアしてアンカーを未確定へ戻し（連続作図。高さは保持）、`true`。
    /// 高さが不正・文字列が空・レイヤーロック等で失敗したらステータスへ ASCII で理由を出し、
    /// アンカー・入力はそのまま残して `false`（再入力・レイヤー変更でリトライできる）。
    fn commit_text(&mut self, anchor: Point2, now: f64) -> bool {
        let Some(height) = parse_text_height(&self.text_height_input) else {
            set_status(
                &mut self.status,
                now,
                "Invalid text height - enter a positive number",
            );
            return false;
        };
        let geom = EntityGeom::Text(TextGeom {
            anchor,
            content: self.text_content_input.clone(),
            height,
            angle: 0.0,
        });
        // 空文字列・非有限などは validate が ASCII の理由で弾く（規約: 可視文字列は ASCII）。
        if let Err(reason) = geom.validate() {
            set_status(&mut self.status, now, format!("Cannot add text: {reason}"));
            return false;
        }
        let cmd = Command::AddEntity(Entity::new(
            geom,
            resolve_tool_layer(&self.document, ToolKind::Text),
            Style::inherited(),
        ));
        match self.document.apply(cmd) {
            Ok(_) => {
                set_status(&mut self.status, now, "Text added");
                // 連続作図: アンカーを未確定へ戻し、文字列だけクリア（高さは既定として保持）。
                // 入力欄の表示フラグ（text_field_shown）はパネル側の set_text_field_shown が
                // 管理するのでここでは触らない。
                self.text_content_input.clear();
                self.tool = ToolKind::Text.spawn();
                true
            }
            Err(err) => {
                set_status(&mut self.status, now, format!("Add text failed: {err}"));
                false
            }
        }
    }

    /// Text 入力欄の表示状態を更新する。**表示→非表示へ転じたフレームで入力中の
    /// 文字列を捨てる**（Esc でのアンカー破棄・ツール切替・確定後など、どの経路で
    /// 非表示になっても前回入力が次のテキストへ持ち越されないようにする）。
    ///
    /// GUI なしで検証できるよう、この遷移ロジックだけを純関数的に切り出している。
    fn set_text_field_shown(&mut self, shown: bool) {
        if self.text_field_shown && !shown {
            self.text_content_input.clear();
        }
        self.text_field_shown = shown;
    }

    /// 設定を config.json へ書く（M8 タスク41）。保存先不明（`config_path` が `None`。
    /// 設定ディレクトリが解決できない環境で起動時に警告済み）なら何もしない。
    /// 書込み失敗はステータスバーへ通知するのみで、アプリの動作は継続する。
    fn persist_config(&mut self, now: f64) {
        let Some(path) = self.config_path.clone() else {
            return;
        };
        if let Err(err) = config::save(&path, &self.config) {
            set_status_important(
                &mut self.status,
                now,
                format!("Settings save failed: {err}"),
            );
        }
    }

    /// 新規ドキュメントへ置き換える（エンティティは一切追加せず、レイヤーは既定セット
    /// [`DEFAULT_EXTRA_LAYERS`] のみ。[`McadApp::new`] / [`fresh_document`] の doc 参照）。
    ///
    /// 未保存の変更があるかどうかは確認しない。呼び出し側（[`McadApp::request_new_document`]
    /// または確認モーダルの「破棄して続行」選択）が確認済みであることを前提とする。
    fn new_document(&mut self, now: f64) {
        self.document = fresh_document(self.config.default_sheet_meta());
        self.current_path = None;
        // 新規ドキュメントの現在世代を保存済み基準点にする（読込直後は未保存でない）。
        self.saved_generation = self.document.generation();
        self.reset_transient_ui_state();
        // 新規文書は常に空なのでフィット対象がない。一貫性のため既定ビューへ戻す
        // （M4タスク13。DESIGN.md 6章の検収基準には明記されていないが望ましい挙動）。
        self.viewport = Viewport::new();
        self.pending_zoom_fit = false;
        set_status(&mut self.status, now, "New document");
    }

    /// ネイティブのファイル選択ダイアログ（`.mcad` フィルタ付き）で選んだファイルを
    /// 読み込む。
    ///
    /// 未保存の変更があるかどうかは確認しない。呼び出し側（[`McadApp::request_open_document`]
    /// または確認モーダルの「破棄して続行」選択）が確認済みであることを前提とする。
    /// 読込失敗時は現在のドキュメントを一切変更せず、理由をステータスバーへ表示する。
    fn open_document(&mut self, now: f64) {
        let mut dialog = rfd::FileDialog::new().add_filter("mcad", &[MCAD_EXTENSION]);
        if let Some(dir) = self.dialog_start_dir() {
            dialog = dialog.set_directory(dir);
        }
        let Some(path) = dialog.pick_file() else {
            return;
        };
        self.open_document_at(&path, now);
    }

    /// [`McadApp::open_document`] のダイアログ非依存部分。ネイティブダイアログを
    /// 一切開かないため headless テストで直接検証できる（Recent メニュー
    /// （[`McadApp::request_open_recent`]）からも呼ぶ、M8 タスク41-2）。
    ///
    /// 未保存の変更があるかどうかは確認しない。呼び出し側が確認済みであることを
    /// 前提とする。成功時は「最後にダイアログで確定したディレクトリ」
    /// （[`McadApp::remember_dialog_dir`]）と「最近使ったファイル」
    /// （[`config::Config::push_recent`]）を更新し、config.json へ保存する
    /// （[`McadApp::persist_config`]）。失敗時はドキュメント・設定を一切変更しない。
    fn open_document_at(&mut self, path: &Path, now: f64) {
        self.remember_dialog_dir(path);
        match load_mcad(path) {
            Ok(LoadSummary {
                document,
                clamped_widths,
            }) => {
                self.document = document;
                self.current_path = Some(path.to_path_buf());
                // load_mcad は再構築後に clear_history 済みで世代が基準点に戻っている。
                // 読込直後を未保存でない状態にするため、その世代へ合わせる。
                self.saved_generation = self.document.generation();
                self.reset_transient_ui_state();
                self.request_zoom_fit();
                self.config.push_recent(path.to_path_buf());
                self.persist_config(now);
                set_status_important(&mut self.status, now, open_status(clamped_widths));
            }
            Err(err) => {
                set_status_important(&mut self.status, now, format!("Open failed: {err}"));
            }
        }
    }

    /// ネイティブのファイル選択ダイアログ（`.dxf` フィルタ付き）で選んだ DXF ファイルを
    /// import する。
    ///
    /// 未保存の変更があるかどうかは確認しない。呼び出し側（[`McadApp::request_open_dxf`]
    /// または確認モーダルの「破棄して続行」選択）が確認済みであることを前提とする。
    /// import失敗時は現在のドキュメントを一切変更せず、理由をステータスバーへ表示する。
    ///
    /// DESIGN.md 6章 設計判断1（DXFは`.mcad`と混同しない）に従い、成功時は
    /// `current_path = None` とし、`saved_generation` を [`DXF_IMPORT_SAVED_GENERATION_SENTINEL`]
    /// へ設定して必ず dirty=true にする（doc 参照）。これにより直後の Ctrl+S は
    /// `save_document` → `save_document_as` 経由で「名前を付けて`.mcad`保存」ダイアログへ
    /// 誘導され、元の DXF ファイルは上書きされない。
    fn open_dxf(&mut self, now: f64) {
        let mut dialog = rfd::FileDialog::new().add_filter("dxf", &[DXF_EXTENSION]);
        if let Some(dir) = self.dialog_start_dir() {
            dialog = dialog.set_directory(dir);
        }
        let Some(path) = dialog.pick_file() else {
            return;
        };
        self.remember_dialog_dir(&path);
        match load_dxf(&path) {
            Ok(summary) => self.apply_imported_dxf(summary, now),
            Err(err) => {
                set_status_important(&mut self.status, now, format!("DXF import failed: {err}"));
            }
        }
    }

    /// [`McadApp::open_dxf`] のダイアログ非依存部分。`load_dxf` が返した
    /// [`ImportSummary`] をアプリ状態へ反映する（ネイティブダイアログを一切開かない
    /// ため headless テストで直接検証できる）。
    fn apply_imported_dxf(&mut self, summary: ImportSummary, now: f64) {
        let ImportSummary {
            document,
            skipped_entities,
            clamped_line_widths,
        } = summary;
        self.document = document;
        self.current_path = None;
        self.saved_generation = DXF_IMPORT_SAVED_GENERATION_SENTINEL;
        self.reset_transient_ui_state();
        self.request_zoom_fit();
        let mut notes = Vec::new();
        if skipped_entities > 0 {
            notes.push(format!(
                "{skipped_entities} entity(ies) skipped (unsupported type)"
            ));
        }
        if clamped_line_widths > 0 {
            notes.push(format!(
                "{clamped_line_widths} line width(s) clamped to the 0.05..=5.0mm range"
            ));
        }
        let message = if notes.is_empty() {
            "Imported DXF file; layer locks are not restored from DXF".to_string()
        } else {
            format!(
                "Imported DXF: {}; layer locks are not restored from DXF",
                notes.join(", ")
            )
        };
        set_status_important(&mut self.status, now, message);
    }

    /// Ctrl+E: ネイティブの保存ダイアログで選んだ先へ現在のドキュメントを DXF として
    /// エクスポートする。
    ///
    /// エクスポートは既存ドキュメントを変更しない読み取り専用操作なので、未保存の
    /// 変更があっても確認モーダルは出さない。成功しても `current_path`・
    /// `saved_generation` は一切変更しない（DESIGN.md 6章 設計判断1: DXFは交換用
    /// 形式であり「保存」とは別物として扱う。dirty 状態は変わらない）。
    fn export_dxf_file(&mut self, now: f64) {
        self.cancel_placement_for_file_op();
        let default_name = self
            .current_path
            .as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|n| n.to_str())
            .map_or_else(
                || DEFAULT_DXF_FILE_NAME.to_string(),
                |stem| format!("{stem}.{DXF_EXTENSION}"),
            );
        let mut dialog = rfd::FileDialog::new()
            .add_filter("dxf", &[DXF_EXTENSION])
            .set_file_name(&default_name);
        if let Some(dir) = self.dialog_start_dir() {
            dialog = dialog.set_directory(dir);
        }
        let Some(path) = dialog.save_file() else {
            return;
        };
        self.remember_dialog_dir(&path);
        let path = ensure_dxf_extension(path);
        match save_dxf(&self.document, &path) {
            Ok(0) => {
                set_status_important(&mut self.status, now, "Exported DXF file");
            }
            Ok(skipped) => {
                // 寸法（長さ/半径）は DXF 未対応でスキップされる（Text はタスク25で
                // export 対応済み）。黙って消えるとデータロスに気づけないため、件数を
                // ステータスへ出す（import 側メッセージと同書式）。
                set_status_important(
                    &mut self.status,
                    now,
                    format!(
                        "Exported DXF file: {skipped} entity(ies) skipped \
                         (dimension not supported)"
                    ),
                );
            }
            Err(err) => {
                set_status_important(&mut self.status, now, format!("DXF export failed: {err}"));
            }
        }
    }

    /// Ctrl+Shift+E: ネイティブの保存ダイアログで選んだ先へ現在のドキュメントを SVG
    /// として書き出す。
    ///
    /// DXF エクスポートと同じく読み取り専用操作なので、未保存の変更があっても確認
    /// モーダルは出さず、成功しても `current_path`・`saved_generation` は変更しない
    /// （M8 タスク39-3。SVG は plot IR の直列化であって「保存」ではない）。
    fn export_svg_file(&mut self, now: f64) {
        self.cancel_placement_for_file_op();
        let default_name = self
            .current_path
            .as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|n| n.to_str())
            .map_or_else(
                || DEFAULT_SVG_FILE_NAME.to_string(),
                |stem| format!("{stem}.{SVG_EXTENSION}"),
            );
        let mut dialog = rfd::FileDialog::new()
            .add_filter("svg", &[SVG_EXTENSION])
            .set_file_name(&default_name);
        if let Some(dir) = self.dialog_start_dir() {
            dialog = dialog.set_directory(dir);
        }
        let Some(path) = dialog.save_file() else {
            return;
        };
        self.remember_dialog_dir(&path);
        let path = ensure_svg_extension(path);
        let page = plot::plot_page(&self.document, self.config.plot_color_mode);
        let svg = plot::to_svg(&page);
        match std::fs::write(&path, svg) {
            Ok(()) => {
                set_status_important(&mut self.status, now, "Exported SVG file");
            }
            Err(err) => {
                set_status_important(&mut self.status, now, format!("SVG export failed: {err}"));
            }
        }
    }

    /// Ctrl+P: ネイティブの保存ダイアログで選んだ先へ現在のドキュメントを PDF
    /// として書き出す。
    ///
    /// DXF/SVG エクスポートと同じく読み取り専用操作なので、未保存の変更があっても
    /// 確認モーダルは出さず、成功しても `current_path`・`saved_generation` は変更
    /// しない（M8 タスク40。PDF は plot IR の直列化であって「保存」ではない）。
    fn export_pdf_file(&mut self, now: f64) {
        self.cancel_placement_for_file_op();
        let default_name = self
            .current_path
            .as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|n| n.to_str())
            .map_or_else(
                || DEFAULT_PDF_FILE_NAME.to_string(),
                |stem| format!("{stem}.{PDF_EXTENSION}"),
            );
        let mut dialog = rfd::FileDialog::new()
            .add_filter("pdf", &[PDF_EXTENSION])
            .set_file_name(&default_name);
        if let Some(dir) = self.dialog_start_dir() {
            dialog = dialog.set_directory(dir);
        }
        let Some(path) = dialog.save_file() else {
            return;
        };
        self.remember_dialog_dir(&path);
        let path = ensure_pdf_extension(path);
        let page = plot::plot_page(&self.document, self.config.plot_color_mode);
        let bytes = plot::to_pdf(&page);
        match std::fs::write(&path, bytes) {
            Ok(()) => {
                set_status_important(&mut self.status, now, "Exported PDF file");
            }
            Err(err) => {
                set_status_important(&mut self.status, now, format!("PDF export failed: {err}"));
            }
        }
    }

    /// Ctrl+S: 開いているファイルパスへ上書き保存する。パスが未定なら
    /// 「名前を付けて保存」（[`McadApp::save_document_as`]）と同じ扱いにする。
    fn save_document(&mut self, now: f64) {
        self.cancel_placement_for_file_op();
        let Some(path) = self.current_path.clone() else {
            self.save_document_as(now);
            return;
        };
        self.save_to(&path, now);
    }

    /// Ctrl+Shift+S: 常にネイティブの保存ダイアログを表示し、選んだ先へ保存する。
    /// 成功時は「現在開いているファイルパス」を選んだ先に更新する。
    fn save_document_as(&mut self, now: f64) {
        self.cancel_placement_for_file_op();
        let default_name = self
            .current_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or(DEFAULT_FILE_NAME);
        let mut dialog = rfd::FileDialog::new()
            .add_filter("mcad", &[MCAD_EXTENSION])
            .set_file_name(default_name);
        if let Some(dir) = self.dialog_start_dir() {
            dialog = dialog.set_directory(dir);
        }
        let Some(path) = dialog.save_file() else {
            return;
        };
        self.remember_dialog_dir(&path);
        // ネイティブダイアログ（特に Linux の xdg-portal 経由）は必ずしも拡張子を
        // 自動付与しないため、`.mcad` 以外（無し含む）なら明示的に付け直す。
        let path = ensure_mcad_extension(path);
        self.save_to(&path, now);
    }

    /// 指定パスへ保存する。成功時は「現在開いているファイルパス」を更新し dirty を
    /// 解除する。失敗時はドキュメント・パスを変更せず、理由をステータスバーへ表示する。
    fn save_to(&mut self, path: &Path, now: f64) {
        match save_mcad(&self.document, path) {
            Ok(()) => {
                self.current_path = Some(path.to_path_buf());
                // 保存成功時点の世代を記録する。以後この世代と一致する限り未保存でない。
                self.saved_generation = self.document.generation();
                self.config.push_recent(path.to_path_buf());
                self.persist_config(now);
                set_status_important(&mut self.status, now, "Saved");
            }
            Err(err) => {
                set_status_important(&mut self.status, now, format!("Save failed: {err}"));
            }
        }
    }
}

impl Default for McadApp {
    fn default() -> Self {
        Self::new()
    }
}

/// パスの拡張子が `.mcad`（大小無視）でなければ付け直す。
fn ensure_mcad_extension(path: PathBuf) -> PathBuf {
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(MCAD_EXTENSION))
    {
        path
    } else {
        path.with_extension(MCAD_EXTENSION)
    }
}

/// パスの拡張子が `ext`（大小無視）でなければ付け直す（`ensure_dxf_extension` 等の
/// 共通実装。M8 タスク40 で3例目の `ensure_pdf_extension` を作る前に共通化した）。
fn ensure_extension(path: PathBuf, ext: &str) -> PathBuf {
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
    {
        path
    } else {
        path.with_extension(ext)
    }
}

/// パスの拡張子が `.dxf`（大小無視）でなければ付け直す。
fn ensure_dxf_extension(path: PathBuf) -> PathBuf {
    ensure_extension(path, DXF_EXTENSION)
}

/// パスの拡張子が `.svg`（大小無視）でなければ付け直す。
fn ensure_svg_extension(path: PathBuf) -> PathBuf {
    ensure_extension(path, SVG_EXTENSION)
}

/// パスの拡張子が `.pdf`（大小無視）でなければ付け直す。
fn ensure_pdf_extension(path: PathBuf) -> PathBuf {
    ensure_extension(path, PDF_EXTENSION)
}

/// ドキュメント中の全エンティティを包む AABB。エンティティが1つもなければ `None`
/// （M4タスク13: ズームフィット対象の算出。[`Viewport::fit_to_aabb`] に渡す）。
///
/// Text は表示上のワールド AABB（[`text_world_aabb`]、`height * k`）を使う（タスク37。
/// 判断(c)により Text はトグル非依存で常に `height * k` のため、ズームフィットも
/// これに合わせないと拡大された文字がフィット範囲からはみ出す）。
fn document_aabb(document: &Document) -> Option<Aabb> {
    let k = document.sheet().scale.world_mm_per_paper_mm();
    document
        .entities()
        .map(|(_, entity)| match &entity.geom {
            EntityGeom::Text(text) => text_world_aabb(text, k),
            _ => entity.geom.aabb(),
        })
        .reduce(|acc, bb| acc.union(&bb))
}

/// ズームフィット対象の AABB（M8タスク38: 枠考慮）。
///
/// `sheet.frame_visible` が ON のとき、[`document_aabb`] へ用紙矩形（ワールド
/// `(0,0)〜(W*k, H*k)`）を合併する。枠 ON で図形が1件もない図面でも、用紙全体が
/// フィット対象になる（ユーザー確定の挙動。[`McadApp::request_zoom_fit`] の判定も
/// これに合わせて拡張している）。
fn fit_target_aabb(document: &Document) -> Option<Aabb> {
    let doc_aabb = document_aabb(document);
    let sheet = document.sheet();
    if !sheet.frame_visible {
        return doc_aabb;
    }
    let (w, h) = sheet.paper_extent_mm();
    let k = sheet.scale.world_mm_per_paper_mm();
    let paper_aabb = Aabb::new(
        paper_to_world(Point2::new(0.0, 0.0), k),
        paper_to_world(Point2::new(w, h), k),
    );
    Some(doc_aabb.map_or(paper_aabb, |a| a.union(&paper_aabb)))
}

/// アプリのグローバルキーボードショートカット（undo/redo・ファイル操作・Ctrl+D 複製・
/// ツール切替・Delete 等）を処理してよいか。
///
/// - モーダル表示中（未保存確認 / 表題欄編集ダイアログ。[`McadApp::modal_open`]）は、
///   裏でドキュメントが変わる副作用を防ぐため抑止する。
/// - テキスト入力欄にフォーカスがある間（オフセット距離入力欄の編集中など）は抑止する。
///   タイプした `Ctrl+Z` がドキュメントを undo する、`d` でツールが切り替わる、といった
///   テキスト入力とショートカットの競合を防ぐ（DESIGN.md M5 設計判断5 の 2026-07-19 追記）。
///
/// egui のフォーカス判定を `bool` で受け取り、この方針を GUI なしで単体テストできるようにする。
fn app_shortcuts_enabled(modal_open: bool, text_focused: bool) -> bool {
    !modal_open && !text_focused
}

/// キャンバス選択操作のうちキー入力で発火するもの（Delete/Backspace による削除、
/// Esc による選択解除・ドラッグ中断）を処理してよいか。
///
/// `handle_select_input` は `!self.modal_open()` の内側でのみ呼ばれる（モーダル表示中は
/// 呼び出し自体が起きない）ため、ここではテキスト入力欄フォーカスだけを見る。右パネルの
/// 数値入力欄（寸法パネルの桁数/公差欄、オフセット距離欄など）にフォーカスがある間に
/// Backspace で文字を消そうとしただけで選択中のエンティティが削除される、という事故を
/// 防ぐ（Codex adversarial review 2026-09-04 差し戻し対応C）。
#[inline]
#[must_use]
fn select_canvas_key_shortcuts_enabled(text_focused: bool) -> bool {
    !text_focused
}

impl eframe::App for McadApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let now = ui.input(|i| i.time);

        // テキスト入力欄（オフセット距離入力欄など）にフォーカスがあるか。egui のフォーカスは
        // フレームをまたいで保持されるので、パネル描画より前のここで前フレームの状態を読めば、
        // 編集中のショートカット競合を全経路まとめて抑止できる（`app_shortcuts_enabled`）。
        let text_focused = ui.memory(|m| m.focused().is_some());

        // 未保存確認モーダル表示中（`confirm_state != Idle`）は、履歴操作・ファイル
        // 操作ショートカットを一切処理しない。モーダル表示中に Ctrl+N 等が先に走ると、
        // 例えば「閉じる」確認中に `confirm_state` が `ConfirmingNew` へ上書きされ、
        // モーダルの文言が終了確認から新規文書確認へすり替わってしまうため。
        // 加えて、テキスト入力欄の編集中も全ショートカットを抑止する（上記フォーカスゲート）。
        if app_shortcuts_enabled(self.modal_open(), text_focused) {
            // Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y で undo/redo。ツール切替キーより先に処理する
            // （handle_tool_shortcut_keys は修飾キー付きの入力を無視するので衝突はしないが、
            // 「履歴操作が最優先」という意図を並び順でも示す）。undo/redo はエンティティを
            // 削除・復活させるため、直後に選択集合から死んだ ID を取り除く。
            let (undo_pressed, redo_pressed) = ui.input(|i| {
                let cmd = i.modifiers.command;
                (
                    cmd && !i.modifiers.shift && i.key_pressed(Key::Z),
                    cmd && (i.key_pressed(Key::Y) || (i.modifiers.shift && i.key_pressed(Key::Z))),
                )
            });
            // undo/redo による dirty 状態の変化は世代カウンタが自動で表す
            // （[`McadApp::is_dirty`] が `document.generation()` を見る）。ここで明示的に
            // dirty を立てる必要はなく、保存時点へ厳密に戻れば自然と `*` が消える。
            if undo_pressed && self.document.undo() {
                self.after_history_change();
            }
            if redo_pressed && self.document.redo() {
                self.after_history_change();
            }

            // Ctrl+N/Ctrl+O/Ctrl+S/Ctrl+Shift+S: 新規/開く/保存/名前を付けて保存。
            // Ctrl+Shift+O/Ctrl+E: DXF を開く/DXF へ書き出す。
            // Ctrl+Shift+E: SVG へ書き出す（M8 タスク39-3）。
            // Ctrl+P: PDF へ書き出す（M8 タスク40）。
            // undo/redo と同様、ツール切替キー（`handle_tool_shortcut_keys`）は Ctrl 併用を
            // 無視するので衝突しない（P 単押しは Polyline ツール切替だが、Ctrl 併用は
            // `handle_tool_shortcut_keys` 側の command 早期 return で除外される）。
            let (
                new_pressed,
                open_pressed,
                save_pressed,
                save_as_pressed,
                open_dxf_pressed,
                export_dxf_pressed,
                export_svg_pressed,
                export_pdf_pressed,
                duplicate_pressed,
            ) = ui.input(|i| {
                let cmd = i.modifiers.command;
                (
                    cmd && !i.modifiers.shift && i.key_pressed(Key::N),
                    cmd && !i.modifiers.shift && i.key_pressed(Key::O),
                    cmd && !i.modifiers.shift && i.key_pressed(Key::S),
                    cmd && i.modifiers.shift && i.key_pressed(Key::S),
                    cmd && i.modifiers.shift && i.key_pressed(Key::O),
                    cmd && !i.modifiers.shift && i.key_pressed(Key::E),
                    cmd && i.modifiers.shift && i.key_pressed(Key::E),
                    cmd && !i.modifiers.shift && i.key_pressed(Key::P),
                    cmd && !i.modifiers.shift && i.key_pressed(Key::D),
                )
            });
            if new_pressed {
                self.request_new_document(now);
            }
            if open_pressed {
                self.request_open_document(now);
            }
            if save_pressed {
                self.save_document(now);
            }
            if save_as_pressed {
                self.save_document_as(now);
            }
            if open_dxf_pressed {
                self.request_open_dxf(now);
            }
            if export_dxf_pressed {
                self.export_dxf_file(now);
            }
            if export_svg_pressed {
                self.export_svg_file(now);
            }
            if export_pdf_pressed {
                self.export_pdf_file(now);
            }
            if duplicate_pressed {
                self.request_duplicate(now);
            }
        }

        // モーダル（未保存確認 / 表題欄編集ダイアログ）が出ている間は配置モード
        // （Ctrl+D 複製等）を解除する。モーダル表示中はキャンバス入力がゲートされ
        // 配置を進められないため、宙ぶらりんの配置ステートを残さない
        // （DESIGN.md 設計判断2: モーダル表示は配置モードを解除）。
        // Ctrl+N/Ctrl+O 等でモーダルを開いたのがこのフレームでも、上のショートカット処理で
        // `confirm_state` が更新済みなので同フレームで確実に解除できる。
        if self.modal_open() {
            if self.select_tool.is_placing() || self.select_tool.is_offsetting() {
                self.select_tool.cancel_placement();
                self.select_tool.cancel_offset();
                self.select_tool.cancel_text_drag();
                self.snap_marker = None;
            }
            // トリム・延長の途中状態（採取済みの境界）もモーダル表示で畳む
            // （DESIGN.md M7 設計判断6 の共通規約: モーダル表示は初期状態へ戻す）。
            self.reset_picked_shape_tool();
        }

        // ウィンドウを閉じる操作（OSの閉じるボタン等）を検出する。未保存の変更が
        // あれば OS 側のクローズを即キャンセルし、egui 内製の非ブロッキングモーダル
        // （下部の `confirm_state == ConfirmingClose` 描画）で破棄可否を確認する。
        // ネイティブダイアログを `ui()` 中に同期表示するとイベントループが止まり
        // 「応答なし」になるため、この経路ではネイティブ確認ダイアログを使わない。
        //
        // eframe 0.35 のネイティブランナー（`epi_integration::update`）は `ui()` 中に
        // 積まれた viewport コマンドを同フレームの close 判定に使うため、ここで
        // `ViewportCommand::CancelClose` を送れば間に合う（`close_requested()` で検出、
        // `CancelClose` でキャンセル、という eframe 側の推奨手順どおり）。
        //
        // 「破棄して終了」で自分が `ViewportCommand::Close` を送ると次フレームで再び
        // `close_requested` が立つが、そのとき dirty はまだ true のままなので、単純な
        // ロジックだと再度 CancelClose してモーダルが出っぱなしになり永久に閉じられない。
        // これを防ぐため `Closing` 状態では close 要求をそのまま OS へ通す。
        let close_requested = ui.ctx().input(|i| i.viewport().close_requested());
        if close_requested {
            match self.confirm_state {
                // 破棄確定済み: 何もせず OS クローズを通す。
                ConfirmState::Closing => {}
                _ => {
                    if self.is_dirty() {
                        // OS 側クローズを即キャンセルし、次フレームで egui モーダルを描く。
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::CancelClose);
                        self.confirm_state = ConfirmState::ConfirmingClose;
                    }
                    // dirty でなければ Idle のまま OS クローズを通す（何もしない）。
                }
            }
        }

        // ウィンドウタイトルへ現在のファイル名（未定なら Untitled）と dirty 状態
        // （末尾の `*`）を表示する。`ViewportCommand::Title` の送信は軽量なので
        // 変化の有無を追跡せず毎フレーム送ってよい。
        let file_label = self
            .current_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map_or_else(|| UNTITLED_LABEL.to_string(), str::to_string);
        let dirty_marker = if self.is_dirty() { "*" } else { "" };
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Title(format!(
                "mcad - {file_label}{dirty_marker}"
            )));

        // モーダル表示中・テキスト欄フォーカス中はツール切替（S/1/L/C/A/P）を処理しない。
        // ツール切替はモーダルの裏で作図ツールが起動する副作用があり、また距離入力欄の編集中に
        // 素の文字キーでツールが切り替わりオフセットモードが解除されるのを防ぐ
        // （AGENTS.md「テキスト入力欄がない前提」はオフセット距離欄の追加で崩れるため、
        // `app_shortcuts_enabled` の共通ゲートで抑止する）。
        if app_shortcuts_enabled(self.modal_open(), text_focused) {
            handle_tool_shortcut_keys(
                ui,
                &mut self.tool_kind,
                &mut self.tool,
                &mut self.select_tool,
            );
        }

        // M8タスク42: Home キーで手動ズームフィット（図面全体が収まるようフィット）。
        // Home はテキスト入力欄ではカーソルを行頭へ移動する一般的なキーなので、
        // ツール切替と同じ `app_shortcuts_enabled`（テキスト欄フォーカス中は抑止）
        // ゲートに合わせる。読込直後の自動フィットと同じ判定・計算を
        // `request_zoom_fit` の再利用で行う（ロジックの二重化を避ける）。
        if app_shortcuts_enabled(self.modal_open(), text_focused)
            && ui.input(|i| i.key_pressed(Key::Home))
        {
            self.request_zoom_fit();
        }

        // F3 でスナップの有効/無効をトグルする（作図時の吸着を一時的に切りたい場面用）。
        // ファンクションキーはテキスト入力と競合しないので、モーダル非表示中なら常に効かせる。
        if !self.modal_open() && ui.input(|i| i.key_pressed(Key::F3)) {
            self.config.snap_enabled = !self.config.snap_enabled;
            if !self.config.snap_enabled {
                self.snap_marker = None;
            }
            self.persist_config(now);
        }

        // F8 で直交モード（ortho）の有効/無効をトグルする。F3 と同じガード条件
        // （モーダル非表示中は常に効く）。ortho は専用マーカーを持たない設計
        // （`ortho.rs` doc 参照）なので、トグル自体はフラグの反転のみでよい。
        if !self.modal_open() && ui.input(|i| i.key_pressed(Key::F8)) {
            self.config.ortho_enabled = !self.config.ortho_enabled;
            self.persist_config(now);
        }

        // F9 で紙基準表示（タスク37。旧: 線幅表示、タスク36b。AutoCAD の LWDISPLAY 相当）の
        // 有効/無効をトグルする。F3/F8 と同じガード条件（モーダル非表示中は常に効く）。
        // 専用マーカーは不要なので、トグル自体はフラグの反転のみでよい（ortho と同じ形）。
        if !self.modal_open() && ui.input(|i| i.key_pressed(Key::F9)) {
            self.config.paper_display_enabled = !self.config.paper_display_enabled;
            self.persist_config(now);
        }

        // 表示時間を過ぎたステータスメッセージは消す。
        if self
            .status
            .as_ref()
            .is_some_and(|m| now - m.shown_at > m.duration_secs)
        {
            self.status = None;
        }

        // Recent メニュー（M8 タスク41-2）。対象は `.mcad` のみ（DXF import は対象外、
        // AGENTS.md/DESIGN.md 判断8参照）。クリックはこのパネルクロージャの外
        // （既存のショートカット処理と同じ流儀）で処理する。
        let mut clicked_recent: Option<PathBuf> = None;
        egui::Panel::top("tool_status").show(ui, |ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(format!("File: {file_label}{dirty_marker}"));
                    ui.separator();
                    ui.menu_button("Recent", |ui| {
                        let existing: Vec<&PathBuf> = self
                            .config
                            .recent_files
                            .iter()
                            .filter(|p| p.exists())
                            .collect();
                        if existing.is_empty() {
                            ui.label("(no recent files)");
                        }
                        for path in existing {
                            let label = path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .map_or_else(|| path.display().to_string(), str::to_owned);
                            if ui
                                .button(label)
                                .on_hover_text(path.display().to_string())
                                .clicked()
                            {
                                clicked_recent = Some(path.clone());
                                ui.close();
                            }
                        }
                    });
                    ui.separator();
                    ui.label(format!("Tool: {}", self.tool_kind.label()));
                    ui.separator();
                    ui.label(format!(
                        "Snap: {}",
                        if self.config.snap_enabled {
                            "ON"
                        } else {
                            "OFF"
                        }
                    ));
                    ui.separator();
                    ui.label(format!(
                        "Ortho: {}",
                        if self.config.ortho_enabled {
                            "ON"
                        } else {
                            "OFF"
                        }
                    ));
                    ui.separator();
                    ui.label(format!(
                        "Paper view: {}",
                        if self.config.paper_display_enabled {
                            "ON"
                        } else {
                            "OFF"
                        }
                    ));
                    ui.separator();
                    let grid_step = viewport::nice_grid_step(self.viewport.zoom, GRID_TARGET_PX);
                    ui.label(format!("Grid: {} mm", format_grid_step(grid_step)));
                    ui.separator();
                    // オフセット距離入力欄（設計判断5）。モーダルにせず上部パネルへ常設だが、
                    // 画面を圧迫しないようオフセットモード中のみ表示する。空欄なら通過点方式
                    // （hint の "through"）、正の有限値なら距離固定＋クリックは側の決定のみ。
                    if self.select_tool.is_offsetting() {
                        ui.label("Offset dist:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.offset_distance_input)
                                .desired_width(56.0)
                                .hint_text("through"),
                        );
                        ui.separator();
                    }
                    // フィレット半径入力欄（M7 タスク32、設計判断6）。オフセット距離欄と同じ
                    // 常設パネル方式で、フィレットツール中のみ表示する。フォーカス中の
                    // ショートカット抑止は既存の `app_shortcuts_enabled`（text_focused）が
                    // 全入力欄まとめて担うので、ここでは何もしなくてよい。
                    if self.tool_kind == ToolKind::Fillet {
                        ui.label("Fillet radius:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.fillet_radius_input)
                                .desired_width(56.0)
                                .hint_text("radius"),
                        );
                        ui.separator();
                    }
                    // Text ツールの文字列・高さ入力欄（M6 タスク23）。アンカー確定後のみ表示し、
                    // Enter で AddEntity。文字列欄は IME 入力（CJK）可。フォーカス管理・
                    // ショートカット抑止は既存の `app_shortcuts_enabled`（text_focused）を流用。
                    let text_anchor = if self.tool_kind == ToolKind::Text {
                        self.tool.as_ref().and_then(|t| t.pending_text_anchor())
                    } else {
                        None
                    };
                    // アンカー確定直後の初回表示（false→true 遷移）で文字列欄へフォーカスを移す。
                    let first_show = text_anchor.is_some() && !self.text_field_shown;
                    if let Some(anchor) = text_anchor {
                        ui.label("Text:");
                        let content_resp = ui.add(
                            egui::TextEdit::singleline(&mut self.text_content_input)
                                .desired_width(160.0)
                                .hint_text("type text, Enter to place"),
                        );
                        if first_show {
                            content_resp.request_focus();
                        }
                        ui.label("H:");
                        let height_resp = ui.add(
                            egui::TextEdit::singleline(&mut self.text_height_input)
                                .desired_width(48.0),
                        );
                        ui.separator();
                        // いずれかの欄で Enter を押したら確定（egui では Enter で欄がフォーカスを失う）。
                        let submit = (content_resp.lost_focus() || height_resp.lost_focus())
                            && ui.input(|i| i.key_pressed(Key::Enter));
                        if submit && !self.commit_text(anchor, now) {
                            // 確定失敗（空文字列・不正な高さ等）は入力を残し、文字列欄へ再フォーカス。
                            content_resp.request_focus();
                        }
                    }
                    // 表示状態を更新し、非表示に転じたフレームでは入力中の文字列を捨てる
                    // （Esc キャンセル・ツール切替・確定後など。[`McadApp::set_text_field_shown`]）。
                    self.set_text_field_shown(text_anchor.is_some());
                    // ステータスメッセージ（動的な成否通知）はキーバインド凡例を2行目へ分離した
                    // ことで、このフレーム内で幅を奪い合う相手がなくなり常に見える。
                    if let Some(msg) = &self.status {
                        ui.colored_label(STATUS_MESSAGE_COLOR, &msg.text);
                        ui.separator();
                    }
                });
                // キーバインド凡例: 1本の長い文字列だと標準的なウィンドウ幅で右端が
                // 画面外に切れるため、項目ごとのラベルを `horizontal_wrapped` で並べて
                // 幅が足りなければ自動的に複数行へ折り返す。
                ui.horizontal_wrapped(|ui| {
                    for binding in KEYBIND_LEGEND {
                        ui.label(*binding);
                    }
                });
            });
        });
        if let Some(path) = clicked_recent {
            self.request_open_recent(path, now);
        }

        egui::Panel::right("layer_panel").show(ui, |ui| {
            layer_panel(ui, &mut self.document, &mut self.status, now);
            entity_style_panel(
                ui,
                &mut self.document,
                self.select_tool.selection(),
                &mut self.status,
                now,
            );
            dim_panel(
                ui,
                &mut self.document,
                self.select_tool.selection(),
                &mut self.dim_edit_target,
                &mut self.dim_tol_editing,
                &mut self.dim_tol_symmetric_input,
                &mut self.dim_tol_upper_input,
                &mut self.dim_tol_lower_input,
                &mut self.dim_tol_fit_input,
                &mut self.dim_tol_input_error,
                &mut self.dim_decimals_editing,
                &mut self.dim_decimals_input,
                &mut self.dim_decimals_input_error,
                &mut self.dim_value_override_input,
                &mut self.status,
                now,
            );
            let (sheet_changed, plot_color_mode_changed) = sheet_panel(
                ui,
                &mut self.document,
                &mut self.sheet_dialog,
                &mut self.scale_custom_selected,
                &mut self.scale_custom_input,
                &mut self.scale_input_error,
                &mut self.config.plot_color_mode,
                &mut self.status,
                now,
            );
            dim_style_panel(ui, &mut self.dim_style_dialog, &self.document);
            if sheet_changed
                && self
                    .config
                    .remember_sheet_defaults(&self.document.sheet().clone())
            {
                self.persist_config(now);
            }
            if plot_color_mode_changed {
                self.persist_config(now);
            }
        });

        egui::CentralPanel::default().show(ui, |ui| {
            let (rect, response) =
                ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());

            // M4タスク13: ファイル読込直後のズームフィットは、読込時点ではまだこの
            // スクリーン矩形（rect）が確定していないため実行できず、ここまで遅延させる
            // 必要がある（`request_zoom_fit` の doc 参照）。
            if self.pending_zoom_fit {
                if let Some(aabb) = fit_target_aabb(&self.document) {
                    self.viewport.fit_to_aabb(aabb, rect);
                }
                self.pending_zoom_fit = false;
            }

            handle_pan_input(ui, &response, &mut self.viewport);
            handle_zoom_input(ui, &response, rect, &mut self.viewport);
            handle_modifier_view_input(
                ui,
                &response,
                rect,
                &mut self.viewport,
                &mut self.alt_zoom_anchor,
                &mut self.alt_view_gesture,
            );
            // モーダル（未保存確認 / 表題欄編集ダイアログ）表示中は、キャンバスへの
            // クリック/ドラッグ/Delete/Enter/Esc などを一切ツール・選択処理へ渡さない。
            // 素通りさせると、モーダルの裏でエンティティが削除・作図確定されてしまう
            // （Delete/Enter は egui::Modal が消費しないため）。パン/ズームは見るだけの
            // 操作なので許容する。
            // 距離入力欄の解析値（正の有限値のみ Some、それ以外は通過点方式へ
            // フォールバック）。入力処理とゴースト描画で同じ値を使う。
            let offset_distance = parse_offset_distance(&self.offset_distance_input);
            if !self.modal_open() {
                if self.tool_kind == ToolKind::Select {
                    let dim_scale_k = self.document.sheet().scale.world_mm_per_paper_mm();
                    handle_select_input(
                        ui,
                        &response,
                        rect,
                        &self.viewport,
                        &mut self.document,
                        &mut self.select_tool,
                        self.config.snap_enabled,
                        &mut self.snap_marker,
                        &mut self.status,
                        now,
                        offset_distance,
                        text_focused,
                        self.config.paper_display_enabled,
                        dim_scale_k,
                    );
                } else {
                    handle_tool_input(
                        ui,
                        &response,
                        rect,
                        &self.viewport,
                        &mut self.document,
                        &mut self.tool_kind,
                        &mut self.tool,
                        &mut self.select_tool,
                        self.config.snap_enabled,
                        &mut self.snap_marker,
                        self.config.ortho_enabled,
                        &mut self.status,
                        now,
                        parse_fillet_radius(&self.fillet_radius_input),
                    );
                }
            }

            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 0.0, Color32::from_gray(30));

            // 紙 1mm あたりのワールド mm（判断4）。線幅（`draw_entities`）・寸法注記
            // （`draw_selection`・`tool.draw_preview`）・Text（`draw_text`）が共通で使う
            // （タスク37）。
            let k = self.document.sheet().scale.world_mm_per_paper_mm();

            draw_grid(&painter, rect, &self.viewport);
            // 図面枠は「グリッドと同格」の派生描画（判断3）。用紙縁は印刷対象ではない
            // 画面専用ヒントなので `FrameLayout` を経由せず、枠と同じ表示条件で
            // まとめて出す（分岐を1つに保つ）。
            if self.document.sheet().frame_visible {
                draw_paper_edge(&painter, rect, &self.viewport, self.document.sheet());
                draw_frame(
                    &painter,
                    rect,
                    &self.viewport,
                    self.document.sheet(),
                    self.config.paper_display_enabled,
                    k,
                );
            }
            draw_entities(
                &painter,
                rect,
                &self.document,
                &self.viewport,
                self.config.paper_display_enabled,
            );
            draw_selection(
                &painter,
                rect,
                &self.document,
                &self.viewport,
                &self.select_tool,
                offset_distance,
                self.config.paper_display_enabled,
                k,
            );
            if let Some(tool) = &self.tool {
                tool.draw_preview(
                    &painter,
                    rect,
                    &self.viewport,
                    dim_render(
                        self.document.dim_style(),
                        self.config.paper_display_enabled,
                        k,
                        self.viewport.zoom,
                    ),
                );
            }
            // Text ツールでアンカー確定後は、入力中の文字列を実サイズ・実位置でプレビューする
            // （文字列・高さを持つ app 層でしか描けないため、tool.draw_preview とは別にここで）。
            // Text はトグル非依存で常に `height * k`（判断(c)）。
            if self.tool_kind == ToolKind::Text
                && let Some(anchor) = self.tool.as_ref().and_then(|t| t.pending_text_anchor())
                && let Some(height) = parse_text_height(&self.text_height_input)
                && !self.text_content_input.is_empty()
            {
                let preview = TextGeom {
                    anchor,
                    content: self.text_content_input.clone(),
                    height,
                    angle: 0.0,
                };
                draw_text(
                    &painter,
                    rect,
                    &self.viewport,
                    &preview,
                    preview.height * k,
                    TEXT_PREVIEW_COLOR,
                );
            }
            if let Some(marker) = &self.snap_marker {
                draw_snap_marker(&painter, rect, &self.viewport, marker);
            }
        });

        // 未保存確認モーダル（OS の閉じるボタン / Ctrl+N / Ctrl+O 共通、非ブロッキング）。
        // egui::Modal は最前面レイヤに背景付きで描かれるので、パネル群の後に描いてよい。
        // 文言は未日本語化領域なので英語のまま（日本語化は領域単位で行う規約）。
        // 3経路とも同じモーダル外観を使い、「破棄」ボタンが押されたときの分岐だけ
        // `confirm_state` で切り替える（[`ConfirmState::prompt`] の doc 参照）。
        if let Some((message, discard_label)) = self.confirm_state.prompt() {
            let modal =
                egui::Modal::new(egui::Id::new("confirm_unsaved_modal")).show(ui.ctx(), |ui| {
                    ui.set_width(280.0);
                    ui.heading("Unsaved changes");
                    ui.label(message);
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(discard_label).clicked() {
                            match self.confirm_state {
                                ConfirmState::ConfirmingClose => {
                                    // 破棄確定。次フレームの close 要求は Closing 分岐で
                                    // OS へ通す。
                                    self.confirm_state = ConfirmState::Closing;
                                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                                }
                                ConfirmState::ConfirmingNew => {
                                    self.confirm_state = ConfirmState::Idle;
                                    self.new_document(now);
                                }
                                ConfirmState::ConfirmingOpen => {
                                    self.confirm_state = ConfirmState::Idle;
                                    self.open_document(now);
                                }
                                ConfirmState::ConfirmingOpenDxf => {
                                    self.confirm_state = ConfirmState::Idle;
                                    self.open_dxf(now);
                                }
                                ConfirmState::ConfirmingOpenRecent => {
                                    self.confirm_state = ConfirmState::Idle;
                                    if let Some(path) = self.pending_recent_path.take() {
                                        self.open_document_at(&path, now);
                                    }
                                }
                                ConfirmState::Idle | ConfirmState::Closing => {}
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            self.confirm_state = ConfirmState::Idle;
                            self.pending_recent_path = None;
                        }
                    });
                });
            // モーダル外クリック / Esc はキャンセル扱い（実行しない）。
            if modal.should_close() {
                self.confirm_state = ConfirmState::Idle;
                self.pending_recent_path = None;
            }
        }

        // 表題欄編集ダイアログ（M8タスク38）。日本語領域（右パネル）から開くダイアログ
        // なので本文も日本語（未保存確認モーダルとは別領域として扱う）。
        if self.sheet_dialog.is_some() {
            let mut ok_clicked = false;
            let mut cancel_clicked = false;
            let modal =
                egui::Modal::new(egui::Id::new("title_block_dialog")).show(ui.ctx(), |ui| {
                    let dialog = self
                        .sheet_dialog
                        .as_mut()
                        .expect("guarded by is_some() above");
                    ui.set_width(320.0);
                    ui.heading("表題欄を編集");
                    egui::Grid::new("title_block_dialog_grid")
                        .num_columns(2)
                        .spacing([8.0, 4.0])
                        .show(ui, |ui| {
                            ui.label("図面番号:");
                            ui.text_edit_singleline(&mut dialog.drawing_number);
                            ui.end_row();

                            ui.label("図面名称:");
                            ui.text_edit_singleline(&mut dialog.drawing_title);
                            ui.end_row();

                            ui.label("投影法:");
                            egui::ComboBox::from_id_salt("title_block_dialog_projection")
                                .selected_text(match dialog.projection {
                                    ProjectionMethod::ThirdAngle => "第三角法",
                                    ProjectionMethod::FirstAngle => "第一角法",
                                })
                                .show_ui(ui, |ui| {
                                    for (projection, label) in [
                                        (ProjectionMethod::ThirdAngle, "第三角法"),
                                        (ProjectionMethod::FirstAngle, "第一角法"),
                                    ] {
                                        if ui
                                            .selectable_label(
                                                dialog.projection == projection,
                                                label,
                                            )
                                            .clicked()
                                        {
                                            dialog.projection = projection;
                                        }
                                    }
                                });
                            ui.end_row();

                            ui.label("作成者:");
                            ui.text_edit_singleline(&mut dialog.author);
                            ui.end_row();

                            ui.label("作成日:");
                            ui.text_edit_singleline(&mut dialog.date);
                            ui.end_row();

                            ui.label("改訂記号:");
                            ui.text_edit_singleline(&mut dialog.revision);
                            ui.end_row();
                        });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("OK").clicked() {
                            ok_clicked = true;
                        }
                        if ui.button("キャンセル").clicked() {
                            cancel_clicked = true;
                        }
                    });
                });
            if ok_clicked {
                // OK でダイアログセッション全体を Command::SetSheet 1回（undo 1単位）で
                // 適用する。作業コピーは fields 全体を置き換えるだけなので、失敗するのは
                // ここでは事実上起こらない（SetSheet の検証対象は表題欄テンプレート寸法の
                // みで、fields の文字列自体に不正値は無い）が、念のためステータス表示する。
                let new_sheet = self
                    .sheet_dialog
                    .as_ref()
                    .expect("ok_clicked implies Some")
                    .to_sheet(self.document.sheet());
                if let Err(err) = self.document.apply(Command::SetSheet(new_sheet)) {
                    set_status(
                        &mut self.status,
                        now,
                        format!("表題欄の変更に失敗しました: {err}"),
                    );
                }
                self.sheet_dialog = None;
            } else if cancel_clicked || modal.should_close() {
                self.sheet_dialog = None;
            }
        }

        // 寸法スタイルダイアログ(M9タスク50-3)。表題欄編集ダイアログと同じ流儀
        // (右パネルの「寸法スタイルを編集…」ボタンから開き、OK で `Command::SetDimStyle`
        // 1回、キャンセル/モーダル外クリックで破棄)。
        if self.dim_style_dialog.is_some() {
            let mut ok_clicked = false;
            let mut cancel_clicked = false;
            let mut reset_clicked = false;
            let modal = egui::Modal::new(egui::Id::new("dim_style_dialog")).show(ui.ctx(), |ui| {
                let dialog = self
                    .dim_style_dialog
                    .as_mut()
                    .expect("guarded by is_some() above");
                ui.set_width(320.0);
                ui.heading("寸法スタイルを編集");
                egui::Grid::new("dim_style_dialog_grid")
                    .num_columns(2)
                    .spacing([8.0, 4.0])
                    .show(ui, |ui| {
                        ui.label("文字高さ[mm]:");
                        ui.text_edit_singleline(&mut dialog.text_height_mm);
                        ui.end_row();

                        ui.label("矢の長さ[mm]:");
                        ui.text_edit_singleline(&mut dialog.arrow_len_mm);
                        ui.end_row();

                        ui.label("小数桁数(0〜4):");
                        ui.text_edit_singleline(&mut dialog.decimals);
                        ui.end_row();

                        ui.label("末尾ゼロを省く:");
                        ui.checkbox(&mut dialog.trim_trailing_zeros, "");
                        ui.end_row();

                        ui.label("補助線のすきま[mm]:");
                        ui.text_edit_singleline(&mut dialog.ext_gap_mm);
                        ui.end_row();

                        ui.label("補助線の突き出し[mm]:");
                        ui.text_edit_singleline(&mut dialog.ext_overshoot_mm);
                        ui.end_row();

                        ui.label("文字とのすきま[mm]:");
                        ui.text_edit_singleline(&mut dialog.text_gap_mm);
                        ui.end_row();

                        ui.label("公差文字の縮小率:");
                        ui.text_edit_singleline(&mut dialog.tolerance_scale);
                        ui.end_row();
                    });
                if let Some(err) = &dialog.error {
                    ui.colored_label(STATUS_MESSAGE_COLOR, err.as_str());
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() {
                        ok_clicked = true;
                    }
                    if ui.button("キャンセル").clicked() {
                        cancel_clicked = true;
                    }
                    if ui.button("既定に戻す").clicked() {
                        reset_clicked = true;
                    }
                });
            });
            if reset_clicked {
                let dialog = self
                    .dim_style_dialog
                    .as_mut()
                    .expect("reset_clicked implies Some");
                *dialog = DimStyleDialogState::from_style(&DimStyle::default());
            } else if ok_clicked {
                let dialog = self
                    .dim_style_dialog
                    .as_mut()
                    .expect("ok_clicked implies Some");
                match dialog.to_dim_style() {
                    Ok(new_style) => match self.document.apply(Command::SetDimStyle(new_style)) {
                        Ok(_) => self.dim_style_dialog = None,
                        Err(err) => {
                            dialog.error = Some(format!("寸法スタイルの変更に失敗しました: {err}"));
                        }
                    },
                    Err(err) => {
                        dialog.error = Some(err);
                    }
                }
            } else if cancel_clicked || modal.should_close() {
                self.dim_style_dialog = None;
            }
        }
    }
}

/// キーボードショートカットでアクティブツールを切り替える（DESIGN.md 3.4 のツール群）。
///
/// `S`=Select, `1`=Point, `L`=Line, `C`=Circle, `A`=Arc, `P`=Polyline, `T`=Text,
/// `D`/`Shift+D`=Linear/Radial Dim, `G`=Diameter Dim, `X`=Trim, `E`=Extend, `F`=Fillet。
/// ツール切替は途中経過を破棄する（新しいツールインスタンスに置き換わるため）。
/// 作図ツールへ切り替えるときは、描画中に古い選択ハイライトが残らないよう選択をクリアする。
fn handle_tool_shortcut_keys(
    ui: &egui::Ui,
    tool_kind: &mut ToolKind,
    tool: &mut Option<Box<dyn Tool>>,
    select_tool: &mut SelectTool,
) {
    // テキスト入力欄がない前提なので、修飾キーなしのキー入力はすべてショートカット
    // として扱ってよい。
    let mut requested: Option<ToolKind> = None;
    ui.input(|i| {
        // Ctrl/Cmd 併用はツール切替として扱わない（Ctrl+Z/Ctrl+Y の undo/redo や
        // 将来の Ctrl+C/Ctrl+S 系ショートカットと衝突させない）。
        if i.modifiers.command {
            return;
        }
        // D は修飾なしで長さ寸法、Shift 併用で半径寸法。Ctrl+D（複製）は上の
        // command 早期 return で既に除かれているので、ここでは Shift の有無だけを見る。
        if i.key_pressed(Key::D) {
            requested = Some(if i.modifiers.shift {
                ToolKind::DimRadial
            } else {
                ToolKind::DimLinear
            });
        } else if i.key_pressed(Key::S) {
            requested = Some(ToolKind::Select);
        } else if i.key_pressed(Key::Num1) {
            requested = Some(ToolKind::Point);
        } else if i.key_pressed(Key::L) {
            requested = Some(ToolKind::Line);
        } else if i.key_pressed(Key::C) {
            requested = Some(ToolKind::Circle);
        } else if i.key_pressed(Key::A) {
            requested = Some(ToolKind::Arc);
        } else if i.key_pressed(Key::P) {
            requested = Some(ToolKind::Polyline);
        } else if i.key_pressed(Key::T) {
            requested = Some(ToolKind::Text);
        } else if i.key_pressed(Key::G) {
            // 未使用キー（G/H/I/J/K/N/Q/U/V/W のうち G を採用。2026-09-04、
            // main.rs/tool.rs をともに `grep -n "Key::G"` して未使用を確認済み）。
            requested = Some(ToolKind::DimDiameter);
        } else if i.key_pressed(Key::X) {
            requested = Some(ToolKind::Trim);
        } else if i.key_pressed(Key::E) {
            // 単押しのみ。Ctrl+E（DXF エクスポート）は上の command 早期 return で
            // 既に除かれている。
            requested = Some(ToolKind::Extend);
        } else if i.key_pressed(Key::F) {
            requested = Some(ToolKind::Fillet);
        } else if i.key_pressed(Key::B) {
            requested = Some(ToolKind::Split);
        }
    });
    if let Some(kind) = requested {
        *tool_kind = kind;
        *tool = kind.spawn();
        // ツール切替は進行中の配置モード（Ctrl+D 複製等）・オフセットモードを解除する。
        // Select のままの再選択（S）でも確定させずに畳む（DESIGN.md 設計判断2・5）。
        select_tool.cancel_placement();
        select_tool.cancel_offset();
        select_tool.cancel_text_drag();
        // 作図ツールへ移るときは選択を解除する（Select のままなら選択は保持）。
        if kind != ToolKind::Select {
            select_tool.clear_selection();
        }
    }
}

/// 新規エンティティを追加する先のレイヤーを、作図ツールの種類から解決する
/// （M9 タスク53: 標準レイヤー構成と自動割当）。
///
/// 寸法ツール（`DimLinear`/`DimRadial`/`DimDiameter`）は [`DIM_LAYER_NAME`]
/// （`寸法線`）、文字ツール（[`ToolKind::Text`]）は [`TEXT_LAYER_NAME`]（`文字`）が
/// 文書に存在すればそこへ、無ければ [`Document::current_layer`] へフォールバックする。
/// それ以外のツールは常にカレントレイヤーを使う。**該当名のレイヤーが無い文書へ
/// 新しくレイヤーを作ることはしない**（読込図面でレイヤーが勝手に増えないための
/// 設計。[`fresh_document`] の doc 参照）。ロック中のレイヤーへ自動割当した結果
/// `AddEntity` が失敗した場合は、既存の `LayerLocked` エラー経路でステータスバーへ
/// 表示される（ここではフォールバックしない — ユーザーがロックした意図を尊重する）。
fn resolve_tool_layer(document: &Document, tool_kind: ToolKind) -> LayerId {
    match tool_kind {
        ToolKind::DimLinear | ToolKind::DimRadial | ToolKind::DimDiameter => {
            layer_named(document, DIM_LAYER_NAME).unwrap_or_else(|| document.current_layer())
        }
        ToolKind::Text => {
            layer_named(document, TEXT_LAYER_NAME).unwrap_or_else(|| document.current_layer())
        }
        _ => document.current_layer(),
    }
}

/// アクティブなツールへ、キャンバス上の入力（クリック/Enter/Esc/マウス移動）を渡す。
///
/// パン操作（中ボタンドラッグ、または Space+左ドラッグ）と作図クリックが衝突しない
/// よう、Space 押下中および実際にパン用ドラッグが進行中は左クリックをツールへ
/// 渡さない。ツールが `Commit`/`Cancel` を返した場合、ツールをリセット（`Commit` は
/// 同じ種類の新しいインスタンスへ、`Cancel` は非アクティブ = `Select` へ）する。
///
/// # スナップの統合
///
/// スクリーン→ワールド変換した素のカーソル位置に対し、`snap_enabled` なら
/// [`snap::snap`] を掛けて候補点へ吸着させた座標を `InputEvent` に載せる。移動時は
/// スナップ先を `snap_marker` に記録し、描画層がマーカーを表示する。スナップは作図
/// ツール（Point/Line/…）にのみ効かせる。選択・編集ツールはドラッグ矩形・移動の
/// 変位を扱い、頂点入力とは性質が異なるため MVP ではスナップ対象外とする
/// （`handle_select_input` は素のワールド座標を使う）。
#[allow(clippy::too_many_arguments)]
fn handle_tool_input(
    ui: &egui::Ui,
    response: &egui::Response,
    rect: Rect,
    viewport: &Viewport,
    document: &mut Document,
    tool_kind: &mut ToolKind,
    tool: &mut Option<Box<dyn Tool>>,
    select_tool: &mut SelectTool,
    snap_enabled: bool,
    snap_marker: &mut Option<snap::SnapResult>,
    ortho_enabled: bool,
    status: &mut Option<StatusMessage>,
    now: f64,
    fillet_radius: Option<f64>,
) {
    let Some(active) = tool.as_mut() else {
        return;
    };

    // 数値入力欄（フィレットの半径）の現在値をツールへ流し込む。フィレットは 2 クリック目の
    // ヒットテストと同時に確定するため、確定判断を行うツール側が値を持っている必要がある
    // （[`Tool::set_radius_input`] の doc 参照）。他ツールでは既定実装の no-op。
    active.set_radius_input(fillet_radius);

    // ピック許容量（ワールド）。半径寸法ツールの円/円弧ヒットテスト（画面上のピック =
    // ズーム依存）が選択のピック許容量と揃うようにする。寸法の退化判定はこれとは別に
    // ツール側のスケール非依存な幾何許容値で行う（tool.rs の DIM_DEGENERATE_EPSILON）。
    let pick_tol = PICK_TOLERANCE_PX / viewport.zoom;

    let ctx = ToolCtx {
        layer: resolve_tool_layer(document, *tool_kind),
        style: Style::inherited(),
    };

    // スナップ用パラメータ（探索半径・グリッド間隔）。半径はピック許容量と同じ考え方で
    // px→ワールド換算する。グリッド間隔は描画グリッドと同じ副グリッド刻み。交点候補の
    // 事前絞り込みはカーソル近傍 AABB を snap 側が内部で構成する（`snap.rs` 参照）。
    let radius = SNAP_RADIUS_PX / viewport.zoom;
    let grid_step = viewport::nice_grid_step(viewport.zoom, GRID_TARGET_PX);

    // マウス移動は毎フレーム流し、プレビュー追従に使ってもらう。スナップ先はマーカー
    // 描画のために記録する（ホバーしていなければマーカーを消す）。
    if let Some(pos) = response.hover_pos() {
        let raw = viewport.screen_to_world(rect, pos);
        if active.wants_shape_pick() {
            // wants_shape_pick 系4ツール（トリム・延長・フィレット・分割）は、クリックが
            // raw のままヒットテストされる（下の Click 節のコメント参照）ため、図面全体
            // スナップのマーカーを出すと「マーカーはクリックが使わない点」という不整合が
            // 生じる（DESIGN.md 7章「分割ツールのスナップ対応」論点(3)）。分割だけは
            // クリック点そのものが分割位置であり実際にスナップを掛ける
            // （`snaps_shape_pick()`）ので、対象上に限定したマーカー（実際に割られる点）を
            // 表示する。トリム・延長・フィレットはマーカー自体を出さない（クリック挙動は
            // 無変更）。
            *snap_marker = if snap_enabled && active.snaps_shape_pick() {
                tool::pick_shape_entity(document, raw, pick_tol).and_then(|hit| {
                    snap::snap_split_position(document, hit.id, &hit.shape, raw, radius)
                })
            } else {
                None
            };
            // Move は Move(_) を無視する仕様（tool.rs 各ツールの on_input 参照）なので
            // raw をそのまま渡してよい。
            let _ = active.on_input(&ctx, InputEvent::Move(raw));
        } else {
            let extra_points = active.snap_points();
            let (_, marker) = apply_snap(
                document,
                snap_enabled,
                raw,
                radius,
                grid_step,
                &extra_points,
            );
            *snap_marker = marker;
            let world = resolve_click_point(marker, raw, ortho_enabled, active.ortho_origin());
            let _ = active.on_input(&ctx, InputEvent::Move(world));
        }
    } else {
        *snap_marker = None;
    }

    // Space 押下中の左ドラッグはパン操作に使われているため、作図クリックとしては
    // 扱わない（`handle_pan_input` と役割が競合しないようにする）。
    let space_down = ui.input(|i| i.key_down(Key::Space));
    let mut result = ToolResult::Continue;
    if !space_down
        && response.clicked_by(egui::PointerButton::Primary)
        && let Some(pos) = response.interact_pointer_pos()
    {
        let raw = viewport.screen_to_world(rect, pos);
        if active.wants_circle_pick() {
            // 半径寸法ツールの1クリック目: 円/円弧のヒットテストは Document を要するため
            // app 層で行う（Tool は Document 非依存の設計。tool.rs 冒頭 doc 参照）。既存
            // エンティティの実位置で当てるのでスナップは掛けず raw を使う。外したら状態は
            // 据え置きで ASCII メッセージを出し、再クリックできるようにする。
            match tool::pick_circle_or_arc(document, raw, pick_tol) {
                Some(hit) => active.on_circle_pick(hit),
                None => set_status(status, now, "Radial dim: click a circle or arc".to_string()),
            }
        } else if active.wants_shape_pick() {
            // 汎用エンティティピック（M7 タスク30）: wants_circle_pick と同じ配線。トリム・
            // 延長（タスク31）が境界・対象の両クリックで使う。対象決定（どれを当てるか）は
            // raw のまま行う: 既存エンティティの実位置で当てる必要があるうえ、トリムの
            // `click` は「捨てる側」を示す意味を持つため、交点へ吸着すると本来通るはずの
            // 操作が交点直上クリックとして拒否されてしまう（トリム・延長・フィレットは
            // ここで終わり、以降のスナップ差し替えは行わない）。
            //
            // 分割（`snaps_shape_pick()`）だけは対象決定の後、分割位置そのものを対象上へ
            // スナップさせる（DESIGN.md 7章「分割ツールのスナップ対応」論点(2)）。ヒットすれば
            // `click` をスナップ先へ差し替え、外れれば raw のまま `on_shape_pick` へ渡す。
            match tool::pick_shape_entity(document, raw, pick_tol) {
                Some(mut hit) => {
                    if snap_enabled
                        && active.snaps_shape_pick()
                        && let Some(snapped) =
                            snap::snap_split_position(document, hit.id, &hit.shape, raw, radius)
                    {
                        hit.click = snapped.point;
                    }
                    result = active.on_shape_pick(hit);
                }
                None => set_status(
                    status,
                    now,
                    "Pick: click a line, arc, circle, or polyline".to_string(),
                ),
            }
        } else {
            let extra_points = active.snap_points();
            let (_, marker) = apply_snap(
                document,
                snap_enabled,
                raw,
                radius,
                grid_step,
                &extra_points,
            );
            let world = resolve_click_point(marker, raw, ortho_enabled, active.ortho_origin());
            result = active.on_input(&ctx, InputEvent::Click(world));
        }
    }

    if matches!(result, ToolResult::Continue) && ui.input(|i| i.key_pressed(Key::Enter)) {
        result = active.on_input(&ctx, InputEvent::Confirm);
    }
    if matches!(result, ToolResult::Continue) && ui.input(|i| i.key_pressed(Key::Escape)) {
        result = active.on_input(&ctx, InputEvent::Cancel);
    }

    match result {
        ToolResult::Commit(cmd) => {
            // ドキュメント側のレイヤーロックなどで失敗することがある（例: カレント
            // レイヤーがロック中の AddEntity）。dirty 状態は世代カウンタが自動で
            // 表すため、成功時に明示的なフラグ操作は不要。
            //
            // 成功時はツールを respawn しない: 各作図ツール（Point/Line/Circle/
            // Arc/Polyline）は確定のたびに自分で次の入力へ備えた状態にする設計
            // であり（例: Line は連続線分モードとして終点を次の始点へ引き継ぐ）、
            // ここで無条件に spawn() し直すとその引き継ぎ状態を消してしまう。
            //
            // 失敗時のみ spawn() でリセットする: 確定できなかった中途状態を
            // 次のクリックへ持ち越さないため。
            match document.apply(cmd) {
                Ok(new_ids) => {
                    // トポロジが変わる確定（2 断片トリム）だけ、新規エンティティを
                    // 選択集合へ載せ替える（DESIGN.md M7 設計判断3a）。それ以外の
                    // ツールは `None` を返すので選択は変わらない。
                    if let Some(ids) = active.take_commit_selection(&new_ids) {
                        select_tool.set_selection(ids);
                    }
                }
                Err(err) => {
                    set_status(status, now, format!("Commit failed: {err}"));
                    if tool_kind.keeps_state_on_commit_failure() {
                        // M7の4ツール（トリム/延長/フィレット/分割）は失敗時も状態を
                        // 据え置く（DESIGN.md M7設計判断6）。spawn() で作り直さない
                        // 代わりに、選択集合の載せ替えフラグが次の確定へ誤って持ち越され
                        // ないようクリアする（Tool::on_commit_failed の doc 参照）。
                        active.on_commit_failed();
                    } else {
                        *tool = tool_kind.spawn();
                    }
                }
            }
        }
        ToolResult::Rejected(reason) => {
            // 退化クリック（寸法の p1≈p2・引出方向＝中心）。ツールは状態を据え置いて
            // いるので respawn せず、理由だけステータスへ出して無反応に見えないようにする。
            set_status(status, now, reason.to_string());
        }
        ToolResult::Cancel => {
            *tool_kind = ToolKind::Select;
            *tool = None;
        }
        ToolResult::Continue => {}
    }
}

/// 「最前面へ」で割り当てる重ね順（[`Layer::order`]）。
///
/// `current` は対象レイヤーの現在の order、`other_orders` は**それ以外の**生存レイヤーの
/// order 列。「他のレイヤーの最大 + 1」を採るが、既に最前面（あるいはレイヤーが1枚だけ）
/// なら `current` をそのまま返すため、[`Command::SetLayerProps`] の既存 no-op 判定
/// （`before == props` なら履歴を汚さない）にそのまま乗る。呼び出し側は戻り値が
/// `current` と等しいかどうかでボタンの有効/無効も決められる。
///
/// `saturating_add` なのは `i32::MAX` のレイヤーがあっても算術オーバーフローで
/// パニックしないため（飽和時は同順位に並ぶだけで、描画は破綻しない）。
fn bring_to_front_order(current: i32, other_orders: impl IntoIterator<Item = i32>) -> i32 {
    other_orders
        .into_iter()
        .max()
        .map_or(current, |max| current.max(max.saturating_add(1)))
}

/// 「最背面へ」で割り当てる重ね順。[`bring_to_front_order`] の対称版
/// （「他のレイヤーの最小 - 1」、既に最背面なら `current` のまま）。
fn send_to_back_order(current: i32, other_orders: impl IntoIterator<Item = i32>) -> i32 {
    other_orders
        .into_iter()
        .min()
        .map_or(current, |min| current.min(min.saturating_sub(1)))
}

/// [`front_order_plan`] / [`back_order_plan`] が返す「その `LayerId` の order を
/// この値へ変える」更新1件。
type OrderUpdate = (LayerId, i32);

/// 「最前面へ」を安全に適用するための更新計画を返す（Codex adversarial review
/// の medium 指摘対応）。
///
/// `layers` は生存レイヤー全件の `(LayerId, order)`（対象 `target` を含む）。
///
/// 通常（他レイヤーの最大 order が `i32::MAX` でない）は [`bring_to_front_order`]
/// と同じ「他レイヤーの最大 + 1」を対象1件だけに適用する計画を返す。すでに
/// 対象が唯一の最前面なら空の計画（no-op）を返す。
///
/// 他レイヤーの最大 order が `i32::MAX` のときは単純な +1 が使えない
/// （`saturating_add` が現在値と同じ `i32::MAX` を返し、最前面にならない）ため、
/// 全レイヤーの order を `0..n` へ再正規化しつつ対象を末尾（最前面）に置く
/// 計画を返す（相対順序は保つ）。この経路は他レイヤーが `i32::MAX` に達している
/// 場合にしか通らないため、通常ケースの履歴を汚さない。
fn front_order_plan(target: LayerId, layers: &[(LayerId, i32)]) -> Vec<OrderUpdate> {
    let Some(current) = layers
        .iter()
        .find(|(id, _)| *id == target)
        .map(|(_, order)| *order)
    else {
        return Vec::new();
    };
    let other_max = layers
        .iter()
        .filter(|(id, _)| *id != target)
        .map(|(_, order)| *order)
        .max();
    match other_max {
        None => Vec::new(),
        Some(max) if current > max => Vec::new(),
        Some(i32::MAX) => renormalize_front(target, layers),
        Some(max) => vec![(target, bring_to_front_order(current, [max]))],
    }
}

/// 「最背面へ」の対称版（[`front_order_plan`] 参照）。
fn back_order_plan(target: LayerId, layers: &[(LayerId, i32)]) -> Vec<OrderUpdate> {
    let Some(current) = layers
        .iter()
        .find(|(id, _)| *id == target)
        .map(|(_, order)| *order)
    else {
        return Vec::new();
    };
    let other_min = layers
        .iter()
        .filter(|(id, _)| *id != target)
        .map(|(_, order)| *order)
        .min();
    match other_min {
        None => Vec::new(),
        Some(min) if current < min => Vec::new(),
        Some(i32::MIN) => renormalize_back(target, layers),
        Some(min) => vec![(target, send_to_back_order(current, [min]))],
    }
}

/// 全レイヤーの order を `0..n` へ再正規化し、`target` をその末尾（＝最前面）に
/// 置く。`target` 以外は元の order の大小関係（相対順序）を保ったまま詰め直す。
///
/// 戻り値には「新しい order が現在の order と異なるレイヤーのみ」を含む
/// （変わらないレイヤーは計画に含めず、無駄な `SetLayerProps` を作らない）。
fn renormalize_front(target: LayerId, layers: &[(LayerId, i32)]) -> Vec<OrderUpdate> {
    let mut others: Vec<(LayerId, i32)> = layers
        .iter()
        .filter(|(id, _)| *id != target)
        .copied()
        .collect();
    others.sort_by_key(|(_, order)| *order);
    let mut plan: Vec<OrderUpdate> = others
        .iter()
        .enumerate()
        .filter_map(|(i, (id, old_order))| {
            let new_order = i as i32;
            (new_order != *old_order).then_some((*id, new_order))
        })
        .collect();
    // 対象は他レイヤー全件より 1 大きい order へ置くので、常に唯一の最前面になる。
    plan.push((target, others.len() as i32));
    plan
}

/// [`renormalize_front`] の対称版。`target` を先頭（＝最背面）に置く。
fn renormalize_back(target: LayerId, layers: &[(LayerId, i32)]) -> Vec<OrderUpdate> {
    let mut others: Vec<(LayerId, i32)> = layers
        .iter()
        .filter(|(id, _)| *id != target)
        .copied()
        .collect();
    others.sort_by_key(|(_, order)| *order);
    let mut plan: Vec<OrderUpdate> = vec![(target, 0)];
    plan.extend(
        others
            .iter()
            .enumerate()
            .filter_map(|(i, (id, old_order))| {
                let new_order = i as i32 + 1;
                (new_order != *old_order).then_some((*id, new_order))
            }),
    );
    plan
}

/// UI で提示する線幅プリセット（紙 mm。ISO 128 系列、規定 7.1 の決定・DESIGN.md
/// M8 設計判断5「UI の線幅入力は ISO 128 系列のプリセット提示」）。
const ISO_LINE_WIDTH_PRESETS_MM: [f32; 9] = [0.13, 0.18, 0.25, 0.35, 0.5, 0.7, 1.0, 1.4, 2.0];

/// [`Linetype`] の日本語表示ラベル（レイヤーパネル・エンティティ上書きパネル共通）。
///
/// `Linetype` は `#[non_exhaustive]` なので、将来の追加は未知の腕として
/// 実線ラベルへフォールバックする（[`dash_pattern_mm`] のフォールバックと対称）。
fn linetype_label(linetype: Linetype) -> &'static str {
    match linetype {
        Linetype::Continuous => "実線",
        Linetype::Dashed => "破線",
        Linetype::DashDot => "一点鎖線",
        Linetype::DashDotDot => "二点鎖線",
        _ => "実線",
    }
}

/// レイヤーパネル・エンティティ上書きパネル共通の線種選択コンボボックス。
///
/// クリックされたら `on_select` へ選ばれた線種を渡す（呼び出し側が
/// `Command` を組み立てて `pending` へ積む）。
fn linetype_combo(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    current: Linetype,
    mut on_select: impl FnMut(Linetype),
) {
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(linetype_label(current))
        .show_ui(ui, |ui| {
            for candidate in [
                Linetype::Continuous,
                Linetype::Dashed,
                Linetype::DashDot,
                Linetype::DashDotDot,
            ] {
                if ui
                    .selectable_label(candidate == current, linetype_label(candidate))
                    .clicked()
                    && candidate != current
                {
                    on_select(candidate);
                }
            }
        });
}

/// レイヤーパネル・エンティティ上書きパネル共通の線幅選択コンボボックス
/// （[`ISO_LINE_WIDTH_PRESETS_MM`] からの選択のみ。任意 mm 値は DXF import 等
/// 別経路のみが生成し、UI からは検証済みプリセットしか選べない）。
fn width_mm_combo(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    current: f32,
    mut on_select: impl FnMut(WidthMm),
) {
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(format!("{current:.2}mm"))
        .show_ui(ui, |ui| {
            for preset in ISO_LINE_WIDTH_PRESETS_MM {
                if ui
                    .selectable_label(
                        (preset - current).abs() < f32::EPSILON,
                        format!("{preset:.2}mm"),
                    )
                    .clicked()
                    && (preset - current).abs() >= f32::EPSILON
                {
                    // プリセットは常に範囲内だが、UI 入力境界での不正線幅拒否という
                    // 規約（判断5）を型で徹底するため `WidthMm::new` の検証を経由する。
                    if let Ok(width) = WidthMm::new(preset) {
                        on_select(width);
                    }
                }
            }
        });
}

/// 右側のレイヤーパネル（DESIGN.md 3.4 の UI レイアウト）。
///
/// 一覧・カレント切替（ラジオ）・色変更・表示/ロック切替・重ね順変更・追加/削除を提供する。
/// すべての変更は [`Command`] として [`Document::apply`] へ載せるため undo/redo の
/// 対象になる。削除の制約（デフォルト/カレント/非空レイヤーは不可）はコア側が
/// 検証し、失敗はステータスバーへ表示する。
///
/// ラベル等のユーザー可視文字列は日本語（M8 で ASCII 限定を撤廃。[`fonts`] が登録する
/// Noto Sans JP へグリフ単位でフォールバックする）。日本語化は領域単位で完結させる規約の
/// ため、このパネル内に英語ラベルを混ぜないこと。なお新規レイヤーの既定名 `Layer {n}` は
/// UI ラベルではなくドキュメントへ保存されるデータなので英語のままにしている。
fn layer_panel(
    ui: &mut egui::Ui,
    document: &mut Document,
    status: &mut Option<StatusMessage>,
    now: f64,
) {
    ui.heading("レイヤー");

    if ui.button("+ レイヤー追加").clicked() {
        let n = document.layer_count();
        let color = LAYER_COLOR_PALETTE[n % LAYER_COLOR_PALETTE.len()];
        let mut layer = Layer::new(format!("Layer {n}"), color);
        // 新規レイヤーは常に最前面へ（既存の最大 order + 1）。空の文書なら 0。
        layer.order = document
            .layers()
            .map(|(_, existing)| existing.order)
            .max()
            .map_or(0, |max| max.saturating_add(1));
        if let Err(err) = document.apply(Command::AddLayer(layer)) {
            set_status(status, now, format!("レイヤーの追加に失敗しました: {err}"));
        }
    }
    ui.separator();

    let current = document.current_layer();
    let default_layer = document.default_layer();
    // パネル描画中の可変借用を避けるため、レイヤー一覧のスナップショットを取り、
    // 操作から生じたコマンドは走査後に一括適用する。
    //
    // 行の並びは重ね順で、**手前のレイヤーが上**（一般的なレイヤーパネルの慣習）。
    // `layers_in_order()` は奥→手前の昇順なので反転する。
    let mut layers: Vec<(LayerId, Layer)> = document
        .layers_in_order()
        .into_iter()
        .map(|(id, layer)| (id, layer.clone()))
        .collect();
    layers.reverse();
    let mut pending: Vec<Command> = Vec::new();

    // 「表示」「ロック」は各行に同じ語を繰り返さず、列見出しとして冒頭行に1度だけ出す。
    // 見出しと列を揃えるには行ごとの幅が一定である必要があるため、`ui.horizontal` では
    // なく [`egui::Grid`] を使う（horizontal はレイヤー名の長さで各行の幅が変わり、
    // 見出しとずれてしまう）。
    egui::Grid::new("layer_panel_grid")
        .num_columns(8)
        .spacing([8.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            ui.label("現在");
            ui.label("色");
            ui.label("表示");
            ui.label("ロック");
            ui.label("名前");
            ui.label("線幅");
            ui.label("線種");
            ui.label("操作");
            ui.end_row();

            for (id, layer) in &layers {
                // カレントレイヤー切替（ラジオ）。新規エンティティの投入先になる。
                if ui
                    .radio(*id == current, "")
                    .on_hover_text("カレントレイヤーに設定")
                    .clicked()
                    && *id != current
                {
                    pending.push(Command::SetCurrentLayer(*id));
                }

                let mut rgb = [layer.color.r, layer.color.g, layer.color.b];
                if ui.color_edit_button_srgb(&mut rgb).changed() {
                    let mut props = layer.clone();
                    props.color = Rgb::new(rgb[0], rgb[1], rgb[2]);
                    pending.push(Command::SetLayerProps { id: *id, props });
                }

                // 意味は列見出しが担うのでラベルは空。単独で見ても分かるようツールチップは残す。
                let mut visible = layer.visible;
                if ui
                    .checkbox(&mut visible, "")
                    .on_hover_text("表示 / 非表示")
                    .changed()
                {
                    let mut props = layer.clone();
                    props.visible = visible;
                    pending.push(Command::SetLayerProps { id: *id, props });
                }

                let mut locked = layer.locked;
                if ui
                    .checkbox(&mut locked, "")
                    .on_hover_text("ロック中は編集できない")
                    .changed()
                {
                    let mut props = layer.clone();
                    props.locked = locked;
                    pending.push(Command::SetLayerProps { id: *id, props });
                }

                ui.label(&layer.name);

                width_mm_combo(ui, ("layer_width", *id), layer.width_mm.mm(), |width| {
                    let mut props = layer.clone();
                    props.width_mm = width;
                    pending.push(Command::SetLayerProps { id: *id, props });
                });

                linetype_combo(ui, ("layer_linetype", *id), layer.linetype, |linetype| {
                    let mut props = layer.clone();
                    props.linetype = linetype;
                    pending.push(Command::SetLayerProps { id: *id, props });
                });

                // 重ね順（最前面へ / 最背面へ）。専用コマンドは作らず、既存の
                // SetLayerProps を並べた Command::Batch として適用する（1クリック =
                // undo 1単位。Batch はサブコマンド1件でも1記録として履歴に積まれるため、
                // 通常ケースでは単発 SetLayerProps と同じ粒度のまま）。
                //
                // 通常は対象レイヤー1件だけを動かす（[`front_order_plan`] /
                // [`back_order_plan`] が「他レイヤーの最大+1 / 最小-1」を計算する）。
                // 他レイヤーが i32::MAX / i32::MIN に達していて単純な +1/-1 が使えない
                // 場合だけ、全レイヤーの order を再正規化する計画が返る（Codex
                // adversarial review の medium 指摘対応）。計画が空なら「既に端にいる」
                // ということなのでボタンを無効化する。
                let all_orders: Vec<(LayerId, i32)> =
                    layers.iter().map(|(lid, l)| (*lid, l.order)).collect();
                let front_plan = front_order_plan(*id, &all_orders);
                let back_plan = back_order_plan(*id, &all_orders);
                let plan_to_batch = |plan: Vec<(LayerId, i32)>| -> Command {
                    Command::Batch(
                        plan.into_iter()
                            .filter_map(|(lid, order)| {
                                let mut props = layers.iter().find(|(l, _)| *l == lid)?.1.clone();
                                props.order = order;
                                Some(Command::SetLayerProps { id: lid, props })
                            })
                            .collect(),
                    )
                };
                // 重ね順と削除はまとめて「操作」列の1セルへ入れる。
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!front_plan.is_empty(), egui::Button::new("前面"))
                        .on_hover_text("最前面へ移動")
                        .clicked()
                    {
                        pending.push(plan_to_batch(front_plan));
                    }
                    if ui
                        .add_enabled(!back_plan.is_empty(), egui::Button::new("背面"))
                        .on_hover_text("最背面へ移動")
                        .clicked()
                    {
                        pending.push(plan_to_batch(back_plan));
                    }

                    // 削除。デフォルトレイヤーにはボタン自体を出さない（コア側でも拒否される）。
                    // カレント・非空レイヤーの削除失敗はコアの検証に任せ、理由を表示する。
                    if *id != default_layer
                        && ui.button("x").on_hover_text("レイヤーを削除").clicked()
                    {
                        pending.push(Command::RemoveLayer(*id));
                    }
                });
                ui.end_row();
            }
        });

    for cmd in pending {
        if let Err(err) = document.apply(cmd) {
            set_status(status, now, format!("レイヤー操作に失敗しました: {err}"));
        }
    }
}

/// 選択中エンティティの線幅・線種を上書き／ByLayer へ戻す最小 UI（M8 タスク36）。
///
/// レイヤーパネルの直下に表示する（選択が空なら何も描かない）。既存の色上書き
/// UI はまだ無いため、線幅・線種のみをタスク36の範囲として提供する。複数選択時は
/// 先頭エンティティの現在値を表示の基準にし、変更は選択集合全体へ同じ値をまとめて
/// 適用する（`Command::Batch` で undo 1単位。空文書判定は `Document::apply` の
/// no-op 集約に任せる）。ロック中レイヤーのエンティティを含む場合、適用時の失敗は
/// バッチ全体をロールバックしてステータスバーへ表示する（[`Command::Batch`] の原子性）。
fn entity_style_panel(
    ui: &mut egui::Ui,
    document: &mut Document,
    selection: &[EntityId],
    status: &mut Option<StatusMessage>,
    now: f64,
) {
    // 選択されていても墓標化・削除済みのIDは無視する（undo/redo の直後等で
    // 選択集合が一時的に古いIDを含みうる。`SelectTool` は自前でこれを掃除する
    // 責務を持たないため、表示側で防御する）。
    let live: Vec<(EntityId, Style, Layer)> = selection
        .iter()
        .filter_map(|&id| {
            let entity = document.entity(id)?;
            let layer = document.layer(entity.layer)?.clone();
            Some((id, entity.style, layer))
        })
        .collect();
    if live.is_empty() {
        return;
    }

    ui.separator();
    ui.heading("選択中のスタイル");

    let (_, first_style, first_layer) = &live[0];
    let effective_width = first_style.effective_width(first_layer.width_mm).mm();
    let effective_linetype = first_style.effective_linetype(first_layer.linetype);
    let width_is_overridden = first_style.width_mm.is_some();
    let linetype_is_overridden = first_style.linetype.is_some();

    let mut pending: Vec<Command> = Vec::new();

    // 選択中の全エンティティの所属レイヤーID（表示用に一括取得）。
    let entity_layer_ids: Vec<LayerId> = live
        .iter()
        .map(|(id, ..)| document.entity(*id).unwrap().layer)
        .collect();
    let first_layer_id = entity_layer_ids[0];
    let layer_selection_uniform = entity_layer_ids
        .iter()
        .all(|&layer| layer == first_layer_id);
    let layers_in_order = document.layers_in_order();
    let current_layer_label = if layer_selection_uniform {
        document
            .layer(first_layer_id)
            .map(|layer| layer.name.clone())
            .unwrap_or_default()
    } else {
        "混在".to_string()
    };

    ui.horizontal(|ui| {
        ui.label("レイヤー:");
        egui::ComboBox::from_id_salt("entity_layer")
            .selected_text(current_layer_label)
            .show_ui(ui, |ui| {
                for (layer_id, layer) in &layers_in_order {
                    let selected = layer_selection_uniform && *layer_id == first_layer_id;
                    if ui.selectable_label(selected, &layer.name).clicked() {
                        for (id, current_layer) in
                            live.iter().map(|(id, ..)| id).zip(&entity_layer_ids)
                        {
                            if current_layer != layer_id {
                                pending.push(Command::SetEntityLayer {
                                    id: *id,
                                    layer: *layer_id,
                                });
                            }
                        }
                    }
                }
            });
    });

    ui.horizontal(|ui| {
        ui.label("線幅:");
        width_mm_combo(ui, "entity_style_width", effective_width, |width| {
            for (id, style, _) in &live {
                let mut new_style = *style;
                new_style.width_mm = Some(width);
                pending.push(Command::SetEntityStyle {
                    id: *id,
                    style: new_style,
                });
            }
        });
        if width_is_overridden
            && ui
                .button("ByLayer")
                .on_hover_text("レイヤー既定の線幅へ戻す")
                .clicked()
        {
            for (id, style, _) in &live {
                let mut new_style = *style;
                new_style.width_mm = None;
                pending.push(Command::SetEntityStyle {
                    id: *id,
                    style: new_style,
                });
            }
        }
    });

    ui.horizontal(|ui| {
        ui.label("線種:");
        linetype_combo(
            ui,
            "entity_style_linetype",
            effective_linetype,
            |linetype| {
                for (id, style, _) in &live {
                    let mut new_style = *style;
                    new_style.linetype = Some(linetype);
                    pending.push(Command::SetEntityStyle {
                        id: *id,
                        style: new_style,
                    });
                }
            },
        );
        if linetype_is_overridden
            && ui
                .button("ByLayer")
                .on_hover_text("レイヤー既定の線種へ戻す")
                .clicked()
        {
            for (id, style, _) in &live {
                let mut new_style = *style;
                new_style.linetype = None;
                pending.push(Command::SetEntityStyle {
                    id: *id,
                    style: new_style,
                });
            }
        }
    });

    // 複数のプロパティ変更が同一フレームで起きることは無い（コンボボックス/ボタンは
    // 排他的にクリックされる）が、将来の拡張に備えて Batch でまとめて適用する。
    if !pending.is_empty()
        && let Err(err) = document.apply(Command::Batch(pending))
    {
        set_status(status, now, format!("スタイルの変更に失敗しました: {err}"));
    }
}

// ---------------------------------------------------------------------
// 右パネル「寸法」セクション（M9タスク50-2）
// ---------------------------------------------------------------------

/// 右パネル「寸法」のコンボの展開メニューに許す最大高さ [px]。egui の既定
/// （`Spacing::combo_height` = 200px）では、パネル下部で開いたときに項目が収まらず
/// スクロールバーが出ることがあった（M9 タスク50 の手動スモークテスト、2026-09-05）。
/// 記号コンボの最大項目数は「なし」+ 5 種で、これを余裕をもって収める値にする。
///
/// **これだけでは足りない**（下記 [`dim_combo_id_salt`] を参照）。
const DIM_COMBO_MAX_HEIGHT: f32 = 400.0;

/// 「寸法」パネルのコンボ（記号・公差種別・矢印配置）が**2回目以降の展開でメニューが
/// 縮んでスクロールバー付きになる**不具合への対応（差し戻し対応、2026-09-05）。
///
/// # 原因（egui 0.35.0 のソースで確認した事実）
///
/// - `ComboBox` のポップアップは内部で必ず `ScrollArea::vertical().max_height(height)`
///   を経由する（`egui-0.35.0/src/containers/combo_box.rs` の `combo_box_dyn` 関数、
///   `Popup::menu(&button_response)...show(|ui| { ui.set_min_width(...);
///   ScrollArea::vertical().max_height(height).show(ui, |ui| { ...; menu_contents(ui) })
///   })` の箇所）。`height` は `ComboBox::height()`（未指定なら
///   `Spacing::combo_height`）で、これは **上限**でしかない。
/// - `ScrollArea::begin`（`egui-0.35.0/src/containers/scroll_area.rs`）は
///   `let outer_size = available_outer.size().at_most(max_size);` で実際の高さ予算を
///   決める。`available_outer = ui.available_rect_before_wrap()` は
///   ポップアップの**外側の `Area`（`Popup::menu` が内部で作る）が今回どれだけの
///   矩形を与えたか**で決まるので、`available_outer` が `max_size`（＝
///   `ComboBox::height()`）より小さければ、`.height()` をいくら増やしても意味がない
///   （ユーザーが 400px を指定しても症状が変わらなかったのはこのため）。
/// - その `Area` 自身の高さは、**前回この `Area`（`Id` ごと）を表示したときに実測した
///   サイズをセッション中ずっと記憶し続ける**
///   （`egui-0.35.0/src/containers/area.rs` の `AreaState::size` の doc:
///   「Area size is intentionally NOT persisted between sessions, so that a bad tooltip
///   or menu size won't be remembered forever」＝アプリ再起動をまたいでは残さないが、
///   **1セッション中は残る**ことが明記されている）。具体的には
///   `Area::show()` の `let size = *state.size.get_or_insert_with(|| { ... })`
///   （初回・`Id` 未使用時だけ計算し直す）と、`Area::end()` の
///   `state.size = Some(content_ui.min_size());`（表示するたびに実測値で上書きする）
///   の組により、一度でも実測が小さければ以後もその小さい値が使われ続け、
///   `ScrollArea` の `auto_shrink`（既定 true。`scroll_area.rs` の
///   `inner_size[d] = inner_size[d].min(content_size[d])`）でその小さい枠へ
///   さらに縮められる ── 縮んだ状態が自己強化されて回復しない。
/// - 初回だけ正しいサイズになるのは、`Id` が初めて使われる回だけ
///   `sizing_pass = true`（`Area::show()` の `let mut sizing_pass = state.is_none();`）
///   になり、`ui_builder.sizing_pass().invisible()`
///   （制約の緩い非表示パスで実測してから即座に再描画する）で実測するため。
///   このサイジングパスは通常の描画パスと計測条件が完全には一致しない
///   （同バージョンの `sides.rs` にある `Ui::is_sizing_pass()` の利用例
///   「When the parent is being auto-sized the gap will be as small as possible」が
///   示すとおり、サイジングパスは通常パスより詰まった計測になりうる余地がある）。
///   **この計測条件の差の詳細（何 px 分ずれるか）は未確認・推測**だが、
///   「一度でも実測が小さいと自己修復しない」という上記の仕組み自体は
///   ソースの該当行で確認済みの事実であり、`.height()` を上げても直らないという
///   実際の再現症状とも整合する。
///
/// # 対応方針
///
/// egui はこの `Area` の記憶を個別に破棄する公開 API を持たない
/// （`Memory::reset_areas()` は全 `Area` を一括で捨てる無差別な操作で、他の
/// ウィンドウ/ポップアップにも影響するため使わない）。そこで、**このコンボの
/// ポップアップが閉じた直後にだけ `Id` を回転させる**（`ComboBox::from_id_salt` へ
/// 渡す salt に単調増加するエポック番号を混ぜる）。次に開いたときは必ず未使用の
/// `Id` になるため、`AreaState::load` は毎回 `None` を返し、`sizing_pass` を
/// 経た実測をやり直す。開いている間・閉じている間はエポックを変えないため、
/// ボタン自身のクリック判定（`Sense::click()` は同一 `Id` が押下フレームと
/// 離上フレームの両方に必要）やスクロール位置の連続性は壊れない。
///
/// `ComboBox::is_open` はボタンの `Id` から内部でポップアップ `Id` を導出して
/// 判定する公開 API なので、ここで使うボタン `Id` は `ComboBox::show_ui` 内部の計算と
/// 完全に一致させる必要がある。**注意: `ComboBox::from_id_salt(salt)` は salt を
/// `IdSalt::new(salt)` で包んでから `ui.make_persistent_id(..)` へ渡す**
/// （`combo_box.rs` の `id_salt: IdSalt::new(id_salt)` と
/// `let button_id = ui.make_persistent_id(id_salt);`）。`IdSalt` を経由すると
/// ハッシュが変わるため、`ui.make_persistent_id(salt)` と素の salt で計算した `Id` は
/// **一致しない**（ヘッドレスの egui 0.35 で実測: 素の salt で `is_open` を問うと常に
/// `false` になり、エポックが一度も進まなかった。2026-09-05）。同じ `ui` で
/// `IdSalt::new(salt)` を包んで計算すること。
///
/// 実測（同じヘッドレス再現、6 項目のメニュー）: 3 項目で開いた後に 6 項目で開くと
/// 回転なしでは高さ 78px（3 項目時 74px）のまま。回転ありでは 137px（最初から 6 項目で
/// 開いたときと同値）。
fn dim_combo_id_salt(ui: &egui::Ui, base: &'static str) -> (&'static str, u64) {
    let epoch_key = egui::Id::new(base).with("dim_combo_reopen_epoch");
    let was_open_key = egui::Id::new(base).with("dim_combo_was_open");
    let epoch: u64 = ui.data(|d| d.get_temp(epoch_key)).unwrap_or(0);
    let salt = (base, epoch);
    let button_id = ui.make_persistent_id(egui::IdSalt::new(salt));
    let is_open = egui::ComboBox::is_open(ui.ctx(), button_id);
    let was_open: bool = ui.data(|d| d.get_temp(was_open_key)).unwrap_or(false);
    if was_open && !is_open {
        // 閉じた瞬間だけエポックを進める。次回開くときは未使用の Id になる。
        ui.data_mut(|d| d.insert_temp(epoch_key, epoch.wrapping_add(1)));
    }
    ui.data_mut(|d| d.insert_temp(was_open_key, is_open));
    salt
}

/// 寸法補助記号の UI 表示ラベル（規定 5-3）。フォント文字の "φ" 等をそのまま使う
/// （G0-⑤ が縛るのは図面描画と出力のみ。DESIGN.md M9 設計判断1）。
/// [`DimSymbol`] は `#[non_exhaustive]` なのでワイルドカード腕を持つ（未知の記号は
/// 安全側の "?" 表示に倒す）。
fn dim_symbol_ui_label(symbol: DimSymbol) -> &'static str {
    match symbol {
        DimSymbol::Diameter => "φ",
        DimSymbol::SphereDiameter => "Sφ",
        DimSymbol::Square => "□",
        DimSymbol::Radius => "R",
        DimSymbol::SphereRadius => "SR",
        DimSymbol::ControlRadius => "CR",
        DimSymbol::Chamfer => "C",
        DimSymbol::Thickness => "t",
        _ => "?",
    }
}

/// 矢印配置コンボの日本語ラベル。
fn dim_arrow_placement_label(placement: ArrowPlacement) -> &'static str {
    match placement {
        ArrowPlacement::Auto => "自動",
        ArrowPlacement::Inside => "内向き",
        ArrowPlacement::Outside => "外向き",
    }
}

/// 寸法パネルの公差種別（UI 表示・編集の入口を選ぶタグ）。[`SizeTolerance`] は
/// `#[non_exhaustive]` なので、まだ知らないバリアントを安全に表示だけする
/// [`DimTolKindUi::Other`] を持つ（編集は提供しない。種別を明示的に変更するまで
/// 既存値を保つ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DimTolKindUi {
    None,
    Symmetric,
    Deviations,
    Fit,
    /// 未知の `SizeTolerance` バリアント（将来追加分の安全側の表示）。
    Other,
}

fn dim_tol_kind_ui(tolerance: &Option<SizeTolerance>) -> DimTolKindUi {
    match tolerance {
        None => DimTolKindUi::None,
        Some(SizeTolerance::Symmetric(_)) => DimTolKindUi::Symmetric,
        Some(SizeTolerance::Deviations { .. }) => DimTolKindUi::Deviations,
        Some(SizeTolerance::Fit(_)) => DimTolKindUi::Fit,
        Some(_) => DimTolKindUi::Other,
    }
}

fn dim_tol_kind_label(kind: DimTolKindUi) -> &'static str {
    match kind {
        DimTolKindUi::None => "なし",
        DimTolKindUi::Symmetric => "± 対称",
        DimTolKindUi::Deviations => "上下偏差",
        DimTolKindUi::Fit => "はめあい記号",
        DimTolKindUi::Other => "(その他)",
    }
}

/// 寸法エンティティの幾何から種別と注記を取り出す（寸法以外は `None`）。
fn dim_kind_and_annotation(geom: &EntityGeom) -> Option<(DimKind, &DimAnnotation)> {
    match geom {
        EntityGeom::DimLinear(d) => Some((DimKind::Linear, &d.annotation)),
        EntityGeom::DimRadial(d) => Some((DimKind::Radial, &d.annotation)),
        EntityGeom::DimDiameter(d) => Some((DimKind::Diameter, &d.annotation)),
        _ => None,
    }
}

/// `geom` の注記だけを `annotation` へ差し替えた新しい幾何を作る（寸法以外は `None`）。
fn dim_geom_with_annotation(geom: &EntityGeom, annotation: DimAnnotation) -> Option<EntityGeom> {
    match geom {
        EntityGeom::DimLinear(d) => Some(EntityGeom::DimLinear(DimLinear {
            annotation,
            ..d.clone()
        })),
        EntityGeom::DimRadial(d) => Some(EntityGeom::DimRadial(DimRadial {
            annotation,
            ..d.clone()
        })),
        EntityGeom::DimDiameter(d) => Some(EntityGeom::DimDiameter(DimDiameter {
            annotation,
            ..d.clone()
        })),
        _ => None,
    }
}

/// イテレータの全要素が等しければ `Some(値)`、1件でも異なれば `None`（空なら `None`）。
/// 寸法パネルの「全件一致していれば表示、不一致なら空欄/混在表示」に使う。
fn all_same<T: PartialEq + Clone>(mut it: impl Iterator<Item = T>) -> Option<T> {
    let first = it.next()?;
    if it.all(|v| v == first) {
        Some(first)
    } else {
        None
    }
}

/// `kinds` に含まれる全種別の [`DimKind::allowed_symbols`] の共通部分を返す
/// （記号コンボの選択肢。core が拒否する組合せを最初から選ばせないための絞り込み。
/// 表の出典は `DimKind::allowed_symbols` のみ — ここではその共通部分を計算するだけで、
/// 記号の可否そのものは二重に持たない）。`kinds` が空なら空を返す。
fn common_allowed_symbols(kinds: impl Iterator<Item = DimKind>) -> Vec<DimSymbol> {
    let mut acc: Option<Vec<DimSymbol>> = None;
    for kind in kinds {
        let set = kind.allowed_symbols();
        acc = Some(match acc {
            None => set.to_vec(),
            Some(prev) => prev.into_iter().filter(|s| set.contains(s)).collect(),
        });
    }
    acc.unwrap_or_default()
}

/// 桁数「スタイルに従う」チェックボックスの表示状態（チェック済み＝スタイルに従う）を、
/// 永続的な編集フラグ `decimals_editing` と現在の注記から算出した `all_follow_style`
/// から導出する（Codex adversarial review 2026-09-04 差し戻し対応A）。
///
/// `all_follow_style` だけを見て判定すると、チェックを外した直後（まだ
/// `decimals_override` を適用していない設計なので `all_follow_style` は依然 `true`）に
/// 次のフレームで即座にチェックへ戻ってしまう「残像」バグが起きる。`decimals_editing`
/// （ユーザーが明示的にチェックを外した、という操作そのものを表す永続フラグ）を
/// 併用し、どちらかが「編集中」を示していれば未チェック（編集欄を出す）とする。
#[inline]
#[must_use]
fn dim_decimals_follow_style_checked(decimals_editing: bool, all_follow_style: bool) -> bool {
    !decimals_editing && all_follow_style
}

/// 「寸法」セクション見出し直下に出す選択集合の内訳（種別ごとの件数）。
///
/// Codex adversarial review 2026-09-04 差し戻し対応B。`SelectTool::on_click` は
/// shift 無しクリックで選択へ**追加**する累積方式（既存・意図的な仕様。同メソッドの
/// doc と `click_accumulates_selection_without_duplicates` テスト参照）であり、
/// 寸法を選び直したつもりでも Shift クリックや Esc を挟まなければ前の選択が残る。
/// その結果、記号コンボの選択肢が縮んだり「(混在)」表示になったりする ── これは
/// `dim_panel` 側の不具合ではなく、累積選択の結果を正しく反映しているだけである。
/// この表示はその状態を隠さず可視化する。
fn dim_selection_summary(live: &[(EntityId, DimKind, DimAnnotation)]) -> String {
    let mut linear = 0usize;
    let mut radial = 0usize;
    let mut diameter = 0usize;
    for (_, kind, _) in live {
        match kind {
            DimKind::Linear => linear += 1,
            DimKind::Radial => radial += 1,
            DimKind::Diameter => diameter += 1,
        }
    }
    format!(
        "選択中: {} 件(長さ {linear}・半径 {radial}・直径 {diameter})",
        live.len()
    )
}

/// 対称公差（± v）の入力欄をパースする（[`SizeTolerance::validate`] の値検証はここでは
/// 行わず core 側〔`build_annotation_edit_commands`〕へ委ねる。ここは文字列 → 数値の
/// パース失敗だけを日本語メッセージへ変換する）。
fn parse_symmetric_tolerance_input(input: &str) -> Result<SizeTolerance, String> {
    input
        .trim()
        .parse::<f64>()
        .map(SizeTolerance::Symmetric)
        .map_err(|_| "公差値は数値で入力してください".to_string())
}

/// 上下偏差の入力欄をパースする（[`parse_symmetric_tolerance_input`] と同じ位置づけ）。
fn parse_deviations_tolerance_input(upper: &str, lower: &str) -> Result<SizeTolerance, String> {
    let parse = |s: &str| -> Result<f64, String> {
        s.trim()
            .parse::<f64>()
            .map_err(|_| "上下偏差は数値で入力してください".to_string())
    };
    Ok(SizeTolerance::Deviations {
        upper: parse(upper)?,
        lower: parse(lower)?,
    })
}

/// `live`（選択中の寸法。id・種別・現在の注記）へ `mutate` を適用し、変化があった分だけ
/// `Command::ModifyEntity` を組み立てる。`DimAnnotation::validate(kind)` を通してから
/// コマンド化するので、種別に許されない記号・不正な公差値・桁数上限超過・非有限座標・
/// 空/制御文字混じりの表示値上書きはここで弾かれ、理由が `errors` へ積まれる
/// （呼び出し側がまとめてステータスバーへ出す）。無変化のエンティティは何も積まない。
fn build_annotation_edit_commands(
    document: &Document,
    live: &[(EntityId, DimKind, DimAnnotation)],
    mut mutate: impl FnMut(&mut DimAnnotation),
) -> (Vec<Command>, Vec<String>) {
    let mut commands = Vec::new();
    let mut errors = Vec::new();
    for (id, kind, annotation) in live {
        let mut new_annotation = annotation.clone();
        mutate(&mut new_annotation);
        if new_annotation == *annotation {
            continue;
        }
        if let Err(err) = new_annotation.validate(*kind) {
            errors.push(err.to_string());
            continue;
        }
        let Some(entity) = document.entity(*id) else {
            continue;
        };
        let Some(new_geom) = dim_geom_with_annotation(&entity.geom, new_annotation) else {
            continue;
        };
        commands.push(Command::ModifyEntity { id: *id, new_geom });
    }
    (commands, errors)
}

/// 選択中の寸法 ID 集合が `edit_target` から変わっていたら、寸法パネルの入力欄一式
/// （公差種別編集モード・各数値入力・エラー表示・桁数入力・表示値上書き入力）を
/// 新しい選択の共通値へ再同期する。戻り値は「再同期した（＝選択が変わった）か」。
///
/// **これが無いと何が起きるか**（Codex adversarial review 2026-09-04 指摘）:
/// 寸法Aを選んで公差や表示値の入力を書きかけのまま選択をBへ切り替えても、UI 状態
/// （`String` バッファ）は寸法IDに紐付いていないためA用の入力がそのまま残り、
/// 「確定」を押すとA用の値がBへ `ModifyEntity` されてしまう。逆に、既に
/// `value_override` を持つ寸法へ選択を移しても入力欄は空のままなので、空欄のまま
/// 「確定」を押すと既存値が消えてしまう。
///
/// 選択が変わっていないフレームでは何もしない（入力途中の文字列を消さないため）。
#[allow(clippy::too_many_arguments)]
fn sync_dim_edit_state(
    edit_target: &mut Vec<EntityId>,
    live: &[(EntityId, DimKind, DimAnnotation)],
    tol_editing: &mut Option<DimTolKindUi>,
    tol_symmetric_input: &mut String,
    tol_upper_input: &mut String,
    tol_lower_input: &mut String,
    tol_fit_input: &mut String,
    tol_input_error: &mut Option<String>,
    decimals_editing: &mut bool,
    decimals_input: &mut String,
    decimals_input_error: &mut Option<String>,
    value_override_input: &mut String,
) -> bool {
    let mut current_ids: Vec<EntityId> = live.iter().map(|(id, _, _)| *id).collect();
    current_ids.sort();
    if *edit_target == current_ids {
        return false;
    }
    *edit_target = current_ids;

    // まず全入力・エラー・編集中フラグをリセットする。
    *tol_editing = None;
    tol_symmetric_input.clear();
    tol_upper_input.clear();
    tol_lower_input.clear();
    tol_fit_input.clear();
    *tol_input_error = None;
    *decimals_editing = false;
    decimals_input.clear();
    *decimals_input_error = None;
    value_override_input.clear();

    // 新しい選択の共通値（全件一致するものだけ）で入力欄を埋め直す。不一致（混在）は
    // 空欄のままにし、パネル側が「(混在)」表示で示す。
    if let Some(Some(tolerance)) = all_same(live.iter().map(|(_, _, a)| a.tolerance.clone())) {
        match tolerance {
            SizeTolerance::Symmetric(v) => *tol_symmetric_input = format!("{v}"),
            SizeTolerance::Deviations { upper, lower } => {
                *tol_upper_input = format!("{upper}");
                *tol_lower_input = format!("{lower}");
            }
            SizeTolerance::Fit(fit) => *tol_fit_input = fit.as_str().to_string(),
            // 将来の SizeTolerance バリアント（#[non_exhaustive]）は編集欄を持たない
            // （DimTolKindUi::Other と同じ「安全側は表示のみ」の扱い）。
            _ => {}
        }
    }
    if let Some(Some(n)) = all_same(live.iter().map(|(_, _, a)| a.decimals_override)) {
        *decimals_input = n.to_string();
    }
    if let Some(Some(value)) = all_same(live.iter().map(|(_, _, a)| a.value_override.clone())) {
        *value_override_input = value;
    }
    true
}

/// 右パネルの「寸法」セクション（M9タスク50-2）。選択中に寸法エンティティ
/// （`DimLinear`/`DimRadial`/`DimDiameter`）が1つ以上含まれるときだけ表示する。
///
/// 複数選択・複数種別混在に対応する: 値は「全件一致していれば表示、不一致なら
/// 空欄/混在表示」、編集は選択中の全寸法へ適用し `Command::Batch` 1回（undo 1単位）で
/// コミットする。記号コンボの選択肢は全件の [`DimKind::allowed_symbols`] の共通部分のみ
/// （核が拒否する組合せを選ばせない）。
#[allow(clippy::too_many_arguments)]
fn dim_panel(
    ui: &mut egui::Ui,
    document: &mut Document,
    selection: &[EntityId],
    edit_target: &mut Vec<EntityId>,
    tol_editing: &mut Option<DimTolKindUi>,
    tol_symmetric_input: &mut String,
    tol_upper_input: &mut String,
    tol_lower_input: &mut String,
    tol_fit_input: &mut String,
    tol_input_error: &mut Option<String>,
    decimals_editing: &mut bool,
    decimals_input: &mut String,
    decimals_input_error: &mut Option<String>,
    value_override_input: &mut String,
    status: &mut Option<StatusMessage>,
    now: f64,
) {
    // 選択されていても墓標化・削除済みの ID、寸法以外のエンティティは除く
    // （`entity_style_panel` と同じ防御。DESIGN.md M9 タスク50）。
    let live: Vec<(EntityId, DimKind, DimAnnotation)> = selection
        .iter()
        .filter_map(|&id| {
            let entity = document.entity(id)?;
            let (kind, annotation) = dim_kind_and_annotation(&entity.geom)?;
            Some((id, kind, annotation.clone()))
        })
        .collect();

    // 選択が変わっていたら入力欄一式を新しい選択の共通値へ再同期する（`live` が空でも
    // 呼ぶ — 選択解除後に別の寸法を選んだときも正しく再同期させるため）。
    sync_dim_edit_state(
        edit_target,
        &live,
        tol_editing,
        tol_symmetric_input,
        tol_upper_input,
        tol_lower_input,
        tol_fit_input,
        tol_input_error,
        decimals_editing,
        decimals_input,
        decimals_input_error,
        value_override_input,
    );

    if live.is_empty() {
        return;
    }

    ui.separator();
    ui.heading("寸法");
    // B の差し戻し対応: SelectTool::on_click は shift 無しクリックで選択へ**追加**する
    // 累積方式（既存・意図的な仕様。`SelectTool::on_click` の doc と
    // `click_accumulates_selection_without_duplicates` テスト参照）。寸法を選び直した
    // つもりでも前の選択が残ったままだと記号コンボ等が意図せず混在扱いになるため、
    // 選択の内訳をここに出して目視できるようにする。
    ui.label(dim_selection_summary(&live));

    let mut pending: Vec<Command> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut apply = |cmds: Vec<Command>, errs: Vec<String>| {
        pending.extend(cmds);
        errors.extend(errs);
    };

    // --- 記号: 全種別の allowed_symbols の共通部分だけを選択肢にする ---
    let allowed_symbols = common_allowed_symbols(live.iter().map(|(_, kind, _)| *kind));
    let symbol_common = all_same(live.iter().map(|(_, _, a)| a.symbol));
    ui.horizontal(|ui| {
        ui.label("記号:");
        let selected_text = match symbol_common {
            Some(Some(sym)) => dim_symbol_ui_label(sym),
            Some(None) => "なし",
            None => "(混在)",
        };
        egui::ComboBox::from_id_salt(dim_combo_id_salt(ui, "dim_symbol"))
            .height(DIM_COMBO_MAX_HEIGHT)
            .selected_text(selected_text)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(symbol_common == Some(None), "なし")
                    .clicked()
                {
                    let (cmds, errs) =
                        build_annotation_edit_commands(document, &live, |a| a.symbol = None);
                    apply(cmds, errs);
                }
                for &sym in &allowed_symbols {
                    if ui
                        .selectable_label(
                            symbol_common == Some(Some(sym)),
                            dim_symbol_ui_label(sym),
                        )
                        .clicked()
                    {
                        let (cmds, errs) = build_annotation_edit_commands(document, &live, |a| {
                            a.symbol = Some(sym);
                        });
                        apply(cmds, errs);
                    }
                }
            });
    });

    // --- 公差: 種別コンボ（なし/±対称/上下偏差/はめあい記号）+ 値入力 ---
    let tol_kind_common = all_same(live.iter().map(|(_, _, a)| dim_tol_kind_ui(&a.tolerance)));
    // 明示的に編集中の種別があればそれを、無ければ選択中の共通種別を表示に使う
    // （`sheet_panel` の尺度カスタム入力と同じ「明示編集フラグが document 由来の値に
    // 優先する」流儀）。
    let display_kind = tol_editing.unwrap_or(tol_kind_common.unwrap_or(DimTolKindUi::None));
    ui.horizontal(|ui| {
        ui.label("公差:");
        let selected_text = if tol_editing.is_none() && tol_kind_common.is_none() {
            "(混在)"
        } else {
            dim_tol_kind_label(display_kind)
        };
        egui::ComboBox::from_id_salt(dim_combo_id_salt(ui, "dim_tol_kind"))
            .height(DIM_COMBO_MAX_HEIGHT)
            .selected_text(selected_text)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(display_kind == DimTolKindUi::None, "なし")
                    .clicked()
                {
                    let (cmds, errs) =
                        build_annotation_edit_commands(document, &live, |a| a.tolerance = None);
                    apply(cmds, errs);
                    *tol_editing = None;
                    *tol_input_error = None;
                }
                for kind in [
                    DimTolKindUi::Symmetric,
                    DimTolKindUi::Deviations,
                    DimTolKindUi::Fit,
                ] {
                    if ui
                        .selectable_label(display_kind == kind, dim_tol_kind_label(kind))
                        .clicked()
                    {
                        // 値の要る種別は選んだだけでは確定しない（数値入力が要るため）。
                        // 編集欄を開き、現在の共通値があれば埋める。
                        *tol_editing = Some(kind);
                        *tol_input_error = None;
                        let common_tol = all_same(live.iter().map(|(_, _, a)| a.tolerance.clone()));
                        match (kind, common_tol) {
                            (DimTolKindUi::Symmetric, Some(Some(SizeTolerance::Symmetric(v)))) => {
                                *tol_symmetric_input = format!("{v}");
                            }
                            (
                                DimTolKindUi::Deviations,
                                Some(Some(SizeTolerance::Deviations { upper, lower })),
                            ) => {
                                *tol_upper_input = format!("{upper}");
                                *tol_lower_input = format!("{lower}");
                            }
                            (DimTolKindUi::Fit, Some(Some(SizeTolerance::Fit(fit)))) => {
                                *tol_fit_input = fit.as_str().to_string();
                            }
                            _ => {}
                        }
                    }
                }
            });
    });
    match display_kind {
        DimTolKindUi::Symmetric => {
            ui.horizontal(|ui| {
                ui.label("± :");
                ui.add(egui::TextEdit::singleline(tol_symmetric_input).desired_width(64.0));
                if ui.button("確定").clicked() {
                    match parse_symmetric_tolerance_input(tol_symmetric_input) {
                        Ok(tolerance) => {
                            let (cmds, errs) =
                                build_annotation_edit_commands(document, &live, |a| {
                                    a.tolerance = Some(tolerance.clone());
                                });
                            if errs.is_empty() {
                                apply(cmds, Vec::new());
                                *tol_editing = None;
                                *tol_input_error = None;
                            } else {
                                *tol_input_error = Some(errs.join("; "));
                            }
                        }
                        Err(err) => {
                            *tol_input_error = Some(err);
                        }
                    }
                }
            });
        }
        DimTolKindUi::Deviations => {
            ui.horizontal(|ui| {
                ui.label("上 :");
                ui.add(egui::TextEdit::singleline(tol_upper_input).desired_width(56.0));
                ui.label("下 :");
                ui.add(egui::TextEdit::singleline(tol_lower_input).desired_width(56.0));
                if ui.button("確定").clicked() {
                    match parse_deviations_tolerance_input(tol_upper_input, tol_lower_input) {
                        Ok(tolerance) => {
                            let (cmds, errs) =
                                build_annotation_edit_commands(document, &live, |a| {
                                    a.tolerance = Some(tolerance.clone());
                                });
                            if errs.is_empty() {
                                apply(cmds, Vec::new());
                                *tol_editing = None;
                                *tol_input_error = None;
                            } else {
                                *tol_input_error = Some(errs.join("; "));
                            }
                        }
                        Err(err) => {
                            *tol_input_error = Some(err);
                        }
                    }
                }
            });
        }
        DimTolKindUi::Fit => {
            ui.horizontal(|ui| {
                ui.label("記号:");
                ui.add(egui::TextEdit::singleline(tol_fit_input).desired_width(56.0));
                if ui.button("確定").clicked() {
                    match FitClass::new(tol_fit_input.clone()) {
                        Ok(fit) => {
                            let (cmds, errs) =
                                build_annotation_edit_commands(document, &live, |a| {
                                    a.tolerance = Some(SizeTolerance::Fit(fit.clone()));
                                });
                            apply(cmds, errs);
                            *tol_editing = None;
                            *tol_input_error = None;
                        }
                        Err(err) => {
                            *tol_input_error = Some(err.to_string());
                        }
                    }
                }
            });
        }
        DimTolKindUi::None | DimTolKindUi::Other => {}
    }
    if let Some(err) = tol_input_error {
        ui.colored_label(STATUS_MESSAGE_COLOR, err.as_str());
    }

    // --- 桁数: 「スタイルに従う」チェック + 外したときの明示値入力 ---
    // A の差し戻し対応: チェックを外した直後はまだ `decimals_override` を適用していない
    // （コマンドはまだ発行しない設計）ため、`all_follow_style` だけでチェック表示を
    // 決めると次のフレームで即座にチェックへ戻ってしまう。永続フラグ `decimals_editing`
    // を併用する `dim_decimals_follow_style_checked` で判定する。
    let style_decimals = document.dim_style().decimals;
    let all_follow_style = live.iter().all(|(_, _, a)| a.decimals_override.is_none());
    ui.horizontal(|ui| {
        let mut follow_style =
            dim_decimals_follow_style_checked(*decimals_editing, all_follow_style);
        let label = format!("スタイルに従う(現在 {style_decimals} 桁)");
        if ui.checkbox(&mut follow_style, label).changed() {
            if follow_style {
                // 再チェック: スタイルへ戻し、編集状態を終える。
                let (cmds, errs) = build_annotation_edit_commands(document, &live, |a| {
                    a.decimals_override = None;
                });
                apply(cmds, errs);
                *decimals_editing = false;
                decimals_input.clear();
                *decimals_input_error = None;
            } else {
                // 外した直後は編集状態に入るだけで、コマンドはまだ適用しない
                // （数値入力→「確定」を待つ）。現在の解決済み桁数（スタイル or
                // 既存の明示値）を編集欄の初期値にする。
                *decimals_editing = true;
                *decimals_input_error = None;
                if decimals_input.is_empty() {
                    let initial = live
                        .first()
                        .map(|(_, _, a)| document.dim_style().resolve_decimals(a.decimals_override))
                        .unwrap_or(style_decimals);
                    *decimals_input = initial.to_string();
                }
            }
        }
        if !follow_style {
            // チェックボックスを外した直接操作を経ずに欄が表示される場合（選択直後から
            // 既に一部/全部が明示上書き済み）にも、空欄のままにしない。
            if decimals_input.is_empty() {
                let initial = live
                    .first()
                    .map(|(_, _, a)| document.dim_style().resolve_decimals(a.decimals_override))
                    .unwrap_or(style_decimals);
                *decimals_input = initial.to_string();
            }
            ui.add(egui::TextEdit::singleline(decimals_input).desired_width(32.0));
            if ui.button("確定").clicked() {
                match decimals_input.trim().parse::<u8>() {
                    Ok(n) if n <= MAX_DIM_DECIMALS => {
                        let (cmds, errs) = build_annotation_edit_commands(document, &live, |a| {
                            a.decimals_override = Some(n);
                        });
                        apply(cmds, errs);
                        *decimals_input_error = None;
                        // 編集状態は維持する（再チェックまでは欄を出し続け、続けて
                        // 別の値を試せるようにする）。
                    }
                    Ok(n) => {
                        *decimals_input_error =
                            Some(format!("{n} は上限 {MAX_DIM_DECIMALS} を超えています"));
                    }
                    Err(_) => {
                        *decimals_input_error = Some("桁数は整数で入力してください".to_string());
                    }
                }
            }
        }
    });
    if let Some(err) = decimals_input_error {
        ui.colored_label(STATUS_MESSAGE_COLOR, err.as_str());
    }

    // --- 矢印の配置 ---
    let arrow_common = all_same(live.iter().map(|(_, _, a)| a.arrow_placement));
    ui.horizontal(|ui| {
        ui.label("矢印の配置:");
        let selected_text = arrow_common.map_or("(混在)", dim_arrow_placement_label);
        egui::ComboBox::from_id_salt(dim_combo_id_salt(ui, "dim_arrow_placement"))
            .height(DIM_COMBO_MAX_HEIGHT)
            .selected_text(selected_text)
            .show_ui(ui, |ui| {
                for placement in [
                    ArrowPlacement::Auto,
                    ArrowPlacement::Inside,
                    ArrowPlacement::Outside,
                ] {
                    if ui
                        .selectable_label(
                            arrow_common == Some(placement),
                            dim_arrow_placement_label(placement),
                        )
                        .clicked()
                    {
                        let (cmds, errs) = build_annotation_edit_commands(document, &live, |a| {
                            a.arrow_placement = placement;
                        });
                        apply(cmds, errs);
                    }
                }
            });
    });

    // --- 表示値の上書き（非比例寸法） ---
    // 選択が変わったときは `sync_dim_edit_state` が既存の共通値（全件一致するもの）を
    // この欄へ読み込み済み（混在時は空欄+「(混在)」相当の表示）。空欄のままの「確定」は
    // 既存値を消してしまわないよう何もしない（値の消去は「解除」ボタンのみで行う）。
    ui.horizontal(|ui| {
        ui.label("表示値の上書き:");
        ui.add(egui::TextEdit::singleline(value_override_input).desired_width(96.0));
        if ui.button("確定").clicked() {
            let trimmed = value_override_input.trim();
            if !trimmed.is_empty() {
                let new_value = Some(trimmed.to_string());
                let (cmds, errs) = build_annotation_edit_commands(document, &live, |a| {
                    a.value_override = new_value.clone();
                });
                apply(cmds, errs);
            }
        }
        let has_override = live.iter().any(|(_, _, a)| a.value_override.is_some());
        if has_override && ui.button("解除").clicked() {
            let (cmds, errs) = build_annotation_edit_commands(document, &live, |a| {
                a.value_override = None;
            });
            apply(cmds, errs);
            value_override_input.clear();
        }
    });

    // --- 文字位置（M9 タスク51）---
    // `text_anchor` はドラッグ（`SelectTool` の文字ブロックドラッグ、main.rs の
    // `handle_select_input`）でしか `Some` にならない。ここは状態表示と「自動配置に
    // 戻す」操作のみを持つ（設計判断7）。
    ui.horizontal(|ui| {
        ui.label("文字位置:");
        let some_count = live
            .iter()
            .filter(|(_, _, a)| a.text_anchor.is_some())
            .count();
        let label = if some_count == 0 {
            "自動"
        } else if some_count == live.len() {
            "手動"
        } else {
            "混在"
        };
        ui.label(label);
        if some_count > 0 && ui.button("自動配置に戻す").clicked() {
            let (cmds, errs) = build_annotation_edit_commands(document, &live, |a| {
                a.text_anchor = None;
            });
            apply(cmds, errs);
        }
    });

    if !pending.is_empty()
        && let Err(err) = document.apply(Command::Batch(pending))
    {
        errors.push(err.to_string());
    }
    if !errors.is_empty() {
        set_status(
            status,
            now,
            format!("寸法の変更に失敗しました: {}", errors.join("; ")),
        );
    }
}

/// 用紙サイズの日本語ラベル（右パネルの用紙コンボ専用）。
///
/// `frame::` 側の欄文字用ラベル（横は "A4"、縦は "A4 縦" と向きを合成する）とは
/// 別の関心事（こちらは向きを別コンボで独立して選ぶ）なので同居させない。
fn paper_size_combo_label(paper: PaperSize) -> &'static str {
    match paper {
        PaperSize::A0 => "A0",
        PaperSize::A1 => "A1",
        PaperSize::A2 => "A2",
        PaperSize::A3 => "A3",
        PaperSize::A4 => "A4",
    }
}

/// UI で提示する尺度プリセット（紙:モデル。`製図規定.md` 第3章の推奨尺度表:
/// 倍尺 50:1〜2:1・現尺 1:1・縮尺 1:2〜1:10000）。
const SCALE_PRESETS: [(u32, u32); 18] = [
    (50, 1),
    (20, 1),
    (10, 1),
    (5, 1),
    (2, 1),
    (1, 1),
    (1, 2),
    (1, 5),
    (1, 10),
    (1, 20),
    (1, 50),
    (1, 100),
    (1, 200),
    (1, 500),
    (1, 1000),
    (1, 2000),
    (1, 5000),
    (1, 10000),
];

/// 表題欄編集ダイアログの作業コピー（`SheetMeta::fields` の編集中の値、M8タスク38）。
///
/// OK で [`TitleBlockDialogState::to_sheet`] が組み立てる `SheetMeta` を
/// `Command::SetSheet` 1回で適用する（ダイアログセッション全体が undo 1単位）。
/// キャンセル/モーダル外クリックでは何も適用しない。
struct TitleBlockDialogState {
    drawing_number: String,
    drawing_title: String,
    projection: ProjectionMethod,
    author: String,
    date: String,
    revision: String,
}

impl TitleBlockDialogState {
    /// 現在の `SheetMeta::fields` から作業コピーを作る（ダイアログを開くとき）。
    fn from_sheet(sheet: &SheetMeta) -> Self {
        Self {
            drawing_number: sheet.fields.drawing_number.clone(),
            drawing_title: sheet.fields.drawing_title.clone(),
            projection: sheet.fields.projection,
            author: sheet.fields.author.clone(),
            date: sheet.fields.date.clone(),
            revision: sheet.fields.revision.clone(),
        }
    }

    /// `base`（現在の `SheetMeta`）の `fields` だけをこの作業コピーで置き換えた新しい
    /// `SheetMeta` を組み立てる（純関数、単体テスト対象）。尺度・用紙・様式・枠表示は
    /// `base` のまま変えない（このダイアログの編集対象は記入欄のみ）。
    fn to_sheet(&self, base: &SheetMeta) -> SheetMeta {
        SheetMeta {
            fields: TitleBlockFields {
                drawing_number: self.drawing_number.clone(),
                drawing_title: self.drawing_title.clone(),
                projection: self.projection,
                author: self.author.clone(),
                date: self.date.clone(),
                revision: self.revision.clone(),
            },
            ..base.clone()
        }
    }
}

/// 右パネルの「図面」セクション（M8タスク38）。用紙・向き・様式・枠表示・尺度の
/// 変更を提供する。用紙/向き/様式コンボ・枠表示チェックボックス・尺度プリセット選択は
/// 各変更が即 `Command::SetSheet` 1回 = 1操作 1 undo（レイヤーパネルの
/// `SetLayerProps` と同じ粒度。core の no-op 判定が同値選択を吸収する）。
/// 表題欄の記入自体はモーダルダイアログ（`McadApp::sheet_dialog`）へ分離する。
///
/// ラベル等は日本語（右パネルは既に日本語領域。日本語化は領域単位で完結させる規約）。
#[allow(clippy::too_many_arguments)]
/// 図面（用紙・向き・様式・尺度・図面枠表示）のパネルを描く。
///
/// 戻り値は `(sheet_changed, plot_color_mode_changed)`。
///
/// `sheet_changed` は「`Command::SetSheet` が適用され成功したか」（M8 タスク41）。
/// 呼び出し側はこれが `true` のときだけ [`config::Config::remember_sheet_defaults`] を
/// 呼び、既定への追随が実際に必要か判断する。
///
/// `plot_color_mode_changed` は出力色コンボの選択が変わったか（モノクロ化-2）。
/// 出力色は `SheetMeta` ではなく `config.json` の設定なので `Command::SetSheet` は
/// 通さない — 呼び出し側はこれが `true` のときだけ `persist_config` を呼ぶ。
fn sheet_panel(
    ui: &mut egui::Ui,
    document: &mut Document,
    sheet_dialog: &mut Option<TitleBlockDialogState>,
    scale_custom_selected: &mut bool,
    scale_custom_input: &mut String,
    scale_input_error: &mut Option<String>,
    plot_color_mode: &mut plot::PlotColorMode,
    status: &mut Option<StatusMessage>,
    now: f64,
) -> (bool, bool) {
    ui.separator();
    ui.heading("図面");

    let sheet = document.sheet().clone();
    let mut pending: Option<SheetMeta> = None;

    ui.horizontal(|ui| {
        ui.label("用紙:");
        egui::ComboBox::from_id_salt("sheet_paper")
            .selected_text(paper_size_combo_label(sheet.paper))
            .show_ui(ui, |ui| {
                for paper in [
                    PaperSize::A0,
                    PaperSize::A1,
                    PaperSize::A2,
                    PaperSize::A3,
                    PaperSize::A4,
                ] {
                    if ui
                        .selectable_label(paper == sheet.paper, paper_size_combo_label(paper))
                        .clicked()
                        && paper != sheet.paper
                    {
                        pending = Some(SheetMeta {
                            paper,
                            ..sheet.clone()
                        });
                    }
                }
            });

        ui.label("向き:");
        egui::ComboBox::from_id_salt("sheet_orientation")
            .selected_text(match sheet.orientation {
                Orientation::Landscape => "横",
                Orientation::Portrait => "縦",
            })
            .show_ui(ui, |ui| {
                for (orientation, label) in [
                    (Orientation::Landscape, "横"),
                    (Orientation::Portrait, "縦"),
                ] {
                    if ui
                        .selectable_label(orientation == sheet.orientation, label)
                        .clicked()
                        && orientation != sheet.orientation
                    {
                        pending = Some(SheetMeta {
                            orientation,
                            ..sheet.clone()
                        });
                    }
                }
            });
    });

    ui.horizontal(|ui| {
        ui.label("様式:");
        // 「ユーザー定義」は Custom の選択中表示のみで、選択肢には出さない
        // （ユーザー確定。A/B/C を選ぶと Custom テンプレートは置換される）。
        let is_custom = matches!(sheet.title_block, TitleBlockKind::Custom(_));
        let selected_text = if is_custom {
            "ユーザー定義"
        } else {
            match sheet.title_block {
                TitleBlockKind::A => "A",
                TitleBlockKind::B => "B",
                TitleBlockKind::C => "C",
                TitleBlockKind::Custom(_) => unreachable!("is_custom はこの腕を除外済み"),
            }
        };
        egui::ComboBox::from_id_salt("sheet_title_block")
            .selected_text(selected_text)
            .show_ui(ui, |ui| {
                for (label, kind) in [
                    ("A", TitleBlockKind::A),
                    ("B", TitleBlockKind::B),
                    ("C", TitleBlockKind::C),
                ] {
                    let selected = !is_custom && sheet.title_block == kind;
                    if ui.selectable_label(selected, label).clicked() && !selected {
                        pending = Some(SheetMeta {
                            title_block: kind,
                            ..sheet.clone()
                        });
                    }
                }
            });

        let mut frame_visible = sheet.frame_visible;
        if ui.checkbox(&mut frame_visible, "図面枠を表示").changed() {
            pending = Some(SheetMeta {
                frame_visible,
                ..sheet.clone()
            });
        }

        if ui.button("表題欄を編集…").clicked() {
            *sheet_dialog = Some(TitleBlockDialogState::from_sheet(&sheet));
        }
    });

    ui.horizontal(|ui| {
        ui.label("尺度:");
        let is_preset = SCALE_PRESETS
            .iter()
            .any(|&(n, d)| n == sheet.scale.num() && d == sheet.scale.den());
        let show_custom = *scale_custom_selected || !is_preset;
        let selected_text = if show_custom {
            "カスタム".to_owned()
        } else {
            format!("{}:{}", sheet.scale.num(), sheet.scale.den())
        };
        egui::ComboBox::from_id_salt("sheet_scale")
            .selected_text(selected_text)
            .show_ui(ui, |ui| {
                for &(n, d) in &SCALE_PRESETS {
                    let selected = !show_custom && n == sheet.scale.num() && d == sheet.scale.den();
                    if ui.selectable_label(selected, format!("{n}:{d}")).clicked() && !selected {
                        *scale_custom_selected = false;
                        *scale_input_error = None;
                        // 次に「カスタム」を開いたとき現在の尺度で埋め直すため空にする
                        // （古い入力値が残っていると、表示中の尺度と違う値を「適用」で
                        // 意図せず復活させてしまう）。
                        scale_custom_input.clear();
                        pending = Some(SheetMeta {
                            scale: Scale::new(n, d).expect("SCALE_PRESETS entries are valid"),
                            ..sheet.clone()
                        });
                    }
                }
                if ui.selectable_label(show_custom, "カスタム").clicked() && !show_custom {
                    *scale_custom_selected = true;
                    // 開いた時点の図面の尺度で同期する（下の「空なら埋める」は
                    // プリセットから切り替えた直後にも効くが、明示しておく）。
                    *scale_custom_input = format!("{}:{}", sheet.scale.num(), sheet.scale.den());
                    *scale_input_error = None;
                }
            });

        if show_custom {
            if scale_custom_input.is_empty() {
                *scale_custom_input = format!("{}:{}", sheet.scale.num(), sheet.scale.den());
            }
            ui.add(egui::TextEdit::singleline(scale_custom_input).desired_width(64.0));
            if ui.button("適用").clicked() {
                match parse_scale_input(scale_custom_input) {
                    Ok(scale) => {
                        *scale_input_error = None;
                        pending = Some(SheetMeta {
                            scale,
                            ..sheet.clone()
                        });
                    }
                    Err(err) => {
                        *scale_input_error = Some(err);
                    }
                }
            }
            // 不正尺度入力時は SetSheet を発行せず（履歴・世代とも不変）、入力文字列は
            // 保持して修正させる。ステータスバーは10秒で消える一時通知のためフォーム
            // 検証には使わず、入力欄横のインライン赤字ラベルで表示する。
            if let Some(err) = scale_input_error {
                ui.colored_label(STATUS_MESSAGE_COLOR, err.as_str());
            }
        }
    });

    // 出力色（モノクロ化-2）: 図面データではなく config.json の設定なので
    // `Command::SetSheet` は通さない。画面表示・図面データには無影響。
    let mut plot_color_mode_changed = false;
    ui.horizontal(|ui| {
        ui.label("出力色:");
        egui::ComboBox::from_id_salt("plot_color_mode")
            .selected_text(plot_color_mode_combo_label(*plot_color_mode))
            .show_ui(ui, |ui| {
                for mode in [
                    plot::PlotColorMode::Monochrome,
                    plot::PlotColorMode::Blueprint,
                    plot::PlotColorMode::Color,
                ] {
                    if ui
                        .selectable_label(
                            mode == *plot_color_mode,
                            plot_color_mode_combo_label(mode),
                        )
                        .clicked()
                        && mode != *plot_color_mode
                    {
                        *plot_color_mode = mode;
                        plot_color_mode_changed = true;
                    }
                }
            })
            .response
            .on_hover_text("SVG/PDF 出力の線色。画面表示と図面データには影響しません。");
    });

    let sheet_changed = match pending {
        Some(new_sheet) => match document.apply(Command::SetSheet(new_sheet)) {
            Ok(_) => true,
            Err(err) => {
                set_status(status, now, format!("図面設定の変更に失敗しました: {err}"));
                false
            }
        },
        None => false,
    };
    (sheet_changed, plot_color_mode_changed)
}

/// 出力色コンボの選択肢ラベル（モノクロ化-2、日本語）。
fn plot_color_mode_combo_label(mode: plot::PlotColorMode) -> &'static str {
    match mode {
        plot::PlotColorMode::Monochrome => "黒(モノクロ)",
        plot::PlotColorMode::Blueprint => "青図(白線)",
        plot::PlotColorMode::Color => "元の色",
    }
}

// ---------------------------------------------------------------------
// 寸法スタイルダイアログ（M9タスク50-3）
// ---------------------------------------------------------------------

/// 寸法スタイルダイアログの作業コピー。数値項目はすべて文字列で保持し（入力途中の
/// 不正値でドキュメントを壊さないため）、OK 時にまとめてパースする。表題欄編集
/// ダイアログ（[`TitleBlockDialogState`]）と同じ「ダイアログセッション全体が
/// `Command::SetDimStyle` 1回 = undo 1単位」の流儀。
struct DimStyleDialogState {
    text_height_mm: String,
    arrow_len_mm: String,
    decimals: String,
    trim_trailing_zeros: bool,
    ext_gap_mm: String,
    ext_overshoot_mm: String,
    text_gap_mm: String,
    tolerance_scale: String,
    /// OK 時のパース/検証失敗をインライン表示するための直近エラー。
    error: Option<String>,
}

impl DimStyleDialogState {
    /// 現在の [`DimStyle`] から作業コピーを作る（ダイアログを開くとき・「既定に戻す」）。
    fn from_style(style: &DimStyle) -> Self {
        Self {
            text_height_mm: format!("{}", style.text_height_mm),
            arrow_len_mm: format!("{}", style.arrow_len_mm),
            decimals: format!("{}", style.decimals),
            trim_trailing_zeros: style.trim_trailing_zeros,
            ext_gap_mm: format!("{}", style.ext_gap_mm),
            ext_overshoot_mm: format!("{}", style.ext_overshoot_mm),
            text_gap_mm: format!("{}", style.text_gap_mm),
            tolerance_scale: format!("{}", style.tolerance_scale),
            error: None,
        }
    }

    /// 作業コピーの文字列をパースして [`DimStyle`] を組み立てる（純関数、単体テスト対象）。
    /// パース失敗は日本語メッセージで返す。組み立てた値は最後に
    /// [`DimStyle::validate`] を通すので、範囲外の値（非有限・0以下・上限超過など）も
    /// ここで拒否される。
    fn to_dim_style(&self) -> Result<DimStyle, String> {
        let parse_mm = |s: &str, label: &str| -> Result<f64, String> {
            s.trim()
                .parse::<f64>()
                .map_err(|_| format!("{label}は数値で入力してください"))
        };
        let text_height_mm = parse_mm(&self.text_height_mm, "文字高さ")?;
        let arrow_len_mm = parse_mm(&self.arrow_len_mm, "矢の長さ")?;
        let decimals: u8 = self
            .decimals
            .trim()
            .parse()
            .map_err(|_| "小数桁数は0〜4の整数で入力してください".to_string())?;
        let ext_gap_mm = parse_mm(&self.ext_gap_mm, "補助線のすきま")?;
        let ext_overshoot_mm = parse_mm(&self.ext_overshoot_mm, "補助線の突き出し")?;
        let text_gap_mm = parse_mm(&self.text_gap_mm, "文字とのすきま")?;
        let tolerance_scale = parse_mm(&self.tolerance_scale, "公差文字の縮小率")?;
        let style = DimStyle {
            text_height_mm,
            arrow_len_mm,
            decimals,
            trim_trailing_zeros: self.trim_trailing_zeros,
            ext_gap_mm,
            ext_overshoot_mm,
            text_gap_mm,
            tolerance_scale,
        };
        style.validate()?;
        Ok(style)
    }
}

/// 右パネルの「寸法スタイル」行（M9タスク50-3）。ボタン1つの薄いセクションとして
/// 図面セクションの下に置く（表題欄編集ダイアログの「表題欄を編集…」ボタンと対になる）。
fn dim_style_panel(
    ui: &mut egui::Ui,
    dim_style_dialog: &mut Option<DimStyleDialogState>,
    document: &Document,
) {
    ui.separator();
    ui.heading("寸法スタイル");
    if ui.button("寸法スタイルを編集…").clicked() {
        *dim_style_dialog = Some(DimStyleDialogState::from_style(document.dim_style()));
    }
}

/// 素のカーソル位置 `raw` にスナップを掛ける。有効かつ候補が見つかれば
/// `(スナップ先, Some(結果))`、無効または候補なしなら `(raw, None)` を返す。
///
/// `extra_points` はアクティブな作図ツールの未確定頂点（[`Tool::snap_points`]）。
/// 作図中ツールを持たない呼び出し箇所（選択・配置モード等）は `&[]` を渡す。
fn apply_snap(
    document: &Document,
    enabled: bool,
    raw: Point2,
    radius: f64,
    grid_step: f64,
    extra_points: &[Point2],
) -> (Point2, Option<snap::SnapResult>) {
    if !enabled {
        return (raw, None);
    }
    match snap::snap(document, raw, radius, grid_step, extra_points) {
        Some(result) => (result.point, Some(result)),
        None => (raw, None),
    }
}

/// スナップと直交モード（ortho、v0.7.1）を「スナップ優先」で合成する（`ortho.rs` §2）。
///
/// `snap_result` が `Some`（スナップ候補が見つかった）ならその座標をそのまま使い、
/// ortho は適用しない。`None`（候補なし）のときだけ、`ortho_enabled` かつ
/// `origin`（[`tool::Tool::ortho_origin`]）が `Some` であれば `raw` を軸拘束する。
/// `Move`/`Click` の両経路が同じこの関数を通ることで、プレビューと確定がずれない。
#[must_use]
fn resolve_click_point(
    snap_result: Option<snap::SnapResult>,
    raw: Point2,
    ortho_enabled: bool,
    origin: Option<Point2>,
) -> Point2 {
    match snap_result {
        Some(result) => result.point,
        None if ortho_enabled => origin.map_or(raw, |o| ortho::constrain(o, raw)),
        None => raw,
    }
}

#[cfg(test)]
mod resolve_click_point_tests {
    use super::*;

    #[test]
    fn snap_wins_over_ortho() {
        let snap_result = Some(snap::SnapResult {
            kind: snap::SnapKind::Endpoint,
            point: Point2::new(9.0, 9.0),
        });
        let raw = Point2::new(10.0, 0.5);
        let origin = Some(Point2::new(0.0, 0.0));
        assert_eq!(
            resolve_click_point(snap_result, raw, true, origin),
            Point2::new(9.0, 9.0)
        );
    }

    #[test]
    fn no_snap_ortho_on_with_origin_constrains() {
        let raw = Point2::new(10.0, 0.5);
        let origin = Some(Point2::new(0.0, 0.0));
        assert_eq!(
            resolve_click_point(None, raw, true, origin),
            Point2::new(10.0, 0.0)
        );
    }

    #[test]
    fn ortho_off_returns_raw() {
        let raw = Point2::new(10.0, 0.5);
        let origin = Some(Point2::new(0.0, 0.0));
        assert_eq!(resolve_click_point(None, raw, false, origin), raw);
    }

    #[test]
    fn no_origin_returns_raw_even_if_ortho_on() {
        // 1点目のクリック待ちなど、基準点がまだ無い局面。
        let raw = Point2::new(10.0, 0.5);
        assert_eq!(resolve_click_point(None, raw, true, None), raw);
    }
}

/// 選択・編集モード（`tool_kind == Select`）のキャンバス入力を [`SelectTool`] へ渡す。
///
/// egui 組み込みのクリック/ドラッグ判定を利用し、単発クリック（＝選択）と
/// ドラッグ（＝矩形選択）を振り分ける。ドラッグは矩形選択専用で、選択集合を書き換える
/// だけ（Document は変更しない）。移動は `M`、複製は `Ctrl+D` の2クリック配置で行う。
/// 削除の確定コマンドは [`Document::apply`] で適用する（`Batch` の原子性・undo/redo 結線は
/// コア側に従う）。
///
/// - `Delete`/`Backspace`: 選択エンティティを 1 バッチで削除。適用成功時のみ選択を解除する。
///   **右パネルのテキスト入力欄（寸法パネルの数値欄等）にフォーカスがある間は無効**
///   （`text_focused`。Codex adversarial review 2026-09-04 差し戻し対応C。これが無いと
///   数値欄で文字を消そうと Backspace を押しただけで選択中の寸法が削除されてしまう）。
/// - `M`: 選択集合の移動配置モードへ入る（基準点→配置先の2クリック、スナップ対応）。
/// - `Esc`: 進行中のドラッグ（または配置モード）を破棄（選択は変えない）。こちらもテキスト
///   入力欄フォーカス中は無効にし、欄側のフォーカス解除に譲る
///   （[`handle_offset_input`] の `typing` ガードと同じ方針）。
/// - `Space` 押下中の左ドラッグはパン用なので、選択操作としては扱わない。
///
/// # 配置モード（Ctrl+D 複製・M 移動）の優先
///
/// 配置モードがアクティブ（[`SelectTool::is_placing`]）な間は、通常のクリック選択・
/// 矩形選択・削除を一切行わず、基準点→配置先の2クリックだけを受け取る（入力ゲート）。
/// 作図ツールと同様にどちらのクリックにもスナップを効かせ、`snap_marker` を更新する。
/// 配置モードでないときは、選択・編集はスナップ対象外なのでマーカーを消す
/// （設計判断は [`handle_tool_input`] の doc を参照）。
#[allow(clippy::too_many_arguments)]
fn handle_select_input(
    ui: &egui::Ui,
    response: &egui::Response,
    rect: Rect,
    viewport: &Viewport,
    document: &mut Document,
    select_tool: &mut SelectTool,
    snap_enabled: bool,
    snap_marker: &mut Option<snap::SnapResult>,
    status: &mut Option<StatusMessage>,
    now: f64,
    offset_distance: Option<f64>,
    text_focused: bool,
    paper_display: bool,
    dim_scale_k: f64,
) {
    // オフセットモード中は専用経路が入力を占有する（配置モードと同じ入力ゲート思想）。
    if select_tool.is_offsetting() {
        handle_offset_input(
            ui,
            response,
            rect,
            viewport,
            document,
            select_tool,
            snap_enabled,
            snap_marker,
            status,
            now,
            offset_distance,
        );
        return;
    }

    // 配置モード中は専用経路が入力を占有し、通常の選択・編集入力へは進ませない。
    if select_tool.is_placing() {
        handle_placement_input(
            ui,
            response,
            rect,
            viewport,
            document,
            select_tool,
            snap_enabled,
            snap_marker,
            status,
            now,
        );
        return;
    }

    // 配置モードでない選択・編集入力はスナップを効かせない。マーカーを消す。
    *snap_marker = None;

    // M（Ctrl/Shift なし）: 選択集合の移動配置モードへ入る（Ctrl+D 複製と同じ2クリック配置）。
    // Shift+M は鏡映に使うため `!shift` ガードを付け、Shift+M で移動が誤起動しないようにする
    // （設計判断4）。選択が空なら案内メッセージを出すだけ。以降のクリックは次フレームから
    // 配置経路が受け取る。
    if ui.input(|i| !i.modifiers.command && !i.modifiers.shift && i.key_pressed(Key::M)) {
        if select_tool.start_move() {
            set_status(status, now, "Move: click base point");
        } else {
            set_status(status, now, "Select entities to move");
        }
        return;
    }

    // Shift+M（Ctrl なし）: 選択集合の鏡映モードへ入る（軸点A→軸点B の2クリック指定）。
    if ui.input(|i| !i.modifiers.command && i.modifiers.shift && i.key_pressed(Key::M)) {
        if select_tool.start_mirror() {
            set_status(status, now, "Mirror: click first axis point");
        } else {
            set_status(status, now, "Select entities to mirror");
        }
        return;
    }

    // R（Ctrl なし）: 選択集合の回転モードへ入る（pivot→基準点→回転先の3クリック相対角）。
    if ui.input(|i| !i.modifiers.command && i.key_pressed(Key::R)) {
        if select_tool.start_rotate() {
            set_status(status, now, "Rotate: click pivot point");
        } else {
            set_status(status, now, "Select entities to rotate");
        }
        return;
    }

    // O（Ctrl なし）: オフセットモードへ入る（単一エンティティ限定、設計判断5）。
    // Ctrl+O（開く）・Ctrl+Shift+O（DXF インポート）は上部のショートカット処理が先に
    // 消費するので、ここへ来る `O` は素の押下のみ。以降のクリックは次フレームから
    // オフセット経路が受け取る。
    if ui.input(|i| !i.modifiers.command && i.key_pressed(Key::O)) {
        // Text・寸法はオフセット対象外（DESIGN.md M6 設計判断1）。pick() の汎用化で Text も
        // 選択できるようになったため、単一 Text 選択で O を押してもモードに入らないよう
        // ここで明示的に拒否する（`offset_click` 側の拒否と二重の防御）。
        let sel = select_tool.selection();
        let unsupported_single = sel.len() == 1
            && document
                .entity(sel[0])
                .is_some_and(|e| e.geom.as_shape().is_none());
        if unsupported_single {
            set_status(
                status,
                now,
                "Offset supports lines, circles, arcs, and polylines only",
            );
        } else if select_tool.start_offset() {
            set_status(
                status,
                now,
                "Offset: click through point (or type distance)",
            );
        } else {
            set_status(status, now, "Select exactly one entity to offset");
        }
        return;
    }

    // ピック許容量（px）をワールド単位へ換算する。
    let tol = PICK_TOLERANCE_PX / viewport.zoom;

    // Delete / Backspace: 選択を 1 バッチで削除。ロックレイヤー混在時は Batch 原子性で
    // 全体失敗しうるので、apply が成功したときだけ選択を解除し、失敗は表示する。
    // テキスト入力欄にフォーカスがある間は無効（差し戻し対応C。欄の編集キーがそのまま
    // ドキュメント操作へ漏れないようにする）。
    let canvas_key_shortcuts_enabled = select_canvas_key_shortcuts_enabled(text_focused);
    if canvas_key_shortcuts_enabled
        && ui.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace))
        && let Some(cmd) = select_tool.delete_command()
    {
        match document.apply(cmd) {
            Ok(_) => select_tool.clear_selection(),
            Err(err) => set_status(status, now, format!("Delete failed: {err}")),
        }
    }

    // Esc: 進行中のドラッグがあればそれだけ破棄（選択維持）、無ければ選択を全解除する
    // （2段階挙動は SelectTool::on_cancel 側に集約）。テキスト入力欄フォーカス中は
    // 欄側のフォーカス解除に譲る（[`handle_offset_input`] と同じ方針）。
    if canvas_key_shortcuts_enabled && ui.input(|i| i.key_pressed(Key::Escape)) {
        select_tool.on_cancel();
    }

    // Space 押下中の左ドラッグはパン。選択操作とは扱わない。
    if ui.input(|i| i.key_down(Key::Space)) {
        return;
    }

    let world_at = |pos| viewport.screen_to_world(rect, pos);
    let pointer = response.interact_pointer_pos();
    if let Some(pos) = pointer {
        let world = world_at(pos);
        if response.drag_started_by(egui::PointerButton::Primary) {
            // ドラッグ開始点が、選択済み寸法の文字ブロックに入っていれば文字ドラッグへ
            // 分岐する（M9 タスク51、設計判断7）。それ以外は従来どおり矩形選択。
            let render = dim_render(
                document.dim_style(),
                paper_display,
                dim_scale_k,
                viewport.zoom,
            );
            match dim_label_hit(document, select_tool.selection(), world, render) {
                Some((id, label_center)) => {
                    select_tool.start_text_drag(id, label_center, world);
                }
                None => select_tool.on_drag_start(world),
            }
        } else if response.dragged_by(egui::PointerButton::Primary) {
            select_tool.on_drag(world);
        } else if response.drag_stopped_by(egui::PointerButton::Primary) {
            if select_tool.is_text_dragging() {
                // 文字ドラッグの確定（M9 タスク51）。移動量が tol 未満なら
                // `end_text_drag` が `None` を返し、履歴を汚さない。
                if let Some(cmd) = select_tool.end_text_drag(document, world, tol)
                    && let Err(err) = document.apply(cmd)
                {
                    set_status(status, now, format!("文字位置の変更に失敗しました: {err}"));
                }
            } else {
                // ドラッグは矩形選択専用。選択集合を書き換えるだけで Document は変更しない。
                select_tool.on_drag_end(document, world);
            }
        } else if response.clicked_by(egui::PointerButton::Primary) {
            let shift = ui.input(|i| i.modifiers.shift);
            select_tool.on_click(document, world, tol, shift);
        }
    }
}

/// 配置モード（Ctrl+D 複製・M 移動の「基準点→配置先」2クリック）のキャンバス入力を処理する。
///
/// [`handle_select_input`] が配置モード中のみ呼ぶ。通常の選択・矩形選択・削除とは
/// 排他（入力ゲート済み）。両クリックにスナップを効かせ、`snap_marker` を更新する。
///
/// - `Esc`: 配置モードを解除（Document は変更しない）。
/// - カーソル移動: プレビュー追従とスナップマーカー更新。
/// - `Space` 押下中の左ドラッグ: パン用なので配置クリックとしては扱わない。
/// - 単発クリック: 1発目=基準点、2発目=配置先。確定コマンドは `Document::apply` し、
///   種別に応じた後処理を行う（複製は `NewIds.entities` を新選択にして "Duplicated N"、
///   移動は選択維持で "Moved N"）。失敗（レイヤーロック等）は Batch 原子性で全体が失敗し、
///   ステータスバーへ表示する。
#[allow(clippy::too_many_arguments)]
fn handle_placement_input(
    ui: &egui::Ui,
    response: &egui::Response,
    rect: Rect,
    viewport: &Viewport,
    document: &mut Document,
    select_tool: &mut SelectTool,
    snap_enabled: bool,
    snap_marker: &mut Option<snap::SnapResult>,
    status: &mut Option<StatusMessage>,
    now: f64,
) {
    // Esc: 配置モードを解除する（Document は変更しない）。
    if ui.input(|i| i.key_pressed(Key::Escape)) {
        select_tool.cancel_placement();
        *snap_marker = None;
        return;
    }

    // スナップ用パラメータ（作図ツールと同じ換算）。
    let radius = SNAP_RADIUS_PX / viewport.zoom;
    let grid_step = viewport::nice_grid_step(viewport.zoom, GRID_TARGET_PX);
    // 確定判定のゼロ変位しきい値はピック許容量基準。
    let tol = PICK_TOLERANCE_PX / viewport.zoom;

    // カーソル追従（プレビュー用）とスナップマーカー更新。
    if let Some(pos) = response.hover_pos() {
        let raw = viewport.screen_to_world(rect, pos);
        let (world, marker) = apply_snap(document, snap_enabled, raw, radius, grid_step, &[]);
        *snap_marker = marker;
        select_tool.placement_move(world);
    } else {
        *snap_marker = None;
    }

    // Space 押下中の左ドラッグはパン。配置クリックとは扱わない。
    if ui.input(|i| i.key_down(Key::Space)) {
        return;
    }

    // 単発クリックで基準点／配置先を確定する（ドラッグではない）。
    if response.clicked_by(egui::PointerButton::Primary)
        && let Some(pos) = response.interact_pointer_pos()
    {
        let raw = viewport.screen_to_world(rect, pos);
        let (world, _) = apply_snap(document, snap_enabled, raw, radius, grid_step, &[]);
        match select_tool.placement_click(document, world, tol) {
            PlacementOutcome::Continue => {}
            PlacementOutcome::Cancelled(msg) => {
                *snap_marker = None;
                set_status(status, now, msg);
            }
            PlacementOutcome::Commit { kind, cmd } => {
                *snap_marker = None;
                // ロックレイヤー混在時は Batch 原子性で全体が失敗し、位置も選択も変わらない。
                match document.apply(cmd) {
                    Ok(new_ids) => match kind {
                        // 複製: 新しい ID 群（コマンド順）を選択にして件数を表示する。
                        PlacementKind::Duplicate => {
                            let n = new_ids.entities.len();
                            select_tool.set_selection(new_ids.entities);
                            set_status(status, now, format!("Duplicated {n} entities"));
                        }
                        // 移動・回転・鏡映: ID は不変なので選択はそのまま維持する。
                        PlacementKind::Move => {
                            let n = select_tool.selection().len();
                            set_status(status, now, format!("Moved {n} entities"));
                        }
                        PlacementKind::Rotate => {
                            let n = select_tool.selection().len();
                            set_status(status, now, format!("Rotated {n} entities"));
                        }
                        PlacementKind::Mirror => {
                            let n = select_tool.selection().len();
                            set_status(status, now, format!("Mirrored {n} entities"));
                        }
                    },
                    Err(err) => match kind {
                        PlacementKind::Duplicate => {
                            set_status(status, now, format!("Duplicate failed: {err}"));
                        }
                        PlacementKind::Move => {
                            set_status(status, now, format!("Move failed: {err}"));
                        }
                        PlacementKind::Rotate => {
                            set_status(status, now, format!("Rotate failed: {err}"));
                        }
                        PlacementKind::Mirror => {
                            set_status(status, now, format!("Mirror failed: {err}"));
                        }
                    },
                }
            }
        }
    }
}

/// オフセット距離入力欄の文字列を解析する。**正の有限値のみ** `Some` を返す。
///
/// 空・0・負・非数は `None`（呼び出し側は通過点方式のフォールバックとして扱う。
/// 設計判断5: 「欄が空・0・非数なら通過点方式へフォールバックする」）。
fn parse_offset_distance(text: &str) -> Option<f64> {
    parse_positive_length(text)
}

/// Text ツールの高さ入力欄（ワールド単位）を解析する。**正の有限値のみ** `Some`。
/// 空・0・負・非数は `None`（確定は拒否し、プレビューは描かない）。
fn parse_text_height(text: &str) -> Option<f64> {
    parse_positive_length(text)
}

/// フィレット半径入力欄を解析する。**正の有限値のみ** `Some`（M7 設計判断6）。
/// 空・0・負・非数は `None` で、[`FilletTool`] が 2 本目のピック時に
/// 「Fillet: enter a radius」として拒否する（半径にはフォールバックが無い）。
fn parse_fillet_radius(text: &str) -> Option<f64> {
    parse_positive_length(text)
}

/// 長さ系の数値入力欄（オフセット距離・テキスト高さ・フィレット半径）に共通の解析。
/// 前後の空白を無視し、**正の有限値のみ** `Some` を返す。3 欄とも「意味を持つのは正の
/// 有限値だけ」という点で規則が同じなので、実体をここへ集約し、欄ごとの薄いラッパで
/// 用途と `None` の扱いを doc に残す。
fn parse_positive_length(text: &str) -> Option<f64> {
    let value: f64 = text.trim().parse().ok()?;
    (value.is_finite() && value > 0.0).then_some(value)
}

/// オフセットモード（`O`、単一エンティティ1クリック。設計判断5）のキャンバス入力を処理する。
///
/// [`handle_select_input`] がオフセットモード中のみ呼ぶ。通常の選択・矩形選択・削除とは
/// 排他（入力ゲート済み）。クリックにスナップを効かせ、`snap_marker` を更新する。
///
/// - `Esc`: オフセットモードを解除（Document は変更しない）。ただし距離入力欄を編集中
///   （`wants_keyboard_input`）は欄側のフォーカス解除に譲り、モードは畳まない。
/// - カーソル移動: プレビュー追従（ゴースト）とスナップマーカー更新。
/// - `Space` 押下中の左ドラッグ: パン用なのでオフセットクリックとしては扱わない。
/// - 単発クリック: [`SelectTool::offset_click`] で確定／キャンセル。確定コマンドは
///   `Document::apply`（元は不変・結果を `AddEntity`。レイヤーロック時はステータス表示）。
#[allow(clippy::too_many_arguments)]
fn handle_offset_input(
    ui: &egui::Ui,
    response: &egui::Response,
    rect: Rect,
    viewport: &Viewport,
    document: &mut Document,
    select_tool: &mut SelectTool,
    snap_enabled: bool,
    snap_marker: &mut Option<snap::SnapResult>,
    status: &mut Option<StatusMessage>,
    now: f64,
    offset_distance: Option<f64>,
) {
    // 距離入力欄を編集中はキーボードを欄が占有しているとみなし、Esc はモード解除ではなく
    // 欄のフォーカス解除に譲る（マウスのクリック確定・プレビューは以降そのまま処理する）。
    // egui のキーボードフォーカス（TextEdit にフォーカスがあれば `Some`）で判定する。
    let typing = ui.memory(|m| m.focused().is_some());

    // Esc: オフセットモードを解除する（Document は変更しない）。
    if !typing && ui.input(|i| i.key_pressed(Key::Escape)) {
        select_tool.cancel_offset();
        select_tool.cancel_text_drag();
        *snap_marker = None;
        return;
    }

    // スナップ用パラメータ（作図・配置ツールと同じ換算）。
    let radius = SNAP_RADIUS_PX / viewport.zoom;
    let grid_step = viewport::nice_grid_step(viewport.zoom, GRID_TARGET_PX);
    // 通過点方式で「通過点が対象上」を判定するゼロ距離しきい値はピック許容量基準。
    let tol = PICK_TOLERANCE_PX / viewport.zoom;

    // カーソル追従（プレビュー用ゴースト）とスナップマーカー更新。
    if let Some(pos) = response.hover_pos() {
        let raw = viewport.screen_to_world(rect, pos);
        let (world, marker) = apply_snap(document, snap_enabled, raw, radius, grid_step, &[]);
        *snap_marker = marker;
        select_tool.offset_move(world);
    } else {
        *snap_marker = None;
    }

    // Space 押下中の左ドラッグはパン。オフセットクリックとは扱わない。
    if ui.input(|i| i.key_down(Key::Space)) {
        return;
    }

    // 単発クリックで確定する（通過点／側の指定）。
    if response.clicked_by(egui::PointerButton::Primary)
        && let Some(pos) = response.interact_pointer_pos()
    {
        let raw = viewport.screen_to_world(rect, pos);
        let (world, _) = apply_snap(document, snap_enabled, raw, radius, grid_step, &[]);
        match select_tool.offset_click(document, world, tol, offset_distance) {
            OffsetOutcome::Cancelled(msg) => {
                *snap_marker = None;
                set_status(status, now, msg);
            }
            OffsetOutcome::Commit(cmd) => {
                *snap_marker = None;
                // 元エンティティは不変。結果を AddEntity で追加する（ロック時は失敗を表示）。
                match document.apply(cmd) {
                    Ok(_) => set_status(status, now, "Offset created"),
                    Err(err) => set_status(status, now, format!("Offset failed: {err}")),
                }
            }
        }
    }
}

/// 中ボタンドラッグ、または Space キー押下中の左ドラッグでパンする。
fn handle_pan_input(ui: &egui::Ui, response: &egui::Response, viewport: &mut Viewport) {
    let space_down = ui.input(|i| i.key_down(Key::Space));
    let panning = response.dragged_by(egui::PointerButton::Middle)
        || (space_down && response.dragged_by(egui::PointerButton::Primary));
    if panning {
        viewport.pan_by_screen_delta(response.drag_delta());
    }
}

/// ホイールスクロールでカーソル位置中心にズームする。
fn handle_zoom_input(
    ui: &egui::Ui,
    response: &egui::Response,
    rect: Rect,
    viewport: &mut Viewport,
) {
    if !response.hovered() {
        return;
    }
    let Some(cursor) = response.hover_pos() else {
        return;
    };
    let scroll_y = f64::from(ui.input(|i| i.smooth_scroll_delta.y));
    if scroll_y == 0.0 {
        return;
    }
    let zoom_factor = (scroll_y * WHEEL_ZOOM_SPEED).exp();
    viewport.zoom_at(rect, cursor, zoom_factor);
}

/// Alt(+Shift)修飾キー+マウス移動によるジェスチャの種類
/// （DESIGN.md 7章「ズーム・パンの追加操作手段」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewGesture {
    /// Alt のみ: 水平移動でズーム（[`Viewport::zoom_by_horizontal_motion`]）。
    Zoom,
    /// Shift+Alt: 2軸移動でパン（既存 [`Viewport::pan_by_screen_delta`]）。
    Pan,
}

/// Alt(+Shift)修飾キージェスチャの有効/種類判定を行う純関数
/// （DESIGN.md 設計判断(a)(d)）。
///
/// 有効化の必要条件は「キャンバスが hovered かつポインタボタンがどれも押されていない
/// （`any_down` が false）かつ command 非押下」。これにより矩形選択・中ボタン/Space
/// パン・作図ドラッグ等の既存ドラッグとは構造的に相互作用しない（ボタン押下中は Alt が
/// 完全に不活性）。command 除外は AltGr が Ctrl+Alt として報告される系（Windows）を弾く。
/// 必要条件を満たした上で `alt` が立っていなければ `None`、`alt && !shift` で
/// [`ViewGesture::Zoom`]、`alt && shift` で [`ViewGesture::Pan`] を返す。
fn modifier_view_gesture(
    alt: bool,
    shift: bool,
    command: bool,
    any_down: bool,
    hovered: bool,
) -> Option<ViewGesture> {
    if !hovered || any_down || command || !alt {
        return None;
    }
    if shift {
        Some(ViewGesture::Pan)
    } else {
        Some(ViewGesture::Zoom)
    }
}

/// Alt(+Shift)+マウス移動によるズーム/パン（DESIGN.md 設計判断(a)〜(d)）。
///
/// 毎フレーム [`modifier_view_gesture`] を評価する。`None` または種類が異なる状態から
/// `Some` へ遷移した瞬間（ジェスチャ開始、またはジェスチャ中の Shift 切替による
/// ズーム↔パン移行）に現在の hover 位置を `anchor` へ記録し、以後ジェスチャが継続する
/// 限り固定する。ズーム中は `pointer.delta().x` を
/// [`Viewport::zoom_by_horizontal_motion`] へ、パン中は `pointer.delta()`（2軸）を
/// 既存 [`Viewport::pan_by_screen_delta`] へそのまま渡す。条件を満たさなくなったら
/// `anchor` を破棄する。
///
/// 呼び出し位置は `handle_pan_input`/`handle_zoom_input` の隣（モーダルゲートの外）。
/// 既存のホイールズーム・中ボタン/Space パン等とは `modifier_view_gesture` の
/// `any_down` 条件により排他的なので、既存挙動には触れない。
fn handle_modifier_view_input(
    ui: &egui::Ui,
    response: &egui::Response,
    rect: Rect,
    viewport: &mut Viewport,
    anchor: &mut Option<Pos2>,
    active_gesture: &mut Option<ViewGesture>,
) {
    let hovered = response.hovered();
    let (alt, shift, command, any_down, delta) = ui.input(|i| {
        (
            i.modifiers.alt,
            i.modifiers.shift,
            i.modifiers.command,
            i.pointer.any_down(),
            i.pointer.delta(),
        )
    });
    let gesture = modifier_view_gesture(alt, shift, command, any_down, hovered);

    // ジェスチャの開始（直前フレームが None）、またはジェスチャ中の Shift 切替
    // （直前フレームと種類が異なる）の両方でアンカーを再取得する。
    if let Some(current) = gesture
        && *active_gesture != Some(current)
    {
        *anchor = response.hover_pos();
    }

    match gesture {
        Some(ViewGesture::Zoom) => {
            if let Some(cursor) = *anchor {
                viewport.zoom_by_horizontal_motion(rect, cursor, delta.x);
            }
        }
        Some(ViewGesture::Pan) => {
            if anchor.is_some() {
                viewport.pan_by_screen_delta(delta);
            }
        }
        None => {
            *anchor = None;
        }
    }
    *active_gesture = gesture;
}

/// ズームレベルに応じて間引いたグリッド線を描画する。
///
/// 副グリッド（`nice_grid_step` が返す基本間隔）と、その5倍の主グリッドの2段。
/// 副グリッドの画面間隔が狭すぎる（読み取れない）場合は副グリッドを省略する。
fn draw_grid(painter: &egui::Painter, rect: Rect, viewport: &Viewport) {
    let minor_step = viewport::nice_grid_step(viewport.zoom, GRID_TARGET_PX);
    let minor_px = minor_step * viewport.zoom;
    let major_step = minor_step * 5.0;

    let minor_stroke = Stroke::new(1.0, Color32::from_gray(55));
    let major_stroke = Stroke::new(1.0, Color32::from_gray(80));

    let visible = viewport.visible_aabb(rect);

    // 副グリッドは画面間隔が十分（>= 6px）ある時だけ描く。
    if minor_px >= 6.0 {
        draw_grid_lines(painter, rect, viewport, &visible, minor_step, minor_stroke);
    }
    draw_grid_lines(painter, rect, viewport, &visible, major_step, major_stroke);
}

/// 間隔 `step`（ワールド単位）でグリッド線を1系統描画する。
fn draw_grid_lines(
    painter: &egui::Painter,
    rect: Rect,
    viewport: &Viewport,
    visible: &Aabb,
    step: f64,
    stroke: Stroke,
) {
    if step <= 0.0 || !step.is_finite() {
        return;
    }
    let x_start = (visible.min.x / step).floor() * step;
    let mut x = x_start;
    while x <= visible.max.x {
        let sx = viewport.world_to_screen(rect, Point2::new(x, 0.0)).x;
        painter.vline(sx, rect.y_range(), stroke);
        x += step;
    }

    let y_start = (visible.min.y / step).floor() * step;
    let mut y = y_start;
    while y <= visible.max.y {
        let sy = viewport.world_to_screen(rect, Point2::new(0.0, y)).y;
        painter.hline(rect.x_range(), sy, stroke);
        y += step;
    }
}

/// 図面枠（輪郭・表題欄の罫線・欄文字）を描く（DESIGN.md M8 設計判断3。タスク38）。
///
/// `frame_layout` が生成する紙 mm 座標を [`paper_to_world`] でワールド座標へ変換して
/// から画面へ写す。**枠は「グリッドと同格」の派生描画**であり `Document::entities()`
/// を経由しないため、選択・ピック・スナップの対象にならない。
///
/// 枠線の線幅は [`resolve_stroke_px_with_toggle`] に従う（F9 OFF = 1px 固定、
/// ON = 紙 mm 比例。判断4/5 の分岐を1関数へ集約する方針を踏襲）。欄文字の高さは
/// トグル非依存で常に紙 mm × `k`（Text エンティティと同根の理由。判断4 実装時追記の
/// (c) 参照）。線分・文字とも高々数十件なのでカリングは行わない。
fn draw_frame(
    painter: &egui::Painter,
    rect: Rect,
    viewport: &Viewport,
    sheet: &SheetMeta,
    paper_display_enabled: bool,
    k: f64,
) {
    let layout = frame_layout(sheet);
    for line in &layout.lines {
        let a = viewport.world_to_screen(rect, paper_to_world(line.a, k));
        let b = viewport.world_to_screen(rect, paper_to_world(line.b, k));
        let stroke_px =
            resolve_stroke_px_with_toggle(paper_display_enabled, line.width_mm, k, viewport.zoom);
        painter.line_segment([a, b], Stroke::new(stroke_px, FRAME_COLOR));
    }
    for text in &layout.texts {
        let anchor = paper_to_world(text.anchor_mm, k);
        // セル幅を超える長い文字列はクリップせずはみ出したまま描く（判断7と同じ流儀。
        // M8 ではクリップ機構を持たない）。`draw_text` はそのまま呼べば足りる。
        let geom = TextGeom {
            anchor,
            content: text.content.clone(),
            height: text.height_mm,
            angle: 0.0,
        };
        draw_text(
            painter,
            rect,
            viewport,
            &geom,
            text.height_mm * k,
            FRAME_COLOR,
        );
    }
}

/// 用紙縁（用紙の外形矩形）を描く。**印刷対象ではない画面専用ヒント**
/// （[`frame::FrameLayout`] には含めない。タスク39 では紙自体がページなので
/// 縁線は不要になる、という区別を今から構造に入れる）。グリッドと同じ流儀の
/// 固定 1px グレー線で描く（[`resolve_stroke_px_with_toggle`] を介さない）。
fn draw_paper_edge(painter: &egui::Painter, rect: Rect, viewport: &Viewport, sheet: &SheetMeta) {
    let (w, h) = sheet.paper_extent_mm();
    let k = sheet.scale.world_mm_per_paper_mm();
    let min = viewport.world_to_screen(rect, paper_to_world(Point2::new(0.0, 0.0), k));
    let max = viewport.world_to_screen(rect, paper_to_world(Point2::new(w, h), k));
    // world_to_screen は y 軸を反転するので、min/max の大小関係は画面座標では
    // 入れ替わりうる。4辺を個別に線分として描けば向きを気にする必要がない。
    let corners = [
        Pos2::new(min.x, min.y),
        Pos2::new(max.x, min.y),
        Pos2::new(max.x, max.y),
        Pos2::new(min.x, max.y),
    ];
    let stroke = Stroke::new(1.0, PAPER_EDGE_COLOR);
    for i in 0..4 {
        painter.line_segment([corners[i], corners[(i + 1) % 4]], stroke);
    }
}

/// ビューポートの可視 AABB と交差し、かつ表示レイヤーに属するエンティティを
/// **描画順（奥→手前）** に並べて返す。
///
/// 順序の規則:
/// - レイヤー間はレイヤーの重ね順（[`Layer::order`] 昇順。大きいほど手前）。
/// - 同一レイヤー内（および同順位レイヤーの間）は [`Document::entities()`] の反復順
///   （＝エンティティの追加順）のまま。[`Vec::sort_by_key`] が安定ソートであることに
///   依存している。
///
/// 性能: 並べ替えるのはカリング後の可視エンティティの件数のみで、全エンティティは
/// ソートしない（`O(n log n)`、`n` は可視件数）。将来 `n` が数万規模になったら
/// 「重ね順やエンティティ集合が変わったときだけ再計算するキャッシュ」が対策になるが、
/// 現状の作図規模ではフレーム時間に測れる影響がないため先回りしない。
///
/// `k` は紙 1mm あたりのワールド mm（判断4）。Text のカリング判定は表示上のワールド
/// AABB（[`text_world_aabb`]、`height * k`）で行う（タスク37。判断(c)により Text は
/// トグル非依存で常に `height * k` を使う）。
fn entities_in_draw_order<'a>(
    document: &'a Document,
    visible: &Aabb,
    k: f64,
) -> Vec<(EntityId, &'a Entity, &'a Layer)> {
    let entity_aabb = |entity: &Entity| match &entity.geom {
        EntityGeom::Text(text) => text_world_aabb(text, k),
        _ => entity.geom.aabb(),
    };
    let mut drawable: Vec<(EntityId, &Entity, &Layer)> = document
        .entities()
        .filter_map(|(id, entity)| {
            let layer = document.layer(entity.layer)?;
            (layer.visible && entity_aabb(entity).intersects(visible))
                .then_some((id, entity, layer))
        })
        .collect();
    drawable.sort_by_key(|(_, _, layer)| layer.order);
    drawable
}

/// ビューポートの可視 AABB と交差するエンティティのみをカリングして描画する。
/// 非表示レイヤーのエンティティは描画しない。
///
/// 描画順は [`entities_in_draw_order`]（レイヤーの重ね順、同一レイヤー内は追加順）。
/// 選択ハイライト・ツールのプレビュー・スナップマーカーはこの関数より後に描くため、
/// 常にエンティティ本体より手前に出る（呼び出し側 [`McadApp::ui`] の描画順を参照）。
fn draw_entities(
    painter: &egui::Painter,
    rect: Rect,
    document: &Document,
    viewport: &Viewport,
    paper_display_enabled: bool,
) {
    let visible = viewport.visible_aabb(rect);
    let k = document.sheet().scale.world_mm_per_paper_mm();
    let render = dim_render(
        document.dim_style(),
        paper_display_enabled,
        k,
        viewport.zoom,
    );
    for (_id, entity, layer) in entities_in_draw_order(document, &visible, k) {
        let color = to_color32(entity.style.effective_color(layer.color));
        let width_mm = entity.style.effective_width(layer.width_mm).mm();
        let stroke_px =
            resolve_stroke_px_with_toggle(paper_display_enabled, width_mm, k, viewport.zoom);
        let linetype = entity.style.effective_linetype(layer.linetype);
        match &entity.geom {
            EntityGeom::Shape(shape) => {
                let stroke = Stroke::new(stroke_px, color);
                draw_shape(painter, rect, viewport, shape, stroke, linetype, k);
            }
            // Text はトグル非依存で常に `height * k`（判断(c)、二重換算防止は
            // `draw_text` のワールド高さ明示引数化で担保する）。
            EntityGeom::Text(text) => {
                draw_text(painter, rect, viewport, text, text.height * k, color)
            }
            EntityGeom::DimLinear(dim) => {
                // 寸法は製図慣行として常に実線で描く（線種は形状エンティティのみが
                // 対象。DESIGN.md M8 タスク36 は `draw_shape` が扱う形状に限定）。
                // 線幅の紙 mm 解決はここでも同じ式を適用し、既定 0.35mm 相当で
                // 従来と同じ見た目を保つ。
                let stroke = Stroke::new(stroke_px, color);
                draw_dim_linear(painter, rect, viewport, dim, stroke, render);
            }
            EntityGeom::DimRadial(dim) => {
                let stroke = Stroke::new(stroke_px, color);
                draw_dim_radial(painter, rect, viewport, dim, stroke, render);
            }
            EntityGeom::DimDiameter(dim) => {
                let stroke = Stroke::new(stroke_px, color);
                draw_dim_diameter(painter, rect, viewport, dim, stroke, render);
            }
            // `EntityGeom` は `#[non_exhaustive]`。未知の幾何は描かない。
            _ => {}
        }
    }
}

/// 寸法の矢先の長さ・文字高さをワールド長で解決する（戻り値: `(arrow_len, text_height)`）。
///
/// 紙基準表示 ON（`paper_display`）: 文書スタイルの紙 mm
/// （[`DimStyle::arrow_len_mm`]/[`DimStyle::text_height_mm`]）× `k`
/// （ズーム非依存 → 図形と一緒に拡縮し、タスク39/40 の SVG/PDF 出力と一致する）。
/// OFF: 画面固定 px（[`DIM_ARROW_PX`]/[`DIM_TEXT_PX`]）÷ `zoom`（タスク36b までの現行の
/// 見た目。スタイルの影響を受けない画面専用モード）。ON モードの注記サイズに px 下限
/// クランプは設けない（[`draw_text`] 既存の [`MIN_TEXT_PX`] 未満スキップに任せる）。
///
/// # 紙 mm の出所を [`DimStyle`] へ一本化してある（M9 タスク49-3）
///
/// タスク37〜39 はここで `plot::DIM_ARROW_MM` / `plot::DIM_TEXT_MM` という定数を使って
/// いたが、M9 タスク49-2 で矢の内外判定（`dimension::arrows_point_outward`）と注記の
/// 表示倍率（`dimension::DimRender::annotation_scale`）が [`DimStyle`] を読むように
/// なったため、「実際に描かれる大きさは定数・判定と組版の比率はスタイル」という
/// 二重の出所になっていた。既定値が一致していたので差は出ていなかったが、スタイル編集
/// UI（M9 タスク50）で文字高さを変えた瞬間に両者が食い違う。ここをスタイル読みへ
/// 揃えることで、画面・SVG・PDF の 3 経路が同じ 1 つの値から大きさを得る。
fn dim_sizes(style: &DimStyle, paper_display: bool, k: f64, zoom: f64) -> (f64, f64) {
    if paper_display {
        (style.arrow_len_mm * k, style.text_height_mm * k)
    } else {
        (DIM_ARROW_PX / zoom, DIM_TEXT_PX / zoom)
    }
}

/// 寸法展開のパラメータ（[`dimension::DimRender`]）を組み立てる。
///
/// 表示モードの解決（[`dim_sizes`]）はここで済ませ、`dimension` モジュールへは
/// **ワールド長になった値だけ**を渡す。文書尺度 `k` は矢の内外判定を紙 mm で行うために
/// 別枠で渡す（[`dimension::DimRender`] の doc: 2 つの換算係数を混同しないこと）。
fn dim_render(
    style: &DimStyle,
    paper_display: bool,
    k: f64,
    zoom: f64,
) -> dimension::DimRender<'_> {
    let (arrow_len_world, text_height_world) = dim_sizes(style, paper_display, k, zoom);
    dimension::DimRender {
        style,
        scale_world_per_paper_mm: k,
        arrow_len_world,
        text_height_world,
    }
}

/// `world` が、**選択集合に含まれる**寸法いずれかの文字ブロック（表示サイズ依存の
/// `label_box`）に入っていれば、その `(EntityId, 現在のラベル中心)` を返す（M9 タスク51）。
///
/// 選択集合だけを対象にするのは設計判断7どおり: 「選択済み寸法の文字ブロックドラッグ」
/// という別経路であり、未選択の寸法をクリックしただけで文字ドラッグへ入ってはいけない
/// （通常のクリック選択・矩形選択の意味を壊さないため）。複数の寸法の label_box が
/// 重なって該当する場合は選択順の先頭を返す（決定的だが優先順位に強い意味はない）。
fn dim_label_hit(
    document: &Document,
    selection: &[EntityId],
    world: Point2,
    render: dimension::DimRender<'_>,
) -> Option<(EntityId, Point2)> {
    for &id in selection {
        let Some(entity) = document.entity(id) else {
            continue;
        };
        if !layer_visible(document, entity) {
            // 非表示レイヤーの寸法は当たり判定の対象外（描画・通常ピックと同じ扱い）。
            continue;
        }
        let ex = match &entity.geom {
            EntityGeom::DimLinear(dim) => dimension::expand_linear(dim, render),
            EntityGeom::DimRadial(dim) => dimension::expand_radial(dim, render),
            EntityGeom::DimDiameter(dim) => dimension::expand_diameter(dim, render),
            _ => continue,
        };
        if let Some(quad) = ex.label_box
            && dimension::label_box_contains(&quad, world)
        {
            return Some((id, dimension::label_box_center(&quad)));
        }
    }
    None
}

/// 長さ寸法を描画する（純関数 helper [`dimension::expand_linear`] の展開を Painter へ）。
fn draw_dim_linear(
    painter: &egui::Painter,
    rect: Rect,
    viewport: &Viewport,
    dim: &DimLinear,
    stroke: Stroke,
    render: dimension::DimRender<'_>,
) {
    let ex = dimension::expand_linear(dim, render);
    draw_dim_expansion(painter, rect, viewport, &ex, stroke);
}

/// 半径寸法を描画する（純関数 helper [`dimension::expand_radial`] の展開を Painter へ）。
fn draw_dim_radial(
    painter: &egui::Painter,
    rect: Rect,
    viewport: &Viewport,
    dim: &DimRadial,
    stroke: Stroke,
    render: dimension::DimRender<'_>,
) {
    let ex = dimension::expand_radial(dim, render);
    draw_dim_expansion(painter, rect, viewport, &ex, stroke);
}

/// 直径寸法を描画する（純関数 helper [`dimension::expand_diameter`] の展開を Painter へ）。
fn draw_dim_diameter(
    painter: &egui::Painter,
    rect: Rect,
    viewport: &Viewport,
    dim: &DimDiameter,
    stroke: Stroke,
    render: dimension::DimRender<'_>,
) {
    let ex = dimension::expand_diameter(dim, render);
    draw_dim_expansion(painter, rect, viewport, &ex, stroke);
}

/// 寸法の展開結果（線分・矢先・記号ストローク・文字）を Painter へ描く。プレビュー
/// （`tool.rs` の `draw_preview`）と確定描画・選択ハイライトが共有する
/// （`crate::draw_dim_expansion`）。矢先は `stroke.color` で塗りつぶし、文字は既存の
/// [`draw_text`] を再利用する。
///
/// 各 `TextGeom::height` は `dimension` の展開関数が `dim_sizes` の戻り値（既にワールド長）
/// から組み立てた高さなので、ここでは**そのまま** `draw_text` のワールド高さ引数へ渡す
/// （`k` を掛けると二重換算になる。タスク37 判断(d)）。
///
/// [`dimension::DimExpansion::symbol_strokes`]（φ・□ のストローク）は寸法線とまったく
/// 同じ `stroke` で、既存の [`draw_shape`] へ通して描く（M9 タスク49-3）。
fn draw_dim_expansion(
    painter: &egui::Painter,
    rect: Rect,
    viewport: &Viewport,
    ex: &dimension::DimExpansion,
    stroke: Stroke,
) {
    for seg in &ex.segments {
        let a = viewport.world_to_screen(rect, seg[0]);
        let b = viewport.world_to_screen(rect, seg[1]);
        painter.line_segment([a, b], stroke);
    }
    for tri in &ex.arrows {
        let pts: Vec<Pos2> = tri
            .iter()
            .map(|p| viewport.world_to_screen(rect, *p))
            .collect();
        painter.add(egui::Shape::convex_polygon(pts, stroke.color, Stroke::NONE));
    }
    // 記号（φ・□）は値と同じラベルの一部なので、文字の直前へ置いて描画順を揃える。
    // 寸法は製図慣行として常に実線なので線種は `Continuous` 固定で、`draw_shape` の `k`
    // （破線パターンの紙 mm → px 換算にしか使わない）は使われない。`tool.rs` の
    // プレビュー描画が既に採っている流儀に合わせて 1.0 を渡す。
    for shape in &ex.symbol_strokes {
        draw_shape(
            painter,
            rect,
            viewport,
            shape,
            stroke,
            Linetype::Continuous,
            1.0,
        );
    }
    for text in &ex.texts {
        draw_text(painter, rect, viewport, text, text.height, stroke.color);
    }
}

/// 選択ハイライトと、進行中のプレビュー（矩形選択枠・複製/移動配置の仮表示）を描画する。
///
/// `draw_entities` の後に呼び、選択エンティティを強調色で上書きする（[`draw_shape`] 再利用）。
#[allow(clippy::too_many_arguments)]
fn draw_selection(
    painter: &egui::Painter,
    rect: Rect,
    document: &Document,
    viewport: &Viewport,
    select_tool: &SelectTool,
    offset_distance: Option<f64>,
    paper_display: bool,
    k: f64,
) {
    let highlight = Stroke::new(SELECTION_WIDTH, SELECTION_COLOR);
    let render = dim_render(document.dim_style(), paper_display, k, viewport.zoom);

    // オフセットモード中は、元エンティティを強調表示したまま、確定結果のゴーストを
    // プレビュー色で重ねる（Document は変更しない）。退化して結果が作れないカーソル
    // 位置ではゴーストを描かない（設計判断5）。オフセットは通常の配置・矩形選択とは
    // 排他なので、こちらを最優先で処理する。
    if select_tool.is_offsetting() {
        draw_selected(
            painter,
            rect,
            document,
            viewport,
            select_tool,
            highlight,
            paper_display,
            k,
        );
        if let Some(ghost) = select_tool.offset_preview(document, offset_distance) {
            let preview = Stroke::new(SELECTION_WIDTH, OFFSET_PREVIEW_COLOR);
            draw_shape(
                painter,
                rect,
                viewport,
                &ghost,
                preview,
                Linetype::Continuous,
                1.0,
            );
        }
        return;
    }

    // 選択集合を `transform` で変換した先を強調色で仮表示する（配置先ゴースト）。
    // Text も変換（移動・回転・鏡映・複製）に追従してゴースト表示する。寸法は後続タスク。
    let draw_ghost = |transform: &dyn Fn(&EntityGeom) -> EntityGeom| {
        for &id in select_tool.selection() {
            if let Some(entity) = document.entity(id) {
                match transform(&entity.geom) {
                    EntityGeom::Shape(shape) => {
                        draw_shape(
                            painter,
                            rect,
                            viewport,
                            &shape,
                            highlight,
                            Linetype::Continuous,
                            1.0,
                        );
                    }
                    // Text はトグル非依存で常に `height * k`（判断(c)）。
                    EntityGeom::Text(text) => {
                        draw_text(
                            painter,
                            rect,
                            viewport,
                            &text,
                            text.height * k,
                            highlight.color,
                        );
                    }
                    EntityGeom::DimLinear(dim) => {
                        draw_dim_linear(painter, rect, viewport, &dim, highlight, render);
                    }
                    EntityGeom::DimRadial(dim) => {
                        draw_dim_radial(painter, rect, viewport, &dim, highlight, render);
                    }
                    EntityGeom::DimDiameter(dim) => {
                        draw_dim_diameter(painter, rect, viewport, &dim, highlight, render);
                    }
                    // `EntityGeom` は `#[non_exhaustive]`。未知の幾何は描かない。
                    _ => {}
                }
            }
        }
    };

    // 配置モード（Ctrl+D 複製・M 移動・R 回転・Shift+M 鏡映）中は、変換後のプレビューを
    // 描く（Document は変更しない）。配置モードは通常のドラッグと排他なので、こちらを優先する。
    match select_tool.placement_preview() {
        // 複製: 元の選択を強調したまま、複製先を重ねて仮表示する。
        Some(PlacementPreview::Duplicate { delta }) => {
            draw_selected(
                painter,
                rect,
                document,
                viewport,
                select_tool,
                highlight,
                paper_display,
                k,
            );
            draw_ghost(&|g| g.translated(delta));
            return;
        }
        // 移動・回転・鏡映: 元の位置は draw_entities が通常色で描く（ゴースト）。
        // 変換後だけを強調表示する。
        Some(PlacementPreview::Move { delta }) => {
            draw_ghost(&|g| g.translated(delta));
            return;
        }
        Some(PlacementPreview::Rotate { pivot, angle }) => {
            draw_ghost(&|g| g.rotated(pivot, angle));
            return;
        }
        Some(PlacementPreview::Mirror { axis_a, axis_b }) => {
            draw_ghost(&|g| g.mirrored(axis_a, axis_b));
            return;
        }
        None => {}
    }

    match select_tool.drag_preview() {
        Some(DragPreview::Rect { start, current }) => {
            // 矩形選択中: 現在の選択はそのまま強調しつつ、ドラッグ矩形を描く。
            draw_selected(
                painter,
                rect,
                document,
                viewport,
                select_tool,
                highlight,
                paper_display,
                k,
            );
            let a = viewport.world_to_screen(rect, start);
            let b = viewport.world_to_screen(rect, current);
            let r = Rect::from_two_pos(a, b);
            painter.rect_filled(r, 0.0, RECT_FILL_COLOR);
            let outline = Stroke::new(1.0, RECT_OUTLINE_COLOR);
            painter.line_segment([r.left_top(), r.right_top()], outline);
            painter.line_segment([r.right_top(), r.right_bottom()], outline);
            painter.line_segment([r.right_bottom(), r.left_bottom()], outline);
            painter.line_segment([r.left_bottom(), r.left_top()], outline);
        }
        Some(DragPreview::DimText { id, anchor }) => {
            // 文字ブロックドラッグ中（M9 タスク51）: 現在の選択・位置はそのまま強調表示し、
            // ドラッグ対象の寸法だけ `text_anchor = Some(anchor)` に差し替えた一時コピーを
            // 重ねて仮表示する（複製・移動プレビューと同じ「ゴースト」流儀。Document は
            // 変更しない）。
            draw_selected(
                painter,
                rect,
                document,
                viewport,
                select_tool,
                highlight,
                paper_display,
                k,
            );
            if let Some(entity) = document.entity(id)
                && let Some((kind, annotation)) = dim_kind_and_annotation(&entity.geom)
            {
                let mut new_annotation = annotation.clone();
                new_annotation.text_anchor = Some(anchor);
                if new_annotation.validate(kind).is_ok()
                    && let Some(new_geom) = dim_geom_with_annotation(&entity.geom, new_annotation)
                {
                    match new_geom {
                        EntityGeom::DimLinear(dim) => {
                            draw_dim_linear(painter, rect, viewport, &dim, highlight, render)
                        }
                        EntityGeom::DimRadial(dim) => {
                            draw_dim_radial(painter, rect, viewport, &dim, highlight, render)
                        }
                        EntityGeom::DimDiameter(dim) => {
                            draw_dim_diameter(painter, rect, viewport, &dim, highlight, render)
                        }
                        _ => {}
                    }
                }
            }
        }
        None => draw_selected(
            painter,
            rect,
            document,
            viewport,
            select_tool,
            highlight,
            paper_display,
            k,
        ),
    }
}

/// 選択エンティティを、その実位置に強調色 `stroke` で重ね描きする。
#[allow(clippy::too_many_arguments)]
fn draw_selected(
    painter: &egui::Painter,
    rect: Rect,
    document: &Document,
    viewport: &Viewport,
    select_tool: &SelectTool,
    stroke: Stroke,
    paper_display: bool,
    k: f64,
) {
    let render = dim_render(document.dim_style(), paper_display, k, viewport.zoom);
    for &id in select_tool.selection() {
        if let Some(entity) = document.entity(id) {
            match &entity.geom {
                EntityGeom::Shape(shape) => {
                    draw_shape(
                        painter,
                        rect,
                        viewport,
                        shape,
                        stroke,
                        Linetype::Continuous,
                        1.0,
                    );
                }
                EntityGeom::Text(text) => {
                    // 文字を強調色で上書きし、加えて表示上のワールド AABB（`height * k`、
                    // 判断(c)）の枠を描く（ヒットテストがこの AABB 近似であることを可視化し、
                    // 選択が分かりやすいように）。
                    draw_text(painter, rect, viewport, text, text.height * k, stroke.color);
                    let aabb = text_world_aabb(text, k);
                    draw_aabb_outline(painter, rect, viewport, &aabb, stroke);
                }
                EntityGeom::DimLinear(dim) => {
                    let ex = dimension::expand_linear(dim, render);
                    draw_dim_expansion(painter, rect, viewport, &ex, stroke);
                    draw_dim_label_box(painter, rect, viewport, &ex, stroke.color);
                }
                EntityGeom::DimRadial(dim) => {
                    let ex = dimension::expand_radial(dim, render);
                    draw_dim_expansion(painter, rect, viewport, &ex, stroke);
                    draw_dim_label_box(painter, rect, viewport, &ex, stroke.color);
                }
                EntityGeom::DimDiameter(dim) => {
                    let ex = dimension::expand_diameter(dim, render);
                    draw_dim_expansion(painter, rect, viewport, &ex, stroke);
                    draw_dim_label_box(painter, rect, viewport, &ex, stroke.color);
                }
                // `EntityGeom` は `#[non_exhaustive]`。未知の幾何は描かない。
                _ => {}
            }
        }
    }
}

/// 選択中の寸法の文字ブロック外形（[`dimension::DimExpansion::label_box`]）を
/// 細線で描く（M9 タスク51）。ドラッグで掴める場所をユーザーへ示すためだけの
/// 表示で、ヒットテスト自体（`dim_label_hit`）は保存データではなく同じ展開を
/// 再計算して使う（表示と判定を食い違わせないため、同じ `label_box` を共有する）。
fn draw_dim_label_box(
    painter: &egui::Painter,
    rect: Rect,
    viewport: &Viewport,
    ex: &dimension::DimExpansion,
    color: Color32,
) {
    let Some(quad) = ex.label_box else {
        return;
    };
    let thin = Stroke::new(1.0, color);
    for i in 0..4 {
        let a = viewport.world_to_screen(rect, quad[i]);
        let b = viewport.world_to_screen(rect, quad[(i + 1) % 4]);
        painter.line_segment([a, b], thin);
    }
}

/// AABB の枠線をスクリーンへ描く（Text 選択の可視化などに使う）。
fn draw_aabb_outline(
    painter: &egui::Painter,
    rect: Rect,
    viewport: &Viewport,
    aabb: &Aabb,
    stroke: Stroke,
) {
    let a = viewport.world_to_screen(rect, aabb.min);
    let b = viewport.world_to_screen(rect, aabb.max);
    let r = Rect::from_two_pos(a, b);
    painter.line_segment([r.left_top(), r.right_top()], stroke);
    painter.line_segment([r.right_top(), r.right_bottom()], stroke);
    painter.line_segment([r.right_bottom(), r.left_bottom()], stroke);
    painter.line_segment([r.left_bottom(), r.left_top()], stroke);
}

/// スナップ先にマーカーを描画する。候補種別ごとに形を変えて、どの種別に吸着したか
/// が一目で分かるようにする（端点=□、交点=×、中点=△、中心=○、グリッド=＋）。
fn draw_snap_marker(
    painter: &egui::Painter,
    rect: Rect,
    viewport: &Viewport,
    marker: &snap::SnapResult,
) {
    use snap::SnapKind;

    let c = viewport.world_to_screen(rect, marker.point);
    let s = SNAP_MARKER_SIZE;
    let stroke = Stroke::new(1.5, SNAP_MARKER_COLOR);
    let seg = |a: Pos2, b: Pos2| painter.line_segment([a, b], stroke);

    match marker.kind {
        SnapKind::Endpoint => {
            // 正方形（4 辺を線分で描く）。
            let r = Rect::from_center_size(c, egui::vec2(s * 2.0, s * 2.0));
            seg(r.left_top(), r.right_top());
            seg(r.right_top(), r.right_bottom());
            seg(r.right_bottom(), r.left_bottom());
            seg(r.left_bottom(), r.left_top());
        }
        SnapKind::Intersection => {
            // ×。
            seg(c + egui::vec2(-s, -s), c + egui::vec2(s, s));
            seg(c + egui::vec2(-s, s), c + egui::vec2(s, -s));
        }
        SnapKind::Midpoint => {
            // 上向き三角形。
            let top = c + egui::vec2(0.0, -s);
            let left = c + egui::vec2(-s, s);
            let right = c + egui::vec2(s, s);
            seg(top, left);
            seg(left, right);
            seg(right, top);
        }
        SnapKind::Center => {
            // 円。
            painter.circle_stroke(c, s, stroke);
        }
        SnapKind::Grid => {
            // ＋。
            seg(c + egui::vec2(-s, 0.0), c + egui::vec2(s, 0.0));
            seg(c + egui::vec2(0.0, -s), c + egui::vec2(0.0, s));
        }
    }
}

/// [`Rgb`] を egui の [`Color32`] へ変換する。
fn to_color32(color: Rgb) -> Color32 {
    Color32::from_rgb(color.r, color.g, color.b)
}

/// 形状 1 つを Painter へ描画する。`Arc` はネイティブの弧描画がないため、
/// 固定分割数（[`ARC_SEGMENTS`]）のポリライン近似で描画する。
fn draw_shape(
    painter: &egui::Painter,
    rect: Rect,
    viewport: &Viewport,
    shape: &Shape,
    stroke: Stroke,
    linetype: Linetype,
    k: f64,
) {
    match shape {
        Shape::Point(p) => {
            // 点は塗りつぶし円で表す実装で、線種の概念がない。
            let sp = viewport.world_to_screen(rect, *p);
            painter.circle_filled(sp, stroke.width.max(2.0), stroke.color);
        }
        Shape::Line(line) => {
            let a = viewport.world_to_screen(rect, line.a);
            let b = viewport.world_to_screen(rect, line.b);
            stroke_polyline(
                painter,
                vec![a, b],
                stroke,
                linetype,
                k,
                viewport.zoom,
                rect,
            );
        }
        Shape::Circle(circle) => {
            // 円は線種を反映するため、円弧と同じくポリライン近似（一周分）で描く。
            // `Continuous` のときだけ従来どおり `circle_stroke`（滑らかな真円）を使う。
            let center = viewport.world_to_screen(rect, circle.center);
            if matches!(linetype, Linetype::Continuous) {
                let radius = (circle.radius * viewport.zoom) as f32;
                painter.circle_stroke(center, radius, stroke);
            } else {
                let points: Vec<Pos2> = (0..=ARC_SEGMENTS)
                    .map(|i| {
                        let t = i as f64 / ARC_SEGMENTS as f64;
                        let angle = t * std::f64::consts::TAU;
                        let p = Point2::new(
                            circle.center.x + circle.radius * angle.cos(),
                            circle.center.y + circle.radius * angle.sin(),
                        );
                        viewport.world_to_screen(rect, p)
                    })
                    .collect();
                stroke_polyline(painter, points, stroke, linetype, k, viewport.zoom, rect);
            }
        }
        Shape::Arc(arc) => {
            draw_arc(painter, rect, viewport, arc, stroke, linetype, k);
        }
        Shape::Polyline(polyline) => {
            draw_polyline(painter, rect, viewport, polyline, stroke, linetype, k);
        }
    }
}

/// Text の表示上のワールド AABB（判断4: `height` は紙 mm、ワールド高さ = `height * k`）。
///
/// `mcad_core::EntityGeom::aabb()`（1:1 解釈、以下「元 AABB」）を anchor 基準に `k` 倍する。
/// この相似拡大が厳密に正しい理由: 元 AABB は `text_aabb` が組む局所 4 隅（`anchor` を
/// 原点として `width`・`text.height` に比例するベクトルをベースライン角で回転したもの）の
/// 外接矩形であり、`width` 自体も文字数 × 係数 × `height` で `height` に**線形**に比例する。
/// したがって `height` を `height * k` に置き換えた（＝「height×k の TextGeom」の）4 隅は、
/// 元の 4 隅を `anchor + k * (corner − anchor)` へ写した点に厳密一致する。この写像は
/// 各軸ごとに単調（`k > 0`、`Scale` の値域は正）なので、4 隅を包む外接矩形（min/max）も
/// 同じ写像で移る。すなわち「`anchor` 基準に元 AABB の min/max を `k` 倍」と
/// 「height×k の TextGeom の aabb()」は同じ結果になる（`mcad-core` 側の実装は
/// 変更しない。tcad が path 依存しているため）。
fn text_world_aabb(text: &TextGeom, k: f64) -> Aabb {
    let local = EntityGeom::Text(text.clone()).aabb();
    let scale_from_anchor = |p: Point2| text.anchor + (p - text.anchor) * k;
    Aabb {
        min: scale_from_anchor(local.min),
        max: scale_from_anchor(local.max),
    }
}

/// テキスト 1 つを Painter へ描画する（M6 タスク23、[`epaint::TextShape`] 使用）。
///
/// # フォントサイズ
///
/// `world_height（ワールド） × zoom` を px として毎フレーム計算し、ズームで文字も
/// 拡大縮小する（ワールド固定サイズ = CAD の期待動作。DESIGN.md M6 設計判断3）。
/// 判読不能な極小は描かず、過大サイズはフォントアトラス肥大を防ぐため [`MAX_TEXT_PX`]
/// で頭打ちにする。
///
/// `world_height` は呼び出し側がワールド長として明示的に渡す（タスク37 判断(d)）。
/// `TextGeom::height` を直接使わず引数化しているのは二重換算防止のため:
/// - Text エンティティ・プレビュー・ゴースト・選択ハイライトは `text.height * k` を渡す
///   （判断(c)、`k` = 紙 1mm あたりのワールド mm）。
/// - `draw_dim_expansion` は `dimension` の展開関数（`expand_linear`/`expand_radial`/
///   `expand_diameter`）が既にワールド長で組み立てた `ex.text.height` を**そのまま**渡す
///   （ここでさらに `k` を掛けると二重換算）。
///
/// # 位置と角度
///
/// アンカー（ベースライン左端）にガリー下端左を合わせる（M6 の近似。降り部は無視）。
/// egui の `TextShape` はガリー左上 `pos` まわりに回すため、下端左がアンカーに載るよう
/// `pos` をずらす。角度はワールド CCW θ をスクリーン（y 下向き）用に `a = −θ` へ符号反転する。
fn draw_text(
    painter: &egui::Painter,
    rect: Rect,
    viewport: &Viewport,
    text: &TextGeom,
    world_height: f64,
    color: Color32,
) {
    if text.content.is_empty() {
        return;
    }
    let font_px = world_height * viewport.zoom;
    if !font_px.is_finite() || font_px < MIN_TEXT_PX {
        return;
    }
    let font_px = font_px.min(MAX_TEXT_PX) as f32;
    let anchor = viewport.world_to_screen(rect, text.anchor);
    let font_id = egui::FontId::new(font_px, egui::FontFamily::Proportional);
    let galley = painter.layout_no_wrap(text.content.clone(), font_id, color);
    let h = galley.size().y;
    // ワールド CCW 角 θ → スクリーン egui 角 a = −θ。
    let a = -(text.angle as f32);
    let (sin_a, cos_a) = a.sin_cos();
    // pos = anchor − Rot(a)·(0, h)。Rot(a)·(0, h) = (−h·sin_a, h·cos_a)。
    let pos = anchor - egui::vec2(-h * sin_a, h * cos_a);
    let shape = egui::epaint::TextShape::new(pos, galley, color).with_angle(a);
    painter.add(egui::Shape::Text(shape));
}

/// 円弧を開始角〜終了角まで [`ARC_SEGMENTS`] 分割のポリラインで近似描画する。
fn draw_arc(
    painter: &egui::Painter,
    rect: Rect,
    viewport: &Viewport,
    arc: &Arc,
    stroke: Stroke,
    linetype: Linetype,
    k: f64,
) {
    let sweep = arc.sweep();
    let points: Vec<Pos2> = (0..=ARC_SEGMENTS)
        .map(|i| {
            let t = i as f64 / ARC_SEGMENTS as f64;
            let angle = arc.start_angle + sweep * t;
            let p = arc.circle().point_at_angle(angle);
            viewport.world_to_screen(rect, p)
        })
        .collect();
    stroke_polyline(painter, points, stroke, linetype, k, viewport.zoom, rect);
}

/// ポリラインを描画する。閉じている場合は末尾から先頭への辺も描く。
fn draw_polyline(
    painter: &egui::Painter,
    rect: Rect,
    viewport: &Viewport,
    polyline: &Polyline,
    stroke: Stroke,
    linetype: Linetype,
    k: f64,
) {
    if polyline.vertices.is_empty() {
        return;
    }
    let mut points: Vec<Pos2> = polyline
        .vertices
        .iter()
        .map(|p| viewport.world_to_screen(rect, *p))
        .collect();
    if polyline.closed && polyline.vertices.len() >= 2 {
        points.push(points[0]);
    }
    stroke_polyline(painter, points, stroke, linetype, k, viewport.zoom, rect);
}

fn main() -> anyhow::Result<()> {
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "mcad",
        native_options,
        Box::new(|cc| {
            // 文書内 Text の CJK グリフ用に、既定フォントの後ろへ Noto Sans JP を追加する
            // （M6 タスク23。M8 以降は UI ラベルの日本語もこの登録で描画される）。
            fonts::install_fallback_fonts(&cc.egui_ctx);
            Ok(Box::new(McadApp::with_config(config::load_startup())))
        }),
    )
    .map_err(|err| anyhow::anyhow!("failed to run mcad-app: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcad_core::Entity;
    use mcad_geom::{Circle, LineSeg};
    // 紙 mm のダッシュパターン定数は `plot` が持つ（タスク39）。画面側の px 換算
    // （[`dash_pattern_px`]）の回帰テストが元の値と突き合わせるために参照する。
    use crate::plot::{DASH_DOT_PATTERN_MM, DASH_PATTERN_MM};

    // rfd のファイルダイアログ（`open_document`/`save_document*` のダイアログ経路）は
    // ネイティブ UI を開くため headless では自動テストできない。未保存確認は
    // egui 内製モーダル（`ConfirmState`）に統一済みなのでロジック自体はテストできるが、
    // 「破棄して続行」時に実際にダイアログを開く経路（`request_open_document` が
    // dirty でないとき即座に `open_document` を呼ぶ分岐、モーダルの
    // `ConfirmingOpen` 分岐）はここでは検証しない。ここでは GUI コンテキストを
    // 要しない部分（拡張子補完・世代ベースの dirty 判定・確認モーダルへの状態遷移・
    // 新規文書のリセット内容）のみを検証する。

    #[test]
    fn format_grid_step_trims_trailing_zeros() {
        assert_eq!(format_grid_step(50.0), "50");
        assert_eq!(format_grid_step(0.05), "0.05");
        assert_eq!(format_grid_step(0.00005), "0.00005");
    }

    #[test]
    fn format_grid_step_rounds_float_artifacts() {
        // 10f64.powf(-1.0) * 2.0 は浮動小数演算の丸め誤差で 0.2 ちょうどにならない
        // ことがある。{:.6} で固定小数化してから整形することで "0.2" に丸まる。
        let step = 2.0 * 10f64.powf(-1.0);
        assert_eq!(format_grid_step(step), "0.2");
    }

    /// 複数種のエンティティ（線分・円・円弧・ポリライン）を追加したドキュメントを作る。
    ///
    /// M3期は `McadApp::new()`（起動直後の画面）が同内容を持っていたが、M4設計判断2
    /// （DESIGN.md 6章: 起動は空文書）により本体からは削除し、複数エンティティを要する
    /// テスト専用のヘルパーとしてここへ残す。
    fn sample_document() -> Document {
        let mut document = Document::new();
        let layer = document.current_layer();

        let sample_entities = [
            Entity::new(
                Shape::Line(LineSeg::new(Point2::new(-5.0, 0.0), Point2::new(5.0, 0.0))),
                layer,
                Style::inherited(),
            ),
            Entity::new(
                Shape::Circle(Circle::new(Point2::new(0.0, 3.0), 2.0)),
                layer,
                Style::inherited(),
            ),
            Entity::new(
                Shape::Arc(Arc::new(
                    Point2::new(-6.0, -4.0),
                    3.0,
                    0.0,
                    std::f64::consts::PI,
                )),
                layer,
                Style {
                    color: Some(Rgb::new(220, 80, 40)),
                    width_mm: Some(WidthMm::new(0.7).unwrap()),
                    linetype: None,
                },
            ),
            Entity::new(
                Shape::Polyline(Polyline::new(
                    vec![
                        Point2::new(2.0, -5.0),
                        Point2::new(4.0, -2.0),
                        Point2::new(6.0, -5.0),
                        Point2::new(8.0, -2.0),
                    ],
                    false,
                )),
                layer,
                Style::inherited(),
            ),
        ];
        for entity in sample_entities {
            document
                .apply(Command::AddEntity(entity))
                .expect("sample entity on current layer must be addable");
        }
        document
    }

    #[test]
    fn ensure_mcad_extension_appends_when_missing() {
        assert_eq!(
            ensure_mcad_extension(PathBuf::from("/tmp/drawing")),
            PathBuf::from("/tmp/drawing.mcad")
        );
    }

    #[test]
    fn ensure_mcad_extension_replaces_other_extension() {
        assert_eq!(
            ensure_mcad_extension(PathBuf::from("/tmp/drawing.json")),
            PathBuf::from("/tmp/drawing.mcad")
        );
    }

    #[test]
    fn ensure_mcad_extension_is_case_insensitive_noop() {
        // 既に（大小問わず）.mcad ならそのまま返す。
        assert_eq!(
            ensure_mcad_extension(PathBuf::from("/tmp/drawing.MCAD")),
            PathBuf::from("/tmp/drawing.MCAD")
        );
    }

    #[test]
    fn new_app_starts_clean_and_untitled() {
        let app = McadApp::new();
        assert!(!app.is_dirty());
        assert!(app.current_path.is_none());
    }

    #[test]
    fn new_app_starts_with_empty_document_and_default_viewport() {
        // M4設計判断2: 起動は空文書。サンプルエンティティは一切追加しない
        // （テストが必要なら `sample_document()` を使う）。
        let app = McadApp::new();
        assert_eq!(app.document.entity_count(), 0);
        // レイヤーは既定セット（"0" + DEFAULT_EXTRA_LAYERS）のみ。
        assert_eq!(app.document.layer_count(), 1 + DEFAULT_EXTRA_LAYERS.len());
        assert_eq!(app.viewport, Viewport::new());
        assert!(!app.pending_zoom_fit);
    }

    #[test]
    fn is_dirty_tracks_generation_against_saved_point() {
        // 世代ベースの dirty 判定を app レベルで確認する（rfd を一切開かない経路）。
        let mut app = McadApp::new();
        assert!(!app.is_dirty());

        let layer = app.document.current_layer();
        let point =
            |x: f64| Entity::new(Shape::Point(Point2::new(x, x)), layer, Style::inherited());

        // 1 操作で未保存の変更あり。
        app.document.apply(Command::AddEntity(point(1.0))).unwrap();
        assert!(app.is_dirty());

        // 「保存した」= saved_generation を現在世代へ合わせると未保存でなくなる。
        app.saved_generation = app.document.generation();
        assert!(!app.is_dirty());

        // さらに 1 操作で dirty。undo で保存時点へ厳密に戻ると clean、redo で再び dirty。
        app.document.apply(Command::AddEntity(point(2.0))).unwrap();
        assert!(app.is_dirty());
        assert!(app.document.undo());
        assert!(!app.is_dirty());
        assert!(app.document.redo());
        assert!(app.is_dirty());
    }

    #[test]
    fn request_new_document_executes_immediately_when_not_dirty() {
        // 未 dirty のときは確認モーダルを出さず、即座に新規文書へ置き換える。
        // McadApp::new() は saved_generation を現在世代へ合わせるので未 dirty で始まる。
        let mut app = McadApp::new();
        app.current_path = Some(PathBuf::from("/tmp/existing.mcad"));
        assert!(!app.is_dirty());

        app.request_new_document(0.0);

        assert_eq!(app.confirm_state, ConfirmState::Idle);
        assert_eq!(app.document.entity_count(), 0);
        assert!(app.current_path.is_none());
    }

    #[test]
    fn request_new_document_defers_to_modal_when_dirty() {
        // dirty のときは即座に置き換えず、ConfirmingNew へ遷移するだけ
        // （ドキュメントは変更されない。実行はモーダルの「破棄して続行」を待つ）。
        let mut app = McadApp::new();
        let layer = app.document.current_layer();
        app.document
            .apply(Command::AddEntity(Entity::new(
                Shape::Point(Point2::new(1.0, 1.0)),
                layer,
                Style::inherited(),
            )))
            .unwrap();
        assert!(app.is_dirty());
        let entity_count_before = app.document.entity_count();

        app.request_new_document(0.0);

        assert_eq!(app.confirm_state, ConfirmState::ConfirmingNew);
        assert_eq!(app.document.entity_count(), entity_count_before);
    }

    #[test]
    fn request_open_document_defers_to_modal_when_dirty() {
        // Ctrl+O も同じ経路。dirty なら rfd のネイティブファイル選択を一切開かず
        // ConfirmingOpen へ遷移するだけなので headless でも安全にテストできる。
        let mut app = McadApp::new();
        let layer = app.document.current_layer();
        app.document
            .apply(Command::AddEntity(Entity::new(
                Shape::Point(Point2::new(2.0, 2.0)),
                layer,
                Style::inherited(),
            )))
            .unwrap();
        assert!(app.is_dirty());

        app.request_open_document(0.0);

        assert_eq!(app.confirm_state, ConfirmState::ConfirmingOpen);
    }

    #[test]
    fn confirm_state_prompt_is_some_only_while_confirming() {
        // Idle と Closing はモーダルを描かない（`None`）。3つの Confirming* は描く。
        assert!(ConfirmState::Idle.prompt().is_none());
        assert!(ConfirmState::Closing.prompt().is_none());
        assert!(ConfirmState::ConfirmingClose.prompt().is_some());
        assert!(ConfirmState::ConfirmingNew.prompt().is_some());
        assert!(ConfirmState::ConfirmingOpen.prompt().is_some());
        assert!(ConfirmState::ConfirmingOpenDxf.prompt().is_some());
    }

    #[test]
    fn app_shortcuts_gated_by_modal_and_text_focus() {
        // モーダル非表示かつテキスト欄フォーカスなしのときだけショートカットを処理する。
        assert!(app_shortcuts_enabled(false, false));
        // 距離入力欄などテキスト欄フォーカス中は、undo/redo・ファイル操作・Ctrl+D・
        // ツール切替を一括で抑止する（Ctrl+Z がドキュメントを undo する等の競合防止）。
        assert!(!app_shortcuts_enabled(false, true));
        // モーダル表示中は（フォーカス有無に関わらず）抑止する。未保存確認・表題欄編集
        // ダイアログのどちらも `McadApp::modal_open()` へ集約されるため、ここでは
        // 集約後の bool のみを扱う（M8タスク38）。
        assert!(!app_shortcuts_enabled(true, false));
        assert!(!app_shortcuts_enabled(true, true));
    }

    #[test]
    fn select_canvas_key_shortcuts_disabled_while_a_text_field_is_focused() {
        // 差し戻し対応C: 寸法パネルの数値欄・オフセット距離欄などにフォーカスがある間は
        // Delete/Backspace/Esc をキャンバス選択操作として処理しない
        // （`handle_select_input` が `select_canvas_key_shortcuts_enabled` で判定する）。
        assert!(select_canvas_key_shortcuts_enabled(false));
        assert!(!select_canvas_key_shortcuts_enabled(true));
    }

    #[test]
    fn modal_open_covers_confirm_state_and_sheet_dialog() {
        let mut app = McadApp::new();
        assert!(!app.modal_open());

        app.confirm_state = ConfirmState::ConfirmingNew;
        assert!(app.modal_open());
        app.confirm_state = ConfirmState::Idle;
        assert!(!app.modal_open());

        app.sheet_dialog = Some(TitleBlockDialogState::from_sheet(app.document.sheet()));
        assert!(app.modal_open());
        app.sheet_dialog = None;
        assert!(!app.modal_open());
    }

    #[test]
    fn title_block_dialog_to_sheet_replaces_only_fields() {
        // OK で fields 全置換の SheetMeta が1つできる。尺度・用紙・様式・枠表示は
        // base のまま変わらない（表題欄の記入内容だけを編集するダイアログのため）。
        let mut base = SheetMeta {
            scale: Scale::new(1, 2).unwrap(),
            paper: PaperSize::A3,
            title_block: TitleBlockKind::C,
            frame_visible: true,
            ..SheetMeta::default()
        };
        base.fields.drawing_number = "OLD-1".to_owned();

        let mut dialog = TitleBlockDialogState::from_sheet(&base);
        assert_eq!(dialog.drawing_number, "OLD-1");
        dialog.drawing_number = "NEW-2".to_owned();
        dialog.drawing_title = "新図面".to_owned();
        dialog.projection = ProjectionMethod::FirstAngle;
        dialog.author = "almaz".to_owned();
        dialog.date = "2026-08-09".to_owned();
        dialog.revision = "B".to_owned();

        let updated = dialog.to_sheet(&base);
        assert_eq!(updated.fields.drawing_number, "NEW-2");
        assert_eq!(updated.fields.drawing_title, "新図面");
        assert_eq!(updated.fields.projection, ProjectionMethod::FirstAngle);
        assert_eq!(updated.fields.author, "almaz");
        assert_eq!(updated.fields.date, "2026-08-09");
        assert_eq!(updated.fields.revision, "B");
        // fields 以外は base のまま。
        assert_eq!(updated.scale, base.scale);
        assert_eq!(updated.paper, base.paper);
        assert_eq!(updated.title_block, base.title_block);
        assert_eq!(updated.frame_visible, base.frame_visible);
    }

    #[test]
    fn title_block_dialog_ok_applies_one_set_sheet_undo_unit_cancel_leaves_document_unchanged() {
        // ダイアログ確定ロジック: OK で SheetMeta が1回の Command::SetSheet として
        // 適用され undo 1単位になる。キャンセル相当（sheet_dialog を捨てるだけ）では
        // document の世代が変わらないことを固定する（M8タスク38 完了条件 9）。
        let mut document = Document::new();
        let generation_before = document.generation();

        let dialog = TitleBlockDialogState {
            drawing_number: "MCAD-100".to_owned(),
            drawing_title: "テスト図面".to_owned(),
            projection: ProjectionMethod::FirstAngle,
            author: "almaz".to_owned(),
            date: "2026-08-09".to_owned(),
            revision: "A".to_owned(),
        };

        // キャンセル: SheetMeta を組み立てずダイアログを破棄するだけなので document は不変。
        assert_eq!(document.generation(), generation_before);

        // OK: 1回の SetSheet で確定し、undo 1回で元に戻る。
        let new_sheet = dialog.to_sheet(document.sheet());
        document.apply(Command::SetSheet(new_sheet)).unwrap();
        let generation_after_ok = document.generation();
        assert_ne!(generation_after_ok, generation_before);
        assert_eq!(document.sheet().fields.drawing_number, "MCAD-100");

        assert!(document.undo());
        assert_eq!(document.generation(), generation_before);
        assert_eq!(document.sheet().fields.drawing_number, "");
    }

    #[test]
    fn invalid_scale_input_does_not_issue_set_sheet() {
        // M8タスク38 完了条件10: 不正尺度入力時に SetSheet を発行しない
        // （document 世代・履歴とも不変）。`sheet_panel` の適用ボタンが行うのと同じ
        // 「まず parse_scale_input で検証してから SetSheet」の流れを document レベルで
        // 固定する。
        let document = Document::new();
        let generation_before = document.generation();

        for bad in ["", "1", "1:2:3", "0:1", "1:0", "abc:1"] {
            let result = frame::parse_scale_input(bad);
            assert!(result.is_err(), "\"{bad}\" should be rejected");
            // 拒否された場合、呼び出し側は SetSheet を組み立てない（アプリの
            // sheet_panel と同じ分岐）ので document は一切変化しない。
            assert_eq!(document.generation(), generation_before);
        }
    }

    #[test]
    fn parse_offset_distance_accepts_only_positive_finite() {
        assert_eq!(parse_offset_distance("2.5"), Some(2.5));
        assert_eq!(parse_offset_distance("  3 "), Some(3.0));
        // 空・0・負・非数・非有限は None（通過点方式へフォールバック）。
        assert_eq!(parse_offset_distance(""), None);
        assert_eq!(parse_offset_distance("0"), None);
        assert_eq!(parse_offset_distance("-1"), None);
        assert_eq!(parse_offset_distance("abc"), None);
        assert_eq!(parse_offset_distance("inf"), None);
        assert_eq!(parse_offset_distance("NaN"), None);
    }

    #[test]
    fn parse_fillet_radius_accepts_only_positive_finite() {
        assert_eq!(parse_fillet_radius("2.5"), Some(2.5));
        assert_eq!(parse_fillet_radius("  3 "), Some(3.0));
        // 空・0・負・非数・非有限は None（FilletTool 側が「enter a radius」で拒否する）。
        for text in ["", "0", "-1", "abc", "inf", "NaN"] {
            assert_eq!(parse_fillet_radius(text), None, "input: {text:?}");
        }
    }

    #[test]
    fn fillet_tool_caches_picked_shapes() {
        // 1 本目の形状スナップショットを抱えるので、undo/redo・ファイル操作の後に
        // 作り直される側（[`McadApp::reset_picked_shape_tool`]）に含まれる。
        assert!(ToolKind::Fillet.caches_picked_shapes());
        assert!(!ToolKind::Line.caches_picked_shapes());
        // Split は ShapePick を状態として跨いで保持しないので含まれない。
        assert!(!ToolKind::Split.caches_picked_shapes());
    }

    #[test]
    fn new_document_resets_to_empty_document_and_clears_path_and_dirty() {
        // new_document 自体は dirty を確認しない（呼び出し側の
        // request_new_document/確認モーダルが確認済みであることを前提とする）。
        let mut app = McadApp::new();
        app.current_path = Some(PathBuf::from("/tmp/existing.mcad"));

        let layer = app.document.current_layer();
        app.document
            .apply(Command::AddEntity(Entity::new(
                Shape::Point(Point2::new(1.0, 1.0)),
                layer,
                Style::inherited(),
            )))
            .unwrap();
        assert!(app.document.entity_count() > 0);

        app.new_document(0.0);

        // Codex レビュー指摘への対応: Ctrl+N はサンプル**エンティティ**を含まない。
        // レイヤーだけは既定セット（"0" + DEFAULT_EXTRA_LAYERS）を持つ（§5）。
        assert_eq!(app.document.entity_count(), 0);
        assert_eq!(app.document.layer_count(), 1 + DEFAULT_EXTRA_LAYERS.len());
        assert!(app.current_path.is_none());
        // 新規文書は saved_generation を新しい基準点へ合わせるので未 dirty。
        assert!(!app.is_dirty());
        assert_eq!(app.tool_kind, ToolKind::Select);
        assert!(app.select_tool.selection().is_empty());
    }

    #[test]
    fn ensure_dxf_extension_appends_when_missing() {
        assert_eq!(
            ensure_dxf_extension(PathBuf::from("/tmp/drawing")),
            PathBuf::from("/tmp/drawing.dxf")
        );
    }

    #[test]
    fn ensure_dxf_extension_replaces_other_extension() {
        assert_eq!(
            ensure_dxf_extension(PathBuf::from("/tmp/drawing.mcad")),
            PathBuf::from("/tmp/drawing.dxf")
        );
    }

    #[test]
    fn ensure_dxf_extension_is_case_insensitive_noop() {
        assert_eq!(
            ensure_dxf_extension(PathBuf::from("/tmp/drawing.DXF")),
            PathBuf::from("/tmp/drawing.DXF")
        );
    }

    #[test]
    fn ensure_pdf_extension_appends_when_missing() {
        assert_eq!(
            ensure_pdf_extension(PathBuf::from("/tmp/drawing")),
            PathBuf::from("/tmp/drawing.pdf")
        );
    }

    #[test]
    fn ensure_pdf_extension_replaces_other_extension() {
        assert_eq!(
            ensure_pdf_extension(PathBuf::from("/tmp/drawing.mcad")),
            PathBuf::from("/tmp/drawing.pdf")
        );
    }

    #[test]
    fn ensure_pdf_extension_is_case_insensitive_noop() {
        assert_eq!(
            ensure_pdf_extension(PathBuf::from("/tmp/drawing.PDF")),
            PathBuf::from("/tmp/drawing.PDF")
        );
    }

    #[test]
    fn request_open_dxf_defers_to_modal_when_dirty() {
        // Ctrl+Shift+O も他の open 系ショートカットと同様、dirty なら rfd のネイティブ
        // ファイル選択を一切開かず ConfirmingOpenDxf へ遷移するだけなので headless でも
        // 安全にテストできる。
        let mut app = McadApp::new();
        let layer = app.document.current_layer();
        app.document
            .apply(Command::AddEntity(Entity::new(
                Shape::Point(Point2::new(3.0, 3.0)),
                layer,
                Style::inherited(),
            )))
            .unwrap();
        assert!(app.is_dirty());

        app.request_open_dxf(0.0);

        assert_eq!(app.confirm_state, ConfirmState::ConfirmingOpenDxf);
    }

    #[test]
    fn apply_imported_dxf_clears_current_path_and_forces_dirty() {
        // DESIGN.md 6章 設計判断1: DXF importは `.mcad` と混同しない。import直後は
        // `current_path = None` になり、`load_dxf` が返す文書の世代は常に 0 だが
        // `saved_generation` はそれと一致しない番兵値になるため必ず dirty になる
        // （Ctrl+S を押すと元の DXF を上書きせず「名前を付けて保存」ダイアログへ誘導される）。
        let mut app = McadApp::new();
        app.current_path = Some(PathBuf::from("/tmp/existing.mcad"));
        // McadApp::new() 直後は not dirty（saved_generation が現在世代に一致）。
        assert!(!app.is_dirty());

        let imported = Document::new();
        assert_eq!(imported.generation(), 0);
        let summary = ImportSummary {
            document: imported,
            skipped_entities: 2,
            clamped_line_widths: 0,
        };

        app.apply_imported_dxf(summary, 0.0);

        assert!(app.current_path.is_none());
        assert!(app.is_dirty());
        assert_eq!(app.saved_generation, DXF_IMPORT_SAVED_GENERATION_SENTINEL);
        assert!(
            app.status
                .as_ref()
                .is_some_and(|m| m.text.contains('2') && m.text.contains("skipped"))
        );
    }

    #[test]
    fn open_status_reports_clamped_legacy_widths() {
        // 旧 `.mcad`（v1〜v3）の線幅移行で上限へ丸めた件数は黙殺せず表示する
        // （DESIGN.md M8 設計判断6 の規則3）。
        //
        // かつては `is_ascii()` も検証していたが、M8 で ASCII 限定の規約を撤廃したため
        // 外した（CJK は fonts.rs のフォールバックで描画できる）。ステータスバーは
        // まだ日本語化していない領域なので文言自体は英語のまま。
        let quiet = open_status(0);
        assert_eq!(quiet, "Opened file");

        let noisy = open_status(3);
        assert!(noisy.contains('3'), "件数が出るべき: {noisy}");
        assert!(noisy.contains("clamped"), "丸めたことが分かるべき: {noisy}");
    }

    #[test]
    fn apply_imported_dxf_resets_transient_ui_state() {
        // import直後は選択集合・作図ツールが読込前のドキュメントを参照しないよう
        // リセットされる（`reset_transient_ui_state` の doc 参照）。
        let mut app = McadApp::new();
        app.tool_kind = ToolKind::Line;
        app.tool = ToolKind::Line.spawn();

        let summary = ImportSummary {
            document: Document::new(),
            skipped_entities: 0,
            clamped_line_widths: 0,
        };
        app.apply_imported_dxf(summary, 0.0);

        assert_eq!(app.tool_kind, ToolKind::Select);
        assert!(app.tool.is_none());
        assert!(app.select_tool.selection().is_empty());
        assert!(
            app.status
                .as_ref()
                .is_some_and(|m| m.text.contains("Imported DXF"))
        );
    }

    #[test]
    fn apply_imported_dxf_with_entities_sets_pending_zoom_fit() {
        // M4タスク13: import直後、スクリーン矩形がまだ確定していないためその場では
        // フィットできず、次フレームの CentralPanel へ委ねる `pending_zoom_fit` を立てる。
        let mut app = McadApp::new();
        assert!(!app.pending_zoom_fit);

        let summary = ImportSummary {
            document: sample_document(),
            skipped_entities: 0,
            clamped_line_widths: 0,
        };
        app.apply_imported_dxf(summary, 0.0);

        assert!(app.pending_zoom_fit);
    }

    #[test]
    fn apply_imported_dxf_with_no_entities_resets_default_viewport_without_pending_fit() {
        // 空のDXFを開いた場合はフィット対象がないので、pending_zoom_fit は立てず
        // その場で既定ビュー（Viewport::new()）へリセットする。
        let mut app = McadApp::new();
        app.viewport.zoom = 42.0;
        app.viewport.center = Point2::new(100.0, -50.0);

        let summary = ImportSummary {
            document: Document::new(),
            skipped_entities: 0,
            clamped_line_widths: 0,
        };
        app.apply_imported_dxf(summary, 0.0);

        assert!(!app.pending_zoom_fit);
        assert_eq!(app.viewport, Viewport::new());
    }

    #[test]
    fn request_zoom_fit_sets_flag_only_when_entities_present() {
        let mut app = McadApp::new();

        app.document = sample_document();
        app.pending_zoom_fit = false;
        app.request_zoom_fit();
        assert!(app.pending_zoom_fit);

        app.document = Document::new();
        app.viewport.zoom = 7.0;
        app.request_zoom_fit();
        assert!(!app.pending_zoom_fit);
        assert_eq!(app.viewport, Viewport::new());
    }

    #[test]
    fn request_zoom_fit_also_fires_for_empty_document_when_frame_is_visible() {
        // M8タスク38: 枠 ON のときは空文書でも用紙矩形がフィット対象になるため、
        // エンティティ0件でも pending_zoom_fit が立つ（枠 OFF は既存挙動のまま）。
        let mut app = McadApp::new();
        app.document = Document::new();

        app.viewport.zoom = 7.0;
        app.request_zoom_fit();
        assert!(!app.pending_zoom_fit, "枠 OFF は従来どおりリセットのみ");
        assert_eq!(app.viewport, Viewport::new());

        let mut sheet = app.document.sheet().clone();
        sheet.frame_visible = true;
        app.document.apply(Command::SetSheet(sheet)).unwrap();

        app.viewport.zoom = 7.0;
        app.pending_zoom_fit = false;
        app.request_zoom_fit();
        assert!(app.pending_zoom_fit, "枠 ON なら空文書でも pending が立つ");
    }

    #[test]
    fn fit_target_aabb_unions_document_and_paper_when_frame_visible() {
        // 枠 OFF: document_aabb のみ（既定挙動）。
        let document = Document::new();
        assert!(fit_target_aabb(&document).is_none());

        // 枠 ON・空文書: 用紙矩形そのもの（A4横・1:1 の既定 = (0,0)-(297,210)）。
        let mut document = Document::new();
        let mut sheet = document.sheet().clone();
        sheet.frame_visible = true;
        document.apply(Command::SetSheet(sheet)).unwrap();
        let aabb = fit_target_aabb(&document).expect("frame visible provides a paper aabb");
        assert_eq!(aabb.min, Point2::new(0.0, 0.0));
        assert_eq!(aabb.max, Point2::new(297.0, 210.0));

        // 枠 ON・エンティティあり: document_aabb と用紙矩形の合併。
        let mut document = sample_document();
        let mut sheet = document.sheet().clone();
        sheet.frame_visible = true;
        document.apply(Command::SetSheet(sheet)).unwrap();
        let doc_only = document_aabb(&document).unwrap();
        let unioned = fit_target_aabb(&document).unwrap();
        let paper = Aabb::new(Point2::new(0.0, 0.0), Point2::new(297.0, 210.0));
        assert_eq!(unioned, doc_only.union(&paper));
    }

    #[test]
    fn new_document_resets_viewport_to_default() {
        let mut app = McadApp::new();
        app.viewport.zoom = 5.0;
        app.viewport.center = Point2::new(3.0, 4.0);
        app.pending_zoom_fit = true;

        app.new_document(0.0);

        assert_eq!(app.viewport, Viewport::new());
        assert!(!app.pending_zoom_fit);
    }

    #[test]
    fn document_aabb_is_none_for_empty_document() {
        let document = Document::new();
        assert!(document_aabb(&document).is_none());
    }

    #[test]
    fn document_aabb_unions_all_entity_bounds() {
        let document = sample_document();
        let aabb = document_aabb(&document).expect("sample document has entities");

        // sample_document のエンティティのうち、最も外側の座標
        // （円弧の左端 x=-9, ポリラインの右端 x=8/y=-5, 円の上端 y=5）を包んでいるはず。
        assert!(aabb.min.x <= -6.0);
        assert!(aabb.max.x >= 8.0);
        assert!(aabb.min.y <= -5.0);
        assert!(aabb.max.y >= 5.0);
    }

    #[test]
    fn commit_text_assigns_new_entity_to_text_layer_when_present() {
        // fresh_document（起動時・Ctrl+N）は "文字" レイヤーを持つので、commit_text は
        // カレントレイヤー（"外形線"）ではなくそちらへ割り当てる（M9 タスク53）。
        let mut app = McadApp::new();
        app.text_content_input = "abc".to_owned();

        let committed = app.commit_text(Point2::new(1.0, 2.0), 0.0);
        assert!(committed);

        let text_layer = layer_named(&app.document, TEXT_LAYER_NAME).unwrap();
        let (_, entity) = app
            .document
            .entities()
            .find(|(_, e)| matches!(e.geom, EntityGeom::Text(_)))
            .expect("text entity must exist");
        assert_eq!(entity.layer, text_layer);
        assert_ne!(entity.layer, app.document.current_layer());
    }

    #[test]
    fn hiding_text_field_clears_pending_content() {
        // Esc でアンカーをキャンセルした等で入力欄が非表示に転じたら、入力中の文字列を
        // 捨てる（次にアンカーを置いたとき前回入力が残らない。coordinator 指摘の回帰）。
        let mut app = McadApp::new();
        app.text_content_input = "abc".to_owned();
        app.text_field_shown = true;

        app.set_text_field_shown(false);
        assert!(app.text_content_input.is_empty());
        assert!(!app.text_field_shown);
    }

    #[test]
    fn text_field_content_survives_while_shown() {
        // 表示が続いている間（入力中）は文字列を消さない。
        let mut app = McadApp::new();
        app.text_content_input = "abc".to_owned();

        // 非表示→表示（初回表示）はクリアしない。
        app.set_text_field_shown(true);
        assert_eq!(app.text_content_input, "abc");
        // 表示継続でもクリアしない。
        app.set_text_field_shown(true);
        assert_eq!(app.text_content_input, "abc");
    }

    #[test]
    fn reset_transient_ui_state_restores_default_text_height() {
        // 新規作成・読込の直後（reset_transient_ui_state）は、別図面へ切り替わるため
        // 前図面で変更した高さ入力を持ち越してはいけない（Codex 指摘の回帰）。
        let mut app = McadApp::new();
        app.text_height_input = "99".to_owned();
        app.text_content_input = "abc".to_owned();

        app.reset_transient_ui_state();

        assert_eq!(app.text_height_input, DEFAULT_TEXT_HEIGHT);
        assert!(app.text_content_input.is_empty());
    }

    #[test]
    fn reset_transient_ui_state_clears_alt_view_gesture_state() {
        // 読込後に押しっぱなしの Alt が旧アンカー/旧ジェスチャ種別を引きずらないこと
        // （DESIGN.md 7章「ズーム・パンの追加操作手段」設計判断(d)）。
        let mut app = McadApp::new();
        app.alt_zoom_anchor = Some(Pos2::new(12.0, 34.0));
        app.alt_view_gesture = Some(ViewGesture::Pan);

        app.reset_transient_ui_state();

        assert!(app.alt_zoom_anchor.is_none());
        assert!(app.alt_view_gesture.is_none());
    }

    /// `modifier_view_gesture` の真理値表（DESIGN.md 設計判断(a)(d)）。
    ///
    /// 有効化の必要条件（hovered かつボタン非押下かつ command 非押下）を満たさない
    /// 組み合わせはすべて `None`、満たした上で `alt` が立っていなければ `None`、
    /// `alt && !shift` で `Zoom`、`alt && shift` で `Pan` になることを固定する。
    #[test]
    fn modifier_view_gesture_truth_table() {
        // (alt, shift, command, any_down, hovered) -> expected
        type Case = (bool, bool, bool, bool, bool, Option<ViewGesture>);
        let cases: &[Case] = &[
            // 必要条件を満たし、alt のみ → Zoom。
            (true, false, false, false, true, Some(ViewGesture::Zoom)),
            // 必要条件を満たし、alt+shift → Pan。
            (true, true, false, false, true, Some(ViewGesture::Pan)),
            // alt が立っていなければ shift/command/any_down/hovered に関わらず None。
            (false, false, false, false, true, None),
            (false, true, false, false, true, None),
            // ポインタボタンが1つでも押されていたら None（矩形選択等との非干渉）。
            (true, false, false, true, true, None),
            (true, true, false, true, true, None),
            // command 押下時は None（AltGr=Ctrl+Alt 系の除外）。
            (true, false, true, false, true, None),
            (true, true, true, false, true, None),
            // hovered でなければ None（パネル上等）。
            (true, false, false, false, false, None),
            (true, true, false, false, false, None),
            // 何も条件を満たさない基準ケース。
            (false, false, false, false, false, None),
        ];
        for &(alt, shift, command, any_down, hovered, expected) in cases {
            assert_eq!(
                modifier_view_gesture(alt, shift, command, any_down, hovered),
                expected,
                "alt={alt} shift={shift} command={command} any_down={any_down} hovered={hovered}"
            );
        }
    }

    #[test]
    fn reset_transient_ui_state_clears_sheet_dialog_and_scale_input() {
        // M8タスク38: 表題欄編集ダイアログ・尺度カスタム入力も別図面へ持ち越さない。
        let mut app = McadApp::new();
        app.sheet_dialog = Some(TitleBlockDialogState::from_sheet(app.document.sheet()));
        app.scale_custom_selected = true;
        app.scale_custom_input = "1:2".to_owned();
        app.scale_input_error = Some("dummy".to_owned());

        app.reset_transient_ui_state();

        assert!(app.sheet_dialog.is_none());
        assert!(!app.scale_custom_selected);
        assert!(app.scale_custom_input.is_empty());
        assert!(app.scale_input_error.is_none());
    }

    #[test]
    fn document_aabb_includes_text_bounds() {
        // zoom fit（document_aabb）が Text の近似 aabb も合算に含めること（M6 タスク23 item5）。
        let mut document = Document::new();
        let layer = document.current_layer();
        // 遠く離れたアンカーの Text だけを置く。document_aabb がその範囲を包めば成功。
        document
            .apply(Command::AddEntity(Entity::new(
                EntityGeom::Text(TextGeom {
                    anchor: Point2::new(100.0, 200.0),
                    content: "hello".to_owned(),
                    height: 5.0,
                    angle: 0.0,
                }),
                layer,
                Style::inherited(),
            )))
            .unwrap();
        let aabb = document_aabb(&document).expect("document has a text entity");
        assert!(aabb.min.x <= 100.0 && aabb.max.x >= 100.0);
        assert!(aabb.min.y <= 200.0 && aabb.max.y >= 205.0);
    }

    // --- 配置モードの解除経路（Codex 敵対的レビュー指摘1・2の回帰） ---

    /// カレントレイヤーに水平線分を1本追加し、その [`mcad_core::EntityId`] を返す。
    fn add_line(app: &mut McadApp, x: f64) -> mcad_core::EntityId {
        let layer = app.document.current_layer();
        app.document
            .apply(Command::AddEntity(Entity::new(
                Shape::Line(LineSeg::new(Point2::new(x, 0.0), Point2::new(x + 1.0, 0.0))),
                layer,
                Style::inherited(),
            )))
            .unwrap()
            .entities[0]
    }

    /// Codex 指摘（M9 タスク50）回帰: undo/redo は選択 ID を変えずに注記だけを変えるため、
    /// 寸法パネルの入力バッファを ID 集合だけで鮮度判定していると、undo 前の値が残って
    /// 「確定」で undo を打ち消してしまう。履歴変更後は対象集合を空にして再同期させる。
    #[test]
    fn after_history_change_invalidates_dimension_edit_buffers() {
        let mut app = McadApp::new();
        let dim = add_test_dim_linear(&mut app.document);
        app.select_tool.set_selection(vec![dim]);
        app.dim_edit_target = vec![dim];
        app.dim_value_override_input = "A".to_string();

        app.after_history_change();

        assert!(app.dim_edit_target.is_empty());
        // 次フレームの同期で、空の対象集合 ≠ 選択中の ID となりバッファが作り直される。
        let live = vec![(dim, DimKind::Linear, DimAnnotation::default())];
        let mut buffers = DimEditBuffers {
            edit_target: app.dim_edit_target.clone(),
            value_override_input: "A".to_string(),
            ..DimEditBuffers::default()
        };
        assert!(buffers.sync(&live));
        assert_eq!(buffers.value_override_input, "");
    }

    #[test]
    fn after_history_change_cancels_placement_and_retains_selection() {
        // 指摘1回帰: 配置モード進行中（基準点確定後〜配置先クリック前）に undo/redo が
        // 起きたら配置を解除する。放置すると生き残った部分集合だけが無警告で複製される。
        let mut app = McadApp::new();
        let a = add_line(&mut app, 0.0);
        let b = add_line(&mut app, 10.0);
        app.select_tool.set_selection(vec![a, b]);

        assert!(app.select_tool.start_duplicate());
        // 基準点を確定して配置先待ちにする。
        assert_eq!(
            app.select_tool
                .placement_click(&app.document, Point2::new(0.0, 0.0), 0.1),
            PlacementOutcome::Continue
        );
        assert!(app.select_tool.is_placing());

        // undo 相当: 直近の AddEntity(b) を巻き戻してから後始末する（ui() の undo 経路）。
        assert!(app.document.undo());
        app.after_history_change();

        // 配置モードは解除され、死んだ ID は選択から掃除されている。
        assert!(!app.select_tool.is_placing());
        assert!(app.snap_marker.is_none());
        assert_eq!(app.select_tool.selection(), &[a]);
    }

    #[test]
    fn cancel_placement_for_file_op_disarms_mode_but_keeps_selection() {
        // 指摘2回帰: 保存系（confirm_state 不変）やキャンセルされたファイル選択でも配置
        // モードが残らないよう、全ファイル操作の入口で呼ぶ解除ヘルパー。
        let mut app = McadApp::new();
        let a = add_line(&mut app, 0.0);
        app.select_tool.set_selection(vec![a]);

        assert!(app.select_tool.start_duplicate());
        assert!(app.select_tool.is_placing());

        app.cancel_placement_for_file_op();

        assert!(!app.select_tool.is_placing());
        assert!(app.snap_marker.is_none());
        // 選択そのものは変えない（ファイル操作の本体が別途処理する）。
        assert_eq!(app.select_tool.selection(), &[a]);
    }

    #[test]
    fn request_new_document_when_not_dirty_disarms_placement() {
        // ファイル操作入口の解除が実際の経路（Ctrl+N・非 dirty・rfd を開かない）でも
        // 効くこと。dirty なら確認モーダル経由の解除に委ねる（別テストで担保済み）。
        let mut app = McadApp::new();
        let a = add_line(&mut app, 0.0);
        app.select_tool.set_selection(vec![a]);
        // saved_generation を現在に合わせて未 dirty にし、即時新規文書の経路へ入れる。
        app.saved_generation = app.document.generation();
        assert!(!app.is_dirty());

        assert!(app.select_tool.start_duplicate());
        assert!(app.select_tool.is_placing());

        app.request_new_document(0.0);

        // 新規文書経路（reset_transient_ui_state 含む）で配置も選択も畳まれる。
        assert!(!app.select_tool.is_placing());
        assert!(app.select_tool.selection().is_empty());
    }

    #[test]
    fn dialog_start_dir_prefers_last_dialog_dir_over_current_path() {
        let mut app = McadApp::new();
        // 両方 None なら None（rfd 既定に任せる）。
        assert_eq!(app.dialog_start_dir(), None);

        // current_path のみあれば、その親ディレクトリへフォールバックする。
        app.current_path = Some(PathBuf::from("/tmp/some/dir/drawing.mcad"));
        assert_eq!(app.dialog_start_dir(), Some(PathBuf::from("/tmp/some/dir")));

        // last_dialog_dir があれば、current_path の親より優先する。
        app.last_dialog_dir = Some(PathBuf::from("/tmp/other/dir"));
        assert_eq!(
            app.dialog_start_dir(),
            Some(PathBuf::from("/tmp/other/dir"))
        );
    }

    #[test]
    fn dialog_start_dir_falls_back_to_recent_files_head() {
        let mut app = McadApp::new();
        app.config
            .recent_files
            .push(PathBuf::from("/tmp/recent/dir/drawing.mcad"));
        // last_dialog_dir も current_path も無ければ recent_files 先頭の親へ落ちる。
        assert_eq!(
            app.dialog_start_dir(),
            Some(PathBuf::from("/tmp/recent/dir"))
        );

        // current_path があれば、recent_files より優先する。
        app.current_path = Some(PathBuf::from("/tmp/some/dir/other.mcad"));
        assert_eq!(app.dialog_start_dir(), Some(PathBuf::from("/tmp/some/dir")));
    }

    #[test]
    fn remember_dialog_dir_stores_parent_of_confirmed_path() {
        let mut app = McadApp::new();
        assert_eq!(app.last_dialog_dir, None);

        app.remember_dialog_dir(Path::new("/home/user/project/output.dxf"));

        assert_eq!(
            app.last_dialog_dir,
            Some(PathBuf::from("/home/user/project"))
        );
    }

    // --- M8タスク41-2（Recent メニュー・最近使ったファイル）---
    //
    // rfd のネイティブダイアログは headless で開けないため、[`McadApp::open_document_at`]
    // （ダイアログ非依存の読込本体）と [`McadApp::save_to`] を実在の一時ファイルへ
    // 直接呼び出して検証する（`config.rs` の `unique_temp_path` と同じ流儀）。

    /// テストごとに一意な一時 `.mcad` ファイルパスを作る
    /// （`std::env::temp_dir()/mcad-app-test-main/<counter>-<name>.mcad`）。
    fn unique_temp_mcad_path(name: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join("mcad-app-test-main");
        std::fs::create_dir_all(&dir).expect("create temp test dir");
        dir.join(format!("{n}-{name}.mcad"))
    }

    #[test]
    fn open_document_at_success_updates_recent_files() {
        let path = unique_temp_mcad_path("open-recent");
        save_mcad(&Document::new(), &path).expect("write a real .mcad file to open");

        let mut app = McadApp::new();
        assert!(app.config.recent_files.is_empty());

        app.open_document_at(&path, 0.0);

        assert_eq!(app.current_path, Some(path.clone()));
        assert!(!app.is_dirty());
        assert_eq!(app.config.recent_files.first(), Some(&path));
    }

    #[test]
    fn open_document_at_failure_leaves_document_and_recent_files_unchanged() {
        let path = unique_temp_mcad_path("does-not-exist");
        let mut app = McadApp::new();
        let path_before = app.current_path.clone();

        app.open_document_at(&path, 0.0);

        assert_eq!(app.current_path, path_before);
        assert!(app.config.recent_files.is_empty());
    }

    #[test]
    fn save_to_success_updates_recent_files() {
        let path = unique_temp_mcad_path("save-recent");
        let mut app = McadApp::new();
        assert!(app.config.recent_files.is_empty());

        app.save_to(&path, 0.0);

        assert!(path.exists());
        assert_eq!(app.current_path, Some(path.clone()));
        assert_eq!(app.config.recent_files.first(), Some(&path));
    }

    #[test]
    fn request_open_recent_defers_to_modal_when_dirty() {
        let path = unique_temp_mcad_path("dirty-defer");
        save_mcad(&Document::new(), &path).expect("write a real .mcad file to open");

        let mut app = McadApp::new();
        // ドキュメントに変更を加えて dirty にする。
        app.document
            .apply(Command::AddLayer(Layer::new("Extra", Rgb::new(0, 0, 0))))
            .expect("AddLayer should succeed on a fresh document");
        assert!(app.is_dirty());

        app.request_open_recent(path.clone(), 0.0);

        // dirty のときはダイアログ非依存の読込を即座に呼ばず、モーダルへ遷移する。
        assert_eq!(app.confirm_state, ConfirmState::ConfirmingOpenRecent);
        assert_eq!(app.pending_recent_path, Some(path));
        assert_ne!(app.current_path, Some(PathBuf::new()));
    }

    #[test]
    fn request_open_recent_opens_immediately_when_not_dirty() {
        let path = unique_temp_mcad_path("clean-immediate");
        save_mcad(&Document::new(), &path).expect("write a real .mcad file to open");

        let mut app = McadApp::new();
        assert!(!app.is_dirty());

        app.request_open_recent(path.clone(), 0.0);

        // dirty でないときは即座に open_document_at を呼び、実ファイルを読み込む。
        assert_eq!(app.confirm_state, ConfirmState::Idle);
        assert_eq!(app.current_path, Some(path));
    }

    #[test]
    fn confirming_open_recent_prompt_is_some() {
        assert!(ConfirmState::ConfirmingOpenRecent.prompt().is_some());
    }

    // --- M7ツールのコミット失敗時の状態保持（Codex adversarial review 2026-07-26 指摘の回帰） ---
    //
    // `handle_tool_input` は `Document::apply(cmd)` が `Err`（レイヤーロック等）のとき、
    // 以前は無条件に `*tool = tool_kind.spawn()` でツールを作り直していた。M7の4ツール
    // （トリム・延長・フィレット・分割）は DESIGN.md M7設計判断6により失敗時も状態据え置き
    // （境界・1本目の線分は選び直さない）という再挑戦フローを規定しており、それが崩れて
    // いた。ここでは `handle_tool_input` の実際のブランチ（`ToolKind::keeps_state_on_commit_failure`
    // による分岐と、`Tool::on_commit_failed` の呼び出し）を模して検証する。
    //
    // `egui::Response` を要する `handle_tool_input` 自体は headless では構築できないため、
    // その `Err` 分岐が呼ぶのと同じ2つの公開経路（`ToolKind::keeps_state_on_commit_failure`
    // / `Tool::on_commit_failed`）を直接使い、内部状態は `Tool` トレイトの公開メソッド
    // （`on_shape_pick`・`take_commit_selection`）だけを通して黒箱的に確認する。

    /// カレントレイヤーをロックする（`mcad-core` の `lock_layer` テストヘルパーと同じ手法）。
    fn lock_current_layer(document: &mut Document) -> mcad_core::LayerId {
        let layer = document.current_layer();
        let mut props = document.layer(layer).unwrap().clone();
        props.locked = true;
        document
            .apply(Command::SetLayerProps { id: layer, props })
            .unwrap();
        layer
    }

    #[test]
    fn trim_tool_keeps_waiting_target_after_locked_layer_commit_failure() {
        // 境界 x=1 の縦線、対象 (0,0)-(2,0) の横線。ロック前に対象を追加してからレイヤーを
        // ロックし、確定を失敗させる。
        let mut document = Document::new();
        let target_shape = Shape::Line(LineSeg::new(Point2::new(0.0, 0.0), Point2::new(2.0, 0.0)));
        let target_id = document
            .apply(Command::AddEntity(Entity::new(
                target_shape.clone(),
                document.current_layer(),
                Style::inherited(),
            )))
            .unwrap()
            .entities[0];
        let layer = lock_current_layer(&mut document);
        let entity_count_before = document.entity_count();

        let mut tool = ToolKind::Trim.spawn().expect("Trim always spawns a tool");
        assert_eq!(
            tool.on_shape_pick(tool::ShapePick {
                id: target_id,
                shape: Shape::Line(LineSeg::new(Point2::new(1.0, -5.0), Point2::new(1.0, 5.0))),
                click: Point2::new(1.0, 0.5),
                layer,
                style: Style::inherited(),
            }),
            ToolResult::Continue,
            "1クリック目は境界を採取するだけ"
        );

        let target = tool::ShapePick {
            id: target_id,
            shape: target_shape,
            click: Point2::new(1.5, 0.0),
            layer,
            style: Style::inherited(),
        };
        let ToolResult::Commit(cmd) = tool.on_shape_pick(target.clone()) else {
            panic!("expected Commit");
        };
        assert!(
            document.apply(cmd).is_err(),
            "ロックレイヤーなので確定は失敗する"
        );
        assert_eq!(document.entity_count(), entity_count_before, "文書は不変");

        // 本来の handle_tool_input の Err 分岐と同じ処理: 作り直さず後始末フックだけ呼ぶ。
        assert!(ToolKind::Trim.keeps_state_on_commit_failure());
        tool.on_commit_failed();

        // 境界がまだ known なら、同じ対象を再ピックしても「新しい境界の指定」
        // （Continue）ではなく「対象への確定」（Commit）になる。ツールを作り直して
        // いた旧実装ではここが Continue に戻ってしまい、境界を選び直す羽目になる。
        let retry = tool.on_shape_pick(target);
        assert!(
            matches!(retry, ToolResult::Commit(_)),
            "境界を持ち越したまま別対象へ再挑戦できるはず、got {retry:?}"
        );
    }

    #[test]
    fn fillet_tool_keeps_first_line_after_locked_layer_commit_failure() {
        // 直角コーナー: a=(0,0)-(10,0), b=(0,0)-(0,10)。半径2で接点(2,0)/(0,2)。
        let mut document = Document::new();
        let a_shape = Shape::Line(LineSeg::new(Point2::new(0.0, 0.0), Point2::new(10.0, 0.0)));
        let b_shape = Shape::Line(LineSeg::new(Point2::new(0.0, 0.0), Point2::new(0.0, 10.0)));
        let a_id = document
            .apply(Command::AddEntity(Entity::new(
                a_shape.clone(),
                document.current_layer(),
                Style::inherited(),
            )))
            .unwrap()
            .entities[0];
        let b_id = document
            .apply(Command::AddEntity(Entity::new(
                b_shape.clone(),
                document.current_layer(),
                Style::inherited(),
            )))
            .unwrap()
            .entities[0];
        let layer = lock_current_layer(&mut document);

        let mut tool = ToolKind::Fillet
            .spawn()
            .expect("Fillet always spawns a tool");
        tool.set_radius_input(Some(2.0));
        assert_eq!(
            tool.on_shape_pick(tool::ShapePick {
                id: a_id,
                shape: a_shape,
                click: Point2::new(8.0, 0.0),
                layer,
                style: Style::inherited(),
            }),
            ToolResult::Continue,
            "1本目のピックは状態を進めるだけ"
        );

        let second = tool::ShapePick {
            id: b_id,
            shape: b_shape,
            click: Point2::new(0.0, 8.0),
            layer,
            style: Style::inherited(),
        };
        let ToolResult::Commit(cmd) = tool.on_shape_pick(second.clone()) else {
            panic!("expected Commit");
        };
        assert!(
            document.apply(cmd).is_err(),
            "ロックレイヤーなので確定は失敗する"
        );

        assert!(ToolKind::Fillet.keeps_state_on_commit_failure());
        tool.on_commit_failed();

        // 1本目を持ち越していれば、2本目の再ピックはまた確定（Commit）になる。作り直して
        // いた旧実装では WaitingFirstLine に戻り、この2本目ピックは「1本目の選び直し」
        // （Continue、しかも b は線分なので通ってしまう）になっていた。
        let retry = tool.on_shape_pick(second);
        assert!(
            matches!(retry, ToolResult::Commit(_)),
            "1本目を持ち越したまま2本目だけ選び直せるはず、got {retry:?}"
        );
    }

    #[test]
    fn split_tool_can_retry_after_locked_layer_commit_failure() {
        let mut document = Document::new();
        let shape = Shape::Line(LineSeg::new(Point2::new(0.0, 0.0), Point2::new(10.0, 0.0)));
        let id = document
            .apply(Command::AddEntity(Entity::new(
                shape.clone(),
                document.current_layer(),
                Style::inherited(),
            )))
            .unwrap()
            .entities[0];
        let layer = lock_current_layer(&mut document);
        let entity_count_before = document.entity_count();

        let mut tool = ToolKind::Split.spawn().expect("Split always spawns a tool");
        let pick = tool::ShapePick {
            id,
            shape,
            click: Point2::new(5.0, 0.0),
            layer,
            style: Style::inherited(),
        };
        let ToolResult::Commit(cmd) = tool.on_shape_pick(pick.clone()) else {
            panic!("expected Commit");
        };
        assert!(
            document.apply(cmd).is_err(),
            "ロックレイヤーなので確定は失敗する"
        );
        assert_eq!(document.entity_count(), entity_count_before, "文書は不変");

        assert!(ToolKind::Split.keeps_state_on_commit_failure());
        tool.on_commit_failed();

        // 分割は単一状態なので、失敗後の再ピックがそのまま確定できることを確認する。
        let retry = tool.on_shape_pick(pick);
        assert!(
            matches!(retry, ToolResult::Commit(_)),
            "失敗後も同じ対象へ再挑戦できるはず、got {retry:?}"
        );
    }

    #[test]
    fn trim_tool_commit_failure_does_not_leak_stale_selection_flag() {
        // 回帰(A-3): 2断片トリムの確定がロックで失敗しても、選択集合の載せ替えフラグ
        // （`select_new_entities`）が立ったまま残ってはいけない。残ると、次に成功した
        // 無関係の確定（1断片トリム）が `take_commit_selection` に誤って消費され、
        // 空の `NewIds.entities` で選択集合が壊れる。
        let mut document = Document::new();

        // 境界1: 円 中心(1,0) 半径0.5。対象(0,0)-(2,0) の中間をクリックすると2断片に割れる
        // （select_new_entities が立つ経路）。この対象はロックレイヤー上に置く。
        let locked_target_shape =
            Shape::Line(LineSeg::new(Point2::new(0.0, 0.0), Point2::new(2.0, 0.0)));
        let locked_target_id = document
            .apply(Command::AddEntity(Entity::new(
                locked_target_shape.clone(),
                document.current_layer(),
                Style::inherited(),
            )))
            .unwrap()
            .entities[0];
        let locked_layer = lock_current_layer(&mut document);

        // 別レイヤー（ロックなし）に、同じ境界円（中心(1,0) 半径0.5）と1点だけ交わる
        // 対象 (1,0)-(1,5) を置く（片端が円内、もう片端が円外なので交点は1つだけ）。
        // クリックは円外側の1点なので、その側が捨てられ1断片トリムになる。
        let unlocked_layer = document
            .apply(Command::AddLayer(mcad_core::Layer::new(
                "unlocked",
                mcad_core::Rgb::WHITE,
            )))
            .unwrap()
            .layers[0];
        let free_target_shape =
            Shape::Line(LineSeg::new(Point2::new(1.0, 0.0), Point2::new(1.0, 5.0)));
        let free_target_id = document
            .apply(Command::AddEntity(Entity::new(
                free_target_shape.clone(),
                unlocked_layer,
                Style::inherited(),
            )))
            .unwrap()
            .entities[0];

        let mut tool = ToolKind::Trim.spawn().expect("Trim always spawns a tool");

        // 1本目: 円境界を選び、ロックされた対象を中間クリックして2断片トリムを試みる。
        assert_eq!(
            tool.on_shape_pick(tool::ShapePick {
                id: locked_target_id,
                shape: Shape::Circle(mcad_geom::Circle::new(Point2::new(1.0, 0.0), 0.5)),
                click: Point2::new(1.5, 0.0),
                layer: locked_layer,
                style: Style::inherited(),
            }),
            ToolResult::Continue
        );
        let ToolResult::Commit(cmd) = tool.on_shape_pick(tool::ShapePick {
            id: locked_target_id,
            shape: locked_target_shape,
            click: Point2::new(1.0, 0.0),
            layer: locked_layer,
            style: Style::inherited(),
        }) else {
            panic!("expected Commit(Batch) for the 2-piece trim");
        };
        assert!(
            document.apply(cmd).is_err(),
            "ロックレイヤーなので確定は失敗する"
        );

        assert!(ToolKind::Trim.keeps_state_on_commit_failure());
        tool.on_commit_failed();

        // 2本目: 境界は同じ円のまま（状態はトリムの規約どおり WaitingTarget に留まって
        // いる）、ロックされていない対象を円外側でクリックして1断片トリムする。
        let ToolResult::Commit(cmd) = tool.on_shape_pick(tool::ShapePick {
            id: free_target_id,
            shape: free_target_shape,
            click: Point2::new(1.0, 3.0),
            layer: unlocked_layer,
            style: Style::inherited(),
        }) else {
            panic!("expected Commit(ModifyEntity) for the 1-piece trim");
        };
        let new_ids = document.apply(cmd).expect("unlocked layer commit succeeds");

        // 1断片トリムは選択集合を触らないはず。フラグが漏れていれば、ここで
        // `Some(new_ids.entities.clone())`（空の Vec）を誤って返してしまう。
        assert_eq!(
            tool.take_commit_selection(&new_ids),
            None,
            "失敗した2断片トリムのフラグが後続の無関係な確定へ漏れてはいけない"
        );
    }

    // ---- レイヤー重ね順（既定レイヤーセット・描画順・Front/Back） ----

    /// レイヤー名を重ね順（奥→手前）で並べて返す。
    fn ordered_layer_names(document: &Document) -> Vec<String> {
        document
            .layers_in_order()
            .into_iter()
            .map(|(_, layer)| layer.name.clone())
            .collect()
    }

    #[test]
    fn fresh_document_has_default_layer_set_in_order_with_clean_history() {
        let document = fresh_document(config::Config::default().default_sheet_meta());

        // "0"（デフォルト、order=0）が最背面で、既定レイヤーが配列順に手前へ載る。
        let mut expected = vec!["0".to_owned()];
        expected.extend(
            DEFAULT_EXTRA_LAYERS
                .iter()
                .map(|(name, ..)| (*name).to_owned()),
        );
        assert_eq!(ordered_layer_names(&document), expected);
        assert_eq!(
            document
                .layers_in_order()
                .into_iter()
                .map(|(_, layer)| layer.order)
                .collect::<Vec<_>>(),
            (0..=DEFAULT_EXTRA_LAYERS.len() as i32).collect::<Vec<_>>()
        );

        // 規定4-1準拠の線種・線幅がそのまま反映されている（M9 タスク53）。
        for (name, color, linetype, width_mm) in DEFAULT_EXTRA_LAYERS {
            let layer_id = layer_named(&document, name).expect("layer must exist");
            let layer = document.layer(layer_id).unwrap();
            assert_eq!(layer.color, color);
            assert_eq!(layer.linetype, linetype);
            assert_eq!(layer.width_mm.mm(), width_mm);
        }

        // カレントレイヤーは既定の作図対象である「外形線」（"0" のままではない）。
        let outline_layer = layer_named(&document, DEFAULT_CURRENT_LAYER_NAME).unwrap();
        assert_eq!(document.current_layer(), outline_layer);
        assert_ne!(document.current_layer(), document.default_layer());
        assert_eq!(document.layer(document.default_layer()).unwrap().name, "0");

        // 既定レイヤー・カレントレイヤーの適用は履歴に残らず、世代は基準点（0）に戻っている。
        assert!(!document.can_undo());
        assert!(!document.can_redo());
        assert_eq!(document.generation(), 0);
        assert_eq!(document.entity_count(), 0);
    }

    #[test]
    fn layer_named_finds_exact_match_and_returns_none_otherwise() {
        let document = fresh_document(config::Config::default().default_sheet_meta());

        let dim_layer = layer_named(&document, DIM_LAYER_NAME).expect("寸法線 must exist");
        assert_eq!(document.layer(dim_layer).unwrap().name, DIM_LAYER_NAME);

        let text_layer = layer_named(&document, TEXT_LAYER_NAME).expect("文字 must exist");
        assert_eq!(document.layer(text_layer).unwrap().name, TEXT_LAYER_NAME);

        assert_eq!(layer_named(&document, "存在しないレイヤー"), None);
        // 部分一致は不可（完全一致のみ）。
        assert_eq!(layer_named(&document, "寸法"), None);
    }

    #[test]
    fn resolve_tool_layer_falls_back_to_current_layer_when_named_layer_is_absent() {
        // Document::new() ベース（読込相当）は "0" のみなので、寸法・文字とも
        // current_layer へフォールバックする（読込図面でレイヤーを増やさない）。
        let document = Document::new();
        assert_eq!(
            resolve_tool_layer(&document, ToolKind::DimLinear),
            document.current_layer()
        );
        assert_eq!(
            resolve_tool_layer(&document, ToolKind::DimRadial),
            document.current_layer()
        );
        assert_eq!(
            resolve_tool_layer(&document, ToolKind::DimDiameter),
            document.current_layer()
        );
        assert_eq!(
            resolve_tool_layer(&document, ToolKind::Text),
            document.current_layer()
        );
        assert_eq!(
            resolve_tool_layer(&document, ToolKind::Line),
            document.current_layer()
        );
    }

    #[test]
    fn resolve_tool_layer_uses_named_layer_when_present_in_fresh_document() {
        let document = fresh_document(config::Config::default().default_sheet_meta());

        let dim_layer = layer_named(&document, DIM_LAYER_NAME).unwrap();
        assert_eq!(
            resolve_tool_layer(&document, ToolKind::DimLinear),
            dim_layer
        );
        assert_eq!(
            resolve_tool_layer(&document, ToolKind::DimRadial),
            dim_layer
        );
        assert_eq!(
            resolve_tool_layer(&document, ToolKind::DimDiameter),
            dim_layer
        );

        let text_layer = layer_named(&document, TEXT_LAYER_NAME).unwrap();
        assert_eq!(resolve_tool_layer(&document, ToolKind::Text), text_layer);

        // それ以外のツールは常にカレントレイヤー（"外形線"）。
        assert_eq!(
            resolve_tool_layer(&document, ToolKind::Line),
            document.current_layer()
        );
    }

    // ---- 設定永続化（M8 タスク41-1） ----

    #[test]
    fn fresh_document_applies_non_default_sheet_meta_without_leaving_undo_history() {
        let sheet = SheetMeta {
            unit: mcad_core::Unit::Millimeter,
            scale: Scale::new(1, 2).unwrap(),
            paper: PaperSize::A2,
            orientation: Orientation::Portrait,
            title_block: TitleBlockKind::C,
            fields: mcad_core::TitleBlockFields::default(),
            frame_visible: false,
        };
        let document = fresh_document(sheet.clone());

        assert_eq!(document.sheet(), &sheet);
        // レイヤー追加と同じく、SetSheet の適用も Ctrl+Z で巻き戻せてはいけない。
        assert!(!document.can_undo());
        assert!(!document.can_redo());
        assert_eq!(document.generation(), 0);
    }

    #[test]
    fn with_config_applies_toggles_and_default_sheet_and_reports_warning() {
        let startup = config::Startup {
            config: config::Config {
                snap_enabled: false,
                ortho_enabled: true,
                paper_display_enabled: true,
                default_paper: PaperSize::A1,
                default_orientation: Orientation::Portrait,
                default_scale: Scale::new(1, 5).unwrap(),
                default_title_block: config::TitleBlockChoice::A,
                recent_files: Vec::new(),
                plot_color_mode: plot::PlotColorMode::default(),
            },
            path: Some(PathBuf::from("/tmp/mcad-app-test/config.json")),
            warning: Some("something went wrong".to_owned()),
        };
        let app = McadApp::with_config(startup);

        assert!(!app.config.snap_enabled);
        assert!(app.config.ortho_enabled);
        assert!(app.config.paper_display_enabled);
        assert_eq!(app.document.sheet().paper, PaperSize::A1);
        assert_eq!(app.document.sheet().orientation, Orientation::Portrait);
        assert_eq!(app.document.sheet().scale, Scale::new(1, 5).unwrap());
        assert_eq!(app.document.sheet().title_block, TitleBlockKind::A);
        assert_eq!(
            app.config_path,
            Some(PathBuf::from("/tmp/mcad-app-test/config.json"))
        );
        let status = app.status.expect("warning should be surfaced on startup");
        assert_eq!(status.text, "something went wrong");
    }

    #[test]
    fn new_matches_previous_hardcoded_defaults() {
        let app = McadApp::new();

        assert!(app.config.snap_enabled);
        assert!(!app.config.ortho_enabled);
        assert!(!app.config.paper_display_enabled);
        assert_eq!(app.config, config::Config::default());
        assert_eq!(app.config_path, None);
        assert!(app.status.is_none());
        assert_eq!(
            app.document.sheet(),
            &config::Config::default().default_sheet_meta()
        );
    }

    #[test]
    fn new_and_new_document_share_the_default_layer_set() {
        // 起動時と Ctrl+N は同じ既定セットを持つ（[`McadApp::new`] の doc）。
        let started = McadApp::new();
        let mut app = McadApp::new();
        app.new_document(0.0);

        assert_eq!(
            ordered_layer_names(&app.document),
            ordered_layer_names(&started.document)
        );
        assert!(
            !app.is_dirty(),
            "既定レイヤーの追加で dirty になってはいけない"
        );
        assert!(
            !app.document.can_undo(),
            "既定レイヤーの追加を Ctrl+Z で巻き戻せてはいけない"
        );
    }

    #[test]
    fn loading_a_document_does_not_inject_default_layers() {
        // 既定セットは「新規文書のテンプレート」であって読込時には足さない（§5）。
        // rfd を開かない DXF import 経路（apply_imported_dxf）で固定する。
        // `.mcad` の open_document も同じく `load_mcad` の戻り値をそのまま代入する。
        let mut app = McadApp::new();
        assert!(app.document.layer_count() > 1, "起動時は既定セットを持つ");

        let imported = Document::new();
        let imported_layers = imported.layer_count();
        app.apply_imported_dxf(
            ImportSummary {
                document: imported,
                skipped_entities: 0,
                clamped_line_widths: 0,
            },
            0.0,
        );

        assert_eq!(app.document.layer_count(), imported_layers);
        assert_eq!(ordered_layer_names(&app.document), vec!["0".to_owned()]);
    }

    #[test]
    fn bring_to_front_order_picks_max_plus_one_and_is_noop_at_the_front() {
        // 他レイヤーの最大 + 1。
        assert_eq!(bring_to_front_order(0, [1, 5, 3]), 6);
        // order が非連続でも「最大 + 1」で足りる（詰め直しは不要）。
        assert_eq!(bring_to_front_order(-10, [-4, 40]), 41);
        // 既に最前面なら現在値のまま（SetLayerProps の no-op 判定に乗る）。
        assert_eq!(bring_to_front_order(7, [1, 5]), 7);
        // レイヤーが1枚だけ（他レイヤーなし）でも現在値のまま。
        assert_eq!(bring_to_front_order(3, []), 3);
        // 飽和: i32::MAX があってもオーバーフローでパニックしない。
        assert_eq!(bring_to_front_order(0, [i32::MAX]), i32::MAX);
        assert_eq!(bring_to_front_order(i32::MAX, [i32::MAX]), i32::MAX);
    }

    #[test]
    fn send_to_back_order_picks_min_minus_one_and_is_noop_at_the_back() {
        assert_eq!(send_to_back_order(4, [1, 5, 3]), 0);
        assert_eq!(send_to_back_order(40, [-4, 10]), -5);
        // 既に最背面なら現在値のまま。
        assert_eq!(send_to_back_order(-2, [1, 5]), -2);
        assert_eq!(send_to_back_order(3, []), 3);
        // 飽和。
        assert_eq!(send_to_back_order(0, [i32::MIN]), i32::MIN);
        assert_eq!(send_to_back_order(i32::MIN, [i32::MIN]), i32::MIN);
    }

    #[test]
    fn front_order_change_is_a_single_undo_step() {
        // Front/Back は専用コマンドを持たず SetLayerProps 1発なので undo も1回で戻る。
        let mut document = fresh_document(config::Config::default().default_sheet_meta());
        let back = document.default_layer();
        let others: Vec<i32> = document
            .layers()
            .filter(|(id, _)| *id != back)
            .map(|(_, layer)| layer.order)
            .collect();
        let before = ordered_layer_names(&document);

        let mut props = document.layer(back).unwrap().clone();
        props.order = bring_to_front_order(props.order, others);
        document
            .apply(Command::SetLayerProps { id: back, props })
            .unwrap();
        assert_eq!(
            ordered_layer_names(&document).last().map(String::as_str),
            Some("0"),
            "\"0\" が最前面へ来る"
        );

        assert!(document.undo());
        assert_eq!(ordered_layer_names(&document), before);
        assert!(!document.can_undo(), "1操作 = undo 1単位");
    }

    /// `front_order_plan` の純関数テスト: 他レイヤーに `i32::MAX` があるときは
    /// 単純な +1 が使えないため、全レイヤーを再正規化する計画を返す（Codex
    /// adversarial review の medium 指摘対応）。相対順序（`c` は `b` より奥）も
    /// 保たれることを確認する。
    #[test]
    fn front_order_plan_renormalizes_when_other_order_is_i32_max() {
        let mut document = Document::new();
        let a = document.default_layer(); // order 0、これを最前面へ動かす対象。
        let b = add_layer_with_order(&mut document, "b", i32::MAX);
        let c = add_layer_with_order(&mut document, "c", 5);

        let all_orders: Vec<(LayerId, i32)> =
            document.layers().map(|(id, l)| (id, l.order)).collect();
        let plan = front_order_plan(a, &all_orders);
        assert!(
            !plan.is_empty(),
            "i32::MAX が既にあるので再正規化が必要なはず"
        );

        let plan_map: std::collections::HashMap<LayerId, i32> = plan.into_iter().collect();
        let resolved = |id: LayerId| -> i32 {
            plan_map
                .get(&id)
                .copied()
                .unwrap_or_else(|| all_orders.iter().find(|(o, _)| *o == id).unwrap().1)
        };
        let a_new = resolved(a);
        let b_new = resolved(b);
        let c_new = resolved(c);
        assert!(a_new > b_new, "対象が唯一の最前面になるはず");
        assert!(a_new > c_new, "対象が唯一の最前面になるはず");
        // 相対順序: 元々 c(5) は b(i32::MAX) より奥だったので、再正規化後も奥のまま。
        assert!(c_new < b_new, "対象以外の相対順序は保たれるはず");
    }

    /// [`front_order_plan_renormalizes_when_other_order_is_i32_max`] の対称版。
    #[test]
    fn back_order_plan_renormalizes_when_other_order_is_i32_min() {
        let mut document = Document::new();
        let a = document.default_layer();
        let b = add_layer_with_order(&mut document, "b", i32::MIN);
        let c = add_layer_with_order(&mut document, "c", -5);

        let all_orders: Vec<(LayerId, i32)> =
            document.layers().map(|(id, l)| (id, l.order)).collect();
        let plan = back_order_plan(a, &all_orders);
        assert!(
            !plan.is_empty(),
            "i32::MIN が既にあるので再正規化が必要なはず"
        );

        let plan_map: std::collections::HashMap<LayerId, i32> = plan.into_iter().collect();
        let resolved = |id: LayerId| -> i32 {
            plan_map
                .get(&id)
                .copied()
                .unwrap_or_else(|| all_orders.iter().find(|(o, _)| *o == id).unwrap().1)
        };
        let a_new = resolved(a);
        let b_new = resolved(b);
        let c_new = resolved(c);
        assert!(a_new < b_new, "対象が唯一の最背面になるはず");
        assert!(a_new < c_new, "対象が唯一の最背面になるはず");
        // 相対順序: 元々 c(-5) は b(i32::MIN) より手前だったので、再正規化後も手前のまま。
        assert!(c_new > b_new, "対象以外の相対順序は保たれるはず");
    }

    /// UI 経路（[`layer_panel`] が組み立てる `Command::Batch`）を模して、境界値
    /// （他レイヤーが `i32::MAX`）で実際に Front を適用すると全レイヤーの order が
    /// 再正規化され、対象が真に最前面になること。また undo 1回で全レイヤーの order
    /// が完全に元へ戻ることを固定する（Codex adversarial review の medium 指摘）。
    #[test]
    fn bring_to_front_at_i32_max_boundary_renormalizes_and_undoes_in_one_step() {
        let mut document = Document::new();
        let default = document.default_layer(); // order 0
        let maxed = add_layer_with_order(&mut document, "maxed", i32::MAX);
        let mid = add_layer_with_order(&mut document, "mid", 3);
        // レイヤー追加自体を undo 対象から外す(検証したいのは Front 操作の undo 単位)。
        document.clear_history();

        let before: std::collections::HashMap<LayerId, i32> =
            document.layers().map(|(id, l)| (id, l.order)).collect();

        let all_orders: Vec<(LayerId, i32)> = before.iter().map(|(id, o)| (*id, *o)).collect();
        let plan = front_order_plan(default, &all_orders);
        assert!(!plan.is_empty());

        let cmds: Vec<Command> = plan
            .into_iter()
            .map(|(id, order)| {
                let mut props = document.layer(id).unwrap().clone();
                props.order = order;
                Command::SetLayerProps { id, props }
            })
            .collect();
        document.apply(Command::Batch(cmds)).unwrap();

        let default_order = document.layer(default).unwrap().order;
        assert!(default_order > document.layer(maxed).unwrap().order);
        assert!(default_order > document.layer(mid).unwrap().order);

        assert!(document.undo(), "Batch は undo 1回で戻るはず");
        let after: std::collections::HashMap<LayerId, i32> =
            document.layers().map(|(id, l)| (id, l.order)).collect();
        assert_eq!(
            after, before,
            "undo 1回で全レイヤーの order が完全に戻るはず"
        );
        assert!(!document.can_undo(), "1操作 = undo 1単位");
    }

    /// [`bring_to_front_at_i32_max_boundary_renormalizes_and_undoes_in_one_step`]
    /// の Back / `i32::MIN` 版。
    #[test]
    fn send_to_back_at_i32_min_boundary_renormalizes_and_undoes_in_one_step() {
        let mut document = Document::new();
        let default = document.default_layer(); // order 0
        let mined = add_layer_with_order(&mut document, "mined", i32::MIN);
        let mid = add_layer_with_order(&mut document, "mid", -3);
        // レイヤー追加自体を undo 対象から外す(検証したいのは Back 操作の undo 単位)。
        document.clear_history();

        let before: std::collections::HashMap<LayerId, i32> =
            document.layers().map(|(id, l)| (id, l.order)).collect();

        let all_orders: Vec<(LayerId, i32)> = before.iter().map(|(id, o)| (*id, *o)).collect();
        let plan = back_order_plan(default, &all_orders);
        assert!(!plan.is_empty());

        let cmds: Vec<Command> = plan
            .into_iter()
            .map(|(id, order)| {
                let mut props = document.layer(id).unwrap().clone();
                props.order = order;
                Command::SetLayerProps { id, props }
            })
            .collect();
        document.apply(Command::Batch(cmds)).unwrap();

        let default_order = document.layer(default).unwrap().order;
        assert!(default_order < document.layer(mined).unwrap().order);
        assert!(default_order < document.layer(mid).unwrap().order);

        assert!(document.undo(), "Batch は undo 1回で戻るはず");
        let after: std::collections::HashMap<LayerId, i32> =
            document.layers().map(|(id, l)| (id, l.order)).collect();
        assert_eq!(
            after, before,
            "undo 1回で全レイヤーの order が完全に戻るはず"
        );
        assert!(!document.can_undo(), "1操作 = undo 1単位");
    }

    /// 指定レイヤー上に、AABB が `visible` に必ず入る点エンティティを1つ追加する。
    fn add_point(document: &mut Document, layer: LayerId, x: f64) -> EntityId {
        document
            .apply(Command::AddEntity(Entity::new(
                Shape::Point(Point2::new(x, 0.0)),
                layer,
                Style::inherited(),
            )))
            .unwrap()
            .entities[0]
    }

    /// 指定した重ね順のレイヤーを追加する。
    fn add_layer_with_order(document: &mut Document, name: &str, order: i32) -> LayerId {
        let mut layer = Layer::new(name, Rgb::WHITE);
        layer.order = order;
        document.apply(Command::AddLayer(layer)).unwrap().layers[0]
    }

    /// 全エンティティを含む十分大きな可視 AABB。
    fn whole_world() -> Aabb {
        Aabb::new(Point2::new(-1000.0, -1000.0), Point2::new(1000.0, 1000.0))
    }

    #[test]
    fn draw_order_follows_layer_order_and_is_stable_within_a_layer() {
        let mut document = Document::new();
        let base = document.default_layer(); // order = 0
        let front = add_layer_with_order(&mut document, "front", 5);
        let back = add_layer_with_order(&mut document, "back", -5);

        // 追加順は base, front, back, base, front（レイヤー順とはあえてずらす）。
        let base_first = add_point(&mut document, base, 0.0);
        let front_first = add_point(&mut document, front, 1.0);
        let back_only = add_point(&mut document, back, 2.0);
        let base_second = add_point(&mut document, base, 3.0);
        let front_second = add_point(&mut document, front, 4.0);

        let drawn: Vec<EntityId> = entities_in_draw_order(&document, &whole_world(), 1.0)
            .into_iter()
            .map(|(id, _, _)| id)
            .collect();

        // レイヤーは order 昇順（back → base → front）、各レイヤー内は追加順のまま。
        assert_eq!(
            drawn,
            vec![
                back_only,
                base_first,
                base_second,
                front_first,
                front_second
            ]
        );
    }

    /// [`draw_order_follows_layer_order_and_is_stable_within_a_layer`] の隣。
    /// 異なるレイヤーが**同順位**（同じ `Layer::order`）のときの描画順を固定する。
    /// `entities_in_draw_order` は `layer.order` だけで安定ソートするため、同順位の
    /// レイヤー間ではレイヤーの境界を無視して [`Document::entities`] の反復順
    /// （＝エンティティの追加順）がそのまま残るはず（`.mcad` v3 は任意の `i32` order を
    /// 正規入力として受理するため、複数レイヤーが同じ order を持つ状態は普通に起こる）。
    #[test]
    fn draw_order_is_stable_across_layers_with_equal_order() {
        let mut document = Document::new();
        let base = document.default_layer(); // order = 0
        let same_a = add_layer_with_order(&mut document, "same_a", 0);
        let same_b = add_layer_with_order(&mut document, "same_b", 0);

        // 追加順は same_a, base, same_b, base（レイヤーをまたいで交互に追加する）。
        let a_first = add_point(&mut document, same_a, 0.0);
        let base_first = add_point(&mut document, base, 1.0);
        let b_first = add_point(&mut document, same_b, 2.0);
        let base_second = add_point(&mut document, base, 3.0);

        let drawn: Vec<EntityId> = entities_in_draw_order(&document, &whole_world(), 1.0)
            .into_iter()
            .map(|(id, _, _)| id)
            .collect();

        // 3レイヤーとも order = 0 なので、レイヤー境界に関係なく追加順のまま。
        assert_eq!(drawn, vec![a_first, base_first, b_first, base_second]);
    }

    #[test]
    fn draw_order_skips_hidden_layers_and_culled_entities() {
        let mut document = Document::new();
        let base = document.default_layer();
        let hidden = add_layer_with_order(&mut document, "hidden", 1);
        let visible_id = add_point(&mut document, base, 0.0);
        let hidden_id = add_point(&mut document, hidden, 0.0);
        let far_id = add_point(&mut document, base, 500.0);

        let mut props = document.layer(hidden).unwrap().clone();
        props.visible = false;
        document
            .apply(Command::SetLayerProps { id: hidden, props })
            .unwrap();

        // 可視範囲は原点付近のみ。非表示レイヤーと範囲外エンティティは落ちる。
        let view = Aabb::new(Point2::new(-10.0, -10.0), Point2::new(10.0, 10.0));
        let drawn: Vec<EntityId> = entities_in_draw_order(&document, &view, 1.0)
            .into_iter()
            .map(|(id, _, _)| id)
            .collect();
        assert_eq!(drawn, vec![visible_id]);
        assert!(!drawn.contains(&hidden_id));
        assert!(!drawn.contains(&far_id));
    }

    // ---- M8 タスク36: 線幅解決の回帰テスト（DESIGN.md 設計判断6 検収(b)） ----
    //
    // 旧描画は `stroke_px = max(width_px, 1.0)`（ズーム非依存の固定 px）だった。
    // 判断6 は「旧 `.mcad` の線幅（px）を `width_mm = 0.35mm * max(width_px, 1.0)`
    // へ移行し、`zoom = 1 / 0.35`（k=1.0 の 1:1 図面）で `resolve_stroke_px` が
    // 旧描画と厳密一致する」ことを要求する。ここでは 35b の移行規則が生成する
    // 具体的な `width_mm` 値（クランプなしの4ケース）を直接使い、
    // `resolve_stroke_px(width_mm, k=1.0, zoom) == max(width_px, 1.0)` を検証する。

    /// 旧 `.mcad` の `width_px`（px）を、判断6 規則2・3の実効表示幅換算で
    /// `width_mm`（紙 mm）へ変換する（クランプ前提: 呼び出し側が範囲内のケースのみ渡す）。
    fn legacy_width_px_to_width_mm(width_px: f32) -> f32 {
        0.35 * width_px.max(1.0)
    }

    #[test]
    fn resolve_stroke_px_matches_legacy_v3_migration_at_reference_zoom() {
        // 判断6 の7ケースのうち、上限クランプが掛からない（黙って値を変えない）
        // 4ケース。ケース1(1.0→None=ByLayer 0.35mm)は下の rule2 テストでカバーする。
        // ケース6(20.0→クランプ+計上)・ケース7(Infinity→パースエラー)は io 層
        // （35b）の担当でありここでは対象外。
        let legacy_width_px_cases = [2.5_f32, 0.5, -3.0, 5.0 / 0.35];
        let k = 1.0; // 1:1 図面。
        for width_px in legacy_width_px_cases {
            let width_mm = legacy_width_px_to_width_mm(width_px);
            let legacy_px = width_px.max(1.0);
            let resolved = resolve_stroke_px(width_mm, k, LEGACY_WIDTH_PX_REFERENCE_ZOOM);
            assert!(
                (resolved - legacy_px).abs() < 1e-3,
                "width_px={width_px}: resolved={resolved}, legacy={legacy_px}"
            );
        }
    }

    #[test]
    fn resolve_stroke_px_rule2_bylayer_matches_legacy_across_zoom_range() {
        // 判断6 規則2: 明示指定なし（旧 width_px == 1.0）は ByLayer(既定0.35mm) へ
        // 移行する。レイヤー幅が既定のままなら、`zoom <= 1/0.35` の全域で旧描画
        // （常に1px）と一致する（判断6 検収(b)後半）。
        let k = 1.0;
        let default_layer_width_mm = WidthMm::DEFAULT.mm();
        assert_eq!(default_layer_width_mm, 0.35);
        let sample_zooms = [
            0.01,
            0.1,
            0.5,
            1.0,
            2.0,
            LEGACY_WIDTH_PX_REFERENCE_ZOOM / 2.0,
            LEGACY_WIDTH_PX_REFERENCE_ZOOM,
        ];
        for zoom in sample_zooms {
            assert!(zoom <= LEGACY_WIDTH_PX_REFERENCE_ZOOM);
            let resolved = resolve_stroke_px(default_layer_width_mm, k, zoom);
            assert!(
                (resolved - 1.0).abs() < 1e-4,
                "zoom={zoom}: resolved={resolved}, expected legacy 1.0px"
            );
        }

        // 境界を超えると新しい可視差（高ズームで太くなる）が意図どおり現れる
        // （判断6「高ズームで太くなるのは M8 の目的そのもの」）。
        let beyond = resolve_stroke_px(
            default_layer_width_mm,
            k,
            LEGACY_WIDTH_PX_REFERENCE_ZOOM * 2.0,
        );
        assert!(beyond > 1.0 + 1e-4);
    }

    #[test]
    fn resolve_stroke_px_clamps_to_min_stroke_px() {
        // ズームアウトで width_mm * k * zoom が 1px を割り込んでも下限で止まる
        // （細線が消えないための下限。出力側はこの下限を適用しない）。
        let resolved = resolve_stroke_px(WidthMm::MIN_MM, 1.0, 0.001);
        assert_eq!(resolved, MIN_STROKE_PX);
    }

    #[test]
    fn resolve_stroke_px_grows_with_zoom_beyond_min() {
        // 判断6 検収(c): 高ズームで太くなることの土台となる単調性。
        let low = resolve_stroke_px(1.0, 1.0, 1.0);
        let high = resolve_stroke_px(1.0, 1.0, 10.0);
        assert!(high > low);
    }

    #[test]
    fn resolve_stroke_px_with_toggle_off_is_always_min_stroke_px() {
        // タスク36b: OFF なら width_mm・k・zoom によらず常に MIN_STROKE_PX
        // （タスク36 以前と同じ固定 1px）。
        let cases = [
            (0.35_f32, 1.0_f64, 1.0_f64),
            (WidthMm::MIN_MM, 1.0, 0.001),
            (2.0, 5.0, 100.0),
            (0.35, 1.0, LEGACY_WIDTH_PX_REFERENCE_ZOOM * 2.0),
        ];
        for (width_mm, k, zoom) in cases {
            let resolved = resolve_stroke_px_with_toggle(false, width_mm, k, zoom);
            assert_eq!(
                resolved, MIN_STROKE_PX,
                "width_mm={width_mm}, k={k}, zoom={zoom}"
            );
        }
    }

    #[test]
    fn resolve_stroke_px_with_toggle_on_matches_resolve_stroke_px() {
        // タスク36b: ON はタスク36 の既存挙動（`resolve_stroke_px`）と厳密一致する。
        let cases = [
            (0.35_f32, 1.0_f64, 1.0_f64),
            (WidthMm::MIN_MM, 1.0, 0.001),
            (2.0, 5.0, 100.0),
            (0.35, 1.0, LEGACY_WIDTH_PX_REFERENCE_ZOOM * 2.0),
        ];
        for (width_mm, k, zoom) in cases {
            let toggled = resolve_stroke_px_with_toggle(true, width_mm, k, zoom);
            let legacy = resolve_stroke_px(width_mm, k, zoom);
            assert_eq!(toggled, legacy, "width_mm={width_mm}, k={k}, zoom={zoom}");
        }
    }

    // ---- M8 タスク37 / M9 タスク49-3: dim_sizes（紙基準表示トグル・スタイル追従）----

    /// 既定とは別サイズのスタイル（文字 5.0mm・矢 6.0mm）。既定（3.5 / 3.0）と両方の
    /// 値が違うので、「たまたま一致していて差が出ない」状態を検出できる。
    fn large_dim_style() -> DimStyle {
        DimStyle {
            text_height_mm: 5.0,
            arrow_len_mm: 6.0,
            ..DimStyle::DEFAULT
        }
    }

    #[test]
    fn dim_sizes_off_matches_legacy_screen_fixed_px_regardless_of_k_and_style() {
        // OFF は k にもスタイルにも依存しない（画面固定 px モード。現行挙動の回帰固定）。
        for style in [DimStyle::DEFAULT, large_dim_style()] {
            for k in [0.5_f64, 1.0, 2.0] {
                for zoom in [0.1_f64, 1.0, 10.0] {
                    let (arrow_len, text_height) = dim_sizes(&style, false, k, zoom);
                    assert!(
                        (arrow_len - DIM_ARROW_PX / zoom).abs() < 1e-9,
                        "k={k}, zoom={zoom}"
                    );
                    assert!(
                        (text_height - DIM_TEXT_PX / zoom).abs() < 1e-9,
                        "k={k}, zoom={zoom}"
                    );
                }
            }
        }
    }

    #[test]
    fn dim_sizes_on_scales_the_style_paper_mm_with_k_and_ignores_zoom() {
        for style in [DimStyle::DEFAULT, large_dim_style()] {
            for k in [0.5_f64, 1.0, 2.0] {
                for zoom in [0.1_f64, 1.0, 10.0] {
                    let (arrow_len, text_height) = dim_sizes(&style, true, k, zoom);
                    assert!(
                        (arrow_len - style.arrow_len_mm * k).abs() < 1e-9,
                        "k={k}, zoom={zoom}"
                    );
                    assert!(
                        (text_height - style.text_height_mm * k).abs() < 1e-9,
                        "k={k}, zoom={zoom}"
                    );
                }
            }
        }
    }

    #[test]
    fn dim_sizes_on_at_scale_one_to_one_matches_the_style_paper_mm() {
        for zoom in [0.1_f64, 1.0, 10.0] {
            let (arrow_len, text_height) = dim_sizes(&DimStyle::DEFAULT, true, 1.0, zoom);
            assert!((arrow_len - DimStyle::DEFAULT.arrow_len_mm).abs() < 1e-9);
            assert!((text_height - DimStyle::DEFAULT.text_height_mm).abs() < 1e-9);
        }
    }

    /// **M9 タスク49-3 の要点**: スタイルの文字高さ・矢先長を変えると、画面に実際に
    /// 描かれる矢先の長さ・文字高さが変わる（判定や組版の比率だけが変わるのではない）。
    ///
    /// 画面描画そのもの（Painter）はテストできないので、描画関数が受け取る展開結果
    /// （`draw_dim_expansion` の入力そのもの）で固定する。出力側の対は
    /// `plot::dim_style_drives_the_plotted_annotation_sizes`。
    #[test]
    fn dim_style_drives_the_on_screen_annotation_sizes() {
        let k = 2.0;
        let dim = DimLinear {
            p1: Point2::ORIGIN,
            p2: Point2::new(200.0, 0.0),
            offset: 20.0,
            annotation: mcad_core::DimAnnotation::default(),
        };

        for style in [DimStyle::DEFAULT, large_dim_style()] {
            let render = dim_render(&style, true, k, 1.0);
            let ex = dimension::expand_linear(&dim, render);

            // 矢先の実長（先端 → 後端）はスタイルの矢先長 × k。
            let [tip, a, b] = ex.arrows[0];
            let drawn_arrow = tip.distance(a.midpoint(b));
            assert!(
                (drawn_arrow - style.arrow_len_mm * k).abs() < 1e-9,
                "arrow {drawn_arrow} for style {style:?}"
            );
            // 文字高さ（`draw_text` へワールド長として渡る値）はスタイルの文字高さ × k。
            assert!(
                (ex.texts[0].height - style.text_height_mm * k).abs() < 1e-9,
                "text {} for style {style:?}",
                ex.texts[0].height
            );
        }
    }

    // ---- M8 タスク37: text_world_aabb（Text の紙基準ワールド AABB）----

    #[test]
    fn text_world_aabb_at_k_one_matches_core_aabb() {
        // k=1 恒等（回転 Text 含む）。
        for angle in [0.0_f64, 0.3, std::f64::consts::FRAC_PI_2] {
            let text = TextGeom {
                anchor: Point2::new(10.0, 20.0),
                content: "abc".to_owned(),
                height: 5.0,
                angle,
            };
            let expected = EntityGeom::Text(text.clone()).aabb();
            let actual = text_world_aabb(&text, 1.0);
            assert!((actual.min.x - expected.min.x).abs() < 1e-9);
            assert!((actual.min.y - expected.min.y).abs() < 1e-9);
            assert!((actual.max.x - expected.max.x).abs() < 1e-9);
            assert!((actual.max.y - expected.max.y).abs() < 1e-9);
        }
    }

    #[test]
    fn text_world_aabb_at_k_two_matches_aabb_of_doubled_height_text() {
        let text = TextGeom {
            anchor: Point2::new(10.0, 20.0),
            content: "abc".to_owned(),
            height: 5.0,
            angle: 0.4,
        };
        let scaled = TextGeom {
            height: text.height * 2.0,
            ..text.clone()
        };
        let expected = EntityGeom::Text(scaled).aabb();
        let actual = text_world_aabb(&text, 2.0);
        assert!((actual.min.x - expected.min.x).abs() < 1e-9);
        assert!((actual.min.y - expected.min.y).abs() < 1e-9);
        assert!((actual.max.x - expected.max.x).abs() < 1e-9);
        assert!((actual.max.y - expected.max.y).abs() < 1e-9);
    }

    #[test]
    fn document_aabb_includes_text_scaled_bounds_at_sheet_scale() {
        // 1:2 の図面では Text の表示上のワールド AABB は height×2 分だけ広がる
        // （タスク37 検収項目7）。
        let mut document = Document::new();
        document
            .apply(Command::SetSheet(mcad_core::SheetMeta {
                scale: mcad_core::Scale::new(1, 2).unwrap(),
                ..Default::default()
            }))
            .unwrap();
        let layer = document.current_layer();
        document
            .apply(Command::AddEntity(Entity::new(
                EntityGeom::Text(TextGeom {
                    anchor: Point2::new(0.0, 0.0),
                    content: "hi".to_owned(),
                    height: 5.0,
                    angle: 0.0,
                }),
                layer,
                Style::inherited(),
            )))
            .unwrap();
        let aabb = document_aabb(&document).expect("document has a text entity");
        // 1:1 解釈での高さは 5.0、1:2（k=2）では 10.0 まで広がる。
        assert!(aabb.max.y >= 10.0 - 1e-9, "aabb={aabb:?}");
    }

    #[test]
    fn dash_pattern_px_is_none_for_continuous() {
        assert!(dash_pattern_px(Linetype::Continuous, 1.0, 100.0).is_none());
    }

    #[test]
    fn dash_pattern_px_falls_back_to_solid_when_period_too_small() {
        // ズームアウトでダッシュ周期が MIN_DASH_PERIOD_PX を割り込むと実線
        // フォールバックする（判断5 検収(c)。極小シェイプの大量生成を防ぐ）。
        let tiny_zoom = 1e-6;
        assert!(dash_pattern_px(Linetype::Dashed, 1.0, tiny_zoom).is_none());
        assert!(dash_pattern_px(Linetype::DashDot, 1.0, tiny_zoom).is_none());
        assert!(dash_pattern_px(Linetype::DashDotDot, 1.0, tiny_zoom).is_none());
    }

    #[test]
    fn dash_pattern_px_scales_dashed_pattern_with_k_and_zoom() {
        let k = 2.0;
        let zoom = 3.0;
        let (dash_lengths, gap_lengths) = dash_pattern_px(Linetype::Dashed, k, zoom).unwrap();
        assert_eq!(dash_lengths, vec![DASH_PATTERN_MM[0] * (k * zoom) as f32]);
        assert_eq!(gap_lengths, vec![DASH_PATTERN_MM[1] * (k * zoom) as f32]);
    }

    #[test]
    fn dash_pattern_px_splits_dash_dot_pattern_into_alternating_arrays() {
        let k = 1.0;
        let zoom = 10.0;
        let (dash_lengths, gap_lengths) = dash_pattern_px(Linetype::DashDot, k, zoom).unwrap();
        let scale = (k * zoom) as f32;
        assert_eq!(
            dash_lengths,
            vec![
                DASH_DOT_PATTERN_MM[0] * scale,
                DASH_DOT_PATTERN_MM[2] * scale
            ]
        );
        assert_eq!(
            gap_lengths,
            vec![
                DASH_DOT_PATTERN_MM[1] * scale,
                DASH_DOT_PATTERN_MM[3] * scale
            ]
        );
    }

    // ---- M8 タスク36 差し戻し対応: ビューポートクリップの回帰テスト ----
    //
    // Codex 一次レビュー指摘: `stroke_polyline` がスクリーン座標の点列をクリップ
    // せずダッシュ化すると、高ズームで画面外まで伸びる線の全長が周期の桁違いに
    // 大きくなり、生成シェイプ数が爆発してフリーズしうる。クリップ後の総延長を
    // ビューポート寸法程度に抑えることで再発を防ぐ。

    #[test]
    fn clip_segment_to_rect_keeps_fully_inside_segment_unchanged() {
        let clip = Rect::from_min_size(Pos2::ZERO, egui::vec2(100.0, 100.0));
        let (a, b) = (Pos2::new(10.0, 10.0), Pos2::new(90.0, 80.0));
        let (ca, cb) = clip_segment_to_rect(a, b, clip).unwrap();
        assert!(points_nearly_equal(ca, a));
        assert!(points_nearly_equal(cb, b));
    }

    #[test]
    fn clip_segment_to_rect_drops_segment_entirely_outside() {
        let clip = Rect::from_min_size(Pos2::ZERO, egui::vec2(100.0, 100.0));
        let (a, b) = (Pos2::new(200.0, 200.0), Pos2::new(300.0, 300.0));
        assert!(clip_segment_to_rect(a, b, clip).is_none());
    }

    #[test]
    fn clip_segment_to_rect_truncates_segment_crossing_boundary() {
        let clip = Rect::from_min_size(Pos2::ZERO, egui::vec2(100.0, 100.0));
        let (a, b) = (Pos2::new(-50.0, 50.0), Pos2::new(150.0, 50.0));
        let (ca, cb) = clip_segment_to_rect(a, b, clip).unwrap();
        assert!(points_nearly_equal(ca, Pos2::new(0.0, 50.0)));
        assert!(points_nearly_equal(cb, Pos2::new(100.0, 50.0)));
    }

    #[test]
    fn clip_polyline_runs_is_empty_for_line_entirely_off_screen() {
        // (b) 完全に画面外の線分はクリップ後 0 区間になる。
        let clip = Rect::from_min_size(Pos2::ZERO, egui::vec2(2000.0, 1500.0));
        let points = vec![Pos2::new(1.0e7, 1.0e7), Pos2::new(2.0e7, 2.0e7)];
        assert!(clip_polyline_runs(&points, clip).is_empty());
    }

    #[test]
    fn clip_polyline_runs_matches_unclipped_for_fully_visible_line() {
        // (c) 画面内に完全に収まる線分はクリップ前後で同じ点列になる。
        let clip = Rect::from_min_size(Pos2::ZERO, egui::vec2(2000.0, 1500.0));
        let points = vec![Pos2::new(10.0, 10.0), Pos2::new(500.0, 800.0)];
        let runs = clip_polyline_runs(&points, clip);
        assert_eq!(runs.len(), 1);
        assert!(points_nearly_equal(runs[0][0], points[0]));
        assert!(points_nearly_equal(*runs[0].last().unwrap(), points[1]));
    }

    #[test]
    fn clip_polyline_runs_splits_into_multiple_runs_when_leaving_and_reentering() {
        // 画面外へ出て別の場所で再び入る折れ線は、独立した複数区間になる
        // （stroke_polyline はこの各区間ごとにダッシュ位相をリセットする）。
        let clip = Rect::from_min_size(Pos2::ZERO, egui::vec2(100.0, 100.0));
        let points = vec![
            Pos2::new(50.0, -50.0),  // 画面外(上)
            Pos2::new(50.0, 50.0),   // 画面内(中央) — 1本目の区間の終端
            Pos2::new(200.0, 50.0),  // 画面外(右) — ここで区間が切れる
            Pos2::new(200.0, 500.0), // 画面外のまま
            Pos2::new(50.0, 500.0),  // 画面外のまま
            Pos2::new(50.0, 50.0),   // 画面内へ再突入 — 2本目の区間の始端
            Pos2::new(50.0, 100.0),  // 画面内(下端)
        ];
        let runs = clip_polyline_runs(&points, clip);
        assert_eq!(runs.len(), 2, "runs={runs:?}");
    }

    #[test]
    fn clip_bounds_dash_generation_for_line_extending_far_beyond_viewport() {
        // (a) 高ズームで画面外まで大きくはみ出す線（クリップなしなら 1e7px 級の
        // 全長になりダッシュ片数が爆発しうる）でも、クリップ後の総延長は
        // ビューポート寸法程度に収まり、見積もりダッシュ片数が MAX_DASH_SEGMENTS を
        // 大幅に下回る。
        let clip = Rect::from_min_size(Pos2::ZERO, egui::vec2(2000.0, 1500.0));
        let points = vec![Pos2::new(-1.0e7, 500.0), Pos2::new(1.0e7, 500.0)];
        let period = 4.5_f32; // stroke_polyline のマージン計算と同じ想定周期。
        let margin = 1.0 + period; // 線幅1px相当 + 1周期分。
        let runs = clip_polyline_runs(&points, clip.expand(margin));
        let total_len = total_path_length(&runs);
        // クリップ矩形の幅(2000px)+マージン程度が上限の目安。
        assert!(total_len < 3000.0, "total_len={total_len}");
        let estimated_dashes = total_len / period;
        assert!(
            estimated_dashes < MAX_DASH_SEGMENTS as f32,
            "estimated_dashes={estimated_dashes}"
        );
    }

    // -----------------------------------------------------------------
    // 寸法パネル（M9タスク50-2）: 記号コンボの選択肢・公差入力パース・注記編集
    // -----------------------------------------------------------------

    #[test]
    fn common_allowed_symbols_intersects_the_grammar_matrix() {
        // 単一種別は DimKind::allowed_symbols とそのまま一致する
        // （唯一の出典を二重に持っていないことの確認）。
        assert_eq!(
            common_allowed_symbols(std::iter::once(DimKind::Linear)),
            DimKind::Linear.allowed_symbols().to_vec()
        );
        // 長さ×半径は共通記号なし（φ/Sφ/□/C/t と R/SR/CR は素な集合）。
        assert!(common_allowed_symbols([DimKind::Linear, DimKind::Radial].into_iter()).is_empty());
        // 長さ×直径は φ/Sφ が共通。
        assert_eq!(
            common_allowed_symbols([DimKind::Linear, DimKind::Diameter].into_iter()),
            vec![DimSymbol::Diameter, DimSymbol::SphereDiameter]
        );
        // 空イテレータは空を返す。
        assert!(common_allowed_symbols(std::iter::empty()).is_empty());
    }

    #[test]
    fn parse_symmetric_tolerance_input_parses_and_rejects() {
        assert_eq!(
            parse_symmetric_tolerance_input(" 0.05 "),
            Ok(SizeTolerance::Symmetric(0.05))
        );
        assert!(parse_symmetric_tolerance_input("abc").is_err());
        assert!(parse_symmetric_tolerance_input("").is_err());
    }

    #[test]
    fn parse_deviations_tolerance_input_parses_and_rejects() {
        assert_eq!(
            parse_deviations_tolerance_input("0.2", "-0.1"),
            Ok(SizeTolerance::Deviations {
                upper: 0.2,
                lower: -0.1,
            })
        );
        assert!(parse_deviations_tolerance_input("x", "-0.1").is_err());
        assert!(parse_deviations_tolerance_input("0.2", "y").is_err());
    }

    /// テスト用に長さ寸法エンティティを1件追加し、その ID を返す。
    fn add_test_dim_linear(document: &mut Document) -> EntityId {
        let layer = document.current_layer();
        let ids = document
            .apply(Command::AddEntity(Entity::new(
                EntityGeom::DimLinear(DimLinear {
                    p1: Point2::new(0.0, 0.0),
                    p2: Point2::new(10.0, 0.0),
                    offset: 5.0,
                    annotation: DimAnnotation::default(),
                }),
                layer,
                Style::inherited(),
            )))
            .unwrap();
        ids.entities[0]
    }

    #[test]
    fn build_annotation_edit_commands_skips_noop_changes() {
        let mut document = Document::new();
        let id = add_test_dim_linear(&mut document);
        let live = vec![(id, DimKind::Linear, DimAnnotation::default())];

        let (cmds, errs) = build_annotation_edit_commands(&document, &live, |_| {});
        assert!(cmds.is_empty());
        assert!(errs.is_empty());
    }

    #[test]
    fn build_annotation_edit_commands_rejects_symbol_not_allowed_for_kind() {
        let mut document = Document::new();
        let id = add_test_dim_linear(&mut document);
        let live = vec![(id, DimKind::Linear, DimAnnotation::default())];

        // R（半径記号）は長さ寸法に許されない（DimKind::allowed_symbols）。
        let (cmds, errs) = build_annotation_edit_commands(&document, &live, |a| {
            a.symbol = Some(DimSymbol::Radius);
        });
        assert!(cmds.is_empty(), "invalid symbol must not produce a command");
        assert_eq!(errs.len(), 1);
    }

    #[test]
    fn build_annotation_edit_commands_builds_modify_entity_for_valid_change() {
        let mut document = Document::new();
        let id = add_test_dim_linear(&mut document);
        let live = vec![(id, DimKind::Linear, DimAnnotation::default())];

        let (cmds, errs) = build_annotation_edit_commands(&document, &live, |a| {
            a.symbol = Some(DimSymbol::Diameter);
        });
        assert!(errs.is_empty());
        assert_eq!(cmds.len(), 1);
        assert!(matches!(cmds[0], Command::ModifyEntity { .. }));

        // 実際に適用できる（core 側の検証も一致していることの確認）。
        assert!(document.apply(Command::Batch(cmds)).is_ok());
        let EntityGeom::DimLinear(dim) = &document.entity(id).unwrap().geom else {
            panic!("expected DimLinear");
        };
        assert_eq!(dim.annotation.symbol, Some(DimSymbol::Diameter));
    }

    // -----------------------------------------------------------------
    // 寸法スタイルダイアログ（M9タスク50-3）
    // -----------------------------------------------------------------

    #[test]
    fn dim_style_dialog_round_trips_the_default_style() {
        let dialog = DimStyleDialogState::from_style(&DimStyle::default());
        assert_eq!(dialog.to_dim_style(), Ok(DimStyle::default()));
    }

    #[test]
    fn dim_style_dialog_reflects_edited_fields() {
        let mut dialog = DimStyleDialogState::from_style(&DimStyle::default());
        dialog.decimals = "3".to_string();
        dialog.trim_trailing_zeros = false;
        let style = dialog.to_dim_style().expect("valid edit");
        assert_eq!(style.decimals, 3);
        assert!(!style.trim_trailing_zeros);
        assert_ne!(style, DimStyle::default());
    }

    #[test]
    fn dim_style_dialog_rejects_unparsable_numbers() {
        let mut dialog = DimStyleDialogState::from_style(&DimStyle::default());
        dialog.text_height_mm = "abc".to_string();
        assert!(dialog.to_dim_style().is_err());
    }

    #[test]
    fn dim_style_dialog_rejects_values_the_style_validator_rejects() {
        // パースは成功するが DimStyle::validate() が拒否する値（矢の長さ 0 は非正）。
        let mut dialog = DimStyleDialogState::from_style(&DimStyle::default());
        dialog.arrow_len_mm = "0".to_string();
        assert!(dialog.to_dim_style().is_err());
    }

    // -----------------------------------------------------------------
    // sync_dim_edit_state（差し戻し対応: 選択切替時の入力欄再同期）
    // -----------------------------------------------------------------

    /// `sync_dim_edit_state` のテスト用に、寸法パネルの入力欄一式をまとめた作業用構造体。
    #[derive(Default)]
    struct DimEditBuffers {
        edit_target: Vec<EntityId>,
        tol_editing: Option<DimTolKindUi>,
        tol_symmetric_input: String,
        tol_upper_input: String,
        tol_lower_input: String,
        tol_fit_input: String,
        tol_input_error: Option<String>,
        decimals_editing: bool,
        decimals_input: String,
        decimals_input_error: Option<String>,
        value_override_input: String,
    }

    impl DimEditBuffers {
        fn sync(&mut self, live: &[(EntityId, DimKind, DimAnnotation)]) -> bool {
            sync_dim_edit_state(
                &mut self.edit_target,
                live,
                &mut self.tol_editing,
                &mut self.tol_symmetric_input,
                &mut self.tol_upper_input,
                &mut self.tol_lower_input,
                &mut self.tol_fit_input,
                &mut self.tol_input_error,
                &mut self.decimals_editing,
                &mut self.decimals_input,
                &mut self.decimals_input_error,
                &mut self.value_override_input,
            )
        }
    }

    #[test]
    fn sync_dim_edit_state_resyncs_buffers_when_selection_changes() {
        // (a) 寸法Aの入力途中に選択をBへ切り替えると、Aの入力がBへ持ち越されない
        // （Codex adversarial review 2026-09-04 指摘。main.rs の元コードは入力欄が
        // 選択集合に紐付いておらず、A用の値がBへ誤って ModifyEntity されうる欠陥があった）。
        let mut document = Document::new();
        let id_a = add_test_dim_linear(&mut document);
        let id_b = add_test_dim_linear(&mut document);

        let mut buffers = DimEditBuffers::default();
        let live_a = vec![(id_a, DimKind::Linear, DimAnnotation::default())];
        assert!(buffers.sync(&live_a), "初回同期は必ず変化扱い");

        // Aの入力途中を模す（確定ボタンはまだ押していない）。
        buffers.tol_editing = Some(DimTolKindUi::Symmetric);
        buffers.tol_symmetric_input = "0.05".to_string();
        buffers.decimals_editing = true;
        buffers.decimals_input = "3".to_string();
        buffers.value_override_input = "A用の入力".to_string();

        // 選択をBへ切り替え。
        let live_b = vec![(id_b, DimKind::Linear, DimAnnotation::default())];
        let changed = buffers.sync(&live_b);

        assert!(changed, "選択集合が変わったので再同期が起きる");
        assert_eq!(
            buffers.tol_editing, None,
            "A の公差編集モードが残ってはいけない"
        );
        assert!(
            !buffers.decimals_editing,
            "A の桁数編集状態が B へ残ってはいけない"
        );
        assert!(
            buffers.tol_symmetric_input.is_empty(),
            "A の対称公差入力が残ってはいけない: {:?}",
            buffers.tol_symmetric_input
        );
        assert!(
            buffers.decimals_input.is_empty(),
            "B は decimals_override が None（共通値なし扱い）なので空欄になる: {:?}",
            buffers.decimals_input
        );
        assert!(
            buffers.value_override_input.is_empty(),
            "A の表示値上書き入力が B へ持ち越されてはいけない: {:?}",
            buffers.value_override_input
        );

        // 選択が変わらないフレームでは何もしない（入力途中の文字列を消さない）。
        buffers.tol_symmetric_input = "0.10".to_string();
        let changed_again = buffers.sync(&live_b);
        assert!(!changed_again);
        assert_eq!(buffers.tol_symmetric_input, "0.10");
    }

    #[test]
    fn sync_dim_edit_state_loads_existing_value_override_without_clearing_it() {
        // (b) 既に value_override を持つ寸法を選ぶと、入力欄にその値が読み込まれる
        // （読み込まれないと、空欄のまま「確定」を押して既存値を消してしまう事故につながる）。
        let mut document = Document::new();
        let id = add_test_dim_linear(&mut document);
        let annotation = DimAnnotation {
            value_override: Some("5-10".to_string()),
            ..DimAnnotation::default()
        };
        let live = vec![(id, DimKind::Linear, annotation.clone())];

        let mut buffers = DimEditBuffers::default();
        buffers.sync(&live);

        assert_eq!(
            buffers.value_override_input, "5-10",
            "既存の表示値上書きが入力欄へ読み込まれること"
        );

        // dim_panel 側のガード契約: 読み込まれた値のまま「確定」相当の処理（空でなければ
        // 適用）をしても、既存値が意図せず None へ落ちない（消去は「解除」ボタンのみ）。
        let trimmed = buffers.value_override_input.trim();
        assert!(!trimmed.is_empty(), "既存値が空欄化されていないこと");
        assert_eq!(Some(trimmed.to_string()), annotation.value_override);
    }

    // -----------------------------------------------------------------
    // A: 桁数「スタイルに従う」チェックの状態遷移（差し戻し対応）
    // -----------------------------------------------------------------

    #[test]
    fn dim_decimals_follow_style_checked_does_not_snap_back_right_after_unchecking() {
        // 外す前: 編集中でなく、全件が None（スタイルに従う）→ チェック済み。
        assert!(dim_decimals_follow_style_checked(false, true));

        // 外した直後: まだ decimals_override は適用していない（all_follow_style は
        // 依然 true）が、editing フラグが立っているので未チェックのまま
        // （＝これが無いと「残像」バグが起きる）。
        assert!(!dim_decimals_follow_style_checked(true, true));

        // 確定して Some(n) を適用した後: all_follow_style は false になり、
        // editing フラグはそのまま true → 引き続き未チェック（編集欄を出し続ける）。
        assert!(!dim_decimals_follow_style_checked(true, false));

        // 再チェック（editing フラグを false へ戻す）: all_follow_style も
        // None へ戻した結果 true になっていれば、チェック済み表示に戻る。
        assert!(dim_decimals_follow_style_checked(false, true));

        // 編集中でなくても、選択の一部/全部が既に明示上書き済みなら未チェック
        // （選択直後、ユーザー操作を経ずに欄が表示されるケース）。
        assert!(!dim_decimals_follow_style_checked(false, false));
    }

    #[test]
    fn dim_decimals_editing_flag_drives_the_full_toggle_confirm_recheck_sequence() {
        // A の状態遷移を実際の `DimEditBuffers`（sync_dim_edit_state と同じ経路）で
        // 一通り確認する: 外す→編集状態、確定→Some、再チェック→None。
        let mut document = Document::new();
        let id = add_test_dim_linear(&mut document);
        let mut buffers = DimEditBuffers::default();
        let mut live = vec![(id, DimKind::Linear, DimAnnotation::default())];
        buffers.sync(&live);

        // 初期状態: 選択中の寸法は decimals_override = None なのでチェック済み。
        let all_follow_style = live.iter().all(|(_, _, a)| a.decimals_override.is_none());
        assert!(dim_decimals_follow_style_checked(
            buffers.decimals_editing,
            all_follow_style
        ));

        // 「外す」操作（dim_panel のチェックボックス changed 分岐、follow_style=false 側）。
        buffers.decimals_editing = true;
        buffers.decimals_input = "3".to_string();
        // コマンドはまだ適用しない → 注記は不変。
        assert_eq!(live[0].2.decimals_override, None);
        assert!(!dim_decimals_follow_style_checked(
            buffers.decimals_editing,
            all_follow_style
        ));

        // 「確定」操作: decimals_override = Some(3) を適用する
        // （build_annotation_edit_commands 経由、実際の dim_panel と同じ経路）。
        let (cmds, errs) = build_annotation_edit_commands(&document, &live, |a| {
            a.decimals_override = Some(3);
        });
        assert!(errs.is_empty());
        document.apply(Command::Batch(cmds)).unwrap();
        let EntityGeom::DimLinear(dim) = &document.entity(id).unwrap().geom else {
            panic!("expected DimLinear");
        };
        assert_eq!(dim.annotation.decimals_override, Some(3));
        live[0].2 = dim.annotation.clone();
        let all_follow_style = live.iter().all(|(_, _, a)| a.decimals_override.is_none());
        // 確定後も editing フラグは維持され、欄は表示され続ける。
        assert!(buffers.decimals_editing);
        assert!(!dim_decimals_follow_style_checked(
            buffers.decimals_editing,
            all_follow_style
        ));

        // 「再チェック」操作: decimals_override = None を適用し、editing を終える。
        let (cmds, errs) = build_annotation_edit_commands(&document, &live, |a| {
            a.decimals_override = None;
        });
        assert!(errs.is_empty());
        document.apply(Command::Batch(cmds)).unwrap();
        buffers.decimals_editing = false;
        let EntityGeom::DimLinear(dim) = &document.entity(id).unwrap().geom else {
            panic!("expected DimLinear");
        };
        assert_eq!(dim.annotation.decimals_override, None);
        live[0].2 = dim.annotation.clone();
        let all_follow_style = live.iter().all(|(_, _, a)| a.decimals_override.is_none());
        assert!(dim_decimals_follow_style_checked(
            buffers.decimals_editing,
            all_follow_style
        ));
    }

    // -----------------------------------------------------------------
    // B: 選択集合の内訳表示（差し戻し対応）
    // -----------------------------------------------------------------

    /// `dim_combo_id_salt` が `ComboBox` の実際のポップアップ `Id` を見ていることの
    /// 回帰テスト（ヘッドレスの `egui::Context` でフレームを回す）。ボタン `Id` の
    /// 計算を `IdSalt::new` で包み忘れると `ComboBox::is_open` が常に `false` になり、
    /// 閉じてもエポックが進まない（2026-09-05 に実機で再現した不具合）。
    #[test]
    fn dim_combo_id_salt_advances_the_epoch_after_the_popup_closes() {
        let ctx = egui::Context::default();
        let observed = std::cell::Cell::new(("", 0u64));
        let popup_id = std::cell::Cell::new(egui::Id::NULL);
        let frame = |ctx: &egui::Context| {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                let salt = dim_combo_id_salt(ui, "test_combo");
                observed.set(salt);
                let button_id = ui.make_persistent_id(egui::IdSalt::new(salt));
                popup_id.set(button_id.with("popup"));
                egui::ComboBox::from_id_salt(salt)
                    .selected_text("x")
                    .show_ui(ui, |ui| {
                        let _ = ui.selectable_label(false, "a");
                    });
            });
        };
        frame(&ctx);
        frame(&ctx);
        assert_eq!(observed.get().1, 0, "開閉前はエポック 0");

        egui::Popup::open_id(&ctx, popup_id.get());
        frame(&ctx);
        frame(&ctx);
        assert_eq!(observed.get().1, 0, "開いている間はエポックを変えない");

        egui::Popup::close_id(&ctx, popup_id.get());
        frame(&ctx);
        frame(&ctx);
        assert_eq!(observed.get().1, 1, "閉じた直後にエポックが 1 進む");

        frame(&ctx);
        assert_eq!(observed.get().1, 1, "閉じたままでは進まない");
    }

    #[test]
    fn dim_selection_summary_counts_each_kind() {
        let mut document = Document::new();
        let linear = add_test_dim_linear(&mut document);
        let layer = document.current_layer();
        let radial = document
            .apply(Command::AddEntity(Entity::new(
                EntityGeom::DimRadial(DimRadial {
                    center: Point2::ORIGIN,
                    radius: 5.0,
                    leader_angle: 0.0,
                    annotation: DimAnnotation::default(),
                }),
                layer,
                Style::inherited(),
            )))
            .unwrap()
            .entities[0];
        let diameter = document
            .apply(Command::AddEntity(Entity::new(
                EntityGeom::DimDiameter(DimDiameter {
                    center: Point2::ORIGIN,
                    radius: 5.0,
                    angle: 0.0,
                    annotation: DimAnnotation::default(),
                }),
                layer,
                Style::inherited(),
            )))
            .unwrap()
            .entities[0];

        let live = vec![
            (linear, DimKind::Linear, DimAnnotation::default()),
            (radial, DimKind::Radial, DimAnnotation::default()),
            (diameter, DimKind::Diameter, DimAnnotation::default()),
        ];
        assert_eq!(
            dim_selection_summary(&live),
            "選択中: 3 件(長さ 1・半径 1・直径 1)"
        );

        // 単一種別のみ（長さ寸法だけを選び直したはずが、累積選択で直径寸法が
        // 残っている状況を模す: B の再現）。
        let mixed = vec![
            (linear, DimKind::Linear, DimAnnotation::default()),
            (diameter, DimKind::Diameter, DimAnnotation::default()),
        ];
        assert_eq!(
            dim_selection_summary(&mixed),
            "選択中: 2 件(長さ 1・半径 0・直径 1)"
        );
    }
}
