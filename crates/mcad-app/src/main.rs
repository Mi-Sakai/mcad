//! `mcad-app` — eguiアプリ本体（バイナリ）。
//!
//! M2タスク4（Viewport+描画）: 座標変換・ズーム/パン・グリッド表示・
//! エンティティ描画（カリング付き）を実装する。
//! M2タスク5（Toolフレームワーク+作図ツール）: Point/Line/Circle/Arc/Polyline の
//! 各ツール（`tool.rs`）を統合し、キーボードショートカットで切り替え、
//! クリック/Enter/Escで確定・キャンセルできるようにする。後続タスクで
//! 選択・編集ツールとスナップエンジンを追加する。

mod snap;
mod tool;
mod viewport;

use std::path::{Path, PathBuf};

use egui::{Color32, Key, Pos2, Rect, Stroke};

use mcad_core::{Command, Document, Layer, LayerId, Rgb, Style};
use mcad_geom::{Aabb, Arc, Point2, Polyline, Shape};
use mcad_io::{ImportSummary, load_dxf, load_mcad, save_dxf, save_mcad};

use tool::{
    ArcTool, CircleTool, DragPreview, InputEvent, LineTool, PointTool, PolylineTool, SelectTool,
    Tool, ToolCtx, ToolResult,
};
use viewport::Viewport;

/// 円弧をポリライン近似する際の分割数（固定値。将来ズーム適応分割は不要）。
const ARC_SEGMENTS: usize = 64;

/// ホイール1ノッチあたりのズーム倍率。
const WHEEL_ZOOM_SPEED: f64 = 0.0015;

/// グリッドの目標スクリーン間隔（ピクセル）。
const GRID_TARGET_PX: f64 = 48.0;

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

/// ステータスメッセージの表示時間（秒）。経過後は自動で消える。
const STATUS_MESSAGE_SECS: f64 = 5.0;

/// `.mcad` ファイルの拡張子（ファイルダイアログのフィルタ・拡張子補完の両方で使う）。
const MCAD_EXTENSION: &str = "mcad";

/// DXF ファイルの拡張子（ファイルダイアログのフィルタ・拡張子補完の両方で使う）。
const DXF_EXTENSION: &str = "dxf";

/// パス未定のドキュメントを「名前を付けて保存」する際の初期ファイル名。
const DEFAULT_FILE_NAME: &str = "Untitled.mcad";

/// DXF エクスポートダイアログの初期ファイル名（`current_path` が未定のとき）。
const DEFAULT_DXF_FILE_NAME: &str = "Untitled.dxf";

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

/// ステータスメッセージの文字色（エラー通知が主用途なので警告寄りの赤）。
const STATUS_MESSAGE_COLOR: Color32 = Color32::from_rgb(255, 120, 120);

/// 選択エンティティのハイライト色（確定済みエンティティ色とは別の強調色）。
const SELECTION_COLOR: Color32 = Color32::from_rgb(80, 200, 255);
/// 選択ハイライトの線の太さ。
const SELECTION_WIDTH: f32 = 2.5;
/// 矩形選択プレビューの塗り色（半透明）。
const RECT_FILL_COLOR: Color32 = Color32::from_rgba_premultiplied(30, 60, 90, 60);
/// 矩形選択プレビューの枠線色。
const RECT_OUTLINE_COLOR: Color32 = Color32::from_rgb(80, 160, 255);

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
        }
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
    /// スナップの有効/無効。`F3` でトグルする（既定は有効）。
    snap_enabled: bool,
    /// 直近フレームで作図ツールのカーソルがスナップした先。マーカー描画に使う。
    /// スナップしていない・作図ツール非アクティブ・スナップ無効のときは `None`。
    snap_marker: Option<snap::SnapResult>,
    /// ステータスバーに一時表示するメッセージ（主にコア操作のエラー通知）。
    /// [`STATUS_MESSAGE_SECS`] 経過で自動的に消える。
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
}

/// ステータスバーに一時表示するメッセージ。
struct StatusMessage {
    /// 表示する文言。
    text: String,
    /// 表示を開始した時刻（`egui::InputState::time`、秒）。
    shown_at: f64,
}

