//! エンティティの幾何を表す [`EntityGeom`]（M6「テキストと寸法」で新設）。
//!
//! # 設計方針（DESIGN.md M6）
//!
//! MVP の [`mcad_geom::Shape`]（点・線・円・円弧・ポリライン）に加えて、テキストと
//! 寸法（長さ・半径）を扱えるようにする層。mcad-geom の [`Shape`] は **変更しない**
//! （`Shape` へバリアントを足すと mcad 内の網羅 match と tcad の `EntityKind::of()` を
//! 直接壊すため。DESIGN.md M6 設計判断1）。代わりに mcad-core 側で `Shape` を包含する
//! [`EntityGeom`] を定義し、[`crate::Entity`] の幾何をこの型で持つ。
//!
//! # 幾何変換の規則
//!
//! [`Shape`] バリアントは既存の [`Shape`] のメソッドへそのまま委譲する。テキスト・寸法は
//! それぞれの意味に沿って変換する（各メソッドの doc 参照）。特に **テキストのミラーは
//! アンカーのみ鏡映し、文字グリフは反転しない**（グリフ反転は egui では不可能。
//! AutoCAD の MIRRTEXT=0 相当。DESIGN.md M6 設計判断1）。
//!
//! **表（[`TableGeom`]、M10）はさらに割り切って、平行移動・回転・鏡映のいずれでも
//! アンカーだけを変換する**（回転角フィールドを持たない。DESIGN.md M10 設計方針3）。

use serde::{Deserialize, Serialize};

use mcad_geom::{Aabb, Point2, Shape, Vec2};

use crate::dim::{DimAnnotation, DimKind};

/// テキストエンティティの幾何。フォント指定は持たない（M6 は埋め込み 1 書体のみ。
/// DESIGN.md M6 設計判断1）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextGeom {
    /// 文字列の基準点（ベースライン左端）。ワールド座標。
    pub anchor: Point2,
    /// 表示する文字列。
    pub content: String,
    /// 文字高さ（紙 mm。DESIGN.md M8 設計判断4）。ワールド高さは
    /// `height * k`（`k` = `Scale::world_mm_per_paper_mm`）で導出する。既定尺度 1:1 では
    /// `k = 1` なので数値・見た目とも従来（ワールド単位解釈）と一致する。この換算は
    /// `mcad-app` 側の責務（[`EntityGeom::aabb`] は 1:1 解釈のまま。表示・選択判定用の
    /// 尺度反映 AABB は app 層の `text_world_aabb` が担う）。
    pub height: f64,
    /// ベースラインの回転角（ラジアン、CCW）。
    pub angle: f64,
}

/// 長さ寸法の**寸法線の向き**（M11 タスク68、DESIGN.md M11-0(3)）。
///
/// # 水平・鉛直の専用バリアントを設けない
///
/// 水平は [`DimDirection::Rotated`]`(0.0)`、鉛直は `Rotated(π/2)` で表す。データとしては
/// 同一で、UI のボタンが角度を入れるだけだからである。専用バリアントを足すと
/// 「水平を `Horizontal` と `Rotated(0)` のどちらで保存するか」という**意味の無い分岐**が
/// core・io・app の 3 層すべてに生じる（DESIGN.md M11-0(3)）。
///
/// # `#[non_exhaustive]`
///
/// 将来の向き指定（例: 他エンティティへ従属する向き）を足せるようにするため、
/// 下位クレート・外部クレートからの網羅 `match` を禁じる。[`DimKind`] を
/// 意図的に閉じた列挙にしているのとは逆の判断で、理由は「バリアントが増えても
/// 各所で必ず更新しなければならない表が無い」こと（向きの解決は
/// [`DimDirection::unit_vector`] 1 箇所に閉じている）。
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DimDirection {
    /// 計測 2 点を結ぶ向き（**整列寸法**）。既定値であり、v6 以前の `.mcad` は
    /// すべてこれとして読む。展開は M10 までと完全に同一（[`crate::expand_linear`]）。
    #[default]
    Aligned,
    /// 絶対角（**ラジアン**、CCW、+x 軸基準）で固定した向き（**回転寸法**）。
    ///
    /// 寸法線はこの向きに引かれ、計測 2 点はそこへ投影される。したがって表示値は
    /// 計測 2 点の実距離ではなく**投影長**になる（[`DimLinear::measured_value`]）。
    /// DXF の group 50（度）の写像先で、度→ラジアンの変換は io 境界に閉じる
    /// （DESIGN.md M11-0(2)）。
    Rotated(f64),
}

impl DimDirection {
    /// 寸法線方向の単位ベクトル。向きが決まらなければ `None`。
    ///
    /// - [`DimDirection::Aligned`]: `(p2 − p1)` を正規化した向き。計測 2 点がほぼ同一
    ///   （[`mcad_geom::EPS`] 以下）なら `None`。
    /// - [`DimDirection::Rotated`]: `(cos θ, sin θ)`。θ が非有限なら `None`
    ///   （[`EntityGeom::validate`] が 3 境界で弾くが、検証を通っていない値で
    ///   NaN を撒かないための防御）。
    ///
    /// **寸法線の位置・測定値・当たり判定はすべてこの向きから導く**ので、向きの
    /// 解決はこのメソッド 1 箇所に閉じる。
    #[must_use]
    pub fn unit_vector(self, p1: Point2, p2: Point2) -> Option<Vec2> {
        match self {
            DimDirection::Aligned => (p2 - p1).normalize(),
            DimDirection::Rotated(theta) => {
                if !theta.is_finite() {
                    return None;
                }
                let (sin, cos) = theta.sin_cos();
                Some(Vec2::new(cos, sin))
            }
        }
    }
}

/// 長さ寸法（非関連の静的寸法）。計測対象への参照は持たず、作成時に座標を採取する
/// （DESIGN.md M6 設計判断2）。表示値は保存せず、描画時に計算する側の責務とする
/// （点が編集されれば値も追従する。計算式は [`DimLinear::measured_value`]）。
///
/// **`Copy` ではない**（M9 タスク47-2）: [`DimAnnotation`] が [`crate::FitClass`]（`String`）を
/// 持ちうるため。値渡しの箇所は `.clone()` へ切り替える。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DimLinear {
    /// 計測点その 1。
    pub p1: Point2,
    /// 計測点その 2。
    pub p2: Point2,
    /// **`p1` から寸法線までの、寸法線方向の法線に沿った符号付き距離**
    /// （法線は `dir.perp()` = `dir` を左 90 度回した向き。`dir` は
    /// [`DimDirection::unit_vector`]）。すなわち寸法線は
    /// `p1 + dir.perp() * offset` を通る `dir` 方向の直線で、その 2 端点は
    /// 計測 2 点をこの直線へ投影した点になる。
    ///
    /// **[`DimDirection::Aligned`] では `dir = (p2 − p1)/|p2 − p1|` なので、
    /// M10 までの「計測線から寸法線までの符号付き距離」と完全に同じ意味**
    /// （投影が恒等になり、寸法線の 2 端点は `p1 + shift` / `p2 + shift` そのもの）。
    /// v6 以前のファイルの見た目は変わらない（M11 タスク68 で意味を一般化した）。
    pub offset: f64,
    /// 寸法線の向き（M11 タスク68）。既定は [`DimDirection::Aligned`]（整列寸法）で、
    /// v6 以前の `.mcad` はこの値で補完される。
    #[serde(default)]
    pub direction: DimDirection,
    /// **寸法補助線の傾き**（絶対角・**ラジアン**・CCW・+x 軸基準。M11 タスク78、
    /// DESIGN.md M11 設計判断6）。`None`（既定）は**寸法線に垂直**で、描画は
    /// M11 タスク72 までと完全に同一。
    ///
    /// `Some(θ)` のとき、寸法線の 2 端点は「計測点を通る角度 θ の直線」と寸法線の
    /// **交点**になる（[`crate::expand_linear`]）。寸法線の直線そのもの（[`offset`] の
    /// 意味）は変わらない。製図規定 5-4 2) a)「寸法補助線は図形の輪郭線に垂直に引く」を
    /// 回転寸法でも満たすための指定で、mcad の寸法は非関連（図形への参照を持たない）
    /// ため輪郭線の向きはユーザーが与えるしかない。
    ///
    /// **DXF の group 52（`extension_line_angle`）とは往復しない。** 52 は基準角
    /// （絶対角か group 50 への加算か）を一次資料から確定できないため import では
    /// 取り込まず捨てて計上する（理由は `mcad-io` の
    /// `ImportSummary::dropped_dimension_details` の doc）。export は寸法を線へ分解して
    /// 書くので、ここで傾けた補助線は傾いたまま DXF へ出る。
    ///
    /// **表示値（[`DimLinear::measured_value`]）は傾きで変わらない**（寸法線方向への
    /// 投影長のまま。AutoCAD の oblique dimension と同じ）。そのため
    /// [`DimDirection::Rotated`] に傾いた補助線を付けると、寸法線の長さ `|d2 − d1|` と
    /// 表示値は一般に一致しない（[`DimDirection::Aligned`] では 2 本の補助線が
    /// 等しくずれるので一致したまま）。
    ///
    /// # 角度の範囲は正規化も拒否もしない
    ///
    /// 絶対角なので `2π` を超えても `−π` 未満でも向きの意味は一意に決まる
    /// （`sin_cos` が同じ単位ベクトルを返す）。範囲へ丸めると回転・鏡映のたびに値が
    /// 跳ね、`.mcad` の往復で保存値が書き換わる。[`DimDirection::Rotated`] と同じ規則で、
    /// **3 境界（[`EntityGeom::validate`] / `.mcad` 読込 / DXF import）が拒否するのは
    /// 非有限の θ だけ**。
    ///
    /// # 寸法線と平行なら補助線を引かない
    ///
    /// θ が寸法線と平行（交点が出ない）なら**補助線を描かず**、寸法線の端点は `None`
    /// と同じ垂線の足へ倒す（寸法線と値ラベルは描く）。退化を黙って別物へ倒さない
    /// 既存流儀に合わせたもので、判定は `crate::expand::linear_frame` 1 箇所
    /// （展開・pick・AABB が同じ骨格を共有する）。
    ///
    /// [`offset`]: DimLinear::offset
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ext_angle: Option<f64>,
    /// 寸法補助記号・公差などの注記（M9 設計判断2）。既定は無注記で、そのときの描画は
    /// M8 までと完全に同じ。許容記号は φ/Sφ/□/C/t（[`DimKind::Linear`]）。
    #[serde(default, skip_serializing_if = "DimAnnotation::is_unannotated")]
    pub annotation: DimAnnotation,
}

impl DimLinear {
    /// 表示する測定値（**寸法線の 2 端点間の距離**）。
    ///
    /// [`DimDirection::Aligned`] では計測 2 点の実距離 `|p2 − p1|`、
    /// [`DimDirection::Rotated`] では寸法線方向への投影長 `|(p2 − p1)・dir|` になる。
    /// θ が非有限で向きが決まらないときだけ `0.0`（`Aligned` の退化は `|p2 − p1|`
    /// がそのまま 0 近傍の値になるので、特別扱いは要らない）。
    ///
    /// **`Aligned` は `|p2 − p1|` を直接計算する**（投影の一般式
    /// `(p2 − p1)・dir` を通さない）。数学的には同値だが、正規化と内積を経由すると
    /// 最終桁が動きうるため、M10 以前に保存された図面の描画を**ビット単位で**
    /// 保つほうを優先した（回帰網は `mcad-app` の寸法スナップショット 29 ケース）。
    ///
    /// **[`DimLinear::ext_angle`]（補助線の傾き、M11 タスク78）はこの値を動かさない。**
    /// 傾きが変えるのは寸法線の端点だけで、表示値は寸法線方向への投影長のままである
    /// （AutoCAD の oblique dimension と同じ。DESIGN.md M11 設計判断6）。
    #[must_use]
    pub fn measured_value(&self) -> f64 {
        match self.direction {
            DimDirection::Aligned => (self.p2 - self.p1).length(),
            DimDirection::Rotated(_) => match self.direction.unit_vector(self.p1, self.p2) {
                Some(dir) => (self.p2 - self.p1).dot(dir).abs(),
                None => 0.0,
            },
        }
    }
}

/// 半径寸法（非関連の静的寸法）。作成時に円／円弧から中心・半径を採取する
/// （DESIGN.md M6 設計判断2）。
///
/// `Copy` でない理由は [`DimLinear`] と同じ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DimRadial {
    /// 円／円弧の中心。
    pub center: Point2,
    /// 半径。
    pub radius: f64,
    /// 引出線の方向（ラジアン、中心からの放射角）。
    pub leader_angle: f64,
    /// 寸法補助記号・公差などの注記（M9 設計判断2）。許容記号は R/SR/CR
    /// （[`DimKind::Radial`]）。
    #[serde(default, skip_serializing_if = "DimAnnotation::is_unannotated")]
    pub annotation: DimAnnotation,
}

/// 直径寸法（非関連の静的寸法。M9 設計判断3 で新設）。
///
/// 寸法線は中心を通る `angle` 方向の直径線で、矢は円周上の 2 点に立つ。半径寸法に
/// φ 記号を上書きする運用（R12 と φ24 の取り違え）を防ぐために、直径は独立した
/// 種別として持つ。
///
/// `angle` は [`DimRadial::leader_angle`] と同じ「中心からの放射角（ラジアン、CCW）」で、
/// 直径線はその方向とその逆方向の 2 点を結ぶ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DimDiameter {
    /// 円／円弧の中心。
    pub center: Point2,
    /// 半径（**直径ではない**）。表示値は `2 * radius` を描画側が計算する。
    ///
    /// 半径で持つのは、採取元の [`mcad_geom::Circle`] や [`DimRadial`] と同じ量にして
    /// 相互変換・比較を素直にするため（`2r` は表示のたびに導出できる）。
    pub radius: f64,
    /// 寸法線（直径線）の方向（ラジアン、中心からの放射角）。
    pub angle: f64,
    /// 寸法補助記号・公差などの注記（M9 設計判断2）。許容記号は φ/Sφ
    /// （[`DimKind::Diameter`]）。
    #[serde(default, skip_serializing_if = "DimAnnotation::is_unannotated")]
    pub annotation: DimAnnotation,
}

/// 座標寸法（[`DimOrdinate`]）がどちらの座標成分を測るか（M11 タスク69）。
///
/// # 図面の X/Y は**用紙の軸**であって図形に付随しない
///
/// [`EntityGeom::rotated`] / [`EntityGeom::mirrored`] はこの値を**変えない**。
/// 座標寸法は「基準（原点）から見た X 座標／Y 座標」を読む寸法で、その X/Y は
/// 図面（用紙）の軸である。図形を 90 度回したからといって「X 座標を読む寸法」が
/// 「Y 座標を読む寸法」に化けてよいわけではなく、そうなると同じ図面の中で
/// 座標系が図形ごとに違うことになる。回転・鏡映で変わるのは 3 つの点だけで、
/// 結果として読み取られる値（成分）が変わる。
///
/// # `#[non_exhaustive]`
///
/// 将来の軸指定（任意方向の基準軸など）を足せるようにするため。
/// [`DimDirection`] と同じ判断で、軸の解決は [`OrdinateAxis::component`] 1 箇所に
/// 閉じている。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum OrdinateAxis {
    /// X 座標（用紙の水平方向）を測る。
    X,
    /// Y 座標（用紙の鉛直方向）を測る。
    Y,
}

