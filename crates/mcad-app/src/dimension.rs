//! 寸法ジオメトリの展開（寸法線・補助線・矢先・文字配置）を計算する純関数群。
//!
//! # 設計方針（DESIGN.md M6 設計判断2）
//!
//! 寸法の **展開ロジックは app 層に閉じる**（＝ mcad-geom には入れない）。データ型
//! （[`DimLinear`] / [`DimRadial`] / [`DimDiameter`]）は永続化・undo の都合で mcad-core に
//! 置くが、矢印・補助線・文字配置の生成はここ（app 層）の純関数で行い、単体テストで固定する。
//!
//! # 描画とヒットテストの一貫性
//!
//! [`expand_linear`] / [`expand_radial`] / [`expand_diameter`] が返す [`DimExpansion`] を
//! 描画（`main.rs` の `draw_dim_expansion`）とプレビューが共有する。ヒットテスト
//! （`SelectTool::pick`）は [`linear_distance`] / [`radial_distance`] /
//! [`diameter_distance`] を使い、**保存データ（p1/p2/offset・center/radius/leader_angle・
//! center/radius/angle）だけで決まるズーム非依存の線分**への最短距離を返す。
//! これにより「tol 以内で最も近いものを拾う」という既存の pick 契約へ素直に合流する
//! （矢印・文字の見かけの大きさに依存しない。M6 タスク23 の Text ヒットテストで得た教訓）。
//!
//! pick 用の線分（[`linear_pick_segments`] / [`radial_pick_segments`] /
//! [`diameter_pick_segments`]）は**保存データだけを引数に取る**。描画側が足す
//! すきま・突き出し・矢の内外配置（いずれもスタイル・表示状態に依存する）は pick 形状へ
//! 一切反映しない。反映すると「同じクリックがズームによって当たったり外れたりする」
//! 不具合（M6 タスク24 の前例）を招くためである。
//!
//! # 文字ブロックの当たり判定は例外（M9 タスク51、設計判断7）
//!
//! [`DimExpansion::label_box`] は**表示サイズ依存**（`DimRender` を通した展開結果）で、
//! 上記のズーム非依存 pick 契約とは別の当たり判定に使う。対象は**選択済み**の寸法の
//! 文字ブロックドラッグ（`text_anchor` の後編集）に限られ、`linear_pick_segments` 等の
//! 通常選択の pick 形状には一切混ぜない。表示サイズに依存してよい理由は、これが
//! 「掴める場所を見た目どおりに掴む」操作であり、ズームで当たり外れが変わっても
//! 実害がない（選択そのものを左右しない）ため。
//!
//! # 展開パラメータ（M9 タスク49-2）
//!
//! 展開関数は [`DimRender`] を受け取る。**このモジュールは紙基準表示トグル（F9）も
//! ズームも知らない**。呼び出し側（`main.rs` の `dim_render`・`plot`）が表示モードを
//! 解決してワールド長を詰め、このモジュールは受け取った長さをそのまま使う。
//!
//! # ラベルの組版（M9 タスク49-1）
//!
//! 注記（寸法補助記号・サイズ公差・桁数上書き・非比例寸法）つきの寸法値を組み上げる
//! 純関数は [`label`] モジュールに分けてある。**配置（ワールドへの回転・平行移動）は
//! 行わず**、ラベルのローカル座標だけを返す点でこのモジュール本体の展開関数とは責務が
//! 分かれている。ワールドへの合流は [`place_label`]（タスク49-2）が担う。

use mcad_core::{
    ArrowPlacement, DimAnnotation, DimDiameter, DimKind, DimLinear, DimRadial, DimStyle, TextGeom,
};
use mcad_geom::{LineSeg, Point2, Shape, Vec2};

use self::label::{DimLabel, layout_dim_label};

/// 文字幅の近似係数。mcad-core の `text_aabb` と同じ ASCII 係数 0.55×height で幅を
/// 近似する（純関数のまま中央寄せ配置を決めるための割り切り。DESIGN.md M6 設計判断1 の
/// 近似方針に沿う）。
///
/// [`label`] の組版は ASCII 以外の文字（`±` や、値上書きに入りうる任意の文字）も
/// 同じ係数で数える。フォントメトリクスを持ち込まない以上いずれ近似でしかなく、
/// mcad-geom の `symbol::FONT_CHAR_ADVANCE_RATIO`（記号の送り量）と**同じ量**なので
/// 値を食い違わせないことのほうが重要である。
const ASCII_CHAR_WIDTH_RATIO: f64 = 0.55;

/// 矢先の半幅と長さの比。全開き角度20°（tan(10°) ≈ 0.1763）。規定 5-4 4) と同値を保つこと。
///
/// M9 タスク49 の手動スモークテスト（2026-08-23、ユーザー実施）で、規定の草案値だった 15°
/// は塗りつぶし矢としては細すぎると判断され、規定 5-4 4) 側の数値ごと 20° へ改めた。
/// 塗りつぶし矢の一般的な慣行に沿う値で、AutoCAD の既定 "Closed filled" も約 19°。
/// 矢先の長さ（[`mcad_core::DimStyle::arrow_len_mm`]、既定 3.0mm）は据え置き。
const ARROW_HALF_WIDTH_RATIO: f64 = 0.1763;

/// 寸法を描画・プレビュー可能な要素へ展開した結果（すべてワールド座標）。
#[derive(Debug, Clone, PartialEq)]
pub struct DimExpansion {
    /// 線分（寸法線・補助線・引出線・非比例寸法の下線）。
    ///
    /// 非比例寸法（規定 5-10）の下線をここへ混ぜているのは、下線が寸法線とまったく
    /// 同じ実線ストロークで描かれるべきものであり、consumers（`main.rs` の
    /// `draw_dim_expansion`・`plot::push_dim`）へ描画分岐を 1 つも増やさずに済むため。
    ///
    /// 並び順は **寸法線（または引出線）→ 非比例寸法の下線（あれば）→ 補助線**。
    /// `segments[0]` が寸法線であることだけは全種別で共通。
    pub segments: Vec<[Point2; 2]>,
    /// 矢先（各要素は塗りつぶす三角形の 3 頂点）。
    pub arrows: Vec<[Point2; 3]>,
    /// フォント文字として描く要素（値・英字記号・公差）。既存の `draw_text` を
    /// そのまま再利用できる形で返す。
    ///
    /// **[`TextGeom::height`] はワールド長**（`TextGeom` 本来の「紙 mm」契約とは違う）。
    /// これはタスク37 からの既存の非対称で、`plot::push_dim` が `÷ k` して紙 mm へ戻す。
    pub texts: Vec<TextGeom>,
    /// ストロークとして描く寸法補助記号（φ・□）。ワールド座標。
    ///
    /// 英字系の記号（R・SR・CR・C・t）はフォント文字なので [`DimExpansion::texts`] 側へ入る
    /// （振り分けは `mcad_geom::dim_symbol_glyph` の `shapes` が空かどうかで決まる）。
    /// 画面（`draw_dim_expansion`）と出力（`plot::push_dim`）のどちらも寸法線と同じ
    /// ストロークで描く。
    pub symbol_strokes: Vec<Shape>,
    /// 文字ブロックの外形（[`label::DimLabel::bounds`] をワールドへ写した凸四角形）。
    ///
    /// 頂点順は `[左下, 右下, 右上, 左上]`（ラベルのローカル軸に沿う。回転しても平行四辺形
    /// のまま歪まない）。**M9 タスク51**（文字位置の後編集）の当たり判定・ドラッグ対象の
    /// 可視化に使う。`place_label` を呼ばない経路は素通しで `None` のまま
    /// （現状すべての展開関数が `place_label` を通るため実質常に `Some`）。
    pub label_box: Option<[Point2; 4]>,
}

impl DimExpansion {
    /// 線分・矢先だけを持つ空のラベル列で作る（ラベルは [`place_label`] が後から積む）。
    fn new(segments: Vec<[Point2; 2]>, arrows: Vec<[Point2; 3]>) -> Self {
        Self {
            segments,
            arrows,
            texts: Vec::new(),
            symbol_strokes: Vec::new(),
            label_box: None,
        }
    }
}

/// 凸四角形 `quad`（[`DimExpansion::label_box`] と同じ頂点順）が点 `p` を含むか。
///
/// 頂点順（時計回り・反時計回りいずれか一定方向）を前提に、各辺について `p` が
/// 同じ側にあるかを符号付き外積で判定する（すべて同符号 = 内側）。`label_box` は
/// 常に矩形を回転しただけの凸四角形なので、この単純な判定で十分。
#[must_use]
pub fn label_box_contains(quad: &[Point2; 4], p: Point2) -> bool {
    let mut sign = 0.0_f64;
    for i in 0..4 {
        let a = quad[i];
        let b = quad[(i + 1) % 4];
        let edge = b - a;
        let to_p = p - a;
        let cross = edge.x * to_p.y - edge.y * to_p.x;
        if cross == 0.0 {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    true
}

/// 凸四角形 `quad`（[`DimExpansion::label_box`] と同じ頂点順）の中心（対角線の交点）。
#[must_use]
pub fn label_box_center(quad: &[Point2; 4]) -> Point2 {
    quad[0].midpoint(quad[2])
}

// ---------------------------------------------------------------------
// 展開パラメータ
// ---------------------------------------------------------------------

/// 寸法をワールドへ展開するときのパラメータ一式（M9 タスク49-2）。
///
/// # 単位はフィールド名で示す
///
/// `*_world` はワールド長、`*_mm` は紙 mm。[`DimStyle`] の長さフィールドは**すべて紙 mm**
/// なので、ワールドへ出すときは必ず [`DimRender::paper_mm_to_world`] を通す。
/// この命名規則は M8 タスク37 の教訓（`DimExpansion` の文字高さはワールド長なのに
/// [`TextGeom::height`] は紙 mm、という取り違えが二重換算を生んだ）への対策である。
///
/// # 2 つの換算係数を混同しないこと
///
/// - [`DimRender::scale_world_per_paper_mm`] は**文書尺度**（`Scale::world_mm_per_paper_mm`）。
///   表示状態に依存しない図面固有の量で、**外向き矢印の自動判定にしか使わない**。
/// - [`DimRender::paper_mm_to_world`] は**注記の表示倍率**。紙基準表示 OFF では
///   ズームに依存する（画面固定 px モード）。すきま・突き出し・文字すきまはこちらで換算する。
#[derive(Debug, Clone, Copy)]
pub struct DimRender<'a> {
    /// 文書単位の寸法スタイル（長さは紙 mm）。
    pub style: &'a DimStyle,
    /// 文書尺度（ワールド mm / 紙 mm、`Scale::world_mm_per_paper_mm`）。
    ///
    /// ワールド長を紙 mm へ**戻す**ために使う。表示状態（紙基準表示トグル・ズーム）に
    /// 依存しないので、これを使う判定はビュー非依存になる。
    pub scale_world_per_paper_mm: f64,
    /// 矢先の長さ（ワールド長）。
    pub arrow_len_world: f64,
    /// 寸法値の基準文字高さ（ワールド長。公差文字はこれの `tolerance_scale` 倍）。
    pub text_height_world: f64,
}

impl DimRender<'_> {
    /// 注記の表示倍率（ワールド長 / 紙 mm）。
    ///
    /// **文書尺度 `k` ではなく「実際に描かれる文字高さ ÷ スタイルの紙 mm 文字高さ」**を
    /// 使う。理由は 2 つ。
    ///
    /// 1. 紙基準表示 ON では `text_height_world = text_height_mm * k` なので、この係数は
    ///    ちょうど `k` に一致する（＝紙の上ですきま 1mm・突き出し 2mm。規定 5-4 2)）。
    /// 2. 紙基準表示 OFF（既定）は注記を画面固定 px で描くモードなので、すきまだけを
    ///    `k` 倍のワールド長で置くと**ズームを上げるほどすきまが文字より大きく**なり、
    ///    補助線が図形から離れて見える。表示倍率で揃えれば、既定スタイルでは
    ///    `text_gap_mm / text_height_mm = 1.4 / 3.5 = 0.4` となり、M8 までの
    ///    `gap = height * 0.4` と**両モードで同値**になる（`DimStyle::text_gap_mm` の doc が
    ///    主張する「既定値のままなら現行描画と同値」を、画面固定 px モードでも保つ）。
    ///
    /// スタイルの文字高さが不正（非有限・非正）なときだけ文書尺度へ退避する
    /// （[`DimStyle::validate`] が通った値なら起こらないが、除算で NaN を撒かないため）。
    fn annotation_scale(&self) -> f64 {
        let nominal_mm = self.style.text_height_mm;
        if nominal_mm.is_finite() && nominal_mm > 0.0 {
            self.text_height_world / nominal_mm
        } else {
            self.scale_world_per_paper_mm
        }
    }

    /// スタイルの紙 mm 長（すきま・突き出し・文字すきま）をワールド長へ換算する。
    fn paper_mm_to_world(&self, mm: f64) -> f64 {
        mm * self.annotation_scale()
    }
}

