//! 部品表プリセット（[`TableGeom`] の特化版。DESIGN.md M10 詳細設計7、タスク60）。
//!
//! `製図規定.md` 2-5 で確定した部品欄の列構成・寸法（タスク55 ユーザー承認）を
//! [`TableGeom`] へ適用する純関数群。**自動集計は実装しない**（M10 設計方針2・
//! 規定 2-5-4 の 5)）— ここにあるのは列構成の置換と表題欄直上への配置ヘルパーだけで、
//! エンティティを数えて行を生成する処理は持たない。
//!
//! # 列構成とヘッダ行の向き
//!
//! [`TableGeom::cells`] は行 0 が最上段（`crate::table` の doc 参照）。部品欄は
//! 「表題欄側（最下段）がヘッダ行、部品行は上へ積む」という規定 2-5-1 の 3) の向きなので、
//! [`apply_bom_preset`] は**最下段**（`rows() - 1`）を列名で上書きする。

use mcad_core::{SheetMeta, TableGeom};
use mcad_geom::Point2;

use crate::frame::{FRAME_MARGIN_MM, paper_to_world};

/// 部品欄の列名（左から右へ。規定 2-5-2）。列数を変えるのは non-goal
/// （規定 2-5-5 は「基本5列を保持し右側に追加してよい」だが、これはユーザー定義様式の
/// 拡張であって本プリセットの対象外 — 追加列の内容は手動編集に委ねる）。
const BOM_COLUMN_NAMES: [&str; 5] = ["品番", "名称", "個数", "材質", "備考"];

/// 様式A/C（幅170mm）の列幅（紙 mm、規定 2-5-3）。
const BOM_WIDTHS_AC_170: [f64; 5] = [10.0, 50.0, 10.0, 30.0, 70.0];

/// 様式B（幅120mm）の列幅（紙 mm、規定 2-5-3）。
const BOM_WIDTHS_B_120: [f64; 5] = [10.0, 42.0, 10.0, 30.0, 28.0];

/// 部品欄の行高さ（紙 mm、規定 2-5-3）。
pub const BOM_ROW_HEIGHT_MM: f64 = 8.0;

/// 部品欄の文字高さ（紙 mm、規定 2-5-3。ヘッダ行を含め全セル共通）。
pub const BOM_TEXT_HEIGHT_MM: f64 = 3.5;

/// `template_width_mm` に対応する部品欄の列名・列幅を返す。
///
/// 170mm（様式A/C）・120mm（様式B）はそれぞれの確定値をそのまま返す。それ以外の幅
/// （ユーザー定義様式）は様式A/C の列幅比を按分し、丸め誤差の蓄積を最後の列
/// （備考）で吸収して合計が `template_width_mm` に厳密に一致するようにする。
#[must_use]
pub fn bom_preset_columns(template_width_mm: f64) -> (&'static [&'static str; 5], Vec<f64>) {
    const AC_TOTAL: f64 = 170.0;

    let widths = if (template_width_mm - AC_TOTAL).abs() < 1e-9 {
        BOM_WIDTHS_AC_170.to_vec()
    } else if (template_width_mm - 120.0).abs() < 1e-9 {
        BOM_WIDTHS_B_120.to_vec()
    } else {
        let ratio = template_width_mm / AC_TOTAL;
        let mut widths: Vec<f64> = BOM_WIDTHS_AC_170.iter().map(|w| w * ratio).collect();
        let last = widths.len() - 1;
        let head_sum: f64 = widths[..last].iter().sum();
        widths[last] = template_width_mm - head_sum;
        widths
    };

    (&BOM_COLUMN_NAMES, widths)
}

/// `table` を部品表プリセット（規定 2-5 の列構成・寸法・ヘッダ行）へ置換した新しい
/// [`TableGeom`] を返す。
///
/// - 列構成: [`bom_preset_columns`] の結果（`template_width_mm` は
///   [`mcad_core::SheetMeta::title_block`] の様式幅）に置換する。既存セルは列数が
///   増減する場合、行ごとに切り詰め/空文字列で補完する。
/// - ヘッダ行: **最下段**を列名で上書きする（表の行数が1のときはヘッダのみの表になる。
///   部品行はユーザーが「行を上に追加」で足す）。
/// - 行高さ・文字高さ: 全行 [`BOM_ROW_HEIGHT_MM`] / [`BOM_TEXT_HEIGHT_MM`] に統一する。
/// - `anchor`: 変更しない（配置は [`bom_anchor_above_title_block`] が別途受け持つ）。
#[must_use]
pub fn apply_bom_preset(table: &TableGeom, template_width_mm: f64) -> TableGeom {
    let (names, widths) = bom_preset_columns(template_width_mm);
    let new_cols = names.len();
    // 行が0（未検証の作業コピー）でもヘッダ1行は必ず持つ。
    let new_rows = table.rows().max(1);

    let mut cells = Vec::with_capacity(new_rows * new_cols);
    for r in 0..new_rows {
        let is_header = r == new_rows - 1;
        for (c, name) in names.iter().enumerate() {
            let cell = if is_header {
                (*name).to_owned()
            } else {
                table.cell(r, c).unwrap_or("").to_owned()
            };
            cells.push(cell);
        }
    }

    TableGeom {
        anchor: table.anchor,
        col_widths_mm: widths,
        row_heights_mm: vec![BOM_ROW_HEIGHT_MM; new_rows],
        text_height_mm: BOM_TEXT_HEIGHT_MM,
        cells,
    }
}

