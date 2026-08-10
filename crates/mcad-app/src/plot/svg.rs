//! SVG 直列化バックエンド（M8 タスク39-2、DESIGN.md M8 設計判断7）。
//!
//! [`PlotPage`]（紙 mm・原点=用紙左下・y-up、[`super`] モジュール doc 参照）を
//! SVG 文字列へ落とす。**y 反転はここで 1 箇所だけ行う**
//! （`y_svg = page.height_mm - y`）。依存クレートを増やさず手書き XML で組み立てる。
//!
//! egui には一切依存しない（`egui::` を import しない）。

use mcad_core::Rgb;
use mcad_geom::Point2;

use super::{PathCmd, PlotPage, PlotPath};

/// [`PlotPage`] を SVG 文字列へ直列化する。
///
/// - ルートは `width="{W}mm" height="{H}mm" viewBox="0 0 {W} {H}"`
///   （1 user unit = 1mm の実寸）。
/// - 用紙全面の白背景 rect を最初に置く（IR には含まれない。暗テーマのビューア対策 +
///   用紙の表現）。
/// - `paths` は奥→手前の順のまま書けば SVG の文書順（後勝ち）と一致する。
/// - 数値はすべて `{:.3}`（0.001mm 精度）で決定的に出力する。
#[must_use]
pub fn to_svg(page: &PlotPage) -> String {
    let w = page.width_mm;
    let h = page.height_mm;
    let mut out = String::new();

    out.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}mm\" height=\"{}mm\" \
         viewBox=\"0 0 {} {}\">\n",
        fmt_num(w),
        fmt_num(h),
        fmt_num(w),
        fmt_num(h)
    ));

    // 用紙全面の白背景（暗テーマのビューア対策 + 用紙の表現。PDF はページ自体が
    // 白なのでタスク40 では不要）。
    out.push_str(&format!(
        "<rect x=\"0\" y=\"0\" width=\"{}\" height=\"{}\" fill=\"#ffffff\"/>\n",
        fmt_num(w),
        fmt_num(h)
    ));

    for path in &page.paths {
        push_path(&mut out, path, h);
    }

    out.push_str("</svg>\n");
    out
}

/// 1 本の `PlotPath` を `<path>` 要素へ書く。
fn push_path(out: &mut String, path: &PlotPath, height_mm: f64) {
    let d = path_data(&path.cmds, height_mm);
    out.push_str("<path d=\"");
    out.push_str(&d);
    out.push('"');

    // `fill` 属性は必ず **ちょうど 1 回だけ** 書く。`fill` と `stroke` が両方
    // `Some` のパスで `fill="none"` と `fill="#..."` を続けて出すと**属性が重複した
    // 不正な XML** になり、厳格なパーサは読み込みに失敗する（現在の `plot_page` は
    // どちらか片方しか作らないが、IR の型としては両立するので構造として塞いでおく）。
    // 塗らないパスの `fill="none"` は必須（SVG の既定 fill は black なので、
    // 忘れると閉じた形状が黒く塗り潰される）。
    match path.fill {
        Some(fill) => out.push_str(&format!(" fill=\"{}\"", fmt_color(fill))),
        None => out.push_str(" fill=\"none\""),
    }
    if let Some(stroke) = path.stroke {
        out.push_str(&format!(" stroke=\"{}\"", fmt_color(stroke.color)));
        out.push_str(&format!(
            " stroke-width=\"{}\"",
            fmt_num(f64::from(stroke.width_mm))
        ));
        if let Some(dash) = stroke.dash_mm {
            let dasharray = dash
                .iter()
                .map(|v| fmt_num(f64::from(*v)))
                .collect::<Vec<_>>()
                .join(" ");
            out.push_str(&format!(" stroke-dasharray=\"{dasharray}\""));
        }
    }
    out.push_str("/>\n");
}

/// パスコマンド列を SVG パスデータ文字列へ変換する。y 反転はここで適用する。
fn path_data(cmds: &[PathCmd], height_mm: f64) -> String {
    let mut parts = Vec::with_capacity(cmds.len());
    for cmd in cmds {
        match cmd {
            PathCmd::MoveTo(p) => {
                parts.push(format!("M {}", fmt_point(*p, height_mm)));
            }
            PathCmd::LineTo(p) => {
                parts.push(format!("L {}", fmt_point(*p, height_mm)));
            }
            PathCmd::CurveTo(c1, c2, p) => {
                parts.push(format!(
                    "C {} {} {}",
                    fmt_point(*c1, height_mm),
                    fmt_point(*c2, height_mm),
                    fmt_point(*p, height_mm)
                ));
            }
            PathCmd::Close => {
                parts.push("Z".to_owned());
            }
        }
    }
    parts.join(" ")
}

/// 紙 mm 座標（y-up）を SVG 座標（y-down）へ反転して `"x y"` として書く。
fn fmt_point(p: Point2, height_mm: f64) -> String {
    format!("{},{}", fmt_num(p.x), fmt_num(height_mm - p.y))
}

/// 数値を 0.001mm 精度で決定的に書く。
fn fmt_num(v: f64) -> String {
    format!("{v:.3}")
}