/// 先端 `tip`・方向 `dir`（単位ベクトル）・長さ `len` の矢先三角形を作る。
fn arrow_triangle(tip: Point2, dir: Vec2, len: f64) -> [Point2; 3] {
    let back = tip - dir * len;
    let half = dir.perp() * (len * ARROW_HALF_WIDTH_RATIO);
    [tip, back + half, back - half]
}

/// 点 `p` から線分群への最短距離。空なら [`f64::INFINITY`]。
fn min_distance_to_segments(segs: &[[Point2; 2]], p: Point2) -> f64 {
    segs.iter()
        .map(|[a, b]| LineSeg::new(*a, *b).closest_point(p).distance(p))
        .fold(f64::INFINITY, f64::min)
}

// ---------------------------------------------------------------------
// JIS の配置規則（M9 タスク49-2）
// ---------------------------------------------------------------------

/// 寸法値の読み方向（規定 5-2 3)）。
///
/// 寸法線方向 `dir` を、**左下から右上へ読める向き**（角度が `(-90°, +90°]` に入る向き）へ
/// 正規化する。水平寸法線は左→右、垂直寸法線は下→上になる。
///
/// この戻り値の [`Vec2::perp`] の正側が「文字を置く上側」であり、規定 5-2 3) a) の
/// 「水平寸法線の上側・垂直寸法線の左側」と一致する（垂直は読み方向 `(0,1)` の
/// `perp()` が `(-1,0)` ＝ 左側）。**ビューには一切依存しない**（`offset` の符号でも
/// 画面の向きでもなく、寸法線の向きだけで決まる）。
fn reading_direction(dir: Vec2) -> Vec2 {
    // 真上向き（`dir.x == 0`）は `-90°` ではなく `+90°` を選ぶため、y の符号で決める。
    if dir.x < 0.0 || (dir.x == 0.0 && dir.y < 0.0) {
        -dir
    } else {
        dir
    }
}

/// 寸法補助線 1 本を作る（規定 5-4 2) b) c)）。
///
/// `measured` は計測点、`end` は寸法線側の端点。図形との間に `gap_world` のすきまを
/// 空けて始まり、寸法線を `overshoot_world` だけ越えて終わる。
///
/// # 退化時は引かない
///
/// 計測点から寸法線までの距離が `gap_world` **以下**のときは `None` を返す（描かない）。
/// この状況ではすきまだけで寸法線に届いてしまい、「図形から寸法線へ引く」という補助線の
/// 役目がまるごと消える。それでも突き出しぶんだけ描くと、**図形と繋がっていない短い線が
/// 寸法線の向こう側に浮く**ことになり、読み手には何の線か分からない。
/// `overshoot_world` が 0 のときは始点が終点を追い越して線が反転もするため、
/// 「引かない」で両方まとめて塞ぐ（テスト
/// `linear_extension_lines_vanish_when_the_offset_is_within_the_gap` が固定）。
///
/// 計測点と寸法線端が完全に一致する（`offset` が 0）ときも向きが決まらないので `None`。
fn extension_line(
    measured: Point2,
    end: Point2,
    gap_world: f64,
    overshoot_world: f64,
) -> Option<[Point2; 2]> {
    let span = end - measured;
    let u = span.normalize()?;
    if span.length() <= gap_world {
        return None;
    }
    Some([measured + u * gap_world, end + u * overshoot_world])
}

/// 矢を外向きに描くか（規定 5-4 4) の内外配置。DESIGN.md M9 設計判断5）。
///
/// [`ArrowPlacement::Inside`] / [`ArrowPlacement::Outside`] は手動指定なのでそのまま従う。
/// [`ArrowPlacement::Auto`] は「ラベル幅 + 矢 2 つ分が寸法線長に収まるか」で決める。
///
/// # 判定はビュー非依存（必須）
///
/// 3 つの量を**すべて紙 mm**で取る。ラベル幅は [`DimStyle::text_height_mm`] で組んだ
/// [`DimLabel::width`]、矢先長は [`DimStyle::arrow_len_mm`]、寸法線長は
/// ワールド長を [`DimRender::scale_world_per_paper_mm`] で割った値である。
/// **描画用のワールド長（紙基準表示 OFF ではズーム依存）を混ぜてはならない。**
/// M9 の検収基準は「画面と SVG/PDF で同一に描画される」ことを要求しており、判定が
/// 表示状態やズームで変われば画面と出力が食い違う（M6 タスク24 で、ズーム依存のピック
/// 許容量を幾何的な退化判定へ流用して同種の不具合を出した前例がある）。
/// テスト `arrow_placement_decision_is_independent_of_zoom_and_paper_display` が固定する。
fn arrows_point_outward(
    line_len_world: f64,
    value: f64,
    annotation: &DimAnnotation,
    kind: DimKind,
    render: DimRender<'_>,
) -> bool {
    match annotation.arrow_placement {
        ArrowPlacement::Inside => false,
        ArrowPlacement::Outside => true,
        ArrowPlacement::Auto => {
            let k = render.scale_world_per_paper_mm;
            // 尺度が壊れていると紙 mm へ戻せない。安全側（内向き＝従来の形）へ倒す。
            if !(k.is_finite() && k > 0.0) {
                return false;
            }
            let line_len_mm = line_len_world / k;
            let label_width_mm = layout_dim_label(
                value,
                annotation,
                render.style,
                kind,
                render.style.text_height_mm,
            )
            .width();
            line_len_mm < label_width_mm + 2.0 * render.style.arrow_len_mm
        }
    }
}

/// 組版済みラベル（ローカル座標）をワールドへ配置し、`out` の各列へ積む。
///
/// `center` はラベル**外形の中心**を置くワールド点、`along` は読み方向の単位ベクトル
/// （[`reading_direction`]）。ローカル座標 `(x, y)` は
/// `origin + along * x + along.perp() * y` へ移る。
///
/// # 幅ではなく外形（[`DimLabel::bounds`]）で中央寄せする理由
///
/// 上下段公差の下段と非比例寸法の下線は**ベースラインより下**へ出るため、原点は外形の
/// 左下ではない。`width() / 2` を引くだけでは、その下方向のはみ出しぶんだけ縦位置が
/// ずれる（ラベルが寸法線側へ寄る）。外形の中心を基準にすれば、公差や下線が付いても
/// 「寸法線から text_gap だけ離す」が正しく保たれる。
///
/// 非比例寸法の下線は [`DimExpansion::segments`] へ入れる（型の doc 参照）。
fn place_label(label: &DimLabel, center: Point2, along: Vec2, out: &mut DimExpansion) {
    let angle = along.angle();
    let up = along.perp();
    let local_center = label.bounds.center();
    let origin = center - along * local_center.x - up * local_center.y;
    let to_world = |p: Point2| origin + along * p.x + up * p.y;

    out.texts.extend(label.runs.iter().map(|run| TextGeom {
        anchor: to_world(run.origin),
        content: run.content.clone(),
        height: run.height,
        angle,
    }));
    // ストロークはローカル原点まわりの回転 → 平行移動。`Shape::rotated` の回転は
    // `to_world` と同じ線形部分（`along` / `up` は角度 `angle` の正規直交基底）。
    let delta = origin - Point2::ORIGIN;
    out.symbol_strokes.extend(
        label
            .strokes
            .iter()
            .map(|shape| shape.rotated(Point2::ORIGIN, angle).translated(delta)),
    );
    if let Some([a, b]) = label.underline {
        out.segments.push([to_world(a), to_world(b)]);
    }
    let bounds = label.bounds;
    out.label_box = Some([
        to_world(Point2::new(bounds.min.x, bounds.min.y)),
        to_world(Point2::new(bounds.max.x, bounds.min.y)),
        to_world(Point2::new(bounds.max.x, bounds.max.y)),
        to_world(Point2::new(bounds.min.x, bounds.max.y)),
    ]);
}

/// 直線的な寸法線を持つ寸法（長さ寸法・直径寸法）の共通展開。
///
/// `d1`→`d2` が寸法線、`dir` はその単位方向ベクトル。補助線は呼び出し側が
/// [`extension_line`] で作って `segments` へ足す（直径寸法には補助線がない）。
///
/// 配置規則は 3 点:
/// 1. 文字は寸法線の**上側**（[`reading_direction`] の `perp()` 正側）へ、
///    [`DimStyle::text_gap_mm`] のすきまを空けて置く（規定 5-2 3)）。
/// 2. 矢は [`arrows_point_outward`] の判定で内外を切り替える（規定 5-4 4)）。
///    外向きのときは矢が乗る線がなくなるので、寸法線を両端へ矢先長ぶん延長する。
/// 3. [`DimAnnotation::text_anchor`] が `Some` ならラベル外形の中心をその点へ置き、
///    1. の自動配置を上書きする。
fn expand_straight(
    d1: Point2,
    d2: Point2,
    dir: Vec2,
    value: f64,
    annotation: &DimAnnotation,
    kind: DimKind,
    render: DimRender<'_>,
) -> DimExpansion {
    let arrow_len = render.arrow_len_world;
    let outward = arrows_point_outward((d2 - d1).length(), value, annotation, kind, render);

    // 矢先の先端は常に寸法線の両端（`d1` / `d2`）。三角形の胴を内・外どちらへ出すかだけが
    // 変わる（`arrow_triangle` は先端から `-dir * len` の側へ胴を作る）。
    let (tail1, tail2) = if outward { (dir, -dir) } else { (-dir, dir) };
    let (line1, line2) = if outward {
        (d1 - dir * arrow_len, d2 + dir * arrow_len)
    } else {
        (d1, d2)
    };
    let mut ex = DimExpansion::new(
        vec![[line1, line2]],
        vec![
            arrow_triangle(d1, tail1, arrow_len),
            arrow_triangle(d2, tail2, arrow_len),
        ],
    );

    let label = layout_dim_label(
        value,
        annotation,
        render.style,
        kind,
        render.text_height_world,
    );
    let along = reading_direction(dir);
    let center = annotation.text_anchor.unwrap_or_else(|| {
        let gap = render.paper_mm_to_world(render.style.text_gap_mm);
        d1.midpoint(d2) + along.perp() * (gap + label.height() * 0.5)
    });
    place_label(&label, center, along, &mut ex);
    ex
}

// ---------------------------------------------------------------------
// 長さ寸法
// ---------------------------------------------------------------------

/// 長さ寸法の寸法線 2 端点 `(d1, d2)` と、寸法線方向の単位ベクトルを返す。
/// 計測 2 点がほぼ同一（法線が定まらない）なら `None`。
fn linear_frame(dim: &DimLinear) -> Option<(Point2, Point2, Vec2)> {
    let dir = (dim.p2 - dim.p1).normalize()?;
    let shift = dir.perp() * dim.offset;
    Some((dim.p1 + shift, dim.p2 + shift, dir))
}

/// 長さ寸法のヒットテスト用線分（寸法線＋補助線 2 本）。矢先・文字の大きさに依らず
/// 保存データ（p1/p2/offset）だけで決まるため、ズーム非依存で pick から使える。
/// 退化（p1≈p2）時は空。
#[must_use]
pub fn linear_pick_segments(dim: &DimLinear) -> Vec<[Point2; 2]> {
    match linear_frame(dim) {
        Some((d1, d2, _)) => vec![[d1, d2], [dim.p1, d1], [dim.p2, d2]],
        None => Vec::new(),
    }
}

/// クリック点 `p` から長さ寸法への最短距離（ヒットテスト用）。退化寸法は p1 への距離で
/// 代替し、選択・削除だけはできるようにする。
#[must_use]
pub fn linear_distance(dim: &DimLinear, p: Point2) -> f64 {
    let segs = linear_pick_segments(dim);
    if segs.is_empty() {
        p.distance(dim.p1)
    } else {
        min_distance_to_segments(&segs, p)
    }
}

/// 長さ寸法を展開する（規定 5-2 3)・5-4 2)・5-4 4)）。
///
/// - 寸法線は `d1`→`d2`。矢は [`arrows_point_outward`] の判定で内外が決まり、外向きの
///   ときは寸法線を両端へ矢先長ぶん延長する。
/// - 補助線は計測点から [`DimStyle::ext_gap_mm`] のすきまを空けて始まり、寸法線を
///   [`DimStyle::ext_overshoot_mm`] だけ越えて終わる（退化時の扱いは [`extension_line`]）。
/// - 値ラベルは寸法線の**上側**へ [`DimStyle::text_gap_mm`] のすきまで置く。
///   `offset` の符号では側を変えない（M9 タスク49-2 の意図した可視差）。
#[must_use]
pub fn expand_linear(dim: &DimLinear, render: DimRender<'_>) -> DimExpansion {
    // 退化時も破綻しない安全な既定方向（+x）を使う。通常はツールが p1≈p2 を弾く。
    let (d1, d2, dir) = linear_frame(dim).unwrap_or((dim.p1, dim.p2, Vec2::new(1.0, 0.0)));

    let value = (dim.p2 - dim.p1).length();
    let mut ex = expand_straight(d1, d2, dir, value, &dim.annotation, DimKind::Linear, render);

    let gap = render.paper_mm_to_world(render.style.ext_gap_mm);
    let overshoot = render.paper_mm_to_world(render.style.ext_overshoot_mm);
    ex.segments
        .extend(extension_line(dim.p1, d1, gap, overshoot));
    ex.segments
        .extend(extension_line(dim.p2, d2, gap, overshoot));
    ex
}