impl OrdinateAxis {
    /// 変位 `delta` のうち、この軸が読む成分。
    ///
    /// 軸の解決はこのメソッド 1 箇所に閉じる（`#[non_exhaustive]` の前提）。
    #[inline]
    #[must_use]
    pub fn component(self, delta: Vec2) -> f64 {
        match self {
            OrdinateAxis::X => delta.x,
            OrdinateAxis::Y => delta.y,
        }
    }
}

/// 角度寸法の反平行判定に使う閾値（ラジアン）。
///
/// [`EntityGeom::validate`]（`DimAngular`）が、`p1` 方向と `p2` 方向のなす角と π の差が
/// この値未満なら反平行（180 度ちょうど）として拒否する。境界は「意図して 180 度ちょうどを
/// 作った場合だけを弾き、179.99 度のような通常の角度は通す」ために選んだ。f64 の `acos` は
/// 1e-8 程度の丸め誤差を持ちうるので、それより十分大きく、かつ通常の作図で意図せず踏むには
/// 小さすぎる値として `1e-6` を取った（M11 タスク69）。
///
/// `crates/mcad-app/src/tool.rs` の `DimAngularTool` も、弧位置クリックより前（2 辺目の
/// クリック時点）で同じ基準を先回りして使うため、この定数を経由して共有する
/// （Codex adversarial review 2026-09-21 指摘、M11 タスク74: 以前は値をリテラルで複製していた）。
pub const DIM_ANGULAR_ANTIPARALLEL_EPS: f64 = 1e-6;

/// 角度寸法（非関連の静的寸法。M11 タスク69 で新設）。
///
/// 頂点 [`DimAngular::vertex`] と、そこから見た 2 方向を決める点
/// [`DimAngular::p1`] / [`DimAngular::p2`] で角を表す。**頂点から `p1` / `p2` までの
/// 距離は寸法に使わない**（方向だけを取る）。寸法線となる弧の位置は
/// [`DimAngular::arc_radius`] が独立に決める。
///
/// # 値は 2 方向のなす角（0〜π）
///
/// [`DimAngular::measured_value`] は**ラジアン**で返し、度への変換と `°` の付加は
/// 表示側（[`crate::expand_angular`] → [`crate::layout_dim_label`]）が行う。
/// mcad の角度はすべてラジアンという既存契約（[`TextGeom::angle`]・
/// [`DimDiameter::angle`]・[`DimDirection::Rotated`]）に揃えてある。
///
/// **優角（180 度を超える角）は扱わない。** 2 方向のなす角は常に短い側を取るので、
/// `p1` と `p2` を入れ替えても値も描かれる弧も変わらない。
///
/// # 反平行（180 度ちょうど）は拒否する
///
/// 180 度未満なら「短い側」が一意に決まるが、`p1` 方向と `p2` 方向がちょうど
/// 反平行（なす角 = π）のときは短い側が 2 つの半円のどちらとも決まらない。
/// 実装は弧の開始角を `p1` の方向から取るため、`p1`/`p2` を入れ替えると測定値は
/// π のままで弧・補助線・矢先・pick が反対側の半円へ飛ぶ（保存データだけからは
/// どちらが正しいか分からない）。この不変条件を保つため
/// [`EntityGeom::validate`] が反平行を拒否する（Codex adversarial review 指摘、
/// M11 タスク69）。弧の側をモデルへ追加で持たせる案は、180 度の角度寸法が実務で
/// ほぼ使われない一方 v7 スキーマと DXF 写像への影響が大きいため採らなかった。
///
/// `Copy` でない理由は [`DimLinear`] と同じ（[`DimAnnotation`] が `String` を持ちうる）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DimAngular {
    /// 角の頂点（弧の中心でもある）。
    pub vertex: Point2,
    /// 一方の辺の向きを決める点（`vertex` からの距離は使わない）。
    pub p1: Point2,
    /// もう一方の辺の向きを決める点（`vertex` からの距離は使わない）。
    pub p2: Point2,
    /// 寸法線となる弧の半径（`vertex` からの距離。ワールド長、正）。
    pub arc_radius: f64,
    /// 寸法補助記号・公差などの注記（M9 設計判断2）。許容記号は**なし**
    /// （[`DimKind::Angular`]）。
    #[serde(default, skip_serializing_if = "DimAnnotation::is_unannotated")]
    pub annotation: DimAnnotation,
}

impl DimAngular {
    /// 表示する測定値（2 方向のなす角、**ラジアン**。`0 <= v <= π`）。
    ///
    /// `vertex` と `p1`（または `p2`）がほぼ同一で向きが決まらないときは `0.0`。
    /// 表示側は `to_degrees()` して `°` を付ける（[`crate::expand_angular`]）。
    #[must_use]
    pub fn measured_value(&self) -> f64 {
        match self.directions() {
            // `clamp` は `dot` が丸めで ±1 を僅かに超えたときに `acos` が NaN を
            // 返すのを防ぐ最終防衛線（単位ベクトル同士なので数学的には範囲内）。
            Some((u1, u2)) => u1.dot(u2).clamp(-1.0, 1.0).acos(),
            None => 0.0,
        }
    }

    /// 頂点から見た 2 辺の単位方向ベクトル。どちらかが決まらなければ `None`。
    ///
    /// 値・弧・補助線・当たり判定のすべてがこの 2 方向から導かれるので、
    /// 向きの解決はこのメソッド 1 箇所に閉じる（[`DimDirection::unit_vector`] と
    /// 同じ流儀）。
    #[must_use]
    pub fn directions(&self) -> Option<(Vec2, Vec2)> {
        let u1 = (self.p1 - self.vertex).normalize()?;
        let u2 = (self.p2 - self.vertex).normalize()?;
        Some((u1, u2))
    }
}

/// 座標寸法（非関連の静的寸法。M11 タスク69 で新設）。
///
/// 基準点 [`DimOrdinate::origin`] から計測点 [`DimOrdinate::feature`] までの、
/// [`DimOrdinate::axis`] が指す座標成分を**符号つきで**読む寸法。引出線は
/// `feature` から [`DimOrdinate::leader_end`] へ 1 本だけ引く。
///
/// `Copy` でない理由は [`DimLinear`] と同じ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DimOrdinate {
    /// 座標の基準点（DXF の group 10、作成時 UCS 原点に相当）。
    pub origin: Point2,
    /// 計測点（引出線の起点）。
    pub feature: Point2,
    /// 引出線の終点（値ラベルはこの先へ置く）。
    pub leader_end: Point2,
    /// どちらの座標成分を読むか。**回転・鏡映でも変わらない**
    /// （理由は [`OrdinateAxis`] の doc）。
    pub axis: OrdinateAxis,
    /// 寸法補助記号・公差などの注記（M9 設計判断2）。許容記号は**なし**
    /// （[`DimKind::Ordinate`]）。
    #[serde(default, skip_serializing_if = "DimAnnotation::is_unannotated")]
    pub annotation: DimAnnotation,
}

impl DimOrdinate {
    /// 表示する測定値（`feature − origin` の [`DimOrdinate::axis`] 成分、**符号つき**）。
    ///
    /// 負の値は負のまま表示する（`-30` など）。絶対値を取らないのは、座標寸法が
    /// 「基準からどちら側か」を含めて読む寸法だからである。
    #[inline]
    #[must_use]
    pub fn measured_value(&self) -> f64 {
        self.axis.component(self.feature - self.origin)
    }
}

/// 汎用テーブルの幾何（表・部品表の土台。DESIGN.md M10 設計方針1・詳細設計1）。
///
/// # 座標系と単位
///
/// [`TableGeom::anchor`] は**表の左下**（ワールド座標）。列幅・行高さ・文字高さは
/// すべて**紙 mm** で、[`TextGeom::height`] と同じ契約に従う（[`EntityGeom::aabb`] は
/// 1:1 解釈。尺度 `k` を反映した表示上の AABB と罫線の組版は `mcad-app` の責務。
/// M10 設計方針4）。
///
/// アンカーが左下なので、行の増減で表は**上方向へ伸縮**し左下は動かない。部品表は
/// 表題欄の直上へ置き、ヘッダを最下段にして部品行を上へ積む（M10 詳細設計2）。
///
/// # セルの並び
///
/// [`TableGeom::cells`] は**行優先**で、**行 0 が最上段**
/// （[`crate::TitleBlockTemplate::rows`] と同じ向き）。`(r, c)` の要素は
/// `cells[r * cols() + c]` で、取り出しには [`TableGeom::cell`] を使う。
///
/// # 持たないもの
///
/// **回転角を持たない**（M10 設計方針3）。罫線の太さ・セル内パディング・文字の寄せも
/// 保存しない（[`crate::expand_table`] の定数で表題欄と同じ見え方に揃える。M10 詳細設計1）。セル結合・
/// 右寄せ/中央寄せ・行ごとの UI 個別高さは non-goal（M10 詳細設計10）。
///
/// **`Copy` ではない**（`Vec` / `String` を持つため）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableGeom {
    /// 表の左下（ワールド座標）。
    pub anchor: Point2,
    /// 各列の幅（紙 mm、左から右へ）。要素数が列数。
    pub col_widths_mm: Vec<f64>,
    /// 各行の高さ（紙 mm、**上から下へ**。`row_heights_mm[0]` が最上段）。要素数が行数。
    pub row_heights_mm: Vec<f64>,
    /// セル文字の高さ（紙 mm）。全セル共通（セルごとの文字高さは non-goal）。
    pub text_height_mm: f64,
    /// セル文字列（行優先、行 0 = 最上段）。要素数はちょうど `rows() * cols()`。
    ///
    /// **空文字列は正当な値**（空欄のある表が普通なので、[`TextGeom::content`] と違って
    /// 空を拒否しない。M10 詳細設計1）。
    pub cells: Vec<String>,
}

/// 表の寸法（個々の列幅・行高さ・文字高さと、それらの合計）として許す最大の紙 mm 値。
///
/// 値と根拠は [`crate::MAX_TITLE_BLOCK_MM`] と同じ（用紙の最大は A0 の 1189mm）。
/// 「用紙に載らない表を早期に弾く」と「非有限・巨大値を配置計算や SVG/PDF 座標へ
/// 流さない」を 1 つの定数で兼ねる。個々の値が上限内でも合計はあふれうるので、
/// [`EntityGeom::validate`] は**合計側にも同じ上限を課す**（表題欄と同じ論点）。
pub const MAX_TABLE_MM: f64 = 10_000.0;

/// 表が持てる行数の上限。
///
/// 寸法の上限（[`MAX_TABLE_MM`]）だけでは、極小の行高さ（1e-6mm など）を大量に並べて
/// 合計を上限以下に保ったまま任意個数の行を通す細工を防げない。表は表題欄と同じく
/// **毎フレーム全セルを走査して描く**（M10 タスク58 の展開）ため、細工された `.mcad` が
/// 持続的な GUI 停止を起こしうる。この論点と対処は [`crate::MAX_TITLE_BLOCK_ROWS`] と
/// 同じ（Codex M8 アドバーサリアルレビュー 2 周目 (2026-08-02) medium 指摘）。
///
/// 値 512 の根拠: 実運用の部品表は数十行、多くても 100 行規模なので実用に対して
/// 5 倍前後の余裕がありながら、展開コストを構造的に定数へ抑えられる。表題欄の 32 より
/// 大きいのは、部品表が「行を積んでいく」表で 32 行では足りないため。
pub const MAX_TABLE_ROWS: usize = 512;

/// 表が持てる列数の上限。論点と根拠は [`MAX_TABLE_ROWS`] と同じ。
///
/// 値 64 の根拠: 規定の部品欄は 5 列（照合番号・名称・個数・材質・備考）なので
/// 1 桁の余裕がある。[`MAX_TABLE_ROWS`] と合わせてセル数は 32768 以下に収まる。
pub const MAX_TABLE_COLS: usize = 64;

impl TableGeom {
    /// 行数（[`TableGeom::row_heights_mm`] の要素数）。
    #[inline]
    #[must_use]
    pub fn rows(&self) -> usize {
        self.row_heights_mm.len()
    }

    /// 列数（[`TableGeom::col_widths_mm`] の要素数）。
    #[inline]
    #[must_use]
    pub fn cols(&self) -> usize {
        self.col_widths_mm.len()
    }

    /// `r` 行 `c` 列（行 0 = 最上段、列 0 = 左端）のセル文字列。範囲外は `None`。
    ///
    /// [`EntityGeom::validate`] を通していない値（`cells` の要素数が行 × 列と食い違う、
    /// 行数・列数が極端に大きい）でも `None` を返すだけで、panic もオーバーフローも
    /// しない。
    #[must_use]
    pub fn cell(&self, r: usize, c: usize) -> Option<&str> {
        if r >= self.rows() || c >= self.cols() {
            return None;
        }
        let index = r.checked_mul(self.cols())?.checked_add(c)?;
        self.cells.get(index).map(String::as_str)
    }

    /// 表全体の幅（列幅の合計、紙 mm）。
    #[must_use]
    pub fn width_mm(&self) -> f64 {
        self.col_widths_mm.iter().sum()
    }

    /// 表全体の高さ（行高さの合計、紙 mm）。
    #[must_use]
    pub fn height_mm(&self) -> f64 {
        self.row_heights_mm.iter().sum()
    }
}

/// エンティティの幾何。[`Shape`]（既存プリミティブ）を包含しつつ、テキスト・寸法・表を
/// 追加する。
///
/// [`crate::Entity`] の `geom` フィールドの型であり、[`crate::Command::ModifyEntity`] の
/// `new_geom` の型でもある。幾何変換・境界ボックス・検証の各メソッドは、[`Shape`]
/// バリアントは既存の [`Shape`] へ委譲し、テキスト・寸法・表はそれぞれの規則で処理する。
///
/// **`#[non_exhaustive]`**（DESIGN.md M8 タスク35a）: 幾何の種別は今後も増える
/// （引出線・GPS 記号・ハッチング等）。クレート外の `match` に常にワイルドカード腕を
/// 要求しておくことで、バリアント追加が mcad-app / mcad-io / tcad の一斉コンパイル
/// エラーにならないようにする。**ワイルドカード腕は「未知の幾何を安全に無視する」
/// 実装にすること**（描画・スナップなら何もしない、集計なら数えない）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum EntityGeom {
    /// 既存の幾何プリミティブ（点・線・円・円弧・ポリライン）。
    Shape(Shape),
    /// テキスト。
    Text(TextGeom),
    /// 長さ寸法。
    DimLinear(DimLinear),
    /// 半径寸法。
    DimRadial(DimRadial),
    /// 直径寸法（M9 設計判断3）。
    DimDiameter(DimDiameter),
    /// 汎用テーブル（表・部品表。M10 設計方針1）。
    Table(TableGeom),
    /// 角度寸法（M11 タスク69）。
    DimAngular(DimAngular),
    /// 座標寸法（M11 タスク69）。
    DimOrdinate(DimOrdinate),
}

