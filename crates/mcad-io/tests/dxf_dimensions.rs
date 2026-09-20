//! 他 CAD が書いた DXF の DIMENSION を読む（M11 タスク67）。
//!
//! # fixture
//!
//! `tests/fixtures/synthetic_dimensions.dxf` は**合成データ**。リポジトリは GitHub で
//! 公開されているため、ユーザーが実際に LibreCAD で描いた図面はそのまま同梱できない
//! （座標を合成値へ差し替えた経緯は同ディレクトリの `README.md` を参照）。座標だけを
//! 単純な値へ差し替え、group code の並び・subclass marker・anonymous block の構造は
//! LibreCAD（libdxfrw 0.6.3）が書いた実ファイルのまま踏襲している。
//!
//! `ENTITIES` セクションは `LINE` 4 件・`CIRCLE` 1 件・`DIMENSION` 6 件を持つ。
//! 寸法 6 件の内訳は次のとおり（group 70 の整数部が種別）。
//!
//! | ブロック参照 | group 70 | 種別 | 計測対象 | 本テストでの期待 |
//! |---|---|---|---|---|
//! | `*D1` | 33 | 整列 | 斜辺 `(0,0)`–`(120,30)` | `DimLinear`（`Aligned`、値 = 実距離 123.693…） |
//! | `*D2` | 32 | 回転（水平、group 50 省略 = 0） | 同じ斜辺 | `DimLinear`（`Rotated(0)`、値 = 水平投影 **120**） |
//! | `*D3` | 32 | 回転（鉛直、group 50 = 90） | 鉛直線 `(150,0)`–`(150,40)` | `DimLinear`（`Rotated(π/2)`、値 = 40） |
//! | `*D4` | 36 | 半径 | 円（中心 `(60,80)`、半径 `25`） | `DimRadial` |
//! | `*D5` | 35 | 直径 | 円（同上、直径両端 `(35,80)`–`(85,80)`） | `DimDiameter` |
//! | `*D6` | 34 | 2 直線角度 | 頂点 `(0,50)` から 2 本の線 | **エンティティとして現れない**（`dxf` 0.6.1 が読まない） |
//!
//! 判定の根拠は `crates/mcad-io/src/dxf_file.rs` の `linear_dimension_to_geom` ほかの
//! doc とモジュール doc「DIMENSION の import」を参照。
//!
//! # 座標は合成値・きれいな数で選んである
//!
//! 半径 25・直径 50・鉛直寸法の値 40 など、期待値は本ファイル内で図形の定義から
//! 計算し直したものであり、fixture の実ファイルから転記した値ではない。
//!
//! # 見た目のブロックは読まない
//!
//! LibreCAD は寸法の見た目（寸法線・矢先・文字）を anonymous block `*D1`〜`*D6` として
//! `BLOCKS` セクションへ書く。この fixture では各ブロックの中身を `LINE` 1〜2 本まで
//! 簡略化しているが、mcad は `BLOCKS` を読まないので、DIMENSION を取り込んでも
//! **線が二重に入らない**ことに変わりはない。取り込み後のエンティティ数がちょうど
//! 10 件（`LINE` 4 + `CIRCLE` 1 + 寸法 5）であることがその確認になっている。
//!
//! # M11 タスク68 で `*D2` の期待値が変わった
//!
//! タスク67 の時点では `mcad_core::DimLinear` が寸法線の向きを持てず、`*D2`（斜辺に
//! 付けた水平寸法）を取り込むと表示値が投影長 120 から実距離 123.693… へ**黙って
//! 変わって**しまうため、`skipped_dimensions` へ計上してスキップしていた。
//! タスク68 で `mcad_core::DimDirection` が入り `.mcad` v7 がこれを保存できるように
//! なったので、いまは `Rotated(0)` として無損失に取り込む（`skipped_dimensions` は 0）。

use std::f64::consts::FRAC_PI_2;
use std::path::PathBuf;

