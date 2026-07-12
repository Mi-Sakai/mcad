//! Tool状態機械（DESIGN.md 3.4）と作図ツール（Point/Line/Circle/Arc/Polyline）。
//!
//! # 責務の分離
//!
//! [`Tool`] トレイトは「入力を受けて状態を進め、確定/継続/中断を返す」ことと
//! 「未確定の途中経過をプレビュー描画する」ことの 2 責務のみを持つ。
//! DESIGN.md 3.4 が示すシグネチャ
//!
//! ```rust,ignore
//! trait Tool {
//!     fn on_input(&mut self, ctx: &ToolCtx, ev: InputEvent) -> ToolResult;
//!     fn draw_preview(&self, painter: &Painter, vp: &Viewport);
//! }
//! ```
//!
//! を、実際の egui/mcad-app の型に合わせて次のように調整している:
//!
//! - `Painter` → `egui::Painter`
//! - `Viewport` → `crate::viewport::Viewport`
//! - `draw_preview` は `Viewport::world_to_screen` がスクリーン矩形を引数に取る
//!   （`Viewport` 自身はスクリーン矩形を保持しない設計、`viewport.rs` 参照）ため、
//!   `rect: egui::Rect` を追加引数として受け取る
//! - 入力 `InputEvent` はワールド座標（[`Point2`]）とキー操作のみからなる軽量な列挙型
//!   にし、egui の生入力（`egui::Event` 等）に依存しない。これにより GUI なしで
//!   ツールの状態遷移を単体テストできる（本ファイル末尾の `tests` 参照）
//!
//! 2 つの責務の分離自体（「入力→ Continue/Commit/Cancel」「プレビュー描画」）は
//! 設計方針どおり維持している。
//!
//! # プレビュー描画の再利用
//!
//! `draw_preview` は `main.rs` の `draw_shape`（`Shape` → `Painter` 変換）を
//! そのまま再利用する。`draw_shape` は crate ルート（`main.rs`）に定義された
//! プライベート関数だが、Rust の可視性規則上プライベート項目は「定義モジュールと
//! その子孫モジュール」から見えるため、`tool` モジュール（`main.rs` の子モジュール）
//! から `crate::draw_shape` として問題なく呼べる。

use egui::{Color32, Painter, Rect, Stroke};

use mcad_core::{Command, Entity, LayerId, Style};
use mcad_geom::{Arc, LineSeg, Point2, Polyline, Shape, circumcircle};

use crate::draw_shape;
use crate::viewport::Viewport;

/// プレビュー線の色（確定済みエンティティと区別しやすい暖色）。
const PREVIEW_COLOR: Color32 = Color32::from_rgb(255, 200, 60);
/// プレビュー線の太さ。
const PREVIEW_WIDTH: f32 = 1.5;

fn preview_stroke() -> Stroke {
    Stroke::new(PREVIEW_WIDTH, PREVIEW_COLOR)
}

/// ツールに渡す実行時コンテキスト。新規エンティティの所属レイヤーとスタイルを持つ。
#[derive(Debug, Clone, Copy)]
pub struct ToolCtx {
    /// 新規エンティティの所属先（呼び出し側は通常 `Document::current_layer()`）。
    pub layer: LayerId,
    /// 新規エンティティのスタイル（通常 `Style::inherited()`）。
    pub style: Style,
}

/// ツールへの入力。ワールド座標とキー操作のみからなる軽量な列挙型
/// （GUIなしで単体テストできるようにするための設計）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputEvent {
    /// マウス移動（クリックを伴わない）。プレビュー更新用。
    Move(Point2),
    /// 左クリック確定。
    Click(Point2),
    /// Enterキー（確定操作。主に Polyline がクリック列を確定するのに使う）。
    Confirm,
    /// Escキー（未確定の状態を破棄する）。
    Cancel,
}

/// [`Tool::on_input`] の結果。
#[derive(Debug, Clone, PartialEq)]
pub enum ToolResult {
    /// まだ確定しない。ツールは内部状態を進めただけ。
    Continue,
    /// 確定した。呼び出し側は `Document::apply` でコマンドを適用する。
    Commit(Command),
    /// キャンセルされた。未確定の状態は破棄済み。
    Cancel,
}

