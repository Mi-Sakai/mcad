//! 幾何拘束の汎用ソルバー（DESIGN.md M13 設計判断1、タスク95）。
//!
//! エンティティを知らない非線形方程式系のソルバー。呼び出し側（mcad-core）が図形の
//! パラメータを 1 本のベクトルに並べ、拘束を「残差と解析的ヤコビアンを返す方程式」
//! （[`Equation`]）へ落として渡す。geom は `EntityId` も拘束の種類も知らない
//! （依存方向の不変条件）。
//!
//! # 入力と出力
//!
//! [`Problem`] に、パラメータの初期値・固定マスク・パラメータごとの重み・方程式・
//! 途中の形を棄却するフックをまとめ、[`solve`] へ渡す。結果は解いたパラメータ
//! （[`Solution`]）か [`SolveError`]。**入力のスライスは書き換えない**（失敗したときの
//! ロールバックは、呼び出し側が結果を捨てるだけで済む）。
//!
//! 1 本の [`Equation`] はスカラーの残差 1 つを表す。x・y の 2 本を持つ拘束（一致・点の固定）は
//! 2 本の方程式として渡す。方程式の順とパラメータの順は呼び出し側が決め、ソルバーは
//! その順のまま処理する（乱数を使わず、同じ入力からは同じ結果か同じ失敗になる）。
//!
//! # 許容値（方程式ごと）
//!
//! 各方程式は、現在のパラメータから残差の種類と尺度（[`ResidualKind`]）を返す。
//! 許容値は [`ResidualKind::tolerance`] で決まる:
//!
//! - 長さ型: `max(RTOL × L, K_ROUNDOFF × f64::EPSILON × C, ATOL_LEN)`。`L` は拘束対象の
//!   代表長さ（参照するエンティティの大きさと拘束の値の最大）、`C` は対象の点の座標の
//!   絶対値の最大。どちらも方程式側が計算する。
//! - 角度型: `ATOL_ANG`（ラジアン。尺度にも位置にも依存しない）。
//!
//! 許容値は**求解の開始時に 1 回だけ計算して固定**する（`tol₀`）。反復中は変えず、
//! ヤコビアンに許容値の微分は含めない（反復中に計算し直すと、残差を減らさずに `L` を
//! 大きくするだけで割った残差が下がり、満たされていない形を収束と誤判定できる）。
//! **成功は、`tol₀` と、解いた後の形から計算し直した許容値の両方で
//! `max |fᵢ / tolᵢ| ≤ 1`** を満たすこと。許容値は有限かつ正であることを確かめてから割る。
//! `.mcad` 読込時の充足検査は [`max_scaled_residual`] / [`is_satisfied`] で同じ判定を使える。
//!
//! # 解法（最小ノルム型の減衰ガウス・ニュートン法）
//!
//! 固定されていないパラメータのうち、どれかの方程式が参照するものを未知数にする
//! （参照されない自由なパラメータは、どの方程式の残差も変えないので動かさない）。
//! パラメータ `j` の重みを `wⱼ` として尺度を変えた変数 `yⱼ = wⱼ xⱼ` を使い、
//!
//! - 割った残差 `rᵢ = fᵢ / tol₀ᵢ`、目的関数 `F = ½ Σ rᵢ²`
//! - 行を許容値で・列を重みで割ったヤコビアン `Aᵢⱼ = (∂fᵢ/∂xⱼ) / (tol₀ᵢ wⱼ)`
//! - 更新量 `Δy = −Aᵀ (A Aᵀ + λI)⁻¹ r`、`Δxⱼ = Δyⱼ / wⱼ`
//!
//! とする。`Δy` は `A` の行空間にあり、`λ → 0` で `‖W Δx‖` を最小にする更新
//! （最小ノルム解）へ近づく。拘束が足りない系では、満たすための動きが（重みで測って）
//! 最小の解を選ぶ。`(A Aᵀ + λI)` は密行列のガウス消去（部分ピボット）で解く。
//!
//! 角度のパラメータ（円弧の開始角・終了角）の重みには、その円弧の半径（求解開始時の値）を
//! 与えると、角度の変化を弧長として長さのパラメータと同じ物差しで測れる
//! （設計判断1「角度パラメータは半径で重みづけする」）。長さのパラメータの重みは 1。
//!
//! # 受理規則
//!
//! 試行点 `x' = x + Δx` を、次をすべて満たすときだけ受理して `x ← x'`・
//! `μ ← max(μ / 10, LAMBDA_MIN)` とする。どれかを満たさなければ `x` を変えず
//! （棄却）、`μ ← μ × 10`:
//!
//! 1. `x'` がすべて有限。
//! 2. 呼び出し側のフック [`Problem::admissible`] が `x'` を受け入れる
//!    （線分の長さ・半径が `ATOL_LEN` を下回る形を棄却する用途）。
//! 3. `x'` での残差がすべて有限で、`F(x') < F(x)`。
//! 4. `x'` でのヤコビアン（割ったもの）とその `A Aᵀ` がすべて有限。
//!
//! 受理したステップでは `F` が必ず減る（[`Solution::objective`] は狭義単調減少）。
//!
//! ## 減衰係数の単位
//!
//! 減衰係数は `λ = μ × s`（`s` は現在の点での `max diag(A Aᵀ)`。0 なら 1）で与え、
//! 定数 [`LAMBDA_0`]・[`LAMBDA_MIN`]・[`LAMBDA_MAX`] は **`μ`（`s` を単位にした無次元の値）**
//! に対する値とする。設計判断1 の `λ = max(λ₀ × max diag(J Jᵀ), λ_MIN)` を `s` 単位で
//! 書いたもので、初期値は同じ `λ₀ × max diag`。下限・上限を絶対値でなく `s` 単位にするのは、
//! `A` の大きさが `1 / tol₀` に比例し、`tol₀` が 1e-12 mm（`ATOL_LEN`）から 1e-4 mm
//! （代表長さ 1e5 mm）まで桁で変わるため:
//! 絶対値の下限は大きい `A` に対して効かず（`A Aᵀ` が階数落ちのとき条件数が発散する）、
//! 絶対値の上限は小さい `A` で早すぎる停滞判定になる。`s` 単位にすると、図形を一様に
//! 拡大縮小しても反復の列が（丸めを除き）そのまま拡大縮小される。`s > 0` なので
//! `A Aᵀ + λI` は常に正則（設計判断1 の「`λ_MIN > 0` で正則」を保つ）。
//!
//! # 成功と失敗
//!
//! - 開始時にパラメータ・残差・ヤコビアン・`F` のどれかが有限でなければ
//!   [`SolveError::NonFinite`]、許容値が有限かつ正でなければ [`SolveError::InvalidTolerance`]。
//! - 開始時にすでに満たされていれば、反復せずに入力をそのまま返す。
//! - 自由な（固定されておらず、どれかの方程式が参照する）パラメータが無く、満たされて
//!   いなければ、反復せずに [`SolveError::NoFreeParameters`]。
//! - 受理したステップのあとで両方の許容値を満たしたら成功。
//! - `μ > LAMBDA_MAX` で [`SolveError::Stalled`]（残差を減らす方向が無い: 解の無い系・
//!   局所解）、試行回数（受理と棄却の合計）が [`MAX_TRIALS`] に達したら
//!   [`SolveError::TrialLimit`]。
//!
//! # 自由度
//!
//! [`degrees_of_freedom`] は「固定していないパラメータの数 − ヤコビアンの階数」を返す
//! （右パネルの表示用。設計判断6）。階数は行・列のスケーリングで変わらないので、重みも
//! 許容値も使わない（重みが要るのは最小ノルムの更新だけ）。ヤコビアンの列を、続いて行を
//! 最大絶対値 1 に正規化し、完全ピボット（行・列とも）のガウス消去で数える。重みを何桁
//! 変えても結果は同じで、円弧の角度の列のように大きさが半径に比例する列があっても、
//! 図形の大きさで自由度が変わらない。
//!
//! # 定数の根拠
//!
//! 各定数の doc コメントを参照（DESIGN.md M13 の実装時追記へ転記する）。

