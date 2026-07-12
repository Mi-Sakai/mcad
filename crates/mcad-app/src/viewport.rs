//! [`Viewport`]: ワールド座標（f64）⇔スクリーン座標（egui, f32）の変換とズーム/パン。
//!
//! DESIGN.md 3.4 の Viewport 要件を実装する。
//!
//! # 座標系の向き
//!
//! ワールド座標は数学・CAD の慣習に従い y 軸が上向き。egui のスクリーン座標は
//! y 軸が下向き（画面上端が 0、下に行くほど増加）なので、変換時に y の符号を反転する。

use egui::{Pos2, Rect, Vec2 as EguiVec2};

use mcad_geom::{Aabb, Point2};

/// ズーム倍率の下限・上限。極端な値で座標変換が発散/消失するのを防ぐ。
pub const MIN_ZOOM: f64 = 1e-6;
pub const MAX_ZOOM: f64 = 1e6;

/// ワールド(f64)↔スクリーン(egui, f32)変換とズーム/パン状態を保持するビューポート。
///
/// `zoom` はワールド単位→スクリーンピクセルの倍率、`center` は画面中心が指す
/// ワールド座標。変換関数はいずれも、現在描画対象となっているスクリーン矩形
/// （`screen_rect`）を受け取る。これはキャンバス領域がフレームごとに変わりうる
/// （ウィンドウリサイズ、パネルレイアウト変更）ため、`Viewport` 自身には
/// スクリーン矩形を保持させない設計。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    /// ワールド単位 → スクリーンピクセルの倍率。
    pub zoom: f64,
    /// 画面中心が指すワールド座標。
    pub center: Point2,
}

impl Viewport {
    /// 既定のビューポート（ズーム1倍、原点中心）。
    #[must_use]
    pub fn new() -> Self {
        Self {
            zoom: 1.0,
            center: Point2::ORIGIN,
        }
    }

    /// ワールド座標をスクリーン座標へ変換する。
    #[must_use]
    pub fn world_to_screen(&self, screen_rect: Rect, p: Point2) -> Pos2 {
        let sc = screen_rect.center();
        let dx = (p.x - self.center.x) * self.zoom;
        let dy = (p.y - self.center.y) * self.zoom;
        // y は上向き(ワールド)→下向き(スクリーン)なので符号反転。
        Pos2::new(sc.x + dx as f32, sc.y - dy as f32)
    }

    /// スクリーン座標をワールド座標へ変換する（`world_to_screen` の逆変換）。
    #[must_use]
    pub fn screen_to_world(&self, screen_rect: Rect, p: Pos2) -> Point2 {
        let sc = screen_rect.center();
        let dx = f64::from(p.x - sc.x) / self.zoom;
        let dy = f64::from(sc.y - p.y) / self.zoom;
        Point2::new(self.center.x + dx, self.center.y + dy)
    }

    /// スクリーン矩形 `screen_rect` に現在のビューポートで見えているワールド空間の
    /// AABB を返す（ビューポートカリング用）。
    #[must_use]
    pub fn visible_aabb(&self, screen_rect: Rect) -> Aabb {
        let a = self.screen_to_world(screen_rect, screen_rect.left_top());
        let b = self.screen_to_world(screen_rect, screen_rect.right_bottom());
        Aabb::from_corners(a, b)
    }