/// 色を `#rrggbb` へ書く。
fn fmt_color(color: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plot::{PlotStroke, dash_pattern_mm};
    use mcad_core::Linetype;

    fn page(width_mm: f64, height_mm: f64, paths: Vec<PlotPath>) -> PlotPage {
        PlotPage {
            width_mm,
            height_mm,
            paths,
        }
    }

    // ---- 1: ルート要素の寸法・向き ----

    #[test]
    fn root_element_has_paper_size_landscape_and_portrait() {
        let landscape = page(297.0, 210.0, vec![]);
        let svg = to_svg(&landscape);
        assert!(svg.contains("width=\"297.000mm\" height=\"210.000mm\""));
        assert!(svg.contains("viewBox=\"0 0 297.000 210.000\""));

        let portrait = page(210.0, 297.0, vec![]);
        let svg = to_svg(&portrait);
        assert!(svg.contains("width=\"210.000mm\" height=\"297.000mm\""));
        assert!(svg.contains("viewBox=\"0 0 210.000 297.000\""));
    }

    // ---- 2: y 反転はここで 1 度だけ ----

    #[test]
    fn paper_point_is_y_flipped_in_svg_output() {
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
        let svg = to_svg(&p);
        // 紙 (10,10) は SVG では y = 210 - 10 = 200。
        assert!(svg.contains("M 10.000,200.000"));
        assert!(svg.contains("L 20.000,200.000"));
    }

    // ---- 3: fill="none" / stroke 属性なし ----

    #[test]
    fn stroke_paths_get_fill_none_and_fill_paths_omit_stroke_attrs() {
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
        let filled = PlotPath::filled(
            vec![
                PathCmd::MoveTo(Point2::new(0.0, 0.0)),
                PathCmd::LineTo(Point2::new(1.0, 0.0)),
                PathCmd::LineTo(Point2::new(1.0, 1.0)),
                PathCmd::Close,
            ],
            Rgb::BLACK,
        );
        let p = page(100.0, 100.0, vec![stroked, filled]);
        let svg = to_svg(&p);

        // stroke パスは fill="none"、fill パスは stroke 属性を持たない。
        let mut path_elems = svg.match_indices("<path").map(|(i, _)| i);
        let first = path_elems.next().unwrap();
        let second = path_elems.next().unwrap();
        let end = svg[second..].find("/>").map(|i| second + i).unwrap();
        let first_elem = &svg[first..second];
        let second_elem = &svg[second..end];
        assert!(first_elem.contains("fill=\"none\""));
        assert!(first_elem.contains("stroke=\""));
        assert!(!second_elem.contains("stroke=\""));
        assert!(second_elem.contains("fill=\"#000000\""));
    }

    /// `fill` と `stroke` が両方 `Some` のパスでも `fill` 属性は 1 回しか出ない。
    ///
    /// 属性が重複した XML は well-formed でなく、厳格なパーサが読み込みに失敗する。
    /// 現在の `plot_page` はこの組み合わせを作らないが、IR の型としては両立するため
    /// 直列化側で塞いであることを固定する。
    #[test]
    fn path_with_both_fill_and_stroke_emits_exactly_one_fill_attribute() {
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
        let svg = to_svg(&page(100.0, 100.0, vec![both]));
        let elem_start = svg.find("<path").unwrap();
        let elem = &svg[elem_start..svg[elem_start..].find("/>").unwrap() + elem_start];

        assert_eq!(
            elem.matches("fill=\"").count(),
            1,
            "fill 属性が重複している"
        );
        assert!(elem.contains("fill=\"#ff0000\""), "塗り色が優先される");
        assert!(elem.contains("stroke=\"#000000\""));
    }

    // ---- 4: dasharray ----

    #[test]
    fn dashed_stroke_emits_dasharray_continuous_omits_it() {
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
        let continuous = PlotPath::stroked(
            vec![
                PathCmd::MoveTo(Point2::new(0.0, 0.0)),
                PathCmd::LineTo(Point2::new(1.0, 0.0)),
            ],
            PlotStroke {
                width_mm: 0.35,
                color: Rgb::BLACK,
                dash_mm: dash_pattern_mm(Linetype::Continuous),
            },
        );
        let svg_dashed = to_svg(&page(100.0, 100.0, vec![dashed]));
        assert!(svg_dashed.contains("stroke-dasharray=\"3.000 1.500\""));

        let svg_continuous = to_svg(&page(100.0, 100.0, vec![continuous]));
        assert!(!svg_continuous.contains("stroke-dasharray"));
    }

    // ---- 5: 線幅・色・白背景 ----

    #[test]
    fn stroke_width_and_color_are_written_literally_and_white_background_exists() {
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
        let p = page(210.0, 297.0, vec![path]);
        let svg = to_svg(&p);
        assert!(svg.contains("stroke-width=\"0.500\""));
        assert!(svg.contains("stroke=\"#c81e28\""));
        assert!(svg.contains(
            "<rect x=\"0\" y=\"0\" width=\"210.000\" height=\"297.000\" fill=\"#ffffff\"/>"
        ));
    }

    // ---- 6: Close は Z ----

    #[test]
    fn close_becomes_z_not_a_duplicate_line_to_start() {
        let path = PlotPath::filled(
            vec![
                PathCmd::MoveTo(Point2::new(0.0, 0.0)),
                PathCmd::LineTo(Point2::new(10.0, 0.0)),
                PathCmd::LineTo(Point2::new(10.0, 5.0)),
                PathCmd::Close,
            ],
            Rgb::BLACK,
        );
        let svg = to_svg(&page(100.0, 100.0, vec![path]));
        assert!(svg.contains("Z"));
        // 先頭点 (0, 0) → SVG では y=100 なので "0.000,100.000" への `L` 展開が
        // 無いことを確認する（`Z` の直前が `L 10.000,95.000` のはず）。
        assert!(!svg.contains("L 0.000,100.000"));
    }

    // ---- 7: 決定性 ----

    #[test]
    fn same_input_produces_identical_output() {
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
        assert_eq!(to_svg(&p), to_svg(&p));
    }
}
