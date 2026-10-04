//! ドキュメントを変更する唯一の手段である [`Command`]。

use crate::{
    Constraint, ConstraintId, DimStyle, Entity, EntityGeom, EntityId, Layer, LayerId, SheetMeta,
    Style,
};

/// ドキュメントへの変更を表すコマンド。
///
/// [`crate::Document`] の状態変更はすべて [`crate::Document::apply`] にこの型を
/// 渡す形で行う（フィールドを直接触らせない）。`apply` はコマンドを実行しつつ、
/// 逆操作に必要な情報を内部の undo 履歴へ積む。
///
/// # 「ツール操作1回 = 1コマンド」
///
/// ドラッグなどの連続操作は、確定時の最終状態を 1 コマンドとして表現する。
/// たとえば移動は「移動後の幾何」を持つ [`Command::ModifyEntity`] 1 回で表し、
/// ドラッグ中の中間状態は履歴へ積まない。[`Command::ModifyEntity`] が変更前の
/// 幾何を保持しないのはこのためで、`apply` が実行時に現在の幾何を記録して逆操作を
/// 構成する（DESIGN 3.2 の `Modify { before, after }` の before は内部生成する）。
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// エンティティを追加する。
    AddEntity(Entity),

    /// エンティティを削除する。
    ///
    /// そのエンティティを参照する拘束（M13）もすべて同じ undo 単位の中で消す
    /// （undo でエンティティと一緒に同じ [`ConstraintId`] へ戻る）。
    RemoveEntity(EntityId),

    /// エンティティの幾何を差し替える（移動・変形などの確定操作）。
    ///
    /// 拘束（M13）は残す。図形の固定（[`Constraint::FixEntity`]）がついた図形の幾何を
    /// 変える場合は [`crate::CoreError::EntityFixed`]、幾何の種類が変わって既存の拘束が
    /// 指せなくなる場合は [`crate::CoreError::InvalidConstraint`] で拒否する。変更後の形が
    /// 拘束を満たさなければ、[`crate::Document::apply`] が**このコマンドで動かした点・半径・
    /// 角度を止めたまま**ほかを解いて追従させる（同じ undo 単位。`apply` の doc「拘束の求解」）。
    /// 線分の片方の端点だけを変えれば、もう片方は自由なまま（グリップ編集）。
    /// 切り詰め・延長には [`Command::ReshapeEntity`] を使う。
    ModifyEntity {
        /// 対象エンティティ。
        id: EntityId,
        /// 差し替え後の幾何。
        new_geom: EntityGeom,
    },

    /// エンティティの形を**切り詰める・延長する**（1 断片のトリム・延長・フィレット・面取り。
    /// M13 タスク96、DESIGN.md M13 設計判断4）。
    ///
    /// 幾何の差し替えという点は [`Command::ModifyEntity`] と同じで、検証（幾何の妥当性・
    /// 編集可能・部品表）も同じ。違いは拘束の扱い: `ModifyEntity` は「動かす・変形する」の
    /// 意味で拘束を残す（残した拘束に反すれば拒否）のに対し、`ReshapeEntity` は切り詰めで
    /// 意味を失う拘束を**同じ undo 単位の中で消す**。
    ///
    /// **対象を参照する拘束が 1 件でもあるときは、支持する直線・円が変わらない変更だけを
    /// 受け付ける**: 線分 → 線分は新しい両端点が元の線分を延長した直線の上にあり、向きが
    /// 反転しないこと。円 → 円弧・円弧 → 円弧は中心と半径が変わらないこと。それ以外
    /// （平行移動・回転・向きの反転・中心や半径の変更・円弧 → 円・種類の変わる変更）は
    /// [`crate::CoreError::InvalidConstraint`] で拒否し、拘束は消さない（動かすには
    /// `ModifyEntity` を使う）。受け付けた変更では:
    ///
    /// - 位置が変わった点（`Start`・`End`・`Center` のうち長さ型の許容値を超えて動いたもの）を
    ///   指す一致・距離は消す。
    /// - 端点が動いた線分を含む等長は消す。
    /// - 向き・無限直線・半径に依存する拘束（水平・垂直・平行・直交・角度・点と線の距離の
    ///   「線」側・接線・半径・等半径）は残す。
    /// - 図形の固定がついた図形、動いた点に点の固定がある図形は
    ///   [`crate::CoreError::EntityFixed`] で拒否する。
    ///
    /// 残った拘束は [`crate::Document::apply`] の最後の求解に掛かる（支持する直線・円が
    /// 変わらないので、ふつうは満たされたままで何も動かない）。拘束の無い
    /// エンティティに対しては `ModifyEntity` と同じ結果になる。同一幾何への差し替えは
    /// 意味的 no-op（拘束も消さない）。
    ReshapeEntity {
        /// 対象エンティティ。
        id: EntityId,
        /// 切り詰め・延長後の幾何。
        new_geom: EntityGeom,
    },

    /// エンティティの描画スタイル（色・線幅・線種の個別上書き）を差し替える
    /// （M8 タスク36。[`Command::SetLayerProps`] と同じ「まとめて差し替え」流儀。
    /// フィールドを `None` に戻すと ByLayer へ復帰する）。
    SetEntityStyle {
        /// 対象エンティティ。
        id: EntityId,
        /// 差し替え後のスタイル一式。
        style: Style,
    },

    /// エンティティの所属レイヤーを差し替える（M9 タスク52。[`Command::SetEntityStyle`]
    /// と同じ「まとめて差し替え」流儀）。
    ///
    /// 移動元エンティティは [`crate::Document::require_editable_entity`]（存在・ロック）、
    /// 移動先レイヤーは [`crate::Document::require_editable_layer`]（存在・ロック。
    /// [`Command::AddEntity`] と対称）の両方を満たす必要がある。同一レイヤーへの
    /// 差し替えは意味的 no-op（履歴を汚さない）。
    SetEntityLayer {
        /// 対象エンティティ。
        id: EntityId,
        /// 差し替え後の所属レイヤー。
        layer: LayerId,
    },

    /// レイヤーを追加する。
    AddLayer(Layer),

    /// レイヤーを削除する。
    ///
    /// デフォルトレイヤー・カレントレイヤー・非空レイヤーは削除できず、
    /// それぞれ [`crate::CoreError`] を返す。
    RemoveLayer(LayerId),

    /// レイヤーのプロパティ（名前・色・表示・ロック）をまとめて差し替える。
    SetLayerProps {
        /// 対象レイヤー。
        id: LayerId,
        /// 差し替え後のプロパティ一式。
        props: Layer,
    },

    /// カレントレイヤーを切り替える。
    SetCurrentLayer(LayerId),

    /// 図面メタデータ（尺度・用紙・向き・表題欄の様式と記入内容・枠表示）を
    /// まとめて差し替える。
    ///
    /// 尺度・用紙・表題欄の記入は「図面の内容」であって UI の表示設定ではないため、
    /// 他の内容変更と同じく undo 対象のコマンドにする（DESIGN.md M8 設計判断2）。
    /// no-op 判定（`before == after` なら履歴を汚さない）は
    /// [`Command::SetLayerProps`] と同じ流儀。
    ///
    /// 不正な尺度・線幅は [`crate::Scale`] / [`crate::WidthMm`] の型で構築時に
    /// 弾かれるため、このコマンドに載ることはない。実行前に検証するのは
    /// ユーザー定義の表題欄様式の寸法のみ（[`crate::SheetMeta::validate`]）。
    SetSheet(SheetMeta),

    /// 文書単位の寸法スタイル（文字高さ・矢先長さ・桁数など）をまとめて差し替える
    /// （M9 タスク47-3。DESIGN.md M9 設計判断4）。
    ///
    /// 名前付き複数スタイルは non-goal で、図面に 1 つだけ持つ。[`Command::SetSheet`] と
    /// 同じ流儀（no-op 判定は `before == after` なら履歴を汚さない。実行前チェックは
    /// [`crate::DimStyle::validate`]）。
    ///
    /// 既存の寸法エンティティが持つ `decimals_override: None` の桁数は、この文書スタイルへの
    /// **生きた参照**（[`crate::DimStyle::resolve_decimals`]）なので、このコマンドを適用すると
    /// 即座に反映される。`Some(n)` の明示上書きを持つ寸法は影響を受けない（DESIGN.md M9
    /// 設計判断4 実装時追記）。このコマンドはエンティティの `annotation` 自体には一切触れない。
    SetDimStyle(DimStyle),

    /// 幾何拘束を追加する（M13 タスク96・97、DESIGN.md M13 設計判断2・3）。
    ///
    /// [`Constraint`] は**完全な値**（側の符号・外接/内接・固定座標・角度を含む）を載せる。
    /// 現在の形からそれらを決めるには [`Constraint`] のコンストラクタ
    /// （[`Constraint::tangent_line_circle`] など）を使う。`.mcad` 読込も同じコマンドを使う。
    ///
    /// 実行前に構造を検証する（参照先の存在と種類、同じ対象の二重指定、値の範囲。
    /// 違反は [`crate::CoreError::InvalidConstraint`] / [`crate::CoreError::EntityNotFound`]）。
    /// 角度の値は `(−π, π]` へ正規化して格納する。レイヤーのロックは見ない（拘束の追加
    /// そのものはエンティティを変えない。ロックされたレイヤーの図形は、求解でも動かない）。
    ///
    /// **現在の形で満たされていなければ、[`crate::Document::apply`] が解いて図形を動かす**
    /// （同じ undo 単位）。満たされていれば何も動かさない。満たす形が無い（ほかの拘束と
    /// 矛盾する）追加は拒否する。退化した図形（長さが `ATOL_LEN` 未満の線分、半径が
    /// `ATOL_LEN` 未満の円・円弧）を参照する拘束は [`crate::CoreError::DegenerateShape`] で
    /// 拒否する。
    ///
    /// # `hold`: 止めておく図形
    ///
    /// 「拘束を付けたときに動くのは後に選んだ図形」（M13 ユーザー確定事項）のための指定。
    /// `Some(e)` なら `e`（先に選んだ図形）の全パラメータを止めて解き、**それで満たせない
    /// ときに限り**、止めずに解き直す（返るエラーは解き直しのもの）。`None` は止める図形なしで、
    /// 動きが最小になる解（最小ノルム）を選ぶ。`e` はこの拘束の対象でなければならない
    /// （違えば [`crate::CoreError::InvalidConstraint`]）。1 つの [`Command::Batch`] に複数の
    /// `AddConstraint` があるときは、拘束でつながった図形のまとまり（連結成分）ごとに、
    /// その中の `hold` をまとめて止めて 1 回解き、満たせなければまとめて外して解き直す。
    AddConstraint {
        /// 追加する拘束。
        constraint: Constraint,
        /// 止めておく図形（先に選んだ図形）。`None` なら止めない。
        hold: Option<EntityId>,
    },

    /// 幾何拘束を削除する。レイヤーのロックは見ない。
    RemoveConstraint(ConstraintId),

    /// 拘束の値（点どうしの距離・点と線の距離・半径・角度）を変える。
    ///
    /// 値を持たない拘束に対しては [`crate::CoreError::InvalidConstraint`]。値の範囲の検証は
    /// [`Command::AddConstraint`] と同じ（点どうしの距離は長さ型の許容値より大きい、
    /// 点と線の距離は 0 以上、半径は正、角度は `(−π, π]` へ正規化）。同じ値の設定は
    /// 意味的 no-op。現在の形が新しい値を満たさなければ、[`crate::Document::apply`] が
    /// 解いて形を変える（同じ undo 単位。止める図形は足さず、動きが最小になる解を選ぶ）。
    SetConstraintValue {
        /// 対象の拘束。
        id: ConstraintId,
        /// 新しい値（長さはワールド単位、角度はラジアン）。
        value: f64,
    },

    /// 既存の拘束を、**同じ [`ConstraintId`] のまま**別の拘束へ差し替える（M13 タスク97）。
    ///
    /// ミラーのように、図形を動かすと拘束の中身（側の符号・角度の符号・円弧の端点の指し方）も
    /// 変えなければ同じ幾何関係を表せない操作のためのコマンド。図形の `ModifyEntity` と同じ
    /// [`Command::Batch`] に入れて使う（差し替える拘束は [`Constraint::mirrored`] で作る）。
    ///
    /// 検証は [`Command::AddConstraint`] と同じ（構造の検証と、角度の正規化）。対象の
    /// エンティティ・拘束の種類が変わる差し替えも受け付ける。同じ値への差し替えは意味的
    /// no-op。存在しない ID は [`crate::CoreError::ConstraintNotFound`]。レイヤーのロックは
    /// 見ない。
    ///
    /// 差し替えた拘束が現在の形で満たされていなければ、[`crate::Document::apply`] が解く
    /// （止める図形は足さない。[`Command::SetConstraintValue`] と同じ）。
    ReplaceConstraint {
        /// 対象の拘束。
        id: ConstraintId,
        /// 差し替え後の拘束。
        constraint: Constraint,
    },

    /// 複数のサブコマンドを **1 つの操作** としてまとめて適用する複合コマンド。
    ///
    /// 矩形選択で複数エンティティをまとめて移動する、といった「ツール操作1回」で
    /// 複数の変更が生じるケースを、undo/redo の 1 単位として扱うためのもの
    /// （DESIGN 3.2「ツール操作1回 = 1コマンド」）。これがないと対象数だけ
    /// [`Command::ModifyEntity`] を個別に `apply` することになり、Ctrl+Z 1 回で
    /// 最後の 1 件しか戻らない不整合が起きる。
    ///
    /// # 原子性
    ///
    /// サブコマンドを先頭から順に検証・実行し、途中で 1 つでも失敗したら、それまでに
    /// 適用した分をすべて巻き戻して呼び出し前の状態へ完全復帰させたうえで
    /// [`crate::CoreError`] を返す（部分適用ゼロをバッチ全体へ拡張）。
    ///
    /// # undo/redo
    ///
    /// バッチは undo 履歴へ **1 記録** として積まれる。undo はサブコマンドの逆操作を
    /// 逆順に、redo は正順に適用するので、Ctrl+Z 1 回でバッチ全体が戻る。
    ///
    /// # 空バッチと意味的no-op
    ///
    /// 空の [`Command::Batch`] や、サブコマンドがすべて意味的 no-op（ネストした空
    /// [`Command::Batch`] を含む）であるバッチは「操作なし」として扱われる。
    /// [`crate::Document::apply`] はこの場合 `Ok(`[`crate::NewIds`]`::default())` を
    /// 返し、undo/redo 履歴には一切触れない。
    ///
    /// これは `Batch` に固有の扱いではなく、状態を変えないコマンド全般（例: 現在と
    /// 同じ幾何・プロパティ・レイヤーを指定する `ModifyEntity`/`SetLayerProps`/
    /// `SetCurrentLayer`）に共通する。判定基準の詳細は [`crate::Document::apply`] を参照。
    ///
    /// サブコマンドに [`Command::Batch`] を含めるネストも `apply` が再帰的に扱うため
    /// 破綻しないが、MVP で意図しているのはフラットな複数操作を 1 単位にすることである。
    Batch(Vec<Command>),
}
