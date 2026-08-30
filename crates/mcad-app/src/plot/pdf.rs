//! PDF 直列化バックエンド（M8 タスク40、DESIGN.md M8 設計判断7）。
//!
//! [`PlotPage`]（紙 mm・原点=用紙左下・y-up、[`super`] モジュール doc 参照）を
//! PDF バイト列へ落とす。[`crate::plot::svg`] の実装パターン
//! （`to_xxx(&PlotPage) -> ...`）のミラーだが、座標系・グラフィックス状態の扱いに
//! PDF 固有の事情があるため以下に記す。
//!
//! egui には一切依存しない（`egui::` を import しない）。
//!
//! # y 反転をしない理由
//!
//! IR は紙 mm・原点=用紙左下・y-up（[`super`] モジュール doc）。SVG は y-down なので
//! `svg.rs` が `y_svg = height_mm - y` を適用するが、**PDF のユーザー空間は元来
//! y-up**（原点は左下）なので、IR の座標をそのまま素通しできる。ここで独自に
//! y 反転を入れると SVG 側と二重に扱いが変わってしまい、判断7が禁じる
//! 「バックエンドごとの分岐」の温床になる。
//!
//! # cm 変換 1 回 + mm 直書きの契約
//!
//! PDF ユーザー空間の既定単位は 1/72 インチ（pt）だが、IR は mm を単位に持つ。
//! 毎回の座標を pt へ換算して書く代わりに、コンテンツストリームの先頭で
//! `cm` 演算子により座標系を `[K 0 0 K 0 0]`（`K` = [`MM_TO_PT`]）でスケールし、
//! 以降のパス座標は **mm の数値をそのまま** 書く。線幅・破線長も紙 mm の数値の
//! ままでよい（`cm` が等方スケールなので線幅にも同じ係数がかかる）。
//!
//! # q/Q を各パスで自己完結させる理由
//!
//! PDF のグラフィックス状態（線幅・色・破線パターン等）はパスをまたいで持続する
//! （SVG の `<path>` 要素が独立した属性を持つのとは違う、PDF との最大の構造差）。
//! 無対策だとあるパスで設定した破線パターンが、破線を持たない後続パスへ
//! 意図せず「リーク」する。これを防ぐため [`push_path`] は必ず
//! `save_state`（`q`）で始め `restore_state`（`Q`）で終える。
//!
//! # 描画演算子は 1 回だけ
//!
//! `(stroke, fill)` の組み合わせごとに描画演算子は **ちょうど 1 回**:
//! `stroke` のみ → `S`、`fill` のみ → `f`（nonzero）、両方 → `B`
//! （`fill_nonzero_and_stroke`）。evenodd 版の `f*`/`B*` は使わない — IR の
//! fill-rule は常に nonzero（[`PlotPath::fill`] のドキュメント参照）。
//!
//! # miter limit 4.0
//!
//! SVG の既定 `stroke-miterlimit` は 4 だが、PDF の既定 miter limit は 10。
//! 両バックエンドで見た目を揃えるため、コンテンツストリームの先頭
//! （最初の `q`/`Q` より前）で `4 M` を明示する。`q`/`Q` の外に置くことで
//! 各パスの `Q` で消えず、ページ全体に効く。
//!
//! # 決定性・無圧縮
//!
//! - `CreationDate` 等の info 辞書は一切書かない。タイムスタンプを含めると
//!   同じ入力から生成したファイルが毎回異なるバイト列になり、テストの
//!   バイト完全一致比較（決定性の検証）ができなくなる。
//! - コンテンツストリームは圧縮しない。テストがストリームを直接文字列として
//!   検証するため、圧縮（Flate 等）を挟むとテストが内容を読めなくなる。

use mcad_core::Rgb;
use pdf_writer::{Content, Finish, Pdf, Rect, Ref};

use super::{PathCmd, PlotPage, PlotPath};

/// mm → pt（PDF ユーザー空間の既定単位）の換算係数。1 インチ = 25.4mm = 72pt。
const MM_TO_PT: f64 = 72.0 / 25.4;