/// ステータスメッセージを設定する（既存の表示は上書き）。
fn set_status(status: &mut Option<StatusMessage>, now: f64, text: impl Into<String>) {
    *status = Some(StatusMessage {
        text: text.into(),
        shown_at: now,
    });
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
    fn new() -> Self {
        let document = Document::new();
        Self {
            // 起動直後（空文書）の世代を保存済み基準点とし、未保存扱いにしない。
            saved_generation: document.generation(),
            document,
            viewport: Viewport::new(),
            tool_kind: ToolKind::Select,
            tool: None,
            select_tool: SelectTool::default(),
            snap_enabled: true,
            snap_marker: None,
            status: None,
            current_path: None,
            confirm_state: ConfirmState::Idle,
            pending_zoom_fit: false,
        }
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
        self.snap_marker = None;
    }

    /// ファイル読込（`.mcad`/DXF共通）直後に呼ぶ。M4タスク13（起動状態とズームフィット）。
    ///
    /// 読込直後の時点ではキャンバスのスクリーン矩形がまだ確定していないため、
    /// フィット計算をその場では行えない。読み込んだドキュメントにエンティティが
    /// 1件以上あれば [`McadApp::pending_zoom_fit`] を立てて次フレームの `CentralPanel`
    /// （スクリーン矩形確定後）へ計算を委ねる。エンティティが0件（空の`.mcad`/DXF）なら
    /// フィット対象がないため、その場で [`Viewport::new`] の既定ビューへリセットする
    /// （DESIGN.md 6章タスク13: 「空文書は既定ビューへリセット」）。
    fn request_zoom_fit_after_load(&mut self) {
        if self.document.entity_count() > 0 {
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
        if self.is_dirty() {
            self.confirm_state = ConfirmState::ConfirmingNew;
        } else {
            self.new_document(now);
        }
    }

    /// Ctrl+O: 未保存の変更があれば確認モーダルを出し、なければ即座にファイル選択へ進む。
    fn request_open_document(&mut self, now: f64) {
        if self.is_dirty() {
            self.confirm_state = ConfirmState::ConfirmingOpen;
        } else {
            self.open_document(now);
        }
    }

    /// Ctrl+Shift+O: 未保存の変更があれば確認モーダルを出し、なければ即座に
    /// DXF ファイル選択へ進む。
    fn request_open_dxf(&mut self, now: f64) {
        if self.is_dirty() {
            self.confirm_state = ConfirmState::ConfirmingOpenDxf;
        } else {
            self.open_dxf(now);
        }
    }

    /// 真に空の新規ドキュメントへ置き換える（サンプルエンティティは一切追加しない。
    /// [`McadApp::new`] の doc 参照）。
    ///
    /// 未保存の変更があるかどうかは確認しない。呼び出し側（[`McadApp::request_new_document`]
    /// または確認モーダルの「破棄して続行」選択）が確認済みであることを前提とする。
    fn new_document(&mut self, now: f64) {
        self.document = Document::new();
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
        let Some(path) = rfd::FileDialog::new()
            .add_filter("mcad", &[MCAD_EXTENSION])
            .pick_file()
        else {
            return;
        };
        match load_mcad(&path) {
            Ok(doc) => {
                self.document = doc;
                self.current_path = Some(path);
                // load_mcad は再構築後に clear_history 済みで世代が基準点に戻っている。
                // 読込直後を未保存でない状態にするため、その世代へ合わせる。
                self.saved_generation = self.document.generation();
                self.reset_transient_ui_state();
                self.request_zoom_fit_after_load();
                set_status(&mut self.status, now, "Opened file");
            }
            Err(err) => {
                set_status(&mut self.status, now, format!("Open failed: {err}"));
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
        let Some(path) = rfd::FileDialog::new()
            .add_filter("dxf", &[DXF_EXTENSION])
            .pick_file()
        else {
            return;
        };
        match load_dxf(&path) {
            Ok(summary) => self.apply_imported_dxf(summary, now),
            Err(err) => {
                set_status(&mut self.status, now, format!("DXF import failed: {err}"));
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
        } = summary;
        self.document = document;
        self.current_path = None;
        self.saved_generation = DXF_IMPORT_SAVED_GENERATION_SENTINEL;
        self.reset_transient_ui_state();
        self.request_zoom_fit_after_load();
        let message = if skipped_entities > 0 {
            format!(
                "Imported DXF: {skipped_entities} entity(ies) skipped \
                 (unsupported type); layer locks are not restored from DXF"
            )
        } else {
            "Imported DXF file; layer locks are not restored from DXF".to_string()
        };
        set_status(&mut self.status, now, message);
    }

    /// Ctrl+E: ネイティブの保存ダイアログで選んだ先へ現在のドキュメントを DXF として
    /// エクスポートする。
    ///
    /// エクスポートは既存ドキュメントを変更しない読み取り専用操作なので、未保存の
    /// 変更があっても確認モーダルは出さない。成功しても `current_path`・
    /// `saved_generation` は一切変更しない（DESIGN.md 6章 設計判断1: DXFは交換用
    /// 形式であり「保存」とは別物として扱う。dirty 状態は変わらない）。
    fn export_dxf_file(&mut self, now: f64) {
        let default_name = self
            .current_path
            .as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|n| n.to_str())
            .map_or_else(
                || DEFAULT_DXF_FILE_NAME.to_string(),
                |stem| format!("{stem}.{DXF_EXTENSION}"),
            );
        let Some(path) = rfd::FileDialog::new()
            .add_filter("dxf", &[DXF_EXTENSION])
            .set_file_name(&default_name)
            .save_file()
        else {
            return;
        };
        let path = ensure_dxf_extension(path);
        match save_dxf(&self.document, &path) {
            Ok(()) => {
                set_status(&mut self.status, now, "Exported DXF file");
            }
            Err(err) => {
                set_status(&mut self.status, now, format!("DXF export failed: {err}"));
            }
        }
    }

    /// Ctrl+S: 開いているファイルパスへ上書き保存する。パスが未定なら
    /// 「名前を付けて保存」（[`McadApp::save_document_as`]）と同じ扱いにする。
    fn save_document(&mut self, now: f64) {
        let Some(path) = self.current_path.clone() else {
            self.save_document_as(now);
            return;
        };
        self.save_to(&path, now);
    }

    /// Ctrl+Shift+S: 常にネイティブの保存ダイアログを表示し、選んだ先へ保存する。
    /// 成功時は「現在開いているファイルパス」を選んだ先に更新する。
    fn save_document_as(&mut self, now: f64) {
        let default_name = self
            .current_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or(DEFAULT_FILE_NAME);
        let Some(path) = rfd::FileDialog::new()
            .add_filter("mcad", &[MCAD_EXTENSION])
            .set_file_name(default_name)
            .save_file()
        else {
            return;
        };
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
                set_status(&mut self.status, now, "Saved");
            }
            Err(err) => {
                set_status(&mut self.status, now, format!("Save failed: {err}"));
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

/// パスの拡張子が `.dxf`（大小無視）でなければ付け直す。
fn ensure_dxf_extension(path: PathBuf) -> PathBuf {
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(DXF_EXTENSION))
    {
        path
    } else {
        path.with_extension(DXF_EXTENSION)
    }
}

/// ドキュメント中の全エンティティを包む AABB。エンティティが1つもなければ `None`
/// （M4タスク13: ズームフィット対象の算出。[`Viewport::fit_to_aabb`] に渡す）。
fn document_aabb(document: &Document) -> Option<Aabb> {
    document
        .entities()
        .map(|(_, entity)| entity.geom.aabb())
        .reduce(|acc, bb| acc.union(&bb))
}

impl eframe::App for McadApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let now = ui.input(|i| i.time);

        // 未保存確認モーダル表示中（`confirm_state != Idle`）は、履歴操作・ファイル
        // 操作ショートカットを一切処理しない。モーダル表示中に Ctrl+N 等が先に走ると、
        // 例えば「閉じる」確認中に `confirm_state` が `ConfirmingNew` へ上書きされ、
        // モーダルの文言が終了確認から新規文書確認へすり替わってしまうため。
        if self.confirm_state == ConfirmState::Idle {
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
                self.select_tool.retain_alive(&self.document);
            }
            if redo_pressed && self.document.redo() {
                self.select_tool.retain_alive(&self.document);
            }

            // Ctrl+N/Ctrl+O/Ctrl+S/Ctrl+Shift+S: 新規/開く/保存/名前を付けて保存。
            // Ctrl+Shift+O/Ctrl+E: DXF を開く/DXF へ書き出す。
            // undo/redo と同様、ツール切替キー（`handle_tool_shortcut_keys`）は Ctrl 併用を
            // 無視するので衝突しない。
            let (
                new_pressed,
                open_pressed,
                save_pressed,
                save_as_pressed,
                open_dxf_pressed,
                export_dxf_pressed,
            ) = ui.input(|i| {
                let cmd = i.modifiers.command;
                (
                    cmd && !i.modifiers.shift && i.key_pressed(Key::N),
                    cmd && !i.modifiers.shift && i.key_pressed(Key::O),
                    cmd && !i.modifiers.shift && i.key_pressed(Key::S),
                    cmd && i.modifiers.shift && i.key_pressed(Key::S),
                    cmd && i.modifiers.shift && i.key_pressed(Key::O),
                    cmd && !i.modifiers.shift && i.key_pressed(Key::E),
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

        // モーダル表示中はツール切替（S/1/L/C/A/P）や F3（スナップ切替）も処理しない。
        // 特にツール切替はモーダルの裏で作図ツールが起動してしまう副作用があるため。
        if self.confirm_state == ConfirmState::Idle {
            handle_tool_shortcut_keys(
                ui,
                &mut self.tool_kind,
                &mut self.tool,
                &mut self.select_tool,
            );

            // F3 でスナップの有効/無効をトグルする（作図時の吸着を一時的に切りたい場面用）。
            if ui.input(|i| i.key_pressed(Key::F3)) {
                self.snap_enabled = !self.snap_enabled;
                if !self.snap_enabled {
                    self.snap_marker = None;
                }
            }
        }

        // 表示時間を過ぎたステータスメッセージは消す。
        if self
            .status
            .as_ref()
            .is_some_and(|m| now - m.shown_at > STATUS_MESSAGE_SECS)
        {
            self.status = None;
        }

        egui::Panel::top("tool_status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("File: {file_label}{dirty_marker}"));
                ui.separator();
                ui.label(format!("Tool: {}", self.tool_kind.label()));
                ui.separator();
                ui.label(format!(
                    "Snap: {}",
                    if self.snap_enabled { "ON" } else { "OFF" }
                ));
                ui.separator();
                // ステータスメッセージ（動的な成否通知）はヘルプ文字列より優先度が高いため、
                // 幅が狭くヘルプ文字列がクリップされてもステータスは見える位置に置く。
                if let Some(msg) = &self.status {
                    ui.colored_label(STATUS_MESSAGE_COLOR, &msg.text);
                    ui.separator();
                }
                ui.label(
                    "S=Select  1=Point  L=Line  C=Circle  A=Arc  P=Polyline  \
                     Del=Delete  Esc=Cancel  F3=Snap  Ctrl+Z=Undo  Ctrl+Y=Redo  \
                     Ctrl+N=New  Ctrl+O=Open  Ctrl+S=Save  Ctrl+Shift+S=Save As  \
                     Ctrl+Shift+O=Import DXF  Ctrl+E=Export DXF",
                );
            });
        });

        egui::Panel::right("layer_panel").show(ui, |ui| {
            layer_panel(ui, &mut self.document, &mut self.status, now);
        });

        egui::CentralPanel::default().show(ui, |ui| {
            let (rect, response) =
                ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());

            // M4タスク13: ファイル読込直後のズームフィットは、読込時点ではまだこの
            // スクリーン矩形（rect）が確定していないため実行できず、ここまで遅延させる
            // 必要がある（`request_zoom_fit_after_load` の doc 参照）。
            if self.pending_zoom_fit {
                if let Some(aabb) = document_aabb(&self.document) {
                    self.viewport.fit_to_aabb(aabb, rect);
                }
                self.pending_zoom_fit = false;
            }

            handle_pan_input(ui, &response, &mut self.viewport);
            handle_zoom_input(ui, &response, rect, &mut self.viewport);
            // 未保存確認モーダル表示中は、キャンバスへのクリック/ドラッグ/Delete/Enter/Esc
            // などを一切ツール・選択処理へ渡さない。素通りさせると、モーダルの裏で
            // エンティティが削除・作図確定されてしまう（Delete/Enter は egui::Modal が
            // 消費しないため）。パン/ズームは見るだけの操作なので許容する。
            if self.confirm_state == ConfirmState::Idle {
                if self.tool_kind == ToolKind::Select {
                    // 選択・編集モードではスナップを効かせない（設計判断は
                    // `handle_tool_input` の doc を参照）。マーカーも消す。
                    self.snap_marker = None;
                    handle_select_input(
                        ui,
                        &response,
                        rect,
                        &self.viewport,
                        &mut self.document,
                        &mut self.select_tool,
                        &mut self.status,
                        now,
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
                        self.snap_enabled,
                        &mut self.snap_marker,
                        &mut self.status,
                        now,
                    );
                }
            }

            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 0.0, Color32::from_gray(30));

            draw_grid(&painter, rect, &self.viewport);
            draw_entities(&painter, rect, &self.document, &self.viewport);
            draw_selection(
                &painter,
                rect,
                &self.document,
                &self.viewport,
                &self.select_tool,
            );
            if let Some(tool) = &self.tool {
                tool.draw_preview(&painter, rect, &self.viewport);
            }
            if let Some(marker) = &self.snap_marker {
                draw_snap_marker(&painter, rect, &self.viewport, marker);
            }
        });

        // 未保存確認モーダル（OS の閉じるボタン / Ctrl+N / Ctrl+O 共通、非ブロッキング）。
        // egui::Modal は最前面レイヤに背景付きで描かれるので、パネル群の後に描いてよい。
        // ユーザー可視文字列は ASCII 限定（egui 既定フォントは CJK 非対応）。
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
                                ConfirmState::Idle | ConfirmState::Closing => {}
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            self.confirm_state = ConfirmState::Idle;
                        }
                    });
                });
            // モーダル外クリック / Esc はキャンセル扱い（実行しない）。
            if modal.should_close() {
                self.confirm_state = ConfirmState::Idle;
            }
        }
    }
}

