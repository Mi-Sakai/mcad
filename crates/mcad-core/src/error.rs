//! コア操作のエラー型。

use crate::{ConstraintId, EntityId, LayerId};

/// [`crate::Document::apply`] などコア操作が返しうるエラー。
///
/// これらのエラーが返るとき、ドキュメントの状態は **変更されない**
/// （部分適用による不整合状態を作らない）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoreError {
    /// 指定した [`EntityId`] のエンティティが存在しない（または既に削除済み）。
    #[error("entity not found: {0:?}")]
    EntityNotFound(EntityId),

    /// 指定した [`LayerId`] のレイヤーが存在しない（または既に削除済み）。
    #[error("layer not found: {0:?}")]
    LayerNotFound(LayerId),

    /// デフォルトレイヤー（レイヤー0相当）は削除できない。
    #[error("the default layer cannot be deleted")]
    CannotDeleteDefaultLayer,

    /// カレントレイヤーは削除できない（先に別レイヤーへ切り替える必要がある）。
    #[error("the current layer cannot be deleted")]
    CannotDeleteCurrentLayer,

    /// エンティティを保持しているレイヤーは削除できない。
    ///
    /// MVP ではカスケード削除（レイヤーとその中身を同時に消す）を行わず、
    /// 宙に浮いた `layer` 参照を防ぐために非空レイヤーの削除を禁止する。
    #[error("layer is not empty and cannot be deleted: {0:?}")]
    LayerNotEmpty(LayerId),

    /// 対象エンティティが所属するレイヤーがロック（[`crate::Layer::locked`]）されているため、
    /// このエンティティへの変更・削除はできない。
    #[error("layer is locked: {0:?}")]
    LayerLocked(LayerId),

    /// ジオメトリが不正（NaN/∞座標、負半径など）で追加・変更を拒否した。
    ///
    /// 判定条件は [`mcad_geom::Shape::validate`]（DESIGN.md M4 タスク15）。
    /// `Command::AddEntity` / `Command::ModifyEntity` の実行前チェックで返る。
    #[error("invalid geometry: {0}")]
    InvalidGeometry(String),

    /// 図面尺度が不正（`num`/`den` に 0 を含む、上限超過）で拒否した。
    ///
    /// 判定条件は [`crate::Scale::new`]（DESIGN.md M8 設計判断2）。
    #[error("invalid scale: {0}")]
    InvalidScale(String),

    /// 線幅が不正（0・負・非有限・許容範囲外）で拒否した。
    ///
    /// 判定条件は [`crate::WidthMm::new`]（DESIGN.md M8 設計判断5）。
    #[error("invalid line width: {0}")]
    InvalidLineWidth(String),

    /// はめあい記号が不正（空・組合せ表記 `"H7/g6"`・等級の欠落など）で拒否した。
    ///
    /// 判定条件は [`crate::FitClass::new`]（DESIGN.md M9 設計判断2）。
    #[error("invalid fit class: {0}")]
    InvalidFitClass(String),

    /// 寸法注記が不正（種別に許されない寸法補助記号、`upper < lower` の公差、非有限値、
    /// 桁数上書きの上限超過など）で拒否した。
    ///
    /// 判定条件は [`crate::DimAnnotation::validate`] と [`crate::SizeTolerance::validate`]
    /// （DESIGN.md M9 設計判断2）。
    #[error("invalid dimension annotation: {0}")]
    InvalidDimAnnotation(String),

    /// 図面メタデータが不正（ユーザー定義の表題欄様式の寸法が非有限・非正など）で
    /// 拒否した。
    ///
    /// 判定条件は [`crate::SheetMeta::validate`]。`Command::SetSheet` の実行前
    /// チェックで返る。
    #[error("invalid sheet metadata: {0}")]
    InvalidSheet(String),

    /// 寸法スタイルが不正（非有限・非正の紙 mm 値、桁数上限超過、公差文字縮小率が
    /// `(0, 1]` の範囲外など）で拒否した。
    ///
    /// 判定条件は [`crate::DimStyle::validate`]（DESIGN.md M9 設計判断4）。
    /// `Command::SetDimStyle` の実行前チェックで返る。
    #[error("invalid dimension style: {0}")]
    InvalidDimStyle(String),

    /// 図面には既に部品表（[`crate::TableKind::Bom`]）があるため、2 つ目の部品表を作る
    /// 操作を拒否した（M12 タスク81、DESIGN.md M12 設計判断1「部品表は 1 図面に 1 つ」）。
    ///
    /// 値は**既存の**部品表の [`EntityId`]（呼び出し側がそれを示せるように）。
    /// `Command::AddEntity` / `Command::ModifyEntity`（`Command::Batch` 内を含む）の
    /// 実行前チェックで返る。削除済み（墓標）の部品表は数えない。
    #[error("the drawing already has a bill of materials: {0:?}")]
    BomAlreadyExists(EntityId),

    /// 指定した [`ConstraintId`] の拘束が存在しない（または既に削除済み）。
    #[error("constraint not found: {0:?}")]
    ConstraintNotFound(ConstraintId),

    /// 拘束の構造・値が不正で拒否した（M13 タスク96、DESIGN.md M13 設計判断2）。
    ///
    /// 参照先の種類が合わない（線分専用の拘束が円を指す、円に `Start` を指定するなど）、
    /// 同じ対象を 2 回指す、値が非有限・範囲外（点どうしの距離が長さ型の許容値以下、
    /// 半径が 0 以下など）、等半径の内接、値を持たない拘束への
    /// [`crate::Command::SetConstraintValue`]、[`crate::Command::ModifyEntity`] で幾何の
    /// 種類が変わり既存の拘束が指せなくなる場合、拘束のついた図形への
    /// [`crate::Command::ReshapeEntity`] が支持する直線・円を変える（切り詰め・延長でない）
    /// 場合に返る。
    #[error("invalid constraint: {0}")]
    InvalidConstraint(String),

    /// コマンドを適用した後の形から拘束を解いたが、満たす形が見つからなかったため、コマンド
    /// 全体を拒否した（M13 タスク97、DESIGN.md M13 設計判断3）。矛盾する拘束の追加、
    /// 拘束どうしが両立しない値への変更、動かした図形に他の図形が追従しきれない編集で返る。
    ///
    /// 値は、解く前（コマンドを適用した直後）の形で満たされていなかった拘束のうち
    /// [`ConstraintId`] 順で最初のもの（決定的。矛盾の原因とは限らない）。拒否された
    /// [`crate::Command::AddConstraint`] が発行するはずだった ID を指すことがあり、その ID は
    /// 文書に残らない（[`crate::Document::constraint`] では引けない）。
    #[error("no shape satisfies the constraints (first unsatisfied: {0:?})")]
    ConstraintUnsolvable(ConstraintId),

    /// 拘束が満たされておらず、その拘束が参照するパラメータがすべて止められているため、
    /// 解かずにコマンド全体を拒否した。値はその拘束（[`ConstraintId`] 順で最初のもの）。
    ///
    /// 止められているのは、図形の固定（[`crate::Constraint::FixEntity`]）の対象、ロックされた
    /// レイヤー上の図形、そのコマンドで利用者が動かした点・半径・角度。水平拘束のついた線分を
    /// 単独で回転する、点の固定がついた点を動かす、ロックされたレイヤーの図形と一致で
    /// つながった図形をまるごと動かす、といった「動かした図形そのものが拘束に反する」編集で
    /// 返る。
    #[error("the constraint cannot be satisfied because everything it refers to is held: {0:?}")]
    ConstraintBlockedByFixed(ConstraintId),

    /// 拘束を満たそうとすると図形が退化するため、コマンド全体を拒否した。値はその図形
    /// （[`EntityId`] 順で最初のもの）。
    ///
    /// 退化とは: 線分の長さ・円と円弧の半径がソルバーの扱える下限 `ATOL_LEN` 未満、または
    /// それを参照する長さ型の拘束の許容値以下へ落ちる（線分が 1 点につぶれる）、円弧の
    /// 掃引角が 0 以下・2π 以上へ達する。拘束が参照する図形が解く前から退化している場合
    /// （長さ 0 の線分への拘束の追加、拘束のついた線分を 1 点へつぶす
    /// [`crate::Command::ModifyEntity`]）もこれを返す。
    #[error("satisfying the constraints would degenerate the shape: {0:?}")]
    DegenerateShape(EntityId),

    /// コマンドを適用した後（解いた場合は解いた後）の形で、拘束の構造条件が崩れるため、
    /// コマンド全体を拒否した。値はその拘束（[`ConstraintId`] 順で最初のもの）。
    ///
    /// 構造条件は [`crate::Command::AddConstraint`] が追加の前に確かめるものと同じ: 内接の
    /// 2 円の半径の差 `s(r1 − r2)` が長さ型の許容値より大きい（同心・等半径になる変更を
    /// 拒否する）、点どうしの距離の値が許容値より大きい（図形が大きくなって許容値が値を
    /// 超える変更を拒否する）。
    #[error("the constraint degenerates in the resulting shape: {0:?}")]
    DegenerateConstraint(ConstraintId),

    /// 拘束でつながった図形のまとまりが大きすぎて、演算量の予算の範囲で解き切れなかったため、
    /// コマンド全体を拒否した（M13 タスク101。解の有無は分からない）。予算は演算回数で数えるので、
    /// 同じ編集なら機械の速さに関わらず同じ結果になる。
    #[error("the connected constraints are too large to solve within the work budget")]
    ConstraintBudgetExceeded,

    /// 固定の拘束（図形の固定、または動かそうとした点の固定）がついた図形の幾何を
    /// 変える操作を拒否した。値は対象のエンティティ。
    ///
    /// 図形の固定（[`crate::Constraint::FixEntity`]）がついた図形への
    /// [`crate::Command::ModifyEntity`] / [`crate::Command::ReshapeEntity`]、および
    /// 点の固定（[`crate::Constraint::FixPoint`]）がついた点を動かす
    /// [`crate::Command::ReshapeEntity`] で返る。
    #[error("entity is fixed by a constraint: {0:?}")]
    EntityFixed(EntityId),
}