/// 長さ型の許容値の相対項 `RTOL × L` の係数。
///
/// 設計判断1 の目安（1e-9 前後）どおり、既存の相対イプシロン [`crate::EPS`] と同じ値。
/// 代表長さ 10 mm の図形で 1e-8 mm（10 pm）、1e5 mm で 1e-4 mm。`f64` の丸め
/// （相対 2.2e-16）より 7 桁粗いので反復で届き、製図上意味のある寸法（0.01 mm）より
/// 十分細かい。
pub const RTOL: f64 = 1e-9;

/// 長さ型の許容値の絶対下限（mm）。
///
/// 許容値を必ず正にするためだけの値で、判定の粗さは決めない（設計判断1 の目安 1e-12 mm）。
/// 原点で一致する 2 点のように `L` と `C` がともに 0 でも、許容値が 0 にならず 0/0 を
/// 起こさない。呼び出し側は、途中の形で線分の長さ・半径がこれを下回るものを
/// [`Problem::admissible`] で棄却し、ノルムのヤコビアンが `‖v‖ < ATOL_LEN` で固定の
/// 方向 `(1, 0)` を使う閾値にも使う（設計判断2）。
pub const ATOL_LEN: f64 = 1e-12;

/// 角度型の許容値（ラジアン）。
///
/// 位置にも尺度にも依存しない。`RTOL` と同じ 1e-9 にすると、水平拘束（長さ型: 端点の
/// y の差 ≤ `RTOL × 長さ`）と、水平な線分への平行拘束（角度型: なす角 ≤ `ATOL_ANG`）の
/// 厳しさがそろう。
///
/// 既知の限界: 原点から `C` 離れた長さ `ℓ` の線分の向きは、座標の丸めだけで
/// およそ `f64::EPSILON × C / ℓ` ずれる。`C = 1e6 mm` なら `ℓ ≲ 0.2 mm` の線分では
/// この値に届かず、角度型の拘束が収束しないことがある（設計判断1 の「遠方でも角度型の
/// 許容値を緩めない」を採った結果。実測: 長さ 0.01 mm の線分の角度拘束は原点では解け、
/// 原点から 1e6 mm では停滞する）。
pub const ATOL_ANG: f64 = 1e-9;

/// 長さ型の許容値の丸め項 `K_ROUNDOFF × f64::EPSILON × C` の係数 `K`。
///
/// 座標が `C` 程度のとき、パラメータ 1 つの表現誤差は `f64::EPSILON × C / 2` 程度で、
/// 残差の計算で数回の加減算を経る。16 倍はその数 ulp に対する余裕。原点から 1e6 mm でも
/// `16 × 2.2e-16 × 1e6 ≈ 3.6e-9 mm` で、設計判断1 の「1e-9 mm 台」に収まり、
/// 代表長さ 3.6 mm 以上の図形では `RTOL × L` の方が大きい（位置が判定に効かない）。
pub const K_ROUNDOFF: f64 = 16.0;