/// キーボードショートカットでアクティブツールを切り替える（DESIGN.md 3.4 のツール群）。
///
/// `S`=Select, `1`=Point, `L`=Line, `C`=Circle, `A`=Arc, `P`=Polyline。
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
        if i.key_pressed(Key::S) {
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
        }
    });
    if let Some(kind) = requested {
        *tool_kind = kind;
        *tool = kind.spawn();
        // 作図ツールへ移るときは選択を解除する（Select のままなら選択は保持）。
        if kind != ToolKind::Select {
            select_tool.clear_selection();
        }
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
    snap_enabled: bool,
    snap_marker: &mut Option<snap::SnapResult>,
    status: &mut Option<StatusMessage>,
    now: f64,
) {
    let Some(active) = tool.as_mut() else {
        return;
    };

    let ctx = ToolCtx {
        layer: document.current_layer(),
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
        let extra_points = active.snap_points();
        let (world, marker) = apply_snap(
            document,
            snap_enabled,
            raw,
            radius,
            grid_step,
            &extra_points,
        );
        *snap_marker = marker;
        let _ = active.on_input(&ctx, InputEvent::Move(world));
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
        let extra_points = active.snap_points();
        let (world, _) = apply_snap(
            document,
            snap_enabled,
            raw,
            radius,
            grid_step,
            &extra_points,
        );
        result = active.on_input(&ctx, InputEvent::Click(world));
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
                Ok(_) => {}
                Err(err) => {
                    set_status(status, now, format!("Commit failed: {err}"));
                    *tool = tool_kind.spawn();
                }
            }
        }
        ToolResult::Cancel => {
            *tool_kind = ToolKind::Select;
            *tool = None;
        }
        ToolResult::Continue => {}
    }
}

