//! 大規模図面の性能実測（DESIGN.md M11 章タスク66、設計判断3）。
//!
//! # 目的
//!
//! 空間インデックス(タスク76)を入れるかどうかを実測で決める。現状どこが遅いのか
//! 分からないまま導入すると、効果の無い複雑さを core へ持ち込む。ここでは
//! 1k/5k/20k エンティティの図面を決定的に生成し、pick・矩形選択・ズームフィット・
//! 描画前処理・SVG/PDF 代理指標(`plot_page`)・保存/読込 の所要時間を測る。
//!
//! # 実行方法
//!
//! ```text
//! cargo test --release -p mcad-app -- --ignored --nocapture
//! ```
//!
//! 通常の `cargo test`(3チェックの一部)では走らない。`#[ignore]` を付けている
//! 理由は、計測が遅く(release ビルドでも数十秒かかりうる)CI の3チェックを
//! 重くしないため。
//!
//! # 測定条件
//!
//! - **release ビルド必須**(`--release` を付けずに実行すると数値に意味がない。
//!   debug ビルドは最適化なしで実際のフレーム性能を反映しない)。
//! - マシンは**実行者のもの**(采配役・実装者のローカル環境)。CI や他マシンとの
//!   比較はできない。絶対値ではなく「16ms(60fps の1フレーム)を超えるか」の
//!   閾値判定に使う。
//! - 各操作はウォームアップ1回 + 本計測5回の**中央値**(ms)を表示する。
//! - egui の実描画(ペイント)は headless で測れないため、描画コストの代理指標として
//!   「描画前処理」(`entities_in_draw_order` + 全エンティティの `entity_world_aabb`)と
//!   `plot::plot_page`(SVG/PDF 生成の共通経路)を測る。**この数値は実際の画面フレーム
//!   時間そのものではない**(egui 自体の描画・レイアウトコストを含まない)。
//!
//! この数値は**空間インデックスの要否判断に使う**(DESIGN.md M11 設計判断3)。
//! 16ms を超える操作が実在すれば導入根拠になり、無ければ「計測結果と見送りの判断」を
//! DESIGN.md へ記録して閉じる。

use std::time::{Duration, Instant};

use mcad_core::{
    Command, DimAnnotation, DimDirection, DimLinear, Document, Entity, EntityGeom, Style, TextGeom,
};
use mcad_geom::{Arc, LineSeg, Point2, Polyline, Shape};

use crate::plot::{PlotColorMode, plot_page};
use crate::tool::SelectTool;
use crate::{document_aabb, entities_in_draw_order, entity_world_aabb};

/// 固定シードの線形合同法(LCG)。乱数クレートを増やさず、決定的な図面生成に使う。
///
/// 定数は Numerical Recipes の LCG(`a = 1664525`, `c = 1013904223`, mod 2^32)。
/// 暗号的な性質は不要で、単に「毎回同じ図面が再現できる」ことだけが目的。
struct Lcg {
    state: u64,
}

impl Lcg {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u32(&mut self) -> u32 {
        self.state = self
            .state
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223)
            & 0xFFFF_FFFF;
        self.state as u32
    }

    /// `[lo, hi)` の範囲の `f64` を返す。
    fn next_f64(&mut self, lo: f64, hi: f64) -> f64 {
        let t = f64::from(self.next_u32()) / f64::from(u32::MAX);
        lo + t * (hi - lo)
    }
}

/// 図面全体の広がり(ワールド mm)。実使用の部品図〜組立図程度の広さを想定した決め打ち。
const SPREAD: f64 = 10_000.0;

