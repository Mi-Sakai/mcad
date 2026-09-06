//! 表（[`TableGeom`]）の展開（罫線セグメント + セル文字）。egui 非依存の純関数
//! （DESIGN.md M10 詳細設計4、タスク58）。
//!
//! # 位置づけ
//!
//! [`TableGeom`] が持つのは「アンカー・列幅・行高さ・文字高さ・セル文字列」だけで、
//! 罫線の太さ・セル内パディング・文字の寄せは**保存しない**（M10 詳細設計1）。
//! それらを与えて実際の線分と文字へ組み上げるのがこのモジュールで、画面
//! （`main.rs` の `draw_table`）・出力（`plot::push_table`）・ピック（`tool.rs`）の
//! **唯一の出所**になる。表題欄（[`crate::frame::frame_layout`]）と同じ見え方に
//! 揃えるため、線幅・パディング・垂直センタリングの式は `frame` の定数と実装を
//! そのまま再利用する。
//!
//! # 座標系と単位（**取り違え注意**）
//!
//! | 値 | 単位 |
//! |---|---|
//! | [`TableSegment::a`] / [`TableSegment::b`] | **ワールド座標** |
//! | [`TableSegment::width_mm`] | **紙 mm**（`k` を掛けない。[`crate::frame::FrameLine`] と同じ） |
//! | [`TextGeom::anchor`] | **ワールド座標** |
//! | [`TextGeom::height`] | **紙 mm**（[`TextGeom`] 本来の契約のまま） |
//!
//! 組版は紙 mm（アンカー = 局所原点、y-up）で行い、最後に `anchor + 局所 mm * k` で
//! ワールドへ写す（`k` = [`mcad_core::Scale::world_mm_per_paper_mm`]）。これは
//! `main.rs` の `text_world_aabb` が採る「anchor 基準で `k` 倍」と同じ写像である。
//!
//! **[`TextGeom::height`] を紙 mm のまま返すのは意図的**で、[`crate::dimension`] の
//! [`crate::dimension::DimExpansion`]（高さがワールド長という既存の非対称）とは違える。
//! 表の文字高さは [`TableGeom::text_height_mm`] という**保存された紙 mm 値**であり、
//! [`TextGeom::height`] とまったく同じ契約なので、consumers は Text エンティティ用の
//! 既存経路（画面 `draw_text(.., text.height * k, ..)`、出力 `plot::push_text`）を
//! そのまま再利用できる。`× k` してから出力側で `÷ k` して戻す往復も要らない。
//!
//! # 紙基準表示トグル（F9）との関係
//!
//! **表の組版は常に紙基準**（トグル非依存）。罫線の位置とセル文字の大きさ・位置は
//! 互いに依存しているので、片方だけを画面固定 px にすると文字が枠からはみ出す・
//! 罫線と文字の位置関係が崩れる（寸法注記のように「線と文字が独立に配置される」
//! 構造ではない）。Text エンティティが判断(c) で既にトグル非依存（常に `height * k`）
//! なのと同じ扱いで、これで画面と SVG/PDF の見え方も一致する。
//!
//! トグルが効くのは**罫線の画面 px 幅の解決だけ**（`resolve_stroke_px_with_toggle`）で、
//! これは図面枠（`draw_frame`）・形状エンティティと同じ規則である。
//!
//! # スナップ・線種・線幅
//!
//! - **表はスナップ源にしない**（`snap.rs` の `_` 腕。寸法と同じ扱い）。罫線の交点を
//!   スナップ源にすると表 1 つで数十〜数百の候補点が増えて近傍探索が実質役に立たなく
//!   なるうえ、表は「図形の寸法を測る対象」ではなく注記であるため。
//! - 罫線は**常に実線**（線種はエンティティのスタイルを見ない）。表題欄・寸法と同じく
//!   製図慣行として実線で描くものだから。
//! - 罫線の**線幅もエンティティ／レイヤーのスタイルを見ない**（外枠
//!   [`FRAME_BORDER_WIDTH_MM`]・内部区切り [`FRAME_DIVIDER_WIDTH_MM`] の固定値）。
//!   M10 詳細設計1 が「罫線の太さは保存しない・表題欄と同じ見え方に揃える」と決めて
//!   いるため。色だけは通常どおりスタイル（レイヤー色・エンティティ色）に従う。
//!
//! # 展開コスト（毎フレーム走査）
//!
//! [`expand_table`] は画面描画のたびに（可視な表 1 つにつき 1 回）呼ばれ、非空セル
//! 1 つあたり [`String`] を 1 個複製する。**実測**（release、2026-09-06 の開発機、
//! 一時ベンチを 20 回平均）: 部品表規模の 100 行 × 5 列（全セル非空）で **約 0.10ms**、
//! core が許す最大の 512 行 × 64 列（32768 セル全て非空）で **約 3.5ms**。前者は
//! フレーム時間に埋もれる。後者は 60fps に対して無視できない大きさだが、
//! [`mcad_core::MAX_TABLE_ROWS`] / [`mcad_core::MAX_TABLE_COLS`] が上限で押さえて
//! いるので**持続的な GUI 停止にはならない**（細工された `.mcad` に対する防御は
//! core 側の上限が担う、という M10 タスク56 の整理どおり）。可視セルだけを展開する
//! 最適化は、実使用でフレーム落ちが観測されてから検討する（先回りしない）。

