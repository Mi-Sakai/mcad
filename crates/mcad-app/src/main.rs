//! `mcad-app` — eguiアプリ本体（バイナリ）。
//!
//! M2タスク4（Viewport+描画）: 座標変換・ズーム/パン・グリッド表示・
//! エンティティ描画（カリング付き）を実装する。
//! M2タスク5（Toolフレームワーク+作図ツール）: Point/Line/Circle/Arc/Polyline の
//! 各ツール（`tool.rs`）を統合し、キーボードショートカットで切り替え、
//! クリック/Enter/Escで確定・キャンセルできるようにする。後続タスクで
//! 選択・編集ツールとスナップエンジンを追加する。

mod tool;
mod viewport;

use egui::{Color32, Key, Pos2, Rect, Stroke};

use mcad_core::{Command, Document, Entity, Rgb, Style};
use mcad_geom::{Aabb, Arc, Circle, LineSeg, Point2, Polyline, Shape};

use tool::{
    ArcTool, CircleTool, InputEvent, LineTool, PointTool, PolylineTool, Tool, ToolCtx, ToolResult,
};
use viewport::Viewport;

/// 円弧をポリライン近似する際の分割数（固定値。将来ズーム適応分割は不要）。
const ARC_SEGMENTS: usize = 64;

/// ホイール1ノッチあたりのズーム倍率。
const WHEEL_ZOOM_SPEED: f64 = 0.0015;

/// グリッドの目標スクリーン間隔（ピクセル）。
const GRID_TARGET_PX: f64 = 48.0;

/// 現在アクティブな作図ツールの種類。`Select` はツール非アクティブ
/// （キャンバスクリックは何もしない）を表す。選択ツール自体は別タスク
/// （M2タスク6: 選択・編集ツール）の範囲。
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
    /// この種類に対応するツールの新しいインスタンスを作る。
    /// `Select` はツールなし（`None`）。
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
    /// 現在アクティブなツール本体。`tool_kind == Select` のとき `None`。
    tool: Option<Box<dyn Tool>>,
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
        handle_tool_shortcut_keys(ui, &mut self.tool_kind, &mut self.tool);

        egui::Panel::top("tool_status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("Tool: {}", self.tool_kind.label()));
                ui.separator();
                ui.label("L=Line  C=Circle  A=Arc  P=Polyline  (点ツールは 1 キー)  Esc=Select");
            });
        });

        egui::CentralPanel::default().show(ui, |ui| {
            let (rect, response) =
                ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());

            handle_pan_input(ui, &response, &mut self.viewport);
            handle_zoom_input(ui, &response, rect, &mut self.viewport);
            handle_tool_input(
                ui,
                &response,
                rect,
                &self.viewport,
                &mut self.document,
                &mut self.tool_kind,
                &mut self.tool,
            );

            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 0.0, Color32::from_gray(30));

            draw_grid(&painter, rect, &self.viewport);
            draw_entities(&painter, rect, &self.document, &self.viewport);
            if let Some(tool) = &self.tool {
                tool.draw_preview(&painter, rect, &self.viewport);
            }
        });
    }
}

/// キーボードショートカットでアクティブツールを切り替える（DESIGN.md 3.4 のツール群）。
///
/// `1`=Point, `L`=Line, `C`=Circle, `A`=Arc, `P`=Polyline, `Esc`=Select（非アクティブ）。
/// ツール切替は途中経過を破棄する（新しいツールインスタンスに置き換わるため）。
fn handle_tool_shortcut_keys(
    ui: &egui::Ui,
    tool_kind: &mut ToolKind,
    tool: &mut Option<Box<dyn Tool>>,
) {
    // テキスト入力欄がない前提なので、修飾キーなしのキー入力はすべてショートカット
    // として扱ってよい。
    let mut requested: Option<ToolKind> = None;
    ui.input(|i| {
        if i.key_pressed(Key::Num1) {
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
    }
}

/// アクティブなツールへ、キャンバス上の入力（クリック/Enter/Esc/マウス移動）を渡す。
///
/// パン操作（中ボタンドラッグ、または Space+左ドラッグ）と作図クリックが衝突しない
/// よう、Space 押下中および実際にパン用ドラッグが進行中は左クリックをツールへ
/// 渡さない。ツールが `Commit`/`Cancel` を返した場合、ツールをリセット（`Commit` は
/// 同じ種類の新しいインスタンスへ、`Cancel` は非アクティブ = `Select` へ）する。
fn handle_tool_input(
    ui: &egui::Ui,
    response: &egui::Response,
    rect: Rect,
    viewport: &Viewport,
    document: &mut Document,
    tool_kind: &mut ToolKind,
    tool: &mut Option<Box<dyn Tool>>,
) {
    let Some(active) = tool.as_mut() else {
        return;
    };

    let ctx = ToolCtx {
        layer: document.current_layer(),
        style: Style::inherited(),
    };

    // マウス移動は毎フレーム流し、プレビュー追従に使ってもらう。
    if let Some(pos) = response.hover_pos() {
        let world = viewport.screen_to_world(rect, pos);
        let _ = active.on_input(&ctx, InputEvent::Move(world));
    }

    // Space 押下中の左ドラッグはパン操作に使われているため、作図クリックとしては
    // 扱わない（`handle_pan_input` と役割が競合しないようにする）。
    let space_down = ui.input(|i| i.key_down(Key::Space));
    let mut result = ToolResult::Continue;
    if !space_down
        && response.clicked_by(egui::PointerButton::Primary)
        && let Some(pos) = response.interact_pointer_pos()
    {
        let world = viewport.screen_to_world(rect, pos);
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
            // ドキュメント側のレイヤー不整合などで失敗することは通常ないが（`ctx.layer`
            // は常にカレントレイヤー）、失敗時もツール状態はリセットして操作を続行可能に
            // しておく（エラー内容の表示は別タスクの範囲）。
            let _ = document.apply(cmd);
            *tool = tool_kind.spawn();
        }
        ToolResult::Cancel => {
            *tool_kind = ToolKind::Select;
            *tool = None;
        }
        ToolResult::Continue => {}
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