use mcad_core::{DimAnnotation, DimDirection, EntityGeom};
use mcad_geom::{Circle, Point2, Shape};
use mcad_io::{ImportSummary, load_dxf};

/// 座標・長さの比較許容。
const EPS: f64 = 1e-9;

fn fixture_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests/fixtures/synthetic_dimensions.dxf");
    path
}

fn import() -> ImportSummary {
    load_dxf(fixture_path()).expect("fixture を読めない")
}

/// 生存エンティティの [`EntityGeom`] を集める（`Document::entities` の順序は未規定
/// なので、本テストは順序に依存した検証をしない）。
fn geoms(summary: &ImportSummary) -> Vec<EntityGeom> {
    summary
        .document
        .entities()
        .map(|(_, e)| e.geom.clone())
        .collect()
}

/// fixture 内の唯一の `CIRCLE`。半径・直径寸法の期待値の出所（中心 `(60,80)`・
/// 半径 `25`）。
fn circle(summary: &ImportSummary) -> Circle {
    geoms(summary)
        .into_iter()
        .find_map(|g| match g {
            EntityGeom::Shape(Shape::Circle(c)) => Some(c),
            _ => None,
        })
        .expect("fixture に CIRCLE がある")
}

fn assert_close(actual: f64, expected: f64, what: &str) {
    assert!(
        (actual - expected).abs() <= EPS,
        "{what}: {actual} != {expected}"
    );
}

fn assert_point_close(actual: Point2, expected: Point2, what: &str) {
    assert_close(actual.x, expected.x, &format!("{what}.x"));
    assert_close(actual.y, expected.y, &format!("{what}.y"));
}

/// 取り込み全体の内訳。`LINE` 4 + `CIRCLE` 1 + 寸法 5 = 10 件で、**スキップは 0 件**
/// （M11 タスク68 で `*D2` も向きごと取り込めるようになった）。
///
/// 2 直線角度寸法 `*D6` は `dxf` 0.6.1 がそもそもエンティティとして返さないため
/// **スキップ件数にも出ない**（M10 タスク61 で判明した `ACAD_TABLE` と同じ制約。
/// モジュール doc 参照）。ここで取り込んだエンティティ数が 10 件（11 件ではない）
/// であることを固定することが、その「読めない種別は通知できない」制約の回帰
/// テストにもなっている。
#[test]
fn synthetic_dimensions_fixture_imports_all_five_readable_dimensions() {
    let summary = import();
    let geoms = geoms(&summary);
    assert_eq!(geoms.len(), 10, "取り込んだエンティティ数");
    assert_eq!(summary.skipped_entities, 0, "未対応の種別は無い");
    assert_eq!(
        summary.skipped_dimensions, 0,
        "タスク68 以降、fixture の寸法はすべて表現できる"
    );
    assert_eq!(summary.dropped_dimension_details, 0, "捨てた寸法属性");

    let mut kinds: Vec<&str> = geoms
        .iter()
        .map(|g| match g {
            EntityGeom::Shape(Shape::Line(_)) => "line",
            EntityGeom::Shape(Shape::Circle(_)) => "circle",
            EntityGeom::DimLinear(_) => "linear",
            EntityGeom::DimRadial(_) => "radial",
            EntityGeom::DimDiameter(_) => "diameter",
            other => panic!("想定外のジオメトリ: {other:?}"),
        })
        .collect();
    kinds.sort_unstable();
    assert_eq!(
        kinds,
        vec![
            "circle", "diameter", "line", "line", "line", "line", "linear", "linear", "linear",
            "radial",
        ],
        "取り込んだ種別の内訳"
    );
}