// ---------------------------------------------------------------------
// 半径寸法
// ---------------------------------------------------------------------

/// 引出方向の単位ベクトルと円周上の点 `pc`（中心から半径方向へ radius 進んだ点）。
fn radial_frame(dim: &DimRadial) -> (Vec2, Point2) {
    let dir = Vec2::new(dim.leader_angle.cos(), dim.leader_angle.sin());
    (dir, dim.center + dir * dim.radius)
}

/// 半径寸法のヒットテスト用線分（中心→円周点の半径線）。保存データ（center/radius/
/// leader_angle）だけで決まるためズーム非依存で pick から使える。描画の引出線も同じ
/// 線分なので、描画とヒットテストの位置が一致する。
#[must_use]
pub fn radial_pick_segments(dim: &DimRadial) -> Vec<[Point2; 2]> {
    let (_, pc) = radial_frame(dim);
    vec![[dim.center, pc]]
}

/// クリック点 `p` から半径寸法（引出線）への最短距離（ヒットテスト用）。
#[must_use]
pub fn radial_distance(dim: &DimRadial, p: Point2) -> f64 {
    min_distance_to_segments(&radial_pick_segments(dim), p)
}

/// 半径寸法を展開する。引出線は中心→円周点の半径線、矢先は円周点で外向き、値ラベルは
/// 円周点の少し外側へ水平配置する（DESIGN.md M6 設計判断2）。
///
/// **引出線・矢の形はタスク49-2 でも変えない**（可視差を増やさない）。変わるのは値の
/// 組版だけで、接頭辞 `R` は [`label::layout_dim_label`] が [`DimKind::Radial`] の既定記号
/// として供給する（ここで `R` を足すと二重に付く）。
#[must_use]
pub fn expand_radial(dim: &DimRadial, render: DimRender<'_>) -> DimExpansion {
    let (dir, pc) = radial_frame(dim);
    let arrow_len = render.arrow_len_world;
    let mut ex = DimExpansion::new(
        vec![[dim.center, pc]],
        vec![arrow_triangle(pc, dir, arrow_len)],
    );

    let label = layout_dim_label(
        dim.radius,
        &dim.annotation,
        render.style,
        DimKind::Radial,
        render.text_height_world,
    );
    // 文字は水平（angle 0）。円周点から矢先＋すきまぶん外側の点を近端にし、そこから
    // 引出向きに応じて左右へ伸ばす（右向き引出は右へ、左向きは左へ）。縦方向は近端に中央寄せ。
    let center = dim.annotation.text_anchor.unwrap_or_else(|| {
        let gap = render.paper_mm_to_world(render.style.text_gap_mm);
        let near = pc + dir * (arrow_len + gap);
        let side = if dir.x >= 0.0 { 1.0 } else { -1.0 };
        Point2::new(near.x + side * label.width() * 0.5, near.y)
    });
    place_label(&label, center, Vec2::new(1.0, 0.0), &mut ex);
    ex
}

// ---------------------------------------------------------------------
// 直径寸法
// ---------------------------------------------------------------------

/// 直径寸法の寸法線方向の単位ベクトルと、直径線の 2 端点 `(d1, d2)`。
///
/// `d1` は `angle` の**逆**方向側、`d2` は `angle` 方向側の円周点。
fn diameter_frame(dim: &DimDiameter) -> (Vec2, Point2, Point2) {
    let dir = Vec2::new(dim.angle.cos(), dim.angle.sin());
    let arm = dir * dim.radius;
    (dir, dim.center - arm, dim.center + arm)
}

/// 直径寸法のヒットテスト用線分（中心を通る直径線 1 本）。保存データ（center/radius/
/// angle）だけで決まるためズーム非依存で pick から使える。描画の寸法線も同じ線分なので、
/// 描画とヒットテストの位置が一致する（矢が外向きになると描画側の線は矢先長ぶん伸びるが、
/// **pick 形状は伸ばさない**。伸ばすとスタイル・表示状態でピック範囲が変わる）。
#[must_use]
pub fn diameter_pick_segments(dim: &DimDiameter) -> Vec<[Point2; 2]> {
    let (_, d1, d2) = diameter_frame(dim);
    vec![[d1, d2]]
}

/// クリック点 `p` から直径寸法（直径線）への最短距離（ヒットテスト用）。
#[must_use]
pub fn diameter_distance(dim: &DimDiameter, p: Point2) -> f64 {
    min_distance_to_segments(&diameter_pick_segments(dim), p)
}

/// 直径寸法を展開する（M9 設計判断3・タスク49-2 で新設）。
///
/// 寸法線は中心を通る `angle` 方向の直径線、矢先は円周 2 点。配置規則は長さ寸法と
/// 共通（[`expand_straight`]）で、値ラベルは直径線の中央・上側に載る。補助線はない
/// （寸法の相手は円そのもの）。記号 φ は [`label::layout_dim_label`] が
/// [`DimKind::Diameter`] の既定記号として供給する。
///
/// 表示値は**直径**（`2 * radius`）。[`DimDiameter::radius`] が半径で持たれているのは
/// 採取元の [`mcad_geom::Circle`] と同じ量にするためで、`2r` は表示のたびに導出する。
#[must_use]
pub fn expand_diameter(dim: &DimDiameter, render: DimRender<'_>) -> DimExpansion {
    let (dir, d1, d2) = diameter_frame(dim);
    expand_straight(
        d1,
        d2,
        dir,
        dim.radius * 2.0,
        &dim.annotation,
        DimKind::Diameter,
        render,
    )
}

// ---------------------------------------------------------------------
// ラベルの組版
// ---------------------------------------------------------------------

pub mod label {
    //! 注記つき寸法値の**組版エンジン**（M9 タスク49-1）。
    //!
    //! [`layout_dim_label`] が [`DimAnnotation`]（寸法補助記号・サイズ公差・桁数上書き・
    //! 非比例寸法の値上書き）と [`DimStyle`]（桁数・ゼロトリム・公差文字の縮小率）から、
    //! 描画可能な要素の集まり（[`DimLabel`]）へ組み上げる。
    //!
    //! # 単位は抽象（呼び出し側の単位でそのまま返す）
    //!
    //! 長さの入力は `text_height` ただ 1 つで、出力の座標・幅・高さはすべて**その同じ
    //! 単位**で返す。ワールド長を渡せばワールド長が、紙 mm を渡せば紙 mm が返る。
    //! [`DimStyle`] から読むのは**無次元の値だけ**（`decimals` / `trim_trailing_zeros` /
    //! `tolerance_scale`）で、紙 mm のフィールド（`text_height_mm` など）は読まない。
    //!
    //! これは M8 タスク37 の教訓（`DimExpansion` の文字高さはワールド長なのに
    //! [`TextGeom::height`](mcad_core::TextGeom::height) は紙 mm、という二重換算の罠）を
    //! 再発させないための契約である。換算はこのモジュールの外（`main.rs` の `dim_sizes`・
    //! `plot`）で一度だけ行う。
    //!
    //! この「単位は抽象」があるから、タスク49-2 の外向き矢印の自動判定は
    //! **同じ関数を紙 mm で呼び直す**だけでビュー非依存の幅を得られる
    //! （[`super::arrows_point_outward`]）。
    //!
    //! # 配置はしない
    //!
    //! 出力はすべて**ラベルのローカル座標**（原点 = 値のベースライン左端、+x = 読み方向、
    //! +y = 上）で、ワールドへの回転・平行移動は行わない
    //! （[`super::place_label`] が担う）。

    use mcad_core::{DimAnnotation, DimKind, DimStyle, SizeTolerance};
    use mcad_geom::{Aabb, DimSymbol, Point2, Shape, SymbolGlyph, Vec2, dim_symbol_glyph};

    use super::ASCII_CHAR_WIDTH_RATIO;

    /// 上下段公差（[`SizeTolerance::Deviations`]）の上段と下段の間に空ける縦すきま ÷
    /// 基準文字高さ。
    ///
    /// 規定 5-12-2 2) は「上公差を上・下公差を下の上下段に置き、文字を 70% に縮小する」
    /// ことだけを定め、行間の値は定めていない。上下段が触れ合わずに読め、かつ全高が
    /// 膨らみすぎない値として 0.1 を採る（既定の文字高さ 3.5mm で 0.35mm）。
    const DEVIATION_LINE_GAP_RATIO: f64 = 0.1;

    /// 非比例寸法（[`DimAnnotation::value_override`]）の下線をベースラインから下げる量 ÷
    /// 基準文字高さ。
    ///
    /// **規定 5-10「非比例寸法の表し方」は見出しのみで本文が空**のため、JIS B 0001 の
    /// 標準慣行（寸法数値の下に実線を 1 本引く）で実装している。規定側に本文が入った
    /// 時点で、この定数と [`DimLabel::underline`] の生成規則を突き合わせること。
    const UNDERLINE_DROP_RATIO: f64 = 0.15;

    /// 組版済みラベルの中の、フォント文字として描く 1 まとまり。
    ///
    /// `origin` は既存の [`TextGeom`](mcad_core::TextGeom) と同じ**ベースライン左端**
    /// アンカーなので、タスク49-2 はラベル全体の姿勢を掛けるだけで `TextGeom` へ移せる。
    #[derive(Debug, Clone, PartialEq)]
    pub struct TextRun {
        /// 描画する文字列。
        pub content: String,
        /// ベースライン左端（ラベルのローカル座標）。
        pub origin: Point2,
        /// 文字高さ（[`layout_dim_label`] の `text_height` と同じ単位）。
        /// 上下段公差だけは `text_height * DimStyle::tolerance_scale` になる。
        pub height: f64,
    }

    /// 組版済みの寸法ラベル。
    ///
    /// 座標はすべて**ラベルのローカル座標**（原点 = 値のベースライン左端、+x = 読み方向、
    /// +y = 上）で、長さの単位は [`layout_dim_label`] へ渡した `text_height` と同じ。
    ///
    /// # 外形を [`Aabb`] で持つ理由
    ///
    /// 上下段公差の下段と非比例寸法の下線は**ベースラインより下**へ出るため、原点は外形の
    /// 左下とは限らない。幅・高さの 2 スカラーだけでは「原点から見て外形がどこにあるか」が
    /// 失われ、タスク49-2 の中央寄せが下方向のはみ出しぶんだけずれる。幅・高さは
    /// [`DimLabel::width`] / [`DimLabel::height`] で取れる。
    ///
    /// 外形は**送り幅ベースの組版ボックス**（各要素について「送り幅 × 文字高さ」の矩形）で
    /// あり、ストロークの実インクの境界ではない。行の中央寄せ・外向き矢印の自動判定は
    /// インクではなく組版ボックスを基準にするのが正しいため。
    #[derive(Debug, Clone, PartialEq)]
    pub struct DimLabel {
        /// フォント文字として描く要素（値・英字記号・公差）。
        pub runs: Vec<TextRun>,
        /// ストロークとして描く記号（φ・□）。[`dim_symbol_glyph`] が返す形状を
        /// ラベルのローカル座標へ配置済み。
        pub strokes: Vec<Shape>,
        /// 非比例寸法（規定 5-10）の下線。`None` は下線なし。値の部分だけに掛かり、
        /// 記号・公差には掛からない。
        pub underline: Option<[Point2; 2]>,
        /// 外形（記号・値・公差・下線をすべて含む組版ボックス）。
        pub bounds: Aabb,
    }

    impl DimLabel {
        /// 外形の幅。
        #[must_use]
        pub fn width(&self) -> f64 {
            self.bounds.width()
        }

        /// 外形の高さ（上下段公差・下線を含む全高）。
        #[must_use]
        pub fn height(&self) -> f64 {
            self.bounds.height()
        }
    }

