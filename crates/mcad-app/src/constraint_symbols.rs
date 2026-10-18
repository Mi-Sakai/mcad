//! 拘束記号の配置と描画（M13 タスク99、DESIGN.md M13 設計判断6）。
//!
//! 配置（[`constraint_symbols`]: 文書とビューポート → 画面上の記号の一覧）と、egui への
//! 描画（[`draw_constraint_symbols`]）を分ける。タスク100 の記号クリック選択は、この一覧の
//! `center`・`size` でヒットテストする。
//!
//! 記号はビューポートにだけ描く（作図の道具であって図面の内容ではないため、SVG/PDF・DXF
//! には出さない。`plot` はこのモジュールを参照しない）。
//!
//! # 配置の規則
//!
//! - 大きさは画面上の固定ピクセル（ズームに依らない）。
//! - 図形に付く記号は、図形ごとの基準点から外向きの法線方向へ、[`mcad_core::ConstraintId`]
//!   順に積む（同じ図形に複数付いても重ならない）。基準点と法線は、線分が画面上の中点と
//!   画面の上向き（垂直な線分なら右向き）の法線、円が 45° の点と半径方向、円弧が掃引の
//!   中央の点と半径方向、点（点の固定）がその点と右上向き。
//! - 2 対象の拘束（平行・直交・等長・等半径・角度）は両方の対象に同じ記号を描く。
//! - 一致は点そのものに丸印、接線は接点の外側に `T`。点どうしの距離（同じ線分の両端なら
//!   その線分に積む）・点と線の距離は 2 点の中ほどに置き、積まない。
//! - 接点が線分の区間・円弧の掃引範囲の外なら、接点からその図形の最寄りの端点まで破線
//!   （[`ConstraintSymbol::dashes`]）。
//! - 非表示レイヤーの図形の記号は出さない（2 対象の記号は見えている側だけ。2 点の間に
//!   置く記号と接線は、参照する図形がすべて見えているときだけ）。

use egui::{Color32, FontId, Painter, Pos2, Rect, Stroke, Vec2, vec2};

use mcad_core::{Constraint, ConstraintId, Document, EntityId, PointRef};
use mcad_geom::{Point2, Shape};

use crate::constraint_tool::ConstraintKind;
use crate::tool::layer_visible;
use crate::viewport::Viewport;

/// 拘束記号の色。寸法（既定は白）・選択（水色）・作図プレビュー（琥珀）・スナップ（緑）・
/// 極トラッキング（薄い水色）・既定レイヤーの 6 色のどれとも違う桃色。キャンバスの背景は
/// テーマに依らず暗色（`from_gray(30)`）で、記号は暗い下地の上に描くので図形の色とも
/// 重ならない。
pub const CONSTRAINT_COLOR: Color32 = Color32::from_rgb(255, 120, 190);
/// 記号の下地（図形の上でも読めるよう不透明に近い暗色）。
const SYMBOL_BG_COLOR: Color32 = Color32::from_rgba_premultiplied(20, 20, 20, 220);
/// 記号の文字の大きさ（px）。
const SYMBOL_FONT_PX: f32 = 12.0;
/// 下地の内側の余白（px）。
const SYMBOL_PAD_PX: f32 = 3.0;
/// 文字の記号の高さの見積もり（px。文字 + 上下の余白）。
const SYMBOL_HEIGHT_PX: f32 = 16.0;
/// 半角文字 1 字の幅の見積もり（px）。
const ASCII_WIDTH_PX: f32 = 7.0;
/// 全角の記号（`∥`・`⊥`・`∠` 等）1 字の幅の見積もり（px）。
const WIDE_WIDTH_PX: f32 = 12.0;
/// 錠前の記号の大きさ（px）。
const LOCK_SIZE_PX: f32 = 14.0;
/// 一致の丸印の半径（px）。
const DOT_RADIUS_PX: f32 = 3.5;
/// 基準点から最初の記号までの隙間（px）。
const SYMBOL_GAP_PX: f32 = 6.0;
/// 積んだ記号どうしの隙間（px）。
const SYMBOL_STACK_GAP_PX: f32 = 2.0;
/// 接線の延長を示す破線のダッシュ長・ギャップ長（px）。
const DASH_PX: f32 = 4.0;
const DASH_GAP_PX: f32 = 3.0;

