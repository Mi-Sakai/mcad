//! 寸法ジオメトリの展開（寸法線・補助線・矢先・文字配置）を計算する純関数群。
//!
//! # 位置づけ（DESIGN.md M6 設計判断2 → M11 設計判断1）
//!
//! M6〜M10 では展開ロジックを `mcad-app` に置いていたが、`mcad-io` から使えないために
//! DXF の表 export が組版を io 側へ再実装する二重化を招いた（M10 タスク61 の残債）。
//! M11 タスク71 で **`mcad-core` へ移し**、画面（`mcad-app`）・出力（SVG/PDF）・
//! DXF export が同じ実装を共有する。データ型（[`DimLinear`] / [`DimRadial`] /
//! [`DimDiameter`]）は従来どおり core にあり、展開はその隣に置かれたことになる。
//!
//! **egui にも [`Document`](crate::Document) にも依存しない純関数**である点は移設前と
//! 変わらない。入力は保存データと [`DimRender`]（ワールド長に解決済みのパラメータ束）
//! だけで、表示状態（紙基準表示トグル・ズーム）は一切知らない。
//!
//! # 描画とヒットテストの一貫性
//!
//! [`expand_linear`] / [`expand_radial`] / [`expand_diameter`] が返す [`DimExpansion`] を
//! 描画（`mcad-app` の `draw_dim_expansion`）とプレビューが共有する。ヒットテスト
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
//! [`DimExpansion::label_box`] は**表示サイズ依存**（[`DimRender`] を通した展開結果）で、
//! 上記のズーム非依存 pick 契約とは別の当たり判定に使う。対象は**選択済み**の寸法の
//! 文字ブロックドラッグ（`text_anchor` の後編集）に限られ、[`linear_pick_segments`] 等の
//! 通常選択の pick 形状には一切混ぜない。表示サイズに依存してよい理由は、これが
//! 「掴める場所を見た目どおりに掴む」操作であり、ズームで当たり外れが変わっても
//! 実害がない（選択そのものを左右しない）ため。
//!
//! # 展開パラメータ（M9 タスク49-2）
//!
//! 展開関数は [`DimRender`] を受け取る。**このモジュールは紙基準表示トグル（F9）も
//! ズームも知らない**。呼び出し側（`mcad-app` の `dimension::dim_render`・`plot`）が
//! 表示モードを解決してワールド長を詰め、このモジュールは受け取った長さをそのまま使う。
//!
//! # ラベルの組版（M9 タスク49-1）
//!
//! 注記（寸法補助記号・サイズ公差・桁数上書き・非比例寸法）つきの寸法値を組み上げる
//! 純関数は隣の `dim_label` モジュール（[`layout_dim_label`](super::layout_dim_label)）に
//! 分けてある。**配置（ワールドへの回転・平行移動）は行わず**、ラベルのローカル座標だけを
//! 返す点でこのモジュールの展開関数とは責務が分かれている。ワールドへの合流は
//! [`place_label`]（タスク49-2）が担う。

use mcad_geom::{ArrowGlyph, ArrowKind, LineSeg, Point2, Shape, Vec2, arrow_glyph};

use crate::{
    ArrowPlacement, DimAnnotation, DimDiameter, DimDirection, DimKind, DimLinear, DimRadial,
    DimStyle, TextGeom,
};

use super::dim_label::{DimLabel, layout_dim_label};

/// 寸法を描画・プレビュー可能な要素へ展開した結果（すべてワールド座標）。
#[derive(Debug, Clone, PartialEq)]
pub struct DimExpansion {
    /// 線分（寸法線・補助線・引出線・非比例寸法の下線）。
    ///
    /// 非比例寸法（規定 5-10）の下線をここへ混ぜているのは、下線が寸法線とまったく
    /// 同じ実線ストロークで描かれるべきものであり、consumers（`mcad-app` の
    /// `draw_dim_expansion`・`plot::push_dim`）へ描画分岐を 1 つも増やさずに済むため。
    ///
    /// 並び順は **寸法線（または引出線）→ 非比例寸法の下線（あれば）→ 補助線**。
    /// `segments[0]` が寸法線であることだけは全種別で共通。
    pub segments: Vec<[Point2; 2]>,
    /// 矢先（各要素は 1 個ぶんの形状を**ワールド座標へ写した** [`ArrowGlyph`]）。
    ///
    /// 形そのものは geom の純関数 [`mcad_geom::arrow_glyph`] が種別
    /// （[`DimStyle::arrow_kind`]）ごとに決め、ここではローカル座標を
    /// `place_arrow` でワールドへ移しただけのものを持つ。消費側（`mcad-app` の
    /// `draw_dim_expansion`・`plot::push_dim`）は [`ArrowGlyph::fills`] を塗り、
    /// [`ArrowGlyph::strokes`] を寸法線と同じストロークで描く。
    /// **種別ごとの分岐は消費側に一切置かない**（画面と SVG/PDF が同一の形を描くのは
    /// 「形を決めるのは geom の純関数ひとつだけ」という M9 判断1・9 の帰結）。
    pub arrows: Vec<ArrowGlyph>,
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
    /// 文字ブロックの外形（[`DimLabel::bounds`](super::DimLabel::bounds) をワールドへ写した凸四角形）。
    ///
    /// 頂点順は `[左下, 右下, 右上, 左上]`（ラベルのローカル軸に沿う。回転しても平行四辺形
    /// のまま歪まない）。**M9 タスク51**（文字位置の後編集）の当たり判定・ドラッグ対象の
    /// 可視化に使う。`place_label` を呼ばない経路は素通しで `None` のまま
    /// （現状すべての展開関数が `place_label` を通るため実質常に `Some`）。
    pub label_box: Option<[Point2; 4]>,
}