/// [`PlotPage`] を単一ページの PDF バイト列へ直列化する。
///
/// [`PlotPage::background`] が白（[`Rgb::WHITE`]）以外のとき（青図モード）だけ、
/// コンテンツストリームの先頭に用紙全面の塗り rect を `q`/`Q` で自己完結させて描く。
/// PDF のページは元来白なので、白背景では何も描かず既存出力のバイト列を変えない。
#[must_use]
pub fn to_pdf(page: &PlotPage) -> Vec<u8> {
    let k = MM_TO_PT as f32;

    let mut content = Content::new();
    content.transform([k, 0.0, 0.0, k, 0.0, 0.0]);
    // SVG の既定 stroke-miterlimit（4）に合わせる。q/Q の外（コンテンツ先頭）に
    // 置くことで、各パスの Q で消えずページ全体へ効く。
    content.set_miter_limit(4.0);
    if page.background != Rgb::WHITE {
        push_background(&mut content, page.width_mm, page.height_mm, page.background);
    }
    for path in &page.paths {
        push_path(&mut content, path);
    }

    let catalog_id = Ref::new(1);
    let page_tree_id = Ref::new(2);
    let page_id = Ref::new(3);
    let content_id = Ref::new(4);

    let mut pdf = Pdf::new();
    pdf.catalog(catalog_id).pages(page_tree_id);
    pdf.pages(page_tree_id).kids([page_id]).count(1);

    let mut pdf_page = pdf.page(page_id);
    pdf_page.media_box(Rect::new(
        0.0,
        0.0,
        (page.width_mm * MM_TO_PT) as f32,
        (page.height_mm * MM_TO_PT) as f32,
    ));
    pdf_page.parent(page_tree_id);
    pdf_page.contents(content_id);
    // 名前参照は 1 つも使わないが、Resources は仕様上必須なので空でも明示する
    // （厳格なリーダー対策）。
    pdf_page.resources();
    pdf_page.finish();

    pdf.stream(content_id, &content.finish());

    // CreationDate 等の info 辞書は書かない（決定性のため、モジュール doc 参照）。
    pdf.finish()
}

/// 用紙全面を塗る矩形を先頭へ書く（青図モードなど背景が白でないときのみ呼ばれる）。
///
/// `q`/`Q` で自己完結させる（[`push_path`] と同じ流儀）ため、後続パスの
/// 塗り色設定へリークしない。座標は紙 mm のまま（コンテンツ先頭の `cm` 変換が
/// 効いている）。
fn push_background(content: &mut Content, width_mm: f64, height_mm: f64, color: Rgb) {
    content.save_state();
    set_rgb_fill(content, color);
    content.rect(0.0, 0.0, width_mm as f32, height_mm as f32);
    content.fill_nonzero();
    content.restore_state();
}

/// 1 本の `PlotPath` をコンテンツストリームへ書く。`q`/`Q` で自己完結させ、
/// グラフィックス状態が後続パスへリークしないようにする（モジュール doc 参照）。
fn push_path(content: &mut Content, path: &PlotPath) {
    if path.stroke.is_none() && path.fill.is_none() {
        return;
    }

    content.save_state();

    if let Some(stroke) = path.stroke {
        content.set_line_width(stroke.width_mm);
        set_rgb_stroke(content, stroke.color);
        if let Some(dash) = stroke.dash_mm {
            // 位相は常に 0（IR 契約どおり、モジュール doc・PlotStroke::dash_mm 参照）。
            content.set_dash_pattern(dash.iter().copied(), 0.0);
        }
    }
    if let Some(fill) = path.fill {
        set_rgb_fill(content, fill);
    }

    for cmd in &path.cmds {
        match cmd {
            PathCmd::MoveTo(p) => {
                content.move_to(p.x as f32, p.y as f32);
            }
            PathCmd::LineTo(p) => {
                content.line_to(p.x as f32, p.y as f32);
            }
            PathCmd::CurveTo(c1, c2, p) => {
                content.cubic_to(
                    c1.x as f32,
                    c1.y as f32,
                    c2.x as f32,
                    c2.y as f32,
                    p.x as f32,
                    p.y as f32,
                );
            }
            PathCmd::Close => {
                content.close_path();
            }
        }
    }

    // 描画演算子は (stroke, fill) の組でちょうど 1 回。
    match (path.stroke.is_some(), path.fill.is_some()) {
        (true, false) => {
            content.stroke();
        }
        (false, true) => {
            content.fill_nonzero();
        }
        (true, true) => {
            content.fill_nonzero_and_stroke();
        }
        (false, false) => unreachable!("早期 return 済み"),
    }

    content.restore_state();
}