/// 表題欄の直上・左端を揃えるアンカー（ワールド座標）。
///
/// `frame::frame_layout` が表題欄外枠を組む式（`block_left = paper_w − FRAME_MARGIN_MM −
/// template.width_mm`、`block_top = FRAME_MARGIN_MM + template.height_mm()`）と同じ式で
/// 紙 mm の左上を求め、`paper_to_world` でワールドへ写す。幅は表題欄と一致させて呼び出す
/// 前提（[`bom_preset_columns`] の幅を使う）なので右端も揃う。
#[must_use]
pub fn bom_anchor_above_title_block(sheet: &SheetMeta, k: f64) -> Point2 {
    let (paper_w_mm, _paper_h_mm) = sheet.paper_extent_mm();
    let template = sheet.title_block.template();
    let block_left = paper_w_mm - FRAME_MARGIN_MM - template.width_mm;
    let block_top = FRAME_MARGIN_MM + template.height_mm();
    paper_to_world(Point2::new(block_left, block_top), k)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::frame_layout;
    use mcad_core::{Scale, TitleBlockKind};

    fn approx(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    fn sample_table(rows: usize, cols: usize) -> TableGeom {
        TableGeom {
            anchor: Point2::new(1.0, 2.0),
            col_widths_mm: vec![10.0; cols],
            row_heights_mm: vec![8.0; rows],
            text_height_mm: 3.5,
            cells: (0..rows * cols).map(|i| format!("v{i}")).collect(),
        }
    }

    // --- bom_preset_columns ---

    #[test]
    fn preset_columns_sum_matches_170() {
        let (names, widths) = bom_preset_columns(170.0);
        assert_eq!(names.len(), 5);
        assert_eq!(widths, vec![10.0, 50.0, 10.0, 30.0, 70.0]);
        approx(widths.iter().sum(), 170.0);
    }

    #[test]
    fn preset_columns_sum_matches_120() {
        let (_names, widths) = bom_preset_columns(120.0);
        assert_eq!(widths, vec![10.0, 42.0, 10.0, 30.0, 28.0]);
        approx(widths.iter().sum(), 120.0);
    }

    #[test]
    fn preset_columns_sum_matches_arbitrary_width_150() {
        let (_names, widths) = bom_preset_columns(150.0);
        let sum: f64 = widths.iter().sum();
        approx(sum, 150.0);
        // 按分された列幅であり、170mm 版の単純な複製ではない。
        assert_ne!(widths, vec![15.0, 65.0, 15.0, 35.0, 40.0]);
    }

    // --- apply_bom_preset ---

    #[test]
    fn apply_bom_preset_puts_header_row_at_the_bottom() {
        let table = sample_table(3, 2);
        let bom = apply_bom_preset(&table, 170.0);
        assert_eq!(bom.rows(), 3);
        assert_eq!(bom.cols(), 5);
        for (c, name) in BOM_COLUMN_NAMES.iter().enumerate() {
            assert_eq!(bom.cell(2, c), Some(*name));
        }
        // 最上段（ヘッダではない）はヘッダ名で上書きされない。
        assert_ne!(bom.cell(0, 0), Some("品番"));
    }

    #[test]
    fn apply_bom_preset_keeps_existing_part_rows_when_columns_grow() {
        let mut table = sample_table(2, 2);
        table.cells = vec![
            "A".to_owned(),
            "B".to_owned(),
            "ignored-header".to_owned(),
            "ignored-header2".to_owned(),
        ];
        let bom = apply_bom_preset(&table, 170.0);
        // 行0（部品行）は旧セルが先頭2列に保持され、増えた列は空欄補完。
        assert_eq!(bom.cell(0, 0), Some("A"));
        assert_eq!(bom.cell(0, 1), Some("B"));
        assert_eq!(bom.cell(0, 2), Some(""));
        assert_eq!(bom.cell(0, 4), Some(""));
    }

    #[test]
    fn apply_bom_preset_truncates_existing_cells_when_columns_shrink() {
        // 6列の表を部品欄（5列）に変換 → 各行の6列目は捨てる。
        let mut table = sample_table(2, 6);
        table.cells[5] = "drop-me".to_owned();
        let bom = apply_bom_preset(&table, 170.0);
        assert_eq!(bom.cols(), 5);
        assert_eq!(bom.cell(0, 0), Some("v0"));
        assert_eq!(bom.cell(0, 4), Some("v4"));
    }

    #[test]
    fn apply_bom_preset_single_row_table_is_header_only() {
        let table = sample_table(1, 3);
        let bom = apply_bom_preset(&table, 120.0);
        assert_eq!(bom.rows(), 1);
        for (c, name) in BOM_COLUMN_NAMES.iter().enumerate() {
            assert_eq!(bom.cell(0, c), Some(*name));
        }
    }

    #[test]
    fn apply_bom_preset_uses_bom_row_height_and_text_height() {
        let table = sample_table(4, 3);
        let bom = apply_bom_preset(&table, 170.0);
        assert!(bom.row_heights_mm.iter().all(|h| *h == BOM_ROW_HEIGHT_MM));
        assert_eq!(bom.text_height_mm, BOM_TEXT_HEIGHT_MM);
    }

    #[test]
    fn apply_bom_preset_result_passes_validate() {
        use mcad_core::EntityGeom;
        let table = sample_table(3, 2);
        let bom = apply_bom_preset(&table, 170.0);
        EntityGeom::Table(bom)
            .validate()
            .expect("bom preset table is valid");
    }

    #[test]
    fn apply_bom_preset_keeps_anchor_unchanged() {
        let table = sample_table(2, 2);
        let bom = apply_bom_preset(&table, 170.0);
        assert_eq!(bom.anchor, table.anchor);
    }

    // --- bom_anchor_above_title_block ---

    fn points_match(p: Point2, target: (f64, f64)) -> bool {
        (p.x - target.0).abs() < 1e-9 && (p.y - target.1).abs() < 1e-9
    }

    #[test]
    fn anchor_matches_title_block_top_left_for_standard_a_at_scale_1_2() {
        let sheet = SheetMeta {
            title_block: TitleBlockKind::A,
            scale: Scale::new(1, 2).expect("valid scale"),
            ..SheetMeta::default()
        };
        let k = sheet.scale.world_mm_per_paper_mm();
        approx(k, 2.0);

        let layout = frame_layout(&sheet);
        // A4横297x210、様式A（幅170・高さ30）: block_left=117, block_top=40。
        let expected_left_mm = 117.0;
        let expected_top_mm = 40.0;
        // frame_layout が実際にこの角を表題欄外枠の線分として持つことを確認
        // （独自計算の重複が frame.rs の実ジオメトリとずれていないことの裏取り）。
        let hit = layout.lines.iter().any(|l| {
            points_match(l.a, (expected_left_mm, expected_top_mm))
                || points_match(l.b, (expected_left_mm, expected_top_mm))
        });
        assert!(
            hit,
            "expected title block top-left corner not found in frame_layout"
        );

        let expected_world = paper_to_world(Point2::new(expected_left_mm, expected_top_mm), k);
        assert_eq!(bom_anchor_above_title_block(&sheet, k), expected_world);
    }

    #[test]
    fn anchor_matches_title_block_top_left_for_standard_b_at_scale_1_2() {
        let sheet = SheetMeta {
            title_block: TitleBlockKind::B,
            scale: Scale::new(1, 2).expect("valid scale"),
            ..SheetMeta::default()
        };
        let k = sheet.scale.world_mm_per_paper_mm();

        let layout = frame_layout(&sheet);
        // A4横297x210、様式B（幅120・高さ25）: block_left=167, block_top=35。
        let expected_left_mm = 167.0;
        let expected_top_mm = 35.0;
        let hit = layout.lines.iter().any(|l| {
            points_match(l.a, (expected_left_mm, expected_top_mm))
                || points_match(l.b, (expected_left_mm, expected_top_mm))
        });
        assert!(
            hit,
            "expected title block top-left corner not found in frame_layout"
        );

        let expected_world = paper_to_world(Point2::new(expected_left_mm, expected_top_mm), k);
        assert_eq!(bom_anchor_above_title_block(&sheet, k), expected_world);
    }
}