/// 作図ツールの状態機械（DESIGN.md 3.4）。
pub trait Tool {
    /// 入力を 1 つ受け取り、内部状態を進めて結果を返す。
    fn on_input(&mut self, ctx: &ToolCtx, ev: InputEvent) -> ToolResult;

    /// 未確定の途中経過（クリック済み点・マウス追従中の線など）をプレビュー描画する。
    fn draw_preview(&self, painter: &Painter, rect: Rect, viewport: &Viewport);
}

// ---------------------------------------------------------------------
// Point
// ---------------------------------------------------------------------

/// 点ツール。クリック1回で [`Shape::Point`] を確定する。
#[derive(Debug, Default)]
pub struct PointTool {
    cursor: Option<Point2>,
}

impl Tool for PointTool {
    fn on_input(&mut self, ctx: &ToolCtx, ev: InputEvent) -> ToolResult {
        match ev {
            InputEvent::Move(p) => {
                self.cursor = Some(p);
                ToolResult::Continue
            }
            InputEvent::Click(p) => ToolResult::Commit(Command::AddEntity(Entity::new(
                Shape::Point(p),
                ctx.layer,
                ctx.style,
            ))),
            InputEvent::Cancel => ToolResult::Cancel,
            InputEvent::Confirm => ToolResult::Continue,
        }
    }

    fn draw_preview(&self, painter: &Painter, rect: Rect, viewport: &Viewport) {
        if let Some(p) = self.cursor {
            draw_shape(painter, rect, viewport, &Shape::Point(p), preview_stroke());
        }
    }
}

// ---------------------------------------------------------------------
// Line
// ---------------------------------------------------------------------

#[derive(Debug, Default)]
enum LineState {
    #[default]
    WaitingFirst,
    WaitingSecond(Point2),
}

/// 線分ツール。クリック2回（始点・終点）で [`Shape::Line`] を確定する。
/// 1点目クリック後はマウス位置までのプレビュー線を表示する。
#[derive(Debug, Default)]
pub struct LineTool {
    state: LineState,
    cursor: Option<Point2>,
}

impl Tool for LineTool {
    fn on_input(&mut self, ctx: &ToolCtx, ev: InputEvent) -> ToolResult {
        match ev {
            InputEvent::Move(p) => {
                self.cursor = Some(p);
                ToolResult::Continue
            }
            InputEvent::Click(p) => match self.state {
                LineState::WaitingFirst => {
                    self.state = LineState::WaitingSecond(p);
                    ToolResult::Continue
                }
                LineState::WaitingSecond(first) => {
                    let cmd = Command::AddEntity(Entity::new(
                        Shape::Line(LineSeg::new(first, p)),
                        ctx.layer,
                        ctx.style,
                    ));
                    self.state = LineState::WaitingFirst;
                    ToolResult::Commit(cmd)
                }
            },
            InputEvent::Cancel => {
                self.state = LineState::WaitingFirst;
                ToolResult::Cancel
            }
            InputEvent::Confirm => ToolResult::Continue,
        }
    }

    fn draw_preview(&self, painter: &Painter, rect: Rect, viewport: &Viewport) {
        if let (LineState::WaitingSecond(first), Some(cursor)) = (&self.state, self.cursor) {
            draw_shape(
                painter,
                rect,
                viewport,
                &Shape::Line(LineSeg::new(*first, cursor)),
                preview_stroke(),
            );
        }
    }
}

// ---------------------------------------------------------------------
// Circle
// ---------------------------------------------------------------------

#[derive(Debug, Default)]
enum CircleState {
    #[default]
    WaitingCenter,
    WaitingRadiusPoint(Point2),
}

/// 円ツール。クリック2回（中心・円周上の1点）で [`Shape::Circle`] を確定する。
/// プレビューは中心からマウス位置までの距離を半径とする円。
#[derive(Debug, Default)]
pub struct CircleTool {
    state: CircleState,
    cursor: Option<Point2>,
}