fn set_rgb_stroke(content: &mut Content, color: Rgb) {
    content.set_stroke_rgb(
        f32::from(color.r) / 255.0,
        f32::from(color.g) / 255.0,
        f32::from(color.b) / 255.0,
    );
}

fn set_rgb_fill(content: &mut Content, color: Rgb) {
    content.set_fill_rgb(
        f32::from(color.r) / 255.0,
        f32::from(color.g) / 255.0,
        f32::from(color.b) / 255.0,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plot::{PlotColorMode, PlotStroke, dash_pattern_mm, plot_page};
    use mcad_core::{
        Command, DimAnnotation, DimDiameter, Document, Entity, EntityGeom, Linetype, Style,
    };
    use mcad_geom::Point2;

    fn page(width_mm: f64, height_mm: f64, paths: Vec<PlotPath>) -> PlotPage {
        PlotPage {
            width_mm,
            height_mm,
            background: Rgb::WHITE,
            paths,
        }
    }

    /// バイト列から最初（かつ唯一）のコンテンツストリームの中身を文字列で切り出す。
    fn content_stream(bytes: &[u8]) -> String {
        let text = String::from_utf8_lossy(bytes);
        let start = text
            .find("stream\n")
            .expect("stream キーワードが見つからない")
            + "stream\n".len();
        let end = text[start..]
            .find("\nendstream")
            .expect("endstream キーワードが見つからない")
            + start;
        text[start..end].to_string()
    }

    /// 空白区切りのトークン。数値は f64 へパース済み、それ以外（演算子・`[`・`]`）は
    /// 文字列のまま持つ簡易トークナイザ。
    #[derive(Debug, Clone, PartialEq)]
    enum Token {
        Num(f64),
        Other(String),
    }

    fn tokenize(s: &str) -> Vec<Token> {
        s.split_whitespace()
            .flat_map(|raw| {
                // `[3 1.5]` のように角括弧がトークンへくっつくことがあるので分離する。
                let mut parts = Vec::new();
                let mut cur = String::new();
                for c in raw.chars() {
                    if c == '[' || c == ']' {
                        if !cur.is_empty() {
                            parts.push(std::mem::take(&mut cur));
                        }
                        parts.push(c.to_string());
                    } else {
                        cur.push(c);
                    }
                }
                if !cur.is_empty() {
                    parts.push(cur);
                }
                parts
            })
            .map(|t| match t.parse::<f64>() {
                Ok(v) => Token::Num(v),
                Err(_) => Token::Other(t),
            })
            .collect()
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-3
    }

    fn assert_approx_num(tok: &Token, expected: f64, what: &str) {
        match tok {
            Token::Num(v) => assert!(approx(*v, expected), "{what}: got {v}, want {expected}"),
            other => panic!("{what}: 数値ではない: {other:?}"),
        }
    }

    /// 演算子 `op` の出現回数。
    fn operator_count(tokens: &[Token], op: &str) -> usize {
        tokens
            .iter()
            .filter(|t| *t == &Token::Other(op.to_string()))
            .count()
    }

    // ---- 1: MediaBox ----

    #[test]
    fn media_box_is_paper_size_scaled_by_mm_to_pt() {
        let p = page(297.0, 210.0, vec![]);
        let bytes = to_pdf(&p);
        let text = String::from_utf8_lossy(&bytes);
        let mb_start = text.find("/MediaBox").expect("MediaBox が無い");
        let mb_end = text[mb_start..].find(']').unwrap() + mb_start;
        let mb = &text[mb_start..=mb_end];
        let nums: Vec<f64> = mb
            .chars()
            .filter(|c| c.is_ascii_digit() || *c == '.' || c.is_whitespace() || *c == '-')
            .collect::<String>()
            .split_whitespace()
            .filter_map(|s| s.parse::<f64>().ok())
            .collect();
        assert_eq!(nums.len(), 4);
        assert!(approx(nums[0], 0.0));
        assert!(approx(nums[1], 0.0));
        assert!(approx(nums[2], 297.0 * MM_TO_PT));
        assert!(approx(nums[3], 210.0 * MM_TO_PT));
    }

    // ---- 2: cm + miter limit のヘッダ ----

    #[test]
    fn content_stream_starts_with_cm_and_miter_limit() {
        let p = page(100.0, 100.0, vec![]);
        let bytes = to_pdf(&p);
        let stream = content_stream(&bytes);
        let tokens = tokenize(&stream);

        let k = MM_TO_PT;
        // "K 0 0 K 0 0 cm"
        assert_approx_num(&tokens[0], k, "K");
        assert_eq!(tokens[1], Token::Num(0.0));
        assert_eq!(tokens[2], Token::Num(0.0));
        assert_approx_num(&tokens[3], k, "K (2つ目)");
        assert_eq!(tokens[4], Token::Num(0.0));
        assert_eq!(tokens[5], Token::Num(0.0));
        assert_eq!(tokens[6], Token::Other("cm".to_string()));

        // "4 M"
        assert_eq!(tokens[7], Token::Num(4.0));
        assert_eq!(tokens[8], Token::Other("M".to_string()));

        assert_eq!(
            tokens
                .iter()
                .filter(|t| *t == &Token::Other("cm".to_string()))
                .count(),
            1,
            "cm はコンテンツ先頭に1回だけ"
        );
    }

    // ---- 3: 座標はそのまま（y 反転なし） ----

    #[test]
    fn paper_point_is_not_y_flipped_in_pdf_output() {
        let stroke = PlotStroke {
            width_mm: 0.35,
            color: Rgb::BLACK,
            dash_mm: None,
        };
        let path = PlotPath::stroked(
            vec![
                PathCmd::MoveTo(Point2::new(10.0, 10.0)),
                PathCmd::LineTo(Point2::new(20.0, 10.0)),
            ],
            stroke,
        );
        let p = page(297.0, 210.0, vec![path]);
        let bytes = to_pdf(&p);
        let stream = content_stream(&bytes);
        let tokens = tokenize(&stream);

        let has_move = tokens.windows(3).any(|w| {
            w[0] == Token::Num(10.0)
                && w[1] == Token::Num(10.0)
                && w[2] == Token::Other("m".to_string())
        });
        let has_line = tokens.windows(3).any(|w| {
            w[0] == Token::Num(20.0)
                && w[1] == Token::Num(10.0)
                && w[2] == Token::Other("l".to_string())
        });
        assert!(has_move, "10 10 m が無い: {stream}");
        assert!(has_line, "20 10 l が無い: {stream}");
    }

    // ---- 4: stroke のみ / fill のみ ----

    #[test]
    fn stroke_only_emits_s_and_no_f_fill_only_emits_f_and_no_w_rg() {
        let stroke = PlotStroke {
            width_mm: 0.5,
            color: Rgb::BLACK,
            dash_mm: None,
        };
        let stroked = PlotPath::stroked(
            vec![
                PathCmd::MoveTo(Point2::new(0.0, 0.0)),
                PathCmd::LineTo(Point2::new(1.0, 0.0)),
            ],
            stroke,
        );
        let bytes = to_pdf(&page(100.0, 100.0, vec![stroked]));
        let stream = content_stream(&bytes);
        let tokens = tokenize(&stream);
        assert!(tokens.contains(&Token::Other("S".to_string())));
        assert!(!tokens.contains(&Token::Other("f".to_string())));

        let filled = PlotPath::filled(
            vec![
                PathCmd::MoveTo(Point2::new(0.0, 0.0)),
                PathCmd::LineTo(Point2::new(1.0, 0.0)),
                PathCmd::LineTo(Point2::new(1.0, 1.0)),
                PathCmd::Close,
            ],
            Rgb::BLACK,
        );
        let bytes = to_pdf(&page(100.0, 100.0, vec![filled]));
        let stream = content_stream(&bytes);
        let tokens = tokenize(&stream);
        assert!(tokens.contains(&Token::Other("f".to_string())));
        assert!(!tokens.contains(&Token::Other("w".to_string())));
        assert!(!tokens.contains(&Token::Other("RG".to_string())));
    }

    // ---- 5: stroke + fill 両方 ----

    #[test]
    fn stroke_and_fill_emit_exactly_one_b_operator_and_both_colors() {
        let both = PlotPath {
            cmds: vec![
                PathCmd::MoveTo(Point2::new(0.0, 0.0)),
                PathCmd::LineTo(Point2::new(1.0, 0.0)),
                PathCmd::LineTo(Point2::new(1.0, 1.0)),
                PathCmd::Close,
            ],
            stroke: Some(PlotStroke {
                width_mm: 0.5,
                color: Rgb::BLACK,
                dash_mm: None,
            }),
            fill: Some(Rgb::new(255, 0, 0)),
        };
        let bytes = to_pdf(&page(100.0, 100.0, vec![both]));
        let stream = content_stream(&bytes);
        let tokens = tokenize(&stream);

        assert_eq!(
            tokens
                .iter()
                .filter(|t| *t == &Token::Other("B".to_string()))
                .count(),
            1
        );
        assert!(!tokens.contains(&Token::Other("S".to_string())));
        assert!(!tokens.contains(&Token::Other("f".to_string())));
        assert!(tokens.contains(&Token::Other("rg".to_string())));
        assert!(tokens.contains(&Token::Other("RG".to_string())));
    }

    // ---- 6: stroke・fill 両方 None ----

    #[test]
    fn path_without_stroke_or_fill_writes_nothing() {
        let empty = PlotPath {
            cmds: vec![
                PathCmd::MoveTo(Point2::new(0.0, 0.0)),
                PathCmd::LineTo(Point2::new(1.0, 0.0)),
            ],
            stroke: None,
            fill: None,
        };
        let with_path = page(100.0, 100.0, vec![empty]);
        let without_path = page(100.0, 100.0, vec![]);
        let stream_with = content_stream(&to_pdf(&with_path));
        let stream_without = content_stream(&to_pdf(&without_path));
        assert_eq!(stream_with, stream_without);
        assert!(!stream_with.contains(" m"));
        assert!(!stream_with.contains(" l"));
    }

    // ---- 7: 破線の状態リークが起きない ----

    #[test]
    fn dash_pattern_does_not_leak_into_following_path() {
        let dashed = PlotPath::stroked(
            vec![
                PathCmd::MoveTo(Point2::new(0.0, 0.0)),
                PathCmd::LineTo(Point2::new(1.0, 0.0)),
            ],
            PlotStroke {
                width_mm: 0.35,
                color: Rgb::BLACK,
                dash_mm: dash_pattern_mm(Linetype::Dashed),
            },
        );
        let plain = PlotPath::stroked(
            vec![
                PathCmd::MoveTo(Point2::new(2.0, 0.0)),
                PathCmd::LineTo(Point2::new(3.0, 0.0)),
            ],
            PlotStroke {
                width_mm: 0.35,
                color: Rgb::BLACK,
                dash_mm: dash_pattern_mm(Linetype::Continuous),
            },
        );
        let bytes = to_pdf(&page(100.0, 100.0, vec![dashed, plain]));
        let stream = content_stream(&bytes);
        let tokens = tokenize(&stream);

        let q_count = tokens
            .iter()
            .filter(|t| *t == &Token::Other("q".to_string()))
            .count();
        let big_q_count = tokens
            .iter()
            .filter(|t| *t == &Token::Other("Q".to_string()))
            .count();
        // q/Q はそれぞれのパスで自己完結（2 パス分 = 2 組）。
        assert_eq!(
            q_count, big_q_count,
            "q と Q の出現数が一致しない: {stream}"
        );
        assert_eq!(q_count, 2, "q はパスごとに1回、計2回のはず: {stream}");

        // トークン列を q..Q ブロックへ分割し、1 番目には d が、2 番目には d が無いことを
        // 確かめる（状態リーク回帰テスト）。
        let q_positions: Vec<usize> = tokens
            .iter()
            .enumerate()
            .filter(|(_, t)| *t == &Token::Other("q".to_string()))
            .map(|(i, _)| i)
            .collect();
        let big_q_positions: Vec<usize> = tokens
            .iter()
            .enumerate()
            .filter(|(_, t)| *t == &Token::Other("Q".to_string()))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(q_positions.len(), 2);
        assert_eq!(big_q_positions.len(), 2);

        let first_block = &tokens[q_positions[0]..=big_q_positions[0]];
        let second_block = &tokens[q_positions[1]..=big_q_positions[1]];
        assert!(
            first_block.contains(&Token::Other("d".to_string())),
            "1番目のブロックに d が無い: {first_block:?}"
        );
        assert!(
            !second_block.contains(&Token::Other("d".to_string())),
            "破線設定が後続パスへリークしている: {second_block:?}"
        );
    }

    // ---- 8: Close ----

    #[test]
    fn close_becomes_h_not_a_duplicate_line_to_start() {
        let path = PlotPath::filled(
            vec![
                PathCmd::MoveTo(Point2::new(0.0, 0.0)),
                PathCmd::LineTo(Point2::new(10.0, 0.0)),
                PathCmd::LineTo(Point2::new(10.0, 5.0)),
                PathCmd::Close,
            ],
            Rgb::BLACK,
        );
        let bytes = to_pdf(&page(100.0, 100.0, vec![path]));
        let stream = content_stream(&bytes);
        let tokens = tokenize(&stream);
        assert!(tokens.contains(&Token::Other("h".to_string())));
        // 始点 (0,0) への `l` 複製が無いことを確認する: `h` の直前が
        // "0 0 l"（先頭点への line_to）になっていないことをトークン列で確かめる。
        let h_idx = tokens
            .iter()
            .position(|t| t == &Token::Other("h".to_string()))
            .unwrap();
        let is_duplicate_line_to_start = tokens[h_idx - 1] == Token::Other("l".to_string())
            && tokens[h_idx - 3] == Token::Num(0.0)
            && tokens[h_idx - 2] == Token::Num(0.0);
        assert!(
            !is_duplicate_line_to_start,
            "Close の直前に先頭点への line_to が複製されている: {stream}"
        );
    }

    // ---- 9: 線幅・色 ----

    #[test]
    fn stroke_width_and_color_operands_are_written() {
        let stroke = PlotStroke {
            width_mm: 0.5,
            color: Rgb::new(200, 30, 40),
            dash_mm: None,
        };
        let path = PlotPath::stroked(
            vec![
                PathCmd::MoveTo(Point2::new(0.0, 0.0)),
                PathCmd::LineTo(Point2::new(1.0, 0.0)),
            ],
            stroke,
        );
        let bytes = to_pdf(&page(210.0, 297.0, vec![path]));
        let stream = content_stream(&bytes);
        let tokens = tokenize(&stream);

        let w_idx = tokens
            .iter()
            .position(|t| t == &Token::Other("w".to_string()))
            .unwrap();
        let Token::Num(w) = tokens[w_idx - 1] else {
            panic!("w の直前が数値でない")
        };
        assert!(approx(w, 0.5));

        let rg_idx = tokens
            .iter()
            .position(|t| t == &Token::Other("RG".to_string()))
            .unwrap();
        let (Token::Num(r), Token::Num(g), Token::Num(b)) = (
            tokens[rg_idx - 3].clone(),
            tokens[rg_idx - 2].clone(),
            tokens[rg_idx - 1].clone(),
        ) else {
            panic!("RG の直前3つが数値でない")
        };
        assert!(approx(r, 200.0 / 255.0));
        assert!(approx(g, 30.0 / 255.0));
        assert!(approx(b, 40.0 / 255.0));
    }

    // ---- 9b: 寸法の記号ストローク・下線が PDF まで届く（M9 タスク49-3）----

    /// 寸法補助記号（φ）のストロークと非比例寸法の下線が、コンテンツストリームへ
    /// 自分の `q`…`Q` ブロックとして書き出される。
    ///
    /// IR の内訳（どのパスが何か）は `crate::plot::tests` 側が固定しているので、ここでは
    /// **IR のパスが 1 本残らずブロックになっていること**と、記号ストロークの実座標が
    /// 実際に `m` 演算子として現れることを見る。
    #[test]
    fn dimension_symbol_strokes_and_underline_reach_the_pdf() {
        // 直径寸法（φ 記号つき）に値上書きを載せ、非比例寸法の下線も同時に出す。
        // 既定文書は A4 横・1:1・枠なしなので、パスは寸法 1 つ分だけになる。
        let mut document = Document::new();
        let layer = document.current_layer();
        document
            .apply(Command::AddEntity(Entity::new(
                EntityGeom::DimDiameter(DimDiameter {
                    center: Point2::new(100.0, 60.0),
                    radius: 20.0,
                    angle: 0.0,
                    annotation: DimAnnotation {
                        value_override: Some("40".to_owned()),
                        ..DimAnnotation::default()
                    },
                }),
                layer,
                Style::inherited(),
            )))
            .unwrap();

        let page = plot_page(&document, PlotColorMode::Monochrome);
        let bytes = to_pdf(&page);
        let stream = content_stream(&bytes);
        let tokens = tokenize(&stream);

        // 背景は白なので背景 rect は出ない → q の数がそのままパス数。
        assert_eq!(operator_count(&tokens, "q"), page.paths.len());
        assert_eq!(operator_count(&tokens, "Q"), page.paths.len());
        // ストローク 4 本（寸法線・下線・φ の円・φ の斜線）+ 塗り 3 つ（矢先 2・値 1）。
        assert_eq!(operator_count(&tokens, "S"), 4, "{stream}");
        assert_eq!(operator_count(&tokens, "f"), 3, "{stream}");
        assert_eq!(operator_count(&tokens, "d"), 0, "寸法は常に実線");

        // φ の円（3 次ベジエ 4 本 + `h`）の始点が `m` として書かれている（y 反転なし）。
        let circle = page
            .paths
            .iter()
            .find(|p| p.stroke.is_some() && p.cmds.len() == 6)
            .expect("φ の円");
        let PathCmd::MoveTo(start) = circle.cmds[0] else {
            unreachable!()
        };
        let has_move = tokens.windows(3).any(|w| match (&w[0], &w[1], &w[2]) {
            (Token::Num(x), Token::Num(y), Token::Other(op)) => {
                op == "m" && approx(*x, start.x) && approx(*y, start.y)
            }
            _ => false,
        });
        assert!(has_move, "φ の円の始点が PDF に無い: {stream}");
    }

    // ---- 10: 決定性 ----

    #[test]
    fn same_input_produces_byte_identical_output() {
        let stroke = PlotStroke {
            width_mm: 0.35,
            color: Rgb::BLACK,
            dash_mm: dash_pattern_mm(Linetype::DashDot),
        };
        let path = PlotPath::stroked(
            vec![
                PathCmd::MoveTo(Point2::new(1.234_567, 2.0)),
                PathCmd::CurveTo(
                    Point2::new(3.0, 4.0),
                    Point2::new(5.0, 6.0),
                    Point2::new(7.0, 8.0),
                ),
                PathCmd::Close,
            ],
            stroke,
        );
        let p = page(297.0, 210.0, vec![path]);
        assert_eq!(to_pdf(&p), to_pdf(&p));
    }

    // ---- 11: 空ページでも妥当な PDF ----

    #[test]
    fn empty_page_produces_valid_single_page_pdf() {
        let p = page(210.0, 297.0, vec![]);
        let bytes = to_pdf(&p);
        assert!(bytes.starts_with(b"%PDF-"));
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("/Type /Catalog"));
        assert!(text.contains("/Type /Pages"));
        assert!(text.contains("/Type /Page"));
        assert_eq!(
            text.matches("/Type /Page\n").count()
                + text.matches("/Type /Page/").count()
                + text.matches("/Type /Page>>").count()
                + text.matches("/Type /Page ").count(),
            1
        );
    }

    // ---- 12: 背景は PlotPage::background に従う（白では出さない・非白では全面塗り） ----

    #[test]
    fn white_background_emits_no_fill_rect_and_matches_prior_byte_output() {
        // 既存の非破壊確認: 背景が白（既定）のときは背景塗りの `re`/`f` が一切
        // 現れない（従来の PDF 出力は背景を描いていなかったため、既存出力の
        // バイト列を変えない）。
        let p = page(210.0, 297.0, vec![]);
        let stream = content_stream(&to_pdf(&p));
        let tokens = tokenize(&stream);
        assert!(!tokens.contains(&Token::Other("re".to_string())));
        assert!(!tokens.contains(&Token::Other("f".to_string())));
        assert!(!tokens.contains(&Token::Other("q".to_string())));
    }

    #[test]
    fn non_white_background_is_drawn_as_a_self_contained_full_page_rect_first() {
        let bg = Rgb::new(0x00, 0x31, 0x53);
        let mut p = page(210.0, 297.0, vec![]);
        p.background = bg;
        let bytes = to_pdf(&p);
        let stream = content_stream(&bytes);
        let tokens = tokenize(&stream);

        // 背景塗り用の rg（塗り色）と re（矩形）と f（塗り）が出る。
        assert!(tokens.contains(&Token::Other("rg".to_string())));
        assert!(tokens.contains(&Token::Other("re".to_string())));
        assert!(tokens.contains(&Token::Other("f".to_string())));

        // q/Q で自己完結している（背景用の 1 組）。
        let q_count = tokens
            .iter()
            .filter(|t| *t == &Token::Other("q".to_string()))
            .count();
        let big_q_count = tokens
            .iter()
            .filter(|t| *t == &Token::Other("Q".to_string()))
            .count();
        assert_eq!(q_count, 1);
        assert_eq!(big_q_count, 1);

        // 背景の rg は指定色そのもの。
        let rg_idx = tokens
            .iter()
            .position(|t| t == &Token::Other("rg".to_string()))
            .unwrap();
        let (Token::Num(r), Token::Num(g), Token::Num(b)) = (
            tokens[rg_idx - 3].clone(),
            tokens[rg_idx - 2].clone(),
            tokens[rg_idx - 1].clone(),
        ) else {
            panic!("rg の直前3つが数値でない")
        };
        assert!(approx(r, f64::from(bg.r) / 255.0));
        assert!(approx(g, f64::from(bg.g) / 255.0));
        assert!(approx(b, f64::from(bg.b) / 255.0));

        // re は用紙全面（0 0 W H）。
        let re_idx = tokens
            .iter()
            .position(|t| t == &Token::Other("re".to_string()))
            .unwrap();
        assert_eq!(tokens[re_idx - 4], Token::Num(0.0));
        assert_eq!(tokens[re_idx - 3], Token::Num(0.0));
        assert_approx_num(&tokens[re_idx - 2], 210.0, "背景幅");
        assert_approx_num(&tokens[re_idx - 1], 297.0, "背景高さ");

        // 背景が先頭（cm/M ヘッダの直後、最初の q より前に他の描画演算子が無い）。
        let first_q = tokens
            .iter()
            .position(|t| t == &Token::Other("q".to_string()))
            .unwrap();
        assert!(
            !tokens[..first_q]
                .iter()
                .any(|t| t == &Token::Other("m".to_string())),
            "背景より前にパス描画があってはいけない"
        );
    }

    #[test]
    fn background_rect_does_not_leak_fill_color_into_following_path() {
        let bg = Rgb::new(0x00, 0x31, 0x53);
        let mut p = page(
            100.0,
            100.0,
            vec![PlotPath::stroked(
                vec![
                    PathCmd::MoveTo(Point2::new(0.0, 0.0)),
                    PathCmd::LineTo(Point2::new(1.0, 0.0)),
                ],
                PlotStroke {
                    width_mm: 0.35,
                    color: Rgb::BLACK,
                    dash_mm: None,
                },
            )],
        );
        p.background = bg;
        let bytes = to_pdf(&p);
        let stream = content_stream(&bytes);
        let tokens = tokenize(&stream);

        // q/Q が背景用 + パス用の 2 組になる。
        let q_count = tokens
            .iter()
            .filter(|t| *t == &Token::Other("q".to_string()))
            .count();
        assert_eq!(q_count, 2);

        // 2 番目のブロック（作図パス）は stroke のみで fill 演算子を持たない。
        let q_positions: Vec<usize> = tokens
            .iter()
            .enumerate()
            .filter(|(_, t)| *t == &Token::Other("q".to_string()))
            .map(|(i, _)| i)
            .collect();
        let big_q_positions: Vec<usize> = tokens
            .iter()
            .enumerate()
            .filter(|(_, t)| *t == &Token::Other("Q".to_string()))
            .map(|(i, _)| i)
            .collect();
        let second_block = &tokens[q_positions[1]..=big_q_positions[1]];
        assert!(!second_block.contains(&Token::Other("f".to_string())));
        assert!(second_block.contains(&Token::Other("S".to_string())));
    }
}