use mcad_core::{TableGeom, TextGeom, approx_text_width};
use mcad_geom::{Aabb, Point2, Vec2};

use crate::frame::{CELL_TEXT_PAD_MM, FRAME_BORDER_WIDTH_MM, FRAME_DIVIDER_WIDTH_MM};

/// 表の罫線 1 本（ワールド座標）。
///
/// [`crate::frame::FrameLine`] と同じ形だが**座標の単位が違う**（あちらは紙 mm）ため
/// 別型にしてある。混ぜると `k` の掛け忘れ・二重掛けが型で防げなくなる。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TableSegment {
    /// 始点（ワールド座標）。
    pub a: Point2,
    /// 終点（ワールド座標）。
    pub b: Point2,
    /// 線幅（**紙 mm**）。外枠は [`FRAME_BORDER_WIDTH_MM`]、内部の行・列境界は
    /// [`FRAME_DIVIDER_WIDTH_MM`]。
    pub width_mm: f32,
}

/// 表の展開結果（罫線 + セル文字）。
#[derive(Debug, Clone, PartialEq)]
pub struct TableExpansion {
    /// 罫線。並び順は **外枠4辺 →（上から）行境界 →（左から）列境界**。
    ///
    /// 外枠と重なる端（最上段の上端・最下段の下端・左端・右端）の内部境界は
    /// 出さない（表題欄 [`crate::frame::frame_layout`] と同じ規則）。したがって
    /// `r` 行 `c` 列の表の本数は常に `4 + (r - 1) + (c - 1)`。
    pub segments: Vec<TableSegment>,
    /// セル文字。**空セルは含まない**（[`TableGeom::cells`] は空文字列を許すが、
    /// 描くものが無い）。並び順は行優先（行 0 = 最上段）。
    ///
    /// [`TextGeom::height`] は**紙 mm**（モジュール doc の表を参照）。
    pub texts: Vec<TextGeom>,
}