/// 減衰係数の初期値（`s = max diag(A Aᵀ)` 単位。モジュール doc「減衰係数の単位」）。
///
/// Madsen・Nielsen・Tingleff "Methods for Non-Linear Least Squares Problems"（2004）が
/// 初期値の目安として挙げる `τ ∈ {1e-6, 1e-3, 1}` の中間。拘束を付けた直後や遠くへの
/// 移動のように初期値が解から遠い場合もあるので、1e-6 より保守的に取る。
pub const LAMBDA_0: f64 = 1e-3;

/// 減衰係数の下限（`s` 単位）。
///
/// 受理が続いても `μ` はこれより下げない。`A Aᵀ` が階数落ち（矛盾しない重複拘束）でも
/// `A Aᵀ + λI` の条件数がおよそ 1e12 以下に収まり、部分ピボットのガウス消去で失う桁が
/// 4 桁程度にとどまる。これより小さくしてもガウス・ニュートン法としての収束は速くならない。
pub const LAMBDA_MIN: f64 = 1e-12;

/// 減衰係数の上限（`s` 単位）。これを超えたら停滞として失敗する。
///
/// `μ` のとき 1 ステップで期待できる `F` の相対的な減少はおよそ `2m / μ`（`m` は方程式の
/// 本数）以下で、`μ = 1e20` では `m` が 1000 本でも `f64::EPSILON` を下回り、`F` の減少が
/// 浮動小数で表せない。超えるまでの連続した棄却は、`LAMBDA_MIN` からで約 33 回、
/// `LAMBDA_0` からで約 24 回。
pub const LAMBDA_MAX: f64 = 1e20;

/// 試行回数（受理と棄却の合計）の上限。
///
/// 実測（`tests/solve.rs` の `trial_counts_stay_well_below_limit`。解から図形の 100 倍
/// 離れた初期値・接線・±π 付近の角度を、代表長さ 1e-3・1・1e5 mm、原点と原点から
/// 1e6 mm で解く）: 収束したものは 5 回以下。解の無い系は、最小二乗の極小へゆっくり
/// 近づいてから停滞と判定されるまで 29〜110 回。最悪の停滞の約 2 倍を上限にして、
/// 解けない系でも決定的に止め、グリップのドラッグ中に毎フレーム解いても失敗する位置で
/// 時間を使い切らない値にする（上限に達したときも [`SolveError::TrialLimit`] で失敗する）。
pub const MAX_TRIALS: usize = 200;

/// [`degrees_of_freedom`] で階数を数えるときのピボットの相対閾値。
///
/// ヤコビアンの列と行を最大絶対値 1 に正規化してから完全ピボットで消去し、残った
/// 部分行列の最大絶対値（次のピボット）が、それまでの最大ピボットのこの倍数以下になったら
/// 残りを 0 とみなす。正規化で最初のピボットは 1 になり、閾値は図形の大きさ・座標の位置・
/// 重みに依らない。矛盾しない重複行は消去後に `f64::EPSILON` の数倍（1e-15 程度）しか
/// 残らず、独立な行はよほど特異に近い形でない限り 1e-3 以上のピボットを持つ。その間の
/// 幾何平均に近い値。行列の次元の係数（`max(m, n) × ε` の形）は掛けない: 1 成分で
/// 数百行の規模でも丸めの残りは 1e-13 程度で、この値より 4 桁小さい。
pub const RANK_RTOL: f64 = 1e-9;

/// 残差の種類と、長さ型なら許容値の元になる尺度（モジュール doc「許容値」）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResidualKind {
    /// 長さ型（一致・距離・水平/垂直・点と線の距離・接線・等長・等半径・半径・点の固定）。
    Length {
        /// 代表長さ `L`（mm）。参照するエンティティの大きさ（線分の長さ・円と円弧の半径）と
        /// 拘束の値の最大。有限かつ 0 以上。
        size: f64,
        /// 対象の点の座標の絶対値の最大 `C`（mm）。有限かつ 0 以上。
        coord: f64,
    },
    /// 角度型（平行・直交・角度）。許容値は [`ATOL_ANG`]。
    Angle,
}

impl ResidualKind {
    /// この種類・尺度の許容値。尺度が有限かつ 0 以上でなければ `None`。
    ///
    /// 戻り値は `Some` なら必ず有限かつ正（[`ATOL_LEN`] 以上）。`f64::max` は NaN を
    /// 黙って捨てるので、尺度の検査を先に行う。
    #[must_use]
    pub fn tolerance(self) -> Option<f64> {
        match self {
            Self::Length { size, coord } => {
                let valid = size.is_finite() && coord.is_finite() && size >= 0.0 && coord >= 0.0;
                if !valid {
                    return None;
                }
                let tol = (RTOL * size)
                    .max(K_ROUNDOFF * f64::EPSILON * coord)
                    .max(ATOL_LEN);
                (tol.is_finite() && tol > 0.0).then_some(tol)
            }
            Self::Angle => Some(ATOL_ANG),
        }
    }
}

/// スカラーの残差 1 つを表す方程式（残差 = 0 が満たされた状態）。
///
/// 実装する側の約束:
///
/// - [`params`](Self::params) は残差が依存するパラメータの添字で、重複を含めない。
///   許容値の尺度（[`kind`](Self::kind)）だけに使うパラメータは含めない
///   （ヤコビアンに許容値の微分は含めない）。
/// - [`gradient`](Self::gradient) は `∂f/∂x[params()[k]]` を `grad[k]` へ書く
///   （`grad.len() == params().len()`）。解析式で書き、[`check_jacobian`] で中心差分と照合する。
/// - どのメソッドも `x` を書き換えず、同じ `x` には同じ値を返す（決定性）。
pub trait Equation {
    /// 残差が依存するパラメータの添字（重複なし）。
    fn params(&self) -> &[usize];

