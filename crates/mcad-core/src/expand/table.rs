//! 表（[`TableGeom`]）の展開（罫線セグメント + セル文字）。GUI 非依存の純関数
//! （DESIGN.md M10 詳細設計4、タスク58。M11 タスク71 で `mcad-app` から移設）。
//!
//! # 位置づけ
//!
//! [`TableGeom`] が持つのは「アンカー・列幅・列ごとの揃え・行高さ・文字高さ・セル文字列」
//! だけで、罫線の太さ・セル内パディングは**保存しない**（M10 詳細設計1。列ごとの揃えは
//! M12 タスク93 で保存するようにした — 下の「揃え」節）。
//! それらを与えて実際の線分と文字へ組み上げるのがこのモジュールで、画面
//! （`mcad-app` の `draw_table`）・出力（`plot::push_table`）・ピック（`tool.rs`）・
//! **DXF export（`mcad-io` の `table_to_dxf_entities`）**の**唯一の出所**になる。
//! M10 タスク61 では `mcad-io` が依存方向の制約（io は app に依存できない）から
//! 同じ組版を独立に実装しており、値が乖離しても検出できない残債になっていた。
//! M11 設計判断1 でこのモジュールを core へ移し、その二重化を解消した。
//!
//! 表題欄（`mcad-app` の `frame::frame_layout`）と同じ見え方に揃えるため、線幅・
//! パディング・垂直センタリングの式は表題欄と共通で、定数
//! （[`FRAME_BORDER_WIDTH_MM`] / [`FRAME_DIVIDER_WIDTH_MM`] / [`CELL_TEXT_PAD_MM`]）も
//! ここが唯一の定義で `mcad-app` の `frame` が再エクスポートしている。
//!
//! # 座標系と単位（**取り違え注意**）
//!
//! | 値 | 単位 |
//! |---|---|
//! | [`TableSegment::a`] / [`TableSegment::b`] | **ワールド座標** |
//! | [`TableSegment::width_mm`] | **紙 mm**（`k` を掛けない。`mcad-app` の `frame::FrameLine` と同じ） |
//! | [`TableCellText::text`] の [`TextGeom::anchor`] | **ワールド座標**（揃えの基準点。下の「揃え」節） |
//! | [`TableCellText::text`] の [`TextGeom::height`] | **紙 mm**（[`TextGeom`] 本来の契約のまま） |
//!
//! 組版は紙 mm（アンカー = 局所原点、y-up）で行い、最後に `anchor + 局所 mm * k` で
//! ワールドへ写す（`k` = [`Scale::world_mm_per_paper_mm`](crate::Scale::world_mm_per_paper_mm)）。
//! これは `mcad-app` の `text_world_aabb` が採る「anchor 基準で `k` 倍」と同じ写像である。
//!
//! **[`TextGeom::height`] を紙 mm のまま返すのは意図的**で、寸法の
//! [`DimExpansion`](super::DimExpansion)（高さがワールド長という既存の非対称）とは違える。
//! 表の文字高さは [`TableGeom::text_height_mm`] という**保存された紙 mm 値**であり、
//! [`TextGeom::height`] とまったく同じ契約なので、consumers は Text エンティティ用の
//! 既存経路（画面 `draw_text(.., text.height * k, ..)`、出力 `plot::push_text`）を
//! そのまま再利用できる。`× k` してから出力側で `÷ k` して戻す往復も要らない。
//!
//! # 揃え（M12 タスク93、DESIGN.md M12 設計判断11）
//!
//! セル文字は [`TableGeom::col_aligns`] の列ごとの揃え（[`HorizontalAlign`]）で置く。
//! 展開はセル文字を**「[`TextGeom`] + 揃え」の組**（[`TableCellText`]）で返し、
//! [`TextGeom::anchor`] には**揃えの基準点**を入れる:
//!
//! | 揃え | 基準点の x（セル左端からの紙 mm） | 意味 |
//! |---|---|---|
//! | 左 | [`CELL_TEXT_PAD_MM`] | 文字列の左端（M12 以前と同じ位置・同じ式） |
//! | 中央 | 列幅 / 2 | 文字列の字送り幅の中央 |
//! | 右 | 列幅 − [`CELL_TEXT_PAD_MM`] | 文字列の字送り幅の右端 |
//!
//! y はどの揃えでも同じ（ベースライン = 行の下端 + (行高さ − 文字高さ) / 2 の近似垂直
//! センタリング。表題欄と同じ式）。
//!
//! **中央・右揃えの位置を core で文字幅を推定して左端アンカーへ換算することはしない。**
//! 推定幅（[`approx_text_width`]）は CJK やフォントの違いで実際の描画幅と食い違い、
//! 画面と出力で位置がずれるからである。代わりに各出力が**自分の描く文字列の実際の幅**で
//! 揃える:
//!
//! - 画面（`mcad-app` の `draw_table`）: egui がレイアウトしたガリーの幅で始点をずらす
//!   （`egui::Align` の係数と同じ 0 / 0.5 / 1。[`HorizontalAlign::anchor_fraction`]）。
//! - SVG/PDF（`mcad-app` の `plot::push_table`）: 文字は `<text>` ではなくアウトライン
//!   （塗りパス）で出す（M8 設計判断7）ので、SVG の `text-anchor` や PDF の文字演算子は
//!   使えない。アウトラインを作る埋め込みフォントの**字送りの合計**で始点をずらす
//!   （描く字形と同じフォント・同じ字送りなので推定ではない）。
//! - DXF（`mcad-io` の `table_to_dxf_entities`）: `TEXT` の水平揃え（group 72）と
//!   揃えの基準点（group 11）をそのまま書き、幅の計算は読み手の CAD に任せる。
//!
//! 3 出力とも「基準点 = 字送り幅の同じ割合の位置」なので、**基準点の位置は定義上一致する**。
//! 字送り幅そのものはフォントの違い（画面の ASCII は egui 既定フォント、出力は
//! Noto Sans JP、DXF は読み手のフォント）で出力ごとに違いうるが、それは左揃えでも
//! 同じ（文字列の右端の位置が違う）で、揃えが持ち込む差ではない。
//!
//! 左揃えは基準点・式とも M12 以前とまったく同じで、各出力も左揃えではずらし計算を
//! 通さない（**M12 以前の出力とビット一致**）。
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
//! [`MAX_TABLE_ROWS`](crate::MAX_TABLE_ROWS) /
//! [`MAX_TABLE_COLS`](crate::MAX_TABLE_COLS) が上限で押さえて
//! いるので**持続的な GUI 停止にはならない**（細工された `.mcad` に対する防御は
//! core 側の上限が担う、という M10 タスク56 の整理どおり）。可視セルだけを展開する
//! 最適化は、実使用でフレーム落ちが観測されてから検討する（先回りしない）。

