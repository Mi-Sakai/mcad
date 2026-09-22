//! 部品表プリセット（[`TableGeom`] の特化版。DESIGN.md M10 詳細設計7、タスク60）。
//!
//! `製図規定.md` 2-5 で確定した部品欄の列構成・寸法（タスク55 ユーザー承認、
//! 2026-09-22 に部番・質量[kg] を追加する形で改訂）を [`TableGeom`] へ適用する
//! 純関数群。**自動集計は実装しない**（M10 設計方針2・規定 2-5-4 の 5)）—
//! ここにあるのは列構成の置換と表題欄直上への配置ヘルパーだけで、エンティティを
//! 数えて行を生成する処理は持たない。
//!
//! # 列構成とヘッダ行の向き
//!
//! [`TableGeom::cells`] は行 0 が最上段（`crate::table` の doc 参照）。部品欄は
//! 「表題欄側（最下段）がヘッダ行、部品行は上へ積む」という規定 2-5-1 の 3) の向きなので、
//! [`apply_bom_preset`] は**最下段**（`rows() - 1`）を列名で上書きする。
//!
//! # 既存の表への適用（列の意味による対応づけ）
//!
//! 列数が様式によって変わる（様式A/Cは7列、様式Bは備考なしの6列）ため、
//! [`apply_bom_preset`] は列番号ではなく**列の意味**でセルを対応づける。3通りに分岐する（2026-09-22 設計確定）:
//!
//! 1. **旧5列の部品表**（v0.11.0 以前のプリセット。見出しが旧5列の並びと完全一致）:
//!    旧「品番」（照合番号だった）は新「部番」へ、名称・個数・材質・備考は同じ意味の列へ移す。
//!    新しい「品番」（品目番号）と「質量[kg]」は空欄になる。
//! 2. **現行の部品表**（見出しが様式A/C の7列・様式B の6列の並びと完全一致）: 同じ名前の
//!    列どうしで移す。対応する列が無い新しい列は空欄になる。
//! 3. **それ以外の表**: 値が入っていれば変換しない（見出しの一致しない表を列番号で移すと、
//!    書き換えた旧部品表で列の意味がずれるため。2026-09-23 ユーザー確定）。空の表
//!    （`K` で置いた直後など）は列の形に関係なく変換できる。
//!
//! どの場合も、**新しい列構成へ移らない列に空でないセルがあれば変換しない**（`Err` で
//! 列名を返し、呼び出し側がステータスバーへ出す）。様式A/C から様式B へ切り替えるとき
//! 備考に値があれば拒否される。見分けを推測しない理由は [`classify_bom_columns`]。

use mcad_core::{SheetMeta, TableGeom};
use mcad_geom::Point2;

use crate::frame::{FRAME_MARGIN_MM, paper_to_world};

/// 部品欄の列名・様式A/C（幅170mm、7列。左から右へ。規定 2-5-2）。
pub const BOM_COLUMN_NAMES_AC: [&str; 7] =
    ["部番", "品番", "名称", "個数", "材質", "質量[kg]", "備考"];

/// 部品欄の列名・様式B（幅120mm、6列。備考なし。規定 2-5-2）。
pub const BOM_COLUMN_NAMES_B: [&str; 6] = ["部番", "品番", "名称", "個数", "材質", "質量[kg]"];

/// v0.11.0 以前の旧プリセットの列名（5列）。[`apply_bom_preset`] が既存の部品表を
/// 見分けるために使う（規定 2-5-2 改訂前の並び）。
const BOM_LEGACY_COLUMN_NAMES: [&str; 5] = ["品番", "名称", "個数", "材質", "備考"];

/// 旧プリセットの各列が新しい列構成のどの列にあたるか（位置 → 新しい列名）。
/// 旧「品番」は照合番号だったので新「部番」にあたる。
const BOM_LEGACY_COLUMN_MEANINGS: [&str; 5] = ["部番", "名称", "個数", "材質", "備考"];

/// 様式A/C（幅170mm）の列幅（紙 mm、規定 2-5-3）。
const BOM_WIDTHS_AC_170: [f64; 7] = [10.0, 35.0, 35.0, 10.0, 30.0, 20.0, 30.0];

