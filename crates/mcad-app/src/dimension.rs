//! 寸法の**表示モード（紙基準表示トグル F9）の解決**。
//!
//! # 役割分担（DESIGN.md M11 設計判断1、タスク71）
//!
//! 寸法の展開（寸法線・補助線・矢先・文字配置をワールド座標へ組む純関数）は
//! **`mcad-core` の `expand` モジュール**にある（[`mcad_core::expand_linear`] /
//! [`mcad_core::expand_radial`] / [`mcad_core::expand_diameter`]、ヒットテスト用の
//! [`mcad_core::linear_pick_segments`] 等、値の組版 [`mcad_core::layout_dim_label`]）。
//! M10 まではこのモジュールに置いていたが、`mcad-io` から使えず DXF export が組版を
//! 再実装する二重化を招いたため core へ移した。
//!
//! ここに残るのは **core が知ってはいけない表示状態**だけである:
//!
//! - [`dim_sizes`][]: 紙基準表示トグル（F9）で矢先長・文字高さをワールド長へ解決する。
//!   ON は文書スタイルの紙 mm × `k`、OFF は画面固定 px ÷ ズーム。
//! - [`dim_render`][]: 解決したワールド長を [`mcad_core::DimRender`] へ詰める。
//!
//! 描画（`main.rs` の `draw_dim_expansion` / `draw_dim_label_box`）と pick の呼び出しも
//! `main.rs` / `tool.rs` に残る。**core へ渡すのはワールド長になった値だけ**で、
//! トグルもズームも core は知らない。

use mcad_core::{DimRender, DimStyle};

use crate::{DIM_ARROW_PX, DIM_TEXT_PX};

/// 寸法の矢先の長さ・文字高さをワールド長で解決する（戻り値: `(arrow_len, text_height)`）。
///
/// 紙基準表示 ON（`paper_display`）: 文書スタイルの紙 mm
/// （[`DimStyle::arrow_len_mm`]/[`DimStyle::text_height_mm`]）× `k`
/// （ズーム非依存 → 図形と一緒に拡縮し、タスク39/40 の SVG/PDF 出力と一致する）。
/// OFF: 画面固定 px（[`DIM_ARROW_PX`](crate::DIM_ARROW_PX)/[`DIM_TEXT_PX`](crate::DIM_TEXT_PX)）
/// ÷ `zoom`（タスク36b までの現行の見た目。スタイルの影響を受けない画面専用モード）。
/// ON モードの注記サイズに px 下限クランプは設けない（`draw_text` 既存の
/// [`MIN_TEXT_PX`](crate::MIN_TEXT_PX) 未満スキップに任せる）。
///
/// # 紙 mm の出所を [`DimStyle`] へ一本化してある（M9 タスク49-3）
///
/// タスク37〜39 はここで `plot::DIM_ARROW_MM` / `plot::DIM_TEXT_MM` という定数を使って
/// いたが、M9 タスク49-2 で矢の内外判定（core の `arrows_point_outward`）と注記の
/// 表示倍率（[`mcad_core::DimRender`] の `annotation_scale`）が [`DimStyle`] を読むように
/// なったため、「実際に描かれる大きさは定数・判定と組版の比率はスタイル」という
/// 二重の出所になっていた。既定値が一致していたので差は出ていなかったが、スタイル編集
/// UI（M9 タスク50）で文字高さを変えた瞬間に両者が食い違う。ここをスタイル読みへ
/// 揃えることで、画面・SVG・PDF の 3 経路が同じ 1 つの値から大きさを得る。
pub fn dim_sizes(style: &DimStyle, paper_display: bool, k: f64, zoom: f64) -> (f64, f64) {
    if paper_display {
        (style.arrow_len_mm * k, style.text_height_mm * k)
    } else {
        (DIM_ARROW_PX / zoom, DIM_TEXT_PX / zoom)
    }
}

/// 寸法展開のパラメータ（[`mcad_core::DimRender`]）を組み立てる。
///
/// 表示モードの解決（[`dim_sizes`]）はここで済ませ、core の展開関数へは
/// **ワールド長になった値だけ**を渡す。文書尺度 `k` は矢の内外判定を紙 mm で行うために
/// 別枠で渡す（[`mcad_core::DimRender`] の doc: 2 つの換算係数を混同しないこと）。
pub fn dim_render(style: &DimStyle, paper_display: bool, k: f64, zoom: f64) -> DimRender<'_> {
    let (arrow_len_world, text_height_world) = dim_sizes(style, paper_display, k, zoom);
    DimRender {
        style,
        scale_world_per_paper_mm: k,
        arrow_len_world,
        text_height_world,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcad_core::{DimAnnotation, DimExpansion, DimLinear, expand_linear};
    use mcad_geom::{Point2, Vec2};

    fn plain_linear(p1: Point2, p2: Point2, offset: f64) -> DimLinear {
        DimLinear {
            p1,
            p2,
            offset,
            annotation: DimAnnotation::default(),
        }
    }

    /// 矢が外向きか（先端から胴への向きが寸法線 `dir` の外側か）を展開結果から読む。
    /// 既定の [`mcad_geom::ArrowKind::ClosedFilled`]（塗りつぶし三角形 1 枚）を前提にする。
    fn arrows_are_outward(ex: &DimExpansion, dir: Vec2) -> bool {
        let tri = &ex.arrows[0].fills[0];
        (tri[1].midpoint(tri[2]) - tri[0]).dot(dir) < 0.0
    }

    /// **必須の不変条件**: 矢の内外判定は紙 mm だけで行い、表示状態（F9）・ズームで
    /// 変わらない。変わると画面と SVG/PDF 出力が食い違う（M9 検収基準）。
    ///
    /// 判定そのものは core の `arrows_point_outward` にあるが、このテストは
    /// **`dim_render` が詰めるワールド長を通しても判定が動かない**ことを見るので
    /// app 側に置く（core 単体では表示モードの分岐を通せない）。
    #[test]
    fn arrow_placement_decision_is_independent_of_zoom_and_paper_display() {
        let dir = Vec2::new(1.0, 0.0);
        let style = DimStyle::DEFAULT;
        let k = 1.0;
        let fits = plain_linear(Point2::new(0.0, 0.0), Point2::new(100.0, 0.0), 2.0);
        let does_not_fit = plain_linear(Point2::new(0.0, 0.0), Point2::new(5.0, 0.0), 2.0);

        for paper_display in [false, true] {
            for zoom in [0.05, 1.0, 37.0, 500.0] {
                let r = dim_render(&style, paper_display, k, zoom);
                assert!(
                    !arrows_are_outward(&expand_linear(&fits, r), dir),
                    "paper_display {paper_display} / zoom {zoom}"
                );
                assert!(
                    arrows_are_outward(&expand_linear(&does_not_fit, r), dir),
                    "paper_display {paper_display} / zoom {zoom}"
                );
            }
        }
    }
}