    /// 残差 `f(x)`。
    fn residual(&self, x: &[f64]) -> f64;

    /// 解析的な偏微分を `params()` の順に `grad` へ書く。
    fn gradient(&self, x: &[f64], grad: &mut [f64]);

    /// `x` での残差の種類と許容値の尺度。
    fn kind(&self, x: &[f64]) -> ResidualKind;
}

impl<T: Equation + ?Sized> Equation for &T {
    fn params(&self) -> &[usize] {
        (**self).params()
    }
    fn residual(&self, x: &[f64]) -> f64 {
        (**self).residual(x)
    }
    fn gradient(&self, x: &[f64], grad: &mut [f64]) {
        (**self).gradient(x, grad);
    }
    fn kind(&self, x: &[f64]) -> ResidualKind {
        (**self).kind(x)
    }
}

impl<T: Equation + ?Sized> Equation for Box<T> {
    fn params(&self) -> &[usize] {
        (**self).params()
    }
    fn residual(&self, x: &[f64]) -> f64 {
        (**self).residual(x)
    }
    fn gradient(&self, x: &[f64], grad: &mut [f64]) {
        (**self).gradient(x, grad);
    }
    fn kind(&self, x: &[f64]) -> ResidualKind {
        (**self).kind(x)
    }
}

/// 途中の形を受け入れてよいかを判定するフックの型（`true` で受け入れる）。
pub type Admissible<'a> = &'a dyn Fn(&[f64]) -> bool;

/// [`solve`] / [`degrees_of_freedom`] への入力。すべて借用で、ソルバーは書き換えない。
pub struct Problem<'a, E> {
    /// パラメータの初期値（有限であること）。
    pub params: &'a [f64],
    /// `true` のパラメータは固定する（図形の固定・ロックされたレイヤー・利用者が動かした
    /// パラメータ。設計判断3）。長さは `params` と同じ。
    pub fixed: &'a [bool],
    /// パラメータごとの重み（有限かつ正）。更新量の最小ノルムを `Σ (wⱼ Δxⱼ)²` で測る。
    /// 長さのパラメータは 1、角度のパラメータは円弧の半径（モジュール doc「解法」）。
    /// 長さは `params` と同じ。
    pub weights: &'a [f64],
    /// 方程式。並び順がそのまま処理の順になる。
    pub equations: &'a [E],
    /// 試行点 `x'` を受け入れてよいかの判定（`None` ならすべて受け入れる）。
    /// 線分の長さ・半径が [`ATOL_LEN`] を下回る形を棄却し、ヤコビアンの分母が 0 に
    /// 近づくのを防ぐ用途（設計判断1）。開始時の形には呼ばない。
    pub admissible: Option<Admissible<'a>>,
}

// 借用だけを持つので、`E: Clone` を求めずに複製できる（derive だと `E: Clone` が付く）。
impl<E> Clone for Problem<'_, E> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<E> Copy for Problem<'_, E> {}

impl<'a, E> Problem<'a, E> {
    /// フックなし（すべての試行点を受け入れる）の入力を作る。
    #[must_use]
    pub fn new(
        params: &'a [f64],
        fixed: &'a [bool],
        weights: &'a [f64],
        equations: &'a [E],
    ) -> Self {
        Self {
            params,
            fixed,
            weights,
            equations,
            admissible: None,
        }
    }

    /// 試行点を受け入れてよいかの判定を付ける。
    #[must_use]
    pub fn with_admissible(mut self, admissible: Admissible<'a>) -> Self {
        self.admissible = Some(admissible);
        self
    }
}

/// [`solve`] が満たした結果。
#[derive(Debug, Clone, PartialEq)]
pub struct Solution {
    /// 解いたパラメータ（入力と同じ長さ。固定したパラメータは入力の値のまま）。
    pub params: Vec<f64>,
    /// 目的関数 `F = ½ Σ (fᵢ / tol₀ᵢ)²` の値。先頭は開始時、以降は受理したステップごと
    /// （狭義単調減少）。
    pub objective: Vec<f64>,
    /// 試行回数（受理と棄却の合計）。開始時に満たされていれば 0。
    pub trials: usize,
}

/// [`solve`] などが解けなかった理由。
///
/// geom は GUI 非依存なので理由の列挙だけを返し、ユーザー向けの文言は持たない
/// （[`crate::FilletError`] などと同じ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SolveError {
    /// 入力の形が不正（呼び出し側の誤り）: `fixed`・`weights` の長さが `params` と違う、
    /// 方程式が範囲外または重複した添字を指す、重みが有限かつ正でない。
    InvalidInput,
    /// 開始時のパラメータ・残差・ヤコビアン・目的関数のいずれかが有限でない。
    NonFinite,
    /// `equation` 番目の方程式の許容値が有限かつ正にならない（尺度が有限かつ 0 以上でない）。
    InvalidTolerance {
        /// 方程式の添字。
        equation: usize,
    },
    /// 自由なパラメータ（固定されておらず、どれかの方程式が参照するもの）が無く、
    /// 満たされていない（固定された図形では拘束を満たせない）。反復せずに返す。
    NoFreeParameters,
    /// 減衰係数が [`LAMBDA_MAX`] を超えた（残差を減らす方向が無い: 解の無い系・局所解）。
    Stalled,
    /// 試行回数が [`MAX_TRIALS`] に達した。
    TrialLimit,
}