/// 記号の見た目。
#[derive(Debug, Clone, PartialEq)]
pub enum SymbolGlyph {
    /// 下地つきの文字（`H`・`V`・`∥`・`⊥`・`=`・`T`、値つきの `↔ 12.5`・`R 5`・`∠ 30°`）。
    Text(String),
    /// 錠前（点の固定・図形の固定）。
    Lock,
    /// 丸印（一致）。
    Dot,
}

/// 画面上の拘束記号 1 つ。
#[derive(Debug, Clone, PartialEq)]
pub struct ConstraintSymbol {
    /// 記号の拘束（2 対象の記号は同じ ID の記号が 2 つ出る）。
    pub id: ConstraintId,
    /// 拘束の種類。
    pub kind: ConstraintKind,
    /// 記号の中心（スクリーン座標）。
    pub center: Pos2,
    /// 記号の大きさの見積もり（px。配置とヒットテストに使う。描画は実際の文字幅）。
    pub size: Vec2,
    /// 見た目。
    pub glyph: SymbolGlyph,
    /// 接線が延長上で接しているとき、接点から図形の最寄りの端点までの破線（スクリーン座標）。
    pub dashes: Vec<[Pos2; 2]>,
}

/// 値の表示（mm は小数 3 桁、度は小数 2 桁まで。末尾の 0 は落とす）。
fn format_number(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        s
    };
    if s == "-0" { "0".to_owned() } else { s }
}

/// 文字の記号の大きさの見積もり。
fn text_size(text: &str) -> Vec2 {
    let w: f32 = text
        .chars()
        .map(|c| {
            if c.is_ascii() {
                ASCII_WIDTH_PX
            } else {
                WIDE_WIDTH_PX
            }
        })
        .sum();
    vec2(w + 2.0 * SYMBOL_PAD_PX, SYMBOL_HEIGHT_PX)
}

fn glyph_size(glyph: &SymbolGlyph) -> Vec2 {
    match glyph {
        SymbolGlyph::Text(t) => text_size(t),
        SymbolGlyph::Lock => vec2(LOCK_SIZE_PX, LOCK_SIZE_PX),
        SymbolGlyph::Dot => vec2(2.0 * DOT_RADIUS_PX + 2.0, 2.0 * DOT_RADIUS_PX + 2.0),
    }
}

/// 長さ 0 なら `fallback`。
fn normalized_or(v: Vec2, fallback: Vec2) -> Vec2 {
    let len = v.length();
    if len > 1e-6 && len.is_finite() {
        v / len
    } else {
        fallback
    }
}

/// 画面の上向き（`y < 0`）の法線。水平に近くない限り上側、ちょうど垂直なら右側。
fn upward_normal(dir: Vec2) -> Vec2 {
    let n = vec2(dir.y, -dir.x);
    if n.y < 0.0 || (n.y == 0.0 && n.x > 0.0) {
        n
    } else {
        -n
    }
}

/// 記号を積む基準（図形・点ごと）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StackKey {
    Entity(EntityId),
    Point(PointRef),
}

/// 基準ごとの、次の記号を置く距離（基準点から法線方向へ px）。
#[derive(Default)]
struct Stacks {
    used: Vec<(StackKey, f32)>,
}

impl Stacks {
    /// `key` の基準点 `anchor` から法線 `n` の向きへ、大きさ `size` の記号を 1 つ積んだ中心。
    fn push(&mut self, key: StackKey, anchor: Pos2, n: Vec2, size: Vec2) -> Pos2 {
        // 法線方向の半幅（軸に平行な箱を法線へ射影した長さ）。
        let extent = n.x.abs() * size.x * 0.5 + n.y.abs() * size.y * 0.5;
        let offset = match self.used.iter_mut().find(|(k, _)| *k == key) {
            Some((_, used)) => {
                let o = *used;
                *used += 2.0 * extent + SYMBOL_STACK_GAP_PX;
                o
            }
            None => {
                self.used
                    .push((key, SYMBOL_GAP_PX + 2.0 * extent + SYMBOL_STACK_GAP_PX));
                SYMBOL_GAP_PX
            }
        };
        anchor + n * (offset + extent)
    }
}