impl Tool for CircleTool {
    fn on_input(&mut self, ctx: &ToolCtx, ev: InputEvent) -> ToolResult {
        match ev {
            InputEvent::Move(p) => {
                self.cursor = Some(p);
                ToolResult::Continue
            }
            InputEvent::Click(p) => match self.state {
                CircleState::WaitingCenter => {
                    self.state = CircleState::WaitingRadiusPoint(p);
                    ToolResult::Continue
                }
                CircleState::WaitingRadiusPoint(center) => {
                    let radius = center.distance(p);
                    let cmd = Command::AddEntity(Entity::new(
                        Shape::Circle(mcad_geom::Circle::new(center, radius)),
                        ctx.layer,
                        ctx.style,
                    ));
                    self.state = CircleState::WaitingCenter;
                    ToolResult::Commit(cmd)
                }
            },
            InputEvent::Cancel => {
                self.state = CircleState::WaitingCenter;
                ToolResult::Cancel
            }
            InputEvent::Confirm => ToolResult::Continue,
        }
    }

    fn draw_preview(&self, painter: &Painter, rect: Rect, viewport: &Viewport) {
        if let (CircleState::WaitingRadiusPoint(center), Some(cursor)) = (&self.state, self.cursor)
        {
            let radius = center.distance(cursor);
            draw_shape(
                painter,
                rect,
                viewport,
                &Shape::Circle(mcad_geom::Circle::new(*center, radius)),
                preview_stroke(),
            );
        }
    }
}

// ---------------------------------------------------------------------
// Arc（3点指定）
// ---------------------------------------------------------------------

#[derive(Debug, Default)]
enum ArcState {
    #[default]
    WaitingP1,
    WaitingP2(Point2),
    WaitingP3(Point2, Point2),
}

/// 円弧ツール。クリック3回で [`Shape::Arc`] を確定する。
///
/// # 3点の解釈
///
/// DESIGN.md 3.4 は Arc ツールを「3点」方式とだけ定めている。ここでは
/// 「1点目=弧の始点、2点目=弧が通過する中間点（円弧の向き・張り出す側を決める）、
/// 3点目=弧の終点」という、3点円弧の一般的な解釈を採用する。
///
/// 3点から外接円（[`circumcircle`]）を求めた後、中心からの角度
/// `a1 = angle(p1)`, `a2 = angle(p2)`, `a3 = angle(p3)` を計算する。
/// `Arc` は開始角から終了角まで常に CCW（反時計回り）で掃引する型なので、
/// `start=a1, end=a3` の CCW 弧が 2 点目（`a2`）を含むかどうかで、実際に
/// 2 点目を通る側の弧を選ぶ（含まなければ `start`/`end` を入れ替えて反対側の
/// 弧を採用する。1つの円は2点の角度によって互いに補完し合う2つの弧に
/// 分割されるため、2点目を含まない側を捨てれば必ずもう一方が2点目を含む）。
///
/// 3点が同一直線上（に近い）場合は外接円が定まらないため、3点目のクリックは
/// 無視し（状態を変えず `Continue` を返す）、ユーザーに別の3点目を促す。
#[derive(Debug, Default)]
pub struct ArcTool {
    state: ArcState,
    cursor: Option<Point2>,
}

/// 3点 `p1`（始点）, `p2`（通過点）, `p3`（終点）から、`p2` を通る側の円弧を求める。
/// 3点が同一直線上に近く外接円が定まらない場合は `None`。
fn build_arc(p1: Point2, p2: Point2, p3: Point2) -> Option<Arc> {
    let circle = circumcircle(p1, p2, p3)?;
    let a1 = (p1 - circle.center).angle();
    let a2 = (p2 - circle.center).angle();
    let a3 = (p3 - circle.center).angle();
    let candidate = Arc::new(circle.center, circle.radius, a1, a3);
    if candidate.contains_angle(a2) {
        Some(candidate)
    } else {
        Some(Arc::new(circle.center, circle.radius, a3, a1))
    }
}