/// 点 `p` を `pivot` を中心に CCW へ `angle` ラジアン回転する（公開 Vec2/Point2 API で組む）。
#[inline]
fn rotate_point(p: Point2, pivot: Point2, angle: f64) -> Point2 {
    pivot + (p - pivot).rotated(angle)
}

/// 点 `p` を `axis_a`→`axis_b` を通る直線に対して鏡映する。
#[inline]
fn mirror_point(p: Point2, axis_a: Point2, axis_b: Point2) -> Point2 {
    axis_a + (p - axis_a).reflected(axis_b - axis_a)
}

/// 絶対角 `theta` を `axis_a`→`axis_b` を通る直線に対して鏡映した角度（`2·alpha − theta`、
/// `alpha` は軸の方向角）。退化軸（2 点がほぼ同一）では角度が定まらないため `theta` を
/// そのまま返す（[`DimDirection::Rotated`] の鏡映と同じ規則）。
#[inline]
fn mirror_angle(axis_a: Point2, axis_b: Point2, theta: f64) -> f64 {
    let axis = axis_b - axis_a;
    match axis.normalize() {
        Some(_) => 2.0 * axis.angle() - theta,
        None => theta,
    }
}

/// 寸法注記を幾何変換に追従させる（[`DimAnnotation::text_anchor`] へ `f` を適用する）。
///
/// 文字位置の手動上書きは**ワールド座標**なので、寸法本体を動かしたら一緒に動かさないと
/// 文字だけが元の場所へ取り残される。自動配置（`None`）はそのまま `None` を保つ。
/// 記号・公差・桁数・矢配置は座標を持たないので変換の影響を受けない。
///
/// [`DimAnnotation::text_rotation`]（M11 タスク70）はここでは**動かさない**
/// （平行移動では絶対角は変わらない）。回転・鏡映は角度も変わるので、呼び出し側
/// （[`EntityGeom::rotated`] / [`EntityGeom::mirrored`]）が [`transform_annotation_rotation`]
/// で別途上書きする。
#[inline]
fn transform_annotation(
    annotation: &DimAnnotation,
    f: impl FnOnce(Point2) -> Point2,
) -> DimAnnotation {
    DimAnnotation {
        text_anchor: annotation.text_anchor.map(f),
        ..annotation.clone()
    }
}

/// [`transform_annotation`] に加えて [`DimAnnotation::text_rotation`] へ `rot` を適用する
/// （M11 タスク70）。文字位置（`text_anchor`）は `pos` で動かし、絶対角の手動固定
/// （`text_rotation`）は `rot` で動かす — 2 つは独立な変換（回転は加算、鏡映は
/// `2·alpha − theta`）なので、それぞれの関数を受け取る形にしてある。
#[inline]
fn transform_annotation_rotation(
    annotation: &DimAnnotation,
    pos: impl FnOnce(Point2) -> Point2,
    rot: impl FnOnce(f64) -> f64,
) -> DimAnnotation {
    DimAnnotation {
        text_anchor: annotation.text_anchor.map(pos),
        text_rotation: annotation.text_rotation.map(rot),
        ..annotation.clone()
    }
}

impl From<Shape> for EntityGeom {
    /// 既存コード（Shape のみを作る作図ツール・ファイル入出力）との互換のため、
    /// `Shape` から [`EntityGeom::Shape`] への変換を提供する。
    #[inline]
    fn from(shape: Shape) -> Self {
        EntityGeom::Shape(shape)
    }
}

impl EntityGeom {
    /// [`EntityGeom::Shape`] のとき内側の [`Shape`] を借用する。それ以外は `None`。
    ///
    /// テキスト・寸法にまだ対応していない Shape 専用の経路（描画・スナップ・オフセット・
    /// DXF export など）が、機械的に既存ロジックへ委譲するために使う。
    #[inline]
    #[must_use]
    pub fn as_shape(&self) -> Option<&Shape> {
        match self {
            EntityGeom::Shape(shape) => Some(shape),
            _ => None,
        }
    }

    /// 軸並行境界ボックス。
    ///
    /// テキストの境界は文字数×高さの **近似**（CJK≈1.0×height、ASCII≈0.55×height）で、
    /// zoom fit 用途を想定する。正確な選択判定は app 層の責務（DESIGN.md M6 設計判断1）。
    ///
    /// **Text は `height` を 1:1（`k=1`）で解釈する**（`height` の紙 mm 意味論は判断4）。
    /// 尺度を反映した表示上のワールド AABB（`height * k`）は `mcad-app` の
    /// `text_world_aabb` が、この AABB を anchor 基準に `k` 倍する相似拡大として提供する
    /// （DESIGN.md M8 タスク37 実装時追記。tcad が本メソッドへ path 依存しているため、
    /// シグネチャ・実装ともここでは変更しない）。
    #[must_use]
    pub fn aabb(&self) -> Aabb {
        match self {
            EntityGeom::Shape(shape) => shape.aabb(),
            EntityGeom::Text(text) => text_aabb(text),
            EntityGeom::DimLinear(dim) => dim_linear_aabb(dim),
            EntityGeom::DimRadial(dim) => {
                // 計測対象の円／円弧を包む近似ボックス（中心 ± 半径）。引出線・文字は
                // その内外へ僅かに出るが、zoom fit 用途では十分な近似とする。
                let r = Vec2::new(dim.radius.abs(), dim.radius.abs());
                Aabb {
                    min: dim.center - r,
                    max: dim.center + r,
                }
            }
            // 直径寸法も同じ近似（寸法線は中心を通る直径線なので、この箱に収まる）。
            EntityGeom::DimDiameter(dim) => {
                let r = Vec2::new(dim.radius.abs(), dim.radius.abs());
                Aabb {
                    min: dim.center - r,
                    max: dim.center + r,
                }
            }
            // 表は列幅・行高さの合計を **1:1 で解釈**した箱（Text の `height` と同じ
            // 契約。M10 設計方針4）。アンカーが左下なので +x/+y 側へ広がる。尺度を
            // 反映した表示上の AABB は app 層の `table_world_aabb`（タスク58）が
            // この箱を anchor 基準に `k` 倍して作る。
            EntityGeom::Table(table) => Aabb::from_corners(
                table.anchor,
                table.anchor + Vec2::new(table.width_mm(), table.height_mm()),
            ),
            EntityGeom::DimAngular(dim) => dim_angular_aabb(dim),
            // 座標寸法は保存されている 3 点（基準・計測点・引出線端）の箱。値ラベルは
            // 引出線の先へ僅かに出るが、[`EntityGeom::DimRadial`] と同じく zoom fit
            // 用途では十分な近似とする。
            EntityGeom::DimOrdinate(dim) => {
                Aabb::from_corners(dim.origin, dim.feature).extended(dim.leader_end)
            }
        }
    }

    /// 変位 `delta` だけ平行移動した新しい幾何。
    ///
    /// 寸法の文字位置上書き（[`DimAnnotation::text_anchor`]）も一緒に動かす
    /// （回転・鏡映も同様。動かさないと文字だけが元の場所へ取り残される）。
    #[must_use]
    pub fn translated(&self, delta: Vec2) -> EntityGeom {
        match self {
            EntityGeom::Shape(shape) => EntityGeom::Shape(shape.translated(delta)),
            EntityGeom::Text(text) => EntityGeom::Text(TextGeom {
                anchor: text.anchor + delta,
                ..text.clone()
            }),
            EntityGeom::DimLinear(dim) => EntityGeom::DimLinear(DimLinear {
                p1: dim.p1 + delta,
                p2: dim.p2 + delta,
                offset: dim.offset,
                // 平行移動では向きは変わらない（絶対角のまま）。補助線の傾き
                // （M11 タスク78）も同じ理由でそのまま。
                direction: dim.direction,
                ext_angle: dim.ext_angle,
                annotation: transform_annotation(&dim.annotation, |p| p + delta),
            }),
            EntityGeom::DimRadial(dim) => EntityGeom::DimRadial(DimRadial {
                center: dim.center + delta,
                radius: dim.radius,
                leader_angle: dim.leader_angle,
                annotation: transform_annotation(&dim.annotation, |p| p + delta),
            }),
            EntityGeom::DimDiameter(dim) => EntityGeom::DimDiameter(DimDiameter {
                center: dim.center + delta,
                radius: dim.radius,
                angle: dim.angle,
                annotation: transform_annotation(&dim.annotation, |p| p + delta),
            }),
            // 表はアンカー（左下）だけを動かす。セル寸法は紙 mm なので平行移動の
            // 影響を受けない。
            EntityGeom::Table(table) => EntityGeom::Table(TableGeom {
                anchor: table.anchor + delta,
                ..table.clone()
            }),
            // 角度寸法は 3 点を動かすだけ（`arc_radius` は頂点からの距離なので不変）。
            EntityGeom::DimAngular(dim) => EntityGeom::DimAngular(DimAngular {
                vertex: dim.vertex + delta,
                p1: dim.p1 + delta,
                p2: dim.p2 + delta,
                arc_radius: dim.arc_radius,
                annotation: transform_annotation(&dim.annotation, |p| p + delta),
            }),
            // 座標寸法は 3 点を動かす。**基準点も一緒に動く**ので測定値は変わらない
            // （寸法全体を動かしたのだから当然だが、基準だけを据え置く解釈もありうる
            // ため明示する。基準を据え置きたい場合はグリップ編集（タスク75）の仕事）。
            EntityGeom::DimOrdinate(dim) => EntityGeom::DimOrdinate(DimOrdinate {
                origin: dim.origin + delta,
                feature: dim.feature + delta,
                leader_end: dim.leader_end + delta,
                axis: dim.axis,
                annotation: transform_annotation(&dim.annotation, |p| p + delta),
            }),
        }
    }

    /// `pivot` を中心に CCW へ `angle` ラジアン回転した新しい幾何。
    ///
    /// テキストはアンカーを回転し、ベースライン角へ `angle` を加える。
    #[must_use]
    pub fn rotated(&self, pivot: Point2, angle: f64) -> EntityGeom {
        match self {
            EntityGeom::Shape(shape) => EntityGeom::Shape(shape.rotated(pivot, angle)),
            EntityGeom::Text(text) => EntityGeom::Text(TextGeom {
                anchor: rotate_point(text.anchor, pivot, angle),
                angle: text.angle + angle,
                ..text.clone()
            }),
            EntityGeom::DimLinear(dim) => EntityGeom::DimLinear(DimLinear {
                p1: rotate_point(dim.p1, pivot, angle),
                p2: rotate_point(dim.p2, pivot, angle),
                offset: dim.offset,
                // 回転寸法の絶対角は回転量を加算して図形と一緒に回す（加算しないと
                // 図形だけが回って寸法線の向きが取り残される。[`DimDiameter::angle`]・
                // [`DimRadial::leader_angle`] と同じ扱い）。`Aligned` は計測 2 点から
                // 向きを導くので何も足さない。
                direction: match dim.direction {
                    DimDirection::Aligned => DimDirection::Aligned,
                    DimDirection::Rotated(theta) => DimDirection::Rotated(theta + angle),
                },
                // 補助線の傾き（M11 タスク78）も絶対角なので、寸法線の向きと同じく
                // 回転量を加算する（加算しないと図形だけが回って補助線の傾きが
                // 取り残される）。
                ext_angle: dim.ext_angle.map(|theta| theta + angle),
                // 文字姿勢の手動固定（[`DimAnnotation::text_rotation`]、M11 タスク70）も
                // 絶対角なので、寸法線の向きと同じく回転量を加算する。
                annotation: transform_annotation_rotation(
                    &dim.annotation,
                    |p| rotate_point(p, pivot, angle),
                    |theta| theta + angle,
                ),
            }),
            EntityGeom::DimRadial(dim) => EntityGeom::DimRadial(DimRadial {
                center: rotate_point(dim.center, pivot, angle),
                radius: dim.radius,
                leader_angle: dim.leader_angle + angle,
                annotation: transform_annotation_rotation(
                    &dim.annotation,
                    |p| rotate_point(p, pivot, angle),
                    |theta| theta + angle,
                ),
            }),
            // 直径線の向き `angle` は [`DimRadial::leader_angle`] と同じ扱い（回転量を
            // 加算して図形と一緒に回す）。加算しないと図形だけが回って寸法線の向きが
            // 取り残される。
            EntityGeom::DimDiameter(dim) => EntityGeom::DimDiameter(DimDiameter {
                center: rotate_point(dim.center, pivot, angle),
                radius: dim.radius,
                angle: dim.angle + angle,
                annotation: transform_annotation_rotation(
                    &dim.annotation,
                    |p| rotate_point(p, pivot, angle),
                    |theta| theta + angle,
                ),
            }),
            // **表は回さない**（DESIGN.md M10 設計方針3・詳細設計1）。[`TableGeom`] は
            // 回転角フィールド自体を持たないので、回転はアンカーの移動としてのみ
            // 反映し、罫線・文字は軸平行のまま残る。Text の MIRRTEXT=0 相当の割り切りを
            // 回転にも適用したもので、組版・pick・DXF 分解が大幅に単純になる
            // （表の回転は non-goal。回したい需要は M10 詳細設計10 で対象外と明記）。
            EntityGeom::Table(table) => EntityGeom::Table(TableGeom {
                anchor: rotate_point(table.anchor, pivot, angle),
                ..table.clone()
            }),
            // 角度寸法は 3 点を回すだけ。2 辺の向きが揃って回るので**なす角は不変**、
            // `arc_radius` も頂点からの距離なので不変（角度を別フィールドで持たない
            // 設計の利点。[`DimDiameter::angle`] のような加算は要らない）。
            EntityGeom::DimAngular(dim) => EntityGeom::DimAngular(DimAngular {
                vertex: rotate_point(dim.vertex, pivot, angle),
                p1: rotate_point(dim.p1, pivot, angle),
                p2: rotate_point(dim.p2, pivot, angle),
                arc_radius: dim.arc_radius,
                annotation: transform_annotation_rotation(
                    &dim.annotation,
                    |p| rotate_point(p, pivot, angle),
                    |theta| theta + angle,
                ),
            }),
            // 座標寸法は 3 点を回す。**`axis` は回さない**（図面の X/Y は用紙の軸で
            // あり図形に付随しないため。理由は [`OrdinateAxis`] の doc）。結果として
            // 測定値は回転後の座標成分へ変わる。
            EntityGeom::DimOrdinate(dim) => EntityGeom::DimOrdinate(DimOrdinate {
                origin: rotate_point(dim.origin, pivot, angle),
                feature: rotate_point(dim.feature, pivot, angle),
                leader_end: rotate_point(dim.leader_end, pivot, angle),
                axis: dim.axis,
                annotation: transform_annotation_rotation(
                    &dim.annotation,
                    |p| rotate_point(p, pivot, angle),
                    |theta| theta + angle,
                ),
            }),
        }
    }