/// 整列寸法 `*D1`: group 13/14 が計測 2 点そのもので、group 10（寸法線の位置）から
/// 求めた `offset` が寸法線をその位置へ戻す。
///
/// 合成データの生の値は 13 = `(0, 0)`・14 = `(120, 30)`・
/// 10 = `(56.361965624455, 29.552137502179978)`（計測線の中点から法線方向へ
/// ちょうど 15 離した点になるよう作ってある）。
#[test]
fn aligned_dimension_keeps_its_two_measured_points_and_dimension_line_position() {
    let summary = import();
    let dim = geoms(&summary)
        .into_iter()
        .find_map(|g| match g {
            EntityGeom::DimLinear(d) if d.direction == DimDirection::Aligned => Some(d),
            _ => None,
        })
        .expect("整列寸法 *D1 がある");

    assert_point_close(dim.p1, Point2::new(0.0, 0.0), "p1（group 13）");
    assert_point_close(dim.p2, Point2::new(120.0, 30.0), "p2（group 14）");
    // 計測長は 2 点間の実距離（整列寸法なので投影しない）。
    assert_close(dim.measured_value(), 123.693_168_768_529_82, "計測長");
    // offset は (10 − p1)・perp((p2−p1)/|p2−p1|)。dimension line position は中点から
    // 法線方向へちょうど 15 離した点になるよう作ってあるので offset = 15。
    assert_close(dim.offset, 15.0, "offset");
    // offset を戻すと group 10 の位置が寸法線上に乗る（mcad-core の linear_frame と
    // 同じ式で寸法線端を作り、そこから group 10 までの距離が 0 になることで確かめる）。
    let dir = (dim.p2 - dim.p1).normalize().expect("計測 2 点は異なる");
    let d1 = dim.p1 + dir.perp() * dim.offset;
    let dim_line_point = Point2::new(56.361_965_624_455, 29.552_137_502_179_978);
    assert_close(
        (dim_line_point - d1).cross(dir).abs(),
        0.0,
        "group 10 が寸法線上に乗る",
    );
    assert_eq!(dim.annotation, DimAnnotation::unannotated(), "注記なし");
}

/// 鉛直な回転寸法 `*D3`（group 50 = 90）: 向きは `Rotated(π/2)` として保存され、
/// 度→ラジアンの変換が io 境界で行われる。計測 2 点が鉛直に並んでいるので投影長と
/// 実距離は一致する。
///
/// 合成データの生の値は 13 = `(150, 0)`・14 = `(150, 40)`・10 = `(130, 20)`
/// （鉛直線から左へちょうど 20 離した点になるよう作ってある）。
#[test]
fn vertical_rotated_dimension_keeps_its_ninety_degree_angle() {
    let summary = import();
    let dim = geoms(&summary)
        .into_iter()
        .find_map(|g| match g {
            EntityGeom::DimLinear(d) if d.p1.x == 150.0 => Some(d),
            _ => None,
        })
        .expect("回転寸法 *D3 がある");

    assert_point_close(dim.p1, Point2::new(150.0, 0.0), "p1（group 13）");
    assert_point_close(dim.p2, Point2::new(150.0, 40.0), "p2（group 14）");
    let DimDirection::Rotated(theta) = dim.direction else {
        panic!("回転寸法として取り込まれていない: {:?}", dim.direction);
    };
    assert_close(theta, FRAC_PI_2, "group 50 = 90 度 → π/2 ラジアン");
    // 2 点は鉛直に並んでいる（x が同じ）ので、group 50 = 90 への投影長 = 実距離。
    assert_close(dim.p1.x, dim.p2.x, "計測 2 点は鉛直");
    assert_close(dim.measured_value(), 40.0, "計測長");
    // dir = (0,1)、perp(dir) = (-1,0) なので group 10 =(130,20) は offset = 20。
    assert_close(dim.offset, 20.0, "offset");
    assert_eq!(dim.annotation, DimAnnotation::unannotated(), "注記なし");
}