/// `n` エンティティの図面を決定的に生成する。
///
/// 内訳(実使用に近い比率の決め打ち): 線 50% / 円弧 20% / ポリライン 10% /
/// 文字 10% / 長さ寸法 10%。座標は `[0, SPREAD)` へ一様に散らす。
fn build_document(n: usize) -> Document {
    let mut document = Document::new();
    let layer = document.current_layer();
    let style = Style::inherited();
    let mut rng = Lcg::new(0x5EED_1234);

    let pt = |rng: &mut Lcg| Point2::new(rng.next_f64(0.0, SPREAD), rng.next_f64(0.0, SPREAD));

    let mut cmds = Vec::with_capacity(n);
    for i in 0..n {
        let bucket = i % 10;
        let geom: EntityGeom = if bucket < 5 {
            // 線 50%
            EntityGeom::Shape(Shape::Line(LineSeg::new(pt(&mut rng), pt(&mut rng))))
        } else if bucket < 7 {
            // 円弧 20%
            let center = pt(&mut rng);
            let radius = rng.next_f64(1.0, 200.0);
            let start = rng.next_f64(0.0, std::f64::consts::TAU);
            let sweep = rng.next_f64(0.1, std::f64::consts::PI);
            EntityGeom::Shape(Shape::Arc(Arc::new(center, radius, start, start + sweep)))
        } else if bucket < 8 {
            // ポリライン 10%(頂点5個)
            let vertices = (0..5).map(|_| pt(&mut rng)).collect();
            EntityGeom::Shape(Shape::Polyline(Polyline::new(vertices, false)))
        } else if bucket < 9 {
            // 文字 10%
            EntityGeom::Text(TextGeom {
                anchor: pt(&mut rng),
                content: format!("TEXT-{i}"),
                height: 3.5,
                angle: 0.0,
            })
        } else {
            // 長さ寸法 10%
            EntityGeom::DimLinear(DimLinear {
                p1: pt(&mut rng),
                p2: pt(&mut rng),
                offset: rng.next_f64(5.0, 30.0),
                direction: DimDirection::Aligned,
                annotation: DimAnnotation::unannotated(),
            })
        };
        cmds.push(Command::AddEntity(Entity::new(geom, layer, style)));
    }
    document
        .apply(Command::Batch(cmds))
        .expect("大規模図面の生成コマンドは成功するはず");
    document
}

/// `f` をウォームアップ1回 + 本計測5回実行し、5回の中央値(ミリ秒)を返す。
fn median_ms(mut f: impl FnMut()) -> f64 {
    f(); // warmup
    let mut samples: Vec<Duration> = (0..5)
        .map(|_| {
            let start = Instant::now();
            f();
            start.elapsed()
        })
        .collect();
    samples.sort();
    samples[2].as_secs_f64() * 1000.0
}

/// 60fps の1フレーム予算(ms)。これを超えた操作には表で印を付ける。
const FRAME_BUDGET_MS: f64 = 16.0;

/// 表の1行(規模 × 操作 × 中央値ms)。
struct Row {
    scale: usize,
    op: &'static str,
    ms: f64,
}

fn print_table(rows: &[Row]) {
    println!(
        "{:>8}  {:<28}  {:>12}  16ms超",
        "規模", "操作", "中央値(ms)"
    );
    for row in rows {
        let mark = if row.ms > FRAME_BUDGET_MS { "*" } else { "" };
        println!(
            "{:>8}  {:<28}  {:>12.3}  {}",
            row.scale, row.op, row.ms, mark
        );
    }
}