    /// `axis_a`→`axis_b` を通る直線に対して鏡映した新しい幾何。
    ///
    /// **テキストはアンカーのみ鏡映し、文字グリフは反転しない**（AutoCAD の MIRRTEXT=0
    /// 相当。DESIGN.md M6 設計判断1）。ベースライン角は方向ベクトルを軸に対して鏡映した
    /// 角度（`2·alpha − angle`、`alpha` は軸の方向角）にする。退化軸（2 点がほぼ同一）では
    /// 角度が定まらないためアンカーのみ鏡映し角度は保つ。
    #[must_use]
    pub fn mirrored(&self, axis_a: Point2, axis_b: Point2) -> EntityGeom {
        match self {
            EntityGeom::Shape(shape) => EntityGeom::Shape(shape.mirrored(axis_a, axis_b)),
            EntityGeom::Text(text) => {
                let axis = axis_b - axis_a;
                let angle = match axis.normalize() {
                    // 軸方向角 alpha に対し、方向ベクトルの反射角は 2·alpha − angle。
                    Some(_) => 2.0 * axis.angle() - text.angle,
                    // 退化軸では角度が定まらないため元の角度を保つ。
                    None => text.angle,
                };
                EntityGeom::Text(TextGeom {
                    anchor: mirror_point(text.anchor, axis_a, axis_b),
                    angle,
                    ..text.clone()
                })
            }
            EntityGeom::DimLinear(dim) => EntityGeom::DimLinear(DimLinear {
                p1: mirror_point(dim.p1, axis_a, axis_b),
                p2: mirror_point(dim.p2, axis_a, axis_b),
                // 符号付きオフセットは鏡映で向きが反転する。鏡映 M は向きを裏返す
                // ので `perp(M(dir)) = −M(perp(dir))` となり、`Aligned` / `Rotated` の
                // どちらでも同じ「符号反転」で寸法線が元の位置へ写る。
                offset: -dim.offset,
                // 回転寸法の絶対角は、方向ベクトルを軸に対して鏡映した角度
                // （`2·alpha − theta`）。退化軸では角度が定まらないため元の角度を保つ
                // （Text のベースライン角・[`DimRadial::leader_angle`] と同じ規則）。
                direction: match dim.direction {
                    DimDirection::Aligned => DimDirection::Aligned,
                    DimDirection::Rotated(theta) => {
                        let axis = axis_b - axis_a;
                        DimDirection::Rotated(match axis.normalize() {
                            Some(_) => 2.0 * axis.angle() - theta,
                            None => theta,
                        })
                    }
                },
                // 補助線の傾き（M11 タスク78）も絶対角なので、寸法線の向きと同じ
                // 鏡映式（`2·alpha − theta`）で写す。
                ext_angle: dim
                    .ext_angle
                    .map(|theta| mirror_angle(axis_a, axis_b, theta)),
                // 文字姿勢の手動固定（M11 タスク70）も絶対角なので、寸法線の向きと
                // 同じ鏡映式（`2·alpha − theta`）で写す。
                annotation: transform_annotation_rotation(
                    &dim.annotation,
                    |p| mirror_point(p, axis_a, axis_b),
                    |theta| mirror_angle(axis_a, axis_b, theta),
                ),
            }),
            EntityGeom::DimRadial(dim) => {
                let axis = axis_b - axis_a;
                let leader_angle = match axis.normalize() {
                    Some(_) => 2.0 * axis.angle() - dim.leader_angle,
                    None => dim.leader_angle,
                };
                EntityGeom::DimRadial(DimRadial {
                    center: mirror_point(dim.center, axis_a, axis_b),
                    radius: dim.radius,
                    leader_angle,
                    annotation: transform_annotation_rotation(
                        &dim.annotation,
                        |p| mirror_point(p, axis_a, axis_b),
                        |theta| mirror_angle(axis_a, axis_b, theta),
                    ),
                })
            }
            // 直径線の向きは半径寸法の引出方向と同じ規則で鏡映する（退化軸では角度が
            // 定まらないため元の角度を保つ）。
            EntityGeom::DimDiameter(dim) => {
                let axis = axis_b - axis_a;
                let angle = match axis.normalize() {
                    Some(_) => 2.0 * axis.angle() - dim.angle,
                    None => dim.angle,
                };
                EntityGeom::DimDiameter(DimDiameter {
                    center: mirror_point(dim.center, axis_a, axis_b),
                    radius: dim.radius,
                    angle,
                    annotation: transform_annotation_rotation(
                        &dim.annotation,
                        |p| mirror_point(p, axis_a, axis_b),
                        |theta| mirror_angle(axis_a, axis_b, theta),
                    ),
                })
            }
            // **表はアンカーのみ鏡映する**（DESIGN.md M10 設計方針3・詳細設計1）。
            // 列の並び順もセル文字も鏡像化しない（Text の MIRRTEXT=0 と同じ割り切り。
            // 鏡像化した表は読めないので、内容を保つほうが実用的）。
            EntityGeom::Table(table) => EntityGeom::Table(TableGeom {
                anchor: mirror_point(table.anchor, axis_a, axis_b),
                ..table.clone()
            }),
            // 角度寸法は 3 点を鏡映するだけ。鏡映は向きを裏返すが、なす角は
            // **短い側**（0〜π）と決めてあるので値も描かれる弧も鏡像として正しく写る
            // （`p1`/`p2` の入れ替えに対して不変なため、辺の順序が裏返っても問題ない）。
            // `arc_radius` は距離なので不変。
            EntityGeom::DimAngular(dim) => EntityGeom::DimAngular(DimAngular {
                vertex: mirror_point(dim.vertex, axis_a, axis_b),
                p1: mirror_point(dim.p1, axis_a, axis_b),
                p2: mirror_point(dim.p2, axis_a, axis_b),
                arc_radius: dim.arc_radius,
                annotation: transform_annotation_rotation(
                    &dim.annotation,
                    |p| mirror_point(p, axis_a, axis_b),
                    |theta| mirror_angle(axis_a, axis_b, theta),
                ),
            }),
            // 座標寸法は 3 点を鏡映する。**`axis` は裏返さない**（回転と同じ理由。
            // 理由は [`OrdinateAxis`] の doc）。y 軸鏡映した X 座標寸法は、鏡映後の
            // 位置の X 座標をそのまま読む（符号が反転した値になる）。
            EntityGeom::DimOrdinate(dim) => EntityGeom::DimOrdinate(DimOrdinate {
                origin: mirror_point(dim.origin, axis_a, axis_b),
                feature: mirror_point(dim.feature, axis_a, axis_b),
                leader_end: mirror_point(dim.leader_end, axis_a, axis_b),
                axis: dim.axis,
                annotation: transform_annotation_rotation(
                    &dim.annotation,
                    |p| mirror_point(p, axis_a, axis_b),
                    |theta| mirror_angle(axis_a, axis_b, theta),
                ),
            }),
        }
    }

    /// 幾何が妥当か検証する。
    ///
    /// [`Shape`] は [`Shape::validate`] へ委譲する。テキストは空文字列・非正の高さ・
    /// 非有限値を、寸法は非有限値（半径・直径寸法は加えて非正半径）を拒否する
    /// （[`Shape::validate`] と同じ境界基準。DESIGN.md M6 設計判断1）。
    ///
    /// **表は行列数・寸法（個々の値と合計）・セル数・文字高さ・セル文字列を検証する**
    /// （DESIGN.md M10 詳細設計1。条件の一覧は本モジュールの `validate_table`）。
    ///
    /// **寸法は加えて [`DimAnnotation::validate`] を通す**（M9 設計判断2）。これが
    /// 「種別に許されない記号・`upper < lower`・不正なはめあい記号・非有限値を
    /// **コマンド境界で**拒否する」実体で、[`crate::Document::apply`] の
    /// `AddEntity` / `ModifyEntity` と `.mcad` 読込の双方がこのメソッドを通る。
    /// 注記側のエラーは [`crate::CoreError::InvalidDimAnnotation`] だが、幾何検証の契約
    /// （`Shape::validate` と揃えた `String`）に合わせてここでは文字列化する
    /// （`Document::apply` は [`crate::CoreError::InvalidGeometry`] として返す）。
    ///
    /// # Errors
    ///
    /// 不正な理由を人が読める `String` で返す。
    pub fn validate(&self) -> Result<(), String> {
        let finite_pt = |p: Point2| p.x.is_finite() && p.y.is_finite();
        match self {
            EntityGeom::Shape(shape) => shape.validate(),
            EntityGeom::Text(text) => {
                if text.content.is_empty() {
                    return Err("empty text content".into());
                }
                if !finite_pt(text.anchor) {
                    return Err("non-finite text anchor".into());
                }
                if !text.height.is_finite() || text.height <= 0.0 {
                    return Err(format!("invalid text height: {}", text.height));
                }
                if !text.angle.is_finite() {
                    return Err("non-finite text angle".into());
                }
                Ok(())
            }
            EntityGeom::DimLinear(dim) => {
                if !finite_pt(dim.p1) || !finite_pt(dim.p2) {
                    return Err("non-finite linear dimension points".into());
                }
                if !dim.offset.is_finite() {
                    return Err("non-finite linear dimension offset".into());
                }
                // 回転寸法の絶対角（M11 タスク68）。非有限だと寸法線の向きが決まらず、
                // 展開・pick・AABB のすべてが NaN を撒く。
                if let DimDirection::Rotated(theta) = dim.direction
                    && !theta.is_finite()
                {
                    return Err("non-finite linear dimension direction angle".into());
                }
                // 補助線の傾きの絶対角（M11 タスク78）。**拒否するのは非有限だけ**で、
                // 範囲（2π 超・負）は正規化も拒否もしない（[`DimLinear::ext_angle`] の
                // doc）。非有限だと交点が NaN になり、展開・pick・AABB が壊れる。
                if let Some(theta) = dim.ext_angle
                    && !theta.is_finite()
                {
                    return Err("non-finite linear dimension extension line angle".into());
                }
                check_annotation(&dim.annotation, DimKind::Linear)
            }
            EntityGeom::DimRadial(dim) => {
                if !finite_pt(dim.center) {
                    return Err("non-finite radial dimension center".into());
                }
                if !dim.radius.is_finite() || dim.radius <= 0.0 {
                    return Err(format!("invalid radial dimension radius: {}", dim.radius));
                }
                if !dim.leader_angle.is_finite() {
                    return Err("non-finite radial dimension leader angle".into());
                }
                check_annotation(&dim.annotation, DimKind::Radial)
            }
            EntityGeom::DimDiameter(dim) => {
                if !finite_pt(dim.center) {
                    return Err("non-finite diameter dimension center".into());
                }
                if !dim.radius.is_finite() || dim.radius <= 0.0 {
                    return Err(format!("invalid diameter dimension radius: {}", dim.radius));
                }
                if !dim.angle.is_finite() {
                    return Err("non-finite diameter dimension angle".into());
                }
                check_annotation(&dim.annotation, DimKind::Diameter)
            }
            EntityGeom::Table(table) => validate_table(table),
            EntityGeom::DimAngular(dim) => {
                if !finite_pt(dim.vertex) || !finite_pt(dim.p1) || !finite_pt(dim.p2) {
                    return Err("non-finite angular dimension points".into());
                }
                if !dim.arc_radius.is_finite() || dim.arc_radius <= 0.0 {
                    return Err(format!(
                        "invalid angular dimension arc radius: {}",
                        dim.arc_radius
                    ));
                }
                // 2 辺の向きが決まらない（頂点と一致する）角度寸法は、値も弧も
                // 補助線も定まらない。半径寸法の「半径 0」と同じ境界で拒否する。
                let Some((u1, u2)) = dim.directions() else {
                    return Err("angular dimension legs must not coincide with the vertex".into());
                };
                // p1 方向と p2 方向が反平行（なす角 = π）だと、弧をどちら側の半円へ
                // 描くかが保存データ（vertex/p1/p2/arc_radius）だけから決まらない
                // （`p1`/`p2` を入れ替えると弧が反対側へ飛ぶ。[`DimAngular`] の doc
                // 「反平行（180 度ちょうど）は拒否する」参照）。
                //
                // 閾値の理由は [`DIM_ANGULAR_ANTIPARALLEL_EPS`] の doc を参照
                // （メッセージの二重管理をしない）。
                let angle = u1.dot(u2).clamp(-1.0, 1.0).acos();
                if (std::f64::consts::PI - angle).abs() < DIM_ANGULAR_ANTIPARALLEL_EPS {
                    return Err(
                        "angular dimension legs must not be antiparallel (arc side is ambiguous at 180°)"
                            .into(),
                    );
                }
                check_annotation(&dim.annotation, DimKind::Angular)
            }
            EntityGeom::DimOrdinate(dim) => {
                if !finite_pt(dim.origin) || !finite_pt(dim.feature) || !finite_pt(dim.leader_end) {
                    return Err("non-finite ordinate dimension points".into());
                }
                // 引出線長 0（`feature == leader_end`）は**拒否しない**。値は
                // `feature − origin` だけで決まるので情報は落ちず、展開も引出線を
                // 引かずにラベルだけを置く（`expand_ordinate`）。3 点が一致していても
                // 「原点そのものを指す座標寸法（値 0）」として意味が通る。
                check_annotation(&dim.annotation, DimKind::Ordinate)
            }
        }
    }
}

/// 寸法注記を検証し、[`EntityGeom::validate`] の契約（人が読める `String`）へ合わせる。
///
/// [`crate::CoreError`] の `Display` をそのまま使うので、理由の文言は注記側の検証と
/// 一致する（メッセージの二重管理をしない）。
fn check_annotation(annotation: &DimAnnotation, kind: DimKind) -> Result<(), String> {
    annotation.validate(kind).map_err(|e| e.to_string())
}

