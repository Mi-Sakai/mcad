//! 図面枠・表題欄の派生描画レイアウト（egui 非依存の純関数、DESIGN.md M8 設計判断3）。
//!
//! 図面枠・表題欄はエンティティではなく、[`SheetMeta`] からの派生描画である
//! （グリッドと同格）。編集は「欄の値を書き換える = `Command::SetSheet`」であり、
//! 図形として選択・移動はできない。**全ピック・スナップ経路は `Document::entities()`
//! のみを走査する**ため、本モジュールを追加しても選択・ピック・スナップの対象には
//! ならない（変更ゼロで対象外性が担保される）。
//!
//! # 座標系
//!
//! 紙 mm 座標（原点=用紙左下、y-up）でレイアウトを生成する。ワールド座標への変換は
//! [`paper_to_world`] が担い、用紙左下がワールド原点になる（DESIGN.md M8 設計判断2の
//! 「紙空間は持たない」を踏まえた配置）。**このモジュールは `egui` に依存しない**
//! （タスク39 の `plot` モジュールが [`FrameLayout`] を印刷対象の中間表現として
//! そのまま再利用できるようにするため）。

use mcad_core::{Orientation, PaperSize, ProjectionMethod, Scale, SheetMeta, TitleBlockField};
use mcad_geom::Point2;

/// 輪郭の余白（紙 mm、`製図規定.md` 2-3 の 1)。綴じ代なし・全周一律）。
pub const FRAME_MARGIN_MM: f64 = 10.0;

/// 輪郭線・表題欄外枠の線幅（紙 mm、`製図規定.md` 2-3 の 2) / 2-4-3 の a) 外枠）。
pub const FRAME_BORDER_WIDTH_MM: f32 = 0.5;

/// 表題欄内部の区切り線（行境界・セル境界）の線幅（紙 mm、`製図規定.md` 2-4-3 の a) 区切り線）。
pub const FRAME_DIVIDER_WIDTH_MM: f32 = 0.13;

/// セル左端から文字アンカーまでの詰め（紙 mm）。`製図規定.md` はセル内の文字の
/// 詰め量までは規定していないため、可読な余白として自前で定義した値。
pub const CELL_TEXT_PAD_MM: f64 = 1.5;

/// 印刷対象の枠の線分 1 本（紙 mm 座標、原点=用紙左下、y-up）。
pub struct FrameLine {
    /// 始点。
    pub a: Point2,
    /// 終点。
    pub b: Point2,
    /// 線幅（紙 mm）。
    pub width_mm: f32,
}

/// 印刷対象の欄文字 1 つ（紙 mm 座標）。
pub struct FrameText {
    /// アンカー（左詰め・近似垂直センタリングの左下基準点、紙 mm）。
    pub anchor_mm: Point2,
    /// 文字高さ（紙 mm）。
    pub height_mm: f64,
    /// 表示文字列（バインド解決済み）。
    pub content: String,
}

/// 印刷対象の枠幾何（紙 mm、原点=用紙左下、y-up）。タスク39 の `plot` はこれを
/// そのまま再利用する想定の中間表現。**用紙縁（用紙の外形矩形）は含まない**
/// （画面専用ヒントであり印刷対象ではないため。呼び出し側 `main.rs` の `draw_paper_edge`
/// を参照）。
// `paper_w_mm`/`paper_h_mm` は M8 時点では `main.rs` 側（`draw_paper_edge`）が
// `SheetMeta::paper_extent_mm()` を直接使うため未読だが、タスク39 の `plot` が
// `FrameLayout` を紙面全体の中間表現として再利用する際の唯一の用紙寸法の出所として
// 残す（フィールド自体は公開 API の一部でテストからも参照するため `dead_code` を許容）。
#[allow(dead_code)]
pub struct FrameLayout {
    /// 用紙幅（紙 mm、向き反映済み）。
    pub paper_w_mm: f64,
    /// 用紙高さ（紙 mm、向き反映済み）。
    pub paper_h_mm: f64,
    /// 輪郭4辺 + 表題欄外枠 + 表題欄内部区切り線。
    pub lines: Vec<FrameLine>,
    /// バインド解決済みの欄文字（空文字列に解決された欄・予備欄は含まない）。
    pub texts: Vec<FrameText>,
}