/// 拘束記号の配置を求める（純関数。描画は [`draw_constraint_symbols`]）。
///
/// 拘束が 1 件も無い文書では何もしない（拘束の無い図面の描画性能を変えない。
/// DESIGN.md M13 検収基準 (xiii)）。
pub fn constraint_symbols(
    document: &Document,
    viewport: &Viewport,
    rect: Rect,
) -> Vec<ConstraintSymbol> {
    if document.constraint_count() == 0 {
        return Vec::new();
    }
    let to_screen = |p: Point2| viewport.world_to_screen(rect, p);
    let shape_of = |id: EntityId| -> Option<&Shape> {
        let entity = document.entity(id)?;
        if !layer_visible(document, entity) {
            return None;
        }
        entity.geom.as_shape()
    };
    let visible = |ids: &[EntityId]| ids.iter().all(|&id| shape_of(id).is_some());
    // 図形の記号の基準点と外向きの法線（スクリーン座標）。
    let entity_anchor = |id: EntityId| -> Option<(Pos2, Vec2)> {
        match shape_of(id)? {
            Shape::Line(l) => {
                let (a, b) = (to_screen(l.a), to_screen(l.b));
                let dir = normalized_or(b - a, vec2(1.0, 0.0));
                Some((a + (b - a) * 0.5, upward_normal(dir)))
            }
            Shape::Circle(c) => {
                let t = std::f64::consts::FRAC_PI_4;
                let p = Point2::new(
                    c.center.x + c.radius * t.cos(),
                    c.center.y + c.radius * t.sin(),
                );
                Some((to_screen(p), vec2(t.cos() as f32, -(t.sin() as f32))))
            }
            Shape::Arc(a) => {
                let t = a.start_angle + a.sweep() * 0.5;
                let p = Point2::new(
                    a.center.x + a.radius * t.cos(),
                    a.center.y + a.radius * t.sin(),
                );
                Some((to_screen(p), vec2(t.cos() as f32, -(t.sin() as f32))))
            }
            Shape::Point(_) | Shape::Polyline(_) => None,
        }
    };
    let point_screen = |p: PointRef| -> Option<Pos2> {
        shape_of(p.entity)?;
        p.position(document).map(to_screen)
    };

    let mut out = Vec::new();
    let mut stacks = Stacks::default();
    for (id, constraint) in document.constraints() {
        let kind = ConstraintKind::of(constraint);
        let mut on_entity =
            |out: &mut Vec<ConstraintSymbol>, entity: EntityId, glyph: SymbolGlyph| {
                if let Some((anchor, n)) = entity_anchor(entity) {
                    let size = glyph_size(&glyph);
                    let center = stacks.push(StackKey::Entity(entity), anchor, n, size);
                    out.push(ConstraintSymbol {
                        id,
                        kind,
                        center,
                        size,
                        glyph,
                        dashes: Vec::new(),
                    });
                }
            };
        let free = |center: Pos2, glyph: SymbolGlyph, dashes: Vec<[Pos2; 2]>| ConstraintSymbol {
            id,
            kind,
            center,
            size: glyph_size(&glyph),
            glyph,
            dashes,
        };
        let text = |s: &str| SymbolGlyph::Text(s.to_owned());
        match *constraint {
            Constraint::Coincident { a, b } => {
                if visible(&[a.entity, b.entity])
                    && let Some(p) = point_screen(a)
                {
                    out.push(free(p, SymbolGlyph::Dot, Vec::new()));
                }
            }
            Constraint::Horizontal { line } => on_entity(&mut out, line, text("H")),
            Constraint::Vertical { line } => on_entity(&mut out, line, text("V")),
            Constraint::Parallel { a, b } => {
                on_entity(&mut out, a, text("∥"));
                on_entity(&mut out, b, text("∥"));
            }
            Constraint::Perpendicular { a, b } => {
                on_entity(&mut out, a, text("⊥"));
                on_entity(&mut out, b, text("⊥"));
            }
            Constraint::EqualLength { a, b } | Constraint::EqualRadius { a, b } => {
                on_entity(&mut out, a, text("="));
                on_entity(&mut out, b, text("="));
            }
            Constraint::Angle { a, b, value } => {
                let label = format!("∠ {}°", format_number(value.to_degrees(), 2));
                on_entity(&mut out, a, text(&label));
                on_entity(&mut out, b, text(&label));
            }
            Constraint::Radius { entity, value } => {
                on_entity(
                    &mut out,
                    entity,
                    text(&format!("R {}", format_number(value, 3))),
                );
            }
            Constraint::FixEntity { entity } => on_entity(&mut out, entity, SymbolGlyph::Lock),
            Constraint::FixPoint { point, .. } => {
                if let Some(p) = point_screen(point) {
                    let n = vec2(1.0, -1.0).normalized();
                    let size = glyph_size(&SymbolGlyph::Lock);
                    let center = stacks.push(StackKey::Point(point), p, n, size);
                    out.push(ConstraintSymbol {
                        id,
                        kind,
                        center,
                        size,
                        glyph: SymbolGlyph::Lock,
                        dashes: Vec::new(),
                    });
                }
            }
            Constraint::Distance { a, b, value } => {
                let label = text(&format!("↔ {}", format_number(value, 3)));
                // 同じ線分の始点と終点（= 線分の長さ）は、その線分の記号として積む。
                let same_line =
                    a.entity == b.entity && matches!(shape_of(a.entity), Some(Shape::Line(_)));
                if same_line {
                    on_entity(&mut out, a.entity, label);
                } else if visible(&[a.entity, b.entity])
                    && let (Some(p), Some(q)) = (point_screen(a), point_screen(b))
                {
                    let n = upward_normal(normalized_or(q - p, vec2(1.0, 0.0)));
                    let size = glyph_size(&label);
                    let extent = n.x.abs() * size.x * 0.5 + n.y.abs() * size.y * 0.5;
                    let center = p + (q - p) * 0.5 + n * (SYMBOL_GAP_PX + extent);
                    out.push(free(center, label, Vec::new()));
                }
            }
            Constraint::PointLineDistance {
                point, line, value, ..
            } => {
                if visible(&[point.entity, line])
                    && let (Some(p), Some(Shape::Line(l))) =
                        (point.position(document), shape_of(line))
                {
                    let foot = foot_on_line(l.a, l.b, p);
                    let center = to_screen(p) + (to_screen(foot) - to_screen(p)) * 0.5;
                    let label = text(&format!("↔ {}", format_number(value, 3)));
                    out.push(free(center, label, Vec::new()));
                }
            }
            Constraint::TangentLineCircle { line, circle, .. } => {
                if visible(&[line, circle])
                    && let Some(contact) = constraint.tangent_contact(document)
                {
                    let center_of = round_center(shape_of(circle));
                    let mut dashes = Vec::new();
                    if contact.first_outside
                        && let Some(end) = nearest_end(shape_of(line), contact.point)
                    {
                        dashes.push([to_screen(contact.point), to_screen(end)]);
                    }
                    if contact.second_outside
                        && let Some(end) = nearest_end(shape_of(circle), contact.point)
                    {
                        dashes.push([to_screen(contact.point), to_screen(end)]);
                    }
                    let glyph = text("T");
                    let center = beside(to_screen(contact.point), center_of.map(to_screen), &glyph);
                    out.push(free(center, glyph, dashes));
                }
            }
            Constraint::TangentCircles { a, b, .. } => {
                if visible(&[a, b])
                    && let Some(contact) = constraint.tangent_contact(document)
                {
                    let mut dashes = Vec::new();
                    if contact.first_outside
                        && let Some(end) = nearest_end(shape_of(a), contact.point)
                    {
                        dashes.push([to_screen(contact.point), to_screen(end)]);
                    }
                    if contact.second_outside
                        && let Some(end) = nearest_end(shape_of(b), contact.point)
                    {
                        dashes.push([to_screen(contact.point), to_screen(end)]);
                    }
                    let glyph = text("T");
                    let center = beside(
                        to_screen(contact.point),
                        round_center(shape_of(a)).map(to_screen),
                        &glyph,
                    );
                    out.push(free(center, glyph, dashes));
                }
            }
        }
    }
    out
}