/// 方程式の系を解く（モジュール doc 参照）。
///
/// # Errors
///
/// [`SolveError`] の各場合。入力の `params` は書き換えない。
pub fn solve<E: Equation>(problem: &Problem<'_, E>) -> Result<Solution, SolveError> {
    validate(problem)?;
    let eqs = problem.equations;
    let m = eqs.len();
    let mut x = problem.params.to_vec();

    let mut f = vec![0.0; m];
    if !eval_residuals(eqs, &x, &mut f) {
        return Err(SolveError::NonFinite);
    }
    let tol0 = eval_tolerances(eqs, &x)?;
    let cols = Columns::new(problem);
    let n = cols.len();
    let mut grad = vec![0.0; max_arity(eqs)];
    let mut a = vec![0.0; m * n];
    if !eval_scaled_jacobian(eqs, &x, &tol0, problem.weights, &cols, &mut grad, &mut a) {
        return Err(SolveError::NonFinite);
    }
    let mut fx = objective(&f, &tol0);
    if !fx.is_finite() {
        return Err(SolveError::NonFinite);
    }
    let mut trace = vec![fx];

    // 開始時は tol₀ が今の形の許容値そのものなので、1 つの判定で両方を満たす。
    if max_ratio(&f, &tol0) <= 1.0 {
        return Ok(Solution {
            params: x,
            objective: trace,
            trials: 0,
        });
    }
    if n == 0 {
        return Err(SolveError::NoFreeParameters);
    }
    let mut aat = vec![0.0; m * m];
    gram(&a, m, n, &mut aat);
    if !all_finite(&aat) {
        return Err(SolveError::NonFinite);
    }

    let mut mu = LAMBDA_0;
    let mut trials = 0;
    let mut sys = vec![0.0; m * m];
    let mut z = vec![0.0; m];
    let mut x_try = x.clone();
    let mut f_try = vec![0.0; m];
    let mut a_try = vec![0.0; m * n];
    let mut aat_try = vec![0.0; m * m];
    loop {
        if trials >= MAX_TRIALS {
            return Err(SolveError::TrialLimit);
        }
        trials += 1;

        let lambda = mu * damping_unit(&aat, m);
        sys.copy_from_slice(&aat);
        for i in 0..m {
            sys[i * m + i] += lambda;
            z[i] = f[i] / tol0[i];
        }
        let mut accepted = gauss_solve(&mut sys, &mut z, m);
        if accepted {
            // Δy = −Aᵀ z、Δx = Δy / w。固定・未参照のパラメータは触らない。
            x_try.copy_from_slice(&x);
            for (c, &p) in cols.params.iter().enumerate() {
                let dy: f64 = (0..m).map(|i| a[i * n + c] * z[i]).sum();
                x_try[p] = x[p] - dy / problem.weights[p];
            }
            accepted = all_finite(&x_try)
                && problem.admissible.is_none_or(|ok| ok(&x_try))
                && eval_residuals(eqs, &x_try, &mut f_try);
        }
        let mut f_new = f64::INFINITY;
        if accepted {
            f_new = objective(&f_try, &tol0);
            accepted = f_new.is_finite()
                && f_new < fx
                && eval_scaled_jacobian(
                    eqs,
                    &x_try,
                    &tol0,
                    problem.weights,
                    &cols,
                    &mut grad,
                    &mut a_try,
                );
        }
        if accepted {
            gram(&a_try, m, n, &mut aat_try);
            accepted = all_finite(&aat_try);
        }

        if accepted {
            std::mem::swap(&mut x, &mut x_try);
            std::mem::swap(&mut f, &mut f_try);
            std::mem::swap(&mut a, &mut a_try);
            std::mem::swap(&mut aat, &mut aat_try);
            fx = f_new;
            trace.push(fx);
            mu = (mu / 10.0).max(LAMBDA_MIN);
            if max_ratio(&f, &tol0) <= 1.0 {
                let tol_now = eval_tolerances(eqs, &x)?;
                if max_ratio(&f, &tol_now) <= 1.0 {
                    return Ok(Solution {
                        params: x,
                        objective: trace,
                        trials,
                    });
                }
            }
        } else {
            mu *= 10.0;
            if mu > LAMBDA_MAX {
                return Err(SolveError::Stalled);
            }
        }
    }
}

/// `params` の形で各方程式を自分の許容値で割った残差の絶対値の最大（方程式が無ければ 0）。
///
/// 許容値は `params` の形から計算する。1 以下なら満たされている（[`is_satisfied`]）。
///
/// # Errors
///
/// 添字が範囲外・重複なら [`SolveError::InvalidInput`]、パラメータか残差が有限でなければ
/// [`SolveError::NonFinite`]、許容値が有限かつ正でなければ [`SolveError::InvalidTolerance`]。
pub fn max_scaled_residual<E: Equation>(
    params: &[f64],
    equations: &[E],
) -> Result<f64, SolveError> {
    validate_equations(params.len(), equations)?;
    if !all_finite(params) {
        return Err(SolveError::NonFinite);
    }
    let mut f = vec![0.0; equations.len()];
    if !eval_residuals(equations, params, &mut f) {
        return Err(SolveError::NonFinite);
    }
    let tol = eval_tolerances(equations, params)?;
    Ok(max_ratio(&f, &tol))
}

/// `params` の形がすべての方程式を満たすか（[`max_scaled_residual`] が 1 以下か）。
///
/// [`solve`] の成功判定・`.mcad` 読込時の充足検査（設計判断5）と同じ判定。
///
/// # Errors
///
/// [`max_scaled_residual`] と同じ。
pub fn is_satisfied<E: Equation>(params: &[f64], equations: &[E]) -> Result<bool, SolveError> {
    max_scaled_residual(params, equations).map(|r| r <= 1.0)
}

