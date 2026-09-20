//! 寸法描画の SVG スナップショットテスト（DESIGN.md M11 タスク64、設計判断4）。
//!
//! # 目的
//!
//! M11-1 で寸法のデータモデルを広げる（`DimDirection`・`DimAngular`・`DimOrdinate`・
//! `DimAnnotation` の拡張）前に、**既存の寸法描画が変わっていないこと**を機械的に
//! 保証する土台を置く。対象は長さ／半径／直径の3寸法 × 矢先7種、注記の代表組合せ、
//! 尺度が効いた寸法で、いずれも [`crate::plot::plot_page`] → [`crate::plot::svg::to_svg`]
//! を通した SVG 文字列を fixture（`tests/snapshots/*.svg`）と突き合わせる。
//!
//! `mcad-app` はバイナリクレートで `lib.rs` を持たないため、`tests/` ディレクトリから
//! クレート内部（`plot` モジュール等）を呼べない。そのためテスト本体はこの
//! `#[cfg(test)]` モジュールに置く（`main.rs` から `mod dim_snapshot_tests;` で登録）。
//! fixture 自体は `tests/snapshots/` に置く（テストが実行時に読み込む場所であって、
//! テストコード自体の置き場所ではない）。
//!
//! # 運用
//!
//! このテストが落ちたら「寸法描画が（意図的にせよ、そうでないにせよ）変わった」こと
//! を意味する。意図した変更（M11-1 のモデル拡張・矢先の見直し等）なら、
//! `UPDATE_SNAPSHOTS=1 cargo test -p mcad-app dim_snapshot_tests -- --test-threads=1`
//! のように環境変数 `UPDATE_SNAPSHOTS=1` を付けて実行し fixture を上書きしたうえで、
//! `git diff` で SVG の差分をレビューすること。意図しない変更なら実装側を直す。
//!
//! fixture は **`include_str!` ではなく実行時に読み込む**（`CARGO_MANIFEST_DIR` 基準の
//! パスを `std::fs::read_to_string` する）。fixture を更新したときに再コンパイル無しで
//! テストが回るようにするため。

use std::path::{Path, PathBuf};

use mcad_core::{
    ArrowPlacement, Command, DimAngular, DimAnnotation, DimDiameter, DimDirection, DimLinear,
    DimOrdinate, DimRadial, DimStyle, Document, Entity, EntityGeom, FitClass, OrdinateAxis, Scale,
    SheetMeta, SizeTolerance,
};
use mcad_geom::{ArrowKind, DimSymbol, Point2};

use crate::plot::svg::to_svg;
use crate::plot::{PlotColorMode, plot_page};

/// [`ArrowKind`] の全既知バリアント（`mcad_geom::symbol` の `ALL_ARROW_KINDS` と同じ
/// 一覧。あちらはテスト専用の private 定数なのでここへも複製する。
/// **種別を増やしたらここへ足すこと。**）。
const ALL_ARROW_KINDS: [ArrowKind; 7] = [
    ArrowKind::ClosedFilled,
    ArrowKind::ClosedBlank,
    ArrowKind::Open30,
    ArrowKind::Open90,
    ArrowKind::Oblique,
    ArrowKind::Dot,
    ArrowKind::None,
];

/// 尺度 `num:den`・枠非表示の空ドキュメント（`crate::plot::tests` の
/// `document_with_scale` と同じ流儀）。
fn document_with_scale(num: u32, den: u32) -> Document {
    let mut document = Document::new();
    let sheet = SheetMeta {
        scale: Scale::new(num, den).unwrap(),
        ..document.sheet().clone()
    };
    document.apply(Command::SetSheet(sheet)).unwrap();
    document
}

/// 尺度 1:1・枠非表示の空ドキュメント。
fn document_1_1() -> Document {
    document_with_scale(1, 1)
}

/// カレントレイヤーへ幾何を追加する（スタイルは ByLayer）。
fn add(document: &mut Document, geom: impl Into<EntityGeom>) {
    let layer = document.current_layer();
    document
        .apply(Command::AddEntity(Entity::new(
            geom,
            layer,
            mcad_core::Style::inherited(),
        )))
        .unwrap();
}