/// [`SheetMeta`] から枠全体のレイアウトを生成する（純関数、唯一の入口）。
///
/// `sheet.scale` には依存しない（尺度欄の文字列表記を除く）。紙 mm 座標のレイアウトは
/// 尺度不変であり、尺度はワールド変換（[`paper_to_world`]）でのみ効く。
#[must_use]
pub fn frame_layout(sheet: &SheetMeta) -> FrameLayout {
    let (paper_w_mm, paper_h_mm) = sheet.paper_extent_mm();
    let mut lines = Vec::new();

    // 輪郭4辺（余白 FRAME_MARGIN_MM、規定 2-3 の 1)/2)）。
    let outline_min = Point2::new(FRAME_MARGIN_MM, FRAME_MARGIN_MM);
    let outline_max = Point2::new(paper_w_mm - FRAME_MARGIN_MM, paper_h_mm - FRAME_MARGIN_MM);
    push_rect(&mut lines, outline_min, outline_max, FRAME_BORDER_WIDTH_MM);

    // 表題欄: 右下隅・輪郭線に接する（規定 2-4 の 1)）。
    let template = sheet.title_block.template();
    let block_left = paper_w_mm - FRAME_MARGIN_MM - template.width_mm;
    let block_bottom = FRAME_MARGIN_MM;
    let block_top = block_bottom + template.height_mm();
    let block_right = block_left + template.width_mm;

    // 外枠。輪郭と接する下辺・右辺が輪郭線と重複して描かれるのは許容する
    // （太い線が勝つだけ。単純さを優先し重複除去はしない）。
    push_rect(
        &mut lines,
        Point2::new(block_left, block_bottom),
        Point2::new(block_right, block_top),
        FRAME_BORDER_WIDTH_MM,
    );

    let mut texts = Vec::new();
    // `rows` の先頭が最上段（title_block.rs の座標系 doc に従う）。
    let mut row_top = block_top;
    for row in &template.rows {
        let row_bottom = row_top - row.height_mm;
        // 行境界の水平線（段の間のみ。最上段の上端・最下段の下端は外枠と重複するため
        // 描かない）。
        if row_bottom > block_bottom {
            lines.push(FrameLine {
                a: Point2::new(block_left, row_bottom),
                b: Point2::new(block_right, row_bottom),
                width_mm: FRAME_DIVIDER_WIDTH_MM,
            });
        }

        let mut cell_left = block_left;
        for cell in &row.cells {
            let cell_right = cell_left + cell.width_mm;
            // セル境界の垂直線（セルの間のみ。行の左端・右端は外枠と重複するため描かない）。
            if cell_right < block_right {
                lines.push(FrameLine {
                    a: Point2::new(cell_right, row_bottom),
                    b: Point2::new(cell_right, row_top),
                    width_mm: FRAME_DIVIDER_WIDTH_MM,
                });
            }
            if let Some(field) = cell.bind {
                let content = bind_text(field, sheet);
                if !content.is_empty() {
                    let anchor_mm = Point2::new(
                        cell_left + CELL_TEXT_PAD_MM,
                        row_bottom + (row.height_mm - cell.text_height_mm) / 2.0,
                    );
                    texts.push(FrameText {
                        anchor_mm,
                        height_mm: cell.text_height_mm,
                        content,
                    });
                }
            }
            cell_left = cell_right;
        }
        row_top = row_bottom;
    }

    FrameLayout {
        paper_w_mm,
        paper_h_mm,
        lines,
        texts,
    }
}

/// 矩形の4辺を `lines` へ追加するヘルパー（輪郭・表題欄外枠で共用）。
fn push_rect(lines: &mut Vec<FrameLine>, min: Point2, max: Point2, width_mm: f32) {
    let p0 = Point2::new(min.x, min.y);
    let p1 = Point2::new(max.x, min.y);
    let p2 = Point2::new(max.x, max.y);
    let p3 = Point2::new(min.x, max.y);
    for (a, b) in [(p0, p1), (p1, p2), (p2, p3), (p3, p0)] {
        lines.push(FrameLine { a, b, width_mm });
    }
}