/// `problem.params` の形での自由度 = 固定していないパラメータの数 − ヤコビアンの階数。
///
/// 解いた後の形を渡す（設計判断1）。どの方程式にも参照されない自由なパラメータも
/// 自由度に数える（例: 水平拘束だけの線分は x 方向の 2 つが自由で、自由度 3）。
/// 階数は行・列のスケーリングで変わらないので、`weights` と許容値は使わない
/// （`weights` は [`solve`] と同じく検査だけする）。ヤコビアンの列と行を最大絶対値 1 に
/// 正規化し、完全ピボットのガウス消去で数える（閾値 [`RANK_RTOL`]）。`admissible` も使わない。
///
/// # Errors
///
/// 入力の形が不正なら [`SolveError::InvalidInput`]、パラメータかヤコビアンが有限でなければ
/// [`SolveError::NonFinite`]。
pub fn degrees_of_freedom<E: Equation>(problem: &Problem<'_, E>) -> Result<usize, SolveError> {
    validate(problem)?;
    let eqs = problem.equations;
    let m = eqs.len();
    let free = problem.fixed.iter().filter(|&&fixed| !fixed).count();
    let cols = Columns::new(problem);
    let n = cols.len();
    let mut grad = vec![0.0; max_arity(eqs)];
    let mut mat = vec![0.0; m * n];
    // 行・列の尺度は階数に効かないので、許容値でも重みでも割らない（`rank` で正規化する）。
    let row_ones = vec![1.0; m];
    let col_ones = vec![1.0; problem.params.len()];
    if !eval_scaled_jacobian(
        eqs,
        problem.params,
        &row_ones,
        &col_ones,
        &cols,
        &mut grad,
        &mut mat,
    ) {
        return Err(SolveError::NonFinite);
    }
    Ok(free - rank(&mut mat, m, n))
}

/// [`check_jacobian`] が見つけた、解析的な偏微分と中心差分の食い違い。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JacobianMismatch {
    /// パラメータの添字。
    pub param: usize,
    /// [`Equation::gradient`] の値。
    pub analytic: f64,
    /// 中心差分の値。
    pub numeric: f64,
}