use mcad_geom::{Aabb, Point2, Vec2};

use crate::{HorizontalAlign, TableGeom, TextGeom, approx_text_width};

/// 輪郭線・表題欄外枠・表の外枠の線幅（紙 mm、`製図規定.md` 2-3 の 2) / 2-4-3 の a) 外枠）。
///
/// `mcad-app` の `frame` モジュールが再エクスポートしており、図面枠・表題欄・表の
/// 3 か所がこの 1 つの値を共有する（M11 タスク71 で `mcad-app` から移設。展開が core へ
/// 移った以上、展開が使う定数も core が持たないと `mcad-io` から参照できない）。
pub const FRAME_BORDER_WIDTH_MM: f32 = 0.5;

/// 表題欄・表の内部区切り線（行境界・セル境界）の線幅
/// （紙 mm、`製図規定.md` 2-4-3 の a) 区切り線）。
pub const FRAME_DIVIDER_WIDTH_MM: f32 = 0.13;

/// セル左端から文字アンカーまでの詰め（紙 mm）。`製図規定.md` はセル内の文字の
/// 詰め量までは規定していないため、可読な余白として自前で定義した値。
///
/// 右揃えの列ではセル右端からの詰めにも同じ値を使う（M12 タスク93。左右で余白を揃える）。
pub const CELL_TEXT_PAD_MM: f64 = 1.5;