/// 表を検証する（[`EntityGeom::validate`] の [`EntityGeom::Table`] 腕の実体）。
///
/// 検証するのは次の 7 つ。
///
/// 1. アンカーが有限であること
/// 2. 行数・列数が 1 以上 [`MAX_TABLE_ROWS`] / [`MAX_TABLE_COLS`] 以下であること
/// 3. 個々の列幅・行高さ・文字高さと、幅・高さの**合計**が「有限・正・[`MAX_TABLE_MM`]
///    以下」であること
/// 4. [`TableGeom::cells`] の要素数がちょうど 行 × 列 であること
/// 5. 文字高さが**最小の行高さ未満**であること（文字が行からはみ出さない）
/// 6. セル文字列に制御文字が混じっていないこと
/// 7. **アンカー + 幅/高さ（表の右上隅）が有限であること**。1・3 はそれぞれ独立の
///    上限だが、実際に組版・描画で使う右上隅そのものを直接検証する防御的チェック
///    （`.mcad` v6・タスク57、Codex adversarial review 2026-09-06 medium 指摘。
///    現行の [`MAX_TABLE_MM`] の下では実際には到達しないことを実測済み —
///    `validate_table` 内のコメント参照）。
///
/// 6 で **空文字列は拒否しない**（空欄のある表が普通。[`TextGeom`] が空を拒否するのとは
/// 意図的に違える。M10 詳細設計1）。制御文字を拒むのは [`crate::DimAnnotation`] の
/// 値上書きと同じ理由で、改行・タブが 1 行 1 セルの組版と SVG/PDF/DXF の文字列を壊すため。
///
/// 2 と 3 の上限は [`crate::TitleBlockTemplate::validate`] と同じ論点（毎フレーム走査の
/// 持続コストと、合計値のあふれ）に対する対処である。
fn validate_table(table: &TableGeom) -> Result<(), String> {
    if !(table.anchor.x.is_finite() && table.anchor.y.is_finite()) {
        return Err("non-finite table anchor".to_owned());
    }
    let (rows, cols) = (table.rows(), table.cols());
    if rows == 0 {
        return Err("table has no rows".to_owned());
    }
    if cols == 0 {
        return Err("table has no columns".to_owned());
    }
    if rows > MAX_TABLE_ROWS {
        return Err(format!(
            "table has too many rows: {rows} (max {MAX_TABLE_ROWS})"
        ));
    }
    if cols > MAX_TABLE_COLS {
        return Err(format!(
            "table has too many columns: {cols} (max {MAX_TABLE_COLS})"
        ));
    }
    for (c, width) in table.col_widths_mm.iter().enumerate() {
        check_table_mm(*width, format_args!("width of column {c}"))?;
    }
    for (r, height) in table.row_heights_mm.iter().enumerate() {
        check_table_mm(*height, format_args!("height of row {r}"))?;
    }
    check_table_mm(table.text_height_mm, format_args!("table text height"))?;
    // 個々の値が上限内でも合計は超えうる（表題欄と同じ論点）。ここまで通っていれば
    // 合計は有限（非有限は個別の検証で弾かれている）。
    check_table_mm(table.width_mm(), format_args!("total table width"))?;
    check_table_mm(table.height_mm(), format_args!("total table height"))?;

    // アンカー自体には上限がない（ワールド座標は任意の有限値を許す）。上の 2 つの
    // check_table_mm は個々の値と合計を [`MAX_TABLE_MM`] 以下に抑えるが、それは
    // 「アンカーからの伸び幅」であって、実際に組版・描画で使う右上隅
    // （`anchor + 幅/高さ`）そのものは検証していない。Codex adversarial review
    // 2026-09-06 medium 指摘（`anchor.x = f64::MAX` で右上隅が非有限になりうる）を
    // 受けての防御的チェック。**実測**: 現行の [`MAX_TABLE_MM`]（1e4）の下では
    // `f64::MAX + MAX_TABLE_MM` は丸めで `f64::MAX` に留まり非有限にはならない
    // （IEEE 754 で `f64::MAX` 近傍の ULP は 1e4 よりはるかに大きいため）。つまり
    // 現行定数の組み合わせではこの分岐は到達しない。それでも `anchor` 自体に上限が
    // ない設計（アンカーはワールド座標で、他の紙 mm 値と違う次元）である以上、
    // 「右上隅を直接検証する」ほうが「個々の値の上限から間接的に導く」より
    // 将来 [`MAX_TABLE_MM`] を引き上げたときにも壊れない。テストは受理側
    // （到達しないことの確認）のみを固定する。
    let far_corner_x = table.anchor.x + table.width_mm();
    let far_corner_y = table.anchor.y + table.height_mm();
    if !far_corner_x.is_finite() {
        return Err(format!(
            "table far corner x is not finite: anchor.x={} + width={}",
            table.anchor.x,
            table.width_mm()
        ));
    }
    if !far_corner_y.is_finite() {
        return Err(format!(
            "table far corner y is not finite: anchor.y={} + height={}",
            table.anchor.y,
            table.height_mm()
        ));
    }

    // rows <= MAX_TABLE_ROWS かつ cols <= MAX_TABLE_COLS まで絞れているので、
    // ここでの積は usize であふれない。
    let expected = rows * cols;
    if table.cells.len() != expected {
        return Err(format!(
            "table cell count mismatch: {} cells for {rows}x{cols} (expected {expected})",
            table.cells.len()
        ));
    }

    // 行高さはすべて有限・正なので最小値も有限・正。等号は拒否する（文字が行を
    // ちょうど埋めると罫線と重なる）。
    let min_row_height = table
        .row_heights_mm
        .iter()
        .copied()
        .fold(f64::INFINITY, f64::min);
    if table.text_height_mm >= min_row_height {
        return Err(format!(
            "table text height {} must be smaller than the smallest row height {min_row_height}",
            table.text_height_mm
        ));
    }

    for (i, cell) in table.cells.iter().enumerate() {
        if cell.chars().any(char::is_control) {
            let (r, c) = (i / cols, i % cols);
            return Err(format!(
                "table cell {r}/{c} must not contain control characters: {cell:?}"
            ));
        }
    }
    Ok(())
}

/// 表の紙寸法 1 つ（個々の値でも合計でもよい）が有限・正・[`MAX_TABLE_MM`] 以下かを
/// 検証する。`what` は失敗時のメッセージに使う説明で、成功時は文字列化しない
/// （`format_args!` を渡すため検証が通る限り確保が起きない）。
fn check_table_mm(value: f64, what: std::fmt::Arguments<'_>) -> Result<(), String> {
    if !(value.is_finite() && value > 0.0) {
        return Err(format!("invalid {what}: {value}"));
    }
    if value > MAX_TABLE_MM {
        return Err(format!(
            "{what} exceeds the {MAX_TABLE_MM} mm limit: {value}"
        ));
    }
    Ok(())
}

/// 文字列の近似幅（文字数×高さ。CJK ≈ 1.0×height、ASCII ≈ 0.55×height。DESIGN.md M6
/// 設計判断1）。[`EntityGeom::aabb`] の Text と、app 層の表（セル文字が列からはみ出す
/// ぶんを表示 AABB へ足す `table_world_aabb`）が**同じ推定式**を使うための唯一の出所。
#[must_use]
pub fn approx_text_width(content: &str, height: f64) -> f64 {
    content
        .chars()
        .map(|c| if c.is_ascii() { 0.55 } else { 1.0 } * height)
        .sum()
}

/// テキストの近似 AABB。文字数×高さの近似幅（CJK≈1.0×height、ASCII≈0.55×height）で
/// 局所ボックスを組み、ベースライン角で回転した 4 隅を包む（DESIGN.md M6 設計判断1）。
fn text_aabb(text: &TextGeom) -> Aabb {
    let width = approx_text_width(&text.content, text.height);
    // 局所座標（アンカー原点、ベースライン +x、上方向 +y）の 4 隅を回転して包む。
    let corners = [
        Vec2::new(0.0, 0.0),
        Vec2::new(width, 0.0),
        Vec2::new(width, text.height),
        Vec2::new(0.0, text.height),
    ];
    let pts = corners
        .into_iter()
        .map(|c| text.anchor + c.rotated(text.angle));
    Aabb::from_points(pts).unwrap_or_else(|| Aabb::from_point(text.anchor))
}

/// 長さ寸法の AABB。計測 2 点と寸法線の 2 端点を包む。
///
/// 寸法線の端点は展開（`crate::expand`）と**同じ純関数**
/// [`crate::expand::linear_frame`] から取る。回転寸法では端点が計測 2 点の箱の外へ
/// 出るため、ここで独自に `p1 + perp * offset` を組むと zoom fit と描画がずれる
/// （M11 設計判断1 の「展開の実装を 1 つにする」をこの経路にも適用した）。
///
/// 投影長 0（方向は決まるが寸法線長が 0）の回転寸法では、展開（[`expand_linear`]
/// ※`crate::expand`）が寸法線・補助線の代わりに描く点
/// （`p1 + dir.perp() * offset`。[`crate::expand::linear_degenerate_dir_and_point`]）
/// まで箱を広げる。ここを計測 2 点だけの箱のままにすると、`offset` が大きいときに
/// 可視ジオメトリが AABB の外へ出て、ズームフィット・カリング・矩形選択が
/// 取りこぼす（M11 タスク68 Codex レビュー指摘2）。
///
/// 向きそのものも決まらない退化寸法（`Aligned` で計測 2 点がほぼ同一 / `Rotated` で
/// θ が非有限）では、計測 2 点のみの箱を返す（展開もこのとき計測 2 点をそのまま使う）。
///
/// [`expand_linear`]: crate::expand::expand_linear
fn dim_linear_aabb(dim: &DimLinear) -> Aabb {
    let base = Aabb::from_corners(dim.p1, dim.p2);
    match crate::expand::linear_frame(dim) {
        Some(frame) => base.extended(frame.d1).extended(frame.d2),
        None => match crate::expand::linear_degenerate_dir_and_point(dim) {
            Some((_, p)) => base.extended(p),
            None => base,
        },
    }
}

