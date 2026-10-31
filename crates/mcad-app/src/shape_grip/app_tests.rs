//! 図形のグリップ編集のアプリ経由のテスト（M13 タスク101、検収基準 (vii)・(iii)）。
//!
//! キャンバスのポインタ操作を `McadApp` へ流し、ドラッグ開始の優先順、プレビュー、確定、
//! 解けない位置での拒否、中断を確かめる。

use eframe::App as _;
use egui::{Key, Pos2, Rect, vec2};
use mcad_core::{
    Command, Constraint, DimAnnotation, DimDirection, DimLinear, Document, Entity, EntityGeom,
    EntityId, Layer, PointKind, PointRef, Rgb, Style,
};
use mcad_geom::{Circle, LineSeg, Point2, Shape};

use super::*;
use crate::tool::{DragPreview, GripKind, SelectTool};
use crate::viewport::Viewport;
use crate::{McadApp, ToolKind, grip_hit, start_select_drag};

/// キャンバス上のどこか（上部パネルの下・右パネルの左）。
const CANVAS_POS: Pos2 = Pos2::new(300.0, 500.0);

struct Harness {
    app: McadApp,
    ctx: egui::Context,
    frame: eframe::Frame,
    /// [`CANVAS_POS`] のワールド座標。
    origin: Point2,
}

impl Harness {
    fn new() -> Self {
        let mut h = Self {
            app: McadApp::new(),
            ctx: egui::Context::default(),
            frame: eframe::Frame::_new_kittest(),
            origin: Point2::ORIGIN,
        };
        h.app.config.snap_enabled = false;
        h.run(vec![egui::Event::PointerMoved(CANVAS_POS)]);
        h.run(Vec::new());
        // キャンバス矩形はアプリが保持しないので、点ツールのクリックで実測する。
        h.run(vec![
            key_press(Key::Num1),
            egui::Event::Text("1".to_owned()),
        ]);
        assert_eq!(h.app.tool_kind, ToolKind::Point);
        h.click(CANVAS_POS);
        let point = h
            .app
            .document
            .entities()
            .find_map(|(_, e)| match e.geom.as_shape() {
                Some(Shape::Point(p)) => Some(*p),
                _ => None,
            })
            .expect("the click placed a point");
        h.origin = point;
        assert!(h.app.document.undo());
        h.run(vec![key_press(Key::Escape)]);
        assert_eq!(h.app.tool_kind, ToolKind::Select);
        h
    }

    fn run(&mut self, events: Vec<egui::Event>) {
        self.run_with(events, egui::Modifiers::NONE);
    }