/// 表を罫線とセル文字へ展開する（このモジュールの唯一の入口、純関数）。
///
/// `k` は [`mcad_core::Scale::world_mm_per_paper_mm`]（紙 1mm あたりのワールド mm）。
///
/// # 全域関数（検証していない値でも panic しない）
///
/// [`mcad_core::EntityGeom::validate`] を通していない [`TableGeom`] — 読込直後や
/// 編集ダイアログの作業コピー（M10 タスク59）— を渡してもよい。`cells` の要素数が
/// 行 × 列と食い違っていても [`TableGeom::cell`] が `None` を返すだけで、添字の
/// パニックは起きない。**行または列が 0 の退化した表は空の展開を返す**（幅も高さも
/// 0 の外枠 4 本を出しても見えず、出力ファイルに退化パスを混ぜるだけなので）。
#[must_use]
pub fn expand_table(table: &TableGeom, k: f64) -> TableExpansion {
    let (rows, cols) = (table.rows(), table.cols());
    if rows == 0 || cols == 0 {
        return TableExpansion {
            segments: Vec::new(),
            texts: Vec::new(),
        };
    }

    let total_w = table.width_mm();
    let total_h = table.height_mm();
    // 局所（紙 mm、アンカー = 原点、y-up）→ ワールド。`text_world_aabb` と同じ
    // 「anchor 基準で k 倍」の写像。
    let to_world = |x_mm: f64, y_mm: f64| table.anchor + Vec2::new(x_mm * k, y_mm * k);

    let mut segments = Vec::with_capacity(2 + rows + cols);
    let mut push_seg = |ax: f64, ay: f64, bx: f64, by: f64, width_mm: f32| {
        segments.push(TableSegment {
            a: to_world(ax, ay),
            b: to_world(bx, by),
            width_mm,
        });
    };

    // 外枠4辺（表題欄の外枠と同じ太さ）。
    push_seg(0.0, 0.0, total_w, 0.0, FRAME_BORDER_WIDTH_MM);
    push_seg(total_w, 0.0, total_w, total_h, FRAME_BORDER_WIDTH_MM);
    push_seg(total_w, total_h, 0.0, total_h, FRAME_BORDER_WIDTH_MM);
    push_seg(0.0, total_h, 0.0, 0.0, FRAME_BORDER_WIDTH_MM);

    // 行境界とセル文字。行 0 が最上段なので、上端から下へ積む（`frame_layout` が
    // `TitleBlockTemplate::rows` を処理するのと同じ向き）。
    let mut texts = Vec::new();
    let mut row_top = total_h;
    for (r, row_height) in table.row_heights_mm.iter().enumerate() {
        let row_bottom = row_top - row_height;
        // 最下段の下端は外枠と重なるので描かない。
        if r + 1 < rows {
            push_seg(0.0, row_bottom, total_w, row_bottom, FRAME_DIVIDER_WIDTH_MM);
        }

        // セル文字: 左詰め（パディング [`CELL_TEXT_PAD_MM`]）・近似垂直センタリングの
        // 左下基準点。式は表題欄（`frame_layout`）とまったく同じで、同じ見え方になる。
        let mut cell_left = 0.0;
        for (c, col_width) in table.col_widths_mm.iter().enumerate() {
            if let Some(content) = table.cell(r, c)
                && !content.is_empty()
            {
                texts.push(TextGeom {
                    anchor: to_world(
                        cell_left + CELL_TEXT_PAD_MM,
                        row_bottom + (row_height - table.text_height_mm) / 2.0,
                    ),
                    content: content.to_owned(),
                    height: table.text_height_mm,
                    angle: 0.0,
                });
            }
            cell_left += col_width;
        }
        row_top = row_bottom;
    }

    // 列境界（右端は外枠と重なるので描かない）。
    let mut cell_right = 0.0;
    for (c, col_width) in table.col_widths_mm.iter().enumerate() {
        cell_right += col_width;
        if c + 1 < cols {
            push_seg(cell_right, 0.0, cell_right, total_h, FRAME_DIVIDER_WIDTH_MM);
        }
    }

    TableExpansion { segments, texts }
}