/// 角度寸法の AABB。**弧と、頂点から弧まで引かれる補助線 2 本**を包む。
///
/// 弧の箱は [`mcad_geom::Arc::aabb`]（象限の通過を見て端点だけでなく極値も含む実装）
/// へ委譲し、そこへ頂点を足す。補助線は頂点と弧の端点を結ぶので、この 2 つの和で
/// 覆われる。
///
/// **`p1` / `p2` は含めない。** 保存はされているが向きを決めるだけの点で、描かれない
/// （距離は寸法に使わない。[`DimAngular`] の doc）。含めると、遠くの点で角を指定した
/// だけでズームフィットが大きく外れる。
///
/// 補助線の突き出し（[`DimStyle::ext_overshoot_mm`](crate::DimStyle::ext_overshoot_mm)）と
/// 値ラベルはこの箱の外へ僅かに出るが、[`EntityGeom::DimRadial`] と同じく zoom fit
/// 用途では十分な近似とする（スタイル・表示状態に依存する量を AABB へ持ち込まない）。
///
/// 骨格（弧の中心・半径・始終角）は展開・pick と**同じ純関数**
/// [`crate::expand::angular_frame`] から取る。退化（向きが決まらない／`arc_radius` が
/// 不正）では頂点だけの箱を返す（展開も弧を描かない）。
fn dim_angular_aabb(dim: &DimAngular) -> Aabb {
    let base = Aabb::from_point(dim.vertex);
    match crate::expand::angular_frame(dim) {
        Some(frame) => base.union(&frame.arc().aabb()),
        None => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dim::{ArrowPlacement, FitClass, SizeTolerance, ValueStyle};
    use mcad_geom::{DimSymbol, LineSeg, Shape};
    use std::f64::consts::{FRAC_PI_2, PI};

    const T: f64 = 1e-9;

    fn text(anchor: Point2, angle: f64) -> TextGeom {
        TextGeom {
            anchor,
            content: "Ab".into(),
            height: 2.0,
            angle,
        }
    }

    fn approx(a: Point2, b: Point2) -> bool {
        a.distance(b) < 1e-6
    }

    #[test]
    fn from_shape_and_as_shape_roundtrip() {
        let shape = Shape::Point(Point2::new(1.0, 2.0));
        let geom: EntityGeom = shape.clone().into();
        assert_eq!(geom, EntityGeom::Shape(shape.clone()));
        assert_eq!(geom.as_shape(), Some(&shape));
        assert_eq!(EntityGeom::Text(text(Point2::ORIGIN, 0.0)).as_shape(), None);
    }

    #[test]
    fn shape_variant_delegates_to_shape() {
        // Shape バリアントは既存 Shape の同名メソッドへ委譲する。
        let shape = Shape::Line(LineSeg::new(Point2::new(0.0, 0.0), Point2::new(4.0, 0.0)));
        let geom = EntityGeom::Shape(shape.clone());
        let d = Vec2::new(1.0, 2.0);
        assert_eq!(geom.translated(d), EntityGeom::Shape(shape.translated(d)));
        assert_eq!(geom.aabb().min, shape.aabb().min);
    }

    #[test]
    fn text_translate_keeps_angle_moves_anchor() {
        let g = EntityGeom::Text(text(Point2::new(1.0, 1.0), 0.3));
        let EntityGeom::Text(t) = g.translated(Vec2::new(2.0, -3.0)) else {
            panic!("expected text");
        };
        assert!(approx(t.anchor, Point2::new(3.0, -2.0)));
        assert!((t.angle - 0.3).abs() < T);
    }

    #[test]
    fn text_rotate_moves_anchor_and_adds_angle() {
        // anchor (1,0)、角 0 を原点まわり +π/2 → anchor (0,1)、角 π/2。
        let g = EntityGeom::Text(text(Point2::new(1.0, 0.0), 0.0));
        let EntityGeom::Text(t) = g.rotated(Point2::ORIGIN, FRAC_PI_2) else {
            panic!("expected text");
        };
        assert!(approx(t.anchor, Point2::new(0.0, 1.0)));
        assert!((t.angle - FRAC_PI_2).abs() < T);
    }

    #[test]
    fn text_mirror_across_x_axis_keeps_horizontal_baseline() {
        // x 軸鏡映: alpha=0 → new_angle = -angle。角 0 の水平ベースラインは角 0 のまま、
        // anchor の y だけ反転する（グリフは反転しない = 角度が水平を保つ）。
        let g = EntityGeom::Text(text(Point2::new(3.0, 2.0), 0.0));
        let EntityGeom::Text(t) = g.mirrored(Point2::ORIGIN, Point2::new(1.0, 0.0)) else {
            panic!("expected text");
        };
        assert!(approx(t.anchor, Point2::new(3.0, -2.0)));
        assert!((t.angle - 0.0).abs() < T);
    }

    #[test]
    fn text_mirror_across_y_axis_flips_baseline_direction() {
        // y 軸鏡映: alpha=π/2 → new_angle = π − angle。右向き(0)ベースラインは左向き(π)へ。
        let g = EntityGeom::Text(text(Point2::new(1.0, 5.0), 0.0));
        let EntityGeom::Text(t) = g.mirrored(Point2::ORIGIN, Point2::new(0.0, 1.0)) else {
            panic!("expected text");
        };
        assert!(approx(t.anchor, Point2::new(-1.0, 5.0)));
        assert!((t.angle - PI).abs() < T);
    }

    #[test]
    fn text_validate_rejects_empty_and_bad_height() {
        let ok = EntityGeom::Text(text(Point2::ORIGIN, 0.0));
        assert!(ok.validate().is_ok());

        let empty = EntityGeom::Text(TextGeom {
            content: String::new(),
            ..text(Point2::ORIGIN, 0.0)
        });
        assert!(empty.validate().is_err());

        let zero_h = EntityGeom::Text(TextGeom {
            height: 0.0,
            ..text(Point2::ORIGIN, 0.0)
        });
        assert!(zero_h.validate().is_err());

        let nan = EntityGeom::Text(text(Point2::new(f64::NAN, 0.0), 0.0));
        assert!(nan.validate().is_err());
    }

    #[test]
    fn text_aabb_is_finite_and_contains_anchor_row() {
        let g = EntityGeom::Text(text(Point2::new(0.0, 0.0), 0.0));
        let bb = g.aabb();
        assert!(bb.min.x.is_finite() && bb.max.x.is_finite());
        // 高さ 2、2 文字（ASCII 0.55×2 ×2 = 2.2 幅）。
        assert!((bb.min.y - 0.0).abs() < T);
        assert!((bb.max.y - 2.0).abs() < T);
        assert!(bb.max.x > 2.0 && bb.max.x < 2.5);
    }

    #[test]
    fn dim_linear_transforms_and_offset_sign() {
        let dim = DimLinear {
            p1: Point2::new(0.0, 0.0),
            p2: Point2::new(4.0, 0.0),
            offset: 1.5,
            direction: DimDirection::Aligned,
            ext_angle: None,
            annotation: DimAnnotation::default(),
        };
        let g = EntityGeom::DimLinear(dim);

        // 平行移動: 2 点が動き offset は不変。
        let EntityGeom::DimLinear(t) = g.translated(Vec2::new(1.0, 1.0)) else {
            panic!();
        };
        assert!(approx(t.p1, Point2::new(1.0, 1.0)));
        assert!(approx(t.p2, Point2::new(5.0, 1.0)));
        assert!((t.offset - 1.5).abs() < T);

        // x 軸鏡映: 符号付き offset が反転する。
        let EntityGeom::DimLinear(m) = g.mirrored(Point2::ORIGIN, Point2::new(1.0, 0.0)) else {
            panic!();
        };
        assert!((m.offset + 1.5).abs() < T);
    }

    #[test]
    fn dim_linear_validate_rejects_non_finite() {
        let ok = EntityGeom::DimLinear(DimLinear {
            p1: Point2::ORIGIN,
            p2: Point2::new(1.0, 0.0),
            offset: 0.0,
            direction: DimDirection::Aligned,
            ext_angle: None,
            annotation: DimAnnotation::default(),
        });
        assert!(ok.validate().is_ok());
        let bad = EntityGeom::DimLinear(DimLinear {
            p1: Point2::ORIGIN,
            p2: Point2::new(1.0, 0.0),
            offset: f64::INFINITY,
            direction: DimDirection::Aligned,
            ext_angle: None,
            annotation: DimAnnotation::default(),
        });
        assert!(bad.validate().is_err());
    }

    // -----------------------------------------------------------------
    // M11 タスク68: 回転寸法（`DimDirection`）
    // -----------------------------------------------------------------

    /// 斜辺 (0,0)-(120,30) に付けた回転寸法（既定は水平 θ = 0、offset 0）。
    fn rotated_linear(theta: f64) -> DimLinear {
        DimLinear {
            p1: Point2::new(0.0, 0.0),
            p2: Point2::new(120.0, 30.0),
            offset: 0.0,
            direction: DimDirection::Rotated(theta),
            ext_angle: None,
            annotation: DimAnnotation::default(),
        }
    }

    #[test]
    fn dim_direction_defaults_to_aligned_and_resolves_the_unit_vector() {
        assert_eq!(DimDirection::default(), DimDirection::Aligned);
        let p1 = Point2::new(2.0, 2.0);
        let p2 = Point2::new(2.0, 8.0);
        // Aligned は計測 2 点の向き。
        let dir = DimDirection::Aligned.unit_vector(p1, p2).unwrap();
        assert!(approx(Point2::ORIGIN + dir, Point2::new(0.0, 1.0)));
        // Aligned の退化は None。
        assert!(DimDirection::Aligned.unit_vector(p1, p1).is_none());
        // Rotated は計測 2 点に依らず絶対角で決まる。
        let dir = DimDirection::Rotated(PI).unit_vector(p1, p2).unwrap();
        assert!(approx(Point2::ORIGIN + dir, Point2::new(-1.0, 0.0)));
        // 非有限な角は None（NaN を撒かない）。
        assert!(
            DimDirection::Rotated(f64::NAN)
                .unit_vector(p1, p2)
                .is_none()
        );
    }

    #[test]
    fn dim_linear_measured_value_projects_for_rotated_directions() {
        // 整列は実距離、水平回転は x 成分、鉛直回転は y 成分。
        let aligned = DimLinear {
            direction: DimDirection::Aligned,
            ..rotated_linear(0.0)
        };
        assert!((aligned.measured_value() - 123.693_168_768_529_82).abs() < T);
        assert!((rotated_linear(0.0).measured_value() - 120.0).abs() < T);
        assert!((rotated_linear(FRAC_PI_2).measured_value() - 30.0).abs() < T);
        // 向きが逆でも長さは正。
        assert!((rotated_linear(PI).measured_value() - 120.0).abs() < T);
        assert_eq!(rotated_linear(f64::INFINITY).measured_value(), 0.0);
    }

    #[test]
    fn dim_linear_rotate_adds_to_the_rotated_angle_and_leaves_aligned_alone() {
        let g = EntityGeom::DimLinear(rotated_linear(0.0));
        let EntityGeom::DimLinear(r) = g.rotated(Point2::ORIGIN, FRAC_PI_2) else {
            panic!();
        };
        assert_eq!(r.direction, DimDirection::Rotated(FRAC_PI_2));
        // 図形と一緒に回るので、投影長は回転前と変わらない。
        assert!((r.measured_value() - 120.0).abs() < T);

        let aligned = EntityGeom::DimLinear(DimLinear {
            direction: DimDirection::Aligned,
            ..rotated_linear(0.0)
        });
        let EntityGeom::DimLinear(r) = aligned.rotated(Point2::ORIGIN, FRAC_PI_2) else {
            panic!();
        };
        assert_eq!(r.direction, DimDirection::Aligned);
    }

    #[test]
    fn dim_linear_mirror_reflects_the_rotated_angle_and_flips_the_offset() {
        // x 軸鏡映（alpha = 0）: θ → −θ、offset は符号反転。
        let g = EntityGeom::DimLinear(DimLinear {
            offset: 7.0,
            ..rotated_linear(FRAC_PI_2)
        });
        let EntityGeom::DimLinear(m) = g.mirrored(Point2::ORIGIN, Point2::new(1.0, 0.0)) else {
            panic!();
        };
        assert!((m.offset + 7.0).abs() < T);
        let DimDirection::Rotated(theta) = m.direction else {
            panic!("回転寸法のまま");
        };
        assert!((theta + FRAC_PI_2).abs() < T, "θ = {theta}");
        // 退化軸では角度が定まらないため元の角度を保つ。
        let EntityGeom::DimLinear(m) = g.mirrored(Point2::ORIGIN, Point2::ORIGIN) else {
            panic!();
        };
        assert_eq!(m.direction, DimDirection::Rotated(FRAC_PI_2));
    }

    #[test]
    fn dim_linear_aabb_covers_the_rotated_dimension_line() {
        // 斜辺に付けた水平寸法（offset 0）。寸法線は (0,0)-(120,0) なので、計測 2 点の
        // 箱（y 0..30）と合わせて x 0..120 / y 0..30 に収まる。
        let bb = EntityGeom::DimLinear(rotated_linear(0.0)).aabb();
        assert!(approx(bb.min, Point2::new(0.0, 0.0)));
        assert!(approx(bb.max, Point2::new(120.0, 30.0)));

        // offset −25（寸法線が下へ出る）。寸法線は y = −25 なので箱が下へ広がる。
        let bb = EntityGeom::DimLinear(DimLinear {
            offset: -25.0,
            ..rotated_linear(0.0)
        })
        .aabb();
        assert!(approx(bb.min, Point2::new(0.0, -25.0)));
        assert!(approx(bb.max, Point2::new(120.0, 30.0)));
    }

    #[test]
    fn dim_linear_aabb_of_an_aligned_dimension_is_unchanged() {
        // M11 タスク68 で AABB の算出を展開（`expand::linear_frame`）へ寄せたが、
        // 整列寸法の結果は M10 までと同じ「計測 2 点 + 法線方向へ offset ずらした
        // 2 点」の箱である。水平な計測 (0,0)-(4,0) に offset +2 → y 0..2。
        let bb = EntityGeom::DimLinear(linear(2.0)).aabb();
        assert!(approx(bb.min, Point2::new(0.0, 0.0)));
        assert!(approx(bb.max, Point2::new(4.0, 2.0)));
        // 符号を反転すると反対側へ広がる。
        let bb = EntityGeom::DimLinear(linear(-2.0)).aabb();
        assert!(approx(bb.min, Point2::new(0.0, -2.0)));
        assert!(approx(bb.max, Point2::new(4.0, 0.0)));
        // 退化（p1≈p2）は計測 2 点のみの箱（向きが決まらないので寸法線を足せない）。
        let bb = EntityGeom::DimLinear(DimLinear {
            p2: Point2::new(0.0, 0.0),
            ..linear(2.0)
        })
        .aabb();
        assert!(approx(bb.min, Point2::ORIGIN));
        assert!(approx(bb.max, Point2::ORIGIN));
    }

    #[test]
    fn dim_linear_aabb_includes_the_expansion_fallback_point_at_zero_projection() {
        // Codex レビュー指摘2: 投影長 0（`Rotated(π/2)` で計測 2 点が寸法線の法線上に
        // 並ぶ）のとき、展開（`expand::expand_linear`）は `linear_frame` の代わりに
        // `p1 + dir.perp() * offset` へ長さ 0 の寸法線と補助線を描く。AABB がこの点を
        // 含まないと、offset が大きいときにズームフィット・カリング・矩形選択が
        // 可視ジオメトリを取りこぼす。
        let dim = DimLinear {
            p1: Point2::new(0.0, 0.0),
            p2: Point2::new(50.0, 0.0),
            offset: 100.0,
            direction: DimDirection::Rotated(FRAC_PI_2),
            ext_angle: None,
            annotation: DimAnnotation::default(),
        };
        assert!(
            dim.measured_value().abs() < T,
            "この検証は投影長 0 が前提: {}",
            dim.measured_value()
        );

        let bb = EntityGeom::DimLinear(dim).aabb();
        // dir = (cos π/2, sin π/2) = (0,1) なので dir.perp() = (-1,0)。
        // フォールバック点 = (0,0) + (-1,0) * 100 = (-100, 0)。
        let fallback = Point2::new(-100.0, 0.0);
        assert!(
            bb.min.x <= fallback.x
                && fallback.x <= bb.max.x
                && bb.min.y <= fallback.y
                && fallback.y <= bb.max.y,
            "AABB {bb:?} が展開のフォールバック点 {fallback:?} を含まない"
        );
    }

    #[test]
    fn dim_linear_validate_rejects_non_finite_direction_angle() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(
                EntityGeom::DimLinear(rotated_linear(bad))
                    .validate()
                    .is_err(),
                "θ = {bad} は拒否されるべき"
            );
        }
        assert!(
            EntityGeom::DimLinear(rotated_linear(0.0))
                .validate()
                .is_ok()
        );
    }

    // -----------------------------------------------------------------
    // M11 タスク78: 補助線の傾き（`DimLinear::ext_angle`）
    // -----------------------------------------------------------------

    #[test]
    fn dim_linear_validate_rejects_only_a_non_finite_extension_line_angle() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let dim = DimLinear {
                ext_angle: Some(bad),
                ..rotated_linear(0.0)
            };
            assert!(
                EntityGeom::DimLinear(dim).validate().is_err(),
                "補助線の傾き {bad} は拒否されるべき"
            );
        }
        // **範囲は拒否も正規化もしない**（絶対角なので 2π 超・負でも意味は決まる）。
        for ok in [0.0, FRAC_PI_2, -3.0, 12.0, 1.0e6] {
            let dim = DimLinear {
                ext_angle: Some(ok),
                ..rotated_linear(0.0)
            };
            assert!(
                EntityGeom::DimLinear(dim.clone()).validate().is_ok(),
                "補助線の傾き {ok} は受理されるべき"
            );
            // 保存値はそのまま（正規化していない）。
            assert_eq!(dim.ext_angle, Some(ok));
        }
    }

    #[test]
    fn dim_linear_extension_line_angle_follows_rotation_and_mirroring() {
        let dim = DimLinear {
            ext_angle: Some(FRAC_PI_2),
            ..rotated_linear(0.0)
        };
        let g = EntityGeom::DimLinear(dim);

        // 平行移動では絶対角は変わらない。
        let EntityGeom::DimLinear(t) = g.translated(Vec2::new(3.0, -1.0)) else {
            panic!();
        };
        assert_eq!(t.ext_angle, Some(FRAC_PI_2));

        // 回転は回転量を加算（寸法線の向きと同じ規則）。
        let EntityGeom::DimLinear(r) = g.rotated(Point2::ORIGIN, FRAC_PI_2) else {
            panic!();
        };
        assert!((r.ext_angle.unwrap() - PI).abs() < T);

        // x 軸鏡映（alpha = 0）は `2·0 − θ = −θ`。
        let EntityGeom::DimLinear(m) = g.mirrored(Point2::ORIGIN, Point2::new(1.0, 0.0)) else {
            panic!();
        };
        assert!((m.ext_angle.unwrap() + FRAC_PI_2).abs() < T);

        // 傾き無しは変換しても無しのまま。
        let plain = EntityGeom::DimLinear(rotated_linear(0.0));
        let EntityGeom::DimLinear(r) = plain.rotated(Point2::ORIGIN, 1.0) else {
            panic!();
        };
        assert_eq!(r.ext_angle, None);
    }

    #[test]
    fn dim_linear_aabb_covers_the_oblique_dimension_line_ends() {
        // 展開・pick と同じ骨格（`expand::linear_frame`）を使うので、傾けて外へ出た
        // 寸法線の端点も AABB に入る。(0,0)-(100,20) の水平寸法・offset 40
        // （寸法線 y = 40）に 45 度の補助線 → 端点は (40,40) と (120,40)。
        let dim = DimLinear {
            p1: Point2::new(0.0, 0.0),
            p2: Point2::new(100.0, 20.0),
            offset: 40.0,
            direction: DimDirection::Rotated(0.0),
            ext_angle: Some(std::f64::consts::FRAC_PI_4),
            annotation: DimAnnotation::default(),
        };
        let bb = EntityGeom::DimLinear(dim).aabb();
        assert!(approx(bb.min, Point2::new(0.0, 0.0)), "{bb:?}");
        assert!(approx(bb.max, Point2::new(120.0, 40.0)), "{bb:?}");
    }

    #[test]
    fn dim_radial_rotate_and_validate() {
        let dim = DimRadial {
            center: Point2::new(1.0, 0.0),
            radius: 3.0,
            leader_angle: 0.0,
            annotation: DimAnnotation::default(),
        };
        let g = EntityGeom::DimRadial(dim.clone());

        // 回転: 中心が動き leader_angle に角が加わる。半径は不変。
        let EntityGeom::DimRadial(r) = g.rotated(Point2::ORIGIN, FRAC_PI_2) else {
            panic!();
        };
        assert!(approx(r.center, Point2::new(0.0, 1.0)));
        assert!((r.leader_angle - FRAC_PI_2).abs() < T);
        assert!((r.radius - 3.0).abs() < T);

        // validate: 非正半径は拒否。
        let bad = EntityGeom::DimRadial(DimRadial { radius: 0.0, ..dim });
        assert!(bad.validate().is_err());
        assert!(g.validate().is_ok());
    }

    // -----------------------------------------------------------------
    // M9 タスク47-2: 注記つき寸法と直径寸法
    // -----------------------------------------------------------------

    fn linear(offset: f64) -> DimLinear {
        DimLinear {
            p1: Point2::new(0.0, 0.0),
            p2: Point2::new(4.0, 0.0),
            offset,
            direction: DimDirection::Aligned,
            ext_angle: None,
            annotation: DimAnnotation::default(),
        }
    }

    fn diameter() -> DimDiameter {
        DimDiameter {
            center: Point2::new(1.0, 0.0),
            radius: 3.0,
            angle: 0.0,
            annotation: DimAnnotation::default(),
        }
    }

    /// 文字位置を上書きした注記（幾何変換への追従を見るため）。
    fn anchored(at: Point2) -> DimAnnotation {
        DimAnnotation {
            text_anchor: Some(at),
            ..DimAnnotation::default()
        }
    }

    #[test]
    fn dim_diameter_translate_rotate_mirror() {
        let g = EntityGeom::DimDiameter(diameter());

        let EntityGeom::DimDiameter(t) = g.translated(Vec2::new(2.0, -1.0)) else {
            panic!();
        };
        assert!(approx(t.center, Point2::new(3.0, -1.0)));
        assert!((t.radius - 3.0).abs() < T);
        assert!((t.angle - 0.0).abs() < T);

        // 回転: 中心が動き、直径線の向きにも回転量が乗る（半径寸法と同じ規則）。
        let EntityGeom::DimDiameter(r) = g.rotated(Point2::ORIGIN, FRAC_PI_2) else {
            panic!();
        };
        assert!(approx(r.center, Point2::new(0.0, 1.0)));
        assert!((r.angle - FRAC_PI_2).abs() < T);
        assert!((r.radius - 3.0).abs() < T);

        // y 軸鏡映: alpha = π/2 → 2·alpha − angle = π。中心の x が反転する。
        let EntityGeom::DimDiameter(m) = g.mirrored(Point2::ORIGIN, Point2::new(0.0, 1.0)) else {
            panic!();
        };
        assert!(approx(m.center, Point2::new(-1.0, 0.0)));
        assert!((m.angle - PI).abs() < T);

        // 退化軸（2 点が同一）では角度が定まらないため元の角度を保つ。
        let EntityGeom::DimDiameter(d) = g.mirrored(Point2::ORIGIN, Point2::ORIGIN) else {
            panic!();
        };
        assert!((d.angle - 0.0).abs() < T);
    }

    #[test]
    fn dim_diameter_aabb_is_the_circle_box() {
        let bb = EntityGeom::DimDiameter(diameter()).aabb();
        assert!(approx(bb.min, Point2::new(-2.0, -3.0)));
        assert!(approx(bb.max, Point2::new(4.0, 3.0)));
    }

    #[test]
    fn dim_diameter_validate_rejects_bad_geometry_and_symbols() {
        assert!(EntityGeom::DimDiameter(diameter()).validate().is_ok());

        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let g = EntityGeom::DimDiameter(DimDiameter {
                radius: bad,
                ..diameter()
            });
            assert!(g.validate().is_err(), "radius {bad} should be rejected");
        }
        assert!(
            EntityGeom::DimDiameter(DimDiameter {
                center: Point2::new(f64::NAN, 0.0),
                ..diameter()
            })
            .validate()
            .is_err()
        );
        assert!(
            EntityGeom::DimDiameter(DimDiameter {
                angle: f64::INFINITY,
                ..diameter()
            })
            .validate()
            .is_err()
        );

        // φ・Sφ は許可、R は不可（DimKind::Diameter の文法）。
        for (symbol, allowed) in [
            (DimSymbol::Diameter, true),
            (DimSymbol::SphereDiameter, true),
            (DimSymbol::Radius, false),
            (DimSymbol::Square, false),
        ] {
            let g = EntityGeom::DimDiameter(DimDiameter {
                annotation: DimAnnotation {
                    symbol: Some(symbol),
                    ..DimAnnotation::default()
                },
                ..diameter()
            });
            assert_eq!(g.validate().is_ok(), allowed, "{symbol:?}");
        }
    }

    /// [`DimAngular`] の最小構成（テスト用の共通道具）。
    fn angular(vertex: Point2, p1: Point2, p2: Point2) -> DimAngular {
        DimAngular {
            vertex,
            p1,
            p2,
            arc_radius: 2.0,
            annotation: DimAnnotation::default(),
        }
    }

    #[test]
    fn dim_angular_validate_rejects_antiparallel_legs() {
        // 頂点 (0,0)、p1 = (10,0)（0 度方向）、p2 = (-10,0)（180 度方向）。
        // なす角はちょうど π で、弧をどちらの半円へ描くか決まらないため拒否する
        // （Codex adversarial review 指摘、M11 タスク69）。
        let g = EntityGeom::DimAngular(angular(
            Point2::ORIGIN,
            Point2::new(10.0, 0.0),
            Point2::new(-10.0, 0.0),
        ));
        let err = g.validate().unwrap_err();
        assert!(err.contains("antiparallel"), "{err}");
    }

    #[test]
    fn dim_angular_validate_accepts_179_9_degrees() {
        // 179.9 度は反平行ではない通常の角度なので、拒否の閾値（1e-6 ラジアン）に
        // 巻き込まれず通る（179.9 度と 180 度の差は約 0.0017 ラジアンで、
        // 閾値よりはるかに大きい）。
        let p2_angle = 179.9f64.to_radians();
        let g = EntityGeom::DimAngular(angular(
            Point2::ORIGIN,
            Point2::new(10.0, 0.0),
            Point2::new(10.0 * p2_angle.cos(), 10.0 * p2_angle.sin()),
        ));
        assert!(g.validate().is_ok());
    }

    #[test]
    fn validate_enforces_the_symbol_grammar_per_dimension_kind() {
        // 長さ寸法に SR（半径系）は不可。半径寸法に φ は不可。
        let linear_sr = EntityGeom::DimLinear(DimLinear {
            annotation: DimAnnotation {
                symbol: Some(DimSymbol::SphereRadius),
                ..DimAnnotation::default()
            },
            ..linear(1.0)
        });
        let err = linear_sr.validate().unwrap_err();
        assert!(err.contains("SphereRadius"), "{err}");

        let radial_phi = EntityGeom::DimRadial(DimRadial {
            center: Point2::ORIGIN,
            radius: 1.0,
            leader_angle: 0.0,
            annotation: DimAnnotation {
                symbol: Some(DimSymbol::Diameter),
                ..DimAnnotation::default()
            },
        });
        assert!(radial_phi.validate().is_err());

        // 許可される組合せは通る（長さ寸法の t、半径寸法の CR）。
        let linear_t = EntityGeom::DimLinear(DimLinear {
            annotation: DimAnnotation {
                symbol: Some(DimSymbol::Thickness),
                ..DimAnnotation::default()
            },
            ..linear(1.0)
        });
        assert!(linear_t.validate().is_ok());
        let radial_cr = EntityGeom::DimRadial(DimRadial {
            center: Point2::ORIGIN,
            radius: 1.0,
            leader_angle: 0.0,
            annotation: DimAnnotation {
                symbol: Some(DimSymbol::ControlRadius),
                ..DimAnnotation::default()
            },
        });
        assert!(radial_cr.validate().is_ok());
    }

    #[test]
    fn validate_rejects_bad_tolerance_on_every_dimension_kind() {
        let bad = DimAnnotation {
            tolerance: Some(SizeTolerance::Deviations {
                upper: -0.2,
                lower: 0.1,
            }),
            ..DimAnnotation::default()
        };
        let cases = [
            EntityGeom::DimLinear(DimLinear {
                annotation: bad.clone(),
                ..linear(1.0)
            }),
            EntityGeom::DimRadial(DimRadial {
                center: Point2::ORIGIN,
                radius: 1.0,
                leader_angle: 0.0,
                annotation: bad.clone(),
            }),
            EntityGeom::DimDiameter(DimDiameter {
                annotation: bad,
                ..diameter()
            }),
        ];
        for g in cases {
            assert!(g.validate().is_err(), "{g:?}");
        }
    }

    #[test]
    fn text_anchor_follows_the_geometry_transform() {
        // 文字位置の手動上書きはワールド座標なので、寸法本体と一緒に動く
        // （動かないと移動・回転で文字だけ取り残される）。
        let g = EntityGeom::DimLinear(DimLinear {
            annotation: anchored(Point2::new(2.0, 2.0)),
            ..linear(1.5)
        });

        let EntityGeom::DimLinear(t) = g.translated(Vec2::new(1.0, 1.0)) else {
            panic!();
        };
        assert!(approx(
            t.annotation.text_anchor.unwrap(),
            Point2::new(3.0, 3.0)
        ));

        let EntityGeom::DimLinear(r) = g.rotated(Point2::ORIGIN, FRAC_PI_2) else {
            panic!();
        };
        assert!(approx(
            r.annotation.text_anchor.unwrap(),
            Point2::new(-2.0, 2.0)
        ));

        let EntityGeom::DimLinear(m) = g.mirrored(Point2::ORIGIN, Point2::new(1.0, 0.0)) else {
            panic!();
        };
        assert!(approx(
            m.annotation.text_anchor.unwrap(),
            Point2::new(2.0, -2.0)
        ));

        // 半径・直径寸法でも同じ（自動配置の `None` は `None` のまま）。
        let radial = EntityGeom::DimRadial(DimRadial {
            center: Point2::ORIGIN,
            radius: 1.0,
            leader_angle: 0.0,
            annotation: anchored(Point2::new(1.0, 0.0)),
        });
        let EntityGeom::DimRadial(rt) = radial.translated(Vec2::new(0.0, 5.0)) else {
            panic!();
        };
        assert!(approx(
            rt.annotation.text_anchor.unwrap(),
            Point2::new(1.0, 5.0)
        ));

        let auto = EntityGeom::DimDiameter(diameter());
        let EntityGeom::DimDiameter(at) = auto.translated(Vec2::new(1.0, 1.0)) else {
            panic!();
        };
        assert_eq!(at.annotation.text_anchor, None);
    }

    /// 文字姿勢の手動固定（[`DimAnnotation::text_rotation`]、M11 タスク70）も絶対角
    /// なので、寸法線の向き（[`DimDirection::Rotated`]）や引出角と同じ規則で追従する
    /// （回転は加算、鏡映は `2·alpha − theta`）。平行移動では変わらない。
    #[test]
    fn text_rotation_follows_the_geometry_transform() {
        let rotated_annotation = || DimAnnotation {
            text_rotation: Some(0.3),
            ..DimAnnotation::default()
        };
        let g = EntityGeom::DimLinear(DimLinear {
            annotation: rotated_annotation(),
            ..linear(1.5)
        });

        // 平行移動: 絶対角は変わらない。
        let EntityGeom::DimLinear(t) = g.translated(Vec2::new(1.0, 1.0)) else {
            panic!();
        };
        assert!((t.annotation.text_rotation.unwrap() - 0.3).abs() < T);

        // 回転: 回転量がそのまま加算される。
        let EntityGeom::DimLinear(r) = g.rotated(Point2::ORIGIN, FRAC_PI_2) else {
            panic!();
        };
        assert!((r.annotation.text_rotation.unwrap() - (0.3 + FRAC_PI_2)).abs() < T);

        // 鏡映（x 軸に対して）: `alpha = 0` なので `2*0 - 0.3 = -0.3`。
        let EntityGeom::DimLinear(m) = g.mirrored(Point2::ORIGIN, Point2::new(1.0, 0.0)) else {
            panic!();
        };
        assert!((m.annotation.text_rotation.unwrap() - (-0.3)).abs() < T);

        // 自動（`None`）はどの変換でも `None` のまま。
        let auto = EntityGeom::DimRadial(DimRadial {
            center: Point2::ORIGIN,
            radius: 1.0,
            leader_angle: 0.0,
            annotation: DimAnnotation::default(),
        });
        let EntityGeom::DimRadial(ar) = auto.rotated(Point2::ORIGIN, FRAC_PI_2) else {
            panic!();
        };
        assert_eq!(ar.annotation.text_rotation, None);
    }

    #[test]
    fn transforms_keep_the_non_geometric_annotation_fields() {
        let annotation = DimAnnotation {
            symbol: Some(DimSymbol::Diameter),
            tolerance: Some(SizeTolerance::Symmetric(0.1)),
            decimals_override: Some(3),
            text_anchor: Some(Point2::new(1.0, 1.0)),
            arrow_placement: ArrowPlacement::Outside,
            value_override: Some("5-10".to_string()),
            text_rotation: Some(0.2),
            value_style: ValueStyle::Reference,
            prefix: Some("2×".to_string()),
            suffix: Some("-M6".to_string()),
        };
        let g = EntityGeom::DimLinear(DimLinear {
            annotation: annotation.clone(),
            ..linear(1.0)
        });
        let EntityGeom::DimLinear(t) = g.translated(Vec2::new(3.0, 0.0)) else {
            panic!();
        };
        assert_eq!(t.annotation.symbol, annotation.symbol);
        assert_eq!(t.annotation.tolerance, annotation.tolerance);
        assert_eq!(t.annotation.decimals_override, annotation.decimals_override);
        assert_eq!(t.annotation.arrow_placement, annotation.arrow_placement);
        assert_eq!(t.annotation.value_override, annotation.value_override);
    }

    // -----------------------------------------------------------------
    // 保存形式の後方互換（v4 との JSON 互換。詳細な .mcad 往復は io 層）
    // -----------------------------------------------------------------

    #[test]
    fn unannotated_dimensions_omit_the_annotation_field() {
        // 無注記なら `annotation` フィールドごと出力されない。
        //
        // **`DimLinear` だけは v4 と同形ではなくなった**（M11 タスク68）: `direction` は
        // `DimStyle::arrow_kind`（M10）と同じく `#[serde(default)]` のみで
        // `skip_serializing_if` を付けない方針なので、整列寸法でも必ず書かれる。
        // 読み手が既定値を知らなくても整列／回転が判るほうが、1 トークン分の
        // ファイル増より価値がある（annotation を省くのは「無注記が大多数で、
        // かつ深い入れ子だから」という別の理由による）。
        let json = serde_json::to_string(&EntityGeom::DimLinear(linear(1.5))).unwrap();
        assert_eq!(
            json,
            r#"{"DimLinear":{"p1":{"x":0.0,"y":0.0},"p2":{"x":4.0,"y":0.0},"offset":1.5,"direction":"Aligned"}}"#
        );
        // 回転寸法は絶対角（ラジアン）を外部タグ形式で持つ。
        let json = serde_json::to_string(&EntityGeom::DimLinear(DimLinear {
            direction: DimDirection::Rotated(FRAC_PI_2),
            ..linear(1.5)
        }))
        .unwrap();
        assert!(
            json.contains(r#""direction":{"Rotated":1.5707963267948966}"#),
            "回転寸法の JSON: {json}"
        );

        let json = serde_json::to_string(&EntityGeom::DimRadial(DimRadial {
            center: Point2::ORIGIN,
            radius: 2.0,
            leader_angle: 0.0,
            annotation: DimAnnotation::default(),
        }))
        .unwrap();
        assert_eq!(
            json,
            r#"{"DimRadial":{"center":{"x":0.0,"y":0.0},"radius":2.0,"leader_angle":0.0}}"#
        );
    }

    #[test]
    fn v4_json_without_annotation_loads_as_unannotated() {
        let parsed: EntityGeom = serde_json::from_str(
            r#"{"DimLinear":{"p1":{"x":0.0,"y":0.0},"p2":{"x":4.0,"y":0.0},"offset":1.5}}"#,
        )
        .unwrap();
        assert_eq!(parsed, EntityGeom::DimLinear(linear(1.5)));

        let parsed: EntityGeom = serde_json::from_str(
            r#"{"DimRadial":{"center":{"x":0.0,"y":0.0},"radius":2.0,"leader_angle":0.0}}"#,
        )
        .unwrap();
        let EntityGeom::DimRadial(dim) = &parsed else {
            panic!();
        };
        assert!(dim.annotation.is_unannotated());
    }

    #[test]
    fn annotated_dimensions_round_trip_through_serde() {
        let g = EntityGeom::DimDiameter(DimDiameter {
            annotation: DimAnnotation {
                symbol: Some(DimSymbol::SphereDiameter),
                tolerance: Some(SizeTolerance::Fit(FitClass::new("H7").unwrap())),
                decimals_override: Some(1),
                text_anchor: Some(Point2::new(1.0, 2.0)),
                arrow_placement: ArrowPlacement::Inside,
                value_override: Some("5-10".to_string()),
                text_rotation: Some(0.1),
                value_style: ValueStyle::TheoreticallyExact,
                prefix: Some("2×".to_string()),
                suffix: Some("-M6".to_string()),
            },
            ..diameter()
        });
        let json = serde_json::to_string(&g).unwrap();
        assert!(json.contains("annotation"), "{json}");
        assert_eq!(serde_json::from_str::<EntityGeom>(&json).unwrap(), g);
    }

    // ------------------------------------------------------------------
    // 表（M10 タスク56）
    // ------------------------------------------------------------------

    /// 2 行 × 3 列の表。幅 30+40+50 = 120mm、高さ 8+10 = 18mm。
    /// `cells` は行優先で行 0（"a" "b" "c"）が最上段。
    fn table(anchor: Point2) -> TableGeom {
        TableGeom {
            anchor,
            col_widths_mm: vec![30.0, 40.0, 50.0],
            row_heights_mm: vec![8.0, 10.0],
            text_height_mm: 3.5,
            cells: ["a", "b", "c", "d", "e", "f"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        }
    }

    /// `rows` 行 × `cols` 列の、寸法としては妥当な表（件数の境界だけを見たいとき用）。
    fn table_with_counts(rows: usize, cols: usize) -> TableGeom {
        TableGeom {
            anchor: Point2::ORIGIN,
            col_widths_mm: vec![1.0; cols],
            row_heights_mm: vec![1.0; rows],
            text_height_mm: 0.5,
            cells: vec![String::new(); rows * cols],
        }
    }

    #[test]
    fn table_helpers_report_shape_extent_and_cells() {
        let t = table(Point2::new(10.0, 20.0));
        assert_eq!(t.rows(), 2);
        assert_eq!(t.cols(), 3);
        assert!((t.width_mm() - 120.0).abs() < T);
        assert!((t.height_mm() - 18.0).abs() < T);

        // 行優先・行 0 が最上段。
        assert_eq!(t.cell(0, 0), Some("a"));
        assert_eq!(t.cell(0, 2), Some("c"));
        assert_eq!(t.cell(1, 0), Some("d"));
        assert_eq!(t.cell(1, 2), Some("f"));

        // 範囲外は None（panic しない）。
        assert_eq!(t.cell(2, 0), None);
        assert_eq!(t.cell(0, 3), None);
        assert_eq!(t.cell(usize::MAX, usize::MAX), None);
    }

    #[test]
    fn table_cell_returns_none_on_a_short_cells_vector() {
        // validate を通っていない値でも `cell` は panic せず None を返す
        // （タスク57/58 が読込直後や編集途中の値を触っても落ちないための契約）。
        let mut t = table(Point2::ORIGIN);
        t.cells.truncate(4);
        assert_eq!(t.cell(0, 0), Some("a"));
        assert_eq!(t.cell(1, 1), None);
        assert!(EntityGeom::Table(t).validate().is_err());
    }

    #[test]
    fn table_aabb_is_the_one_to_one_paper_box_from_the_bottom_left_anchor() {
        // 紙 mm を 1:1 で解釈し、アンカー（左下）から +x/+y へ広がる（M10 設計方針4）。
        let g = EntityGeom::Table(table(Point2::new(10.0, 20.0)));
        let bb = g.aabb();
        assert!(approx(bb.min, Point2::new(10.0, 20.0)));
        assert!(approx(bb.max, Point2::new(130.0, 38.0)));
    }

    #[test]
    fn table_transforms_move_only_the_anchor() {
        // 表は回さない・鏡像化しない（M10 設計方針3・詳細設計1）。平行移動・回転・
        // 鏡映のいずれでもアンカー以外のフィールドは一切変わらない。
        let original = table(Point2::new(10.0, 20.0));
        let g = EntityGeom::Table(original.clone());

        let unchanged_except_anchor = |moved: &EntityGeom, expected_anchor: Point2, why: &str| {
            let EntityGeom::Table(t) = moved else {
                panic!("{why}: expected a Table, got {moved:?}");
            };
            assert!(approx(t.anchor, expected_anchor), "{why}: {:?}", t.anchor);
            assert_eq!(t.col_widths_mm, original.col_widths_mm, "{why}");
            assert_eq!(t.row_heights_mm, original.row_heights_mm, "{why}");
            assert_eq!(t.text_height_mm, original.text_height_mm, "{why}");
            assert_eq!(t.cells, original.cells, "{why}");
        };

        unchanged_except_anchor(
            &g.translated(Vec2::new(3.0, -4.0)),
            Point2::new(13.0, 16.0),
            "translate",
        );
        // 90° 回転しても罫線は軸平行のまま（アンカーだけが回る）。
        unchanged_except_anchor(
            &g.rotated(Point2::ORIGIN, FRAC_PI_2),
            Point2::new(-20.0, 10.0),
            "rotate",
        );
        // y 軸に対する鏡映。列の並びもセル文字も鏡像化しない。
        unchanged_except_anchor(
            &g.mirrored(Point2::ORIGIN, Point2::new(0.0, 1.0)),
            Point2::new(-10.0, 20.0),
            "mirror",
        );
    }

    #[test]
    fn table_validate_accepts_the_reference_table() {
        // 受理側も固定する（常に Err を返す実装への退行防止）。
        assert_eq!(EntityGeom::Table(table(Point2::ORIGIN)).validate(), Ok(()));
    }

    #[test]
    fn table_validate_rejects_degenerate_structure() {
        let no_rows = TableGeom {
            row_heights_mm: vec![],
            cells: vec![],
            ..table(Point2::ORIGIN)
        };
        let err = EntityGeom::Table(no_rows).validate().unwrap_err();
        assert!(err.contains("no rows"), "{err}");

        let no_cols = TableGeom {
            col_widths_mm: vec![],
            cells: vec![],
            ..table(Point2::ORIGIN)
        };
        let err = EntityGeom::Table(no_cols).validate().unwrap_err();
        assert!(err.contains("no columns"), "{err}");

        // セル数の過不足はどちらも拒否する（行×列とちょうど一致すること）。
        let mut too_few = table(Point2::ORIGIN);
        too_few.cells.pop();
        let err = EntityGeom::Table(too_few).validate().unwrap_err();
        assert!(err.contains("cell count mismatch"), "{err}");

        let mut too_many = table(Point2::ORIGIN);
        too_many.cells.push("g".to_owned());
        let err = EntityGeom::Table(too_many).validate().unwrap_err();
        assert!(err.contains("cell count mismatch"), "{err}");
    }

    #[test]
    fn table_validate_rejects_non_finite_and_non_positive_dimensions() {
        let nan_anchor = EntityGeom::Table(table(Point2::new(f64::NAN, 0.0)));
        assert!(nan_anchor.validate().is_err());

        let mut nan_width = table(Point2::ORIGIN);
        nan_width.col_widths_mm[1] = f64::NAN;
        assert!(EntityGeom::Table(nan_width).validate().is_err());

        let mut inf_height = table(Point2::ORIGIN);
        inf_height.row_heights_mm[0] = f64::INFINITY;
        assert!(EntityGeom::Table(inf_height).validate().is_err());

        let mut zero_width = table(Point2::ORIGIN);
        zero_width.col_widths_mm[0] = 0.0;
        assert!(EntityGeom::Table(zero_width).validate().is_err());

        let mut negative_height = table(Point2::ORIGIN);
        negative_height.row_heights_mm[1] = -1.0;
        assert!(EntityGeom::Table(negative_height).validate().is_err());

        let mut zero_text = table(Point2::ORIGIN);
        zero_text.text_height_mm = 0.0;
        assert!(EntityGeom::Table(zero_text).validate().is_err());

        let mut nan_text = table(Point2::ORIGIN);
        nan_text.text_height_mm = f64::NAN;
        assert!(EntityGeom::Table(nan_text).validate().is_err());
    }

    #[test]
    fn table_validate_requires_text_height_below_the_smallest_row_height() {
        // 最小の行高さは 8.0。等号は拒否、わずかに下は受理（境界の向きを固定する）。
        let mut equal = table(Point2::ORIGIN);
        equal.text_height_mm = 8.0;
        let err = EntityGeom::Table(equal).validate().unwrap_err();
        assert!(err.contains("smallest row height"), "{err}");

        let mut above = table(Point2::ORIGIN);
        above.text_height_mm = 9.0;
        assert!(EntityGeom::Table(above).validate().is_err());

        let mut just_below = table(Point2::ORIGIN);
        just_below.text_height_mm = 8.0 - 1e-9;
        assert_eq!(EntityGeom::Table(just_below).validate(), Ok(()));

        // 判定に使うのは最大でも平均でもなく **最小** の行高さ。
        let mut small_row_last = table(Point2::ORIGIN);
        small_row_last.row_heights_mm = vec![10.0, 4.0];
        small_row_last.text_height_mm = 5.0;
        assert!(EntityGeom::Table(small_row_last).validate().is_err());
    }

    #[test]
    fn table_validate_allows_empty_cells_but_rejects_control_characters() {
        // 空欄のある表は普通なので空文字列は受理する（TextGeom とは意図的に違える）。
        let mut empty_cells = table(Point2::ORIGIN);
        empty_cells.cells = vec![String::new(); 6];
        assert_eq!(EntityGeom::Table(empty_cells).validate(), Ok(()));

        // 改行・タブ・その他の制御文字は 1 行 1 セルの組版と出力の文字列を壊す。
        for bad in ["a\nb", "a\tb", "a\u{7}b"] {
            let mut t = table(Point2::ORIGIN);
            t.cells[4] = bad.to_owned();
            let err = EntityGeom::Table(t).validate().unwrap_err();
            assert!(err.contains("control characters"), "{bad:?}: {err}");
            // 行優先のインデックス（4 = 1 行 1 列）が報告される。
            assert!(err.contains("cell 1/1"), "{bad:?}: {err}");
        }

        // 日本語・空白は通常の文字として受理する。
        let mut japanese = table(Point2::ORIGIN);
        japanese.cells[0] = "照合番号 1".to_owned();
        assert_eq!(EntityGeom::Table(japanese).validate(), Ok(()));
    }

    #[test]
    fn table_validate_rejects_too_many_rows_or_columns() {
        // 寸法の上限だけでは極小の行高さを大量に並べる細工を防げない
        // （表題欄 MAX_TITLE_BLOCK_ROWS と同じ論点）。上限ちょうどは受理する。
        assert_eq!(
            EntityGeom::Table(table_with_counts(MAX_TABLE_ROWS, MAX_TABLE_COLS)).validate(),
            Ok(())
        );

        let too_many_rows = table_with_counts(MAX_TABLE_ROWS + 1, 1);
        let err = EntityGeom::Table(too_many_rows).validate().unwrap_err();
        assert!(err.contains("too many rows"), "{err}");

        let too_many_cols = table_with_counts(1, MAX_TABLE_COLS + 1);
        let err = EntityGeom::Table(too_many_cols).validate().unwrap_err();
        assert!(err.contains("too many columns"), "{err}");

        // 極小値で合計を上限以下に保っても件数で弾かれる（回帰の本体）。
        let mut tiny = table_with_counts(MAX_TABLE_ROWS + 1, 1);
        tiny.row_heights_mm = vec![1e-6; MAX_TABLE_ROWS + 1];
        tiny.text_height_mm = 1e-9;
        assert!(tiny.height_mm() < MAX_TABLE_MM, "テスト前提: 合計は上限内");
        assert!(EntityGeom::Table(tiny).validate().is_err());
    }

    #[test]
    fn table_validate_rejects_huge_dimensions_and_sums() {
        // 個々には有限でも上限を超える値は弾く（巨大値が AABB や SVG/PDF 座標へ
        // 届かないようにする。表題欄 MAX_TITLE_BLOCK_MM と同じ論点）。
        let mut huge_col = table(Point2::ORIGIN);
        huge_col.col_widths_mm[0] = f64::MAX;
        assert!(EntityGeom::Table(huge_col).validate().is_err());

        // 個々の値が上限内でも合計が超える構成も弾く。
        let big = MAX_TABLE_MM * 0.9;

        let mut wide = table(Point2::ORIGIN);
        wide.col_widths_mm = vec![big, big];
        wide.cells = vec![String::new(); 4];
        let err = EntityGeom::Table(wide).validate().unwrap_err();
        assert!(err.contains("total table width"), "{err}");

        let mut tall = table(Point2::ORIGIN);
        tall.row_heights_mm = vec![big, big];
        tall.cells = vec![String::new(); 6];
        let err = EntityGeom::Table(tall).validate().unwrap_err();
        assert!(err.contains("total table height"), "{err}");

        // 上限ちょうどは受理する（境界の向きを固定する）。
        let at_limit = TableGeom {
            anchor: Point2::ORIGIN,
            col_widths_mm: vec![MAX_TABLE_MM],
            row_heights_mm: vec![MAX_TABLE_MM],
            text_height_mm: 3.5,
            cells: vec![String::new()],
        };
        assert_eq!(EntityGeom::Table(at_limit).validate(), Ok(()));
    }

    #[test]
    fn table_validate_accepts_extreme_but_finite_anchors() {
        // Codex adversarial review 2026-09-06 medium 指摘への回帰固定
        // （`validate_table` 内のコメント参照）。現行 [`MAX_TABLE_MM`] の下では
        // `anchor + 幅/高さ`（右上隅）は f64 の丸めでも非有限へあふれない
        // （実測: `f64::MAX + MAX_TABLE_MM == f64::MAX`）ので、極端なアンカーでも
        // 受理されることを固定する。
        for x in [f64::MAX, -f64::MAX, f64::MAX - 1.0] {
            let extreme = TableGeom {
                anchor: Point2::new(x, x),
                ..table(Point2::ORIGIN)
            };
            assert_eq!(
                EntityGeom::Table(extreme).validate(),
                Ok(()),
                "anchor {x} should be accepted (far corner stays finite)"
            );
        }
    }

    #[test]
    fn table_round_trips_through_serde() {
        // `.mcad` は EntityGeom を serde でそのまま直列化する（v6 はタスク57）。
        let g = EntityGeom::Table(table(Point2::new(1.5, -2.5)));
        let json = serde_json::to_string(&g).unwrap();
        assert_eq!(serde_json::from_str::<EntityGeom>(&json).unwrap(), g);
    }
}