/// 表の罫線 1 本（ワールド座標）。
///
/// `mcad-app` の `frame::FrameLine` と同じ形だが**座標の単位が違う**（あちらは紙 mm）ため
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

/// 表のセル文字 1 つ（M12 タスク93）。**[`TextGeom`] と水平揃えの組**。
///
/// [`TextGeom::anchor`] は揃えの**基準点**（ワールド座標、ベースライン上）で、
/// 左揃えのときだけ [`TextGeom`] 本来の意味（ベースライン左端）と一致する。中央・右揃えの
/// ときは、描く側が自分の文字列の字送り幅 × [`HorizontalAlign::anchor_fraction`] だけ
/// 始点をずらす（モジュール doc「揃え」節）。**`text` だけを取り出して Text エンティティと
/// 同じ経路へ流すと、中央・右揃えの文字が右へずれる**ので、必ず `align` と組で扱う。
///
/// 揃えを [`TextGeom`] のフィールドにしないのは、[`TextGeom`] が文字エンティティの
/// 保存形式でもあるため（足すと `.mcad` の文字エンティティの形が変わる）。
#[derive(Debug, Clone, PartialEq)]
pub struct TableCellText {
    /// セル文字（アンカーは揃えの基準点。高さは紙 mm）。
    pub text: TextGeom,
    /// 水平揃え（その列の [`TableGeom::col_aligns`]）。
    pub align: HorizontalAlign,
}

/// 表の展開結果（罫線 + セル文字）。
#[derive(Debug, Clone, PartialEq)]
pub struct TableExpansion {
    /// 罫線。並び順は **外枠4辺 →（上から）行境界 →（左から）列境界**。
    ///
    /// 外枠と重なる端（最上段の上端・最下段の下端・左端・右端）の内部境界は
    /// 出さない（表題欄 `mcad-app` の `frame::frame_layout` と同じ規則）。したがって
    /// `r` 行 `c` 列の表の本数は常に `4 + (r - 1) + (c - 1)`。
    pub segments: Vec<TableSegment>,
    /// セル文字（[`TextGeom`] + 揃え）。**空セルは含まない**（[`TableGeom::cells`] は
    /// 空文字列を許すが、描くものが無い）。並び順は行優先（行 0 = 最上段）。
    ///
    /// [`TextGeom::height`] は**紙 mm**、[`TextGeom::anchor`] は揃えの基準点
    /// （モジュール doc の表と「揃え」節を参照）。
    pub texts: Vec<TableCellText>,
}

/// セル左端 `cell_left`（局所の紙 mm）・列幅 `col_width` の列で、揃え `align` の基準点の
/// x（局所の紙 mm）を返す。モジュール doc「揃え」節の表の実体。
///
/// [`expand_table`]（描く位置）と [`table_world_aabb`]（はみ出しの推定）が共有する。
/// 左揃えの式は M12 以前の `cell_left + CELL_TEXT_PAD_MM` のまま（ビット一致）。
fn cell_anchor_x(cell_left: f64, col_width: f64, align: HorizontalAlign) -> f64 {
    match align {
        HorizontalAlign::Left => cell_left + CELL_TEXT_PAD_MM,
        HorizontalAlign::Center => cell_left + col_width / 2.0,
        HorizontalAlign::Right => cell_left + col_width - CELL_TEXT_PAD_MM,
    }
}

/// 列 `c` の揃え。**要素数が列数に足りない**（[`EntityGeom::validate`] を通していない
/// 値）ときは左揃えとして扱う（[`expand_table`] の全域性。`validate` は食い違いを拒否する）。
///
/// [`EntityGeom::validate`]: crate::EntityGeom::validate
fn column_align(table: &TableGeom, c: usize) -> HorizontalAlign {
    table.col_aligns.get(c).copied().unwrap_or_default()
}