    /// カーソル位置 `cursor`（スクリーン座標）を中心に `zoom_factor` 倍する。
    ///
    /// `cursor` が指しているワールド座標が、ズーム後も同じスクリーン位置に
    /// 留まるように `center` を調整する。`zoom_factor` は 1.0 より大きければ
    /// 拡大、小さければ縮小。結果の `zoom` は [`MIN_ZOOM`]..=[`MAX_ZOOM`] にクランプする。
    pub fn zoom_at(&mut self, screen_rect: Rect, cursor: Pos2, zoom_factor: f64) {
        if !zoom_factor.is_finite() || zoom_factor <= 0.0 {
            return;
        }
        let world_before = self.screen_to_world(screen_rect, cursor);
        self.zoom = (self.zoom * zoom_factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let world_after = self.screen_to_world(screen_rect, cursor);
        // world_before が再び cursor の位置に来るよう center を平行移動する。
        self.center.x += world_before.x - world_after.x;
        self.center.y += world_before.y - world_after.y;
    }

    /// スクリーン空間の変位 `screen_delta`（ドラッグ量）だけパンする。
    ///
    /// カーソルでキャンバスを掴んでドラッグする操作を想定しており、ドラッグ方向に
    /// 引きずられて画面内容がついてくるように（= `center` はドラッグと逆方向へ動く
    /// ように、ただし y はスクリーンとワールドで向きが逆なので符号が反転する）`center`
    /// を更新する。
    pub fn pan_by_screen_delta(&mut self, screen_delta: EguiVec2) {
        if self.zoom.abs() <= f64::EPSILON {
            return;
        }
        self.center.x -= f64::from(screen_delta.x) / self.zoom;
        self.center.y += f64::from(screen_delta.y) / self.zoom;
    }
}

impl Default for Viewport {
    fn default() -> Self {
        Self::new()
    }
}

/// スクリーン上の目標間隔 `target_px` に近くなるような「きりのよい」グリッド間隔
/// （ワールド単位）を選ぶ。
///
/// 1・2・5・10 の倍数系列から選択する、一般的なグラフ用グリッド刻み選定アルゴリズム。
/// `zoom` は世界単位→ピクセルの倍率（[`Viewport::zoom`] と同じ意味）。
#[must_use]
pub fn nice_grid_step(zoom: f64, target_px: f64) -> f64 {
    let zoom = zoom.abs().max(MIN_ZOOM);
    // target_px ピクセルに相当するワールド単位の量。
    let raw = target_px / zoom;
    let exponent = raw.log10().floor();
    let base = 10f64.powf(exponent);
    let fraction = raw / base;
    let nice = if fraction < 1.5 {
        1.0
    } else if fraction < 3.5 {
        2.0
    } else if fraction < 7.5 {
        5.0
    } else {
        10.0
    };
    nice * base
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(w: f32, h: f32) -> Rect {
        Rect::from_min_size(Pos2::new(0.0, 0.0), egui::vec2(w, h))
    }

    #[test]
    fn world_screen_roundtrip_is_identity() {
        // スクリーン座標は egui の f32 (`Pos2`) を経由するため、DESIGN.md の方針
        // （「描画時のみf32へ変換」）どおり、ここでの往復は f32 精度に由来する
        // 誤差（1e-5 オーダー）までは許容する。恒等でなくなる（丸め誤差以上に
        // ずれる）ようなロジックバグのみを検出できれば十分。
        let vp = Viewport {
            zoom: 3.5,
            center: Point2::new(12.3, -4.2),
        };
        let r = rect(800.0, 600.0);
        // 極端に画面外へ離れた座標（スクリーン側で f32 の ULP が粗くなる領域）は
        // 実運用でも AABB カリングで描画対象にならないため、ここでは
        // 現実的にスクリーン近傍～画面外程度の範囲に留める。
        let pts = [
            Point2::new(0.0, 0.0),
            Point2::new(12.3, -4.2),
            Point2::new(-100.0, 250.0),
            Point2::new(500.0, -500.0),
        ];
        for p in pts {
            let screen = vp.world_to_screen(r, p);
            let back = vp.screen_to_world(r, screen);
            assert!((back.x - p.x).abs() < 1e-4, "x roundtrip failed for {p:?}");
            assert!((back.y - p.y).abs() < 1e-4, "y roundtrip failed for {p:?}");
        }
    }

    #[test]
    fn screen_world_roundtrip_is_identity() {
        let vp = Viewport {
            zoom: 0.25,
            center: Point2::new(-7.0, 9.0),
        };
        let r = rect(1024.0, 768.0);
        let screen_pts = [
            Pos2::new(0.0, 0.0),
            r.center(),
            Pos2::new(1024.0, 768.0),
            Pos2::new(500.0, 100.0),
        ];
        for sp in screen_pts {
            let world = vp.screen_to_world(r, sp);
            let back = vp.world_to_screen(r, world);
            assert!(
                (back.x - sp.x).abs() < 1e-3,
                "x roundtrip failed for {sp:?}"
            );
            assert!(
                (back.y - sp.y).abs() < 1e-3,
                "y roundtrip failed for {sp:?}"
            );
        }
    }

    #[test]
    fn center_maps_to_screen_rect_center() {
        let vp = Viewport {
            zoom: 2.0,
            center: Point2::new(5.0, 5.0),
        };
        let r = rect(400.0, 300.0);
        let screen = vp.world_to_screen(r, vp.center);
        assert!((screen.x - r.center().x).abs() < 1e-6);
        assert!((screen.y - r.center().y).abs() < 1e-6);
    }

    #[test]
    fn y_axis_is_flipped_between_world_and_screen() {
        // ワールド y+ 方向（上）はスクリーンでは y が減る方向（上）に対応するはず。
        let vp = Viewport::new();
        let r = rect(400.0, 400.0);
        let up = vp.world_to_screen(r, Point2::new(0.0, 10.0));
        let down = vp.world_to_screen(r, Point2::new(0.0, -10.0));
        assert!(up.y < r.center().y);
        assert!(down.y > r.center().y);
    }

    #[test]
    fn zoom_at_cursor_keeps_cursor_world_position_fixed() {
        let mut vp = Viewport {
            zoom: 1.0,
            center: Point2::new(3.0, -2.0),
        };
        let r = rect(1000.0, 800.0);
        let cursor = Pos2::new(650.0, 200.0);

        let world_before = vp.screen_to_world(r, cursor);
        vp.zoom_at(r, cursor, 2.5);
        let world_after = vp.screen_to_world(r, cursor);

        assert!(
            (world_before.x - world_after.x).abs() < 1e-9,
            "cursor world x moved: {world_before:?} -> {world_after:?}"
        );
        assert!(
            (world_before.y - world_after.y).abs() < 1e-9,
            "cursor world y moved: {world_before:?} -> {world_after:?}"
        );
        assert!((vp.zoom - 2.5).abs() < 1e-9);
    }

    #[test]
    fn zoom_at_repeated_zoom_out_keeps_cursor_fixed() {
        let mut vp = Viewport::new();
        let r = rect(1200.0, 900.0);
        let cursor = Pos2::new(100.0, 850.0);

        let world_before = vp.screen_to_world(r, cursor);
        for _ in 0..5 {
            vp.zoom_at(r, cursor, 0.8);
        }
        let world_after = vp.screen_to_world(r, cursor);
        assert!((world_before.x - world_after.x).abs() < 1e-6);
        assert!((world_before.y - world_after.y).abs() < 1e-6);
    }

    #[test]
    fn zoom_at_ignores_non_positive_or_non_finite_factor() {
        let mut vp = Viewport::new();
        let r = rect(800.0, 600.0);
        let before = vp;
        vp.zoom_at(r, r.center(), 0.0);
        assert_eq!(vp, before);
        vp.zoom_at(r, r.center(), -1.0);
        assert_eq!(vp, before);
        vp.zoom_at(r, r.center(), f64::NAN);
        assert_eq!(vp, before);
    }

    #[test]
    fn zoom_clamped_to_bounds() {
        let mut vp = Viewport::new();
        let r = rect(800.0, 600.0);
        vp.zoom_at(r, r.center(), 1e-30);
        assert!(vp.zoom >= MIN_ZOOM);
        vp.zoom = 1.0;
        vp.zoom_at(r, r.center(), 1e30);
        assert!(vp.zoom <= MAX_ZOOM);
    }

    #[test]
    fn pan_by_screen_delta_moves_center_opposite_and_y_flipped() {
        let mut vp = Viewport {
            zoom: 2.0,
            center: Point2::ORIGIN,
        };
        // ドラッグで右・下に動かすと、center は左・上（ワールドで上）へ動く。
        vp.pan_by_screen_delta(egui::vec2(20.0, 10.0));
        assert!((vp.center.x - (-10.0)).abs() < 1e-9);
        assert!((vp.center.y - 5.0).abs() < 1e-9);
    }

    #[test]
    fn pan_keeps_grabbed_world_point_under_cursor() {
        // ドラッグ操作は「掴んだワールド点がカーソルに追従する」ことを保証するべき。
        let mut vp = Viewport {
            zoom: 1.7,
            center: Point2::new(2.0, 2.0),
        };
        let r = rect(640.0, 480.0);
        let cursor_start = Pos2::new(300.0, 200.0);
        let grabbed_world = vp.screen_to_world(r, cursor_start);

        let delta = egui::vec2(40.0, -15.0);
        let cursor_end = cursor_start + delta;
        vp.pan_by_screen_delta(delta);

        let world_at_cursor_end = vp.screen_to_world(r, cursor_end);
        assert!((grabbed_world.x - world_at_cursor_end.x).abs() < 1e-6);
        assert!((grabbed_world.y - world_at_cursor_end.y).abs() < 1e-6);
    }

    #[test]
    fn visible_aabb_matches_corners() {
        let vp = Viewport {
            zoom: 10.0,
            center: Point2::new(1.0, 1.0),
        };
        let r = rect(200.0, 100.0);
        let bb = vp.visible_aabb(r);
        // 幅 200px / zoom10 = 20 ワールド単位、高さ 100px/zoom10 = 10 ワールド単位。
        assert!((bb.width() - 20.0).abs() < 1e-6);
        assert!((bb.height() - 10.0).abs() < 1e-6);
        assert!((bb.center().x - 1.0).abs() < 1e-6);
        assert!((bb.center().y - 1.0).abs() < 1e-6);
    }

    #[test]
    fn nice_grid_step_picks_1_2_5_family() {
        // ズーム 1.0、目標 50px の時、raw = 50。5 の倍数系列で近いのは 50。
        let step = nice_grid_step(1.0, 50.0);
        assert!((step - 50.0).abs() < 1e-9);

        // ズームが大きくなる（拡大）と、同じ画面間隔に必要なワールド間隔は小さくなる。
        let step_zoomed_in = nice_grid_step(100.0, 50.0);
        assert!(step_zoomed_in < step);

        // ズームが小さくなる（縮小）と、必要なワールド間隔は大きくなる。
        let step_zoomed_out = nice_grid_step(0.01, 50.0);
        assert!(step_zoomed_out > step);
    }

    #[test]
    fn nice_grid_step_is_from_1_2_5_decade_family() {
        for zoom_exp in -6..6 {
            let zoom = 10f64.powi(zoom_exp);
            let step = nice_grid_step(zoom, 50.0);
            let exponent = step.log10().round();
            let base = 10f64.powf(exponent);
            let mantissa = step / base;
            let is_nice = [0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0]
                .iter()
                .any(|&m| (mantissa - m).abs() < 1e-6);
            assert!(
                is_nice,
                "step {step} (mantissa {mantissa}) not from 1/2/5 family"
            );
        }
    }
}