/// 方程式の解析的ヤコビアンを中心差分と照合する（テスト用。設計判断1）。
///
/// `eq.params()` の各パラメータ `p` について、刻み `h = step × max(|x[p]|, 1)` の中心差分
/// `(f(x + h) − f(x − h)) / 2h` を計算し、解析値との差が
/// `tol × max(1, |解析値|, |差分値|)` を超えたら（どちらかが非有限でも）最初の食い違いを返す。
/// 刻みの誤差は `O(h² f‴) + O(ε |f| / h)` なので、座標が 1〜1000 程度の図形なら
/// `step = 1e-6`・`tol = 1e-5` 程度が目安。残差が不連続な点（角度の主値化の `±π`）や、
/// ノルムが 0 に近い点の近くでは食い違って当然なので避ける。
///
/// # Errors
///
/// 最初に食い違ったパラメータを [`JacobianMismatch`] で返す。
pub fn check_jacobian<E: Equation + ?Sized>(
    eq: &E,
    x: &[f64],
    step: f64,
    tol: f64,
) -> Result<(), JacobianMismatch> {
    let ps = eq.params();
    let mut grad = vec![0.0; ps.len()];
    eq.gradient(x, &mut grad);
    let mut xp = x.to_vec();
    for (&p, &analytic) in ps.iter().zip(&grad) {
        let orig = x[p];
        let h = step * orig.abs().max(1.0);
        xp[p] = orig + h;
        let hi = xp[p];
        let f_hi = eq.residual(&xp);
        xp[p] = orig - h;
        let lo = xp[p];
        let f_lo = eq.residual(&xp);
        xp[p] = orig;
        // 実際に表現された刻み（hi − lo）で割り、座標の丸めを差分へ持ち込まない。
        let numeric = (f_hi - f_lo) / (hi - lo);
        let diff = (analytic - numeric).abs();
        let bound = tol * analytic.abs().max(numeric.abs()).max(1.0);
        if !diff.is_finite() || diff > bound {
            return Err(JacobianMismatch {
                param: p,
                analytic,
                numeric,
            });
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 内部実装
// ---------------------------------------------------------------------------

/// 未知数にする列（自由で、どれかの方程式が参照するパラメータ。添字の昇順）。
struct Columns {
    /// 列 → パラメータの添字。
    params: Vec<usize>,
    /// パラメータの添字 → 列。
    col_of: Vec<Option<usize>>,
}

impl Columns {
    fn new<E: Equation>(problem: &Problem<'_, E>) -> Self {
        let mut used = vec![false; problem.params.len()];
        for eq in problem.equations {
            for &p in eq.params() {
                used[p] = true;
            }
        }
        let mut params = Vec::new();
        let mut col_of = vec![None; used.len()];
        for (p, &u) in used.iter().enumerate() {
            if u && !problem.fixed[p] {
                col_of[p] = Some(params.len());
                params.push(p);
            }
        }
        Self { params, col_of }
    }

    fn len(&self) -> usize {
        self.params.len()
    }
}

fn validate<E: Equation>(problem: &Problem<'_, E>) -> Result<(), SolveError> {
    let n = problem.params.len();
    if problem.fixed.len() != n || problem.weights.len() != n {
        return Err(SolveError::InvalidInput);
    }
    if !problem.weights.iter().all(|w| w.is_finite() && *w > 0.0) {
        return Err(SolveError::InvalidInput);
    }
    validate_equations(n, problem.equations)?;
    if !all_finite(problem.params) {
        return Err(SolveError::NonFinite);
    }
    Ok(())
}

fn validate_equations<E: Equation>(n: usize, equations: &[E]) -> Result<(), SolveError> {
    for eq in equations {
        let ps = eq.params();
        for (k, &p) in ps.iter().enumerate() {
            if p >= n || ps[..k].contains(&p) {
                return Err(SolveError::InvalidInput);
            }
        }
    }
    Ok(())
}

fn max_arity<E: Equation>(eqs: &[E]) -> usize {
    eqs.iter().map(|e| e.params().len()).max().unwrap_or(0)
}

fn all_finite(v: &[f64]) -> bool {
    v.iter().all(|x| x.is_finite())
}

/// 残差を `out` へ書き、すべて有限なら `true`。
fn eval_residuals<E: Equation>(eqs: &[E], x: &[f64], out: &mut [f64]) -> bool {
    for (o, eq) in out.iter_mut().zip(eqs) {
        *o = eq.residual(x);
    }
    all_finite(out)
}

fn eval_tolerances<E: Equation>(eqs: &[E], x: &[f64]) -> Result<Vec<f64>, SolveError> {
    eqs.iter()
        .enumerate()
        .map(|(i, eq)| {
            eq.kind(x)
                .tolerance()
                .ok_or(SolveError::InvalidTolerance { equation: i })
        })
        .collect()
}

/// 行を `row_scale` で、列を重みで割ったヤコビアンを `a`（`m × cols.len()`、行優先）へ書く。
/// 固定・未参照のパラメータの偏微分も有限性だけは確かめる。すべて有限なら `true`。
fn eval_scaled_jacobian<E: Equation>(
    eqs: &[E],
    x: &[f64],
    row_scale: &[f64],
    weights: &[f64],
    cols: &Columns,
    grad: &mut [f64],
    a: &mut [f64],
) -> bool {
    let n = cols.len();
    a.fill(0.0);
    for (i, eq) in eqs.iter().enumerate() {
        let ps = eq.params();
        let g = &mut grad[..ps.len()];
        g.fill(0.0);
        eq.gradient(x, g);
        if !all_finite(g) {
            return false;
        }
        for (&p, &gk) in ps.iter().zip(g.iter()) {
            if let Some(c) = cols.col_of[p] {
                a[i * n + c] = gk / (row_scale[i] * weights[p]);
            }
        }
    }
    all_finite(a)
}

/// `A Aᵀ`（`m × m`）を `out` へ書く。
fn gram(a: &[f64], m: usize, n: usize, out: &mut [f64]) {
    for i in 0..m {
        let ri = &a[i * n..(i + 1) * n];
        for j in 0..=i {
            let rj = &a[j * n..(j + 1) * n];
            let d: f64 = ri.iter().zip(rj).map(|(p, q)| p * q).sum();
            out[i * m + j] = d;
            out[j * m + i] = d;
        }
    }
}

/// 減衰係数の単位 `s = max diag(A Aᵀ)`。0（または非有限）なら 1。
fn damping_unit(aat: &[f64], m: usize) -> f64 {
    let s = (0..m).map(|i| aat[i * m + i]).fold(0.0, f64::max);
    if s > 0.0 && s.is_finite() { s } else { 1.0 }
}

fn objective(f: &[f64], tol: &[f64]) -> f64 {
    0.5 * f
        .iter()
        .zip(tol)
        .map(|(fi, ti)| (fi / ti) * (fi / ti))
        .sum::<f64>()
}

fn max_ratio(f: &[f64], tol: &[f64]) -> f64 {
    f.iter()
        .zip(tol)
        .map(|(fi, ti)| (fi / ti).abs())
        .fold(0.0, f64::max)
}

/// `a`（`n × n`、行優先）`· z = b` を部分ピボット付きのガウス消去で解き、`b` を解で
/// 上書きする。ピボットが 0・非有限、または解が非有限なら `false`。
fn gauss_solve(a: &mut [f64], b: &mut [f64], n: usize) -> bool {
    for k in 0..n {
        // 同じ大きさなら先の行を選ぶ（決定性）。
        let mut piv = k;
        let mut best = a[k * n + k].abs();
        for r in k + 1..n {
            let v = a[r * n + k].abs();
            if v > best {
                best = v;
                piv = r;
            }
        }
        if best == 0.0 || !best.is_finite() {
            return false;
        }
        if piv != k {
            for c in 0..n {
                a.swap(k * n + c, piv * n + c);
            }
            b.swap(k, piv);
        }
        let d = a[k * n + k];
        for r in k + 1..n {
            let factor = a[r * n + k] / d;
            if factor == 0.0 {
                continue;
            }
            a[r * n + k] = 0.0;
            for c in k + 1..n {
                a[r * n + c] -= factor * a[k * n + c];
            }
            b[r] -= factor * b[k];
        }
    }
    for k in (0..n).rev() {
        let tail: f64 = (k + 1..n).map(|c| a[k * n + c] * b[c]).sum();
        b[k] = (b[k] - tail) / a[k * n + k];
    }
    all_finite(b)
}

/// `a`（`rows × cols`、行優先）の階数。`a` は壊す。
///
/// 1. 各列を最大絶対値 1 に正規化する（ゼロ列はそのまま）。列のスケーリングは階数を
///    変えないので、列ごとに桁の違う偏微分（円弧の角度の列は半径に比例する）をそろえる。
/// 2. 続いて各行を最大絶対値 1 に正規化する（行の尺度も階数を変えない）。
/// 3. 完全ピボット（残った部分行列の最大絶対値の要素を行・列の入れ替えでピボットに置く）の
///    ガウス消去を行い、次のピボットがそれまでの最大ピボットの [`RANK_RTOL`] 倍以下に
///    なったら打ち切る。同じ大きさの候補は、行優先で先に見つかった要素を選ぶ（決定性）。
fn rank(a: &mut [f64], rows: usize, cols: usize) -> usize {
    for c in 0..cols {
        let mx = (0..rows).fold(0.0, |acc: f64, r| acc.max(a[r * cols + c].abs()));
        if mx > 0.0 {
            for r in 0..rows {
                a[r * cols + c] /= mx;
            }
        }
    }
    for r in 0..rows {
        let row = &mut a[r * cols..(r + 1) * cols];
        let mx = row.iter().fold(0.0, |acc: f64, v| acc.max(v.abs()));
        if mx > 0.0 {
            for v in row.iter_mut() {
                *v /= mx;
            }
        }
    }
    let mut rank = 0;
    let mut max_pivot = 0.0f64;
    while rank < rows.min(cols) {
        let (mut pr, mut pc, mut best) = (rank, rank, 0.0f64);
        for r in rank..rows {
            for c in rank..cols {
                let v = a[r * cols + c].abs();
                if v > best {
                    (pr, pc, best) = (r, c, v);
                }
            }
        }
        max_pivot = max_pivot.max(best);
        if best == 0.0 || best <= RANK_RTOL * max_pivot {
            break;
        }
        if pr != rank {
            for c in 0..cols {
                a.swap(rank * cols + c, pr * cols + c);
            }
        }
        if pc != rank {
            for r in 0..rows {
                a.swap(r * cols + rank, r * cols + pc);
            }
        }
        let d = a[rank * cols + rank];
        for r in rank + 1..rows {
            let factor = a[r * cols + rank] / d;
            if factor == 0.0 {
                continue;
            }
            a[r * cols + rank] = 0.0;
            for c in rank + 1..cols {
                a[r * cols + c] -= factor * a[rank * cols + c];
            }
        }
        rank += 1;
    }
    rank
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gauss_solve_needs_pivoting() {
        // 先頭のピボットが 0 の系。部分ピボットが無いと解けない。
        let mut a = vec![0.0, 2.0, 1.0, 1.0, 1.0, 0.0, 2.0, 0.0, 3.0];
        let mut b = vec![7.0, 3.0, 11.0];
        assert!(gauss_solve(&mut a, &mut b, 3));
        // 解は (1, 2, 3)
        for (v, want) in b.iter().zip([1.0, 2.0, 3.0]) {
            assert!((v - want).abs() < 1e-12, "{b:?}");
        }
    }

    #[test]
    fn gauss_solve_rejects_singular() {
        let mut a = vec![1.0, 2.0, 2.0, 4.0];
        let mut b = vec![1.0, 2.0];
        assert!(!gauss_solve(&mut a, &mut b, 2));
    }

    #[test]
    fn rank_counts_independent_rows() {
        // 3 行目 = 1 行目 + 2 行目、4 行目 = 1 行目 × 1e6（行の尺度は階数に効かない）。
        let mut a = vec![
            1.0, 0.0, 2.0, //
            0.0, 1.0, -1.0, //
            1.0, 1.0, 1.0, //
            1e6, 0.0, 2e6,
        ];
        assert_eq!(rank(&mut a, 4, 3), 2);
        let mut id = vec![1.0, 0.0, 0.0, 1e-3];
        assert_eq!(rank(&mut id, 2, 2), 2);
        let mut zero = vec![0.0; 6];
        assert_eq!(rank(&mut zero, 2, 3), 0);
        assert_eq!(rank(&mut [], 0, 0), 0);
        // 先頭の列がゼロ列でも、後ろの列で数える（完全ピボット）。
        let mut shifted = vec![0.0, 0.0, 1.0, 0.0, 1.0, 0.0];
        assert_eq!(rank(&mut shifted, 2, 3), 2);
    }

    /// 列のスケーリングで階数が変わらない（Codex adversarial review の指摘: 列を重みで
    /// 割ってから行だけを正規化し、絶対閾値で判定していた旧実装は、独立な行
    /// [1, 1]・[1, 1 + δ] の列に大きく違う倍率が掛かると階数 1 と誤判定した）。
    #[test]
    fn rank_is_invariant_under_column_scaling() {
        let scales = [1e-6, 1e-3, 1.0, 1e3, 1e6];
        for (delta, want) in [(1e-4, 2), (1e-6, 2), (1e-15, 1), (0.0, 1)] {
            for &s0 in &scales {
                for &s1 in &scales {
                    let mut a = vec![s0, s1, s0, s1 * (1.0 + delta)];
                    assert_eq!(rank(&mut a, 2, 2), want, "delta={delta} s=({s0}, {s1})");
                }
            }
        }
        // 行の尺度・行の順序にも依らない。
        let mut a = vec![1e-6, 1e-6 * (1.0 + 1e-6), 1e6, 1e6];
        assert_eq!(rank(&mut a, 2, 2), 2);
    }

    #[test]
    fn tolerance_formula() {
        let len = |size, coord| ResidualKind::Length { size, coord }.tolerance();
        assert_eq!(len(10.0, 0.0), Some(1e-8));
        assert_eq!(len(0.0, 0.0), Some(ATOL_LEN));
        // 座標の項は 1e6 mm でも 1e-9 mm 台で、代表長さ 10 mm の相対項を超えない。
        let far = len(0.0, 1e6).unwrap();
        assert!(far > 1e-9 && far < 1e-8, "{far}");
        assert_eq!(len(10.0, 1e6), Some(1e-8));
        assert_eq!(len(f64::NAN, 0.0), None);
        assert_eq!(len(1.0, f64::INFINITY), None);
        assert_eq!(len(-1.0, 0.0), None);
        assert_eq!(ResidualKind::Angle.tolerance(), Some(ATOL_ANG));
    }
}
