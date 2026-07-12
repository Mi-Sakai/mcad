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

use egui::{Color32, Key, Pos2, Rect, Stroke};

use mcad_core::{Command, Document, Entity, Layer, LayerId, Rgb, Style};
use mcad_geom::{Aabb, Arc, Circle, LineSeg, Point2, Polyline, Shape};

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
    /// 動作確認用にサンプルエンティティ（線分・円・円弧・ポリライン）を
    /// 追加したドキュメントを持つアプリを作る。
    ///
    /// 本番の「ファイルを開く」機能は別タスク（.mcad保存/読込）の範囲であり、
    /// ここでは Viewport・描画の動作確認だけが目的。
    fn new() -> Self {
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

        Self {
            document,
            viewport: Viewport::new(),
            tool_kind: ToolKind::Select,
            tool: None,
            select_tool: SelectTool::default(),
            snap_enabled: true,
            snap_marker: None,
            status: None,
        }
    }
}

impl Default for McadApp {
    fn default() -> Self {
        Self::new()
    }
}

impl eframe::App for McadApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let now = ui.input(|i| i.time);

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
        if undo_pressed && self.document.undo() {
            self.select_tool.retain_alive(&self.document);
        }
        if redo_pressed && self.document.redo() {
            self.select_tool.retain_alive(&self.document);
        }

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
                ui.label(format!("Tool: {}", self.tool_kind.label()));
                ui.separator();
                ui.label(format!(
                    "Snap: {}",
                    if self.snap_enabled { "ON" } else { "OFF" }
                ));
                ui.separator();
                ui.label(
                    "S=Select  1=Point  L=Line  C=Circle  A=Arc  P=Polyline  \
                     Del=Delete  Esc=Cancel  F3=Snap  Ctrl+Z=Undo  Ctrl+Y=Redo",
                );
                if let Some(msg) = &self.status {
                    ui.separator();
                    ui.colored_label(STATUS_MESSAGE_COLOR, &msg.text);
                }
            });
        });

        egui::Panel::right("layer_panel").show(ui, |ui| {
            layer_panel(ui, &mut self.document, &mut self.status, now);
        });

        egui::CentralPanel::default().show(ui, |ui| {
            let (rect, response) =
                ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());

            handle_pan_input(ui, &response, &mut self.viewport);
            handle_zoom_input(ui, &response, rect, &mut self.viewport);
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
        let (world, marker) = apply_snap(document, snap_enabled, raw, radius, grid_step);
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
        let (world, _) = apply_snap(document, snap_enabled, raw, radius, grid_step);
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
            // レイヤーがロック中の AddEntity）。失敗はステータスバーへ表示し、
            // ツール状態はリセットして操作を続行可能にしておく。
            if let Err(err) = document.apply(cmd) {
                set_status(status, now, format!("Commit failed: {err}"));
            }
            *tool = tool_kind.spawn();
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
fn apply_snap(
    document: &Document,
    enabled: bool,
    raw: Point2,
    radius: f64,
    grid_step: f64,
) -> (Point2, Option<snap::SnapResult>) {
    if !enabled {
        return (raw, None);
    }
    match snap::snap(document, raw, radius, grid_step) {
        Some(result) => (result.point, Some(result)),
        None => (raw, None),
    }
}

/// 選択・編集モード（`tool_kind == Select`）のキャンバス入力を [`SelectTool`] へ渡す。
///
/// egui 組み込みのクリック/ドラッグ判定を利用し、単発クリック（＝選択）と
/// ドラッグ（＝矩形選択 or 移動）を振り分ける。移動・削除の確定コマンドは
/// [`Document::apply`] で適用する（`Batch` の原子性・undo/redo 結線はコア側に従う）。
///
/// - `Delete`/`Backspace`: 選択エンティティを 1 バッチで削除。適用成功時のみ選択を解除する。
/// - `Esc`: 進行中のドラッグを破棄（選択は変えない）。
/// - `Space` 押下中の左ドラッグはパン用なので、選択操作としては扱わない。
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