impl Tool for ArcTool {
    fn on_input(&mut self, ctx: &ToolCtx, ev: InputEvent) -> ToolResult {
        match ev {
            InputEvent::Move(p) => {
                self.cursor = Some(p);
                ToolResult::Continue
            }
            InputEvent::Click(p) => match self.state {
                ArcState::WaitingP1 => {
                    self.state = ArcState::WaitingP2(p);
                    ToolResult::Continue
                }
                ArcState::WaitingP2(p1) => {
                    self.state = ArcState::WaitingP3(p1, p);
                    ToolResult::Continue
                }
                ArcState::WaitingP3(p1, p2) => match build_arc(p1, p2, p) {
                    Some(arc) => {
                        let cmd =
                            Command::AddEntity(Entity::new(Shape::Arc(arc), ctx.layer, ctx.style));
                        self.state = ArcState::WaitingP1;
                        ToolResult::Commit(cmd)
                    }
                    // 同一直線上の3点目は無視し、別の3点目のクリックを待つ。
                    None => ToolResult::Continue,
                },
            },
            InputEvent::Cancel => {
                self.state = ArcState::WaitingP1;
                ToolResult::Cancel
            }
            InputEvent::Confirm => ToolResult::Continue,
        }
    }

    fn draw_preview(&self, painter: &Painter, rect: Rect, viewport: &Viewport) {
        match (&self.state, self.cursor) {
            (ArcState::WaitingP2(p1), Some(cursor)) => {
                draw_shape(
                    painter,
                    rect,
                    viewport,
                    &Shape::Line(LineSeg::new(*p1, cursor)),
                    preview_stroke(),
                );
            }
            (ArcState::WaitingP3(p1, p2), Some(cursor)) => {
                if let Some(arc) = build_arc(*p1, *p2, cursor) {
                    draw_shape(painter, rect, viewport, &Shape::Arc(arc), preview_stroke());
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------
// Polyline
// ---------------------------------------------------------------------

/// ポリラインツール。クリックで頂点を追加し、Enterキーで確定する（2点以上必要）。
/// Escでキャンセル。ダブルクリックでの確定は実装しない（Enterのみ）。
#[derive(Debug, Default)]
pub struct PolylineTool {
    vertices: Vec<Point2>,
    cursor: Option<Point2>,
}

impl Tool for PolylineTool {
    fn on_input(&mut self, ctx: &ToolCtx, ev: InputEvent) -> ToolResult {
        match ev {
            InputEvent::Move(p) => {
                self.cursor = Some(p);
                ToolResult::Continue
            }
            InputEvent::Click(p) => {
                self.vertices.push(p);
                ToolResult::Continue
            }
            InputEvent::Confirm => {
                if self.vertices.len() >= 2 {
                    let vertices = std::mem::take(&mut self.vertices);
                    ToolResult::Commit(Command::AddEntity(Entity::new(
                        Shape::Polyline(Polyline::new(vertices, false)),
                        ctx.layer,
                        ctx.style,
                    )))
                } else {
                    // 2点未満では確定できない。Enter は無視して入力を続ける。
                    ToolResult::Continue
                }
            }
            InputEvent::Cancel => {
                self.vertices.clear();
                ToolResult::Cancel
            }
        }
    }

    fn draw_preview(&self, painter: &Painter, rect: Rect, viewport: &Viewport) {
        if self.vertices.is_empty() {
            return;
        }
        let mut preview_vertices = self.vertices.clone();
        if let Some(cursor) = self.cursor {
            preview_vertices.push(cursor);
        }
        if preview_vertices.len() < 2 {
            return;
        }
        draw_shape(
            painter,
            rect,
            viewport,
            &Shape::Polyline(Polyline::new(preview_vertices, false)),
            preview_stroke(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcad_core::Document;

    fn ctx() -> (Document, ToolCtx) {
        let doc = Document::new();
        let ctx = ToolCtx {
            layer: doc.current_layer(),
            style: Style::inherited(),
        };
        (doc, ctx)
    }

    fn shape_of(result: ToolResult) -> Shape {
        match result {
            ToolResult::Commit(Command::AddEntity(entity)) => entity.geom,
            other => panic!("expected Commit(AddEntity(..)), got {other:?}"),
        }
    }

    // --- Point ---

    #[test]
    fn point_tool_commits_on_single_click() {
        let (_doc, ctx) = ctx();
        let mut tool = PointTool::default();
        let result = tool.on_input(&ctx, InputEvent::Click(Point2::new(1.0, 2.0)));
        assert_eq!(shape_of(result), Shape::Point(Point2::new(1.0, 2.0)));
    }

    #[test]
    fn point_tool_cancel_returns_cancel() {
        let (_doc, ctx) = ctx();
        let mut tool = PointTool::default();
        assert_eq!(tool.on_input(&ctx, InputEvent::Cancel), ToolResult::Cancel);
    }

    // --- Line ---

    #[test]
    fn line_tool_needs_two_clicks_to_commit() {
        let (_doc, ctx) = ctx();
        let mut tool = LineTool::default();
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(3.0, 4.0);
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Click(a)),
            ToolResult::Continue
        );
        let result = tool.on_input(&ctx, InputEvent::Click(b));
        assert_eq!(shape_of(result), Shape::Line(LineSeg::new(a, b)));
    }

    #[test]
    fn line_tool_resets_after_commit_for_next_line() {
        let (_doc, ctx) = ctx();
        let mut tool = LineTool::default();
        tool.on_input(&ctx, InputEvent::Click(Point2::new(0.0, 0.0)));
        tool.on_input(&ctx, InputEvent::Click(Point2::new(1.0, 0.0)));
        // 1本目確定後、2本目もちゃんと2クリックで確定できる（状態がリセットされている）。
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Click(Point2::new(5.0, 5.0))),
            ToolResult::Continue
        );
        let result = tool.on_input(&ctx, InputEvent::Click(Point2::new(6.0, 5.0)));
        assert_eq!(
            shape_of(result),
            Shape::Line(LineSeg::new(Point2::new(5.0, 5.0), Point2::new(6.0, 5.0)))
        );
    }

    #[test]
    fn line_tool_cancel_discards_first_point() {
        let (_doc, ctx) = ctx();
        let mut tool = LineTool::default();
        tool.on_input(&ctx, InputEvent::Click(Point2::new(0.0, 0.0)));
        assert_eq!(tool.on_input(&ctx, InputEvent::Cancel), ToolResult::Cancel);
        // キャンセル後は新しい始点として扱われる（2クリック必要）。
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Click(Point2::new(9.0, 9.0))),
            ToolResult::Continue
        );
    }

    // --- Circle ---

    #[test]
    fn circle_tool_commits_with_center_and_radius_point() {
        let (_doc, ctx) = ctx();
        let mut tool = CircleTool::default();
        let center = Point2::new(0.0, 0.0);
        let edge = Point2::new(3.0, 4.0);
        tool.on_input(&ctx, InputEvent::Click(center));
        let result = tool.on_input(&ctx, InputEvent::Click(edge));
        match shape_of(result) {
            Shape::Circle(c) => {
                assert_eq!(c.center, center);
                assert!((c.radius - 5.0).abs() < 1e-9);
            }
            other => panic!("expected Circle, got {other:?}"),
        }
    }

    // --- Arc ---

    #[test]
    fn arc_tool_commits_after_three_clicks_on_unit_circle() {
        let (_doc, ctx) = ctx();
        let mut tool = ArcTool::default();
        // 単位円上の3点: 角度 0, π/2, π。始点(1,0)→中間点(0,1)→終点(-1,0)。
        let p1 = Point2::new(1.0, 0.0);
        let p2 = Point2::new(0.0, 1.0);
        let p3 = Point2::new(-1.0, 0.0);
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Click(p1)),
            ToolResult::Continue
        );
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Click(p2)),
            ToolResult::Continue
        );
        let result = tool.on_input(&ctx, InputEvent::Click(p3));
        match shape_of(result) {
            Shape::Arc(arc) => {
                assert!(arc.center.distance(Point2::ORIGIN) < 1e-9);
                assert!((arc.radius - 1.0).abs() < 1e-9);
                // 中間点(0,1)、すなわち角度 π/2 は弧の範囲内でなければならない。
                assert!(arc.contains_angle(std::f64::consts::FRAC_PI_2));
                // 始点・終点も弧上（角度が範囲内）。
                assert!(arc.contains_angle(0.0));
                assert!(arc.contains_angle(std::f64::consts::PI));
            }
            other => panic!("expected Arc, got {other:?}"),
        }
    }

    #[test]
    fn arc_tool_picks_the_side_containing_midpoint() {
        // 中間点を反対側（下半分, 角度 -π/2 = 3π/2）にすると、そちら側を通る弧を選ぶはず。
        let (_doc, ctx) = ctx();
        let mut tool = ArcTool::default();
        let p1 = Point2::new(1.0, 0.0);
        let p2 = Point2::new(0.0, -1.0);
        let p3 = Point2::new(-1.0, 0.0);
        tool.on_input(&ctx, InputEvent::Click(p1));
        tool.on_input(&ctx, InputEvent::Click(p2));
        let result = tool.on_input(&ctx, InputEvent::Click(p3));
        match shape_of(result) {
            Shape::Arc(arc) => {
                assert!(arc.contains_angle(3.0 * std::f64::consts::FRAC_PI_2));
                // 反対側（上半分, π/2）は含まれないはず。
                assert!(!arc.contains_angle(std::f64::consts::FRAC_PI_2));
            }
            other => panic!("expected Arc, got {other:?}"),
        }
    }

    #[test]
    fn arc_tool_ignores_collinear_third_point() {
        let (_doc, ctx) = ctx();
        let mut tool = ArcTool::default();
        tool.on_input(&ctx, InputEvent::Click(Point2::new(0.0, 0.0)));
        tool.on_input(&ctx, InputEvent::Click(Point2::new(1.0, 0.0)));
        // 同一直線上の3点目は無視され、Continue のまま3点目待ちが継続する。
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Click(Point2::new(2.0, 0.0))),
            ToolResult::Continue
        );
        // 有効な3点目を与えれば、そこで確定できる。
        let result = tool.on_input(&ctx, InputEvent::Click(Point2::new(1.0, 1.0)));
        assert!(matches!(result, ToolResult::Commit(_)));
    }

    #[test]
    fn arc_tool_cancel_discards_progress() {
        let (_doc, ctx) = ctx();
        let mut tool = ArcTool::default();
        tool.on_input(&ctx, InputEvent::Click(Point2::new(0.0, 0.0)));
        tool.on_input(&ctx, InputEvent::Click(Point2::new(1.0, 0.0)));
        assert_eq!(tool.on_input(&ctx, InputEvent::Cancel), ToolResult::Cancel);
        // キャンセル後は最初の点として扱われる（3クリック必要）。
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Click(Point2::new(9.0, 9.0))),
            ToolResult::Continue
        );
    }

    // --- Polyline ---

    #[test]
    fn polyline_tool_requires_enter_to_commit() {
        let (_doc, ctx) = ctx();
        let mut tool = PolylineTool::default();
        let pts = [
            Point2::new(0.0, 0.0),
            Point2::new(1.0, 0.0),
            Point2::new(1.0, 1.0),
        ];
        for p in pts {
            assert_eq!(
                tool.on_input(&ctx, InputEvent::Click(p)),
                ToolResult::Continue
            );
        }
        let result = tool.on_input(&ctx, InputEvent::Confirm);
        assert_eq!(
            shape_of(result),
            Shape::Polyline(Polyline::new(pts.to_vec(), false))
        );
    }

    #[test]
    fn polyline_tool_confirm_with_single_point_is_ignored() {
        let (_doc, ctx) = ctx();
        let mut tool = PolylineTool::default();
        tool.on_input(&ctx, InputEvent::Click(Point2::new(0.0, 0.0)));
        // 1点だけでは Enter しても確定しない（2点以上必要）。
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Confirm),
            ToolResult::Continue
        );
    }

    #[test]
    fn polyline_tool_cancel_discards_all_vertices() {
        let (_doc, ctx) = ctx();
        let mut tool = PolylineTool::default();
        tool.on_input(&ctx, InputEvent::Click(Point2::new(0.0, 0.0)));
        tool.on_input(&ctx, InputEvent::Click(Point2::new(1.0, 0.0)));
        assert_eq!(tool.on_input(&ctx, InputEvent::Cancel), ToolResult::Cancel);
        // キャンセル後は新しいポリラインとして扱われる（1点だけでは確定不可）。
        tool.on_input(&ctx, InputEvent::Click(Point2::new(9.0, 9.0)));
        assert_eq!(
            tool.on_input(&ctx, InputEvent::Confirm),
            ToolResult::Continue
        );
    }
}