/// 水平な回転寸法 `*D2` は**投影長のまま**取り込まれる（M11 タスク68）。
///
/// 計測 2 点は `*D1` と同じ斜辺 `(0, 0)`–`(120, 30)` で、group 50 は省略（= 0、水平）。
/// DXF の寸法値は水平方向への投影 `120` で、実距離 `123.693168768529…` ではない。
/// 向きが [`mcad_core::DimDirection::Rotated`]`(0)` として保存されるので、mcad が
/// 表示する値も `120` になる。**この 1 件がタスク68 の眼目**で、タスク67 までは
/// 「値が黙って変わる」のを避けるためにスキップしていた。
#[test]
fn horizontal_rotated_dimension_over_a_slanted_edge_measures_the_projection() {
    let summary = import();
    let matching: Vec<_> = geoms(&summary)
        .into_iter()
        .filter_map(|g| match g {
            EntityGeom::DimLinear(d)
                if (d.p1 - Point2::new(0.0, 0.0)).length() <= EPS
                    && (d.p2 - Point2::new(120.0, 30.0)).length() <= EPS =>
            {
                Some(d)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        matching.len(),
        2,
        "同じ斜辺に *D1（整列）と *D2（回転・水平）の 2 件が付く: {matching:?}"
    );

    let rotated = matching
        .iter()
        .find(|d| d.direction != DimDirection::Aligned)
        .expect("*D2 が回転寸法として取り込まれている");
    assert_eq!(
        rotated.direction,
        DimDirection::Rotated(0.0),
        "group 50 省略 = 0"
    );
    // **実距離 123.693… ではなく水平投影の 120。**
    assert_close(rotated.measured_value(), 120.0, "計測長（水平投影）");
    // group 10 = (60, 0) は寸法線（y = 0 の水平線）の上にあり、dir = (1,0)・
    // perp(dir) = (0,1) なので offset = 0。
    assert_close(rotated.offset, 0.0, "offset");
    assert_eq!(rotated.annotation, DimAnnotation::unannotated(), "注記なし");
}

/// 半径寸法 `*D4`: group 10 = 中心・group 15 = 円周上の点。中心と半径が図面の
/// `CIRCLE`（中心 `(60,80)`・半径 `25`）と一致する。
#[test]
fn radial_dimension_matches_the_circle_it_measures() {
    let summary = import();
    let circle = circle(&summary);
    let dim = geoms(&summary)
        .into_iter()
        .find_map(|g| match g {
            EntityGeom::DimRadial(d) => Some(d),
            _ => None,
        })
        .expect("半径寸法 *D4 がある");

    assert_point_close(dim.center, circle.center, "中心（group 10）");
    assert_close(dim.radius, circle.radius, "半径 = |15 − 10|");
    assert_close(dim.radius, 25.0, "半径の実測値");
    // 引出線は中心から group 15 = (85, 80)（中心の真右）を向くので角度は 0。
    assert_close(dim.leader_angle, 0.0, "引出線角（ラジアン）");
    assert_eq!(dim.annotation, DimAnnotation::unannotated(), "注記なし");
}

/// 直径寸法 `*D5`: group 10 と group 15 が**直径の両端**。中点が円の中心、
/// 2 点間距離が直径（`50`）と一致する。
#[test]
fn diameter_dimension_spans_the_circle_it_measures() {
    let summary = import();
    let circle = circle(&summary);
    let dim = geoms(&summary)
        .into_iter()
        .find_map(|g| match g {
            EntityGeom::DimDiameter(d) => Some(d),
            _ => None,
        })
        .expect("直径寸法 *D5 がある");

    assert_point_close(dim.center, circle.center, "中心 =(10 + 15)/2");
    assert_close(dim.radius, circle.radius, "半径 = |15 − 10|/2");
    assert_close(dim.radius * 2.0, 50.0, "直径の実測値");
    // 直径の両端は (35, 80) と (85, 80)（x 軸に平行）なので角度は 0。
    assert_close(dim.angle, 0.0, "直径線の角（ラジアン）");
    let half = mcad_geom::Vec2::new(dim.angle.cos(), dim.angle.sin()) * dim.radius;
    assert_point_close(dim.center - half, Point2::new(35.0, 80.0), "group 10");
    assert_point_close(dim.center + half, Point2::new(85.0, 80.0), "group 15");
    assert_eq!(dim.annotation, DimAnnotation::unannotated(), "注記なし");
}