    /// 注記つきの寸法値を組版する。
    ///
    /// `value` は座標から計算した実測値を呼び出し側が渡す（半径寸法なら半径、直径寸法なら
    /// 直径 `2r`）。`text_height` は基準文字高さで、**出力はすべてこれと同じ単位**になる
    /// （モジュール doc の「単位は抽象」を参照）。
    ///
    /// # 組版規則（出典は `製図規定.md`）
    ///
    /// - **桁数**（5-2 2)）: [`DimStyle::resolve_decimals`] で解決する。
    ///   [`DimAnnotation::decimals_override`] が `Some(n)` ならそれ、`None` なら
    ///   [`DimStyle::decimals`]（文書スタイルへの生きた参照）。
    /// - **ゼロトリム**: [`DimStyle::trim_trailing_zeros`] が真なら末尾の 0 と、
    ///   結果として余る小数点を落とす（`120.00` → `120`、`12.50` → `12.5`）。
    /// - **値の差し替え**（5-10）: [`DimAnnotation::value_override`] が `Some(s)` なら
    ///   数値の代わりに `s` を使い、値の幅ぶんの下線を引く。
    /// - **寸法補助記号**（5-2 4)）: 数値の**左**へ置く。[`DimAnnotation::symbol`] が
    ///   `None` のときは寸法種別で補う（長さ = なし / 半径 = R / 直径 = φ。表示時の補完
    ///   であって core のデータには書かない）。
    /// - **公差**（5-12）: 対称許容差は `± v` を同じ文字高さで値の右へ（5-12-2 1)）、
    ///   上下偏差は値の右に上下 2 段で [`DimStyle::tolerance_scale`] 倍に縮小して
    ///   （5-12-2 2)・3)）、はめあい記号は値の直後へ隙間なく連結して（5-12-3 2)）置く。
    ///
    /// # 未知バリアントの扱い
    ///
    /// [`SizeTolerance`] は `#[non_exhaustive]` なので、将来の表記方式は
    /// **「公差なし」として値だけを組む**（ワイルドカード腕）。同様に [`DimSymbol`] へ
    /// 追加された未対応の記号は、送り幅だけ確保して文字・ストロークを出さない。
    /// どちらも黙って描画が壊れるより保守的な既定だが、**追加時にここを更新しないと
    /// 図面から情報が落ちる**。単体テスト `every_known_symbol_is_typeset` が既知記号の
    /// 取りこぼしを検出する。
    #[must_use]
    pub fn layout_dim_label(
        value: f64,
        annotation: &DimAnnotation,
        style: &DimStyle,
        kind: DimKind,
        text_height: f64,
    ) -> DimLabel {
        let decimals = style.resolve_decimals(annotation.decimals_override);
        let trim = style.trim_trailing_zeros;
        let mut builder = LabelBuilder::new();

        // 1) 寸法補助記号は数値の左（規定 5-2 4)）。
        if let Some(symbol) = annotation.symbol.or_else(|| default_symbol(kind)) {
            builder.push_symbol(symbol, text_height);
        }

        // 2) 値。非比例寸法は数値を差し替えて下線を引く（規定 5-10）。
        let value_x = builder.cursor;
        let value_text = match &annotation.value_override {
            Some(text) => text.clone(),
            None => format_number(value, decimals, trim),
        };
        let value_width = builder.push_run(value_text, text_height, 0.0);
        let mut underline = None;
        if annotation.value_override.is_some() {
            let y = -text_height * UNDERLINE_DROP_RATIO;
            builder.include(value_x, y, value_x + value_width, y);
            underline = Some([
                Point2::new(value_x, y),
                Point2::new(value_x + value_width, y),
            ]);
        }

        // 3) サイズ公差（規定 5-12）。
        if let Some(tolerance) = &annotation.tolerance {
            // 値と公差の間は ASCII 空白 1 文字ぶん空ける（規定 5-12-2 1) の記入例
            // `50 ± 0.5` の空白に相当。独自の定数を作らず文字幅係数をそのまま使う）。
            let space = ASCII_CHAR_WIDTH_RATIO * text_height;
            match tolerance {
                // 5-12-2 1): ± と公差値は寸法数値と同じ高さ。
                SizeTolerance::Symmetric(v) => {
                    builder.advance(space);
                    let text = format!("± {}", format_number(*v, decimals, trim));
                    builder.push_run(text, text_height, 0.0);
                }
                // 5-12-2 2)・3): 上下段、公差文字は tolerance_scale 倍に縮小。
                SizeTolerance::Deviations { upper, lower } => {
                    builder.advance(space);
                    builder.push_deviations(
                        &format_deviation(*upper, decimals, trim),
                        &format_deviation(*lower, decimals, trim),
                        text_height,
                        style.tolerance_scale,
                    );
                }
                // 5-12-3 2): はめあい記号は基準寸法の直後（φ20H7）。表示のみで偏差値へ
                // 展開しない（core の `FitClass` が検証済みの文字列をそのまま出す）。
                SizeTolerance::Fit(fit) => {
                    builder.push_run(fit.as_str().to_string(), text_height, 0.0);
                }
                // `#[non_exhaustive]` への追加は「公差なし」へ倒す（関数 doc 参照）。
                _ => {}
            }
        }

        DimLabel {
            runs: builder.runs,
            strokes: builder.strokes,
            underline,
            bounds: builder.bounds,
        }
    }

    /// 記号が明示されていないとき（[`DimAnnotation::symbol`] が `None`）に、寸法種別から
    /// 補う既定の記号。
    ///
    /// 半径・直径寸法は記号がないと値の意味が決まらない（R12 と φ24 の取り違えを招く。
    /// M8 までの `expand_radial` も `R` を値文字列へ焼き込んでいた）。長さ寸法は記号なしが
    /// 既定。
    ///
    /// **これは表示時の補完であり、core のデータへは書かない。** 書いてしまうと「未指定」と
    /// 「明示的に R を選んだ」が区別できなくなり、`decimals_override` で避けたのと同じ
    /// 「生きた参照が死ぬ」問題を起こす。
    ///
    /// [`DimKind`] は `#[non_exhaustive]` ではない（種別追加をコンパイルエラーで検出する
    /// 設計）ので、ここも網羅 `match` で書く。
    fn default_symbol(kind: DimKind) -> Option<DimSymbol> {
        match kind {
            DimKind::Linear => None,
            DimKind::Radial => Some(DimSymbol::Radius),
            DimKind::Diameter => Some(DimSymbol::Diameter),
        }
    }