/// `document` を SVG 文字列へ直列化する（モノクロ、`to_svg(plot_page(..))`）。
fn render(document: &Document) -> String {
    to_svg(&plot_page(document, PlotColorMode::Monochrome))
}

/// fixture ファイルのパス（`tests/snapshots/{name}.svg`）。
fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(format!("{name}.svg"))
}

/// `document` の SVG 出力を `name` の fixture と突き合わせる。
///
/// `UPDATE_SNAPSHOTS=1` が設定されているときは比較せず fixture を書き直す
/// （初回生成・意図的な更新用）。
fn assert_snapshot(name: &str, document: &Document) {
    let actual = render(document);
    let path = fixture_path(name);

    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(&path, &actual)
            .unwrap_or_else(|e| panic!("failed to write fixture {path:?}: {e}"));
        return;
    }

    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "failed to read fixture {path:?}: {e}\n\
             fixture が無いなら `UPDATE_SNAPSHOTS=1 cargo test -p mcad-app dim_snapshot_tests` \
             で生成すること。"
        )
    });

    assert_eq!(
        actual, expected,
        "寸法描画の SVG 出力が fixture {path:?} と一致しない。\n\
         意図した変更なら `UPDATE_SNAPSHOTS=1 cargo test -p mcad-app dim_snapshot_tests` \
         で更新し、`git diff` で差分をレビューすること。"
    );
}

// ---------------------------------------------------------------------
// 1: 長さ寸法 × 矢先7種
// ---------------------------------------------------------------------

#[test]
fn linear_dimension_snapshot_per_arrow_kind() {
    for kind in ALL_ARROW_KINDS {
        let name = format!("linear_arrow_{}", arrow_kind_name(kind));
        let mut document = document_1_1();
        document
            .apply(Command::SetDimStyle(DimStyle {
                arrow_kind: kind,
                ..DimStyle::DEFAULT
            }))
            .unwrap();
        add(
            &mut document,
            EntityGeom::DimLinear(DimLinear {
                p1: Point2::ORIGIN,
                p2: Point2::new(100.0, 0.0),
                offset: 20.0,
                direction: DimDirection::Aligned,
                annotation: DimAnnotation::default(),
            }),
        );
        assert_snapshot(&name, &document);
    }
}

/// snake_case のファイル名断片（`ArrowKind` のバリアント名を小文字化）。
fn arrow_kind_name(kind: ArrowKind) -> &'static str {
    match kind {
        ArrowKind::ClosedFilled => "closed_filled",
        ArrowKind::ClosedBlank => "closed_blank",
        ArrowKind::Open30 => "open30",
        ArrowKind::Open90 => "open90",
        ArrowKind::Oblique => "oblique",
        ArrowKind::Dot => "dot",
        ArrowKind::None => "none",
        // `#[non_exhaustive]`: 未知バリアントは `ClosedFilled` 同等に丸める
        // （geom 側の呼び出し規約と同じ、DESIGN.md M10 設計確定3）。
        _ => "unknown",
    }
}

// ---------------------------------------------------------------------
// 2: 半径寸法・直径寸法 × 矢先7種
//
// 半径寸法は矢 1 つ（引出線の先端）、直径寸法は矢 2 つ（中心を通る寸法線の両端）で、
// 長さ寸法とは別の配置経路（`expand_radial` / `expand_straight`）を通る。矢先を
// 既定だけにすると、その経路の回帰を検出できない（Codex review 2026-09-13 指摘）。
// ---------------------------------------------------------------------

#[test]
fn radial_dimension_snapshot_per_arrow_kind() {
    for kind in ALL_ARROW_KINDS {
        let name = format!("radial_arrow_{}", arrow_kind_name(kind));
        let mut document = document_1_1();
        document
            .apply(Command::SetDimStyle(DimStyle {
                arrow_kind: kind,
                ..DimStyle::DEFAULT
            }))
            .unwrap();
        add(
            &mut document,
            EntityGeom::DimRadial(DimRadial {
                center: Point2::ORIGIN,
                radius: 30.0,
                leader_angle: std::f64::consts::FRAC_PI_4,
                annotation: DimAnnotation::default(),
            }),
        );
        assert_snapshot(&name, &document);
    }
}