/// 欄バインドの文字列解決（純関数、単体テスト対象）。
///
/// `Scale`/`PaperSize` は [`mcad_core::TitleBlockFields`] ではなく `sheet` 本体から
/// 導出する（二重に保持しない。DESIGN.md M8 設計判断2）。
fn bind_text(field: TitleBlockField, sheet: &SheetMeta) -> String {
    match field {
        TitleBlockField::DrawingTitle => sheet.fields.drawing_title.clone(),
        TitleBlockField::DrawingNumber => sheet.fields.drawing_number.clone(),
        TitleBlockField::Author => sheet.fields.author.clone(),
        TitleBlockField::Date => sheet.fields.date.clone(),
        TitleBlockField::Revision => sheet.fields.revision.clone(),
        // 既約化しない（sheet.scale は Scale::new が生成した値をそのまま表示。判断2）。
        TitleBlockField::Scale => format!("{}:{}", sheet.scale.num(), sheet.scale.den()),
        TitleBlockField::Projection => projection_label(sheet.fields.projection).to_owned(),
        TitleBlockField::PaperSize => paper_size_label(sheet.paper, sheet.orientation),
    }
}

/// 投影法の図面内容としての文字（UI ラベルではなく、SVG/PDF にもこのまま出力される。
/// ユーザー確定）。
fn projection_label(projection: ProjectionMethod) -> &'static str {
    match projection {
        ProjectionMethod::ThirdAngle => "第三角法",
        ProjectionMethod::FirstAngle => "第一角法",
    }
}

/// 用紙サイズ欄の文字（横は "A4" 等、縦(Portrait) は "A4 縦"。ユーザー確定）。
fn paper_size_label(paper: PaperSize, orientation: Orientation) -> String {
    let base = match paper {
        PaperSize::A0 => "A0",
        PaperSize::A1 => "A1",
        PaperSize::A2 => "A2",
        PaperSize::A3 => "A3",
        PaperSize::A4 => "A4",
    };
    match orientation {
        Orientation::Landscape => base.to_owned(),
        Orientation::Portrait => format!("{base} 縦"),
    }
}

/// 紙 mm 座標（原点=用紙左下、y-up）→ワールド座標。用紙左下はワールド原点
/// （DESIGN.md M8 設計判断2「紙空間は持たない」）。`k` は
/// [`mcad_core::Scale::world_mm_per_paper_mm`]。
#[must_use]
pub fn paper_to_world(p_mm: Point2, k: f64) -> Point2 {
    Point2::new(p_mm.x * k, p_mm.y * k)
}