    fn run_with(&mut self, events: Vec<egui::Event>, modifiers: egui::Modifiers) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))),
            events,
            modifiers,
            ..Default::default()
        };
        let Self {
            app, ctx, frame, ..
        } = self;
        let _ = ctx.run_ui(input, |ui| app.ui(ui, frame));
    }

    fn click(&mut self, pos: Pos2) {
        self.run(vec![egui::Event::PointerMoved(pos), button(pos, true)]);
        self.run(vec![button(pos, false)]);
        self.run(Vec::new());
    }

    /// キャンバスのピクセル `(dx, dy)`（[`CANVAS_POS`] から。y は下向き）のワールド座標。
    fn world(&self, dx: f32, dy: f32) -> Point2 {
        let zoom = self.app.viewport.zoom;
        Point2::new(
            self.origin.x + f64::from(dx) / zoom,
            self.origin.y - f64::from(dy) / zoom,
        )
    }

    fn line(&mut self, a: (f32, f32), b: (f32, f32)) -> EntityId {
        let geom = LineSeg::new(self.world(a.0, a.1), self.world(b.0, b.1));
        self.add(Shape::Line(geom))
    }

    fn add(&mut self, shape: Shape) -> EntityId {
        let layer = self.app.document.current_layer();
        self.app
            .document
            .apply(Command::AddEntity(Entity::new(
                shape,
                layer,
                Style::inherited(),
            )))
            .unwrap()
            .entities[0]
    }

    fn constrain(&mut self, constraint: Constraint) {
        self.app
            .document
            .apply(Command::AddConstraint {
                constraint,
                hold: None,
            })
            .unwrap();
    }

    /// 100×-60 ピクセルの矩形（下の辺から反時計回り。角の一致 + 水平・垂直）。
    fn rectangle(&mut self) -> [EntityId; 4] {
        let corners = [(0.0, 0.0), (100.0, 0.0), (100.0, -60.0), (0.0, -60.0)];
        let sides: [EntityId; 4] =
            std::array::from_fn(|i| self.line(corners[i], corners[(i + 1) % 4]));
        for i in 0..4 {
            self.constrain(Constraint::Coincident {
                a: PointRef::new(sides[i], PointKind::End),
                b: PointRef::new(sides[(i + 1) % 4], PointKind::Start),
            });
            let line = sides[i];
            self.constrain(if i % 2 == 0 {
                Constraint::Horizontal { line }
            } else {
                Constraint::Vertical { line }
            });
        }
        sides
    }

    fn select(&mut self, ids: &[EntityId]) {
        self.app.select_tool.set_selection(ids.to_vec());
    }

    /// `from` を押してカーソルを `to` まで動かす（離さない）。
    fn press_and_move(&mut self, from: (f32, f32), to: (f32, f32)) {
        let at = |(x, y): (f32, f32)| CANVAS_POS + vec2(x, y);
        self.run(vec![egui::Event::PointerMoved(at(from))]);
        self.run(vec![button(at(from), true)]);
        // egui のドラッグ閾値を超えたフレームが「ドラッグ開始」で、その次から「ドラッグ中」。
        self.run(vec![egui::Event::PointerMoved(at(to))]);
        self.run(vec![egui::Event::PointerMoved(at(to))]);
    }

    fn move_to(&mut self, to: (f32, f32)) {
        self.run(vec![egui::Event::PointerMoved(
            CANVAS_POS + vec2(to.0, to.1),
        )]);
    }

    fn release(&mut self, at: (f32, f32)) {
        self.run(vec![button(CANVAS_POS + vec2(at.0, at.1), false)]);
        self.run(Vec::new());
    }

    fn geom(&self, id: EntityId) -> EntityGeom {
        self.app.document.entity(id).unwrap().geom.clone()
    }

    fn line_of(&self, id: EntityId) -> LineSeg {
        match self.geom(id).as_shape() {
            Some(Shape::Line(l)) => *l,
            other => panic!("{other:?}"),
        }
    }

    fn status(&self) -> Option<&str> {
        self.app.status.as_ref().map(|s| s.text.as_str())
    }

    fn all_geoms(&self) -> Vec<(EntityId, EntityGeom)> {
        self.app
            .document
            .entities()
            .map(|(id, e)| (id, e.geom.clone()))
            .collect()
    }
}