    /// フォント文字として組む記号の文字列。
    ///
    /// ストロークを生成する記号（φ・□）と複合記号 Sφ は [`LabelBuilder::push_symbol`] が
    /// 先に処理するので、ここへは来ない。`None` は「[`DimSymbol`] に追加されたが組版側が
    /// 未対応」の意味（[`DimSymbol`] は `#[non_exhaustive]`）。
    fn symbol_font_text(symbol: DimSymbol) -> Option<&'static str> {
        match symbol {
            DimSymbol::Radius => Some("R"),
            DimSymbol::SphereRadius => Some("SR"),
            DimSymbol::ControlRadius => Some("CR"),
            DimSymbol::Chamfer => Some("C"),
            DimSymbol::Thickness => Some("t"),
            _ => None,
        }
    }

    /// 数値を桁数 `decimals` で文字列化し、`trim_trailing_zeros` なら末尾の 0 と、
    /// 結果として余る小数点を落とす（`120.00` → `120`、`12.50` → `12.5`）。
    ///
    /// 小数点を含まないとき（`decimals` が 0）はトリムしない。`120` から末尾の 0 を
    /// 落として `12` にしてしまわないための境界である。
    fn format_number(value: f64, decimals: u8, trim_trailing_zeros: bool) -> String {
        let precision = usize::from(decimals);
        let mut text = format!("{value:.precision$}");
        if trim_trailing_zeros && text.contains('.') {
            let kept = text.trim_end_matches('0').trim_end_matches('.').len();
            text.truncate(kept);
        }
        text
    }

    /// 上下偏差の 1 段ぶんを符号つきで文字列化する（`+0.2` / `-0.1`）。
    ///
    /// 桁数・ゼロトリムは寸法値と同じ解決結果を使う。**0 には符号を付けない**（片側公差
    /// の記入例 `+0.3` / `0`。規定 5-12-2 3)）。判定は生値ではなく整形後の文字列で行う
    /// ので、桁数の丸めで 0 になった値も `+0.00` ではなく `0.00` になる。
    fn format_deviation(value: f64, decimals: u8, trim_trailing_zeros: bool) -> String {
        let magnitude = format_number(value.abs(), decimals, trim_trailing_zeros);
        if is_zero_text(&magnitude) {
            magnitude
        } else if value < 0.0 {
            format!("-{magnitude}")
        } else {
            format!("+{magnitude}")
        }
    }

    /// [`format_number`] の出力が 0 を表すか（`"0"` / `"0.00"`）。
    fn is_zero_text(text: &str) -> bool {
        !text.is_empty() && text.chars().all(|c| c == '0' || c == '.')
    }

    /// 左から右へ 1 行ぶんを積み上げる組版カーソル。
    ///
    /// 要素を置くたびに外形へ「送り幅 × 文字高さ」のセルを取り込むので、[`DimLabel::bounds`]
    /// は常に実際に置いた内容と一致する（幅・高さを別計算しない）。
    struct LabelBuilder {
        runs: Vec<TextRun>,
        strokes: Vec<Shape>,
        /// 次の要素を置く左端（ローカル x）。
        cursor: f64,
        bounds: Aabb,
    }

    impl LabelBuilder {
        fn new() -> Self {
            Self {
                runs: Vec::new(),
                strokes: Vec::new(),
                cursor: 0.0,
                // 原点（ベースライン左端）は常に外形に含まれる。最初の要素は x = 0 から
                // 始まり、ベースラインより上へ伸びるので、これで外形が膨らむことはない。
                bounds: Aabb::from_point(Point2::ORIGIN),
            }
        }

        /// 外形へ矩形セルを取り込む。
        fn include(&mut self, x0: f64, y0: f64, x1: f64, y1: f64) {
            let cell = Aabb::new(Point2::new(x0, y0), Point2::new(x1, y1));
            self.bounds = self.bounds.union(&cell);
        }

        /// 文字を伴わない送り（すきま）。
        fn advance(&mut self, dx: f64) {
            self.cursor += dx;
        }

        /// 文字列の近似幅（[`ASCII_CHAR_WIDTH_RATIO`] × 文字数 × 高さ）。
        fn run_width(content: &str, height: f64) -> f64 {
            content.chars().count() as f64 * ASCII_CHAR_WIDTH_RATIO * height
        }

        /// 送り位置を動かさずに 1 ランを置き、その幅を返す。
        fn place_run(&mut self, x: f64, content: String, height: f64, baseline: f64) -> f64 {
            let width = Self::run_width(&content, height);
            self.include(x, baseline, x + width, baseline + height);
            self.runs.push(TextRun {
                content,
                origin: Point2::new(x, baseline),
                height,
            });
            width
        }

        /// 現在の送り位置へ 1 ランを置き、その幅ぶん送る。戻り値はランの幅。
        fn push_run(&mut self, content: String, height: f64, baseline: f64) -> f64 {
            let width = self.place_run(self.cursor, content, height, baseline);
            self.cursor += width;
            width
        }

        /// 幅 `advance`・高さ `height` の記号セルを外形へ取り込み、送り位置を進める。
        fn reserve_symbol_cell(&mut self, advance: f64, height: f64) {
            self.include(self.cursor, 0.0, self.cursor + advance, height);
            self.cursor += advance;
        }

        /// ストローク記号を現在位置へ置き、[`SymbolGlyph::advance`] ぶん送る。
        fn push_symbol_strokes(&mut self, glyph: &SymbolGlyph, height: f64) {
            let delta = Vec2::new(self.cursor, 0.0);
            self.strokes
                .extend(glyph.shapes.iter().map(|shape| shape.translated(delta)));
            self.reserve_symbol_cell(glyph.advance, height);
        }

        /// フォント文字の記号を現在位置へ置き、`advance` ぶん送る。
        ///
        /// 送り幅を文字数から計算せず [`SymbolGlyph::advance`] に従うのは、記号の幅の
        /// 出所を mcad-geom 側へ一本化するためである（`ASCII_CHAR_WIDTH_RATIO` と
        /// `symbol::FONT_CHAR_ADVANCE_RATIO` は同じ量なので現状は一致するが、二重に
        /// 計算していると片方だけ変えたときに黙ってずれる）。外形はランの近似幅と
        /// `advance` の両方を取り込むので、仮に食い違っても内容を覆い続ける。
        fn push_symbol_text(&mut self, content: &str, advance: f64, height: f64) {
            self.place_run(self.cursor, content.to_string(), height, 0.0);
            self.reserve_symbol_cell(advance, height);
        }

        /// 寸法補助記号 1 つを現在位置へ組む。
        ///
        /// ストロークかフォント文字かの振り分けは **`match` の腕ではなく
        /// [`SymbolGlyph::shapes`] の空 / 非空**で行う（`DimSymbol` は
        /// `#[non_exhaustive]` なので、ワイルドカード腕でフォント文字へ倒すと将来
        /// ストローク生成が必要な記号が追加されたとき黙って崩れる。空 = 「フォント文字と
        /// して組め」は mcad-geom の `symbol` モジュール doc が定める契約）。
        fn push_symbol(&mut self, symbol: DimSymbol, height: f64) {
            let glyph = dim_symbol_glyph(symbol, height);
            if !glyph.shapes.is_empty() {
                self.push_symbol_strokes(&glyph, height);
                return;
            }
            if symbol == DimSymbol::SphereDiameter {
                // Sφ だけは「フォント文字 S + ストローク φ」の複合で、geom は shapes を
                // 空にしたまま advance に両者の合算を返す。S の送りをその合算から φ の
                // advance を引いて求めることで、合計を必ず geom の advance に一致させる。
                let phi = dim_symbol_glyph(DimSymbol::Diameter, height);
                let s_advance = (glyph.advance - phi.advance).max(0.0);
                self.push_symbol_text("S", s_advance, height);
                self.push_symbol_strokes(&phi, height);
                return;
            }
            match symbol_font_text(symbol) {
                Some(text) => self.push_symbol_text(text, glyph.advance, height),
                // 未対応の記号。文字は出せないが送りだけは geom の advance どおり確保し、
                // 外形（＝タスク49-2 の中央寄せ・矢印判定の入力）が実際の描画とずれない
                // ようにする。
                None => self.reserve_symbol_cell(glyph.advance, height),
            }
        }

        /// 上下段公差を現在位置へ組み、広いほうの幅ぶん送る（規定 5-12-2 2)）。
        ///
        /// 2 段は左揃えで、上下段の中間が寸法数値の中央高さ（`text_height / 2`）へ来るよう
        /// 対称に置く。上段・下段の文字高さはどちらも `text_height * tolerance_scale`。
        fn push_deviations(
            &mut self,
            upper: &str,
            lower: &str,
            text_height: f64,
            tolerance_scale: f64,
        ) {
            let height = text_height * tolerance_scale;
            let gap = text_height * DEVIATION_LINE_GAP_RATIO;
            let middle = text_height * 0.5;
            let x = self.cursor;
            let upper_width = self.place_run(x, upper.to_string(), height, middle + gap * 0.5);
            let lower_width =
                self.place_run(x, lower.to_string(), height, middle - gap * 0.5 - height);
            self.cursor += upper_width.max(lower_width);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::label::{DimLabel, TextRun, layout_dim_label};
    use super::*;
    use mcad_core::{DimAnnotation, DimKind, DimStyle, FitClass, SizeTolerance};
    use mcad_geom::{Aabb, DimSymbol, dim_symbol_glyph};
    use std::f64::consts::{FRAC_PI_2, PI};

    const T: f64 = 1e-9;

    fn approx(a: Point2, b: Point2) -> bool {
        a.distance(b) < 1e-6
    }

    // --- 展開テストの共通道具 ---

    /// 展開テストの既定スタイル。[`DimRender`] が参照で持つので `static` に置く。
    static DEFAULT_STYLE: DimStyle = DimStyle::DEFAULT;

    /// 展開テスト用の [`DimRender`]（既定スタイル・尺度 1:1）。`arrow_len_world` /
    /// `text_height_world` は M8 までの展開テストが使ってきた 0.5 / 1.0 をそのまま渡す。
    fn render(arrow_len_world: f64, text_height_world: f64) -> DimRender<'static> {
        DimRender {
            style: &DEFAULT_STYLE,
            scale_world_per_paper_mm: 1.0,
            arrow_len_world,
            text_height_world,
        }
    }

    /// [`render`] が作るパラメータでの「紙 mm → ワールド長」換算
    /// （＝ `text_height_world / DimStyle::text_height_mm`）。
    fn mm_to_world(mm: f64) -> f64 {
        mm / DimStyle::DEFAULT.text_height_mm
    }

    /// 矢が外向きか（先端から胴への向きが寸法線 `dir` の外側か）を展開結果から読む。
    fn arrows_are_outward(ex: &DimExpansion, dir: Vec2) -> bool {
        let [tip, a, b] = ex.arrows[0];
        (a.midpoint(b) - tip).dot(dir) < 0.0
    }

    /// 展開結果のフォント文字ラン（φ・□ のストロークは含まない）。
    fn contents_of(ex: &DimExpansion) -> Vec<&str> {
        ex.texts.iter().map(|t| t.content.as_str()).collect()
    }

    fn plain_linear(p1: Point2, p2: Point2, offset: f64) -> DimLinear {
        DimLinear {
            p1,
            p2,
            offset,
            annotation: DimAnnotation::default(),
        }
    }

    // --- 長さ寸法 ---

    #[test]
    fn linear_pick_segments_offset_positive_side() {
        // 水平な計測 (0,0)-(4,0)、offset +2 → 寸法線は y=2 側（法線 = (0,1)）。
        let dim = plain_linear(Point2::new(0.0, 0.0), Point2::new(4.0, 0.0), 2.0);
        let segs = linear_pick_segments(&dim);
        assert_eq!(segs.len(), 3);
        // 寸法線。
        assert!(approx(segs[0][0], Point2::new(0.0, 2.0)));
        assert!(approx(segs[0][1], Point2::new(4.0, 2.0)));
        // 補助線（計測点 → 寸法線端）。**すきま・突き出しは pick 形状へ入れない**
        // （スタイル・表示状態でピック範囲が変わらないようにするため。モジュール doc）。
        assert!(approx(segs[1][0], Point2::new(0.0, 0.0)));
        assert!(approx(segs[1][1], Point2::new(0.0, 2.0)));
        assert!(approx(segs[2][0], Point2::new(4.0, 0.0)));
        assert!(approx(segs[2][1], Point2::new(4.0, 2.0)));
    }

    #[test]
    fn linear_offset_sign_flips_dimension_line_side() {
        // 同じ計測で offset の符号を反転すると寸法線が反対側（y=-2）へ出る。
        let dim = plain_linear(Point2::new(0.0, 0.0), Point2::new(4.0, 0.0), -2.0);
        let segs = linear_pick_segments(&dim);
        assert!(approx(segs[0][0], Point2::new(0.0, -2.0)));
        assert!(approx(segs[0][1], Point2::new(4.0, -2.0)));
    }

    #[test]
    fn linear_degenerate_has_no_segments_but_distance_falls_back() {
        // p1≈p2 は法線が定まらず線分なし。距離は p1 への距離で代替（選択可能に保つ）。
        let dim = plain_linear(Point2::new(1.0, 1.0), Point2::new(1.0, 1.0), 3.0);
        assert!(linear_pick_segments(&dim).is_empty());
        assert!((linear_distance(&dim, Point2::new(1.0, 4.0)) - 3.0).abs() < T);
    }

    #[test]
    fn linear_distance_is_zero_on_dimension_line() {
        let dim = plain_linear(Point2::new(0.0, 0.0), Point2::new(4.0, 0.0), 2.0);
        // 寸法線 (0,2)-(4,2) 上の点は距離 0。
        assert!(linear_distance(&dim, Point2::new(2.0, 2.0)) < T);
        // 補助線 (0,0)-(0,2) 上の点も距離 0。
        assert!(linear_distance(&dim, Point2::new(0.0, 1.0)) < T);
        // 離れた点は正の距離（寸法線から 1）。
        assert!((linear_distance(&dim, Point2::new(2.0, 3.0)) - 1.0).abs() < T);
    }

    #[test]
    fn expand_linear_value_and_arrow_count() {
        let dim = plain_linear(Point2::new(0.0, 0.0), Point2::new(3.0, 4.0), 1.0); // 長さ 5
        let ex = expand_linear(&dim, render(0.5, 1.0));
        // 寸法線 1 + 補助線 2（非比例寸法の下線はないので 3 本）。
        assert_eq!(ex.segments.len(), 3);
        assert_eq!(ex.arrows.len(), 2);
        // 既定スタイルはゼロトリム ON なので `5.00` ではなく `5`（M9 判断5 (a)）。
        assert_eq!(contents_of(&ex), ["5"]);
    }

    #[test]
    fn expand_linear_arrow_tips_sit_on_dimension_line_ends() {
        let dim = plain_linear(Point2::new(0.0, 0.0), Point2::new(4.0, 0.0), 2.0);
        let ex = expand_linear(&dim, render(0.5, 1.0));
        // 矢先の先端（各三角形の第 1 頂点）は、内外配置に関わらず寸法線の両端。
        assert!(approx(ex.arrows[0][0], Point2::new(0.0, 2.0)));
        assert!(approx(ex.arrows[1][0], Point2::new(4.0, 2.0)));
    }

    #[test]
    fn expand_linear_puts_the_label_above_the_line_for_either_offset_sign() {
        // 規定 5-2 3) a)。M8 までは `offset` の符号側へ置いていた（＝下側にも回った）が、
        // タスク49-2 からは常に読み方向の上側（水平寸法線なら上）へ置く。
        let up = Vec2::new(0.0, 1.0);
        for offset in [2.0, -2.0] {
            let dim = plain_linear(Point2::new(0.0, 0.0), Point2::new(4.0, 0.0), offset);
            let ex = expand_linear(&dim, render(0.5, 1.0));
            // 水平ベースライン（角 0）。
            assert!(ex.texts[0].angle.abs() < T);
            let line_y = offset;
            assert!(
                (ex.texts[0].anchor - Point2::new(0.0, line_y)).dot(up) > 0.0,
                "offset {offset} でラベルが寸法線の下へ回った"
            );
        }
    }

    #[test]
    fn expand_linear_flips_baseline_for_leftward_measure() {
        // 右→左の計測（dir.x<0）はベースラインを反転して読みやすい向き（角 0）にする。
        let dim = plain_linear(Point2::new(4.0, 0.0), Point2::new(0.0, 0.0), 2.0);
        let ex = expand_linear(&dim, render(0.5, 1.0));
        assert!(ex.texts[0].angle.abs() < T);
    }

    #[test]
    fn expand_linear_vertical_measure_reads_bottom_up_and_sits_on_the_left() {
        // 規定 5-2 3) a)（垂直寸法線は左側）。上向き・下向きどちらの計測でも、読み方向は
        // 下→上（角 +π/2）に正規化され、その `perp()` 正側 ＝ 寸法線の左側へ文字が来る。
        for (p1, p2) in [
            (Point2::new(0.0, 0.0), Point2::new(0.0, 4.0)),
            (Point2::new(0.0, 4.0), Point2::new(0.0, 0.0)),
        ] {
            for offset in [2.0, -2.0] {
                let dim = plain_linear(p1, p2, offset);
                let ex = expand_linear(&dim, render(0.5, 1.0));
                assert!(
                    (ex.texts[0].angle - FRAC_PI_2).abs() < T,
                    "{p1:?}->{p2:?} の読み方向が下→上でない"
                );
                // 寸法線は x = ±2 のどちらか（offset の符号 × 法線）。文字は寸法線の
                // 左（x が小さい側）で、offset の符号では側が変わらない。
                let line_x = ex.segments[0][0].x;
                assert!(
                    ex.texts[0].anchor.x < line_x,
                    "{p1:?}->{p2:?} / offset {offset} で文字が寸法線の左に来ていない"
                );
            }
        }
    }

    #[test]
    fn expand_linear_diagonal_label_reads_from_lower_left_to_upper_right() {
        // 規定 5-2 3) b)。どちら向きに計測しても読み方向は同じで、文字は寸法線の上側。
        for (p1, p2) in [
            (Point2::new(0.0, 0.0), Point2::new(3.0, 4.0)),
            (Point2::new(3.0, 4.0), Point2::new(0.0, 0.0)),
        ] {
            for offset in [1.0, -1.0] {
                let dim = plain_linear(p1, p2, offset);
                let ex = expand_linear(&dim, render(0.5, 1.0));
                let angle = ex.texts[0].angle;
                assert!(
                    angle > 0.0 && angle < FRAC_PI_2,
                    "{p1:?}->{p2:?}: angle {angle}"
                );
                // 上側 = 読み方向の perp 正側（offset の符号では側が変わらない）。
                let up = Vec2::new(angle.cos(), angle.sin()).perp();
                let mid = ex.segments[0][0].midpoint(ex.segments[0][1]);
                assert!(
                    (ex.texts[0].anchor - mid).dot(up) > 0.0,
                    "{p1:?}->{p2:?} / offset {offset}"
                );
            }
        }
    }

    #[test]
    fn linear_extension_lines_have_the_style_gap_and_overshoot() {
        // 規定 5-4 2) b) c)（すきま 1mm・突き出し 2mm を紙 mm で持ち、注記倍率で換算）。
        let gap = mm_to_world(DimStyle::DEFAULT.ext_gap_mm);
        let over = mm_to_world(DimStyle::DEFAULT.ext_overshoot_mm);
        assert!(gap > 0.0 && over > gap);

        let dim = plain_linear(Point2::new(0.0, 0.0), Point2::new(4.0, 0.0), 2.0);
        let ex = expand_linear(&dim, render(0.5, 1.0));
        assert_eq!(ex.segments.len(), 3);
        assert!(approx(ex.segments[1][0], Point2::new(0.0, gap)));
        assert!(approx(ex.segments[1][1], Point2::new(0.0, 2.0 + over)));
        assert!(approx(ex.segments[2][0], Point2::new(4.0, gap)));
        assert!(approx(ex.segments[2][1], Point2::new(4.0, 2.0 + over)));

        // 反対側（offset 負）も計測点 → 寸法線の向きへ同じだけ空け、同じだけ突き出す。
        let dim = plain_linear(Point2::new(0.0, 0.0), Point2::new(4.0, 0.0), -2.0);
        let ex = expand_linear(&dim, render(0.5, 1.0));
        assert!(approx(ex.segments[1][0], Point2::new(0.0, -gap)));
        assert!(approx(ex.segments[1][1], Point2::new(0.0, -2.0 - over)));
    }

    #[test]
    fn linear_extension_lines_vanish_when_the_offset_is_within_the_gap() {
        // すきまだけで寸法線に届いてしまう配置では補助線を引かない（`extension_line` の doc）。
        let gap = mm_to_world(DimStyle::DEFAULT.ext_gap_mm);
        for offset in [0.0, gap * 0.5, gap] {
            let dim = plain_linear(Point2::new(0.0, 0.0), Point2::new(4.0, 0.0), offset);
            let ex = expand_linear(&dim, render(0.5, 1.0));
            assert_eq!(ex.segments.len(), 1, "offset {offset} で補助線が残った");
        }
        // すきまを超えた瞬間から引かれる。
        let dim = plain_linear(Point2::new(0.0, 0.0), Point2::new(4.0, 0.0), gap * 2.0);
        assert_eq!(expand_linear(&dim, render(0.5, 1.0)).segments.len(), 3);
    }

    #[test]
    fn expand_linear_underline_of_a_non_proportional_value_goes_into_segments() {
        // 規定 5-10。下線は寸法線と同じストロークなので `segments` に混ぜる（型の doc）。
        let dim = DimLinear {
            p1: Point2::new(0.0, 0.0),
            p2: Point2::new(4.0, 0.0),
            offset: 2.0,
            annotation: DimAnnotation {
                value_override: Some("50".to_string()),
                ..DimAnnotation::default()
            },
        };
        let ex = expand_linear(&dim, render(0.5, 1.0));
        assert_eq!(contents_of(&ex), ["50"]);
        // 寸法線 + 下線 + 補助線 2。
        assert_eq!(ex.segments.len(), 4);
        let [a, b] = ex.segments[1];
        assert!((a.y - b.y).abs() < T, "下線は寸法線と平行");
        assert!(a.y < ex.texts[0].anchor.y, "下線は値のベースラインより下");
    }

    #[test]
    fn expand_linear_centres_the_label_by_its_bounds_not_its_width() {
        // 上下段公差の下段はベースラインより下へ出る。外形で中央寄せしていれば
        // **ラベル全体の下端**が寸法線から text_gap だけ離れる（幅と文字高さだけで
        // 中央寄せすると下段が寸法線側へはみ出す）。
        let dim = DimLinear {
            p1: Point2::new(0.0, 0.0),
            p2: Point2::new(40.0, 0.0),
            offset: 2.0,
            annotation: DimAnnotation {
                tolerance: Some(SizeTolerance::Deviations {
                    upper: 0.2,
                    lower: -0.1,
                }),
                ..DimAnnotation::default()
            },
        };
        let ex = expand_linear(&dim, render(0.5, 1.0));
        let gap = mm_to_world(DimStyle::DEFAULT.text_gap_mm);
        let lowest = ex
            .texts
            .iter()
            .map(|t| t.anchor.y)
            .fold(f64::INFINITY, f64::min);
        assert!((lowest - (2.0 + gap)).abs() < T, "{lowest}");
        // 値そのもののベースラインは、下段のぶんだけ持ち上がっている。
        assert!(ex.texts[0].anchor.y > 2.0 + gap + T);
    }

    #[test]
    fn expand_linear_text_anchor_override_places_the_label_bounds_centre() {
        // タスク47 で入ったデータ（手動の文字位置）に展開が従う。基準点は**外形の中心**
        // （自動配置が置くのと同じ点）なので、自動 ⇄ 手動の切り替えで飛ばない。
        let anchor = Point2::new(-10.0, 7.0);
        let annotation = DimAnnotation {
            text_anchor: Some(anchor),
            ..DimAnnotation::default()
        };
        let dim = DimLinear {
            p1: Point2::new(0.0, 0.0),
            p2: Point2::new(4.0, 0.0),
            offset: 2.0,
            annotation: annotation.clone(),
        };
        let ex = expand_linear(&dim, render(0.5, 1.0));
        let label = layout_dim_label(4.0, &annotation, &DimStyle::DEFAULT, DimKind::Linear, 1.0);
        let c = label.bounds.center();
        // 読み方向は水平なので、外形中心を anchor へ置いた分だけ原点が左下へずれる。
        assert!(approx(
            ex.texts[0].anchor,
            Point2::new(anchor.x - c.x, anchor.y - c.y)
        ));
    }

    // --- 矢の内外配置（規定 5-4 4)・M9 設計判断5） ---

    #[test]
    fn arrow_placement_turns_outward_when_the_label_does_not_fit() {
        let dir = Vec2::new(1.0, 0.0);
        let arrow_len = 0.5;

        // 100mm の寸法線には「100」+ 矢 2 つ（各 3mm）が余裕で収まる → 内向き。
        let long = plain_linear(Point2::new(0.0, 0.0), Point2::new(100.0, 0.0), 2.0);
        let ex = expand_linear(&long, render(arrow_len, 1.0));
        assert!(!arrows_are_outward(&ex, dir));
        assert!(approx(ex.segments[0][0], Point2::new(0.0, 2.0)));
        assert!(approx(ex.segments[0][1], Point2::new(100.0, 2.0)));

        // 5mm では「5」+ 矢 2 つ（6mm）が収まらない → 外向き。矢先の先端は寸法線の端の
        // ままで、胴が外へ出る。矢が乗る線がなくなるので寸法線を両端へ矢先長ぶん延長する。
        let short = plain_linear(Point2::new(0.0, 0.0), Point2::new(5.0, 0.0), 2.0);
        let ex = expand_linear(&short, render(arrow_len, 1.0));
        assert!(arrows_are_outward(&ex, dir));
        assert!(approx(ex.arrows[0][0], Point2::new(0.0, 2.0)));
        assert!(approx(ex.arrows[1][0], Point2::new(5.0, 2.0)));
        assert!(approx(ex.segments[0][0], Point2::new(-arrow_len, 2.0)));
        assert!(approx(ex.segments[0][1], Point2::new(5.0 + arrow_len, 2.0)));
    }

    #[test]
    fn arrow_placement_decision_is_independent_of_zoom_and_paper_display() {
        // **必須の不変条件**: 判定は紙 mm だけで行い、表示状態（F9）・ズームで変わらない。
        // 変わると画面と SVG/PDF 出力が食い違う（M9 検収基準）。
        let dir = Vec2::new(1.0, 0.0);
        let style = DimStyle::DEFAULT;
        let k = 1.0;
        let fits = plain_linear(Point2::new(0.0, 0.0), Point2::new(100.0, 0.0), 2.0);
        let does_not_fit = plain_linear(Point2::new(0.0, 0.0), Point2::new(5.0, 0.0), 2.0);

        for paper_display in [false, true] {
            for zoom in [0.05, 1.0, 37.0, 500.0] {
                let r = crate::dim_render(&style, paper_display, k, zoom);
                assert!(
                    !arrows_are_outward(&expand_linear(&fits, r), dir),
                    "paper_display {paper_display} / zoom {zoom}"
                );
                assert!(
                    arrows_are_outward(&expand_linear(&does_not_fit, r), dir),
                    "paper_display {paper_display} / zoom {zoom}"
                );
            }
        }
    }

    #[test]
    fn manual_arrow_placement_overrides_the_automatic_decision() {
        let dir = Vec2::new(1.0, 0.0);
        let with = |placement, p2x: f64| DimLinear {
            p1: Point2::new(0.0, 0.0),
            p2: Point2::new(p2x, 0.0),
            offset: 2.0,
            annotation: DimAnnotation {
                arrow_placement: placement,
                ..DimAnnotation::default()
            },
        };
        // 自動なら外向きになる短い寸法を、手動 Inside で内向きへ戻す。
        let dim = with(ArrowPlacement::Inside, 5.0);
        let ex = expand_linear(&dim, render(0.5, 1.0));
        assert!(!arrows_are_outward(&ex, dir));
        assert!(
            approx(ex.segments[0][1], Point2::new(5.0, 2.0)),
            "延長しない"
        );

        // 自動なら内向きになる長い寸法を、手動 Outside で外向きにする。
        let dim = with(ArrowPlacement::Outside, 100.0);
        let ex = expand_linear(&dim, render(0.5, 1.0));
        assert!(arrows_are_outward(&ex, dir));
        assert!(approx(ex.segments[0][1], Point2::new(100.5, 2.0)));
    }

    // --- 半径寸法 ---

    #[test]
    fn radial_pick_segment_is_radius_line() {
        // 中心原点・半径 5・引出角 0 → 円周点 (5,0)。引出線は (0,0)-(5,0)。
        let dim = DimRadial {
            center: Point2::ORIGIN,
            radius: 5.0,
            leader_angle: 0.0,
            annotation: DimAnnotation::default(),
        };
        let segs = radial_pick_segments(&dim);
        assert_eq!(segs.len(), 1);
        assert!(approx(segs[0][0], Point2::ORIGIN));
        assert!(approx(segs[0][1], Point2::new(5.0, 0.0)));
    }

    #[test]
    fn radial_distance_zero_on_leader_line() {
        let dim = DimRadial {
            center: Point2::ORIGIN,
            radius: 5.0,
            leader_angle: 0.0,
            annotation: DimAnnotation::default(),
        };
        // 引出線 (0,0)-(5,0) 上。
        assert!(radial_distance(&dim, Point2::new(2.0, 0.0)) < T);
        // 引出線から 1 離れた点。
        assert!((radial_distance(&dim, Point2::new(2.0, 1.0)) - 1.0).abs() < T);
    }

    #[test]
    fn expand_radial_prefixes_r_exactly_once() {
        let dim = DimRadial {
            center: Point2::ORIGIN,
            radius: 12.5,
            leader_angle: 0.0,
            annotation: DimAnnotation::default(),
        };
        let ex = expand_radial(&dim, render(0.5, 1.0));
        assert_eq!(ex.segments.len(), 1);
        assert_eq!(ex.arrows.len(), 1);
        // 接頭辞 R は組版側（`DimKind::Radial` の既定記号）だけが供給する。展開側で
        // `R{:.2}` を焼き込むと `RR12.5` になるので、記号と値は別のランに分かれる。
        assert_eq!(contents_of(&ex), ["R", "12.5"]);
        assert!(ex.symbol_strokes.is_empty(), "R はフォント文字");
        // 矢先の先端は円周点 (12.5,0)。
        assert!(approx(ex.arrows[0][0], Point2::new(12.5, 0.0)));
        // 文字は水平（角 0）で円周点より外側（x>12.5）。
        assert!(ex.texts.iter().all(|t| t.angle.abs() < T));
        assert!(ex.texts[0].anchor.x > 12.5);
    }

    #[test]
    fn expand_radial_leader_direction_follows_angle() {
        // 引出角 +π/2 → 円周点 (0, radius)。
        let dim = DimRadial {
            center: Point2::ORIGIN,
            radius: 3.0,
            leader_angle: FRAC_PI_2,
            annotation: DimAnnotation::default(),
        };
        let ex = expand_radial(&dim, render(0.5, 1.0));
        assert!(approx(ex.segments[0][1], Point2::new(0.0, 3.0)));
    }

    #[test]
    fn expand_radial_leftward_leader() {
        // 引出角 π → 円周点 (−radius, 0)。文字は左へ伸ばす。
        let dim = DimRadial {
            center: Point2::ORIGIN,
            radius: 4.0,
            leader_angle: PI,
            annotation: DimAnnotation::default(),
        };
        let ex = expand_radial(&dim, render(0.5, 1.0));
        assert!(approx(ex.segments[0][1], Point2::new(-4.0, 0.0)));
        assert!(ex.texts[0].anchor.x < -4.0);
    }

    // --- 直径寸法（タスク49-2 で新設） ---

    fn plain_diameter(radius: f64, angle: f64) -> DimDiameter {
        DimDiameter {
            center: Point2::ORIGIN,
            radius,
            angle,
            annotation: DimAnnotation::default(),
        }
    }

    #[test]
    fn diameter_pick_segment_is_the_diameter_line() {
        // 中心原点・半径 5・角 0 → 直径線 (−5,0)-(5,0)。保存データだけで決まる。
        let dim = plain_diameter(5.0, 0.0);
        let segs = diameter_pick_segments(&dim);
        assert_eq!(segs.len(), 1);
        assert!(approx(segs[0][0], Point2::new(-5.0, 0.0)));
        assert!(approx(segs[0][1], Point2::new(5.0, 0.0)));

        // 角 +π/2 なら縦の直径線。
        let dim = plain_diameter(5.0, FRAC_PI_2);
        let segs = diameter_pick_segments(&dim);
        assert!(approx(segs[0][0], Point2::new(0.0, -5.0)));
        assert!(approx(segs[0][1], Point2::new(0.0, 5.0)));
    }

    #[test]
    fn diameter_distance_is_zero_on_the_diameter_line() {
        let dim = plain_diameter(5.0, 0.0);
        assert!(diameter_distance(&dim, Point2::new(2.0, 0.0)) < T);
        assert!(diameter_distance(&dim, Point2::ORIGIN) < T);
        assert!((diameter_distance(&dim, Point2::new(0.0, 3.0)) - 3.0).abs() < T);
        // 線分の外（円の外側）は端点までの距離。
        assert!((diameter_distance(&dim, Point2::new(8.0, 0.0)) - 3.0).abs() < T);
    }

    #[test]
    fn expand_diameter_shows_twice_the_radius_with_the_phi_symbol() {
        // 表示値は直径（2r）。φ は geom のストロークで出る（`symbol_strokes`）。
        let dim = plain_diameter(30.0, 0.0);
        let ex = expand_diameter(&dim, render(0.5, 1.0));
        assert_eq!(contents_of(&ex), ["60"]);
        assert!(!ex.symbol_strokes.is_empty(), "φ はストローク");
        // 寸法線は中心を通る直径線、矢先は円周 2 点。補助線はない。
        assert_eq!(ex.segments.len(), 1);
        assert!(approx(ex.segments[0][0], Point2::new(-30.0, 0.0)));
        assert!(approx(ex.segments[0][1], Point2::new(30.0, 0.0)));
        assert_eq!(ex.arrows.len(), 2);
        assert!(approx(ex.arrows[0][0], Point2::new(-30.0, 0.0)));
        assert!(approx(ex.arrows[1][0], Point2::new(30.0, 0.0)));
        assert!(!arrows_are_outward(&ex, Vec2::new(1.0, 0.0)));
    }

    #[test]
    fn expand_diameter_places_the_label_above_the_diameter_line() {
        // 長さ寸法と同じ配置規則（読み方向の上側・text_gap のすきま）。
        let gap = mm_to_world(DimStyle::DEFAULT.text_gap_mm);
        let dim = plain_diameter(30.0, 0.0);
        let ex = expand_diameter(&dim, render(0.5, 1.0));
        assert!(ex.texts[0].angle.abs() < T);
        let lowest = ex
            .texts
            .iter()
            .map(|t| t.anchor.y)
            .fold(f64::INFINITY, f64::min);
        assert!((lowest - gap).abs() < T, "{lowest}");

        // 斜めの直径線でも読み方向は左下→右上、文字はその上側。
        let dim = plain_diameter(30.0, PI * 0.75);
        let ex = expand_diameter(&dim, render(0.5, 1.0));
        let angle = ex.texts[0].angle;
        assert!((angle - (-PI * 0.25)).abs() < T, "{angle}");
        let up = Vec2::new(angle.cos(), angle.sin()).perp();
        assert!((ex.texts[0].anchor - Point2::ORIGIN).dot(up) > 0.0);
    }

    #[test]
    fn expand_diameter_follows_the_same_arrow_placement_rule() {
        // 小さな穴では矢が収まらない → 外向き。手動指定も長さ寸法と同じく効く
        // （効かないと `ArrowPlacement::Outside` の指定が黙って落ちる）。
        let dir = Vec2::new(1.0, 0.0);
        let small = plain_diameter(2.0, 0.0);
        assert!(arrows_are_outward(
            &expand_diameter(&small, render(0.5, 1.0)),
            dir
        ));

        let forced_inside = DimDiameter {
            annotation: DimAnnotation {
                arrow_placement: ArrowPlacement::Inside,
                ..DimAnnotation::default()
            },
            ..plain_diameter(2.0, 0.0)
        };
        assert!(!arrows_are_outward(
            &expand_diameter(&forced_inside, render(0.5, 1.0)),
            dir
        ));

        let forced_outside = DimDiameter {
            annotation: DimAnnotation {
                arrow_placement: ArrowPlacement::Outside,
                ..DimAnnotation::default()
            },
            ..plain_diameter(30.0, 0.0)
        };
        assert!(arrows_are_outward(
            &expand_diameter(&forced_outside, render(0.5, 1.0)),
            dir
        ));
    }

    // --- ラベルの組版（タスク49-1） ---

    /// 組版テストの基準文字高さ。ASCII 1 文字の近似幅がちょうど 5.5 になる値を選び、
    /// 期待値を手計算できるようにする。
    const H: f64 = 10.0;

    /// 基準文字高さ [`H`] における ASCII 1 文字の近似送り幅。
    const CW: f64 = H * ASCII_CHAR_WIDTH_RATIO;

    /// [`DimSymbol`] の全バリアント（geom 側は `#[non_exhaustive]` なのでここに列挙する。
    /// 記号が増えたらこの配列を更新すること。`mcad_core::dim` の同名配列と同じ流儀）。
    const ALL_SYMBOLS: [DimSymbol; 8] = [
        DimSymbol::Diameter,
        DimSymbol::SphereDiameter,
        DimSymbol::Square,
        DimSymbol::Radius,
        DimSymbol::SphereRadius,
        DimSymbol::ControlRadius,
        DimSymbol::Chamfer,
        DimSymbol::Thickness,
    ];

    fn layout_with(
        style: &DimStyle,
        kind: DimKind,
        value: f64,
        annotation: &DimAnnotation,
    ) -> DimLabel {
        layout_dim_label(value, annotation, style, kind, H)
    }

    /// 既定スタイル（桁数 2・ゼロトリム ON・公差 70%）で長さ寸法として組む。
    fn layout(value: f64, annotation: &DimAnnotation) -> DimLabel {
        layout_with(&DimStyle::DEFAULT, DimKind::Linear, value, annotation)
    }

    fn contents(label: &DimLabel) -> Vec<&str> {
        label.runs.iter().map(|r| r.content.as_str()).collect()
    }

    /// 記号を持たないラベルの値ラン（先頭のラン）。
    fn value_run(label: &DimLabel) -> &TextRun {
        label.runs.first().expect("値のランは必ずある")
    }

    fn with_tolerance(tolerance: SizeTolerance) -> DimAnnotation {
        DimAnnotation {
            tolerance: Some(tolerance),
            ..DimAnnotation::default()
        }
    }

    fn with_symbol(symbol: DimSymbol) -> DimAnnotation {
        DimAnnotation {
            symbol: Some(symbol),
            ..DimAnnotation::default()
        }
    }

    /// 外形が実際に置いた内容（ラン・ストローク・下線）をすべて覆っていること。
    /// 境界ちょうどで接する要素があるので、丸め誤差ぶんだけ広げて判定する。
    fn assert_bounds_cover_content(label: &DimLabel) {
        let bounds = label.bounds.expanded(T);
        for run in &label.runs {
            let width = run.content.chars().count() as f64 * ASCII_CHAR_WIDTH_RATIO * run.height;
            let box_ = Aabb::new(
                run.origin,
                Point2::new(run.origin.x + width, run.origin.y + run.height),
            );
            assert!(bounds.contains(&box_), "run {:?} is outside", run.content);
        }
        for shape in &label.strokes {
            assert!(bounds.contains(&shape.aabb()), "stroke is outside");
        }
        if let Some([a, b]) = label.underline {
            assert!(bounds.contains_point(a) && bounds.contains_point(b));
        }
        assert!((label.width() - label.bounds.width()).abs() < T);
        assert!((label.height() - label.bounds.height()).abs() < T);
    }

    #[test]
    fn label_decimals_prefer_the_annotation_override() {
        // 桁数だけを見たいのでゼロトリムは切る。
        let style2 = DimStyle {
            decimals: 2,
            trim_trailing_zeros: false,
            ..DimStyle::DEFAULT
        };
        let style3 = DimStyle {
            decimals: 3,
            ..style2
        };
        let plain = DimAnnotation::default();
        let overridden = DimAnnotation {
            decimals_override: Some(1),
            ..DimAnnotation::default()
        };

        // `None` は文書スタイルへの生きた参照（スタイル変更に追従する）。
        let label = layout_with(&style2, DimKind::Linear, 12.5, &plain);
        assert_eq!(value_run(&label).content, "12.50");
        let label = layout_with(&style3, DimKind::Linear, 12.5, &plain);
        assert_eq!(value_run(&label).content, "12.500");

        // `Some(n)` は明示上書き（スタイル変更に追従しない）。
        let label = layout_with(&style2, DimKind::Linear, 12.5, &overridden);
        assert_eq!(value_run(&label).content, "12.5");
        let label = layout_with(&style3, DimKind::Linear, 12.5, &overridden);
        assert_eq!(value_run(&label).content, "12.5");
    }

    #[test]
    fn label_trims_trailing_zeros_only_when_the_style_asks() {
        let trimming = DimStyle::DEFAULT; // 桁数 2・トリム ON
        let fixed = DimStyle {
            trim_trailing_zeros: false,
            ..DimStyle::DEFAULT
        };
        let plain = DimAnnotation::default();

        let text = |style: &DimStyle, value: f64| {
            layout_with(style, DimKind::Linear, value, &plain).runs[0]
                .content
                .clone()
        };
        assert_eq!(text(&trimming, 120.0), "120");
        assert_eq!(text(&trimming, 12.5), "12.5");
        assert_eq!(text(&fixed, 120.0), "120.00");
        assert_eq!(text(&fixed, 12.5), "12.50");

        // 桁数 0 のとき、整数の末尾の 0 まで落とさない（`120` → `12` にしない）。
        let integral = DimStyle {
            decimals: 0,
            ..DimStyle::DEFAULT
        };
        assert_eq!(text(&integral, 120.0), "120");
    }

    #[test]
    fn label_symmetric_tolerance_shares_the_value_text_height() {
        // 規定 5-12-2 1)（`50 ± 0.5`。± と公差値は寸法数値と同じ高さ）。
        let label = layout(50.0, &with_tolerance(SizeTolerance::Symmetric(0.5)));
        assert_eq!(contents(&label), ["50", "± 0.5"]);

        let value = &label.runs[0];
        let tolerance = &label.runs[1];
        assert!((tolerance.height - value.height).abs() < T);
        assert!(
            (tolerance.origin.y - value.origin.y).abs() < T,
            "同じベースライン"
        );
        // 値の右へ、ASCII 空白 1 文字ぶん空けて置く。
        assert!((tolerance.origin.x - (2.0 * CW + CW)).abs() < T);
        assert_bounds_cover_content(&label);
    }

    #[test]
    fn label_deviations_stack_at_the_tolerance_scale_with_explicit_signs() {
        // 規定 5-12-2 2)（上公差を上・下公差を下、公差文字は 70% に縮小）。
        let label = layout(
            50.0,
            &with_tolerance(SizeTolerance::Deviations {
                upper: 0.2,
                lower: -0.1,
            }),
        );
        assert_eq!(contents(&label), ["50", "+0.2", "-0.1"]);

        let upper = &label.runs[1];
        let lower = &label.runs[2];
        assert!((upper.height - H * DimStyle::DEFAULT.tolerance_scale).abs() < T);
        assert!((lower.height - upper.height).abs() < T);
        assert!(upper.origin.y > lower.origin.y, "上段が上");
        assert!((upper.origin.x - lower.origin.x).abs() < T, "左揃え");
        // 上下段の中間は寸法数値の中央高さ。
        let stack_center = ((upper.origin.y + upper.height) + lower.origin.y) * 0.5;
        assert!((stack_center - H * 0.5).abs() < T);
        // 下段はベースラインより下へ出る（外形が原点の左下から始まらない根拠）。
        assert!(lower.origin.y < 0.0);
        assert!(label.bounds.min.y < 0.0);
        assert_bounds_cover_content(&label);
    }

    #[test]
    fn label_deviation_zero_is_written_without_a_sign() {
        // 規定 5-12-2 3)（片側公差）。0 の段には符号を付けない。
        let label = layout(
            50.0,
            &with_tolerance(SizeTolerance::Deviations {
                upper: 0.3,
                lower: 0.0,
            }),
        );
        assert_eq!(contents(&label), ["50", "+0.3", "0"]);

        let label = layout(
            50.0,
            &with_tolerance(SizeTolerance::Deviations {
                upper: 0.0,
                lower: -0.2,
            }),
        );
        assert_eq!(contents(&label), ["50", "0", "-0.2"]);

        // 上下とも負（穴基準の軸など）でも符号を明示する。
        let label = layout(
            50.0,
            &with_tolerance(SizeTolerance::Deviations {
                upper: -0.05,
                lower: -0.1,
            }),
        );
        assert_eq!(contents(&label), ["50", "-0.05", "-0.1"]);
    }

    #[test]
    fn label_fit_class_is_appended_directly_to_the_value() {
        // 規定 5-12-3 2)（`φ20H7`。はめあい記号は基準寸法の直後、表示のみ）。
        let annotation = DimAnnotation {
            symbol: Some(DimSymbol::Diameter),
            tolerance: Some(SizeTolerance::Fit(FitClass::new("H7").unwrap())),
            ..DimAnnotation::default()
        };
        let label = layout(20.0, &annotation);
        // φ はストロークなのでランには出ない。
        assert_eq!(contents(&label), ["20", "H7"]);
        assert!(!label.strokes.is_empty());

        let value = &label.runs[0];
        let fit = &label.runs[1];
        assert!((fit.height - value.height).abs() < T);
        assert!((fit.origin.y - value.origin.y).abs() < T);
        // すきまなしで連結する（対称許容差と違い空白を入れない）。
        assert!((fit.origin.x - (value.origin.x + 2.0 * CW)).abs() < T);
        assert_bounds_cover_content(&label);
    }

    #[test]
    fn label_symbol_is_typeset_to_the_left_of_the_value() {
        // 規定 5-2 4)（寸法補助記号は数値の左）。
        let label = layout(12.5, &with_symbol(DimSymbol::Radius));
        assert_eq!(contents(&label), ["R", "12.5"]);
        assert!(label.runs[0].origin.x.abs() < T, "記号は原点から");
        // 値は記号の送り幅ぶんだけ右（送り幅の出所は geom の `SymbolGlyph::advance`）。
        let advance = dim_symbol_glyph(DimSymbol::Radius, H).advance;
        assert!((label.runs[1].origin.x - advance).abs() < T);
    }

    #[test]
    fn label_draws_diameter_as_strokes_and_radius_as_a_font_run() {
        // φ は geom がジオメトリを生成する（フォントに頼らない。M9 設計判断1）。
        let phi = layout(25.0, &with_symbol(DimSymbol::Diameter));
        assert_eq!(contents(&phi), ["25"], "φ はランに出ない");
        assert!(!phi.strokes.is_empty());

        // R はフォント文字。
        let r = layout_with(
            &DimStyle::DEFAULT,
            DimKind::Radial,
            10.0,
            &with_symbol(DimSymbol::Radius),
        );
        assert_eq!(contents(&r), ["R", "10"]);
        assert!(r.strokes.is_empty());
    }

    #[test]
    fn label_sphere_diameter_combines_a_font_s_with_the_diameter_strokes() {
        let label = layout(30.0, &with_symbol(DimSymbol::SphereDiameter));
        // "S" はフォント文字、φ はストローク。
        assert_eq!(contents(&label), ["S", "30"]);
        assert!(!label.strokes.is_empty());
        assert!(label.runs[0].origin.x.abs() < T);
        // 値の開始位置 = geom が返す Sφ の advance（S と φ を独自計算で分けない）。
        let advance = dim_symbol_glyph(DimSymbol::SphereDiameter, H).advance;
        assert!((label.runs[1].origin.x - advance).abs() < T);
        // φ のストロークは "S" より右にある。
        let phi_left = label
            .strokes
            .iter()
            .map(|s| s.aabb().min.x)
            .fold(f64::INFINITY, f64::min);
        assert!(phi_left >= CW - T);
        assert_bounds_cover_content(&label);
    }

    #[test]
    fn label_value_override_replaces_the_number_and_is_underlined() {
        // 規定 5-10（非比例寸法）。本文が空なので JIS B 0001 の慣行（数値の下に実線）。
        let annotation = DimAnnotation {
            value_override: Some("50".to_string()),
            ..DimAnnotation::default()
        };
        let label = layout(37.123, &annotation);
        assert_eq!(contents(&label), ["50"], "実測値ではなく上書き値を組む");

        let [a, b] = label.underline.expect("非比例寸法には下線が付く");
        assert!(a.x.abs() < T);
        assert!((b.x - 2.0 * CW).abs() < T, "下線は値の幅ぶん");
        assert!(a.y < 0.0, "ベースラインより下");
        assert!((a.y - b.y).abs() < T, "水平");

        // 上書きなしなら下線なし。
        assert!(
            layout(37.123, &DimAnnotation::default())
                .underline
                .is_none()
        );
        assert_bounds_cover_content(&label);
    }

    #[test]
    fn label_underline_covers_only_the_value() {
        let annotation = DimAnnotation {
            symbol: Some(DimSymbol::Diameter),
            tolerance: Some(SizeTolerance::Symmetric(0.5)),
            value_override: Some("50".to_string()),
            ..DimAnnotation::default()
        };
        let label = layout(37.123, &annotation);
        let value = value_run(&label);
        let [a, b] = label.underline.expect("下線");
        // 記号（φ）には掛からない。
        assert!(a.x > 0.0);
        assert!((a.x - value.origin.x).abs() < T);
        // 公差にも掛からない。
        assert!((b.x - (value.origin.x + 2.0 * CW)).abs() < T);
        assert!(b.x < label.bounds.max.x);
        assert_bounds_cover_content(&label);
    }

    #[test]
    fn label_bounds_match_the_typeset_content() {
        let annotation = DimAnnotation {
            symbol: Some(DimSymbol::SphereDiameter),
            tolerance: Some(SizeTolerance::Deviations {
                upper: 0.2,
                lower: -0.1,
            }),
            value_override: Some("50".to_string()),
            ..DimAnnotation::default()
        };
        let label = layout(37.123, &annotation);
        assert_bounds_cover_content(&label);

        // 幅は「記号 + 値 + すきま + 公差列」の総送り幅と一致する。
        let symbol = dim_symbol_glyph(DimSymbol::SphereDiameter, H).advance;
        let deviation_width = 4.0 * CW * DimStyle::DEFAULT.tolerance_scale; // "+0.2" / "-0.1"
        let expected = symbol + 2.0 * CW + CW + deviation_width;
        assert!((label.width() - expected).abs() < T, "{}", label.width());
        assert!(label.bounds.min.x.abs() < T);

        // 高さは上下段公差と下線を含む全高。
        let upper = &label.runs[2];
        let lower = &label.runs[3];
        let top = upper.origin.y + upper.height;
        let bottom = label.underline.expect("下線")[0].y.min(lower.origin.y);
        assert!((label.height() - (top - bottom)).abs() < T);
        assert!(label.height() > H, "上下段は寸法数値より高い");
    }

    #[test]
    fn label_default_symbol_depends_on_the_dimension_kind() {
        let plain = DimAnnotation::default();
        // 長さ寸法は記号なし。
        let linear = layout_with(&DimStyle::DEFAULT, DimKind::Linear, 50.0, &plain);
        assert_eq!(contents(&linear), ["50"]);
        assert!(linear.strokes.is_empty());

        // 半径寸法は R（M8 までの `expand_radial` の `R{:.2}` と同じ見え方）。
        let radial = layout_with(&DimStyle::DEFAULT, DimKind::Radial, 50.0, &plain);
        assert_eq!(contents(&radial), ["R", "50"]);

        // 直径寸法は φ（ストローク）。
        let diameter = layout_with(&DimStyle::DEFAULT, DimKind::Diameter, 50.0, &plain);
        assert_eq!(contents(&diameter), ["50"]);
        assert!(!diameter.strokes.is_empty());
    }

    #[test]
    fn label_explicit_symbol_overrides_the_kind_default() {
        let label = layout_with(
            &DimStyle::DEFAULT,
            DimKind::Radial,
            50.0,
            &with_symbol(DimSymbol::SphereRadius),
        );
        assert_eq!(contents(&label), ["SR", "50"]);
    }

    #[test]
    fn every_known_symbol_is_typeset() {
        // 既知の記号がストローク・フォント文字のどちらでも出ないまま送り幅だけ確保される
        // （= 組版側の未対応）ことを検出する保守用テスト。文法（種別 × 記号）は core の
        // 責務なので、ここでは種別を固定して組版だけを見る。
        for symbol in ALL_SYMBOLS {
            let label = layout(1.0, &with_symbol(symbol));
            let has_symbol_run = label.runs.len() > 1;
            assert!(
                has_symbol_run || !label.strokes.is_empty(),
                "{symbol:?} は文字でもストロークでも描かれていない"
            );
            // 値の開始位置は geom の advance と一致する（送り幅の二重計算をしない）。
            let advance = dim_symbol_glyph(symbol, H).advance;
            let value = label.runs.last().expect("値のラン");
            assert_eq!(value.content, "1");
            assert!(
                (value.origin.x - advance).abs() < T,
                "{symbol:?}: {} != {advance}",
                value.origin.x
            );
        }
    }

    #[test]
    fn label_geometry_scales_with_the_text_height() {
        // 単位は抽象（入力 `text_height` と同じ単位で返す）。同じ注記なら、文字高さを
        // k 倍したラベルは幾何がちょうど k 倍になる。
        let annotation = DimAnnotation {
            symbol: Some(DimSymbol::Diameter),
            tolerance: Some(SizeTolerance::Deviations {
                upper: 0.2,
                lower: -0.1,
            }),
            value_override: Some("50".to_string()),
            ..DimAnnotation::default()
        };
        let unit = layout_dim_label(37.0, &annotation, &DimStyle::DEFAULT, DimKind::Linear, 1.0);
        let scaled = layout_dim_label(37.0, &annotation, &DimStyle::DEFAULT, DimKind::Linear, 10.0);

        assert_eq!(contents(&unit), contents(&scaled), "文字列は単位に依らない");
        assert!((scaled.width() - unit.width() * 10.0).abs() < T);
        assert!((scaled.height() - unit.height() * 10.0).abs() < T);
        for (a, b) in unit.runs.iter().zip(&scaled.runs) {
            assert!((b.origin.x - a.origin.x * 10.0).abs() < T);
            assert!((b.origin.y - a.origin.y * 10.0).abs() < T);
            assert!((b.height - a.height * 10.0).abs() < T);
        }
        let [ua, _] = unit.underline.expect("下線");
        let [sa, _] = scaled.underline.expect("下線");
        assert!((sa.y - ua.y * 10.0).abs() < T);
    }

    // --- 文字ブロック外形（M9 タスク51） ---

    #[test]
    fn expand_linear_label_box_encloses_the_label_centre() {
        // 自動配置時、`label_box` の中心はラベルの表示中心（＝ `place_label` が置いた点）
        // と一致し、四隅は必ずその中心を囲む（凸四角形として退化していない限り）。
        let dim = plain_linear(Point2::new(0.0, 0.0), Point2::new(4.0, 0.0), 2.0);
        let ex = expand_linear(&dim, render(0.5, 1.0));
        let quad = ex.label_box.expect("label_box が展開されている");
        let center = label_box_center(&quad);
        assert!(label_box_contains(&quad, center));
        // 四隅は退化しない（幅・高さがゼロでない）。
        assert!(quad[0].distance(quad[1]) > T);
        assert!(quad[1].distance(quad[2]) > T);
    }

    #[test]
    fn expand_linear_label_box_is_centred_on_the_text_anchor() {
        // `text_anchor` を指定すると、`label_box` の中心はその点になる
        // （`place_label` が外形の中心を `center` へ置く、という既存の配置規則どおり）。
        let anchor = Point2::new(-10.0, 7.0);
        let dim = DimLinear {
            p1: Point2::new(0.0, 0.0),
            p2: Point2::new(4.0, 0.0),
            offset: 2.0,
            annotation: DimAnnotation {
                text_anchor: Some(anchor),
                ..DimAnnotation::default()
            },
        };
        let ex = expand_linear(&dim, render(0.5, 1.0));
        let quad = ex.label_box.expect("label_box が展開されている");
        assert!(approx(label_box_center(&quad), anchor));
    }

    #[test]
    fn label_box_contains_checks_convex_quad_membership() {
        // 軸並行の正方形 [0,0]-[2,0]-[2,2]-[0,2] で内外を確かめる。
        let quad = [
            Point2::new(0.0, 0.0),
            Point2::new(2.0, 0.0),
            Point2::new(2.0, 2.0),
            Point2::new(0.0, 2.0),
        ];
        assert!(label_box_contains(&quad, Point2::new(1.0, 1.0)));
        assert!(label_box_contains(&quad, Point2::new(0.0, 0.0))); // 境界も含む
        assert!(!label_box_contains(&quad, Point2::new(3.0, 1.0)));
        assert!(!label_box_contains(&quad, Point2::new(1.0, -0.1)));
    }
}