#[test]
#[ignore = "release ビルドでの実測用。`cargo test --release -p mcad-app -- --ignored --nocapture` で実行する"]
fn measure_large_document_performance() {
    let mut rows = Vec::new();

    for &n in &[1_000usize, 5_000, 20_000] {
        let document = build_document(n);
        let k = document.sheet().scale.world_mm_per_paper_mm();
        let aabb = document_aabb(&document).expect("空でない図面には aabb があるはず");

        // pick: 図形の上(既知の1件目のエンティティ位置近傍)・図形から遠い位置の2パターン。
        // `pick` は private なので公開経路 `SelectTool::on_click` 経由で測る
        // (実体は同じ探索コストで、選択集合の push/contains は無視できる程度)。
        let (_, first_entity) = document.entities().next().expect("非空");
        let hit_point = match &first_entity.geom {
            EntityGeom::Shape(Shape::Line(l)) => l.a,
            _ => Point2::new(0.0, 0.0),
        };
        let far_point = Point2::new(SPREAD * 10.0, SPREAD * 10.0);
        let tol = 1.0;

        rows.push(Row {
            scale: n,
            op: "pick(命中)",
            ms: median_ms(|| {
                let mut tool = SelectTool::default();
                tool.on_click(&document, hit_point, tol, false);
            }),
        });
        rows.push(Row {
            scale: n,
            op: "pick(遠方・不命中)",
            ms: median_ms(|| {
                let mut tool = SelectTool::default();
                tool.on_click(&document, far_point, tol, false);
            }),
        });

        // 矩形選択: 全体を囲む・一部(左下 1/4)を囲む。
        let full_start = Point2::new(-1.0, -1.0);
        let full_end = Point2::new(SPREAD + 1.0, SPREAD + 1.0);
        let partial_start = Point2::new(0.0, 0.0);
        let partial_end = Point2::new(SPREAD / 4.0, SPREAD / 4.0);

        rows.push(Row {
            scale: n,
            op: "矩形選択(全体)",
            ms: median_ms(|| {
                let mut tool = SelectTool::default();
                tool.on_drag_start(full_start);
                tool.on_drag_end(&document, full_end);
            }),
        });
        rows.push(Row {
            scale: n,
            op: "矩形選択(一部)",
            ms: median_ms(|| {
                let mut tool = SelectTool::default();
                tool.on_drag_start(partial_start);
                tool.on_drag_end(&document, partial_end);
            }),
        });

        // ズームフィット対象の AABB 計算。
        rows.push(Row {
            scale: n,
            op: "document_aabb",
            ms: median_ms(|| {
                let _ = document_aabb(&document);
            }),
        });

        // 描画前処理: 可視エンティティの抽出・整列 + 各エンティティの表示 AABB。
        rows.push(Row {
            scale: n,
            op: "描画前処理(カリング+AABB)",
            ms: median_ms(|| {
                let drawable = entities_in_draw_order(&document, &aabb, k);
                for (_, entity, _) in &drawable {
                    let _ = entity_world_aabb(&entity.geom, k);
                }
            }),
        });

        // plot_page: SVG/PDF 出力の共通経路(描画コストの代理指標)。
        rows.push(Row {
            scale: n,
            op: "plot_page(Monochrome)",
            ms: median_ms(|| {
                let _ = plot_page(&document, PlotColorMode::Monochrome);
            }),
        });

        // 保存/読込。
        rows.push(Row {
            scale: n,
            op: ".mcad 保存(to_json)",
            ms: median_ms(|| {
                let _ = mcad_io::to_json(&document).expect("保存に成功するはず");
            }),
        });
        let json = mcad_io::to_json(&document).expect("保存に成功するはず");
        rows.push(Row {
            scale: n,
            op: ".mcad 読込(from_json)",
            ms: median_ms(|| {
                let _ = mcad_io::from_json(&json).expect("読込に成功するはず");
            }),
        });
    }

    print_table(&rows);
}

/// 実 GUI での体感確認用に、計測と同じ 20,000 エンティティの図面を書き出す。
///
/// headless の計測（[`measure_large_document_performance`]）は egui の実描画を含まない。
/// 「画面に出したときに重いか」は実ウィンドウでしか分からないので、同じ図面を `.mcad` として
/// 保存し、ユーザーが開いて確かめられるようにする。出力先は環境変数 `PERF_SAMPLE_OUT`
/// （既定 `/tmp/mcad-perf-20k.mcad`）。
///
/// ```text
/// cargo test --release -p mcad-app -- --ignored export_sample_document --nocapture
/// ```
#[test]
#[ignore = "計測用の図面を書き出すだけ。通常のテストでは走らせない"]
fn export_sample_document_for_manual_check() {
    let path =
        std::env::var("PERF_SAMPLE_OUT").unwrap_or_else(|_| "/tmp/mcad-perf-20k.mcad".to_owned());
    let document = build_document(20_000);
    mcad_io::save_mcad(&document, &path).expect("write sample document");
    let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    println!(
        "20,000 エンティティの図面を書き出した: {path}（{:.1} MB）",
        bytes as f64 / 1_048_576.0
    );
}