fn key_press(key: Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

fn button(pos: Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

fn close(a: Point2, b: Point2) -> bool {
    a.distance(b) < 1e-3
}

#[test]
fn dragging_a_line_end_without_constraints_moves_only_that_end_and_undoes_once() {
    let mut h = Harness::new();
    let l = h.line((0.0, 0.0), (100.0, 0.0));
    let before = h.line_of(l);
    let generation = h.app.document.generation();
    h.select(&[l]);

    h.press_and_move((100.0, 0.0), (130.0, 40.0));
    assert!(h.app.select_tool.is_grip_dragging());
    // ドラッグ中は図面を変えず、解いた形をプレビューに持つ。
    assert_eq!(h.line_of(l), before);
    assert_eq!(h.app.document.generation(), generation);
    let target = h.world(130.0, 40.0);
    match h.app.select_tool.shape_grip_preview() {
        [(id, geom)] => {
            assert_eq!(*id, l);
            let Some(Shape::Line(shown)) = geom.as_shape() else {
                panic!("{geom:?}");
            };
            assert_eq!(shown.a, before.a);
            assert!(close(shown.b, target), "{shown:?} {target:?}");
        }
        other => panic!("{other:?}"),
    }

    h.release((130.0, 40.0));
    assert!(!h.app.select_tool.is_grip_dragging());
    let after = h.line_of(l);
    assert_eq!(after.a, before.a, "the other end keeps its bits");
    assert!(close(after.b, target), "{after:?} {target:?}");
    assert_eq!(h.app.select_tool.selection(), [l], "the selection stays");

    assert!(h.app.document.undo());
    assert_eq!(h.line_of(l), before);
    // 次の undo は線分の追加まで戻る = グリップ編集は履歴の 1 件だった。
    assert!(h.app.document.undo());
    assert!(h.app.document.entity(l).is_none());
}

#[test]
fn circle_and_arc_grips_work_through_the_canvas() {
    let mut h = Harness::new();
    let center = h.world(0.0, 0.0);
    let radius = h.world(50.0, 0.0).distance(center);
    let circle = h.add(Shape::Circle(Circle::new(center, radius)));
    h.select(&[circle]);
    // 半径（90° の四半円点。スクリーンの上）。
    h.press_and_move((0.0, -50.0), (0.0, -80.0));
    h.release((0.0, -80.0));
    let Some(Shape::Circle(c)) = h.geom(circle).as_shape().cloned() else {
        panic!();
    };
    assert_eq!(c.center, center);
    assert!((c.radius - h.world(0.0, -80.0).distance(center)).abs() < 1e-3);
    // 中心。
    h.press_and_move((0.0, 0.0), (25.0, 15.0));
    h.release((25.0, 15.0));
    let Some(Shape::Circle(moved)) = h.geom(circle).as_shape().cloned() else {
        panic!();
    };
    assert_eq!(moved.radius, c.radius);
    assert!(close(moved.center, h.world(25.0, 15.0)));
    assert!(h.app.document.undo());
    assert!(h.app.document.undo());
    assert_eq!(
        h.geom(circle),
        EntityGeom::Shape(Shape::Circle(Circle::new(center, radius)))
    );

    // 円弧の終点（カーソルは円周の外でも、半径は変わらない）。
    let mut h = Harness::new();
    let center = h.world(0.0, 0.0);
    let radius = h.world(50.0, 0.0).distance(center);
    let arc = h.add(Shape::Arc(mcad_geom::Arc::new(
        center,
        radius,
        0.0,
        std::f64::consts::FRAC_PI_2,
    )));
    h.select(&[arc]);
    h.press_and_move((0.0, -50.0), (-80.0, 0.0));
    h.release((-80.0, 0.0));
    let Some(Shape::Arc(a)) = h.geom(arc).as_shape().cloned() else {
        panic!();
    };
    assert_eq!((a.center, a.radius, a.start_angle), (center, radius, 0.0));
    assert!((a.end_angle - std::f64::consts::PI).abs() < 1e-3, "{a:?}");
}

#[test]
fn a_rectangle_follows_and_stays_rectangular_and_one_undo_restores_it() {
    let mut h = Harness::new();
    let sides = h.rectangle();
    let before = h.all_geoms();
    h.select(&[sides[0]]);

    // 下の辺の終点（右下の角）を右下へ引く。
    h.press_and_move((100.0, 0.0), (140.0, 30.0));
    assert_eq!(
        h.all_geoms(),
        before,
        "the preview does not touch the document"
    );
    let preview: Vec<(EntityId, EntityGeom)> = h.app.select_tool.shape_grip_preview().to_vec();
    assert!(preview.len() >= 3, "followers are previewed: {preview:?}");

    h.release((140.0, 30.0));
    // プレビューと確定は同じ形（離した位置はプレビューの位置と同じ）。
    for (id, geom) in &preview {
        assert_eq!(h.geom(*id), *geom);
    }
    let [bottom, right, top, left] = sides.map(|id| h.line_of(id));
    let target = h.world(140.0, 30.0);
    assert!(close(bottom.b, target), "{bottom:?} {target:?}");
    // 角がつながり、水平・垂直のまま。
    let tol = 1e-6;
    for (a, b) in [
        (bottom.b, right.a),
        (right.b, top.a),
        (top.b, left.a),
        (left.b, bottom.a),
    ] {
        assert!(a.distance(b) < tol, "{a:?} {b:?}");
    }
    assert!((bottom.a.y - bottom.b.y).abs() < tol);
    assert!((top.a.y - top.b.y).abs() < tol);
    assert!((right.a.x - right.b.x).abs() < tol);
    assert!((left.a.x - left.b.x).abs() < tol);

    assert!(h.app.document.undo());
    assert_eq!(h.all_geoms(), before);
}

#[test]
fn an_unsolvable_position_keeps_the_last_solved_preview_and_release_changes_nothing() {
    let mut h = Harness::new();
    let sides = h.rectangle();
    // 上の辺を固定すると、右の辺の x は動かせない（縦には動かせる）。
    h.constrain(Constraint::FixEntity { entity: sides[2] });
    let before = h.all_geoms();
    let generation = h.app.document.generation();
    h.select(&[sides[0]]);
    h.app.status = None;

    // 真下へ: 解ける。
    h.press_and_move((100.0, 0.0), (100.0, 30.0));
    assert!(h.status().is_none(), "{:?}", h.status());
    let solved: Vec<(EntityId, EntityGeom)> = h.app.select_tool.shape_grip_preview().to_vec();
    assert!(!solved.is_empty());

    // 右へ: 解けない。直前の形を保ち、理由を出す。
    h.move_to((140.0, 30.0));
    assert_eq!(h.app.select_tool.shape_grip_preview(), solved.as_slice());
    let status = h.status().expect("the reason is shown").to_owned();
    assert!(
        status.starts_with("グリップ編集に失敗しました: "),
        "{status}"
    );
    assert_eq!(h.all_geoms(), before);

    // 解けない位置で離す: 直前の解けた形では確定せず、図面は変わらない。
    h.app.status = None;
    h.release((140.0, 30.0));
    assert!(!h.app.select_tool.is_grip_dragging());
    assert_eq!(h.all_geoms(), before);
    assert_eq!(h.app.document.generation(), generation);
    let status = h.status().expect("the reason is shown");
    assert!(
        status.starts_with("グリップ編集に失敗しました: "),
        "{status}"
    );
}

#[test]
fn a_degenerate_release_is_rejected_before_it_reaches_the_document() {
    let mut h = Harness::new();
    let l = h.line((0.0, 0.0), (100.0, 0.0));
    let before = h.line_of(l);
    let generation = h.app.document.generation();
    h.select(&[l]);
    // 始点を終点へ重ねる（スナップなしでもピクセルの位置で重なる）。
    h.press_and_move((0.0, 0.0), (100.0, 0.0));
    // 解放位置がちょうど重ならない丸めに左右されないよう、確定の入口を直接確かめる。
    h.app.select_tool.on_grip_drag(before.b);
    assert!(
        h.app
            .select_tool
            .refresh_shape_grip_preview(&h.app.document, 1.0 / h.app.viewport.zoom)
            .is_some_and(|m| m.starts_with("グリップ編集に失敗しました: "))
    );
    let tol = 1.0 / h.app.viewport.zoom;
    match h.app.select_tool.end_grip_drag(&h.app.document, tol) {
        crate::GripDragOutcome::Rejected(reason) => {
            assert!(
                reason.starts_with("グリップ編集に失敗しました: "),
                "{reason}"
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(h.line_of(l), before);
    assert_eq!(h.app.document.generation(), generation);
}

#[test]
fn a_tiny_move_does_nothing() {
    let mut h = Harness::new();
    let l = h.line((0.0, 0.0), (100.0, 0.0));
    let generation = h.app.document.generation();
    h.select(&[l]);
    h.app
        .select_tool
        .start_grip_drag(l, ShapeGripKind::LineEnd, h.world(100.0, 0.0));
    h.app.select_tool.on_grip_drag(h.world(100.5, 0.0));
    let tol = 6.0 / h.app.viewport.zoom;
    assert_eq!(
        h.app.select_tool.end_grip_drag(&h.app.document, tol),
        crate::GripDragOutcome::None
    );
    assert_eq!(h.app.document.generation(), generation);
}

#[test]
fn escape_and_undo_discard_the_drag_without_changing_the_document() {
    let mut h = Harness::new();
    let l = h.line((0.0, 0.0), (100.0, 0.0));
    let other = h.line((0.0, 80.0), (100.0, 80.0));
    let before = h.all_geoms();
    h.select(&[l]);

    h.press_and_move((100.0, 0.0), (130.0, 40.0));
    assert!(h.app.select_tool.is_grip_dragging());
    h.run(vec![key_press(Key::Escape)]);
    assert!(!h.app.select_tool.is_grip_dragging());
    assert!(h.app.select_tool.shape_grip_preview().is_empty());
    h.release((130.0, 40.0));
    assert_eq!(h.all_geoms(), before);
    assert_eq!(
        h.app.select_tool.selection(),
        [l],
        "only the drag is discarded"
    );

    // undo の経路（`Ctrl+Z`）: `other` の追加が戻り、ドラッグは捨てられる。
    h.press_and_move((100.0, 0.0), (130.0, 40.0));
    assert!(h.app.select_tool.is_grip_dragging());
    h.run_with(
        vec![egui::Event::Key {
            key: Key::Z,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        }],
        egui::Modifiers::COMMAND,
    );
    assert!(!h.app.select_tool.is_grip_dragging());
    assert!(h.app.document.entity(other).is_none());
}

#[test]
fn snapping_ignores_the_dragged_shape_but_not_the_others() {
    let mut h = Harness::new();
    let l = h.line((0.0, 0.0), (100.0, 0.0));
    let other = h.line((-3.0, 40.0), (-3.0, 90.0));
    h.app.config.snap_enabled = true;
    h.app.config.snap_kinds.grid = false;
    h.select(&[l]);

    // 自分の中点のすぐそば: 吸着しない（ほかに候補が無いのでカーソルのまま）。
    h.press_and_move((0.0, 0.0), (51.0, 1.0));
    let Some(DragPreview::Grip { point, .. }) = h.app.select_tool.drag_preview() else {
        panic!();
    };
    let midpoint = h.world(50.0, 0.0);
    assert!(point.distance(midpoint) > 1e-3, "{point:?}");
    assert!(close(point, h.world(51.0, 1.0)), "{point:?}");

    // 別の図形の端点へは吸着する。
    h.move_to((-1.0, 41.0));
    let Some(DragPreview::Grip { point, .. }) = h.app.select_tool.drag_preview() else {
        panic!();
    };
    assert_eq!(point, h.line_of(other).a);

    // 解放位置にも掛け直す。
    h.release((-1.0, 41.0));
    assert_eq!(h.line_of(l).a, h.line_of(other).a);
}

/// 同じカーソル位置では解き直さない。動かしたとき・文書が変わったときだけ解く。
#[test]
fn the_preview_is_solved_only_when_the_cursor_moves() {
    let mut h = Harness::new();
    let sides = h.rectangle();
    h.select(&[sides[0]]);
    h.press_and_move((100.0, 0.0), (120.0, 10.0));
    assert_eq!(h.app.select_tool.shape_grip_preview_solves(), 1);
    // ボタンを押したまま動かないフレーム。
    for _ in 0..5 {
        h.run(Vec::new());
        h.move_to((120.0, 10.0));
    }
    assert_eq!(h.app.select_tool.shape_grip_preview_solves(), 1);
    h.move_to((125.0, 10.0));
    assert_eq!(h.app.select_tool.shape_grip_preview_solves(), 2);
    h.move_to((125.0, 10.0));
    assert_eq!(h.app.select_tool.shape_grip_preview_solves(), 2);

    // 文書が変わったら（世代番号）同じ位置でも解き直す。
    h.app
        .document
        .apply(Command::AddLayer(Layer::new("other", Rgb::WHITE)))
        .unwrap();
    h.move_to((125.0, 10.0));
    assert_eq!(h.app.select_tool.shape_grip_preview_solves(), 3);
}

#[test]
fn shape_grips_are_not_grabbed_on_locked_layers_fixed_shapes_or_large_selections() {
    // ロックされたレイヤー: 矩形選択になる。
    let mut h = Harness::new();
    let l = h.line((0.0, 0.0), (100.0, 0.0));
    let layer = h.app.document.entity(l).unwrap().layer;
    h.select(&[l]);
    let mut props = h.app.document.layer(layer).unwrap().clone();
    props.locked = true;
    h.app
        .document
        .apply(Command::SetLayerProps { id: layer, props })
        .unwrap();
    h.press_and_move((100.0, 0.0), (130.0, 40.0));
    assert!(!h.app.select_tool.is_grip_dragging());
    assert!(matches!(
        h.app.select_tool.drag_preview(),
        Some(DragPreview::Rect { .. })
    ));
    h.release((130.0, 40.0));

    // 図形の固定。
    let mut h = Harness::new();
    let l = h.line((0.0, 0.0), (100.0, 0.0));
    h.constrain(Constraint::FixEntity { entity: l });
    h.select(&[l]);
    h.press_and_move((100.0, 0.0), (130.0, 40.0));
    assert!(!h.app.select_tool.is_grip_dragging());
    h.release((130.0, 40.0));

    // 選択が 100 個を超える。
    let mut h = Harness::new();
    let mut ids: Vec<EntityId> = (0..SHAPE_GRIP_SELECTION_LIMIT)
        .map(|i| {
            let y = 200.0 + i as f32;
            h.line((0.0, y), (100.0, y))
        })
        .collect();
    let target = h.line((0.0, 0.0), (100.0, 0.0));
    ids.push(target);
    h.select(&ids);
    h.press_and_move((100.0, 0.0), (130.0, 40.0));
    assert!(!h.app.select_tool.is_grip_dragging());
    h.release((130.0, 40.0));

    // 選択していない図形のグリップは掴めない。
    let mut h = Harness::new();
    h.line((0.0, 0.0), (100.0, 0.0));
    h.press_and_move((100.0, 0.0), (130.0, 40.0));
    assert!(!h.app.select_tool.is_grip_dragging());
    h.release((130.0, 40.0));
}

/// 単発クリックは従来どおりピックになる（グリップの上でもドラッグにならない）。
#[test]
fn a_click_on_a_shape_grip_is_a_plain_pick() {
    let mut h = Harness::new();
    let a = h.line((0.0, 0.0), (100.0, 0.0));
    let b = h.line((100.0, 0.0), (100.0, 80.0));
    let generation = h.app.document.generation();
    h.select(&[a]);
    h.click(CANVAS_POS + vec2(100.0, 0.0));
    assert!(!h.app.select_tool.is_grip_dragging());
    assert_eq!(h.app.document.generation(), generation);
    // クリックは選択への追加（`a` か `b` のどちらかを拾う。どちらでも `a` は残る）。
    let selection = h.app.select_tool.selection();
    assert!(selection.contains(&a), "{selection:?}");
    assert!(
        selection.iter().all(|id| *id == a || *id == b),
        "{selection:?}"
    );
}

/// ドラッグ開始の優先順: 寸法グリップ > 図形グリップ > 文字ブロック > 矩形選択。
#[test]
fn drag_start_priority_is_dim_grip_shape_grip_label_then_rectangle() {
    let rect = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
    let viewport = Viewport::new();
    let mut doc = Document::new();
    let layer = doc.current_layer();
    let world = |x: f32, y: f32| viewport.screen_to_world(rect, Pos2::new(x, y));
    let add = |doc: &mut Document, geom: EntityGeom| {
        doc.apply(Command::AddEntity(Entity::new(
            geom,
            layer,
            Style::inherited(),
        )))
        .unwrap()
        .entities[0]
    };
    // 文字ブロックを離れた位置へ置いた寸法。
    let (p1, p2) = (world(100.0, 300.0), world(200.0, 300.0));
    let anchor = world(500.0, 100.0);
    let dim = add(
        &mut doc,
        EntityGeom::DimLinear(DimLinear {
            p1,
            p2,
            offset: 30.0 / viewport.zoom,
            direction: DimDirection::Aligned,
            ext_angle: None,
            annotation: DimAnnotation {
                text_anchor: Some(anchor),
                ..DimAnnotation::default()
            },
        }),
    );
    // 寸法の p1 に始点を重ねた線分と、文字ブロックの中に端点を置いた線分。
    let on_grip = add(
        &mut doc,
        EntityGeom::Shape(Shape::Line(LineSeg::new(p1, world(50.0, 400.0)))),
    );
    let on_label = add(
        &mut doc,
        EntityGeom::Shape(Shape::Line(LineSeg::new(anchor, world(600.0, 400.0)))),
    );

    let begin = |selection: &[EntityId], press: Pos2| {
        let mut tool = SelectTool::default();
        tool.set_selection(selection.to_vec());
        start_select_drag(&doc, &mut tool, rect, &viewport, press, false, 1.0);
        tool
    };
    let press_grip = viewport.world_to_screen(rect, p1);
    let press_label = viewport.world_to_screen(rect, anchor);

    // 寸法のグリップと図形のグリップが重なる: 選択順に依らず寸法。
    for selection in [[on_grip, dim], [dim, on_grip]] {
        let tool = begin(&selection, press_grip);
        assert!(
            matches!(
                tool.drag_preview(),
                Some(DragPreview::Grip {
                    kind: GripKind::Dim(_),
                    id,
                    ..
                }) if id == dim
            ),
            "{:?}",
            tool.drag_preview()
        );
    }
    // 図形のグリップだけ。
    let tool = begin(&[on_grip], press_grip);
    assert!(matches!(
        tool.drag_preview(),
        Some(DragPreview::Grip {
            kind: GripKind::Shape(ShapeGripKind::LineStart),
            ..
        })
    ));
    // 文字ブロックと図形のグリップが重なる: 図形のグリップ。
    let tool = begin(&[dim, on_label], press_label);
    assert!(matches!(
        tool.drag_preview(),
        Some(DragPreview::Grip {
            kind: GripKind::Shape(ShapeGripKind::LineStart),
            id,
            ..
        }) if id == on_label
    ));
    // 文字ブロックだけ。
    let tool = begin(&[dim], press_label);
    assert!(tool.is_text_dragging(), "{:?}", tool.drag_preview());
    // 何も無い。
    let tool = begin(&[dim, on_label, on_grip], Pos2::new(700.0, 550.0));
    assert!(matches!(
        tool.drag_preview(),
        Some(DragPreview::Rect { .. })
    ));
    // 当たり判定の窓: 枠の外。
    assert!(
        grip_hit(
            &doc,
            &[on_grip],
            rect,
            &viewport,
            press_grip + vec2(20.0, 0.0)
        )
        .is_none()
    );
}

/// キャッシュと拒否の単体（egui を通さない）。
#[test]
fn refresh_reports_the_failure_and_keeps_the_last_shapes() {
    let mut doc = Document::new();
    let layer = doc.current_layer();
    let l = doc
        .apply(Command::AddEntity(Entity::new(
            Shape::Line(LineSeg::new(Point2::new(0.0, 0.0), Point2::new(10.0, 0.0))),
            layer,
            Style::inherited(),
        )))
        .unwrap()
        .entities[0];
    let mut tool = SelectTool::default();
    tool.set_selection(vec![l]);
    tool.start_grip_drag(l, ShapeGripKind::LineEnd, Point2::new(10.0, 0.0));
    assert_eq!(tool.shape_grip_target(), Some(l));

    tool.on_grip_drag(Point2::new(5.0, 5.0));
    assert_eq!(tool.refresh_shape_grip_preview(&doc, 0.01), None);
    let shown = tool.shape_grip_preview().to_vec();
    assert_eq!(shown.len(), 1);

    // 両端が一致: 退化で拒否。直前の形を保つ。同じ位置なら同じ文言を返し、解き直さない。
    tool.on_grip_drag(Point2::new(0.0, 0.0));
    let failure = tool
        .refresh_shape_grip_preview(&doc, 0.01)
        .expect("rejected");
    assert!(
        failure.starts_with("グリップ編集に失敗しました: "),
        "{failure}"
    );
    assert_eq!(tool.shape_grip_preview(), shown.as_slice());
    let solves = tool.shape_grip_preview_solves();
    assert_eq!(tool.refresh_shape_grip_preview(&doc, 0.01), Some(failure));
    assert_eq!(tool.shape_grip_preview_solves(), solves);

    // 解ける位置へ戻ると文言は消える。
    tool.on_grip_drag(Point2::new(5.0, 6.0));
    assert_eq!(tool.refresh_shape_grip_preview(&doc, 0.01), None);
    assert_ne!(tool.shape_grip_preview(), shown.as_slice());

    // 寸法のグリップでは何もしない。
    tool.cancel_grip_drag();
    assert!(tool.shape_grip_preview().is_empty());
    tool.start_grip_drag(l, crate::tool::DimGripKind::LinearP1, Point2::new(0.0, 0.0));
    assert_eq!(tool.shape_grip_target(), None);
    assert_eq!(tool.refresh_shape_grip_preview(&doc, 0.01), None);
    assert!(tool.shape_grip_preview().is_empty());
}

/// プレビューが予算で打ち切られる大きさの連鎖: 掴んだ図形だけを描いて理由を出し、離すと確定の
/// 予算で解いて全体が追従する（M13 タスク101 後半、設計判断7 の改訂）。60 本は、小さな移動でも
/// 1 フレームの予算を超え、debug でも確定が 1 秒程度で済む大きさ。
#[test]
fn a_preview_too_large_to_solve_shows_only_the_grabbed_shape_and_release_still_applies() {
    let mut h = Harness::new();
    let node = |i: u32| (10.0 * i as f32, if i % 2 == 1 { -6.0 } else { 0.0 });
    let links: Vec<EntityId> = (0..60).map(|i| h.line(node(i), node(i + 1))).collect();
    let mut constraints: Vec<Command> = links
        .windows(2)
        .map(|pair| Command::AddConstraint {
            constraint: Constraint::Coincident {
                a: PointRef::new(pair[0], PointKind::End),
                b: PointRef::new(pair[1], PointKind::Start),
            },
            hold: None,
        })
        .collect();
    for &link in &links {
        constraints.push(Command::AddConstraint {
            constraint: Constraint::distance(
                &h.app.document,
                PointRef::new(link, PointKind::Start),
                PointRef::new(link, PointKind::End),
            )
            .unwrap(),
            hold: None,
        });
    }
    h.app.document.apply(Command::Batch(constraints)).unwrap();
    let before = h.all_geoms();
    let second_before = h.geom(links[1]);
    let generation = h.app.document.generation();
    h.select(&[links[0]]);
    h.app.status = None;

    // 先頭の終点を引く。プレビューは打ち切られ、掴んだ図形だけが見える。
    h.press_and_move(node(1), (18.0, -12.0));
    assert!(h.app.select_tool.is_grip_dragging());
    assert_eq!(h.status(), Some(crate::tool::SHAPE_GRIP_PREVIEW_TOO_LARGE));
    let target = h.world(18.0, -12.0);
    let check_grabbed_only =
        |h: &Harness, target: Point2| match h.app.select_tool.shape_grip_preview() {
            [(id, geom)] => {
                assert_eq!(*id, links[0]);
                let Some(Shape::Line(shown)) = geom.as_shape() else {
                    panic!("{geom:?}");
                };
                assert!(close(shown.b, target), "{shown:?} {target:?}");
                // 反対の端点は解いていないので元のまま。
                assert_eq!(shown.a, h.line_of(links[0]).a);
            }
            other => panic!("{other:?}"),
        };
    check_grabbed_only(&h, target);
    assert_eq!(h.all_geoms(), before);
    assert_eq!(h.app.document.generation(), generation);

    // カーソルに追従する。
    h.move_to((22.0, -14.0));
    let target = h.world(22.0, -14.0);
    check_grabbed_only(&h, target);

    // 離すと確定の予算で解けて、連鎖が追従する。undo 1 回で全部戻る。
    h.release((22.0, -14.0));
    assert!(!h.app.select_tool.is_grip_dragging());
    assert!(close(h.line_of(links[0]).b, target));
    assert!(h.app.document.generation() != generation);
    for pair in links.windows(2) {
        assert!(h.line_of(pair[0]).b.distance(h.line_of(pair[1]).a) < 1e-6);
    }
    assert_ne!(h.geom(links[1]), second_before, "the chain followed");
    assert!(h.app.document.undo());
    assert_eq!(h.all_geoms(), before);
}