/// 表の表示上のワールド AABB（列幅・行高さは紙 mm なので、ワールドでは `k` 倍。
/// M10 設計方針4）。
///
/// `mcad_core::EntityGeom::aabb()`（1:1 解釈）を anchor 基準に `k` 倍したものに等しい
/// （`table_world_aabb_at_k_one_matches_core_aabb` / `..._scales_from_the_anchor` で
/// 固定）。アンカーは左下なので、写像は右上隅を `anchor + (幅, 高さ) * k` へ動かす
/// だけになる。
///
/// `text_world_aabb` と違って core の `aabb()` を呼ばずに直接組み立てているのは、
/// `EntityGeom::Table(table.clone())` が全セル文字列を複製してしまうため
/// （この関数はピック・カリング・ズームフィットで毎フレーム・全エンティティぶん
/// 呼ばれる）。
///
/// ピック（`tool.rs` の `SelectTool::pick`）と矩形選択の内包判定はこの AABB を使う
/// （Text と同じ割り切り。罫線 1 本ずつへの距離は取らない — 表は「面」として掴む方が
/// 操作として自然で、セルの空白部分でも掴める）。
///
/// # セル文字のはみ出し(Codex adversarial review 2026-09-06)
///
/// `TableGeom::validate` はセル文字列の長さを制限しないので、長い文字は列幅を超えて
/// 右へはみ出して描かれる(表題欄と同じく切り抜かない)。AABB が格子の矩形だけだと、
/// はみ出した文字がカリングで消えたり、ズームフィットで切れたり、矩形選択の内包判定と
/// 見た目が食い違う。そこで**非空セルの文字の右端**(`approx_text_width` — core の Text
/// AABB と同じ推定式)を格子の右端と比べ、超えるぶんだけ AABB を右へ広げる。文字は
/// 水平・高さは行高さ未満(validate)なので、はみ出しうるのは右方向だけ。
/// 文字列の複製はせず、`chars()` を走査するだけ(毎フレーム呼ばれる関数なので)。
#[must_use]
pub fn table_world_aabb(table: &TableGeom, k: f64) -> Aabb {
    let total_w = table.width_mm();
    let mut right_mm = total_w;
    let cols = table.cols();
    for r in 0..table.rows() {
        let mut cell_left = 0.0;
        for (c, col_width) in table.col_widths_mm.iter().enumerate().take(cols) {
            if let Some(content) = table.cell(r, c)
                && !content.is_empty()
            {
                let end =
                    cell_left + CELL_TEXT_PAD_MM + approx_text_width(content, table.text_height_mm);
                right_mm = right_mm.max(end);
            }
            cell_left += col_width;
        }
    }
    let size = Vec2::new(right_mm * k, table.height_mm() * k);
    Aabb::from_corners(table.anchor, table.anchor + size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcad_core::EntityGeom;

    const T: f64 = 1e-9;

    /// `rows` 行 `cols` 列・列幅 10mm・行高さ 8mm・文字高さ 3.5mm の表。セルは空。
    fn table(rows: usize, cols: usize, anchor: Point2) -> TableGeom {
        TableGeom {
            anchor,
            col_widths_mm: vec![10.0; cols],
            row_heights_mm: vec![8.0; rows],
            text_height_mm: 3.5,
            cells: vec![String::new(); rows * cols],
        }
    }

    fn close_to(a: f64, b: f64) {
        assert!((a - b).abs() < T, "{a} != {b}");
    }

    fn point_close_to(p: Point2, x: f64, y: f64) {
        assert!(
            (p.x - x).abs() < T && (p.y - y).abs() < T,
            "{p:?} != ({x}, {y})"
        );
    }

    fn borders(ex: &TableExpansion) -> Vec<&TableSegment> {
        ex.segments
            .iter()
            .filter(|s| (s.width_mm - FRAME_BORDER_WIDTH_MM).abs() < f32::EPSILON)
            .collect()
    }

    fn dividers(ex: &TableExpansion) -> Vec<&TableSegment> {
        ex.segments
            .iter()
            .filter(|s| (s.width_mm - FRAME_DIVIDER_WIDTH_MM).abs() < f32::EPSILON)
            .collect()
    }

    // ---- 罫線の本数と太さ ----

    #[test]
    fn rule_count_is_border_four_plus_inner_boundaries() {
        for (rows, cols) in [(1, 1), (1, 5), (5, 1), (3, 3), (12, 7)] {
            let ex = expand_table(&table(rows, cols, Point2::ORIGIN), 1.0);
            assert_eq!(
                ex.segments.len(),
                4 + (rows - 1) + (cols - 1),
                "{rows}x{cols}"
            );
            assert_eq!(borders(&ex).len(), 4, "{rows}x{cols}");
            assert_eq!(
                dividers(&ex).len(),
                (rows - 1) + (cols - 1),
                "{rows}x{cols}"
            );
        }
    }

    #[test]
    fn single_cell_table_has_only_the_outer_border() {
        let ex = expand_table(&table(1, 1, Point2::ORIGIN), 1.0);
        assert_eq!(ex.segments.len(), 4);
        assert!(dividers(&ex).is_empty());
    }

    #[test]
    fn inner_boundaries_span_the_whole_table_and_sit_on_the_cumulative_offsets() {
        let t = TableGeom {
            anchor: Point2::ORIGIN,
            col_widths_mm: vec![10.0, 20.0, 30.0],
            row_heights_mm: vec![5.0, 8.0],
            text_height_mm: 3.5,
            cells: vec![String::new(); 6],
        };
        let ex = expand_table(&t, 1.0);
        let inner = dividers(&ex);
        // 行境界1本（上から 5mm の位置 = 下から 8mm）+ 列境界2本（x = 10, 30）。
        assert_eq!(inner.len(), 3);

        let row_line = inner
            .iter()
            .find(|s| (s.a.y - s.b.y).abs() < T)
            .expect("行境界");
        point_close_to(row_line.a, 0.0, 8.0);
        point_close_to(row_line.b, 60.0, 8.0);

        let mut col_x: Vec<f64> = inner
            .iter()
            .filter(|s| (s.a.x - s.b.x).abs() < T)
            .map(|s| s.a.x)
            .collect();
        col_x.sort_by(f64::total_cmp);
        assert_eq!(col_x.len(), 2);
        close_to(col_x[0], 10.0);
        close_to(col_x[1], 30.0);
        // 列境界は表の下端から上端まで通る（外枠と重なる端は描かない、が高さは全高）。
        for s in inner.iter().filter(|s| (s.a.x - s.b.x).abs() < T) {
            close_to(s.a.y.min(s.b.y), 0.0);
            close_to(s.a.y.max(s.b.y), 13.0);
        }
    }

    #[test]
    fn outer_border_covers_the_four_corners_at_the_anchor() {
        let anchor = Point2::new(-4.0, 7.0);
        let ex = expand_table(&table(2, 3, anchor), 1.0);
        // 列幅 10 × 3 = 30、行高さ 8 × 2 = 16。アンカーは左下。
        for corner in [(-4.0, 7.0), (26.0, 7.0), (26.0, 23.0), (-4.0, 23.0)] {
            let hit = borders(&ex).iter().any(|s| {
                (s.a.x - corner.0).abs() < T && (s.a.y - corner.1).abs() < T
                    || (s.b.x - corner.0).abs() < T && (s.b.y - corner.1).abs() < T
            });
            assert!(hit, "外枠に頂点 {corner:?} が無い");
        }
    }

    // ---- 尺度（anchor 基準で k 倍） ----

    #[test]
    fn expansion_at_k_two_keeps_the_anchor_and_doubles_every_offset() {
        let anchor = Point2::new(100.0, -50.0);
        let mut t = table(2, 2, anchor);
        t.cells[0] = "A".to_owned();
        let one = expand_table(&t, 1.0);
        let two = expand_table(&t, 2.0);

        assert_eq!(one.segments.len(), two.segments.len());
        for (a, b) in one.segments.iter().zip(two.segments.iter()) {
            point_close_to(
                b.a,
                anchor.x + (a.a.x - anchor.x) * 2.0,
                anchor.y + (a.a.y - anchor.y) * 2.0,
            );
            point_close_to(
                b.b,
                anchor.x + (a.b.x - anchor.x) * 2.0,
                anchor.y + (a.b.y - anchor.y) * 2.0,
            );
            // 線幅は紙 mm なので尺度で変わらない。
            assert_eq!(a.width_mm, b.width_mm);
        }

        assert_eq!(one.texts.len(), 1);
        assert_eq!(two.texts.len(), 1);
        point_close_to(
            two.texts[0].anchor,
            anchor.x + (one.texts[0].anchor.x - anchor.x) * 2.0,
            anchor.y + (one.texts[0].anchor.y - anchor.y) * 2.0,
        );
        // 文字高さは紙 mm の契約のまま（`× k` しない）。
        assert_eq!(one.texts[0].height, 3.5);
        assert_eq!(two.texts[0].height, 3.5);
    }

    #[test]
    fn table_world_aabb_at_k_one_matches_core_aabb() {
        for (rows, cols) in [(1, 1), (3, 4)] {
            let t = table(rows, cols, Point2::new(2.5, -1.5));
            let expected = EntityGeom::Table(t.clone()).aabb();
            let actual = table_world_aabb(&t, 1.0);
            point_close_to(actual.min, expected.min.x, expected.min.y);
            point_close_to(actual.max, expected.max.x, expected.max.y);
        }
    }

    #[test]
    fn table_world_aabb_scales_from_the_anchor() {
        let anchor = Point2::new(3.0, 4.0);
        let t = table(2, 3, anchor); // 30mm × 16mm。
        let aabb = table_world_aabb(&t, 2.0);
        point_close_to(aabb.min, 3.0, 4.0);
        point_close_to(aabb.max, 3.0 + 60.0, 4.0 + 32.0);
        // 展開結果もこの箱に収まる（罫線は AABB の内側）。
        for s in &expand_table(&t, 2.0).segments {
            assert!(
                aabb.contains_point(s.a) && aabb.contains_point(s.b),
                "{s:?}"
            );
        }
    }

    // ---- セル文字 ----

    #[test]
    fn cell_text_is_left_padded_and_vertically_centred_like_the_title_block() {
        let t = TableGeom {
            anchor: Point2::ORIGIN,
            col_widths_mm: vec![10.0, 20.0],
            row_heights_mm: vec![8.0, 6.0],
            text_height_mm: 3.5,
            cells: vec![
                "r0c0".to_owned(),
                "r0c1".to_owned(),
                "r1c0".to_owned(),
                "r1c1".to_owned(),
            ],
        };
        let ex = expand_table(&t, 1.0);
        assert_eq!(ex.texts.len(), 4);
        // 並びは行優先（行 0 = 最上段）。
        let contents: Vec<&str> = ex.texts.iter().map(|t| t.content.as_str()).collect();
        assert_eq!(contents, ["r0c0", "r0c1", "r1c0", "r1c1"]);

        // 表の全高 14mm。行 0（最上段、高さ 8）の下端は y=6、行 1（高さ 6）の下端は y=0。
        // x はセル左端 + CELL_TEXT_PAD_MM、y は行下端 + (行高さ − 文字高さ)/2。
        point_close_to(ex.texts[0].anchor, 1.5, 6.0 + (8.0 - 3.5) / 2.0);
        point_close_to(ex.texts[1].anchor, 11.5, 6.0 + (8.0 - 3.5) / 2.0);
        point_close_to(ex.texts[2].anchor, 1.5, (6.0 - 3.5) / 2.0);
        point_close_to(ex.texts[3].anchor, 11.5, (6.0 - 3.5) / 2.0);
        assert!(ex.texts.iter().all(|t| t.angle == 0.0));
        assert!(ex.texts.iter().all(|t| t.height == 3.5));
    }

    #[test]
    fn row_zero_is_the_top_row() {
        let mut t = table(3, 1, Point2::ORIGIN);
        t.cells[0] = "top".to_owned();
        t.cells[2] = "bottom".to_owned();
        let ex = expand_table(&t, 1.0);
        assert_eq!(ex.texts.len(), 2);
        assert_eq!(ex.texts[0].content, "top");
        assert_eq!(ex.texts[1].content, "bottom");
        assert!(ex.texts[0].anchor.y > ex.texts[1].anchor.y);
    }

    #[test]
    fn empty_cells_produce_no_text() {
        let mut t = table(2, 2, Point2::ORIGIN);
        t.cells[1] = "only".to_owned();
        let ex = expand_table(&t, 1.0);
        assert_eq!(ex.texts.len(), 1);
        assert_eq!(ex.texts[0].content, "only");
    }

    // ---- 全域性（検証していない値でも panic しない） ----

    #[test]
    fn table_world_aabb_grows_to_the_right_for_overflowing_cell_text() {
        // 列幅 10mm・文字高さ 3.5mm に ASCII 40 文字(推定幅 0.55×3.5×40 = 77mm)を入れると
        // 文字は列を大きく超える。AABB の右端は格子(30mm)ではなく文字の右端
        // (1.5 + 77 = 78.5mm、k 倍)になり、左端・上下は変わらない。
        let mut t = table(2, 3, Point2::new(5.0, 5.0));
        t.cells[0] = "A".repeat(40);
        let k = 2.0;
        let bb = table_world_aabb(&t, k);
        let expected_right = 5.0 + (CELL_TEXT_PAD_MM + 0.55 * 3.5 * 40.0) * k;
        assert!((bb.max.x - expected_right).abs() < T, "{bb:?}");
        assert!((bb.min.x - 5.0).abs() < T);
        assert!((bb.max.y - (5.0 + 16.0 * k)).abs() < T);
        // 収まる文字では格子の矩形のまま。
        t.cells[0] = "ab".to_owned();
        let bb2 = table_world_aabb(&t, k);
        assert!((bb2.max.x - (5.0 + 30.0 * k)).abs() < T, "{bb2:?}");
    }

    #[test]
    fn degenerate_and_unvalidated_tables_expand_without_panicking() {
        // 行 0・列 0（`validate` は拒否するが、編集中の作業コピーは持ちうる）。
        let no_rows = TableGeom {
            row_heights_mm: Vec::new(),
            cells: Vec::new(),
            ..table(1, 3, Point2::ORIGIN)
        };
        assert!(expand_table(&no_rows, 1.0).segments.is_empty());
        let no_cols = TableGeom {
            col_widths_mm: Vec::new(),
            cells: Vec::new(),
            ..table(3, 1, Point2::ORIGIN)
        };
        assert!(expand_table(&no_cols, 1.0).segments.is_empty());

        // `cells` が短い（要素数 ≠ 行 × 列）。足りないセルは空欄として扱う。
        let short = TableGeom {
            cells: vec!["a".to_owned()],
            ..table(3, 3, Point2::ORIGIN)
        };
        let ex = expand_table(&short, 1.0);
        assert_eq!(ex.segments.len(), 4 + 2 + 2);
        assert_eq!(ex.texts.len(), 1);
    }
}