/// 尺度入力文字列のパース（UI 直接入力境界での拒否）。
///
/// トリム→`':'` 分割→`u32` パース→[`Scale::new`] の順に検証する。境界の妥当性
/// （`num > 0 && den > 0`・上限）は `Scale::new` に委ねる。
///
/// # Errors
///
/// 形式不正（`':'` が無い/複数ある）・非数値・`Scale::new` が拒否する値
/// （0・負値相当・上限超過）を人が読めるメッセージで返す。
pub fn parse_scale_input(s: &str) -> Result<Scale, String> {
    let trimmed = s.trim();
    let parts: Vec<&str> = trimmed.split(':').collect();
    let [num_str, den_str] = parts.as_slice() else {
        return Err(format!(
            "尺度は \"分子:分母\" の形式で入力してください（例 \"1:2\"）: \"{trimmed}\""
        ));
    };
    let num: u32 = num_str
        .trim()
        .parse()
        .map_err(|_| format!("尺度の分子が正しい整数ではありません: \"{num_str}\""))?;
    let den: u32 = den_str
        .trim()
        .parse()
        .map_err(|_| format!("尺度の分母が正しい整数ではありません: \"{den_str}\""))?;
    Scale::new(num, den).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcad_core::{TitleBlockFields, TitleBlockKind};

    fn approx(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    #[test]
    fn a4_landscape_outline_is_10mm_inset_with_half_mm_border() {
        let sheet = SheetMeta::default();
        let layout = frame_layout(&sheet);
        approx(layout.paper_w_mm, 297.0);
        approx(layout.paper_h_mm, 210.0);

        let outline: Vec<&FrameLine> = layout
            .lines
            .iter()
            .filter(|l| (l.width_mm - FRAME_BORDER_WIDTH_MM).abs() < f32::EPSILON)
            .collect();
        // 輪郭4辺 + 表題欄外枠4辺 = 8本。
        assert_eq!(outline.len(), 8);
        let corners: Vec<(f64, f64)> = [
            Point2::new(10.0, 10.0),
            Point2::new(287.0, 10.0),
            Point2::new(287.0, 200.0),
            Point2::new(10.0, 200.0),
        ]
        .iter()
        .map(|p| (p.x, p.y))
        .collect();
        for corner in corners {
            let hit = outline
                .iter()
                .any(|l| points_match(l.a, corner) || points_match(l.b, corner));
            assert!(hit, "missing outline corner {corner:?}");
        }
    }

    fn points_match(p: Point2, target: (f64, f64)) -> bool {
        (p.x - target.0).abs() < 1e-9 && (p.y - target.1).abs() < 1e-9
    }

    #[test]
    fn standard_b_title_block_is_bottom_right_aligned() {
        let sheet = SheetMeta::default(); // 様式B、A4横。
        let layout = frame_layout(&sheet);
        // ブロック左下 = (paper_w - 10 - 120, 10) = (167, 10)。
        let block_corner_hits = layout.lines.iter().any(|l| {
            (points_match(l.a, (167.0, 10.0)) || points_match(l.b, (167.0, 10.0)))
                && (l.width_mm - FRAME_BORDER_WIDTH_MM).abs() < f32::EPSILON
        });
        assert!(block_corner_hits, "expected block bottom-left corner");

        let dividers: Vec<&FrameLine> = layout
            .lines
            .iter()
            .filter(|l| (l.width_mm - FRAME_DIVIDER_WIDTH_MM).abs() < f32::EPSILON)
            .collect();
        // 行境界(1) + row0のセル境界(1) + row1のセル境界(5) = 7本。
        assert_eq!(dividers.len(), 7);

        // 罫線本数の合計: 輪郭4 + 外枠4 + 区切り7 = 15本。
        assert_eq!(layout.lines.len(), 15);
    }

    #[test]
    fn standard_a_and_c_line_counts_are_fixed() {
        let sheet_a = SheetMeta {
            title_block: TitleBlockKind::A,
            ..SheetMeta::default()
        };
        let layout_a = frame_layout(&sheet_a);
        // A: 2段(2,6セル)。行境界1 + セル境界(1+5)=6 → 区切り7本。輪郭4+外枠4+7=15。
        assert_eq!(layout_a.lines.len(), 15);

        let sheet_c = SheetMeta {
            title_block: TitleBlockKind::C,
            ..SheetMeta::default()
        };
        let layout_c = frame_layout(&sheet_c);
        // C: 3段(1,4,5セル)。行境界2 + セル境界(0+3+4)=7 → 区切り9本。輪郭4+外枠4+9=17。
        assert_eq!(layout_c.lines.len(), 17);

        // 様式Cの予備欄2つ（bind=None）は texts に出ない。既定 fields（空文字列）では
        // 残る文字欄のうち Scale/Projection/PaperSize のみ非空文字列に解決されるため、
        // texts は3件になる。
        assert_eq!(layout_c.texts.len(), 3);
        assert!(layout_c.texts.iter().any(|t| t.content == "1:1"));
        assert!(layout_c.texts.iter().any(|t| t.content == "第三角法"));
        assert!(layout_c.texts.iter().any(|t| t.content == "A4"));
    }

    fn full_fields() -> TitleBlockFields {
        TitleBlockFields {
            drawing_number: "MCAD-001".to_owned(),
            drawing_title: "部品図".to_owned(),
            projection: ProjectionMethod::ThirdAngle,
            author: "almaz".to_owned(),
            date: "2026-08-09".to_owned(),
            revision: "A".to_owned(),
        }
    }

    #[test]
    fn bind_resolution_covers_all_fields_and_skips_empty_and_spare_cells() {
        let sheet = SheetMeta {
            fields: full_fields(),
            ..SheetMeta::default()
        };
        let layout = frame_layout(&sheet);
        let content = |needle: &str| layout.texts.iter().any(|t| t.content == needle);
        assert!(content("MCAD-001"));
        assert!(content("部品図"));
        assert!(content("1:1"));
        assert!(content("A4"));
        assert!(content("第三角法"));
        assert!(content("almaz"));
        assert!(content("2026-08-09"));
        assert!(content("A"));

        // 第一角法へ切替。
        let mut sheet_first_angle = sheet.clone();
        sheet_first_angle.fields.projection = ProjectionMethod::FirstAngle;
        let layout_first_angle = frame_layout(&sheet_first_angle);
        assert!(
            layout_first_angle
                .texts
                .iter()
                .any(|t| t.content == "第一角法")
        );

        // 既定（空文字列の fields）では該当欄が texts から除外される。
        let default_sheet = SheetMeta::default();
        let default_layout = frame_layout(&default_sheet);
        assert!(!default_layout.texts.iter().any(|t| t.content.is_empty()));
        // 尺度・用紙サイズ・投影法は既定でも常に非空文字列として出る。
        assert!(default_layout.texts.iter().any(|t| t.content == "1:1"));
        assert!(default_layout.texts.iter().any(|t| t.content == "A4"));
        assert!(default_layout.texts.iter().any(|t| t.content == "第三角法"));

        // 様式Cの予備欄(bind=None)は texts の件数から見て現れない
        // （full_fieldsでも様式Bの欄数と同じ8件のみ）。
        assert_eq!(layout.texts.len(), 8);
    }

    #[test]
    fn portrait_extent_swaps_and_paper_size_label_gets_suffix() {
        let sheet = SheetMeta {
            orientation: Orientation::Portrait,
            ..SheetMeta::default()
        };
        let layout = frame_layout(&sheet);
        approx(layout.paper_w_mm, 210.0);
        approx(layout.paper_h_mm, 297.0);
        assert!(layout.texts.iter().any(|t| t.content == "A4 縦"));

        // 表題欄は右下へ追従: ブロック左下 x = 210 - 10 - 120 = 80。
        let hit = layout.lines.iter().any(|l| {
            (points_match(l.a, (80.0, 10.0)) || points_match(l.b, (80.0, 10.0)))
                && (l.width_mm - FRAME_BORDER_WIDTH_MM).abs() < f32::EPSILON
        });
        assert!(hit, "expected portrait block bottom-left corner at x=80");
    }

    #[test]
    fn frame_layout_does_not_depend_on_scale() {
        let mut sheet = SheetMeta {
            fields: full_fields(),
            ..SheetMeta::default()
        };
        let layout_1_1 = frame_layout(&sheet);

        sheet.scale = Scale::new(1, 2).unwrap();
        let layout_1_2 = frame_layout(&sheet);

        assert_eq!(layout_1_1.paper_w_mm, layout_1_2.paper_w_mm);
        assert_eq!(layout_1_1.paper_h_mm, layout_1_2.paper_h_mm);
        assert_eq!(layout_1_1.lines.len(), layout_1_2.lines.len());
        for (a, b) in layout_1_1.lines.iter().zip(layout_1_2.lines.iter()) {
            assert_eq!(a.a, b.a);
            assert_eq!(a.b, b.b);
            assert_eq!(a.width_mm, b.width_mm);
        }
        // 尺度欄の文字列以外は座標・高さとも一致する。
        for (a, b) in layout_1_1.texts.iter().zip(layout_1_2.texts.iter()) {
            assert_eq!(a.anchor_mm, b.anchor_mm);
            assert_eq!(a.height_mm, b.height_mm);
        }
        assert!(layout_1_1.texts.iter().any(|t| t.content == "1:1"));
        assert!(layout_1_2.texts.iter().any(|t| t.content == "1:2"));
    }

    #[test]
    fn paper_to_world_scales_by_k() {
        let p = Point2::new(10.0, 10.0);
        approx(paper_to_world(p, 2.0).x, 20.0);
        approx(paper_to_world(p, 2.0).y, 20.0);
        approx(paper_to_world(p, 0.5).x, 5.0);
        approx(paper_to_world(p, 0.5).y, 5.0);
    }

    #[test]
    fn parse_scale_input_accepts_valid_forms() {
        assert_eq!(parse_scale_input("1:2").unwrap(), Scale::new(1, 2).unwrap());
        assert_eq!(parse_scale_input("2:5").unwrap(), Scale::new(2, 5).unwrap());
        assert_eq!(
            parse_scale_input("  1 : 2  ").unwrap(),
            Scale::new(1, 2).unwrap()
        );
    }

    #[test]
    fn parse_scale_input_rejects_malformed_and_invalid_values() {
        for bad in [
            "", "1", "1:2:3", "0:1", "1:0", "abc:1", "1:abc", "-1:2", "1:-2",
        ] {
            assert!(
                parse_scale_input(bad).is_err(),
                "\"{bad}\" should be rejected"
            );
        }
        let over_limit = format!("{}:1", mcad_core::MAX_SCALE_TERM + 1);
        assert!(parse_scale_input(&over_limit).is_err());
    }
}