/// 表を罫線とセル文字へ展開する（このモジュールの唯一の入口、純関数）。
///
/// `k` は [`Scale::world_mm_per_paper_mm`](crate::Scale::world_mm_per_paper_mm)
/// （紙 1mm あたりのワールド mm）。
///
/// # 全域関数（検証していない値でも panic しない）
///
/// [`EntityGeom::validate`](crate::EntityGeom::validate) を通していない
/// [`TableGeom`] — 読込直後や
/// 編集ダイアログの作業コピー（M10 タスク59）— を渡してもよい。`cells` の要素数が
/// 行 × 列と食い違っていても [`TableGeom::cell`] が `None` を返すだけで、添字の
/// パニックは起きない。`col_aligns` が足りない列は左揃えとして描く。
/// **行または列が 0 の退化した表は空の展開を返す**（幅も高さも
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

        // セル文字: 列の揃えの基準点（左 = パディング [`CELL_TEXT_PAD_MM`]、中央、
        // 右 = 右端からパディング。[`cell_anchor_x`]）・近似垂直センタリングのベースライン。
        // 左揃えの式は表題欄（`frame_layout`）とまったく同じで、同じ見え方になる。
        let mut cell_left = 0.0;
        for (c, col_width) in table.col_widths_mm.iter().enumerate() {
            if let Some(content) = table.cell(r, c)
                && !content.is_empty()
            {
                let align = column_align(table, c);
                texts.push(TableCellText {
                    text: TextGeom {
                        anchor: to_world(
                            cell_anchor_x(cell_left, *col_width, align),
                            row_bottom + (row_height - table.text_height_mm) / 2.0,
                        ),
                        content: content.to_owned(),
                        height: table.text_height_mm,
                        angle: 0.0,
                    },
                    align,
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
/// `EntityGeom::aabb()`（1:1 解釈）を anchor 基準に `k` 倍したものに等しい
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
/// はみ出して描かれる(表題欄と同じく切り抜かない)。AABB が格子の矩形だけだと、
/// はみ出した文字がカリングで消えたり、ズームフィットで切れたり、矩形選択の内包判定と
/// 見た目が食い違う。そこで**非空セルの文字の両端**(`approx_text_width` — core の Text
/// AABB と同じ推定式)を格子の両端と比べ、超えるぶんだけ AABB を広げる。文字は
/// 水平・高さは行高さ未満(validate)なので、はみ出しうるのは左右だけ: 左揃えは右へ、
/// 右揃えは左へ、中央揃えは両側へ(M12 タスク93。揃えの基準点は [`expand_table`] と
/// 同じ [`cell_anchor_x`])。ここでの幅は**推定**だが、AABB は描画位置を決めない
/// (カリング・選択の当たり判定の近似。Text の AABB と同じ割り切り)。
/// 文字列の複製はせず、`chars()` を走査するだけ(毎フレーム呼ばれる関数なので)。
///
/// 左揃えだけの表では左端は格子の左端(= アンカー)のままで、結果は M12 以前と一致する。
#[must_use]
pub fn table_world_aabb(table: &TableGeom, k: f64) -> Aabb {
    let total_w = table.width_mm();
    let mut left_mm: f64 = 0.0;
    let mut right_mm = total_w;
    let cols = table.cols();
    for r in 0..table.rows() {
        let mut cell_left = 0.0;
        for (c, col_width) in table.col_widths_mm.iter().enumerate().take(cols) {
            if let Some(content) = table.cell(r, c)
                && !content.is_empty()
            {
                let align = column_align(table, c);
                let anchor_x = cell_anchor_x(cell_left, *col_width, align);
                let width = approx_text_width(content, table.text_height_mm);
                if align == HorizontalAlign::Left {
                    // M12 以前と同じ式（ビット一致のため分けておく）。
                    right_mm = right_mm.max(anchor_x + width);
                } else {
                    let start = anchor_x - width * align.anchor_fraction();
                    left_mm = left_mm.min(start);
                    right_mm = right_mm.max(start + width);
                }
            }
            cell_left += col_width;
        }
    }
    let size = Vec2::new(right_mm * k, table.height_mm() * k);
    let min = if left_mm < 0.0 {
        Point2::new(table.anchor.x + left_mm * k, table.anchor.y)
    } else {
        table.anchor
    };
    Aabb::from_corners(min, table.anchor + size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EntityGeom, TableKind};

    const T: f64 = 1e-9;

    /// `rows` 行 `cols` 列・列幅 10mm・行高さ 8mm・文字高さ 3.5mm の表。セルは空。
    fn table(rows: usize, cols: usize, anchor: Point2) -> TableGeom {
        TableGeom {
            kind: TableKind::Table,
            anchor,
            col_widths_mm: vec![10.0; cols],
            col_aligns: vec![HorizontalAlign::Left; cols],
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
            kind: TableKind::Table,
            anchor: Point2::ORIGIN,
            col_widths_mm: vec![10.0, 20.0, 30.0],
            col_aligns: vec![HorizontalAlign::Left; 3],
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
            two.texts[0].text.anchor,
            anchor.x + (one.texts[0].text.anchor.x - anchor.x) * 2.0,
            anchor.y + (one.texts[0].text.anchor.y - anchor.y) * 2.0,
        );
        // 文字高さは紙 mm の契約のまま（`× k` しない）。
        assert_eq!(one.texts[0].text.height, 3.5);
        assert_eq!(two.texts[0].text.height, 3.5);
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
            kind: TableKind::Table,
            anchor: Point2::ORIGIN,
            col_widths_mm: vec![10.0, 20.0],
            col_aligns: vec![HorizontalAlign::Left; 2],
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
        let contents: Vec<&str> = ex.texts.iter().map(|t| t.text.content.as_str()).collect();
        assert_eq!(contents, ["r0c0", "r0c1", "r1c0", "r1c1"]);

        // 表の全高 14mm。行 0（最上段、高さ 8）の下端は y=6、行 1（高さ 6）の下端は y=0。
        // x はセル左端 + CELL_TEXT_PAD_MM、y は行下端 + (行高さ − 文字高さ)/2。
        point_close_to(ex.texts[0].text.anchor, 1.5, 6.0 + (8.0 - 3.5) / 2.0);
        point_close_to(ex.texts[1].text.anchor, 11.5, 6.0 + (8.0 - 3.5) / 2.0);
        point_close_to(ex.texts[2].text.anchor, 1.5, (6.0 - 3.5) / 2.0);
        point_close_to(ex.texts[3].text.anchor, 11.5, (6.0 - 3.5) / 2.0);
        assert!(ex.texts.iter().all(|t| t.text.angle == 0.0));
        assert!(ex.texts.iter().all(|t| t.text.height == 3.5));
    }

    #[test]
    fn row_zero_is_the_top_row() {
        let mut t = table(3, 1, Point2::ORIGIN);
        t.cells[0] = "top".to_owned();
        t.cells[2] = "bottom".to_owned();
        let ex = expand_table(&t, 1.0);
        assert_eq!(ex.texts.len(), 2);
        assert_eq!(ex.texts[0].text.content, "top");
        assert_eq!(ex.texts[1].text.content, "bottom");
        assert!(ex.texts[0].text.anchor.y > ex.texts[1].text.anchor.y);
    }

    #[test]
    fn empty_cells_produce_no_text() {
        let mut t = table(2, 2, Point2::ORIGIN);
        t.cells[1] = "only".to_owned();
        let ex = expand_table(&t, 1.0);
        assert_eq!(ex.texts.len(), 1);
        assert_eq!(ex.texts[0].text.content, "only");
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

    // ---- 揃え（M12 タスク93） ----

    /// 1 行 3 列（列幅 10/20/30mm、左・中央・右揃え）・行高さ 8mm・文字高さ 3.5mm。
    fn aligned_table(anchor: Point2) -> TableGeom {
        TableGeom {
            kind: TableKind::Table,
            anchor,
            col_widths_mm: vec![10.0, 20.0, 30.0],
            col_aligns: vec![
                HorizontalAlign::Left,
                HorizontalAlign::Center,
                HorizontalAlign::Right,
            ],
            row_heights_mm: vec![8.0],
            text_height_mm: 3.5,
            cells: vec!["L".to_owned(), "C".to_owned(), "R".to_owned()],
        }
    }

    #[test]
    fn cell_text_anchor_is_the_alignment_reference_point_of_its_column() {
        // 左 = セル左端 + パディング、中央 = セル中央、右 = セル右端 − パディング
        // （モジュール doc「揃え」節の表）。y（ベースライン）は揃えに依らない。
        let ex = expand_table(&aligned_table(Point2::ORIGIN), 1.0);
        assert_eq!(ex.texts.len(), 3);
        let baseline = (8.0 - 3.5) / 2.0;
        point_close_to(ex.texts[0].text.anchor, CELL_TEXT_PAD_MM, baseline);
        point_close_to(ex.texts[1].text.anchor, 10.0 + 20.0 / 2.0, baseline);
        point_close_to(ex.texts[2].text.anchor, 60.0 - CELL_TEXT_PAD_MM, baseline);
        let aligns: Vec<HorizontalAlign> = ex.texts.iter().map(|t| t.align).collect();
        assert_eq!(
            aligns,
            [
                HorizontalAlign::Left,
                HorizontalAlign::Center,
                HorizontalAlign::Right
            ]
        );
        // 文字の中身・高さ・角度は揃えに関係なく同じ契約（紙 mm・水平）。
        assert!(ex.texts.iter().all(|t| t.text.height == 3.5));
        assert!(ex.texts.iter().all(|t| t.text.angle == 0.0));
    }

    #[test]
    fn aligned_anchors_scale_from_the_table_anchor_like_the_rules() {
        // 基準点も罫線と同じ「anchor 基準で k 倍」の写像に乗る（尺度をまたいで紙の上の
        // 位置が変わらない）。
        let anchor = Point2::new(100.0, -50.0);
        let one = expand_table(&aligned_table(anchor), 1.0);
        let two = expand_table(&aligned_table(anchor), 2.0);
        for (a, b) in one.texts.iter().zip(two.texts.iter()) {
            assert_eq!(a.align, b.align);
            point_close_to(
                b.text.anchor,
                anchor.x + (a.text.anchor.x - anchor.x) * 2.0,
                anchor.y + (a.text.anchor.y - anchor.y) * 2.0,
            );
        }
    }

    #[test]
    fn left_aligned_cell_text_is_bit_identical_to_the_pre_m12_formula() {
        // 左揃えは M12 以前の式 `anchor + ((cell_left + PAD) * k, (row_bottom +
        // (row_height − text_height) / 2) * k)` と**ビット一致**する（許容誤差で比べない）。
        // 半端な値・尺度で丸めの順序まで固定する。
        let t = TableGeom {
            kind: TableKind::Table,
            anchor: Point2::new(123.456_789, -7.891_011),
            col_widths_mm: vec![12.3, 45.67, 8.9],
            col_aligns: vec![HorizontalAlign::Left; 3],
            row_heights_mm: vec![7.7, 9.13],
            text_height_mm: 3.3,
            cells: (0..6).map(|i| format!("c{i}")).collect(),
        };
        let k = 0.37;
        let ex = expand_table(&t, k);
        assert_eq!(ex.texts.len(), 6);
        let mut expected = Vec::new();
        let mut row_top = t.height_mm();
        for row_height in &t.row_heights_mm {
            let row_bottom = row_top - row_height;
            let mut cell_left = 0.0;
            for col_width in &t.col_widths_mm {
                expected.push(
                    t.anchor
                        + Vec2::new(
                            (cell_left + CELL_TEXT_PAD_MM) * k,
                            (row_bottom + (row_height - t.text_height_mm) / 2.0) * k,
                        ),
                );
                cell_left += col_width;
            }
            row_top = row_bottom;
        }
        let actual: Vec<Point2> = ex.texts.iter().map(|c| c.text.anchor).collect();
        assert_eq!(actual, expected);
        assert!(ex.texts.iter().all(|c| c.align == HorizontalAlign::Left));
    }

    #[test]
    fn missing_column_alignments_expand_as_left() {
        // `col_aligns` が列数に足りない未検証の値（`validate` は拒否する）でも panic せず、
        // 足りない列は左揃えで描く（全域関数の契約）。
        let t = TableGeom {
            col_aligns: vec![HorizontalAlign::Right],
            ..aligned_table(Point2::ORIGIN)
        };
        let ex = expand_table(&t, 1.0);
        assert_eq!(ex.texts.len(), 3);
        assert_eq!(ex.texts[0].align, HorizontalAlign::Right);
        assert_eq!(ex.texts[1].align, HorizontalAlign::Left);
        assert_eq!(ex.texts[2].align, HorizontalAlign::Left);
        point_close_to(ex.texts[1].text.anchor, 10.0 + CELL_TEXT_PAD_MM, 2.25);
        // AABB も同じ扱いで panic しない。
        let _ = table_world_aabb(&t, 1.0);
    }

    #[test]
    fn table_world_aabb_grows_to_the_left_for_right_and_centre_aligned_overflow() {
        // 列幅 10mm・文字高さ 3.5mm に ASCII 40 文字（推定幅 0.55×3.5×40 = 77mm）。
        let long = "A".repeat(40);
        let width = 0.55 * 3.5 * 40.0;
        let anchor = Point2::new(5.0, 5.0);
        let k = 2.0;
        let one_col = |align: HorizontalAlign| TableGeom {
            kind: TableKind::Table,
            anchor,
            col_widths_mm: vec![10.0],
            col_aligns: vec![align],
            row_heights_mm: vec![8.0],
            text_height_mm: 3.5,
            cells: vec![long.clone()],
        };

        // 右揃え: 右端はセル右端 − パディング（格子内）、左端は基準点 − 幅（左へはみ出す）。
        let bb = table_world_aabb(&one_col(HorizontalAlign::Right), k);
        let start = 10.0 - CELL_TEXT_PAD_MM - width;
        assert!((bb.min.x - (5.0 + start * k)).abs() < T, "{bb:?}");
        assert!((bb.max.x - (5.0 + 10.0 * k)).abs() < T, "{bb:?}");
        assert!((bb.min.y - 5.0).abs() < T && (bb.max.y - (5.0 + 8.0 * k)).abs() < T);

        // 中央揃え: 両側へ半分ずつ。
        let bb = table_world_aabb(&one_col(HorizontalAlign::Center), k);
        assert!(
            (bb.min.x - (5.0 + (5.0 - width / 2.0) * k)).abs() < T,
            "{bb:?}"
        );
        assert!(
            (bb.max.x - (5.0 + (5.0 + width / 2.0) * k)).abs() < T,
            "{bb:?}"
        );

        // 左揃え: 従来どおり右へだけ広がり、左下はアンカーのまま（厳密に一致）。
        let bb = table_world_aabb(&one_col(HorizontalAlign::Left), k);
        assert_eq!(bb.min, anchor);
        assert!((bb.max.x - (5.0 + (CELL_TEXT_PAD_MM + width) * k)).abs() < T);

        // 収まる文字なら揃えに関係なく格子の矩形のまま。
        for align in [
            HorizontalAlign::Left,
            HorizontalAlign::Center,
            HorizontalAlign::Right,
        ] {
            let t = TableGeom {
                cells: vec!["ab".to_owned()],
                ..one_col(align)
            };
            let bb = table_world_aabb(&t, k);
            assert_eq!(bb.min, anchor, "{align:?}");
            assert!((bb.max.x - (5.0 + 10.0 * k)).abs() < T, "{align:?}: {bb:?}");
        }
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