/// 様式B（幅120mm）の列幅（紙 mm、規定 2-5-3）。
const BOM_WIDTHS_B_120: [f64; 6] = [10.0, 25.0, 25.0, 10.0, 30.0, 20.0];

/// 部品欄の行高さ（紙 mm、規定 2-5-3）。
pub const BOM_ROW_HEIGHT_MM: f64 = 8.0;

/// 部品欄の文字高さ（紙 mm、規定 2-5-3。ヘッダ行を含め全セル共通）。
pub const BOM_TEXT_HEIGHT_MM: f64 = 3.5;

/// `template_width_mm` に対応する部品欄の列名・列幅を返す。
///
/// 170mm（様式A/C、7列）・120mm（様式B、6列・備考なし）はそれぞれの確定値をそのまま
/// 返す。それ以外の幅（ユーザー定義様式）は様式A/C の7列の列幅比を按分し、丸め誤差の
/// 蓄積を最後の列（備考）で吸収して合計が `template_width_mm` に厳密に一致するように
/// する（規定 2-5-3・2026-09-22 改訂）。列数が様式で変わるため、戻り値は固定長配列
/// ではなく静的スライス。
#[must_use]
pub fn bom_preset_columns(template_width_mm: f64) -> (&'static [&'static str], Vec<f64>) {
    const AC_TOTAL: f64 = 170.0;

    if (template_width_mm - AC_TOTAL).abs() < 1e-9 {
        (&BOM_COLUMN_NAMES_AC, BOM_WIDTHS_AC_170.to_vec())
    } else if (template_width_mm - 120.0).abs() < 1e-9 {
        (&BOM_COLUMN_NAMES_B, BOM_WIDTHS_B_120.to_vec())
    } else {
        let ratio = template_width_mm / AC_TOTAL;
        let mut widths: Vec<f64> = BOM_WIDTHS_AC_170.iter().map(|w| w * ratio).collect();
        let last = widths.len() - 1;
        let head_sum: f64 = widths[..last].iter().sum();
        widths[last] = template_width_mm - head_sum;
        (&BOM_COLUMN_NAMES_AC, widths)
    }
}