#[test]
fn diameter_dimension_snapshot_per_arrow_kind() {
    for kind in ALL_ARROW_KINDS {
        let name = format!("diameter_arrow_{}", arrow_kind_name(kind));
        let mut document = document_1_1();
        document
            .apply(Command::SetDimStyle(DimStyle {
                arrow_kind: kind,
                ..DimStyle::DEFAULT
            }))
            .unwrap();
        add(
            &mut document,
            EntityGeom::DimDiameter(DimDiameter {
                center: Point2::ORIGIN,
                radius: 25.0,
                angle: std::f64::consts::FRAC_PI_6,
                annotation: DimAnnotation::default(),
            }),
        );
        assert_snapshot(&name, &document);
    }
}

// ---------------------------------------------------------------------
// 3: 注記の代表組合せ
// ---------------------------------------------------------------------

/// 注記だけを差し替えた長さ寸法ドキュメント（既定の矢先・尺度 1:1）。
fn linear_with_annotation(annotation: DimAnnotation) -> Document {
    let mut document = document_1_1();
    add(
        &mut document,
        EntityGeom::DimLinear(DimLinear {
            p1: Point2::ORIGIN,
            p2: Point2::new(50.0, 0.0),
            offset: 15.0,
            direction: DimDirection::Aligned,
            annotation,
        }),
    );
    document
}

#[test]
fn annotation_symbol_diameter_snapshot() {
    let document = linear_with_annotation(DimAnnotation {
        symbol: Some(DimSymbol::Diameter),
        ..DimAnnotation::default()
    });
    assert_snapshot("annotation_symbol_diameter", &document);
}

#[test]
fn annotation_tolerance_deviations_snapshot() {
    let document = linear_with_annotation(DimAnnotation {
        tolerance: Some(SizeTolerance::Deviations {
            upper: 0.05,
            lower: -0.02,
        }),
        ..DimAnnotation::default()
    });
    assert_snapshot("annotation_tolerance_deviations", &document);
}

#[test]
fn annotation_tolerance_fit_snapshot() {
    let document = linear_with_annotation(DimAnnotation {
        tolerance: Some(SizeTolerance::Fit(FitClass::new("H7").unwrap())),
        ..DimAnnotation::default()
    });
    assert_snapshot("annotation_tolerance_fit", &document);
}

#[test]
fn annotation_value_override_underlined_snapshot() {
    let document = linear_with_annotation(DimAnnotation {
        value_override: Some("50".to_string()),
        ..DimAnnotation::default()
    });
    assert_snapshot("annotation_value_override_underlined", &document);
}

#[test]
fn annotation_decimals_override_snapshot() {
    let document = linear_with_annotation(DimAnnotation {
        decimals_override: Some(3),
        ..DimAnnotation::default()
    });
    assert_snapshot("annotation_decimals_override", &document);
}

#[test]
fn annotation_arrow_placement_outside_snapshot() {
    let document = linear_with_annotation(DimAnnotation {
        arrow_placement: ArrowPlacement::Outside,
        ..DimAnnotation::default()
    });
    assert_snapshot("annotation_arrow_placement_outside", &document);
}

#[test]
fn annotation_text_anchor_manual_snapshot() {
    let document = linear_with_annotation(DimAnnotation {
        text_anchor: Some(Point2::new(25.0, 40.0)),
        ..DimAnnotation::default()
    });
    assert_snapshot("annotation_text_anchor_manual", &document);
}

/// 参考寸法（丸括弧。M11 タスク70）。JIS B 0001:2019 11.1 / JIS Z 8317-1:2008 7.11。
#[test]
fn annotation_value_style_reference_snapshot() {
    let document = linear_with_annotation(DimAnnotation {
        value_style: mcad_core::ValueStyle::Reference,
        ..DimAnnotation::default()
    });
    assert_snapshot("annotation_value_style_reference", &document);
}

/// 理論的に正確な寸法（矩形の枠。M11 タスク70）。`製図規定.md` に本文が無いため
/// JIS の慣行に倣う（[`mcad_core::ValueStyle::TheoreticallyExact`] の doc 参照）。
#[test]
fn annotation_value_style_theoretically_exact_snapshot() {
    let document = linear_with_annotation(DimAnnotation {
        value_style: mcad_core::ValueStyle::TheoreticallyExact,
        ..DimAnnotation::default()
    });
    assert_snapshot("annotation_value_style_theoretically_exact", &document);
}