/// 直線 `a`-`b`（延長した直線）へ `p` から下ろした垂線の足。退化していれば `a`。
fn foot_on_line(a: Point2, b: Point2, p: Point2) -> Point2 {
    let d = b - a;
    let len2 = d.x * d.x + d.y * d.y;
    if len2 <= 0.0 || !len2.is_finite() {
        return a;
    }
    let t = ((p.x - a.x) * d.x + (p.y - a.y) * d.y) / len2;
    Point2::new(a.x + d.x * t, a.y + d.y * t)
}

/// 円・円弧の中心。
fn round_center(shape: Option<&Shape>) -> Option<Point2> {
    match shape? {
        Shape::Circle(c) => Some(c.center),
        Shape::Arc(a) => Some(a.center),
        _ => None,
    }
}

/// 線分・円弧の端点のうち `p` に近い方（円・その他は `None`）。
fn nearest_end(shape: Option<&Shape>, p: Point2) -> Option<Point2> {
    let (s, e) = match shape? {
        Shape::Line(l) => (l.a, l.b),
        Shape::Arc(a) => (a.start_point(), a.end_point()),
        _ => return None,
    };
    Some(if s.distance(p) <= e.distance(p) { s } else { e })
}

/// 接点 `contact` の、円の中心 `center` から見て外側に記号を置いた中心。
fn beside(contact: Pos2, center: Option<Pos2>, glyph: &SymbolGlyph) -> Pos2 {
    let n = normalized_or(
        center.map_or(vec2(0.0, -1.0), |c| contact - c),
        vec2(0.0, -1.0),
    );
    let size = glyph_size(glyph);
    let extent = n.x.abs() * size.x * 0.5 + n.y.abs() * size.y * 0.5;
    contact + n * (SYMBOL_GAP_PX + extent)
}