/// `table` を部品表プリセット（規定 2-5 の列構成・寸法・ヘッダ行）へ置換した新しい
/// [`TableGeom`] を返す。
///
/// - 列構成: [`bom_preset_columns`] の結果（`template_width_mm` は
///   [`mcad_core::SheetMeta::title_block`] の様式幅）に置換する。
/// - 既存セルの対応づけ: モジュール doc「既存の表への適用」の3分岐（旧5列プリセット／
///   現行プリセットの表／それ以外の表）に従い、名前または列番号でセルを移す。
/// - ヘッダ行: **最下段**を列名で上書きする（表の行数が1のときはヘッダのみの表になる。
///   部品行はユーザーが「行を上に追加」で足す）。
/// - 行高さ・文字高さ: 全行 [`BOM_ROW_HEIGHT_MM`] / [`BOM_TEXT_HEIGHT_MM`] に統一する。
/// - `anchor`: 変更しない（配置は [`bom_anchor_above_title_block`] が別途受け持つ）。
///
/// # Errors
///
/// 新しい列構成へ移らない列に空でないセル（見出し行を除く）があるとき、その列名を挙げた
/// メッセージを返す（値を黙って捨てない）。
pub fn apply_bom_preset(table: &TableGeom, template_width_mm: f64) -> Result<TableGeom, String> {
    let (names, widths) = bom_preset_columns(template_width_mm);
    let new_cols = names.len();
    // 行が0（未検証の作業コピー）でもヘッダ1行は必ず持つ。
    let new_rows = table.rows().max(1);

    // 既存の見出し行（最下段）を読む。行が無い表は見出し名を判定できないので
    // 空とし、後段の分岐は自然と「汎用表（列番号で移す）」に落ちる。
    let old_cols = table.cols();
    let header: Vec<&str> = if table.rows() > 0 {
        let header_row = table.rows() - 1;
        (0..old_cols)
            .map(|c| table.cell(header_row, c).unwrap_or(""))
            .collect()
    } else {
        Vec::new()
    };

    // 既存の表がどの列構成の部品表かを見分け、列ごとの意味（新しい列名）を決める
    // （[`classify_bom_columns`]）。見分けられない表は `None` で、従来どおり列番号で移す。
    let meanings = classify_bom_columns(&header);
    let source_index_for = |target_name: &str| -> Option<usize> {
        meanings.as_ref()?.iter().position(|m| *m == target_name)
    };

    // 見出しが既知の並びと一致しない表は、値が入っていれば変換しない（2026-09-23 ユーザー
    // 確定）。列番号で移すと、見出しを一部書き換えた旧部品表では新しく挟まった「品番」の分だけ
    // 列の意味がずれる（値は消えないので下の検査では拒否されない。Codex adversarial review
    // 4 回目 [high]）。空の表（`K` で置いた直後など）は従来どおり変換できる。
    // 見分けられない表では最下段が見出しだという根拠が無いので、最下段も含めて全行を見る
    // （最下段を除くと、1 行だけの表や最下段にだけ値がある表が空に見え、値が見出しで
    // 上書きされて消える。Codex adversarial review 5 回目 [high]）。
    let has_values = (0..table.rows())
        .any(|r| (0..old_cols).any(|c| !table.cell(r, c).unwrap_or("").is_empty()));
    if meanings.is_none() && has_values {
        return Err(
            "見出しが部品表の列構成と一致しないため、値の入った表は部品表にできません(見出しを合わせるか、値を空にしてから変換してください)"
                .to_owned(),
        );
    }

    // 値を捨てる変換はしない（2026-09-22 ユーザー確定）: 新しい列構成へ移らない旧列に
    // 空でないセル（見出し行を除く）が 1 つでもあれば、その列名を挙げて拒否する。
    let carried = |sc: usize| match &meanings {
        Some(m) => names.contains(&m[sc]),
        None => sc < new_cols,
    };
    let lost: Vec<String> = (0..old_cols)
        .filter(|&sc| !carried(sc))
        .filter(|&sc| {
            (0..table.rows().saturating_sub(1)).any(|r| !table.cell(r, sc).unwrap_or("").is_empty())
        })
        .map(|sc| match header.get(sc) {
            Some(h) if !h.is_empty() => format!("「{h}」"),
            _ => format!("{} 列目", sc + 1),
        })
        .collect();
    if !lost.is_empty() {
        return Err(format!(
            "{} の値が新しい列構成に入らないため、部品表にできません(先に値を消すか、別の表へ移してください)",
            lost.join("・")
        ));
    }

    let mut cells = Vec::with_capacity(new_rows * new_cols);
    for r in 0..new_rows {
        let is_header = r == new_rows - 1;
        for (c, name) in names.iter().enumerate() {
            let cell = if is_header {
                (*name).to_owned()
            } else if meanings.is_some() {
                source_index_for(name)
                    .and_then(|sc| table.cell(r, sc))
                    .unwrap_or("")
                    .to_owned()
            } else {
                // 部品表と見分けられない表（汎用表）: 従来どおり列番号で移す。
                table.cell(r, c).unwrap_or("").to_owned()
            };
            cells.push(cell);
        }
    }

    Ok(TableGeom {
        anchor: table.anchor,
        col_widths_mm: widths,
        row_heights_mm: vec![BOM_ROW_HEIGHT_MM; new_rows],
        text_height_mm: BOM_TEXT_HEIGHT_MM,
        cells,
    })
}