/// 接頭辞・接尾辞（M11 タスク70。`2×φ10` の `2×`、`4-M6` の `-M6` に相当）。
#[test]
fn annotation_prefix_and_suffix_snapshot() {
    let document = linear_with_annotation(DimAnnotation {
        prefix: Some("2×".to_string()),
        suffix: Some("-M6".to_string()),
        ..DimAnnotation::default()
    });
    assert_snapshot("annotation_prefix_and_suffix", &document);
}

/// 文字回転（水平固定。M11 タスク70）。斜めの計測 2 点で通常は文字も斜めになるが、
/// `text_rotation: Some(0.0)` で水平に固定する。
#[test]
fn annotation_text_rotation_horizontal_on_a_diagonal_dimension_snapshot() {
    let mut document = document_1_1();
    add(
        &mut document,
        EntityGeom::DimLinear(DimLinear {
            p1: Point2::ORIGIN,
            p2: Point2::new(40.0, 30.0),
            offset: 10.0,
            direction: DimDirection::Aligned,
            annotation: DimAnnotation {
                text_rotation: Some(0.0),
                ..DimAnnotation::default()
            },
        }),
    );
    assert_snapshot("annotation_text_rotation_horizontal", &document);
}

// ---------------------------------------------------------------------
// 4: 尺度 1:2 の長さ寸法（紙基準の拡縮）
// ---------------------------------------------------------------------

#[test]
fn linear_dimension_snapshot_at_scale_1_to_2() {
    let mut document = document_with_scale(1, 2);
    add(
        &mut document,
        EntityGeom::DimLinear(DimLinear {
            p1: Point2::ORIGIN,
            p2: Point2::new(100.0, 0.0),
            offset: 20.0,
            direction: DimDirection::Aligned,
            annotation: DimAnnotation::default(),
        }),
    );
    assert_snapshot("linear_scale_1_to_2", &document);
}

// ---------------------------------------------------------------------
// 5: 角度寸法・座標寸法（M11 タスク69、DXF import 引き継ぎ分の回帰）
// ---------------------------------------------------------------------

#[test]
fn angular_dimension_snapshot() {
    let mut document = document_1_1();
    add(
        &mut document,
        EntityGeom::DimAngular(DimAngular {
            vertex: Point2::ORIGIN,
            p1: Point2::new(100.0, 0.0),
            p2: Point2::new(50.0, 50.0 * 3f64.sqrt()),
            arc_radius: 40.0,
            annotation: DimAnnotation::default(),
        }),
    );
    assert_snapshot("angular_dimension", &document);
}

/// 矢先を既定（`ClosedFilled`）以外へ変えた角度寸法。矢が弧の接線方向へ置かれる
/// 経路（[`mcad_core::expand_angular`] の doc）の回帰を検出する
/// （矢先を既定のままにすると、この経路の崩れを検出できない。radial/diameter の
/// 既存スナップショットと同じ理由）。
#[test]
fn angular_dimension_snapshot_with_open90_arrow() {
    let mut document = document_1_1();
    document
        .apply(Command::SetDimStyle(DimStyle {
            arrow_kind: ArrowKind::Open90,
            ..DimStyle::DEFAULT
        }))
        .unwrap();
    add(
        &mut document,
        EntityGeom::DimAngular(DimAngular {
            vertex: Point2::ORIGIN,
            p1: Point2::new(100.0, 0.0),
            p2: Point2::new(50.0, 50.0 * 3f64.sqrt()),
            arc_radius: 40.0,
            annotation: DimAnnotation::default(),
        }),
    );
    assert_snapshot("angular_dimension_arrow_open90", &document);
}

#[test]
fn ordinate_dimension_snapshot() {
    let mut document = document_1_1();
    add(
        &mut document,
        EntityGeom::DimOrdinate(DimOrdinate {
            origin: Point2::ORIGIN,
            feature: Point2::new(60.0, 20.0),
            leader_end: Point2::new(60.0, 45.0),
            axis: OrdinateAxis::X,
            annotation: DimAnnotation::default(),
        }),
    );
    assert_snapshot("ordinate_dimension", &document);
}