/// 右側のレイヤーパネル（DESIGN.md 3.4 の UI レイアウト）。
///
/// 一覧・カレント切替（ラジオ）・色変更・表示/ロック切替・追加/削除を提供する。
/// すべての変更は [`Command`] として [`Document::apply`] へ載せるため undo/redo の
/// 対象になる。削除の制約（デフォルト/カレント/非空レイヤーは不可）はコア側が
/// 検証し、失敗はステータスバーへ表示する。
///
/// ラベル等のユーザー可視文字列を ASCII に限定しているのは、egui の既定フォントが
/// CJK グリフを含まず日本語が豆腐（□）になるため。
fn layer_panel(
    ui: &mut egui::Ui,
    document: &mut Document,
    status: &mut Option<StatusMessage>,
    now: f64,
) {
    ui.heading("Layers");

    if ui.button("+ Add layer").clicked() {
        let n = document.layer_count();
        let color = LAYER_COLOR_PALETTE[n % LAYER_COLOR_PALETTE.len()];
        let cmd = Command::AddLayer(Layer::new(format!("Layer {n}"), color));
        if let Err(err) = document.apply(cmd) {
            set_status(status, now, format!("Add layer failed: {err}"));
        }
    }
    ui.separator();

    let current = document.current_layer();
    let default_layer = document.default_layer();
    // パネル描画中の可変借用を避けるため、レイヤー一覧のスナップショットを取り、
    // 操作から生じたコマンドは走査後に一括適用する。
    let layers: Vec<(LayerId, Layer)> = document
        .layers()
        .map(|(id, layer)| (id, layer.clone()))
        .collect();
    let mut pending: Vec<Command> = Vec::new();

    for (id, layer) in &layers {
        ui.horizontal(|ui| {
            // カレントレイヤー切替（ラジオ）。新規エンティティの投入先になる。
            if ui
                .radio(*id == current, "")
                .on_hover_text("Set current layer")
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

            let mut visible = layer.visible;
            if ui.checkbox(&mut visible, "show").changed() {
                let mut props = layer.clone();
                props.visible = visible;
                pending.push(Command::SetLayerProps { id: *id, props });
            }

            let mut locked = layer.locked;
            if ui.checkbox(&mut locked, "lock").changed() {
                let mut props = layer.clone();
                props.locked = locked;
                pending.push(Command::SetLayerProps { id: *id, props });
            }

            ui.label(&layer.name);

            // 削除。デフォルトレイヤーにはボタン自体を出さない（コア側でも拒否される）。
            // カレント・非空レイヤーの削除失敗はコアの検証に任せ、理由を表示する。
            if *id != default_layer && ui.button("x").on_hover_text("Delete layer").clicked() {
                pending.push(Command::RemoveLayer(*id));
            }
        });
    }

    for cmd in pending {
        if let Err(err) = document.apply(cmd) {
            set_status(status, now, format!("Layer operation failed: {err}"));
        }
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

/// 選択・編集モード（`tool_kind == Select`）のキャンバス入力を [`SelectTool`] へ渡す。
///
/// egui 組み込みのクリック/ドラッグ判定を利用し、単発クリック（＝選択）と
/// ドラッグ（＝矩形選択）を振り分ける。ドラッグは矩形選択専用で、選択集合を書き換える
/// だけ（Document は変更しない）。移動は `M`、複製は `Ctrl+D` の2クリック配置で行う。
/// 削除の確定コマンドは [`Document::apply`] で適用する（`Batch` の原子性・undo/redo 結線は
/// コア側に従う）。
///
/// - `Delete`/`Backspace`: 選択エンティティを 1 バッチで削除。適用成功時のみ選択を解除する。
/// - `M`: 選択集合の移動配置モードへ入る（基準点→配置先の2クリック、スナップ対応）。
/// - `Esc`: 進行中のドラッグ（または配置モード）を破棄（選択は変えない）。
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
    status: &mut Option<StatusMessage>,
    now: f64,
) {
    // ピック許容量（px）をワールド単位へ換算する。
    let tol = PICK_TOLERANCE_PX / viewport.zoom;

    // Delete / Backspace: 選択を 1 バッチで削除。ロックレイヤー混在時は Batch 原子性で
    // 全体失敗しうるので、apply が成功したときだけ選択を解除し、失敗は表示する。
    if ui.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace))
        && let Some(cmd) = select_tool.delete_command()
    {
        match document.apply(cmd) {
            Ok(_) => select_tool.clear_selection(),
            Err(err) => set_status(status, now, format!("Delete failed: {err}")),
        }
    }

    // Esc: 進行中のドラッグだけ破棄する（選択集合は変えない）。
    if ui.input(|i| i.key_pressed(Key::Escape)) {
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
            select_tool.on_drag_start(document, world, tol);
        } else if response.dragged_by(egui::PointerButton::Primary) {
            select_tool.on_drag(world);
        } else if response.drag_stopped_by(egui::PointerButton::Primary) {
            if let Some(cmd) = select_tool.on_drag_end(document, world) {
                // 移動の確定。ロックレイヤー混在時は Batch 原子性で全体が失敗し、
                // 何も動かない。失敗理由はステータスバーへ表示する。
                if let Err(err) = document.apply(cmd) {
                    set_status(status, now, format!("Move failed: {err}"));
                }
            }
        } else if response.clicked_by(egui::PointerButton::Primary) {
            select_tool.on_click(document, world, tol);
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

/// ビューポートの可視 AABB と交差するエンティティのみをカリングして描画する。
/// 非表示レイヤーのエンティティは描画しない。
fn draw_entities(painter: &egui::Painter, rect: Rect, document: &Document, viewport: &Viewport) {
    let visible = viewport.visible_aabb(rect);
    for (_id, entity) in document.entities() {
        let Some(layer) = document.layer(entity.layer) else {
            continue;
        };
        if !layer.visible {
            continue;
        }
        if !entity.geom.aabb().intersects(&visible) {
            continue;
        }
        let color = entity.style.effective_color(layer.color);
        let stroke = Stroke::new(entity.style.width.max(1.0), to_color32(color));
        draw_shape(painter, rect, viewport, &entity.geom, stroke);
    }
}

/// 選択ハイライトと、ドラッグ中のプレビュー（矩形選択枠・移動の仮表示）を描画する。
///
/// `draw_entities` の後に呼び、選択エンティティを強調色で上書きする（[`draw_shape`] 再利用）。
fn draw_selection(
    painter: &egui::Painter,
    rect: Rect,
    document: &Document,
    viewport: &Viewport,
    select_tool: &SelectTool,
) {
    let highlight = Stroke::new(SELECTION_WIDTH, SELECTION_COLOR);
    match select_tool.drag_preview() {
        Some(DragPreview::Move { delta }) => {
            // 移動中: 元の位置は draw_entities が通常色で描くので、ここでは移動後の
            // 位置を強調色で仮表示する（元＝ゴースト、プレビュー＝ハイライト）。
            for &id in select_tool.selection() {
                if let Some(entity) = document.entity(id) {
                    draw_shape(
                        painter,
                        rect,
                        viewport,
                        &entity.geom.translated(delta),
                        highlight,
                    );
                }
            }
        }
        Some(DragPreview::Rect { start, current }) => {
            // 矩形選択中: 現在の選択はそのまま強調しつつ、ドラッグ矩形を描く。
            draw_selected(painter, rect, document, viewport, select_tool, highlight);
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
        None => draw_selected(painter, rect, document, viewport, select_tool, highlight),
    }
}

/// 選択エンティティを、その実位置に強調色 `stroke` で重ね描きする。
fn draw_selected(
    painter: &egui::Painter,
    rect: Rect,
    document: &Document,
    viewport: &Viewport,
    select_tool: &SelectTool,
    stroke: Stroke,
) {
    for &id in select_tool.selection() {
        if let Some(entity) = document.entity(id) {
            draw_shape(painter, rect, viewport, &entity.geom, stroke);
        }
    }
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
) {
    match shape {
        Shape::Point(p) => {
            let sp = viewport.world_to_screen(rect, *p);
            painter.circle_filled(sp, stroke.width.max(2.0), stroke.color);
        }
        Shape::Line(line) => {
            let a = viewport.world_to_screen(rect, line.a);
            let b = viewport.world_to_screen(rect, line.b);
            painter.line_segment([a, b], stroke);
        }
        Shape::Circle(circle) => {
            let center = viewport.world_to_screen(rect, circle.center);
            let radius = (circle.radius * viewport.zoom) as f32;
            painter.circle_stroke(center, radius, stroke);
        }
        Shape::Arc(arc) => {
            draw_arc(painter, rect, viewport, arc, stroke);
        }
        Shape::Polyline(polyline) => {
            draw_polyline(painter, rect, viewport, polyline, stroke);
        }
    }
}

/// 円弧を開始角〜終了角まで [`ARC_SEGMENTS`] 分割のポリラインで近似描画する。
fn draw_arc(painter: &egui::Painter, rect: Rect, viewport: &Viewport, arc: &Arc, stroke: Stroke) {
    let sweep = arc.sweep();
    let points: Vec<Pos2> = (0..=ARC_SEGMENTS)
        .map(|i| {
            let t = i as f64 / ARC_SEGMENTS as f64;
            let angle = arc.start_angle + sweep * t;
            let p = arc.circle().point_at_angle(angle);
            viewport.world_to_screen(rect, p)
        })
        .collect();
    painter.line(points, stroke);
}

/// ポリラインを描画する。閉じている場合は末尾から先頭への辺も描く。
fn draw_polyline(
    painter: &egui::Painter,
    rect: Rect,
    viewport: &Viewport,
    polyline: &Polyline,
    stroke: Stroke,
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
    painter.line(points, stroke);
}

fn main() -> anyhow::Result<()> {
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "mcad",
        native_options,
        Box::new(|_cc| Ok(Box::new(McadApp::new()))),
    )
    .map_err(|err| anyhow::anyhow!("failed to run mcad-app: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcad_core::Entity;
    use mcad_geom::{Circle, LineSeg};

    // rfd のファイルダイアログ（`open_document`/`save_document*` のダイアログ経路）は
    // ネイティブ UI を開くため headless では自動テストできない。未保存確認は
    // egui 内製モーダル（`ConfirmState`）に統一済みなのでロジック自体はテストできるが、
    // 「破棄して続行」時に実際にダイアログを開く経路（`request_open_document` が
    // dirty でないとき即座に `open_document` を呼ぶ分岐、モーダルの
    // `ConfirmingOpen` 分岐）はここでは検証しない。ここでは GUI コンテキストを
    // 要しない部分（拡張子補完・世代ベースの dirty 判定・確認モーダルへの状態遷移・
    // 新規文書のリセット内容）のみを検証する。

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
                    width: 2.0,
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
        assert_eq!(app.document.layer_count(), 1);
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

        // Codex レビュー指摘への対応: Ctrl+N はサンプルを含まない真に空の文書にする。
        assert_eq!(app.document.entity_count(), 0);
        assert_eq!(app.document.layer_count(), 1);
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
    fn apply_imported_dxf_resets_transient_ui_state() {
        // import直後は選択集合・作図ツールが読込前のドキュメントを参照しないよう
        // リセットされる（`reset_transient_ui_state` の doc 参照）。
        let mut app = McadApp::new();
        app.tool_kind = ToolKind::Line;
        app.tool = ToolKind::Line.spawn();

        let summary = ImportSummary {
            document: Document::new(),
            skipped_entities: 0,
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
        };
        app.apply_imported_dxf(summary, 0.0);

        assert!(!app.pending_zoom_fit);
        assert_eq!(app.viewport, Viewport::new());
    }

    #[test]
    fn request_zoom_fit_after_load_sets_flag_only_when_entities_present() {
        let mut app = McadApp::new();

        app.document = sample_document();
        app.pending_zoom_fit = false;
        app.request_zoom_fit_after_load();
        assert!(app.pending_zoom_fit);

        app.document = Document::new();
        app.viewport.zoom = 7.0;
        app.request_zoom_fit_after_load();
        assert!(!app.pending_zoom_fit);
        assert_eq!(app.viewport, Viewport::new());
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
}