/// 既存の表が部品表のどの列構成か（旧5列・様式A/C の7列・様式B の6列）を見分け、
/// 各列の意味（新しい列構成での列名）を返す。見分けられなければ `None`（値の入った表は
/// 変換しない）。
///
/// **見出し行がその構成の列名の並びと完全一致する表だけ**を見分ける（2026-09-22 ユーザー
/// 確定）。見出しの一部一致・過半数一致・列幅で推測する案は、どれも「汎用表を部品表と
/// 取り違えて値を落とす」か「見出しを書き換えた部品表を汎用表と取り違えて意味がずれる」の
/// どちらかの反例を持った（同日の Codex adversarial review 3 回）。表の中身だけでは
/// 部品表かどうかを確実に見分けられないため、推測はせず、表の種類そのものを持たせる
/// 設計（「部品表」と「表」の分離）を M12 で行う。それまでは [`apply_bom_preset`] が
/// 「見分けられない表に値があれば変換しない」「値を捨てるなら変換しない」で守る。
fn classify_bom_columns(header: &[&str]) -> Option<Vec<&'static str>> {
    if header == BOM_LEGACY_COLUMN_NAMES.as_slice() {
        Some(BOM_LEGACY_COLUMN_MEANINGS.to_vec())
    } else if header == BOM_COLUMN_NAMES_AC.as_slice() {
        Some(BOM_COLUMN_NAMES_AC.to_vec())
    } else if header == BOM_COLUMN_NAMES_B.as_slice() {
        Some(BOM_COLUMN_NAMES_B.to_vec())
    } else {
        None
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
            // 見出しが既知の並びと一致しない表は値が入っていると変換できないので、空にしておく。
            cells: vec![String::new(); rows * cols],
        }
    }

    // --- bom_preset_columns ---

    #[test]
    fn preset_columns_sum_matches_170() {
        let (names, widths) = bom_preset_columns(170.0);
        assert_eq!(names, &BOM_COLUMN_NAMES_AC);
        assert_eq!(widths, vec![10.0, 35.0, 35.0, 10.0, 30.0, 20.0, 30.0]);
        approx(widths.iter().sum(), 170.0);
    }

    #[test]
    fn preset_columns_sum_matches_120() {
        let (names, widths) = bom_preset_columns(120.0);
        assert_eq!(names, &BOM_COLUMN_NAMES_B);
        assert_eq!(widths, vec![10.0, 25.0, 25.0, 10.0, 30.0, 20.0]);
        approx(widths.iter().sum(), 120.0);
    }

    #[test]
    fn preset_columns_sum_matches_arbitrary_width_150() {
        let (names, widths) = bom_preset_columns(150.0);
        assert_eq!(names, &BOM_COLUMN_NAMES_AC);
        let sum: f64 = widths.iter().sum();
        approx(sum, 150.0);
        // 按分された列幅であり、170mm 版の単純な複製ではない。
        assert_ne!(
            widths,
            vec![
                10.0 * 150.0 / 170.0,
                35.0 * 150.0 / 170.0,
                35.0 * 150.0 / 170.0,
                10.0 * 150.0 / 170.0,
                30.0 * 150.0 / 170.0,
                20.0 * 150.0 / 170.0,
                40.0
            ]
        );
    }

    // --- apply_bom_preset ---

    #[test]
    fn apply_bom_preset_puts_header_row_at_the_bottom() {
        let table = sample_table(3, 2);
        let bom = apply_bom_preset(&table, 170.0).unwrap();
        assert_eq!(bom.rows(), 3);
        assert_eq!(bom.cols(), 7);
        for (c, name) in BOM_COLUMN_NAMES_AC.iter().enumerate() {
            assert_eq!(bom.cell(2, c), Some(*name));
        }
        // 最上段（ヘッダではない）はヘッダ名で上書きされない。
        assert_ne!(bom.cell(0, 0), Some("部番"));
    }

    #[test]
    fn apply_bom_preset_single_row_table_is_header_only() {
        let table = sample_table(1, 3);
        let bom = apply_bom_preset(&table, 120.0).unwrap();
        assert_eq!(bom.rows(), 1);
        for (c, name) in BOM_COLUMN_NAMES_B.iter().enumerate() {
            assert_eq!(bom.cell(0, c), Some(*name));
        }
    }

    #[test]
    fn apply_bom_preset_uses_bom_row_height_and_text_height() {
        let table = sample_table(4, 3);
        let bom = apply_bom_preset(&table, 170.0).unwrap();
        assert!(bom.row_heights_mm.iter().all(|h| *h == BOM_ROW_HEIGHT_MM));
        assert_eq!(bom.text_height_mm, BOM_TEXT_HEIGHT_MM);
    }

    #[test]
    fn apply_bom_preset_result_passes_validate() {
        use mcad_core::EntityGeom;
        let table = sample_table(3, 2);
        let bom = apply_bom_preset(&table, 170.0).unwrap();
        EntityGeom::Table(bom)
            .validate()
            .expect("bom preset table is valid");
    }

    #[test]
    fn apply_bom_preset_keeps_anchor_unchanged() {
        let table = sample_table(2, 2);
        let bom = apply_bom_preset(&table, 170.0).unwrap();
        assert_eq!(bom.anchor, table.anchor);
    }

    #[test]
    fn apply_bom_preset_maps_legacy_five_column_bom_by_name() {
        // v0.11.0 以前の旧プリセット（5列: 品番=照合番号・名称・個数・材質・備考）。
        // 部品行1行 + ヘッダ行。
        let table = TableGeom {
            anchor: Point2::new(0.0, 0.0),
            col_widths_mm: vec![10.0, 50.0, 10.0, 30.0, 70.0],
            row_heights_mm: vec![8.0, 8.0],
            text_height_mm: 3.5,
            cells: vec![
                "P-001".to_owned(),
                "ブラケット".to_owned(),
                "2".to_owned(),
                "S45C".to_owned(),
                "表面処理あり".to_owned(),
                "品番".to_owned(),
                "名称".to_owned(),
                "個数".to_owned(),
                "材質".to_owned(),
                "備考".to_owned(),
            ],
        };
        let bom = apply_bom_preset(&table, 170.0).unwrap();
        assert_eq!(bom.cols(), 7);
        // 部番(0) ← 旧「品番」(照合番号)。
        assert_eq!(bom.cell(0, 0), Some("P-001"));
        // 品番(1) ← 対応する旧列が無いので空欄。
        assert_eq!(bom.cell(0, 1), Some(""));
        assert_eq!(bom.cell(0, 2), Some("ブラケット")); // 名称
        assert_eq!(bom.cell(0, 3), Some("2")); // 個数
        assert_eq!(bom.cell(0, 4), Some("S45C")); // 材質
        // 質量[kg](5) ← 対応する旧列が無いので空欄。
        assert_eq!(bom.cell(0, 5), Some(""));
        assert_eq!(bom.cell(0, 6), Some("表面処理あり")); // 備考
        // ヘッダ行は新しい列名で上書きされる。
        for (c, name) in BOM_COLUMN_NAMES_AC.iter().enumerate() {
            assert_eq!(bom.cell(1, c), Some(*name));
        }
    }

    #[test]
    fn apply_bom_preset_ac_to_b_rejects_filled_remarks_and_moves_named_columns() {
        // 新構成（様式A/C、7列）で埋まった部品表を様式B（120mm、6列・備考なし）へ適用。
        let table = TableGeom {
            anchor: Point2::new(0.0, 0.0),
            col_widths_mm: vec![10.0, 35.0, 35.0, 10.0, 30.0, 20.0, 30.0],
            row_heights_mm: vec![8.0, 8.0],
            text_height_mm: 3.5,
            cells: vec![
                "1".to_owned(),
                "PN-1".to_owned(),
                "軸".to_owned(),
                "4".to_owned(),
                "SUS304".to_owned(),
                "0.125".to_owned(),
                "面取りC1".to_owned(),
                "部番".to_owned(),
                "品番".to_owned(),
                "名称".to_owned(),
                "個数".to_owned(),
                "材質".to_owned(),
                "質量[kg]".to_owned(),
                "備考".to_owned(),
            ],
        };
        // 備考に値があると様式B（備考なし）へは変換しない。
        let err = apply_bom_preset(&table, 120.0).unwrap_err();
        assert!(err.contains("「備考」"), "{err}");
        // 備考を空にすれば、同じ名前の列どうしで移る。
        let mut table = table;
        table.cells[6] = String::new();
        let bom = apply_bom_preset(&table, 120.0).unwrap();
        assert_eq!(bom.cols(), 6);
        assert_eq!(bom.cell(0, 0), Some("1")); // 部番
        assert_eq!(bom.cell(0, 1), Some("PN-1")); // 品番
        assert_eq!(bom.cell(0, 2), Some("軸")); // 名称
        assert_eq!(bom.cell(0, 3), Some("4")); // 個数
        assert_eq!(bom.cell(0, 4), Some("SUS304")); // 材質
        assert_eq!(bom.cell(0, 5), Some("0.125")); // 質量[kg]
        for (c, name) in BOM_COLUMN_NAMES_B.iter().enumerate() {
            assert_eq!(bom.cell(1, c), Some(*name));
        }
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

    /// 値行 1 行 + 最下段の見出し行 1 行の表を作る。
    fn two_row_table(values: &[&str], header: &[&str]) -> TableGeom {
        assert_eq!(values.len(), header.len());
        TableGeom {
            anchor: Point2::new(0.0, 0.0),
            col_widths_mm: vec![20.0; header.len()],
            row_heights_mm: vec![8.0, 8.0],
            text_height_mm: 3.5,
            cells: values
                .iter()
                .chain(header.iter())
                .map(|s| (*s).to_owned())
                .collect(),
        }
    }

    #[test]
    fn apply_bom_preset_b_to_ac_keeps_named_columns_and_adds_empty_remarks() {
        let table = two_row_table(
            &["1", "PN-1", "軸", "4", "SUS304", "0.125"],
            &BOM_COLUMN_NAMES_B,
        );
        let bom = apply_bom_preset(&table, 170.0).unwrap();
        assert_eq!(bom.cols(), 7);
        for (c, v) in ["1", "PN-1", "軸", "4", "SUS304", "0.125", ""]
            .iter()
            .enumerate()
        {
            assert_eq!(bom.cell(0, c), Some(*v));
        }
    }

    #[test]
    fn apply_bom_preset_rejects_filled_tables_with_unknown_headers() {
        // 見出しが既知の並びと一致しない表は、値が入っていれば変換しない（2026-09-23 ユーザー
        // 確定）。Codex adversarial review の 4 回の指摘の反例をまとめて固定する。
        let cases = [
            // 見出しを一部書き換えた旧部品表（列番号で移すと意味がずれる。4 回目）
            two_row_table(
                &["1", "軸", "4", "SUS304", "面取り"],
                &["品番", "名称", "数量", "材質", "備考"],
            ),
            // 旧部品表と見出しの過半数が同じ位置で一致する購買表（3 回目）
            two_row_table(
                &["1", "軸", "4", "1200", "4800"],
                &["品番", "名称", "個数", "単価", "金額"],
            ),
            // 見出しにたまたま「名称」を含む汎用表（1 回目）
            two_row_table(&["軸", "1200", "4800"], &["名称", "単価", "金額"]),
        ];
        // 最下段にだけ値がある表・1 行だけの表（5 回目。最下段を見出しとみなして値を見落とした）
        let mut last_row_only = sample_table(3, 3);
        last_row_only.cells[7] = "値".to_owned();
        let mut one_row = sample_table(1, 3);
        one_row.cells[0] = "値".to_owned();
        let cases: Vec<TableGeom> = cases.into_iter().chain([last_row_only, one_row]).collect();
        for table in &cases {
            let err = apply_bom_preset(table, 170.0).unwrap_err();
            assert!(err.contains("見出しが部品表の列構成と一致しない"), "{err}");
        }
    }

    #[test]
    fn apply_bom_preset_converts_empty_table_with_any_shape() {
        // 空の表（`K` で置いた直後など）は列数に関係なく変換できる。
        for cols in [2, 5, 8] {
            let table = sample_table(3, cols);
            let bom = apply_bom_preset(&table, 170.0).unwrap();
            assert_eq!(bom.cols(), 7);
            assert_eq!(bom.cell(0, 0), Some(""));
            assert_eq!(bom.cell(2, 0), Some("部番"));
        }
    }
}