/// 拘束記号を描く（[`constraint_symbols`] の結果をそのまま描く）。
pub fn draw_constraint_symbols(painter: &Painter, symbols: &[ConstraintSymbol]) {
    let stroke = Stroke::new(1.0, CONSTRAINT_COLOR);
    for symbol in symbols {
        for [from, to] in &symbol.dashes {
            painter.extend(egui::Shape::dashed_line(
                &[*from, *to],
                stroke,
                DASH_PX,
                DASH_GAP_PX,
            ));
        }
        match &symbol.glyph {
            SymbolGlyph::Text(text) => {
                let galley = painter.layout_no_wrap(
                    text.clone(),
                    FontId::proportional(SYMBOL_FONT_PX),
                    CONSTRAINT_COLOR,
                );
                let size = galley.size() + vec2(2.0 * SYMBOL_PAD_PX, 2.0 * SYMBOL_PAD_PX);
                let rect = Rect::from_center_size(symbol.center, size);
                painter.rect_filled(rect, 3.0, SYMBOL_BG_COLOR);
                painter.rect_stroke(rect, 3.0, stroke, egui::StrokeKind::Inside);
                painter.galley(
                    rect.min + vec2(SYMBOL_PAD_PX, SYMBOL_PAD_PX),
                    galley,
                    CONSTRAINT_COLOR,
                );
            }
            SymbolGlyph::Lock => draw_lock(painter, symbol.center, stroke),
            SymbolGlyph::Dot => {
                painter.circle_filled(symbol.center, DOT_RADIUS_PX, CONSTRAINT_COLOR);
                painter.circle_stroke(
                    symbol.center,
                    DOT_RADIUS_PX + 1.0,
                    Stroke::new(1.0, SYMBOL_BG_COLOR),
                );
            }
        }
    }
}

/// 錠前の簡易図形（下地の箱・胴・つる）。
fn draw_lock(painter: &Painter, center: Pos2, stroke: Stroke) {
    let s = LOCK_SIZE_PX;
    let bg = Rect::from_center_size(center, vec2(s, s));
    painter.rect_filled(bg, 3.0, SYMBOL_BG_COLOR);
    // 胴（下半分の塗った箱）。
    let body = Rect::from_min_max(
        Pos2::new(center.x - s * 0.3, center.y - s * 0.05),
        Pos2::new(center.x + s * 0.3, center.y + s * 0.38),
    );
    painter.rect_filled(body, 1.0, CONSTRAINT_COLOR);
    // つる（胴の上の半円）。
    let r = s * 0.2;
    let c = Pos2::new(center.x, center.y - s * 0.05);
    let arc: Vec<Pos2> = (0..=8)
        .map(|i| {
            let t = std::f32::consts::PI * (i as f32) / 8.0;
            Pos2::new(c.x - r * t.cos(), c.y - r * 1.4 * t.sin())
        })
        .collect();
    painter.add(egui::Shape::line(arc, Stroke::new(1.5, stroke.color)));
}

#[cfg(test)]
mod tests;