impl DimExpansion {
    /// 線分・矢先だけを持つ空のラベル列で作る（ラベルは [`place_label`] が後から積む）。
    fn new(segments: Vec<[Point2; 2]>, arrows: Vec<ArrowGlyph>) -> Self {
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
/// なので、ワールドへ出すときは必ず `DimRender::paper_mm_to_world` を通す。
/// この命名規則は M8 タスク37 の教訓（`DimExpansion` の文字高さはワールド長なのに
/// [`TextGeom::height`] は紙 mm、という取り違えが二重換算を生んだ）への対策である。
///
/// # 2 つの換算係数を混同しないこと
///
/// - [`DimRender::scale_world_per_paper_mm`] は**文書尺度**（`Scale::world_mm_per_paper_mm`）。
///   表示状態に依存しない図面固有の量で、**外向き矢印の自動判定にしか使わない**。
/// - `DimRender::paper_mm_to_world` は**注記の表示倍率**。紙基準表示 OFF では
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

/// 矢先グリフ（ローカル座標）を、先端 `tip`・向き `dir`（単位ベクトル。**先端が指す
/// 方向**）でワールドへ配置する。
///
/// [`ArrowGlyph`] のローカル座標 `(x, y)` は `tip + dir * x + dir.perp() * y` へ移る
/// （M9 までの `arrow_triangle` が `[tip, back + half, back - half]` を作っていたのと
/// 同じ姿勢: `dir` が +x、その左手側が +y）。
fn place_arrow(tip: Point2, dir: Vec2, glyph: &ArrowGlyph) -> ArrowGlyph {
    let up = dir.perp();
    let to_world = |p: Point2| tip + dir * p.x + up * p.y;
    ArrowGlyph {
        fills: glyph
            .fills
            .iter()
            .map(|poly| poly.iter().map(|p| to_world(*p)).collect())
            .collect(),
        strokes: glyph
            .strokes
            .iter()
            .map(|[a, b]| [to_world(*a), to_world(*b)])
            .collect(),
        along_len: glyph.along_len,
    }
}

/// 矢先種別 `kind` が寸法線に沿って場所を取るか（＝内外配置の指定が描画に現れるか）。
///
/// `false` になるのは `Oblique` / `Dot` / `None`。UI（右パネルの矢印配置コンボ）が
/// この判定で無効化するために公開している（DESIGN.md 7章「随時対応」の「寸法矢印の
/// ブロック化」設計確定4・6）。[`mcad_geom::ArrowGlyph::along_len`] は `len` に比例
/// するので、長さ 1 の代表値で問うても種別ごとの答えは変わらない。
#[must_use]
pub fn arrow_kind_occupies_line(kind: ArrowKind) -> bool {
    arrow_glyph(kind, 1.0).along_len > 0.0
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
/// # 寸法線に沿って場所を取らない矢先は常に内向き（M10 タスク63）
///
/// 「矢 2 つ分」は矢先長ではなく [`ArrowGlyph::along_len`]（矢先が寸法線に沿って占める
/// 長さ）で測る。`Oblique` / `Dot` / `None` はこれが 0 なので、自動判定は外向きにならず、
/// 手動 [`ArrowPlacement::Outside`] も無視して内向き扱いにする（描画がまったく変わらない
/// のに寸法線だけが伸びるのを防ぐ。7章「随時対応」の「寸法矢印のブロック化」設計確定4）。
/// UI 側はこの種別のとき矢印配置コンボを無効化する（[`arrow_kind_occupies_line`]）。
///
/// # 判定はビュー非依存（必須）
///
/// 3 つの量を**すべて紙 mm**で取る。ラベル幅は [`DimStyle::text_height_mm`] で組んだ
/// [`DimLabel::width`]、矢先の占有長は [`DimStyle::arrow_len_mm`] で作ったグリフの
/// `along_len`、寸法線長は
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
    // 紙 mm で作ったグリフの占有長。種別が `Oblique` / `Dot` / `None` のとき、および
    // スタイルの矢先長が使えない値のときは 0 になる。
    let along_len_mm = arrow_glyph(render.style.arrow_kind, render.style.arrow_len_mm).along_len;
    if along_len_mm <= 0.0 {
        return false;
    }
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
            line_len_mm < label_width_mm + 2.0 * along_len_mm
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
/// `extension_line` で作って `segments` へ足す（直径寸法には補助線がない）。
///
/// 配置規則は 3 点:
/// 1. 文字は寸法線の**上側**（[`reading_direction`] の `perp()` 正側）へ、
///    [`DimStyle::text_gap_mm`] のすきまを空けて置く（規定 5-2 3)）。
/// 2. 矢は `arrows_point_outward` の判定で内外を切り替える（規定 5-4 4)）。
///    外向きのときは矢が乗る線がなくなるので、寸法線を両端へ矢先の占有長
///    （[`ArrowGlyph::along_len`]）ぶん延長する。
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
    let glyph = arrow_glyph(render.style.arrow_kind, render.arrow_len_world);
    let outward = arrows_point_outward((d2 - d1).length(), value, annotation, kind, render);

    // 矢先の先端は常に寸法線の両端（`d1` / `d2`）。胴を内・外どちらへ出すかだけが変わる
    // （`place_arrow` は先端から `-dir` の側へ胴を作る）。
    let (tail1, tail2) = if outward { (dir, -dir) } else { (-dir, dir) };
    // 外向きのときだけ寸法線を伸ばす。伸ばす量は矢先が線に沿って占める長さそのもの
    // （`outward` が真になる種別では `render.arrow_len_world` と一致する）。
    let (line1, line2) = if outward {
        (d1 - dir * glyph.along_len, d2 + dir * glyph.along_len)
    } else {
        (d1, d2)
    };
    let mut ex = DimExpansion::new(
        vec![[line1, line2]],
        vec![
            place_arrow(d1, tail1, &glyph),
            place_arrow(d2, tail2, &glyph),
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

/// 長さ寸法の寸法線の骨格（M11 タスク68）。保存データ（p1/p2/offset/direction）
/// だけで決まり、スタイルにも表示状態にも依存しない。
pub(crate) struct LinearFrame {
    /// 寸法線の端点（`p1` を寸法線へ投影した足）。
    pub(crate) d1: Point2,
    /// 寸法線の端点（`p2` を寸法線へ投影した足）。
    pub(crate) d2: Point2,
    /// **描画用**の寸法線方向の単位ベクトル（`(d2 − d1)` の向きに揃えてある）。
    ///
    /// `offset`（寸法線の位置決め）は [`DimDirection::unit_vector`] 由来の向きで
    /// 計算し終えてから、この場は「矢の内外配置・寸法線の延長方向」の基準として
    /// **`d1` から `d2` を向く向き**へ揃え直す（M11 タスク68 Codex レビュー指摘1）。
    /// `Rotated` で投影が負（計測点の順序が寸法線の向きと逆）だと 2 つの向きが
    /// 食い違い、揃えないと矢先・寸法線の延長が反転したまま描かれる。`Aligned` は
    /// 元々 `dir ∥ (p2 − p1)` で一致しているため実質変わらない。
    pub(crate) dir: Vec2,
    /// 表示する測定値（`|d2 − d1|`。[`DimLinear::measured_value`] と同値）。
    pub(crate) value: f64,
}

/// 長さ寸法の寸法線の骨格を求める（M11 タスク68 で回転寸法へ一般化）。
///
/// 寸法線は「[`DimLinear::offset`] が指す点 `p1 + dir.perp() * offset` を通る `dir`
/// 方向の直線」で、その 2 端点は計測 2 点をその直線へ**投影**した足である
/// （`dir` は [`DimDirection::unit_vector`]）。**`offset` の適用（寸法線の位置決め）は
/// 常にこの `dir`（`DimDirection` 由来）で行う**。[`LinearFrame::dir`] として返す値は
/// 描画用に向きを揃え直したもので、これとは別（doc 参照）。
///
/// # `Aligned` は閉じた形で計算する
///
/// [`DimDirection::Aligned`] では `dir ∥ (p2 − p1)` なので投影は恒等写像になり、
/// 端点は `p1 + shift` / `p2 + shift`（`shift = dir.perp() * offset`）そのものである。
/// 一般式（内積で投影パラメータを出して `dir` 倍する）を通しても数学的には同じだが
/// 最終桁が動きうるため、**M10 以前に保存された図面の描画をビット単位で保つ**ために
/// 閉じた形のまま残す（回帰網は `mcad-app` の寸法スナップショット 29 ケース）。
///
/// # `None`（描かない）になる条件
///
/// - [`DimDirection::Aligned`] で計測 2 点がほぼ同一（向きが定まらない。M10 までと同じ）
/// - [`DimDirection::Rotated`] で θ が非有限（向きが定まらない）
/// - [`DimDirection::Rotated`] で投影長が 0（**寸法線の長さが 0**）。計測 2 点が
///   寸法線の法線上に並んだ場合で、値 0 の寸法を描いても読めないため引かない。
pub(crate) fn linear_frame(dim: &DimLinear) -> Option<LinearFrame> {
    let dir = dim.direction.unit_vector(dim.p1, dim.p2)?;
    let shift = dir.perp() * dim.offset;
    match dim.direction {
        DimDirection::Aligned => Some(LinearFrame {
            d1: dim.p1 + shift,
            d2: dim.p2 + shift,
            dir,
            value: (dim.p2 - dim.p1).length(),
        }),
        DimDirection::Rotated(_) => {
            let span = (dim.p2 - dim.p1).dot(dir);
            // 寸法線長 0（投影がつぶれた）は描かない。`Vec2::normalize` と同じ
            // 最終防衛線の閾値で判定する（座標スケールに依らない量ではないが、
            // ここも「0 で割らない」ための下限でしかない）。
            if span.abs() <= mcad_geom::EPS {
                return None;
            }
            let d1 = dim.p1 + shift;
            Some(LinearFrame {
                d1,
                d2: d1 + dir * span,
                // 描画用の向きは `(d2 − d1)` へ揃える（`span` が負なら `dir` の逆側）。
                // `offset` に使った `shift` はこの前で確定済みなので位置決めには
                // 影響しない。
                dir: dir * span.signum(),
                value: span.abs(),
            })
        }
    }
}

/// [`linear_frame`] が `None`（投影長 0 の `Rotated`、または `Aligned`/`Rotated` で
/// 向きそのものが決まらない）を返すときに、展開（[`expand_linear`]）が寸法線の代わりに
/// 使う「方向・点」のうち、**方向が決まる場合**の値。
///
/// 方向 `dir`（[`DimDirection::unit_vector`]）が決まらない（`Aligned` で計測 2 点が
/// ほぼ同一、または `Rotated` で θ が非有限）ときは `None`。
///
/// AABB（`entity_geom::dim_linear_aabb`）はこの点を展開と共有するためにこの関数を呼ぶ
/// （M11 タスク68 Codex レビュー指摘2）。展開・AABB のどちらかだけを直して食い違わせる
/// ことがないよう、計算をこの 1 箇所へ寄せてある。
#[must_use]
pub(crate) fn linear_degenerate_dir_and_point(dim: &DimLinear) -> Option<(Vec2, Point2)> {
    let dir = dim.direction.unit_vector(dim.p1, dim.p2)?;
    Some((dir, dim.p1 + dir.perp() * dim.offset))
}

/// 長さ寸法のヒットテスト用線分（寸法線＋補助線 2 本）。矢先・文字の大きさに依らず
/// 保存データ（p1/p2/offset/direction）だけで決まるため、ズーム非依存で pick から
/// 使える。退化（[`linear_frame`] が `None`）時は空。
#[must_use]
pub fn linear_pick_segments(dim: &DimLinear) -> Vec<[Point2; 2]> {
    match linear_frame(dim) {
        Some(f) => vec![[f.d1, f.d2], [dim.p1, f.d1], [dim.p2, f.d2]],
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
/// - 寸法線は `d1`→`d2`（[`linear_frame`]。回転寸法では計測 2 点を投影した足）。
///   矢は `arrows_point_outward` の判定で内外が決まり、外向きのときは寸法線を
///   両端へ矢先長ぶん延長する。
/// - 補助線は計測点から**その足まで** [`DimStyle::ext_gap_mm`] のすきまを空けて始まり、
///   寸法線を [`DimStyle::ext_overshoot_mm`] だけ越えて終わる（退化時の扱いは
///   `extension_line`）。回転寸法では補助線が計測線と直交しない（寸法線の法線方向へ
///   引かれる）が、これは JIS でも一般的な回転寸法の描き方である。
/// - 値ラベルは寸法線の**上側**へ [`DimStyle::text_gap_mm`] のすきまで置く。
///   `offset` の符号では側を変えない（M9 タスク49-2 の意図した可視差）。
/// - 表示値は**寸法線の 2 端点間の距離**（[`DimLinear::measured_value`]）。
///   [`DimDirection::Aligned`] では `|p2 − p1|` に一致する。
#[must_use]
pub fn expand_linear(dim: &DimLinear, render: DimRender<'_>) -> DimExpansion {
    // 退化時も破綻しない骨格へ倒す（通常はツールが p1≈p2 を弾く）。
    // - 向きが決まる（回転寸法で投影長だけが 0）: 位置と向きは正しいので長さ 0 の
    //   寸法線を置く。
    // - 向きも決まらない（`Aligned` で p1≈p2 / θ が非有限）: M10 までと同じく
    //   計測 2 点をそのまま使い、向きは +x へ倒す。
    let frame = linear_frame(dim).unwrap_or_else(|| match linear_degenerate_dir_and_point(dim) {
        Some((dir, d)) => LinearFrame {
            d1: d,
            d2: d,
            dir,
            value: 0.0,
        },
        None => LinearFrame {
            d1: dim.p1,
            d2: dim.p2,
            dir: Vec2::new(1.0, 0.0),
            value: (dim.p2 - dim.p1).length(),
        },
    });

    let mut ex = expand_straight(
        frame.d1,
        frame.d2,
        frame.dir,
        frame.value,
        &dim.annotation,
        DimKind::Linear,
        render,
    );

    let gap = render.paper_mm_to_world(render.style.ext_gap_mm);
    let overshoot = render.paper_mm_to_world(render.style.ext_overshoot_mm);
    ex.segments
        .extend(extension_line(dim.p1, frame.d1, gap, overshoot));
    ex.segments
        .extend(extension_line(dim.p2, frame.d2, gap, overshoot));
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
/// 組版だけで、接頭辞 `R` は [`layout_dim_label`](super::layout_dim_label) が [`DimKind::Radial`] の既定記号
/// として供給する（ここで `R` を足すと二重に付く）。
#[must_use]
pub fn expand_radial(dim: &DimRadial, render: DimRender<'_>) -> DimExpansion {
    let (dir, pc) = radial_frame(dim);
    let arrow_len = render.arrow_len_world;
    let glyph = arrow_glyph(render.style.arrow_kind, arrow_len);
    let mut ex = DimExpansion::new(vec![[dim.center, pc]], vec![place_arrow(pc, dir, &glyph)]);

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
/// 共通（`expand_straight`）で、値ラベルは直径線の中央・上側に載る。補助線はない
/// （寸法の相手は円そのもの）。記号 φ は [`layout_dim_label`](super::layout_dim_label) が
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expand::dim_label::{ASCII_CHAR_WIDTH_RATIO, DimLabel, TextRun, layout_dim_label};
    use crate::{DimAnnotation, DimKind, DimStyle, FitClass, SizeTolerance};
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

    /// 配置済み矢先グリフの先端（＝ローカル原点を写した点）。
    ///
    /// 既定の [`ArrowKind::ClosedFilled`] は `[先端, 後端+半幅, 後端−半幅]` の三角形
    /// 1 枚なので、最初の塗り多角形の頂点 0 が先端になる（`arrow_glyph` の
    /// `closed_filled_is_a_triangle_with_a_30_degree_included_angle` が頂点順を固定
    /// している）。既定以外の種別を使うテストではこの helper を使わないこと。
    fn arrow_tip(glyph: &ArrowGlyph) -> Point2 {
        glyph.fills[0][0]
    }

    /// 矢が外向きか（先端から胴への向きが寸法線 `dir` の外側か）を展開結果から読む。
    /// [`arrow_tip`] と同じく既定の塗りつぶし三角形を前提にする。
    fn arrows_are_outward(ex: &DimExpansion, dir: Vec2) -> bool {
        let tri = &ex.arrows[0].fills[0];
        (tri[1].midpoint(tri[2]) - tri[0]).dot(dir) < 0.0
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
            direction: DimDirection::Aligned,
            annotation: DimAnnotation::default(),
        }
    }

    /// 回転寸法（絶対角 `theta` ラジアン）。計測 2 点・オフセットは
    /// [`plain_linear`] と同じ引数。
    fn rotated_linear(p1: Point2, p2: Point2, offset: f64, theta: f64) -> DimLinear {
        DimLinear {
            direction: DimDirection::Rotated(theta),
            ..plain_linear(p1, p2, offset)
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

    // --- 回転寸法（M11 タスク68）---

    #[test]
    fn rotated_linear_projects_the_measured_points_onto_the_dimension_line() {
        // 斜辺 (0,0)-(120,30) に付けた**水平**寸法（θ = 0、offset = 0）。
        // 寸法線は p1 を通る水平線なので、端点は (0,0) と (120,0)。
        let dim = rotated_linear(Point2::new(0.0, 0.0), Point2::new(120.0, 30.0), 0.0, 0.0);
        let segs = linear_pick_segments(&dim);
        assert_eq!(segs.len(), 3);
        assert!(approx(segs[0][0], Point2::new(0.0, 0.0)));
        assert!(approx(segs[0][1], Point2::new(120.0, 0.0)));
        // 補助線は各計測点からその足まで（p1 の足は p1 自身なので長さ 0）。
        assert!(approx(segs[1][0], Point2::new(0.0, 0.0)));
        assert!(approx(segs[1][1], Point2::new(0.0, 0.0)));
        assert!(approx(segs[2][0], Point2::new(120.0, 30.0)));
        assert!(approx(segs[2][1], Point2::new(120.0, 0.0)));
    }

    #[test]
    fn rotated_linear_measures_the_projected_length_not_the_distance() {
        // 同じ斜辺でも θ = 0 なら 120、θ = π/2 なら 30、整列なら実距離。
        let p1 = Point2::new(0.0, 0.0);
        let p2 = Point2::new(120.0, 30.0);
        assert!((rotated_linear(p1, p2, 0.0, 0.0).measured_value() - 120.0).abs() < T);
        assert!((rotated_linear(p1, p2, 0.0, FRAC_PI_2).measured_value() - 30.0).abs() < T);
        let aligned = plain_linear(p1, p2, 0.0).measured_value();
        assert!((aligned - 123.693_168_768_529_82).abs() < T);
        // 展開が使う値も同じ（ラベルの内容で確認する）。
        let ex = expand_linear(&rotated_linear(p1, p2, 20.0, 0.0), render(0.5, 1.0));
        assert_eq!(contents_of(&ex), vec!["120"]);
    }

    #[test]
    fn rotated_linear_offset_is_measured_along_the_dimension_line_normal() {
        // θ = π/2（鉛直な寸法線）。法線は perp((0,1)) = (-1,0) なので、
        // offset +20 は p1 から x 方向へ **−20** 進んだ側に寸法線を置く。
        let dim = rotated_linear(
            Point2::new(150.0, 0.0),
            Point2::new(150.0, 40.0),
            20.0,
            FRAC_PI_2,
        );
        let segs = linear_pick_segments(&dim);
        assert!(approx(segs[0][0], Point2::new(130.0, 0.0)));
        assert!(approx(segs[0][1], Point2::new(130.0, 40.0)));
        assert!((dim.measured_value() - 40.0).abs() < T);
    }

    #[test]
    fn rotated_linear_equals_aligned_when_the_angle_matches_the_measured_points() {
        // 計測 2 点が θ と平行なら、回転寸法は整列寸法と同じ図になる
        // （DXF import の受入条件が M11 タスク67 で依拠していた性質）。
        let p1 = Point2::new(1.0, 2.0);
        let p2 = Point2::new(1.0, 42.0);
        let aligned = linear_pick_segments(&plain_linear(p1, p2, 7.0));
        let rotated = linear_pick_segments(&rotated_linear(p1, p2, 7.0, FRAC_PI_2));
        assert_eq!(aligned.len(), rotated.len());
        for (a, r) in aligned.iter().zip(rotated.iter()) {
            assert!(approx(a[0], r[0]) && approx(a[1], r[1]), "{a:?} != {r:?}");
        }
    }

    #[test]
    fn rotated_linear_with_zero_projection_is_not_drawn() {
        // 計測 2 点が寸法線の法線上に並ぶ（投影長 0）。値 0 の寸法は描かない。
        let dim = rotated_linear(Point2::new(0.0, 0.0), Point2::new(0.0, 40.0), 5.0, 0.0);
        assert!(linear_pick_segments(&dim).is_empty());
        assert!((dim.measured_value() - 0.0).abs() < T);
        // 展開は破綻せず、寸法線長 0・向きは θ のままの骨格へ倒れる（値 0 では矢が
        // 外向きになるので、描かれる線は骨格の 1 点を中心に矢先長ぶん両側へ伸びる）。
        let ex = expand_linear(&dim, render(0.5, 1.0));
        let [a, b] = ex.segments[0];
        assert!(approx(a.midpoint(b), Point2::new(0.0, 5.0)));
        assert!((a.y - b.y).abs() < T, "寸法線は θ = 0 の向き: {a:?}-{b:?}");
        assert_eq!(contents_of(&ex), vec!["0"]);
    }

    #[test]
    fn rotated_linear_with_non_finite_angle_is_not_drawn() {
        // `EntityGeom::validate` が 3 境界で弾く値だが、展開が NaN を撒かないこと。
        let dim = rotated_linear(Point2::new(0.0, 0.0), Point2::new(4.0, 0.0), 2.0, f64::NAN);
        assert!(linear_pick_segments(&dim).is_empty());
        assert_eq!(dim.measured_value(), 0.0);
        let ex = expand_linear(&dim, render(0.5, 1.0));
        assert!(ex.segments.iter().flatten().all(|p| p.x.is_finite()));
    }

    #[test]
    fn rotated_linear_negative_projection_matches_swapped_measurement_order() {
        // θ = 0（水平）で計測点の順序が寸法線の向きと逆（p1 が右 120、p2 が左 0）。
        // Codex レビュー指摘1: `linear_frame` が返す描画用 `dir` を `(d2-d1)` へ揃えて
        // いないと、投影が負のときだけ矢先の向き・外向き延長の方向が反転する。
        // 矢の内外配置を `Outside` に固定して外向き延長を強制的に発生させる。
        fn outside(p1: Point2, p2: Point2) -> DimLinear {
            DimLinear {
                annotation: DimAnnotation {
                    arrow_placement: ArrowPlacement::Outside,
                    ..DimAnnotation::default()
                },
                ..rotated_linear(p1, p2, 5.0, 0.0)
            }
        }

        // 投影が負（バグの温床）と、p1/p2 を入れ替えた投影が正の参照。物理的には
        // 同じ寸法線（x=0〜x=120, y=5）を表すはずで、矢先・延長の形は一致するべき。
        let neg = outside(Point2::new(120.0, 0.0), Point2::new(0.0, 0.0));
        let pos = outside(Point2::new(0.0, 0.0), Point2::new(120.0, 0.0));
        let render = render(0.5, 1.0);
        let ex_neg = expand_linear(&neg, render);
        let ex_pos = expand_linear(&pos, render);

        // 寸法線の 2 端点は正しい位置（x=0〜120, y=5 のどこか）にあり、順序に依らず一致。
        let as_sorted = |[a, b]: [Point2; 2]| {
            if a.x <= b.x { [a, b] } else { [b, a] }
        };
        let [neg_lo, neg_hi] = as_sorted(ex_neg.segments[0]);
        let [pos_lo, pos_hi] = as_sorted(ex_pos.segments[0]);
        assert!(approx(neg_lo, pos_lo) && approx(neg_hi, pos_hi));
        assert!((neg_lo.y - 5.0).abs() < T && (neg_hi.y - 5.0).abs() < T);
        // 外向きなので寸法線は矢先ぶん両側へ延び、計測値 120 より長くなる
        // （内側へ縮むのが指摘1の症状）。
        assert!(
            neg_hi.x - neg_lo.x > 120.0,
            "外向きなのに寸法線が延長されていない: {neg_lo:?}-{neg_hi:?}"
        );

        // 矢先（先端位置と、先端から胴へ向かう向き）の集合は計測点の順序に依らず一致する。
        let arrow_key = |ex: &DimExpansion| -> Vec<(Point2, Vec2)> {
            ex.arrows
                .iter()
                .map(|g| {
                    let tri = &g.fills[0];
                    let tip = tri[0];
                    let back_mid = tri[1].midpoint(tri[2]);
                    (
                        tip,
                        (tip - back_mid).normalize().expect("矢の胴は退化しない"),
                    )
                })
                .collect()
        };
        let mut keys_neg = arrow_key(&ex_neg);
        let mut keys_pos = arrow_key(&ex_pos);
        let by_x = |v: &mut Vec<(Point2, Vec2)>| {
            v.sort_by(|a, b| a.0.x.partial_cmp(&b.0.x).unwrap());
        };
        by_x(&mut keys_neg);
        by_x(&mut keys_pos);
        assert_eq!(keys_neg.len(), 2);
        assert_eq!(keys_neg.len(), keys_pos.len());
        for ((p_neg, d_neg), (p_pos, d_pos)) in keys_neg.iter().zip(keys_pos.iter()) {
            assert!(
                approx(*p_neg, *p_pos),
                "矢先の位置が計測順序で変わった: {p_neg:?} != {p_pos:?}"
            );
            assert!(
                d_neg.dot(*d_pos) > 0.999,
                "矢先の向きが計測順序で変わった: {d_neg:?} != {d_pos:?}"
            );
        }
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
        assert!(approx(arrow_tip(&ex.arrows[0]), Point2::new(0.0, 2.0)));
        assert!(approx(arrow_tip(&ex.arrows[1]), Point2::new(4.0, 2.0)));
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
            direction: DimDirection::Aligned,
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
            direction: DimDirection::Aligned,
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
            direction: DimDirection::Aligned,
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

        // 100mm の寸法線には「100」+ 矢 2 つ（各 5mm＝既定 `arrow_len_mm`）が余裕で
        // 収まる → 内向き。
        let long = plain_linear(Point2::new(0.0, 0.0), Point2::new(100.0, 0.0), 2.0);
        let ex = expand_linear(&long, render(arrow_len, 1.0));
        assert!(!arrows_are_outward(&ex, dir));
        assert!(approx(ex.segments[0][0], Point2::new(0.0, 2.0)));
        assert!(approx(ex.segments[0][1], Point2::new(100.0, 2.0)));

        // 5mm では「5」+ 矢 2 つ（10mm）が収まらない → 外向き。矢先の先端は寸法線の端の
        // ままで、胴が外へ出る。矢が乗る線がなくなるので寸法線を両端へ矢先長ぶん延長する。
        let short = plain_linear(Point2::new(0.0, 0.0), Point2::new(5.0, 0.0), 2.0);
        let ex = expand_linear(&short, render(arrow_len, 1.0));
        assert!(arrows_are_outward(&ex, dir));
        assert!(approx(arrow_tip(&ex.arrows[0]), Point2::new(0.0, 2.0)));
        assert!(approx(arrow_tip(&ex.arrows[1]), Point2::new(5.0, 2.0)));
        assert!(approx(ex.segments[0][0], Point2::new(-arrow_len, 2.0)));
        assert!(approx(ex.segments[0][1], Point2::new(5.0 + arrow_len, 2.0)));
    }

    #[test]
    fn manual_arrow_placement_overrides_the_automatic_decision() {
        let dir = Vec2::new(1.0, 0.0);
        let with = |placement, p2x: f64| DimLinear {
            p1: Point2::new(0.0, 0.0),
            p2: Point2::new(p2x, 0.0),
            offset: 2.0,
            direction: DimDirection::Aligned,
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
        assert!(approx(arrow_tip(&ex.arrows[0]), Point2::new(12.5, 0.0)));
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
        assert!(approx(arrow_tip(&ex.arrows[0]), Point2::new(-30.0, 0.0)));
        assert!(approx(arrow_tip(&ex.arrows[1]), Point2::new(30.0, 0.0)));
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

    // --- 矢先のブロック化（M10 タスク63） ---

    /// [`ArrowKind`] の全既知バリアント（geom 側は `#[non_exhaustive]` なのでここに列挙
    /// する。[`ALL_SYMBOLS`] と同じ流儀で、**種別が増えたらここへ足すこと**）。
    const ALL_ARROW_KINDS: [ArrowKind; 7] = [
        ArrowKind::ClosedFilled,
        ArrowKind::ClosedBlank,
        ArrowKind::Open30,
        ArrowKind::Open90,
        ArrowKind::Oblique,
        ArrowKind::Dot,
        ArrowKind::None,
    ];

    /// 矢先種別だけを差し替えた展開パラメータ（尺度 1:1）。
    fn render_with_arrow_kind(
        style: &DimStyle,
        arrow_len_world: f64,
        text_height_world: f64,
    ) -> DimRender<'_> {
        DimRender {
            style,
            scale_world_per_paper_mm: 1.0,
            arrow_len_world,
            text_height_world,
        }
    }

    /// 展開結果の矢先は「geom の純関数が作った形を先端・向きへ写しただけ」である。
    ///
    /// 種別ごとの形の責務は geom にしかない（app 側にも消費側にも分岐が無い）ことを
    /// 全種別で固定する。画面（`draw_dim_expansion`）と SVG/PDF（`plot::push_dim`）は
    /// この同じ [`DimExpansion`] を描くので、これが「7 種が画面と出力で同一に描かれる」
    /// ことの app 層側の担保になる。
    #[test]
    fn expansion_arrows_are_the_geom_glyph_placed_at_the_line_ends() {
        let dim = plain_linear(Point2::new(0.0, 0.0), Point2::new(100.0, 0.0), 2.0);
        for kind in ALL_ARROW_KINDS {
            let style = DimStyle {
                arrow_kind: kind,
                ..DimStyle::DEFAULT
            };
            let ex = expand_linear(&dim, render_with_arrow_kind(&style, 4.0, 1.0));
            assert_eq!(ex.arrows.len(), 2, "{kind:?}");
            // 100mm の寸法線にはどの種別でも収まる → 内向き（先端は寸法線の両端で、
            // 胴は内側へ出るので向きは d1 が −x、d2 が +x）。
            let glyph = arrow_glyph(kind, 4.0);
            assert_eq!(
                ex.arrows[0],
                place_arrow(Point2::new(0.0, 2.0), Vec2::new(-1.0, 0.0), &glyph),
                "{kind:?}"
            );
            assert_eq!(
                ex.arrows[1],
                place_arrow(Point2::new(100.0, 2.0), Vec2::new(1.0, 0.0), &glyph),
                "{kind:?}"
            );
        }
    }

    /// 設計確定4: 寸法線に沿って場所を取らない種別（斜線・点・なし）は、自動判定でも
    /// 手動 [`ArrowPlacement::Outside`] でも外向きにならず、寸法線も延長されない。
    #[test]
    fn arrow_kinds_without_line_occupancy_never_extend_the_dimension_line() {
        let arrow_len = 0.5;
        let with = |kind, placement| {
            (
                DimStyle {
                    arrow_kind: kind,
                    ..DimStyle::DEFAULT
                },
                DimLinear {
                    p1: Point2::ORIGIN,
                    // 5mm は「5」+ 矢 2 つが収まらない長さ（自動なら確実に外向き）。
                    p2: Point2::new(5.0, 0.0),
                    offset: 2.0,
                    direction: DimDirection::Aligned,
                    annotation: DimAnnotation {
                        arrow_placement: placement,
                        ..DimAnnotation::default()
                    },
                },
            )
        };
        let placements = [
            ArrowPlacement::Auto,
            ArrowPlacement::Inside,
            ArrowPlacement::Outside,
        ];

        for kind in [ArrowKind::Oblique, ArrowKind::Dot, ArrowKind::None] {
            assert!(!arrow_kind_occupies_line(kind), "{kind:?}");
            for placement in placements {
                let (style, dim) = with(kind, placement);
                let ex = expand_linear(&dim, render_with_arrow_kind(&style, arrow_len, 1.0));
                assert!(
                    approx(ex.segments[0][0], Point2::new(0.0, 2.0))
                        && approx(ex.segments[0][1], Point2::new(5.0, 2.0)),
                    "{kind:?} / {placement:?} で寸法線が延長された: {:?}",
                    ex.segments[0]
                );
            }
        }

        // 対照: 場所を取る 4 種は同じ寸法で外向きになり、両端が矢先長ぶん伸びる。
        for kind in [
            ArrowKind::ClosedFilled,
            ArrowKind::ClosedBlank,
            ArrowKind::Open30,
            ArrowKind::Open90,
        ] {
            assert!(arrow_kind_occupies_line(kind), "{kind:?}");
            let (style, dim) = with(kind, ArrowPlacement::Auto);
            let ex = expand_linear(&dim, render_with_arrow_kind(&style, arrow_len, 1.0));
            // 延長量は矢先長ではなく glyph の along_len（Open90 は len/2）。
            let reach = mcad_geom::arrow_glyph(kind, arrow_len).along_len;
            assert!(
                approx(ex.segments[0][0], Point2::new(-reach, 2.0))
                    && approx(ex.segments[0][1], Point2::new(5.0 + reach, 2.0)),
                "{kind:?} で寸法線が延長されていない: {:?}",
                ex.segments[0]
            );
        }
    }

    /// 半径寸法の矢先も文書スタイルの種別に従う（長さ寸法と同じ経路であることの固定）。
    #[test]
    fn radial_arrow_follows_the_document_arrow_kind() {
        let dim = DimRadial {
            center: Point2::ORIGIN,
            radius: 12.5,
            leader_angle: 0.0,
            annotation: DimAnnotation::default(),
        };
        for kind in ALL_ARROW_KINDS {
            let style = DimStyle {
                arrow_kind: kind,
                ..DimStyle::DEFAULT
            };
            let ex = expand_radial(&dim, render_with_arrow_kind(&style, 0.5, 1.0));
            assert_eq!(ex.arrows.len(), 1, "{kind:?}");
            assert_eq!(
                ex.arrows[0],
                place_arrow(
                    Point2::new(12.5, 0.0),
                    Vec2::new(1.0, 0.0),
                    &arrow_glyph(kind, 0.5)
                ),
                "{kind:?}"
            );
        }
    }

    // --- ラベルの組版（タスク49-1） ---

    /// 組版テストの基準文字高さ。ASCII 1 文字の近似幅がちょうど 5.5 になる値を選び、
    /// 期待値を手計算できるようにする。
    const H: f64 = 10.0;

    /// 基準文字高さ [`H`] における ASCII 1 文字の近似送り幅。
    const CW: f64 = H * ASCII_CHAR_WIDTH_RATIO;

    /// [`DimSymbol`] の全バリアント（geom 側は `#[non_exhaustive]` なのでここに列挙する。
    /// 記号が増えたらこの配列を更新すること。`crate::dim` の同名配列と同じ流儀）。
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
            direction: DimDirection::Aligned,
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
