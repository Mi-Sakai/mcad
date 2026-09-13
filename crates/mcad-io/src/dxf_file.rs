//! DXF（Drawing Exchange Format）の export/import。
//!
//! # 設計方針（DESIGN.md 3.3）
//!
//! `dxf` クレート（v0.6.1）で LINE / CIRCLE / ARC / LWPOLYLINE / POINT / TEXT と
//! レイヤーテーブルを相互変換する。未対応エンティティは読み飛ばし、無視した件数を
//! [`ImportSummary::skipped_entities`] として返す（エラーにしない）。
//!
//! TEXT（[`mcad_core::EntityGeom::Text`]）は M6 で export（タスク25）・import
//! （タスク25b。当初 M9 予定だったが 2026-07-25 のユーザー判断で M6 へ前倒し）の
//! 双方に対応した。
//!
//! 寸法は **import のみ M11 タスク67 で対応**した（下の「DIMENSION の import」）。
//! **export は未対応**で、[`ExportSummary::skipped_entities`] に計上される
//! （M11 タスク72 で「寸法線・補助線・矢先・文字へ分解して `LINE`/`TEXT` として
//! 書く」方式になる予定。DESIGN.md M11 設計判断2。表と同じ非対称往復になる）。
//!
//! 表（[`mcad_core::EntityGeom::Table`]、M10）は**非対称往復**の best-effort で
//! 対応する（DESIGN.md M10 設計方針6・詳細設計8、タスク61）:
//!
//! - **export**: [`table_to_dxf_entities`] が罫線を `LINE`、非空セル文字を `TEXT`
//!   へ分解して書く。組版は `mcad-core` の [`mcad_core::expand_table`]（画面・SVG/PDF と
//!   同じ唯一の出所。M11 タスク71 で core へ移すまでは依存方向の制約から `mcad-io` 側に
//!   独立実装していた）。分解できるため `skipped_entities` には計上しない
//!   （Text と同じ「対応済み」扱い）。
//! - **import**: DXF に「表」に相当するプリミティブは無いので、分解後の
//!   `LINE`/`TEXT` を import しても [`mcad_core::EntityGeom::Table`] へは戻らず、
//!   ただの直線・文字の集合として復元される。**表として export したファイルを
//!   import しても表エンティティには戻らない**（往復非対称。AGENTS.md にも記録）。
//! - 他 CAD が書いた `ACAD_TABLE` 系のテーブルオブジェクトは、`dxf` 0.6.1 の
//!   `EntityType` にテーブル系のバリアントが無いため、そもそも `drawing.entities()`
//!   に現れず読み飛ばされる（実測、詳細は
//!   `tests::import_ignores_acad_table_and_still_reads_other_entities` の doc）。
//!   したがってこの経路は `ImportSummary::skipped_entities` を増やさない
//!   （SPLINE・ELLIPSE 等、`EntityType` にバリアントはあるが `dxf_entity_to_geom`
//!   が変換しないものは通常どおり計上される、という既存の区別と異なる）。
//!
//! **寸法の非対称は表と向きが逆**で、現状は import だけができる（M11 タスク67）。
//! タスク72 で分解 export が入ると表と同じ向きの非対称（書けるが読み戻せない）にも
//! なるため、そのときは「mcad が書いた寸法は読み戻せないが、他 CAD が書いた
//! `DIMENSION` は読める」という二重の非対称になる。
//!
//! # DIMENSION の import（M11 タスク67）
//!
//! 他 CAD が書いた寸法を [`mcad_core::EntityGeom`] の `DimLinear` / `DimRadial` /
//! `DimDiameter` へ写す（**export は未対応**。上記「設計方針」参照）。対応表は次のとおりで、
//! 「クレートの型」は `dxf` 0.6.1 が group 100 のサブクラス名で選ぶ `EntityType`。
//!
//! | DXF 種別（group 70 の整数部） | クレートの型 | mcad | 備考 |
//! |---|---|---|---|
//! | 整列 = 1 | `RotatedDimension` | `DimLinear` | 13/14 = 計測 2 点、10 = 寸法線の位置 |
//! | 回転 = 0 | `RotatedDimension` | `DimLinear`（条件付き） | 下記「回転寸法」 |
//! | 直径 = 3 | `DiameterDimension` | `DimDiameter` | 10 と 15 が直径の両端 |
//! | 半径 = 4 | `RadialDimension` | `DimRadial` | 10 = 中心、15 = 円周上の点 |
//! | 3 点角度 = 5 | `AngularThreePointDimension` | **スキップ** | モデル新設は M11 タスク69 |
//! | 座標 = 6 | `OrdinateDimension` | **スキップ** | 同上 |
//! | 2 直線角度 = 2 | **無し** | **読めない** | 下記「読めない種別」 |
//! | 弧長 | **無し** | **読めない** | `DimensionType` に値自体が無い |
//!
//! 整列寸法と回転寸法は**クレートでは同じ `RotatedDimension` 型**になる（区別は
//! `dimension_base.dimension_type`）。group 70 は「0〜6 の整数 + 32/64/128 のビット」
//! だが、クレートが `DimensionType` と `is_ordinate_x_type` /
//! `is_at_user_defined_location` / `is_block_reference_referenced_by_this_block_only` へ
//! 分解済みなので、このモジュールでビット演算はしない。
//!
//! ## 回転寸法は「整列寸法と同じ図になる」ものだけ受け入れる
//!
//! 回転寸法の値は計測 2 点を group 50 の方向へ**投影した長さ**で、
//! [`mcad_core::DimLinear`] が描く `|p2 − p1|` とは一般に一致しない。向きを保存できる
//! `DimDirection` が入るのは M11 タスク68 なので、それまでは**計測 2 点の向きが
//! group 50 と平行なものだけ**を取り込み、残りは
//! [`ImportSummary::skipped_dimensions`] へ計上してスキップする（対応している
//! 種別だが表現できないだけで、「未対応の種別」ではない。判定と実測根拠は
//! [`linear_dimension_to_geom`] の doc）。**水平／鉛直という向きの問題ではない**
//! （斜辺に付けた水平寸法は落ち、鉛直に並んだ 2 点の鉛直寸法は通る）。
//!
//! ## 角度の単位
//!
//! DXF の角度フィールド（50 / 51 / 52 / 53）は**度**、mcad は**すべてラジアン**
//! （DESIGN.md M11-0(2)）。変換はこのモジュールに閉じる。
//!
//! ## 読めない種別は件数にも出ない
//!
//! 2 直線角度寸法（`AcDb2LineAngularDimension`）と弧長寸法には `dxf` 0.6.1 に読込
//! 分岐が無く、**`drawing.entities()` に現れないまま捨てられる**（実測: クレートの
//! `Entity::read` は未知のサブクラス名で `continue 'new_entity` する）。したがって
//! 「読めない寸法があった」ことをユーザーへ伝えられない（`ACAD_TABLE` と同じ制約）。
//! LibreCAD の角度寸法はこの形式で書かれるため、**LibreCAD の角度寸法は無言で消える**。
//!
//! ## 見た目の anonymous block は読まない（これでよい）
//!
//! LibreCAD（libdxfrw）や AutoCAD は、寸法の見た目（寸法線・補助線・矢先・文字）を
//! anonymous block（group 2 の `*D1` 等）として `BLOCKS` セクションへ書き、DIMENSION は
//! それを参照する。mcad は `BLOCKS` を読まないので、**DIMENSION を取り込んでも同じ
//! 線が二重に入らない**（block 側の `LINE`/`SOLID`/`MTEXT` は丸ごと無視される）。
//! group 2 の参照名も使わない。回帰テストは `tests/dxf_dimensions.rs` の
//! `synthetic_dimensions_fixture_imports_four_dimensions_and_skips_the_rotated_one`
//! （取り込み後のエンティティ数が block の中身を含まないことを固定する）。
//!
//! ## 落ちる情報
//!
//! 寸法ごと捨てるものは 2 種類ある。**種別自体が未対応**（3 点角度寸法・座標寸法。
//! [`ImportSummary::skipped_entities`]）と、**対応している種別（整列・回転・
//! 半径・直径）だが mcad のモデルでは測定値や位置を変えずに表現できない**もの
//! （回転寸法の非平行・group 51・押し出し法線が +Z でない・計測点の退化・実測値の
//! 食い違い。[`ImportSummary::skipped_dimensions`]）。ここへさらに、取り込んだうえで
//! 属性だけ捨てるもの（[`ImportSummary::dropped_dimension_details`]）と、
//! **黙って捨てるもの**（計上しない）を合わせて 4 段階になる。どれがどれかは
//! [`ImportSummary::skipped_dimensions`]・[`ImportSummary::dropped_dimension_details`]
//! と [`dimension_annotation`] の doc に書いてある。押し出し法線が +Z でない寸法を
//! 拒否する理由は [`is_dimension_plane_supported`] の doc。
//!
//! ### 実測値（group 42）が食い違う寸法は取り込まずスキップする
//!
//! group 42（`actual_measurement`、「書いた CAD が表示していた値」）が書かれていて、
//! mcad が定義点から再計算した値と食い違う寸法は、[`ImportSummary::dropped_dimension_details`]
//! へ計上して取り込む（＝値だけ黙って変わる）のではなく、**取り込まず
//! [`ImportSummary::skipped_dimensions`] へ計上してスキップする**（判定は
//! [`actual_measurement_mismatches`]）。典型的な原因は DIMSTYLE の測定倍率
//! （DIMLFAC）で、mcad には測定倍率の概念が無いため表現できない。この食い違いは
//! **幾何が平行のまま値だけ違う**ケースがあり、回転寸法の「計測 2 点が group 50 と
//! 平行なものだけ受け入れる」判定（上記）では防げないため、別枠で判定する。
//!
//! group 42 が省略された DXF（実測: libdxfrw 0.6.3 = LibreCAD）では、省略と
//! 「値が 0」を区別できないためこの検査自体が効かず、寸法は無条件で取り込まれる
//! （`tests/fixtures/synthetic_dimensions.dxf` の回帰テストはこの経路を通る）。
//!
//! DIMSTYLE（group 3）は**名前ごと捨てる**。mcad は文書に 1 つの
//! [`mcad_core::DimStyle`] しか持たない（M9 設計判断）ため、矢先の形・文字高さ・
//! 桁数はすべて mcad 側の現在のスタイルで描かれ、**他 CAD で見たときと寸法の見た目は
//! 一致しない**（値と定義点だけが保存される）。色の ACI 近似・未知線種の
//! `Continuous` フォールバックと同じ「確定的に解釈して計上しない」扱い。
//!
//! 関連付け（DIMASSOC）はクレートの公開フィールドに無いため、**すべて静的寸法として
//! 取り込む**（M9 設計判断どおり非関連。図形を動かしても寸法値は追従しない）。
//!
//! # 文字列は UTF-8 で書く（ヘッダバージョン R2007）
//!
//! export のヘッダバージョンは **R2007**（[`export_dxf`] で `$ACADVER` を設定）。
//! `dxf` クレート 0.6.1 の文字列 codec はここで分岐する:
//!
//! - `$ACADVER <= R2004`: ASCII 範囲外の文字をコードポイントごとに
//!   `\U+XXXX`（4桁大文字16進）へエスケープして書き（`escape_unicode_to_ascii`）、
//!   読込側は `$ACADVER < R2007` を WINDOWS_1252 と見なして
//!   `un_escape_ascii_to_unicode` で戻す。
//! - `$ACADVER >= R2007`: UTF-8 のまま書き、読込側も `read_as_utf8()` で
//!   UTF-8 として読む（エスケープ経路を通らない）。
//!
//! 当初は R2000 を使っていたが、上記エスケープ codec に往復でデータを壊す欠陥が
//! 3 つある（非BMP文字が化ける / 末尾バックスラッシュが消える / リテラル
//! `\U+XXXX` が 1 文字に潰れる）ことが 2026-07-25 に判明したため R2007 へ
//! 引き上げた。欠陥の詳細と、逆に R14 未満へ下げられない理由（LWPOLYLINE が
//! 黙って落ちる）は [`export_dxf`] のコード内コメントにまとめてある。
//!
//! いずれの経路でも変換はクレート側が閉じて行うため、このモジュールに自前の
//! エスケープ／アンエスケープは持たない。UTF-8 で書かれること自体は
//! `tests::save_dxf_writes_cjk_text_as_utf8` が、往復一致は
//! `tests::round_trip_preserves_cjk_and_ascii_text` と
//! `tests::round_trip_preserves_pathological_text` が固定する。
//!
//! [`mcad_file`](crate::mcad_file) の `.mcad`（JSON）と違い、DXF はネイティブに
//! レイヤー名参照・エンティティ種別を持つため、`.mcad` のようなポータブル DTO は
//! 経由しない。`dxf::Drawing` そのものが export 先／import 元の「ファイル形式表現」
//! を兼ねる。ただし [`Document`] の再構築は `.mcad` と同じ規律に従う: 必ず
//! [`Document::new`] から [`Command`]（`AddLayer` / `AddEntity` /
//! `SetLayerProps` / `SetCurrentLayer`）で組み立て、完了後に
//! [`Document::clear_history`] を呼ぶ（`Document` の内部フィールドへ直接触らない）。
//!
//! # レイヤー "0" の重複回避
//!
//! `dxf::Drawing::new()` は内部で `normalize()` を呼び、`$CLAYER`（既定 `"0"`）に
//! 対応するレイヤーを自動的に 1 つ追加する。一方 `mcad_core::Document::new()` の
//! デフォルトレイヤーも常に名前 `"0"` を持つため、何もせず `add_layer` すると
//! `"0"` という名前のレイヤーが 2 つできてしまう。これを避けるため、export では
//! `Drawing::new()` 直後に自動追加されたレイヤーをすべて取り除いてから、
//! `Document` のレイヤーだけを追加する。
//!
//! # カレントレイヤー
//!
//! DXF のヘッダ変数 `$CLAYER`（`Drawing::header::current_layer`）にカレント
//! レイヤー名を書き込み、import 時にその名前のレイヤーが見つかればカレントに
//! 設定する（見つからなければデフォルトレイヤーのままにする）。これは DXF の
//! 標準的な仕組みを使ったベストエフォートの復元であり、保証はしない。
//!
//! # レイヤーロックは保存されない
//!
//! `dxf::tables::Layer` にはロック状態に対応するフィールドが見当たらない
//! （AutoCAD の DXF 仕様上はグループコード 70 のフラグビットが担うが、この
//! クレートのバージョンでは公開されていない）。そのため DXF 往復ではロック状態を
//! 保存できず、import 時は常に `locked: false` として復元する。
//!
//! # レイヤーの重ね順は保存されない
//!
//! `dxf::tables::Layer`（クレート 0.6.1 の生成コード
//! `target/debug/build/dxf-*/out/generated/tables.rs`）のフィールドは
//! `name, handle, color, line_type_name, is_layer_plotted, line_weight,
//! is_layer_on` 等のみで、重ね順／z-index に相当するフィールドが**存在しない**
//! （実測。ロックが同クレートに無いのと同じ理由）。DXF の LAYER テーブル自体に
//! 重ね順という概念がないため、これは仕様上の制約であり回避できない。
//!
//! - **export** は、デフォルトレイヤーを LAYER テーブルの先頭に固定し、残りを
//!   [`Document::layers_in_order`](mcad_core::Document::layers_in_order) の順
//!   （[`Layer::order`](mcad_core::Layer::order) 昇順、奥→手前）で続けて
//!   `drawing.add_layer` を呼ぶ（[`export_dxf`] 参照）。先頭固定は、下記
//!   import の「テーブル先頭 = デフォルトレイヤー」規則を常に成り立たせる
//!   ためで、これを崩すと**デフォルトレイヤー（削除できない特別なレイヤー）が
//!   往復で入れ替わる**実害のあるバグになる（2026-07-26 の Codex adversarial
//!   review [high] 指摘で発覚、[`export_dxf`] のコメント参照）。
//!   デフォルト以外のレイヤー間では、DXF はこの出現順を重ね順として解釈しない
//!   ため、これは**他 CAD のレイヤー一覧表示がテーブル出現順に従った場合にだけ
//!   意味を持つ best-effort** であり、保証ではない。
//! - **import** は LAYER テーブルの出現順（`enumerate()` の `index`）を
//!   そのまま `order = index as i32` として採用する（[`import_dxf`] 参照）。
//! - したがって「mcad → DXF export → 同じ mcad で import」の往復では、
//!   デフォルト以外のレイヤーは export が出現順を重ね順に揃えているため
//!   **`order`（＝見た目の重ね順）は結果的に保たれる**。一方、**デフォルト
//!   レイヤー自身の `order` は先頭固定のため往復で失われ、re-import 後は常に
//!   `order = 0` になる**（先頭固定を優先した意図的な割り切り）。また、他 CAD
//!   がテーブルを並べ替えて保存した DXF や、LAYER テーブルを手で編集した DXF を
//!   import する場合は、その CAD が採用した出現順がそのまま `order` になるため、
//!   **mcad 側の元の重ね順とは一致しない可能性がある**（ロック・線幅と同じ
//!   「往復で失われうる」仕様）。
//!
//! # 色は近似（ACI ⇔ RGB）
//!
//! mcad の [`Rgb`] は真の 24bit RGB だが、`dxf` クレートの `Color` は AutoCAD
//! Color Index（ACI, 1〜255 の索引）のみを表現でき、任意 RGB への変換 API を
//! 持たない。ここでは「ACI の標準的な基本 9 色（1〜9）」の固定パレットを持ち、
//! export 時は最も近い（二乗距離最小の）パレット色の ACI へ、import 時はその ACI
//! に対応する固定 RGB へ変換する。パレットに載っていない ACI（10〜255）を読んだ
//! 場合は中間グレーで近似する。
//!
//! これは往復の完全一致を **保証しない**（パレットに厳密に一致する色を使った
//! 場合のみ完全往復する）。任意の RGB を ACI へ丸めて DXF に書き出す方式（設計上の
//! 選択肢 (a)）と、色比較を往復テストから除外する方式（選択肢 (b)）のどちらも
//! 許容されると判断したが、他の CAD ソフトで DXF を開いたときに色がまったくの
//! `by_layer` 一色に潰れるより、基本色だけでも近い色が付く方が実用上親切だと
//! 考え、(a) 側（固定パレットによる近似）を選んだ。
//!
//! # 未対応エンティティ・不正ジオメトリの扱い（import）
//!
//! - `EntityType` のうち Line/Circle/Arc/LwPolyline/ModelPoint/Text と、DIMENSION の
//!   4 経路（整列・回転・半径・直径。上記「DIMENSION の import」）以外
//!   （SPLINE・ELLIPSE・INSERT・3 点角度／座標寸法など）は無視し、
//!   `skipped_entities` としてカウントする。
//! - TEXT のうち位置基準（justification）が「水平 Left かつ垂直 Baseline」以外の
//!   ものも無視してカウントする。この場合、文字位置は `location`（group code 10）
//!   ではなく alignment point（group code 11）が持つが、mcad の
//!   [`mcad_core::TextGeom`] は左寄せ・ベースライン基準の 1 点しか表現できず、
//!   逆算にはフォントメトリクスが必要で io 層には無い。詳細と将来の対応方針は
//!   [`is_text_justification_supported`] の doc を参照。
//! - 対応エンティティ種別であっても、非有限座標・負半径・非正の文字高さ・空文字列
//!   など [`mcad_core::EntityGeom::validate`] が拒否するジオメトリは、
//!   **読込全体を失敗させず** 無視してカウントに含める。DXF は他の CAD
//!   ソフトが吐き出したファイルであることも多く、壊れたエンティティ 1 件で
//!   ファイル全体の import を失敗させたくないため。ただし DXF ファイル自体が
//!   壊れている（構文が壊れている、テーブルが読めない等）場合は
//!   `dxf::DxfError` 由来の [`crate::IoError::Dxf`] として失敗する
//!   （こちらは個別エンティティの問題ではなく全体構造の異常）。この検証落ちが
//!   寸法（`DimLinear`/`DimRadial`/`DimDiameter`。例: 半径・直径寸法の半径 0）で
//!   起きた場合は、種別自体は対応しているため `skipped_entities` ではなく
//!   `skipped_dimensions` へ計上する（[`ImportSummary::skipped_dimensions`] 参照）。
//!
//! # 未知のレイヤー名を参照するエンティティ
//!
//! DXF ファイルによっては、LAYER テーブルに列挙されていないレイヤー名を
//! エンティティが直接参照することがある（各種 CAD ソフトの出力・手書き DXF で
//! 起こりうる）。これを無視するとエンティティごと復元できなくなってしまうため、
//! 未知のレイヤー名を見つけた時点でその場に新しいレイヤーを追加し、そちらへ
//! 割り当てる（復元性を優先する）。
//!
//! # 単位（`$INSUNITS`）
//!
//! export は DXF ヘッダの `$INSUNITS`（`dxf::Header::default_drawing_units`）に
//! `Units::Millimeters`（値 4）を書く（DESIGN.md M8 設計判断1）。mcad のワールド
//! 単位が常に mm であることの明示であり、他 CAD が単位系変換の要否を判断する
//! 手がかりになる。import 側は `$INSUNITS` を読まない（もともと 1 ワールド単位
//! = 1mm の無変換取り込みであり、DESIGN.md M8 設計判断1が既存図面もこの前提で
//! 再解釈すると決めているため）。
//!
//! # TEXT 高さの尺度契約（DESIGN.md M8 設計判断4a）
//!
//! [`TextGeom::height`] は紙 mm（表示・出力先ごとに換算される「格納値」）で持つ。
//! DXF の TEXT 高さ（group code 40）はモデル空間の長さなので、そのまま転写すると
//! 尺度 1:1 以外で誤る。ここでは:
//!
//! - **export**: 図面の [`mcad_core::SheetMeta::scale`] から
//!   `k = Scale::world_mm_per_paper_mm()` を求め、`text_height = height * k` を書く
//!   （[`text_to_dxf_entity`]）。1:1（`k = 1.0`）では従来どおり無変換と等価。
//! - **import**: DXF ファイルには図面尺度の概念がないため、**1:1 とみなして**
//!   `text_height` の数値をそのまま紙 mm として取り込む（[`dxf_entity_to_geom`]）。
//!
//! この結果、尺度が 1:1 以外の図面を export → import すると「紙 mm としての
//! 意味」は往復しない（import 後は常に尺度 1:1 の図面として解釈される）が、
//! **モデル空間での幾何サイズ（ワールド高さ）は正しく保存される**
//! （1:2 で紙 mm 3.5 → DXF 7.0 → import 後は尺度 1:1 の紙 mm 7.0 = ワールド高さ
//! 7.0 で不変）。レイヤー重ね順・色近似と同種の「仕様として非保存」であり、
//! テストは `text_height_scales_by_sheet_scale_on_export` と
//! `text_geometric_height_survives_round_trip_across_scale_reinterpretation` が
//! 固定する。
//!
//! # 線幅・線種（lineweight / LTYPE）は best-effort（DESIGN.md M8 設計判断5）
//!
//! `dxf` 0.6.1 の実測結果（2026-08-02）:
//!
//! - **線幅（エンティティ）**: `dxf::entities::EntityCommon::lineweight_enum_value`
//!   は生の `pub i16`（group code 370、単位は DXF 仕様どおり **1/100mm**）で、
//!   任意の値を読み書きできる。[`WidthMm`] との変換は
//!   [`width_mm_to_dxf_lineweight_raw`] / [`dxf_lineweight_raw_to_width_mm`]。
//!   **`raw <= 0` はすべて ByLayer**（[`Style::width_mm`] = `None`）として扱う
//!   （[`dxf_lineweight_to_style_width`]）。負値は DXF 仕様の group code 370 の
//!   慣例（`-1` = BYLAYER, `-2` = BYBLOCK, `-3` = DEFAULT）に基づく。`0` も
//!   同じ扱いに含めるのは、`EntityCommon::lineweight_enum_value` の spec 上の
//!   既定値が `0`（group code 370 を**省略**した DXF ファイルもこの値で入って
//!   くる）であり、これを「明示的な極細線」としてクランプ対象にすると、
//!   lineweight を書かない大多数の外部 DXF で毎エンティティがクランプ計上
//!   されるノイズになるため（色の by_block/by_entity と同じ「クレートの対応
//!   範囲を超える区別は ByLayer へ丸める」方針の延長）。正の raw 値は
//!   [`WidthMm`] の範囲（0.05〜5.0mm = raw 5〜500）へクランプし、クランプが
//!   起きた件数を [`ImportSummary::clamped_line_widths`] に積む（色 ACI・
//!   DXF import の他の best-effort 変換と同じ「黙って丸めない」流儀）。export
//!   には型の構成上、検証済みの [`WidthMm`] しか到達しないためクランプ不要
//!   （[`width_mm_to_dxf_lineweight_raw`] は常に範囲内の raw 値を返す）。
//! - **線幅（レイヤー）は export できない（実測で判明した非対称な制約）**:
//!   `dxf::tables::Layer::line_weight` は `i16` ではなく不透明な `LineWeight`
//!   型（group code 370）で、**任意値を作れる公開コンストラクタが存在しない**
//!   （`LineWeight::from_raw_value` は `pub(crate)`、公開 API は `by_block()`
//!   （raw `-1`）・`by_layer()`（raw `-2`）・`Default::default()`（raw `0`）の
//!   3 つの固定値のみ。実測: `dxf::LineWeight::from_raw_value(35)` を
//!   `mcad-io` 側から呼ぶと `E0624 private associated function` でコンパイル
//!   エラーになる）。読む側（`.raw_value()`）は公開されているため **import は
//!   他 CAD が書いたレイヤー線幅を最大限取り込める**が、**export はレイヤーの
//!   `width_mm` を DXF へ書けない**（`DxfLayer::line_weight` は既定値
//!   `raw = 0` のまま）。import 側（[`dxf_layer_lineweight_to_width_mm`]）は
//!   `raw <= 0` を「未指定」として [`WidthMm::DEFAULT`] を返しクランプ計上
//!   しない（mcad 自身が export したレイヤーが必ずこの経路を通るため、計上する
//!   と re-import のたびに全レイヤーがクランプ扱いになりノイズになる）。正の
//!   raw 値（他 CAD が明示的に書いた値）は通常どおり範囲外をクランプし計上する。
//!   これはロック・重ね順と同じ「クレート側 API の欠落による仕様上の制約」で
//!   あり、レイヤー個別色（`color: Color`、公開コンストラクタ `Color::from_index`
//!   あり）とは非対称。エンティティ個別上書き（[`Style::width_mm`]）は
//!   `lineweight_enum_value` が生 `i16` のためこの制約を受けず、往復する。
//! - **線種**: `dxf::tables::LineType`（LTYPE テーブル、`Drawing::add_line_type`）
//!   と、レイヤー・エンティティ双方の `line_type_name`（`String`、group code 6）
//!   が存在し読み書きできる。ただし DXF の LTYPE 名は AutoCAD 標準ライブラリ
//!   （`acad.lin` 等）の任意の名前空間であり、mcad の [`Linetype`] が持つのは
//!   4 種のみ。export は mcad 側の名前（`CONTINUOUS` / `DASHED` / `DASHDOT` /
//!   `DIVIDE`。`DashDotDot`（二点鎖線）は AutoCAD 標準ライブラリ（acad.lin）の
//!   慣例名 `DIVIDE` を使う — `DASHDOT2` は同ライブラリでは「半スケールの
//!   一点鎖線」を指すため、それを二点鎖線として書く／読むと他 CAD 由来の本物の
//!   `DASHDOT2` を誤解釈する）で LTYPE テーブルへ簡易パターン
//!   （[`dxf_line_type_defs`]）を登録し、レイヤー・エンティティへその名前を
//!   書く。import は名前を
//!   大文字小文字を無視して mcad の 4 種へ逆引きし（[`dxf_name_to_linetype`]）、
//!   **一致しない名前（他 CAD 由来の未知の線種）は `Linetype::Continuous` へ
//!   フォールバックする**（DESIGN.md M8 設計判断5「未知の線種名は Continuous
//!   へフォールバック」）。この場合は「無視して計上」ではなく「実線として
//!   確定的に解釈する」ため計上しない（色近似が常に何らかの色を返すのと同じ
//!   考え方）。エンティティの `line_type_name` が `BYLAYER`/`BYBLOCK`（大文字
//!   小文字を無視）の場合はレイヤー継承（[`Style::linetype`] = `None`）として
//!   扱う。
//!   LTYPE のダッシュパターン長（`total_pattern_length`・
//!   `dash_dot_space_lengths`）は往復に使わない mcad 独自の簡易値であり、他
//!   CAD ソフトでの見た目の一致は保証しない（画面表示のダッシュ長は app 層の
//!   紙 mm 定数が別途決める。DESIGN.md M8 設計判断5・4a 換算表参照）。

use std::collections::HashMap;
use std::path::Path;

use dxf::entities::{
    Arc as DxfArc, Circle as DxfCircle, DiameterDimension, DimensionBase, Entity as DxfEntity,
    EntityType, Line as DxfLine, LwPolyline, ModelPoint, RadialDimension, RotatedDimension,
    Text as DxfText,
};
use dxf::enums::{
    DimensionType, HorizontalTextJustification, Units as DxfUnits, VerticalTextJustification,
};
use dxf::tables::{Layer as DxfLayer, LineType as DxfLineType};
use dxf::{Color, Drawing, LwPolylineVertex, Point as DxfPoint};

use mcad_core::{
    Command, DimAnnotation, DimDiameter, DimLinear, DimRadial, Document, Entity, EntityGeom, Layer,
    LayerId, Linetype, Rgb, Style, TableGeom, TextGeom, WidthMm, expand_table,
};
use mcad_geom::{Arc, Circle, LineSeg, Point2, Polyline, Shape};

use crate::IoError;

/// DXF ACI の標準的な基本色パレット（1〜9）と、それぞれに割り当てる近似 RGB。
///
/// 色番号の慣例的な意味づけ（AutoCAD の標準パレット）に基づく。7 は本来
/// 背景色に応じて白／黒のどちらかとして描画されるが、ここでは白として扱う。
const ACI_PALETTE: [(u8, Rgb); 9] = [
    (1, Rgb::new(255, 0, 0)),     // 赤
    (2, Rgb::new(255, 255, 0)),   // 黄
    (3, Rgb::new(0, 255, 0)),     // 緑
    (4, Rgb::new(0, 255, 255)),   // シアン
    (5, Rgb::new(0, 0, 255)),     // 青
    (6, Rgb::new(255, 0, 255)),   // マゼンタ
    (7, Rgb::new(255, 255, 255)), // 白（慣例上は白/黒）
    (8, Rgb::new(65, 65, 65)),    // 濃灰
    (9, Rgb::new(128, 128, 128)), // 灰
];

/// パレットに載っていない ACI インデックスを読んだ場合の近似 RGB（中間グレー）。
const ACI_FALLBACK_RGB: Rgb = Rgb::new(191, 191, 191);

/// DXF import の結果。
///
/// 未対応エンティティ・不正ジオメトリのエンティティを無視するのは失敗ではないため
/// `Result` の `Err` にはせず、この構造体の `skipped_entities` として返す。
///
/// `Document` が `Debug` を実装していないため、この構造体自体も `Debug` は
/// 導出しない。
pub struct ImportSummary {
    /// 再構築されたドキュメント。
    pub document: Document,
    /// 無視したエンティティ数（未対応の種別 + 不正なジオメトリ）。
    ///
    /// 寸法については**種別自体が未対応**（3 点角度寸法・座標寸法。上記
    /// 「DIMENSION の import」対応表参照）のものだけがここに入る。整列・回転・
    /// 半径・直径という対応している種別の寸法が個別の理由で取り込めなかった場合は
    /// [`skipped_dimensions`](Self::skipped_dimensions) 側に入り、ここには含まれない。
    pub skipped_entities: usize,
    /// **対応している種別**（整列・回転・半径・直径）の寸法のうち、mcad の
    /// モデルでは測定値や位置を変えずに表現できないため取り込まなかった数
    /// （M11 タスク67）。未対応の種別ではない（それは
    /// [`skipped_entities`](Self::skipped_entities) 側）。
    ///
    /// 数えるのは次の 5 つ（詳細と実測根拠は
    /// [`linear_dimension_to_geom`] / [`is_dimension_plane_supported`] /
    /// [`actual_measurement_mismatches`] の doc）。
    ///
    /// - 回転寸法で、計測 2 点の向きが group 50 の方向と平行でない（投影値と
    ///   実距離が食い違うため表現できない）
    /// - 回転寸法で group 51（水平方向角）が 0 でない
    /// - 押し出し法線（group 210/220/230）が +Z でない
    /// - 計測点が退化している（長さ寸法の計測 2 点が同一、半径・直径寸法の半径が 0）
    /// - 実測値（group 42）が mcad の再計算値と食い違う（典型的には DIMSTYLE の
    ///   測定倍率 DIMLFAC が原因）
    pub skipped_dimensions: usize,
    /// [`WidthMm`] の範囲（0.05..=5.0mm）外の DXF lineweight を範囲へクランプして
    /// 取り込んだ件数（レイヤー + エンティティの合計）。無視はしない（エンティティ
    /// ごと捨てる `skipped_entities` とは別枠）が、黙って値を変えたことを
    /// ステータス表示できるよう計上する（モジュール doc「線幅・線種は
    /// best-effort」参照）。
    pub clamped_line_widths: usize,
    /// **取り込んだ**寸法のうち、mcad のモデルで表現できない属性を捨てた件数
    /// （M11 タスク67、モジュール doc「DIMENSION の import」参照）。
    ///
    /// [`clamped_line_widths`](Self::clamped_line_widths) と同じ「捨てずに取り込むが
    /// 黙って変えない」枠で、**寸法 1 件につき最大 1**（属性ごとではなく寸法ごとに
    /// 数える）。読めなかった寸法そのものは種別が未対応なら
    /// [`skipped_entities`](Self::skipped_entities)、対応している種別だが表現できない
    /// なら [`skipped_dimensions`](Self::skipped_dimensions) 側に入る。
    ///
    /// 数えるのは次の 3 つ。
    ///
    /// - 文字テンプレート（group 1）が空でも `<>` でもない（スペース 1 文字の
    ///   「文字を出さない」指定を含む）。M11 タスク70 までは無注記で取り込む
    /// - 補助線の傾き（group 52 `extension_line_angle`）が 0 でない
    /// - 寸法文字の回転（group 53 `text_rotation_angle`）が 0 でない
    ///
    /// 実測値（group 42 `actual_measurement`）が mcad の再計算値と食い違う寸法は
    /// **ここへは計上せず**、取り込まずに [`skipped_dimensions`](Self::skipped_dimensions)
    /// へ計上する（モジュール doc「実測値（group 42）が食い違う寸法は取り込まず
    /// スキップする」参照）。
    ///
    /// DIMSTYLE 名（group 3）・引出線長（group 40）・文字の寄せ（group 71）は
    /// **数えない**（全寸法で常に落ちる情報で、計上するとノイズにしかならない。
    /// 色の ACI 近似・未知線種の `Continuous` フォールバックと同じ扱い。モジュール
    /// doc 参照）。
    pub dropped_dimension_details: usize,
}

/// DXF export の結果。
///
/// M6 で `Entity.geom` が [`mcad_core::EntityGeom`] 化され、Text・寸法（非 Shape
/// バリアント）を持てるようになった。Text（[`EntityGeom::Text`]）はタスク25で
/// DXF `TEXT` エンティティとして export されるが、寸法（`DimLinear` / `DimRadial`）は
/// DXF に対応するプリミティブがなく export 時にスキップされる。黙って消えると
/// 呼び出し側がデータロスに気づけないため、[`ImportSummary`] と対称に、スキップ件数を
/// 返してステータス表示できるようにする。
///
/// `dxf::Drawing` は `Debug` を導出しているが、対称性と将来の拡張余地のためこの構造体は
/// `Debug` を導出しない（[`ImportSummary`] と同じ扱い）。
pub struct ExportSummary {
    /// 生成した図面。
    pub drawing: Drawing,
    /// DXF 非対応でスキップしたエンティティ数（寸法。Text はタスク25で export 対応済み）。
    pub skipped_entities: usize,
}

/// 2 色間の二乗距離（近似色検索に使う）。
fn color_distance_sq(a: Rgb, b: Rgb) -> i32 {
    let dr = i32::from(a.r) - i32::from(b.r);
    let dg = i32::from(a.g) - i32::from(b.g);
    let db = i32::from(a.b) - i32::from(b.b);
    dr * dr + dg * dg + db * db
}

/// RGB に最も近いパレット色の ACI インデックスへ変換する。
fn rgb_to_aci(rgb: Rgb) -> Color {
    let index = ACI_PALETTE
        .iter()
        .min_by_key(|(_, palette_rgb)| color_distance_sq(*palette_rgb, rgb))
        .map(|(index, _)| *index)
        .unwrap_or(7);
    Color::from_index(index)
}

/// ACI インデックスを固定の近似 RGB へ変換する。
fn aci_to_rgb(index: u8) -> Rgb {
    ACI_PALETTE
        .iter()
        .find(|(i, _)| *i == index)
        .map(|(_, rgb)| *rgb)
        .unwrap_or(ACI_FALLBACK_RGB)
}

/// エンティティ個別色（[`Style::color`]）を DXF の `Color` へ変換する。
///
/// `None`（レイヤー色継承）は `Color::by_layer()` として表現する。
fn style_color_to_dxf(color: Option<Rgb>) -> Color {
    match color {
        Some(rgb) => rgb_to_aci(rgb),
        None => Color::by_layer(),
    }
}

/// DXF の `Color` をエンティティ個別色（[`Style::color`]）へ変換する。
///
/// `by_layer` はレイヤー色継承（`None`）として扱う。`by_block` / `by_entity` /
/// 消灯（負値）はこのクレートの対応範囲では意味を持たないため、同様に
/// レイヤー継承として扱う。
fn dxf_color_to_style(color: &Color) -> Option<Rgb> {
    color.index().map(aci_to_rgb)
}

/// [`WidthMm`] を DXF lineweight の raw 値（1/100mm 単位、group code 370）へ変換する。
///
/// [`WidthMm`] は常に `0.05..=5.0` の範囲（= raw `5..=500`）なので、この変換は
/// 常に妥当な非負値を返す（クランプ不要。モジュール doc「線幅・線種は
/// best-effort」参照）。
fn width_mm_to_dxf_lineweight_raw(width: WidthMm) -> i16 {
    (f64::from(width.mm()) * 100.0).round() as i16
}

/// DXF lineweight の raw 値（1/100mm 単位）を [`WidthMm`] へ変換する。
///
/// [`WidthMm`] の範囲外（負値含む）は範囲へクランプし、クランプが起きたかを
/// 2 番目の戻り値で返す（呼び出し側が [`ImportSummary::clamped_line_widths`] へ
/// 積む）。`raw` は `i16` なので常に有限であり、[`WidthMm::clamped`] が `None`
/// を返すことはない。
fn dxf_lineweight_raw_to_width_mm(raw: i16) -> (WidthMm, bool) {
    let mm = f32::from(raw) / 100.0;
    match WidthMm::new(mm) {
        Ok(w) => (w, false),
        Err(_) => (
            WidthMm::clamped(mm).expect("raw i16 / 100.0 is always finite"),
            true,
        ),
    }
}

/// DXF エンティティの lineweight raw 値（group code 370）を [`Style::width_mm`]
/// （個別上書き。`None` = ByLayer）へ変換する。
///
/// `raw <= 0` はすべて ByLayer として扱う: 負値は DXF 仕様の group code 370 の
/// 慣例で `-1` = BYLAYER・`-2` = BYBLOCK・`-3` = DEFAULT（モジュール doc 参照）。
/// `0` も含めるのは、このクレートの `EntityCommon::lineweight_enum_value` の
/// 既定値（spec の `DefaultValue="0"`）が `0` であり、**code 370 を省略した
/// DXF ファイル**（他 CAD ソフトの出力に多い、「個別指定なし」の意図）を読むと
/// この既定値のまま入ってくるため。`0` を「明示的な極細線」と区別なく
/// クランプ対象にすると、code 370 を書かない大多数の外部 DXF で毎エンティティが
/// クランプ計上されるノイズになる。クランプが起きたかは 2 番目の戻り値で返す。
fn dxf_lineweight_to_style_width(raw: i16) -> (Option<WidthMm>, bool) {
    if raw <= 0 {
        (None, false)
    } else {
        let (width, clamped) = dxf_lineweight_raw_to_width_mm(raw);
        (Some(width), clamped)
    }
}

/// レイヤーの DXF lineweight raw 値を [`WidthMm`] へ変換する。
///
/// `raw <= 0`（このクレートの `LineWeight::default()` が返す `0` を含む。export
/// 側は `dxf` 0.6.1 の API 制約でレイヤーへ任意の raw 値を書けないため、
/// **mcad 自身が export したレイヤーは常にこの経路を通る**。モジュール doc
/// 「線幅・線種は best-effort」参照）は「未指定」とみなし、[`WidthMm::DEFAULT`]
/// を返す（クランプではないので計上しない。計上すると自分が export した DXF を
/// re-import するたびに全レイヤーがクランプ扱いになりノイズになる）。正の raw
/// 値（他 CAD が明示的に書いた値）は通常どおり範囲外をクランプし、計上する。
fn dxf_layer_lineweight_to_width_mm(raw: i16) -> (WidthMm, bool) {
    if raw <= 0 {
        (WidthMm::DEFAULT, false)
    } else {
        dxf_lineweight_raw_to_width_mm(raw)
    }
}

/// mcad の [`Linetype`] を、export が LTYPE テーブルへ登録する DXF 線種名へ変換する。
///
/// [`Linetype`] は `#[non_exhaustive]` なので、クレート外の本関数では将来追加される
/// 未知バリアントに備えてワイルドカード腕で `CONTINUOUS`（実線）へフォールバックする。
///
/// `DashDotDot`（二点鎖線）は AutoCAD 標準ライブラリ（acad.lin）の慣例名
/// `DIVIDE` を使う。`DASHDOT2` を使わない理由: 同ライブラリでは `DASHDOT2` は
/// 「半スケールの一点鎖線（dash-dot）」を指す既存の名前であり、それを二点鎖線と
/// して書くと、受け手の CAD には一点鎖線と紛らわしい名前を渡すことになる
/// （モジュール doc「線幅・線種は best-effort」参照）。
fn linetype_to_dxf_name(linetype: Linetype) -> &'static str {
    match linetype {
        Linetype::Continuous => "CONTINUOUS",
        Linetype::Dashed => "DASHED",
        Linetype::DashDot => "DASHDOT",
        Linetype::DashDotDot => "DIVIDE",
        _ => "CONTINUOUS",
    }
}

/// DXF 線種名を mcad の [`Linetype`] へ逆引きする。
///
/// 大文字小文字を無視して [`linetype_to_dxf_name`] の逆写像と比較する。一致しない
/// 名前（他 CAD ソフト由来の未知の線種）は `Linetype::Continuous` へフォールバック
/// する（DESIGN.md M8 設計判断5。モジュール doc 参照）。`DASHDOT2` の腕は意図的に
/// 持たない: acad.lin の慣例では `DASHDOT2` は一点鎖線（半スケール）であり
/// `DashDotDot` ではないため、他 CAD 由来の本物の `DASHDOT2` は未知名として
/// `Continuous` へフォールバックさせる（[`linetype_to_dxf_name`] のdoc参照）。
fn dxf_name_to_linetype(name: &str) -> Linetype {
    match name.trim().to_ascii_uppercase().as_str() {
        "DASHED" => Linetype::Dashed,
        "DASHDOT" => Linetype::DashDot,
        "DIVIDE" => Linetype::DashDotDot,
        _ => Linetype::Continuous,
    }
}

/// DXF エンティティの `line_type_name`（group code 6）を [`Style::linetype`]
/// （個別上書き。`None` = ByLayer）へ変換する。
///
/// `BYLAYER`/`BYBLOCK`（大文字小文字を無視）はレイヤー継承として扱う。それ以外は
/// [`dxf_name_to_linetype`] で mcad の 4 種へ確定的に解釈する（未知名は
/// Continuous。フォールバックであり ByLayer ではない）。
fn dxf_line_type_name_to_style_linetype(name: &str) -> Option<Linetype> {
    let upper = name.trim().to_ascii_uppercase();
    if upper == "BYLAYER" || upper == "BYBLOCK" {
        None
    } else {
        Some(dxf_name_to_linetype(name))
    }
}

/// export 時に LTYPE テーブルへ登録する 4 線種の簡易定義。
///
/// パターン長（`total_pattern_length`・`dash_dot_space_lengths`）は往復に使わない
/// mcad 独自の見た目用の値で、他 CAD ソフトでの表示一致は保証しない（モジュール
/// doc「線幅・線種は best-effort」参照。画面上のダッシュ長は app 層の紙 mm 定数が
/// 別途決める）。要素は正 = 線分、負 = 空白、`0.0` = 点。
fn dxf_line_type_defs() -> [DxfLineType; 4] {
    [
        DxfLineType {
            name: linetype_to_dxf_name(Linetype::Continuous).to_string(),
            description: "Solid line".to_string(),
            ..Default::default()
        },
        DxfLineType {
            name: linetype_to_dxf_name(Linetype::Dashed).to_string(),
            description: "Dashed".to_string(),
            alignment_code: 'A' as i32,
            element_count: 2,
            total_pattern_length: 0.75,
            dash_dot_space_lengths: vec![0.5, -0.25],
            ..Default::default()
        },
        DxfLineType {
            name: linetype_to_dxf_name(Linetype::DashDot).to_string(),
            description: "Dash dot".to_string(),
            alignment_code: 'A' as i32,
            element_count: 4,
            total_pattern_length: 1.0,
            dash_dot_space_lengths: vec![0.5, -0.25, 0.0, -0.25],
            ..Default::default()
        },
        DxfLineType {
            name: linetype_to_dxf_name(Linetype::DashDotDot).to_string(),
            description: "Divide (dash dot dot)".to_string(),
            alignment_code: 'A' as i32,
            element_count: 6,
            total_pattern_length: 1.25,
            dash_dot_space_lengths: vec![0.5, -0.25, 0.0, -0.25, 0.0, -0.25],
            ..Default::default()
        },
    ]
}

fn to_dxf_point(p: Point2) -> DxfPoint {
    DxfPoint::new(p.x, p.y, 0.0)
}

fn from_dxf_point(p: &DxfPoint) -> Point2 {
    Point2::new(p.x, p.y)
}

/// [`Shape`] を対応する DXF エンティティへ変換する。
fn shape_to_dxf_entity(shape: &Shape) -> DxfEntity {
    let specific = match shape {
        Shape::Point(p) => EntityType::ModelPoint(ModelPoint::new(to_dxf_point(*p))),
        Shape::Line(l) => EntityType::Line(DxfLine {
            p1: to_dxf_point(l.a),
            p2: to_dxf_point(l.b),
            ..Default::default()
        }),
        Shape::Circle(c) => EntityType::Circle(DxfCircle {
            center: to_dxf_point(c.center),
            radius: c.radius,
            ..Default::default()
        }),
        Shape::Arc(a) => EntityType::Arc(DxfArc {
            center: to_dxf_point(a.center),
            radius: a.radius,
            start_angle: a.start_angle.to_degrees(),
            end_angle: a.end_angle.to_degrees(),
            ..Default::default()
        }),
        Shape::Polyline(pl) => {
            let mut flags = 0;
            if pl.closed {
                flags |= 1;
            }
            let vertices = pl
                .vertices
                .iter()
                .map(|v| LwPolylineVertex {
                    x: v.x,
                    y: v.y,
                    bulge: 0.0,
                    ..Default::default()
                })
                .collect();
            EntityType::LwPolyline(LwPolyline {
                flags,
                vertices,
                ..Default::default()
            })
        }
    };
    DxfEntity::new(specific)
}

/// DXF `TEXT` の位置基準（justification）が mcad の [`TextGeom`] で表現できるか判定する。
///
/// 受理するのは水平 = `Left`（group code 72 = 0）かつ垂直 = `Baseline`
/// （group code 73 = 0）の組み合わせだけ。どちらも DXF の既定値であり、
/// [`text_to_dxf_entity`] が書き出す mcad 自身の TEXT は常にこの組み合わせになる。
/// それ以外は [`dxf_entity_to_geom`] が `None` を返し、呼び出し側が
/// [`ImportSummary::skipped_entities`] に計上する。
///
/// # なぜ alignment point から逆算せず、スキップするのか
///
/// DXF TEXT の仕様では、水平 justification が `Left` 以外、または垂直
/// justification が `Baseline` 以外のとき、**実際の文字位置は alignment point
/// （group code 11 = [`DxfText::second_alignment_point`]）が持ち、`location`
/// （group code 10）は意味を持たない**。そのため `location` をそのまま
/// [`TextGeom::anchor`] へ入れると、外部 CAD が作った中央揃え・右揃え・非ベースライン
/// 揃えの TEXT は**間違った位置に配置される**（mcad 自身の export は常に
/// Left/Baseline なので、既存の往復テストではこの不整合を検出できない）。
///
/// では alignment point から `anchor` を逆算すればよい、とはならない。
/// [`TextGeom`] は「左寄せ・ベースライン基準の 1 点（`anchor`）」しか持たないため、
/// 例えば中央揃えの alignment point（描画される文字列の中央）から左端を求めるには
/// **文字列の描画幅、すなわちフォントメトリクスが必要**になる。フォント
/// （Noto Sans JP）を持つのは `mcad-app` であり、`mcad-io` はアーキテクチャ上の
/// 依存方向（app → io → core → geom）から app を参照できない。つまり io 層では
/// 原理的に正しい逆算ができない。「文字幅を係数で近似する」のは結局位置がずれる
/// ので、黙って誤配置する現状と同じ問題を残すだけである。
///
/// 誤った位置に黙って置くより、既存のスキップ機構へ乗せて件数をユーザーへ通知する
/// 方が「無警告のデータロスを防ぐ」方針（モジュール doc の「未対応エンティティ・
/// 不正ジオメトリの扱い（import）」）に合う、と判断した。
///
/// 将来対応するなら、io 層は justification と alignment point を素通しし、
/// フォントメトリクスを持つ app 層で `anchor` へ変換する構造が必要になる。
fn is_text_justification_supported(text: &DxfText) -> bool {
    matches!(
        text.horizontal_text_justification,
        HorizontalTextJustification::Left
    ) && matches!(
        text.vertical_text_justification,
        VerticalTextJustification::Baseline
    )
}

// ---------------------------------------------------------------------
// DIMENSION の import（M11 タスク67）
// ---------------------------------------------------------------------

/// 押し出し法線（group 210/220/230）を +Z と見なす許容。
///
/// libdxfrw・AutoCAD とも 2D 図面では厳密に `(0,0,1)` を書く（または省略して
/// クレート既定の `Vector::z_axis()` になる）ので、これは書式上の丸め誤差だけを
/// 吸収するための値で、傾いた面を通すためのものではない。
const DIM_NORMAL_EPS: f64 = 1e-9;

/// 「2 方向が平行か」の許容（単位ベクトルの外積の絶対値）。
///
/// `1e-9` は角度にして約 6e-8 度。`50 = 90` の寸法で `cos(90°)` が `6.1e-17` に
/// なる程度の丸めは通し、実際に傾いている寸法（例: [`linear_dimension_to_geom`] の
/// doc が挙げる水平寸法の外積 0.137）は通さない。
const DIM_PARALLEL_EPS: f64 = 1e-9;

/// 実測値（group 42）と mcad の再計算値の一致判定に使う相対許容。
const DIM_MEASUREMENT_REL_EPS: f64 = 1e-6;

/// 寸法が mcad の XY 平面（法線 +Z）に載っているか。
///
/// mcad は 2D 専用で、[`EntityGeom`] の座標は常にワールド XY 平面上の点として
/// 解釈される。押し出し法線が `(0,0,1)` 以外の DIMENSION は、定義点が OCS
/// （object coordinate system）で書かれているため、そのまま WCS の座標として
/// 読むと**鏡像・回転した位置に置かれる**（典型は法線 `(0,0,-1)` で、X 軸が
/// 反転する）。逆変換して平面へ畳む方法は幾何的には書けるが、畳んだ結果は
/// 元の 3D 配置とは別物であり、「mcad が表現できないものは黙って変形せず
/// スキップして件数を通知する」既存方針（[`is_text_justification_supported`] の
/// doc）に合わせて**拒否する**。
///
/// 拒否した寸法は（対応している種別なのに座標が平面に載らないため表現できない、
/// という扱いなので）[`ImportSummary::skipped_dimensions`] に入る。
fn is_dimension_plane_supported(base: &DimensionBase) -> bool {
    let n = &base.normal;
    n.x.abs() <= DIM_NORMAL_EPS
        && n.y.abs() <= DIM_NORMAL_EPS
        && (n.z - 1.0).abs() <= DIM_NORMAL_EPS
}

/// DIMENSION 共通部（[`DimensionBase`]）から [`DimAnnotation`] を組む。
///
/// mcad のモデルで表現できない属性があれば `dropped_detail` を立てる
/// （[`ImportSummary::dropped_dimension_details`] へ 1 件として積まれる）。
///
/// # 写す
///
/// - **文字位置**: `is_at_user_defined_location`（group 70 の 128 ビット）が真なら、
///   文字位置が作図者の明示指定なので `text_mid_point`（group 11）を
///   [`DimAnnotation::text_anchor`] へ写す。偽なら DXF 側も自動配置なので `None`
///   （mcad 側も自動配置）。**group 11 は自動配置でも「そのとき文字が置かれた位置」
///   として書かれている**ため、常に写すと mcad の自動配置が効かなくなる。
///
/// # 捨てる（`dropped_detail` を立てる）
///
/// - **文字テンプレート**（group 1）: 空文字列と `<>` が「実測値を描く」＝ mcad の
///   既定と同じなので何もしない。それ以外（スペース 1 文字の「文字を出さない」指定、
///   `<>` を含むテンプレート、任意の固定文字列）は M11 タスク70 の
///   `prefix` / `suffix` / `value_override` が入るまで表現できないので、
///   **無注記で取り込んで計上する**（DESIGN.md M11-0「group 1 の扱い」）。
/// - **文字の回転**（group 53）: [`DimAnnotation`] に `text_rotation` が入るのは
///   M11 タスク70。
///
/// # 黙って捨てる（計上しない）
///
/// - **DIMSTYLE 名**（group 3）: mcad は文書に 1 つの [`mcad_core::DimStyle`] しか
///   持たない（M9 設計判断）。外部 DXF はほぼ必ず名前付きスタイルを参照するため、
///   計上すると全寸法が警告になりノイズにしかならない。色の ACI 近似・未知線種の
///   `Continuous` フォールバックと同じ「確定的に解釈する」扱い。
/// - **文字の寄せ**（group 71 `attachment_point`）・**行間**（41/72）: mcad の
///   組版は寸法線に対する位置を自前で決める（`mcad_core::expand_dim_linear` 等）。
/// - **anonymous block 参照**（group 2、`*D1` 等）: mcad は BLOCKS セクションを
///   読まないため実害がない（モジュール doc「DIMENSION の import」参照）。
fn dimension_annotation(base: &DimensionBase, dropped_detail: &mut bool) -> DimAnnotation {
    // 空 / "<>" は「実測値を描く」。それ以外は M11 タスク70 まで表現できない。
    if !(base.text.is_empty() || base.text == "<>") {
        *dropped_detail = true;
    }
    if base.text_rotation_angle != 0.0 {
        *dropped_detail = true;
    }
    DimAnnotation {
        text_anchor: base
            .is_at_user_defined_location
            .then(|| from_dxf_point(&base.text_mid_point)),
        ..DimAnnotation::unannotated()
    }
}

/// 実測値（group 42）が書かれていて、mcad の再計算値と食い違うかを判定する。
///
/// mcad は寸法値を**保存せず毎回座標から計算する**（DESIGN.md M6 設計判断2）ので、
/// 採用するのは常に再計算値のほうである。group 42 は「書いた CAD が表示していた値」
/// なので、両者の食い違いは**定義点の解釈が食い違っているサイン**になる。典型的な
/// 原因は DIMSTYLE の測定倍率（DIMLFAC）で、mcad には測定倍率の概念が無いため
/// 表現できない。このケースは**幾何は平行のまま値だけ食い違う**ため、回転寸法の
/// 平行判定（[`linear_dimension_to_geom`] の doc）では防げない。取り込むと DXF が
/// 持っていた値と違う値を黙って表示することになるため、食い違う寸法は
/// **取り込まずスキップする**（呼び出し側が `true` を見て `None` を返す。
/// [`ImportSummary::skipped_dimensions`] へ計上される）。
///
/// group 42 は省略されることが多く（実測: libdxfrw 0.6.3 = LibreCAD は書かない）、
/// クレートの既定値は `0.0` である。**省略と「値が 0」は区別できない**ため、`0` 以下は
/// 「書かれていない」とみなして比較しない（このときこの検査は効かず、寸法は
/// 無条件で取り込まれる。寸法値 0 自体が退化している）。
///
/// `expected` は DXF の group 42 と同じ量で渡すこと（直径寸法なら**直径**、
/// 半径寸法なら半径、長さ寸法なら長さ）。
fn actual_measurement_mismatches(base: &DimensionBase, expected: f64) -> bool {
    let actual = base.actual_measurement;
    if actual <= 0.0 || !actual.is_finite() {
        return false;
    }
    (actual - expected).abs() > DIM_MEASUREMENT_REL_EPS * actual.abs()
}

/// `AcDbAlignedDimension` / `AcDbRotatedDimension`（クレートでは同じ
/// [`RotatedDimension`]）を [`EntityGeom::DimLinear`] へ写す。
///
/// - `definition_point_2`（group 13）・`definition_point_3`（group 14）= 計測 2 点。
/// - `dimension_base.definition_point_1`（group 10）= 寸法線の位置。
///   [`mcad_core::DimLinear::offset`] は「計測線 `p1→p2` の法線方向に、寸法線まで
///   取った符号付き距離」なので、`offset = (p10 − p1)・perp((p2−p1)/|p2−p1|)` で求まる
///   （`perp` は左 90 度回転。`mcad_core::expand` の `linear_frame` が
///   `p1 + perp(dir) * offset` で寸法線端を作るのと同じ式）。
///
/// # 回転寸法（`DimensionType::RotatedHorizontalOrVertical`）の受入条件
///
/// 回転寸法の値は「計測 2 点を group 50 の方向へ**投影した長さ**」であり、
/// [`mcad_core::DimLinear`] が描く `|p2 − p1|` とは一般に一致しない。向きを保存できる
/// `DimDirection` が入るのは M11 タスク68 なので、ここでは
/// **投影長と実距離が一致する回転寸法、すなわち計測 2 点の向きが group 50 の方向と
/// 平行なものだけ**を受け入れる（このとき整列寸法と描画が完全に同一になる）。
/// 平行でないものは [`ImportSummary::skipped_dimensions`] へ計上してスキップする。
///
/// 実測（ユーザーが LibreCAD = libdxfrw 0.6.3 で描いた図面。fixture は同じ構造の合成データ）:
///
/// - 斜辺 `(115, 163.75)`–`(243.75, 146)` に付けた**水平**寸法（group 50 省略 = 0）は
///   単位ベクトルの外積が `0.137` で平行でない。整列寸法として取り込むと表示値が
///   `128.75` から `129.968` へ変わってしまうのでスキップする。
/// - 垂直な辺に付けた**鉛直**寸法（group 50 = 90）は平行なので受け入れる。
///   `cos(90°)` の丸め（`6.1e-17`）は [`DIM_PARALLEL_EPS`] が吸収する。
///
/// つまり「角度が 0 か」ではなく「回転寸法が整列寸法と同じ図になるか」で判定する。
/// 水平／鉛直という**よくある向きでも、計測 2 点がその向きに並んでいなければ
/// 投影が効いている**ため受け入れられない。
///
/// # そのほかの拒否条件
///
/// - `horizontal_direction_angle`（group 51）が 0 でない回転寸法。51 は寸法の
///   水平方向（UCS の X 軸）を回す指定で、group 50 がどの基準からの角度になるかが
///   変わる。誤った向きで平行判定をするより拒否するほうが安全（整列寸法は寸法線の
///   向きを計測 2 点だけで決めるので、51 は文字の姿勢にしか効かず拒否しない）。
/// - 計測 2 点がほぼ同一（法線が決まらず `offset` を定義できない）。
/// - 押し出し法線が +Z でない（[`is_dimension_plane_supported`]）。
/// - `dimension_type` が整列でも回転でもない（クレートは group 100 のサブクラス名で
///   型を決めるため、group 70 と食い違う DXF はここへ来うる）。
/// - 実測値（group 42）が書かれていて、mcad の再計算値と食い違う
///   （[`actual_measurement_mismatches`]。典型的には DIMSTYLE の測定倍率 DIMLFAC が
///   原因で、幾何が平行のままでも起きるため上記の平行判定では防げない）。
fn linear_dimension_to_geom(
    dim: &RotatedDimension,
    dropped_detail: &mut bool,
) -> Option<EntityGeom> {
    let base = &dim.dimension_base;
    if !is_dimension_plane_supported(base) {
        return None;
    }
    let p1 = from_dxf_point(&dim.definition_point_2);
    let p2 = from_dxf_point(&dim.definition_point_3);
    let dir = (p2 - p1).normalize()?;
    match base.dimension_type {
        DimensionType::Aligned => {}
        DimensionType::RotatedHorizontalOrVertical => {
            if base.horizontal_direction_angle != 0.0 {
                return None;
            }
            let (sin, cos) = dim.rotation_angle.to_radians().sin_cos();
            if (dir.x * sin - dir.y * cos).abs() > DIM_PARALLEL_EPS {
                return None;
            }
        }
        _ => return None,
    }
    if actual_measurement_mismatches(base, (p2 - p1).length()) {
        return None;
    }
    if dim.extension_line_angle != 0.0 {
        *dropped_detail = true;
    }
    let offset = (from_dxf_point(&base.definition_point_1) - p1).dot(dir.perp());
    Some(EntityGeom::DimLinear(DimLinear {
        p1,
        p2,
        offset,
        annotation: dimension_annotation(base, dropped_detail),
    }))
}

/// `AcDbRadialDimension` を [`EntityGeom::DimRadial`] へ写す。
///
/// `dimension_base.definition_point_1`（group 10）= 円の中心、`definition_point_2`
/// （group 15）= 円周上の点。半径は 2 点間の距離、
/// [`mcad_core::DimRadial::leader_angle`] は中心から円周点を向く放射角。
///
/// 実測（`tests/fixtures/synthetic_dimensions.dxf`）: 中心・半径とも図面中の `CIRCLE`
/// （中心 `(144.2322775263952, 137)`・半径 `13.4551940975304`）と一致する。
///
/// 引出線長（group 40 `leader_length`）は mcad が描画時に決めるため使わない
/// （計上もしない。[`dimension_annotation`] の doc 参照）。半径 0（中心と円周点が
/// 同一）は [`EntityGeom::validate`] が拒否するので、呼び出し側でスキップに回る。
/// group 42（実測値）が半径と食い違えば取り込まずスキップする
/// （[`actual_measurement_mismatches`]）。
fn radial_dimension_to_geom(
    dim: &RadialDimension,
    dropped_detail: &mut bool,
) -> Option<EntityGeom> {
    let base = &dim.dimension_base;
    if !is_dimension_plane_supported(base) {
        return None;
    }
    let center = from_dxf_point(&base.definition_point_1);
    let rim = from_dxf_point(&dim.definition_point_2);
    let spoke = rim - center;
    if actual_measurement_mismatches(base, spoke.length()) {
        return None;
    }
    Some(EntityGeom::DimRadial(DimRadial {
        center,
        radius: spoke.length(),
        leader_angle: spoke.angle(),
        annotation: dimension_annotation(base, dropped_detail),
    }))
}

/// `AcDbDiametricDimension` を [`EntityGeom::DimDiameter`] へ写す。
///
/// `dimension_base.definition_point_1`（group 10）と `definition_point_2`
/// （group 15）は**直径の両端**（DXF Reference の言い方では 10 が「15 の反対側の点」）。
/// したがって中心は 2 点の中点、[`mcad_core::DimDiameter::radius`] は 2 点間の
/// 距離の半分、`angle` は 10 → 15 の向き（直径線は逆向きも同じ線なので、どちらを
/// 向けても描画は同じ）。
///
/// 実測（`tests/fixtures/synthetic_dimensions.dxf`）: 中点が図面中の `CIRCLE` の中心と、
/// 2 点間距離が直径（`2 × 13.4551940975304`）と一致する。
///
/// group 42（実測値）との比較は**直径**で行う（DXF の寸法値は直径寸法なら直径）。
/// 食い違えば取り込まずスキップする（[`actual_measurement_mismatches`]）。
fn diameter_dimension_to_geom(
    dim: &DiameterDimension,
    dropped_detail: &mut bool,
) -> Option<EntityGeom> {
    let base = &dim.dimension_base;
    if !is_dimension_plane_supported(base) {
        return None;
    }
    let far = from_dxf_point(&base.definition_point_1);
    let near = from_dxf_point(&dim.definition_point_2);
    let across = near - far;
    if actual_measurement_mismatches(base, across.length()) {
        return None;
    }
    Some(EntityGeom::DimDiameter(DimDiameter {
        center: far.midpoint(near),
        radius: across.length() * 0.5,
        angle: across.angle(),
        annotation: dimension_annotation(base, dropped_detail),
    }))
}

/// [`dxf_entity_to_geom`] の結果。
///
/// エンティティを取り込めたかどうか（`Option`）とは別に、**取り込めたが一部の属性を
/// 捨てた**ことを呼び出し側（[`import_dxf`]）へ伝えるために、ジオメトリと
/// 「詳細を捨てたか」をまとめて返す。現状これが立つのは寸法だけなので、
/// [`import_dxf`] は [`ImportSummary::dropped_dimension_details`] へ積む。
struct ImportedGeom {
    geom: EntityGeom,
    dropped_detail: bool,
}

/// [`dxf_entity_to_geom`] が変換できなかったときの理由。呼び出し側
/// （[`import_dxf`]）が [`ImportSummary::skipped_entities`] と
/// [`ImportSummary::skipped_dimensions`] のどちらへ計上するかをこれで判定する。
enum DxfImportSkip {
    /// `EntityType` 自体が非対応（3 点角度寸法・座標寸法・SPLINE・ELLIPSE 等）、
    /// または寸法以外（Shape・TEXT）で [`EntityGeom::validate`] が拒否する不正な
    /// ジオメトリ。[`ImportSummary::skipped_entities`] へ計上する。
    UnsupportedOrInvalid,
    /// **対応している**寸法種別（整列・回転・半径・直径）だが、mcad のモデルでは
    /// 測定値や位置を変えずに表現できないため取り込まなかった。「種別が未対応」
    /// ではないので [`ImportSummary::skipped_dimensions`] へ計上する（詳細は
    /// [`ImportSummary::skipped_dimensions`] の doc）。
    Dimension,
}

/// DXF エンティティの `specific` を [`EntityGeom`] へ変換する。
///
/// Shape 系（POINT / LINE / CIRCLE / ARC / LWPOLYLINE）に加え、TEXT を
/// [`EntityGeom::Text`] へ変換する（タスク25b、[`text_to_dxf_entity`] の逆写像）。
/// TEXT のうち位置基準が Left/Baseline 以外のものは変換せず
/// `Err(DxfImportSkip::UnsupportedOrInvalid)` を返す（理由は
/// [`is_text_justification_supported`] の doc）。
///
/// DIMENSION は整列・回転（整列と同じ図になるものだけ）・半径・直径の 4 経路を
/// [`EntityGeom`] の `DimLinear` / `DimRadial` / `DimDiameter` へ写す
/// （M11 タスク67、[`linear_dimension_to_geom`] / [`radial_dimension_to_geom`] /
/// [`diameter_dimension_to_geom`]）。この 4 経路が個別の理由で変換できなかった
/// 場合は `Err(DxfImportSkip::Dimension)`（種別自体は対応しているので
/// [`ImportSummary::skipped_dimensions`] 側）。3 点角度寸法・座標寸法はモデルが
/// 無いので `Err(DxfImportSkip::UnsupportedOrInvalid)`、2 直線角度寸法・弧長寸法は
/// そもそもクレートが読まない（モジュール doc「DIMENSION の import」参照）。
///
/// 対応していない `EntityType`、または非有限座標・負半径・非正の文字高さなど
/// [`EntityGeom::validate`] が拒否する不正なジオメトリは `Err` を返す
/// （呼び出し側で「無視」としてカウントする）。`Shape` バリアントの判定基準は
/// `.mcad` import（[`crate::mcad_file::import_document`]）と同一であり、
/// `EntityGeom::validate` が `Shape::validate` へ委譲するため divergence しない。
fn dxf_entity_to_geom(specific: &EntityType) -> Result<ImportedGeom, DxfImportSkip> {
    let mut dropped_detail = false;
    let geom: EntityGeom = match specific {
        EntityType::ModelPoint(p) => Shape::Point(from_dxf_point(&p.location)).into(),
        EntityType::Line(l) => {
            Shape::Line(LineSeg::new(from_dxf_point(&l.p1), from_dxf_point(&l.p2))).into()
        }
        EntityType::Circle(c) => {
            Shape::Circle(Circle::new(from_dxf_point(&c.center), c.radius)).into()
        }
        EntityType::Arc(a) => Shape::Arc(Arc::new(
            from_dxf_point(&a.center),
            a.radius,
            a.start_angle.to_radians(),
            a.end_angle.to_radians(),
        ))
        .into(),
        EntityType::LwPolyline(pl) => Shape::Polyline(Polyline::new(
            pl.vertices.iter().map(|v| Point2::new(v.x, v.y)).collect(),
            pl.is_closed(),
        ))
        .into(),
        // TEXT: text_to_dxf_entity の逆写像。`rotation` は度なのでラジアンへ戻す。
        // 書体名（`text_style_name`）・各種寸法比は mcad の TextGeom に対応する
        // フィールドがないため捨てる（DESIGN.md M6 設計判断5）。
        //
        // 位置基準（group code 72 / 73）が Left/Baseline 以外の TEXT は `location` が
        // 文字位置を持たないため、そのまま anchor に入れると誤配置になる。io 層では
        // 正しく逆算できないのでスキップする（is_text_justification_supported の doc）。
        EntityType::Text(t) => {
            if !is_text_justification_supported(t) {
                return Err(DxfImportSkip::UnsupportedOrInvalid);
            }
            EntityGeom::Text(TextGeom {
                anchor: from_dxf_point(&t.location),
                content: t.value.clone(),
                height: t.text_height,
                angle: t.rotation.to_radians(),
            })
        }
        // DIMENSION（M11 タスク67）。クレートは group 100 のサブクラス名で型を分け、
        // 整列寸法と回転寸法はどちらも `RotatedDimension` になる（区別は
        // `dimension_base.dimension_type`）。これらの 4 経路は「種別としては対応
        // している」ので、個別の理由で変換できなかった場合は
        // `DxfImportSkip::Dimension`（skipped_dimensions）へ回す。
        EntityType::RotatedDimension(d) => {
            linear_dimension_to_geom(d, &mut dropped_detail).ok_or(DxfImportSkip::Dimension)?
        }
        EntityType::RadialDimension(d) => {
            radial_dimension_to_geom(d, &mut dropped_detail).ok_or(DxfImportSkip::Dimension)?
        }
        EntityType::DiameterDimension(d) => {
            diameter_dimension_to_geom(d, &mut dropped_detail).ok_or(DxfImportSkip::Dimension)?
        }
        // 3 点角度寸法・座標寸法（種別自体が非対応）はここへ来る。
        _ => return Err(DxfImportSkip::UnsupportedOrInvalid),
    };
    if geom.validate().is_ok() {
        Ok(ImportedGeom {
            geom,
            dropped_detail,
        })
    } else if matches!(
        geom,
        EntityGeom::DimLinear(_) | EntityGeom::DimRadial(_) | EntityGeom::DimDiameter(_)
    ) {
        // 変換自体は成功したが、構築後の検証で拒否された寸法（例: 半径・直径寸法の
        // 半径 0）。種別は対応しているため skipped_dimensions 側（モジュール doc
        // 「落ちる情報」参照）。
        Err(DxfImportSkip::Dimension)
    } else {
        Err(DxfImportSkip::UnsupportedOrInvalid)
    }
}

/// [`TextGeom`] を DXF `TEXT` エンティティへ変換する（タスク25、高さの尺度契約は
/// タスク35c）。
///
/// `anchor` → `location`、`angle`（ラジアン）→ `rotation`（度）をそのまま
/// マッピングする。1 書体のみのため `text_style_name` は既定（`STANDARD`）の
/// ままにする（DESIGN.md M6 設計判断5）。
///
/// `height`（紙 mm）→ `text_height`（DXF モデル空間の長さ）は **`height * k`**
/// を書く（`k` は呼び出し側が図面の [`mcad_core::SheetMeta::scale`] から渡す
/// `Scale::world_mm_per_paper_mm()`。DESIGN.md M8 設計判断4a、モジュール doc
/// 「TEXT 高さの尺度契約」参照）。逆写像（import 側は 1:1 とみなす非対称な写像）は
/// [`dxf_entity_to_geom`]。
fn text_to_dxf_entity(text: &TextGeom, world_mm_per_paper_mm: f64) -> DxfEntity {
    let specific = EntityType::Text(DxfText {
        location: to_dxf_point(text.anchor),
        text_height: text.height * world_mm_per_paper_mm,
        value: text.content.clone(),
        rotation: text.angle.to_degrees(),
        ..Default::default()
    });
    DxfEntity::new(specific)
}

/// [`TableGeom`]（表・M10）を DXF の `LINE`（罫線）・`TEXT`（セル文字）へ**分解**する
/// （タスク61、DESIGN.md M10 設計方針6・詳細設計8）。
///
/// DXF に「表」に相当するプリミティブが無いため、[`crate::mcad_file`] のような
/// ポータブル DTO 経由ではなく、罫線とセル文字の座標を組んでから書く。
///
/// # 組版は `mcad-core` の [`expand_table`] が唯一の出所（M11 タスク71）
///
/// M10 タスク61 の時点では展開が `mcad-app` にあり、`mcad-io` は `mcad-app` へ依存
/// できない（アーキテクチャ不変条件、依存方向は app → io → core → geom）ため
/// **同じ組版規則をここへ独立に実装していた**。値が乖離しても検出できない残債だった
/// ので、M11 設計判断1 で展開を `mcad-core` へ移し、この関数は
/// [`expand_table`] の結果を DXF エンティティへ写すだけになった。セル内パディングの
/// 定数（`CELL_TEXT_PAD_MM`）の複製もここで消えている。したがって
/// **画面・SVG/PDF・DXF の罫線位置とセル文字位置は定義上一致する**。
///
/// - 罫線の順序は [`expand_table`] のまま（外枠4辺 →（上から）行境界 →
///   （左から）列境界）。`LINE` をすべて出してから `TEXT` を出す。
/// - 罫線の太さ（外枠 0.5mm・内部 0.13mm の区別、
///   [`mcad_core::TableSegment::width_mm`]）は
///   **書かない**（[`export_dxf`] が呼び出し側でエンティティの [`Style::width_mm`] を
///   全罫線へ一律に適用するため、表題欄と同じ「太い枠・細い仕切り」の描き分けは DXF には
///   残らない。dxf 0.6.1 のレイヤー線幅と同じ「クレート側 API ではなく mcad の
///   設計上の割り切りとして保存されない」制約としてモジュール doc に記録する）。
/// - セル文字の高さは [`text_to_dxf_entity`] と同じ尺度契約（`text_height_mm * k`）。
/// - **空セルは出さない**・**退化した表（行数・列数が 0）は空を返す**のは
///   [`expand_table`] の性質をそのまま受け継ぐ（`EntityGeom::validate` を通していない
///   値が渡っても panic しない全域関数）。
///
/// この分解は **非対称往復**（import しても表エンティティへは戻らない。モジュール doc
/// 「未対応エンティティ・不正ジオメトリの扱い（import）」および AGENTS.md 参照）。
fn table_to_dxf_entities(table: &TableGeom, world_mm_per_paper_mm: f64) -> Vec<DxfEntity> {
    let expansion = expand_table(table, world_mm_per_paper_mm);

    let lines = expansion.segments.iter().map(|seg| {
        DxfEntity::new(EntityType::Line(DxfLine {
            p1: to_dxf_point(seg.a),
            p2: to_dxf_point(seg.b),
            ..Default::default()
        }))
    });
    // セル文字の `height` は紙 mm（`expand_table` の契約）なので、TEXT と同じく `× k`
    // してモデル空間長へ直す（[`text_to_dxf_entity`] と同一の写像）。
    let texts = expansion
        .texts
        .iter()
        .map(|text| text_to_dxf_entity(text, world_mm_per_paper_mm));

    lines.chain(texts).collect()
}

/// `Document` を `dxf::Drawing` へ変換する。
///
/// 生存中のレイヤー・エンティティのみを列挙する（undo/redo 履歴は含めない）。
/// Text（[`EntityGeom::Text`]）は DXF `TEXT` エンティティとして export する
/// （タスク25）。寸法（[`EntityGeom`] の `DimLinear` / `DimRadial`）は DXF に
/// 対応するプリミティブがなくスキップし、その件数を
/// [`ExportSummary::skipped_entities`] に積む。
#[must_use]
pub fn export_dxf(doc: &Document) -> ExportSummary {
    let mut drawing = Drawing::new();

    // ヘッダバージョンは R2007。下限が上下 2 つの理由から挟まれており、**下げてはいけない**。
    //
    // ## R14 未満では LWPOLYLINE が黙って落ちる
    //
    // LWPOLYLINE（`EntityType::LwPolyline`）は AutoCAD R14 以降のエンティティで、
    // `dxf` クレートは書き出し時に `if version >= AcadVersion::R14` でガードしている
    // （生成コード `build/entity_generator.rs`）。クレート既定の R12 のままだと
    // ポリラインが **エラーにもならず消える**。
    //
    // ## R2004 以下では文字列 codec が往復でデータを壊す
    //
    // `dxf` 0.6.1 は `$ACADVER <= R2004` のとき文字列を `\U+XXXX`（4桁大文字16進）へ
    // エスケープして書き（`escape_unicode_to_ascii`）、読込時は `$ACADVER < R2007` を
    // WINDOWS_1252 と見なして `un_escape_ascii_to_unicode` でアンエスケープする。
    // この codec には往復でデータを壊す欠陥が 3 つあり、いずれも実測で確認した
    // （2026-07-25、Codex レビューの high 指摘が契機）:
    //
    // 1. **非BMP文字**: `😀`（U+1F600）は `\U+1F600` と書かれるが、デコーダが 16 進を
    //    4 桁ちょうどしか消費しないため `ὠ`（U+1F60）+ `0` の 2 文字に化ける。
    // 2. **末尾バックスラッシュ**: `path\` の末尾 `\` はエスケープ開始と誤認され、
    //    未完のシーケンスが flush されないまま捨てられて消える。
    // 3. **リテラル `\U+XXXX`**: 書き出し側がバックスラッシュを二重化しないため、
    //    ユーザーが入力した 7 文字の `\U+0041` が読込で `A` 1 文字に化ける。
    //
    // ## R2007 を選ぶ理由
    //
    // `$ACADVER >= R2007` なら書き出しは `text_as_ascii = false`、読込は
    // `read_as_utf8()` で UTF-8 経路に入り、上記エスケープを一切通らないため
    // 3 ケースすべてが往復一致する（`tests::round_trip_preserves_pathological_text`
    // で固定）。R2010 / R2013 / R2018 でも同じ UTF-8 経路だが、**UTF-8 経路に入る
    // 最小のバージョン**を採るのが保守的（読める CAD ソフトの範囲が最も広い）と
    // 判断して R2007 とする。DESIGN.md M6 設計判断5 参照。
    drawing.header.version = dxf::enums::AcadVersion::R2007;

    // 単位は mm 固定（DESIGN.md M8 設計判断1、モジュール doc「単位（$INSUNITS）」）。
    drawing.header.default_drawing_units = DxfUnits::Millimeters;

    // LTYPE テーブルへ mcad の 4 線種を簡易パターンで登録する（モジュール doc
    // 「線幅・線種は best-effort」参照）。レイヤー・エンティティの
    // `line_type_name` はこの名前を参照する。
    for line_type in dxf_line_type_defs() {
        drawing.add_line_type(line_type);
    }

    // `Drawing::new()` が自動追加するレイヤー（モジュール doc 参照）をすべて
    // 取り除き、`Document` のレイヤーだけで組み直す。
    while drawing.remove_layer(0).is_some() {}

    // レイヤーは「デフォルトレイヤーを先頭に固定し、残りを mcad の重ね順
    // （layers_in_order、奥→手前の昇順）で続ける」順で書き出す（タスク41の
    // Codex adversarial review [high] 指摘対応）。
    //
    // なぜ先頭固定が必要か: import 側は昔から「LAYER テーブルの先頭 = mcad の
    // デフォルトレイヤー（削除できないレイヤー）」という規則で `Document` を
    // 再構築する（下の import_dxf 参照）。これは layers_in_order 順で書いていた
    // 旧実装では、デフォルトレイヤーより order が小さい（奥の）レイヤーが
    // 存在すると破れる: そちらが先頭に来てしまい、import 時に「デフォルトレイヤー
    // が入れ替わる」（削除できないはずのレイヤーが変わる）という実害のある
    // バグになる。デフォルトを先頭へ固定すればこの規則は常に保たれる。
    //
    // 代償: デフォルトレイヤー自身の重ね順（order）は先頭に固定されるため、
    // DXF 往復では復元できない（re-import 時に index 0 → order = 0 になる）。
    // モジュール doc「レイヤーの重ね順は保存されない」が述べるとおり、DXF の
    // 重ね順往復はもともと best-effort で保証しないため、これは許容する
    // （デフォルト以外のレイヤーの相対順序は従来どおり保たれる）。
    let mut layer_names: HashMap<LayerId, String> = HashMap::new();
    let default_id = doc.default_layer();
    let ordered_ids: Vec<LayerId> = std::iter::once(default_id)
        .chain(
            doc.layers_in_order()
                .into_iter()
                .map(|(id, _)| id)
                .filter(|id| *id != default_id),
        )
        .collect();
    for id in ordered_ids {
        let layer = doc.layer(id).expect("Document invariant: layer is alive");
        layer_names.insert(id, layer.name.clone());
        drawing.add_layer(DxfLayer {
            name: layer.name.clone(),
            color: rgb_to_aci(layer.color),
            is_layer_on: layer.visible,
            line_type_name: linetype_to_dxf_name(layer.linetype).to_string(),
            // レイヤーの線幅（`width_mm`）は書けない: `dxf::tables::Layer::line_weight`
            // は不透明な `LineWeight` 型で、任意値を作れる公開コンストラクタが
            // ないため（実測、モジュール doc「線幅・線種は best-effort」参照）。
            // 既定（raw 0）のまま export し、往復では「未指定」として
            // `WidthMm::DEFAULT`（0.35mm）へ復元される（クランプ計上はしない。
            // 理由は `dxf_layer_lineweight_to_width_mm` のdoc参照）。
            ..Default::default()
        });
    }

    // ベストエフォート: $CLAYER にカレントレイヤー名を書いておく（import 側で
    // 復元を試みる。モジュール doc の「カレントレイヤー」を参照）。
    if let Some(name) = layer_names.get(&doc.current_layer()) {
        drawing.header.current_layer = name.clone();
    }

    // TEXT 高さの尺度契約（DESIGN.md M8 設計判断4a）: `height * k` を書く。
    let world_mm_per_paper_mm = doc.sheet().scale.world_mm_per_paper_mm();

    let mut skipped_entities = 0usize;
    for (_, entity) in doc.entities() {
        let layer_name = layer_names
            .get(&entity.layer)
            .expect("Document invariant: entity's layer must be alive")
            .clone();
        // M6: Shape・Text は DXF エンティティへ変換する（タスク25で Text も対応）。
        // 表（M10 タスク61）は複数の LINE/TEXT へ分解する（[`table_to_dxf_entities`]）。
        // 寸法（DimLinear/DimRadial）は DXF に対応するプリミティブがないためスキップし
        // 件数を数える。件数は呼び出し側がステータス表示し、無警告のデータロスを防ぐ。
        let dxf_entities: Vec<DxfEntity> = match &entity.geom {
            EntityGeom::Shape(shape) => vec![shape_to_dxf_entity(shape)],
            EntityGeom::Text(text) => vec![text_to_dxf_entity(text, world_mm_per_paper_mm)],
            EntityGeom::Table(table) => table_to_dxf_entities(table, world_mm_per_paper_mm),
            // 寸法（DimLinear/DimRadial）と、将来 core へ追加される未知の幾何
            // （`EntityGeom` は `#[non_exhaustive]`）はまとめてスキップ側に回す。
            _ => {
                skipped_entities += 1;
                continue;
            }
        };
        for mut dxf_entity in dxf_entities {
            dxf_entity.common.layer = layer_name.clone();
            dxf_entity.common.color = style_color_to_dxf(entity.style.color);
            // 線種・線幅は ByLayer（`Style` の該当フィールドが `None`）なら DXF の
            // 既定（`line_type_name = "BYLAYER"`）のままにする。線幅の既定
            // （`lineweight_enum_value = 0`）は「明示的に 0」を意味してしまうため、
            // ByLayer を表す `-1`（group code 370 の慣例）を明示的に書く必要がある。
            if let Some(linetype) = entity.style.linetype {
                dxf_entity.common.line_type_name = linetype_to_dxf_name(linetype).to_string();
            }
            dxf_entity.common.lineweight_enum_value = entity
                .style
                .width_mm
                .map_or(-1, width_mm_to_dxf_lineweight_raw);
            drawing.add_entity(dxf_entity);
        }
    }

    ExportSummary {
        drawing,
        skipped_entities,
    }
}

/// 未知のレイヤー名を解決する。既知なら既存 `LayerId` を返し、未知ならその場で
/// `AddLayer` して登録する（モジュール doc の「未知のレイヤー名を参照する
/// エンティティ」を参照）。
fn resolve_layer(
    doc: &mut Document,
    layer_ids: &mut HashMap<String, LayerId>,
    name: &str,
) -> Result<LayerId, IoError> {
    if let Some(id) = layer_ids.get(name) {
        return Ok(*id);
    }
    let new_ids = doc.apply(Command::AddLayer(Layer::new(name, Rgb::WHITE)))?;
    let id = new_ids.layers[0];
    layer_ids.insert(name.to_string(), id);
    Ok(id)
}

/// `dxf::Drawing` から [`Document`] を再構築する。
///
/// 再構築の手順は [`crate::mcad_file::import_document`] と同じ規律に従う:
/// [`Document::new`] から [`Command`] 列で組み立て、完了後
/// [`Document::clear_history`] を呼ぶ。未対応エンティティ・不正ジオメトリは
/// 無視して [`ImportSummary::skipped_entities`] に積む（モジュール doc 参照）。
/// TEXT は [`EntityGeom::Text`] として復元する（タスク25b）。ただし位置基準が
/// Left/Baseline 以外の TEXT はスキップ側に回る（理由は
/// `is_text_justification_supported` の doc）。
///
/// # Errors
///
/// 再構築コマンドをコアが拒否した場合（バリデーション通過後は起きない想定の
/// 防御的経路）に [`IoError::Core`] を返す。
pub fn import_dxf(drawing: &Drawing) -> Result<ImportSummary, IoError> {
    let mut doc = Document::new();
    let mut layer_ids: HashMap<String, LayerId> = HashMap::new();
    let mut clamped_line_widths = 0usize;

    let dxf_layers: Vec<&DxfLayer> = drawing.layers().collect();
    for (index, layer) in dxf_layers.iter().enumerate() {
        // レイヤーの線幅は「読める」（`LineWeight::raw_value()` は公開）ので、
        // 他 CAD が書いた値も含めてベストエフォートで取り込む（export 側は
        // 書けない非対称な制約。モジュール doc「線幅・線種は best-effort」参照）。
        let (width_mm, width_clamped) =
            dxf_layer_lineweight_to_width_mm(layer.line_weight.raw_value());
        if width_clamped {
            clamped_line_widths += 1;
        }
        let mcad_layer = Layer {
            name: layer.name.clone(),
            color: dxf_color_to_style(&layer.color).unwrap_or(ACI_FALLBACK_RGB),
            linetype: dxf_name_to_linetype(&layer.line_type_name),
            width_mm,
            visible: layer.is_layer_on,
            // dxf::tables::Layer にロック状態のフィールドがないため常に未ロックで
            // 復元する（モジュール doc「レイヤーロックは保存されない」参照）。
            locked: false,
            // DXF に重ね順の概念はないため、LAYER テーブルの並び順を重ね順として
            // 採用する（未規定の反復順に依存しない決定的な規則）。
            order: i32::try_from(index).unwrap_or(i32::MAX),
        };
        if index == 0 {
            let id = doc.default_layer();
            doc.apply(Command::SetLayerProps {
                id,
                props: mcad_layer,
            })?;
            layer_ids.insert(layer.name.clone(), id);
        } else {
            let new_ids = doc.apply(Command::AddLayer(mcad_layer))?;
            layer_ids.insert(layer.name.clone(), new_ids.layers[0]);
        }
    }
    // LAYER テーブルが空の DXF（テーブルを省略したミニマルなファイル）でも、
    // `Document` は常にデフォルトレイヤー "0" を持つ。これを layer_ids に
    // 登録しておかないと、後続の resolve_layer が "0" 参照のエンティティに
    // 対して重複するレイヤーを作ってしまう。
    if dxf_layers.is_empty() {
        let default_id = doc.default_layer();
        let default_name = doc
            .layer(default_id)
            .expect("default layer exists")
            .name
            .clone();
        layer_ids.insert(default_name, default_id);
    }

    let mut skipped_entities = 0usize;
    let mut skipped_dimensions = 0usize;
    let mut dropped_dimension_details = 0usize;
    for entity in drawing.entities() {
        let (geom, dropped_detail) = match dxf_entity_to_geom(&entity.specific) {
            Ok(ImportedGeom {
                geom,
                dropped_detail,
            }) => (geom, dropped_detail),
            Err(DxfImportSkip::UnsupportedOrInvalid) => {
                skipped_entities += 1;
                continue;
            }
            Err(DxfImportSkip::Dimension) => {
                skipped_dimensions += 1;
                continue;
            }
        };
        if dropped_detail {
            dropped_dimension_details += 1;
        }
        let layer_id = resolve_layer(&mut doc, &mut layer_ids, &entity.common.layer)?;
        let (width_mm, width_clamped) =
            dxf_lineweight_to_style_width(entity.common.lineweight_enum_value);
        if width_clamped {
            clamped_line_widths += 1;
        }
        let style = Style {
            color: dxf_color_to_style(&entity.common.color),
            width_mm,
            linetype: dxf_line_type_name_to_style_linetype(&entity.common.line_type_name),
        };
        doc.apply(Command::AddEntity(Entity::new(geom, layer_id, style)))?;
    }

    // ベストエフォート: $CLAYER に対応するレイヤーが見つかればカレントに設定する。
    if let Some(&id) = layer_ids.get(&drawing.header.current_layer) {
        doc.apply(Command::SetCurrentLayer(id))?;
    }

    doc.clear_history();
    Ok(ImportSummary {
        document: doc,
        skipped_entities,
        skipped_dimensions,
        clamped_line_widths,
        dropped_dimension_details,
    })
}

/// ドキュメントを DXF ファイルへ保存する。
///
/// 戻り値は DXF 非対応でスキップしたエンティティ数（寸法。Text はタスク25で
/// export 対応済みのためスキップされない）で、呼び出し側がステータス表示に使う
/// （[`ExportSummary`] 参照）。
///
/// # Errors
///
/// DXF への書き出し失敗時に [`IoError::Dxf`] を返す。
pub fn save_dxf(doc: &Document, path: impl AsRef<Path>) -> Result<usize, IoError> {
    let ExportSummary {
        drawing,
        skipped_entities,
    } = export_dxf(doc);
    drawing.save_file(path)?;
    Ok(skipped_entities)
}

/// DXF ファイルからドキュメントを読み込む。
///
/// # Errors
///
/// 読込・DXF 構文解析の失敗時に [`IoError::Dxf`] を返す。未対応エンティティ・
/// 不正ジオメトリはエラーにせず [`ImportSummary::skipped_entities`] に積む。
pub fn load_dxf(path: impl AsRef<Path>) -> Result<ImportSummary, IoError> {
    let drawing = Drawing::load_file(path)?;
    import_dxf(&drawing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dxf::Vector as DxfVector;
    use dxf::entities::{AngularThreePointDimension, Ellipse, OrdinateDimension};
    use std::f64::consts::{FRAC_1_SQRT_2, FRAC_PI_2};
    use std::fs;

    const EPS: f64 = 1e-9;

    fn approx_point(a: Point2, b: Point2) {
        assert!((a.x - b.x).abs() < EPS, "x mismatch: {a:?} vs {b:?}");
        assert!((a.y - b.y).abs() < EPS, "y mismatch: {a:?} vs {b:?}");
    }

    /// ラジアン→度→ラジアン変換の丸め誤差を許容する角度比較。
    const ANGLE_EPS: f64 = 1e-6;

    fn approx_shape(expected: &Shape, actual: &Shape) {
        match (expected, actual) {
            (Shape::Point(a), Shape::Point(b)) => approx_point(*a, *b),
            (Shape::Line(a), Shape::Line(b)) => {
                approx_point(a.a, b.a);
                approx_point(a.b, b.b);
            }
            (Shape::Circle(a), Shape::Circle(b)) => {
                approx_point(a.center, b.center);
                assert!((a.radius - b.radius).abs() < EPS);
            }
            (Shape::Arc(a), Shape::Arc(b)) => {
                approx_point(a.center, b.center);
                assert!((a.radius - b.radius).abs() < EPS);
                assert!((a.start_angle - b.start_angle).abs() < ANGLE_EPS);
                assert!((a.end_angle - b.end_angle).abs() < ANGLE_EPS);
            }
            (Shape::Polyline(a), Shape::Polyline(b)) => {
                assert_eq!(a.closed, b.closed);
                assert_eq!(a.vertices.len(), b.vertices.len());
                for (va, vb) in a.vertices.iter().zip(b.vertices.iter()) {
                    approx_point(*va, *vb);
                }
            }
            _ => panic!("shape kind mismatch: {expected:?} vs {actual:?}"),
        }
    }

    /// 全 Shape 種（Polyline は開閉両方）・複数レイヤー（可視/非表示・ロック含む）・
    /// パレット上の色（entity 個別色 1 件・レイヤー色 2 件）・非デフォルトの
    /// カレントレイヤーを持つドキュメントを作る。
    ///
    /// 色はすべて [`ACI_PALETTE`] に載っている値を選ぶことで、色についても
    /// 完全往復することを確認できるようにする（パレット外の色は近似のみで、
    /// このテストの対象ではない）。
    fn full_document() -> Document {
        let mut doc = Document::new();
        let default = doc.default_layer();
        // デフォルトレイヤー "0" の色は Rgb::WHITE = ACI 7 と一致するのでそのまま。

        let second = doc
            .apply(Command::AddLayer(Layer::new(
                "second",
                Rgb::new(0, 255, 0), // ACI 3（緑）と厳密一致
            )))
            .unwrap()
            .layers[0];

        let shapes = [
            Shape::Point(Point2::new(1.0, 2.0)),
            Shape::Line(LineSeg::new(Point2::new(0.0, 0.0), Point2::new(3.0, 4.0))),
            Shape::Circle(Circle::new(Point2::new(-1.0, 5.0), 2.5)),
            Shape::Arc(Arc::new(Point2::new(2.0, 2.0), 1.5, 0.3, 2.8)),
            Shape::Polyline(Polyline::new(
                vec![
                    Point2::new(0.0, 0.0),
                    Point2::new(1.0, 1.0),
                    Point2::new(2.0, 0.0),
                ],
                false, // 開いたポリライン
            )),
            Shape::Polyline(Polyline::new(
                vec![
                    Point2::new(0.0, 0.0),
                    Point2::new(1.0, 1.0),
                    Point2::new(2.0, 0.0),
                ],
                true, // 閉じたポリライン
            )),
        ];
        for (i, shape) in shapes.into_iter().enumerate() {
            let layer = if i % 2 == 0 { default } else { second };
            let style = if i == 0 {
                Style {
                    color: Some(Rgb::new(255, 0, 0)), // ACI 1（赤）と厳密一致
                    // 線幅・線種の DXF 往復はタスク35c の担当。ここでは色だけを
                    // 個別指定し、線幅・線種は ByLayer のままにする。
                    ..Style::inherited()
                }
            } else {
                Style::inherited()
            };
            doc.apply(Command::AddEntity(Entity::new(shape, layer, style)))
                .unwrap();
        }

        // second をカレントにし、ロック+非表示にする（ロックは DXF 往復で
        // 保存されないため、import 後は false になることを確認する側で使う）。
        doc.apply(Command::SetCurrentLayer(second)).unwrap();
        let mut props = doc.layer(second).unwrap().clone();
        props.locked = true;
        props.visible = false;
        doc.apply(Command::SetLayerProps { id: second, props })
            .unwrap();
        doc
    }

    /// レイヤー名・可視性を比較する（ロックは DXF 往復で保存されないため
    /// 比較しない。往復後は常に false になることを別途確認する）。
    fn assert_layers_match(doc: &Document, imported: &Document) {
        assert_eq!(doc.layer_count(), imported.layer_count());
        let orig_layers: Vec<_> = doc.layers().collect();
        let imp_layers: Vec<_> = imported.layers().collect();
        for ((_, ol), (_, il)) in orig_layers.iter().zip(imp_layers.iter()) {
            assert_eq!(ol.name, il.name);
            assert_eq!(ol.visible, il.visible);
            assert!(!il.locked, "DXF 往復後のレイヤーは常に unlocked");
        }
    }

    #[test]
    fn round_trip_preserves_entities_and_layers() {
        let doc = full_document();
        let export = export_dxf(&doc);
        assert_eq!(export.skipped_entities, 0);
        let summary = import_dxf(&export.drawing).unwrap();
        assert_eq!(summary.skipped_entities, 0);
        let imported = summary.document;

        assert_eq!(imported.entity_count(), doc.entity_count());
        assert_layers_match(&doc, &imported);

        // レイヤー色: パレットと厳密一致する色を選んでいるので完全往復するはず。
        let orig_layers: Vec<_> = doc.layers().map(|(_, l)| l.clone()).collect();
        let imp_layers: Vec<_> = imported.layers().map(|(_, l)| l.clone()).collect();
        for (ol, il) in orig_layers.iter().zip(imp_layers.iter()) {
            assert_eq!(ol.color, il.color, "layer {}", ol.name);
        }

        // エンティティ: 挿入順が export/import を通じて保たれる前提で、位置ごとに
        // 幾何・所属レイヤー名・個別色を比較する。
        let orig_entities: Vec<_> = doc.entities().collect();
        let imp_entities: Vec<_> = imported.entities().collect();
        assert_eq!(orig_entities.len(), imp_entities.len());
        for ((_, oe), (_, ie)) in orig_entities.iter().zip(imp_entities.iter()) {
            // full_document は Shape 系のみを含む（Text の往復は
            // round_trip_preserves_cjk_and_ascii_text が担当）。
            let os = oe.geom.as_shape().expect("Shape エンティティのはず");
            let is = ie.geom.as_shape().expect("Shape エンティティのはず");
            approx_shape(os, is);
            let oname = &doc.layer(oe.layer).unwrap().name;
            let iname = &imported.layer(ie.layer).unwrap().name;
            assert_eq!(oname, iname);
            assert_eq!(oe.style.color, ie.style.color);
        }

        // カレントレイヤー（$CLAYER 経由のベストエフォート復元）。
        let orig_current_name = &doc.layer(doc.current_layer()).unwrap().name;
        let imp_current_name = &imported.layer(imported.current_layer()).unwrap().name;
        assert_eq!(orig_current_name, imp_current_name);
    }

    #[test]
    fn import_leaves_history_empty() {
        let doc = full_document();
        let summary = import_dxf(&export_dxf(&doc).drawing).unwrap();
        assert!(
            !summary.document.can_undo(),
            "読込直後に undo できてはならない"
        );
        assert!(!summary.document.can_redo());
    }

    #[test]
    fn round_trip_empty_document() {
        let doc = Document::new();
        let export = export_dxf(&doc);
        assert_eq!(export.skipped_entities, 0);
        let summary = import_dxf(&export.drawing).unwrap();
        assert_eq!(summary.skipped_entities, 0);
        assert_eq!(summary.document.entity_count(), 0);
        assert_eq!(summary.document.layer_count(), 1);
        assert_eq!(
            summary
                .document
                .layer(summary.document.default_layer())
                .unwrap()
                .name,
            "0"
        );
    }

    /// 「未対応エンティティ種別はスキップして数える」機構の回帰テスト。
    ///
    /// 代表は ELLIPSE。タスク25b で TEXT が、M11 タスク67 で DIMENSION の 4 経路
    /// （整列・回転・半径・直径）が import 対応になったため、以前ここで使っていた
    /// RADIALDIMENSION はもう「未対応種別」の代表にならない
    /// （退化した既定値がジオメトリ検証で落ちるだけになる）。
    #[test]
    fn unsupported_entity_is_skipped_and_counted() {
        let doc = full_document();
        let mut drawing = export_dxf(&doc).drawing;
        let before = doc.entity_count();

        let mut ellipse = DxfEntity::new(EntityType::Ellipse(Ellipse::default()));
        ellipse.common.layer = "0".to_string();
        drawing.add_entity(ellipse);

        let summary = import_dxf(&drawing).unwrap();
        assert_eq!(summary.skipped_entities, 1);
        assert_eq!(summary.document.entity_count(), before);
    }

    // -----------------------------------------------------------------
    // DIMENSION の import（M11 タスク67）
    //
    // 実ファイル（LibreCAD 出力）での検証は `tests/dxf_dimensions.rs`。ここでは
    // その fixture に現れない条件（法線・文字位置の明示指定・文字テンプレート・
    // 実測値の食い違い・退化）を合成データで固定する。
    // -----------------------------------------------------------------

    /// レイヤー "0" だけを持つ最小の `Drawing` へエンティティを 1 件載せる。
    fn drawing_with_one_entity(specific: EntityType) -> Drawing {
        let mut drawing = Drawing::new();
        while drawing.remove_layer(0).is_some() {}
        drawing.add_layer(DxfLayer {
            name: "0".to_string(),
            ..Default::default()
        });
        let mut entity = DxfEntity::new(specific);
        entity.common.layer = "0".to_string();
        drawing.add_entity(entity);
        drawing
    }

    /// 整列寸法の共通部。計測 2 点は `(0,0)`–`(10,0)`、寸法線（group 10）は `y = 3`。
    fn aligned_base() -> DimensionBase {
        DimensionBase {
            dimension_type: DimensionType::Aligned,
            definition_point_1: DxfPoint::new(0.0, 3.0, 0.0),
            ..Default::default()
        }
    }

    /// `base` に計測 2 点 `p1`–`p2` を付けた `RotatedDimension`。
    fn linear_dim(base: DimensionBase, p1: (f64, f64), p2: (f64, f64)) -> EntityType {
        EntityType::RotatedDimension(RotatedDimension {
            dimension_base: base,
            definition_point_2: DxfPoint::new(p1.0, p1.1, 0.0),
            definition_point_3: DxfPoint::new(p2.0, p2.1, 0.0),
            ..Default::default()
        })
    }

    /// 寸法 1 件だけを読み、
    /// `(ジオメトリ, skipped_entities, skipped_dimensions, 捨てた属性の件数)` を返す。
    fn import_one(specific: EntityType) -> (Option<EntityGeom>, usize, usize, usize) {
        let summary = import_dxf(&drawing_with_one_entity(specific)).unwrap();
        let geom = summary
            .document
            .entities()
            .next()
            .map(|(_, e)| e.geom.clone());
        (
            geom,
            summary.skipped_entities,
            summary.skipped_dimensions,
            summary.dropped_dimension_details,
        )
    }

    fn linear_of(geom: Option<EntityGeom>) -> DimLinear {
        match geom {
            Some(EntityGeom::DimLinear(d)) => d,
            other => panic!("長さ寸法として取り込まれていない: {other:?}"),
        }
    }

    /// `offset` の符号は `mcad-core` の契約（寸法線端 = `p1 + perp(dir) * offset`、
    /// `perp` は左 90 度回転）と一致する。group 10 を計測線の左右へ振ると符号だけが
    /// 反転する。
    #[test]
    fn linear_dimension_offset_follows_the_core_sign_convention() {
        let left = linear_of(import_one(linear_dim(aligned_base(), (0.0, 0.0), (10.0, 0.0))).0);
        approx_point(left.p1, Point2::new(0.0, 0.0));
        approx_point(left.p2, Point2::new(10.0, 0.0));
        assert!((left.offset - 3.0).abs() < EPS, "offset: {}", left.offset);

        let right = linear_of(
            import_one(linear_dim(
                DimensionBase {
                    definition_point_1: DxfPoint::new(0.0, -3.0, 0.0),
                    ..aligned_base()
                },
                (0.0, 0.0),
                (10.0, 0.0),
            ))
            .0,
        );
        assert!((right.offset + 3.0).abs() < EPS, "offset: {}", right.offset);
    }

    /// 回転寸法は「整列寸法と同じ図になる」ものだけ受け入れる。計測 2 点が group 50 の
    /// 方向に並んでいなければ、DXF の寸法値（投影長）を mcad が表現できないのでスキップ。
    #[test]
    fn rotated_dimension_is_skipped_unless_its_points_lie_along_its_angle() {
        // 水平（50 省略 = 0）なのに計測 2 点が斜め → 投影が効いている → スキップ。
        let base = DimensionBase {
            dimension_type: DimensionType::RotatedHorizontalOrVertical,
            ..aligned_base()
        };
        let (geom, skipped_entities, skipped_dimensions, dropped) =
            import_one(linear_dim(base, (0.0, 0.0), (10.0, 5.0)));
        assert!(geom.is_none(), "斜めの計測点を持つ回転寸法が通った");
        assert_eq!(
            (skipped_entities, skipped_dimensions, dropped),
            (0, 1, 0),
            "種別は対応しているので skipped_dimensions へ計上する"
        );

        // 鉛直（50 = 90）で計測 2 点も鉛直 → 整列寸法と同一なので受け入れる。
        let vertical = EntityType::RotatedDimension(RotatedDimension {
            dimension_base: DimensionBase {
                dimension_type: DimensionType::RotatedHorizontalOrVertical,
                definition_point_1: DxfPoint::new(4.0, 0.0, 0.0),
                ..aligned_base()
            },
            definition_point_2: DxfPoint::new(0.0, 0.0, 0.0),
            definition_point_3: DxfPoint::new(0.0, 10.0, 0.0),
            rotation_angle: 90.0,
            ..Default::default()
        });
        let (geom, skipped_entities, skipped_dimensions, dropped) = import_one(vertical);
        let dim = linear_of(geom);
        approx_point(dim.p2, Point2::new(0.0, 10.0));
        // dir = (0,1)、perp(dir) = (-1,0) なので group 10 =(4,0) は offset = -4。
        assert!((dim.offset + 4.0).abs() < EPS, "offset: {}", dim.offset);
        assert_eq!((skipped_entities, skipped_dimensions, dropped), (0, 0, 0));
    }

    /// group 51（寸法の水平方向 = UCS の X 軸）が 0 でない**回転**寸法は、group 50 の
    /// 基準が変わるのでスキップする。**整列**寸法は寸法線の向きを計測 2 点だけで
    /// 決めるので 51 の影響を受けず、そのまま受け入れる。
    #[test]
    fn rotated_dimension_with_a_rotated_ucs_is_skipped_but_aligned_one_is_not() {
        let rotated = linear_dim(
            DimensionBase {
                dimension_type: DimensionType::RotatedHorizontalOrVertical,
                horizontal_direction_angle: 30.0,
                ..aligned_base()
            },
            (0.0, 0.0),
            (10.0, 0.0),
        );
        assert_eq!(
            import_one(rotated).2,
            1,
            "51 付きの回転寸法はスキップ（skipped_dimensions）"
        );

        let aligned = linear_dim(
            DimensionBase {
                horizontal_direction_angle: 30.0,
                ..aligned_base()
            },
            (0.0, 0.0),
            (10.0, 0.0),
        );
        let (geom, skipped_entities, skipped_dimensions, dropped) = import_one(aligned);
        assert!(geom.is_some(), "51 付きの整列寸法まで落ちている");
        assert_eq!((skipped_entities, skipped_dimensions, dropped), (0, 0, 0));
    }

    /// 押し出し法線（group 210/220/230）が +Z でない寸法は、定義点が OCS で書かれて
    /// いるためスキップする（[`is_dimension_plane_supported`] の doc）。
    #[test]
    fn dimension_outside_the_xy_plane_is_skipped() {
        for normal in [
            DxfVector::new(0.0, 0.0, -1.0),
            DxfVector::new(1.0, 0.0, 0.0),
            DxfVector::new(0.0, FRAC_1_SQRT_2, FRAC_1_SQRT_2),
        ] {
            let base = DimensionBase {
                normal: normal.clone(),
                ..aligned_base()
            };
            let (geom, skipped_entities, skipped_dimensions, _) =
                import_one(linear_dim(base, (0.0, 0.0), (10.0, 0.0)));
            assert!(geom.is_none(), "法線 {normal:?} の寸法が通った");
            assert_eq!((skipped_entities, skipped_dimensions), (0, 1));
        }

        // 半径・直径も同じ判定を通る。
        let radial = EntityType::RadialDimension(RadialDimension {
            dimension_base: DimensionBase {
                normal: DxfVector::new(0.0, 0.0, -1.0),
                ..Default::default()
            },
            definition_point_2: DxfPoint::new(5.0, 0.0, 0.0),
            ..Default::default()
        });
        assert_eq!(import_one(radial).2, 1);
    }

    /// 文字位置の明示指定（group 70 の 128 ビット）があるときだけ、group 11 を
    /// [`DimAnnotation::text_anchor`] へ写す。自動配置の寸法にも group 11 は書かれて
    /// いるので、常に写すと mcad の自動配置が効かなくなる。
    #[test]
    fn user_defined_text_location_becomes_the_text_anchor() {
        let with_override = DimensionBase {
            is_at_user_defined_location: true,
            text_mid_point: DxfPoint::new(2.0, 9.0, 0.0),
            ..aligned_base()
        };
        let dim = linear_of(import_one(linear_dim(with_override, (0.0, 0.0), (10.0, 0.0))).0);
        approx_point(
            dim.annotation.text_anchor.expect("文字位置が写っていない"),
            Point2::new(2.0, 9.0),
        );

        let automatic = DimensionBase {
            text_mid_point: DxfPoint::new(2.0, 9.0, 0.0),
            ..aligned_base()
        };
        let dim = linear_of(import_one(linear_dim(automatic, (0.0, 0.0), (10.0, 0.0))).0);
        assert_eq!(dim.annotation, DimAnnotation::unannotated());
    }

    /// group 1（文字テンプレート）は空文字列と `<>` だけが「実測値を描く」＝ mcad の
    /// 既定と同じ。それ以外（スペース 1 文字の「文字を出さない」指定、固定文字列、
    /// `<>` を含むテンプレート）は M11 タスク70 まで表現できないので、**無注記で
    /// 取り込んで計上する**。
    #[test]
    fn dimension_text_template_other_than_the_measured_value_is_dropped_and_counted() {
        for text in ["", "<>"] {
            let base = DimensionBase {
                text: text.to_string(),
                ..aligned_base()
            };
            let (geom, _, _, dropped) = import_one(linear_dim(base, (0.0, 0.0), (10.0, 0.0)));
            assert!(geom.is_some());
            assert_eq!(dropped, 0, "text {text:?} は既定と同じなので計上しない");
        }
        for text in [" ", "M10", "2×<>", "<> H7"] {
            let base = DimensionBase {
                text: text.to_string(),
                ..aligned_base()
            };
            let (geom, _, _, dropped) = import_one(linear_dim(base, (0.0, 0.0), (10.0, 0.0)));
            let dim = linear_of(geom);
            assert_eq!(
                dim.annotation,
                DimAnnotation::unannotated(),
                "text {text:?}"
            );
            assert_eq!(dropped, 1, "text {text:?} を計上していない");
        }
    }

    /// 補助線の傾き（group 52）・寸法文字の回転（group 53）は mcad に概念が無いので
    /// 捨てて計上する。**計上は寸法 1 件につき最大 1**（属性ごとに増やさない）。
    #[test]
    fn oblique_extension_lines_and_text_rotation_are_dropped_once_per_dimension() {
        let oblique = EntityType::RotatedDimension(RotatedDimension {
            dimension_base: aligned_base(),
            definition_point_2: DxfPoint::new(0.0, 0.0, 0.0),
            definition_point_3: DxfPoint::new(10.0, 0.0, 0.0),
            extension_line_angle: 15.0,
            ..Default::default()
        });
        let (geom, skipped_entities, skipped_dimensions, dropped) = import_one(oblique);
        assert!(
            geom.is_some(),
            "52 付きの寸法は取り込む（捨てるのは 52 だけ）"
        );
        assert_eq!((skipped_entities, skipped_dimensions, dropped), (0, 0, 1));

        let rotated_text = DimensionBase {
            text_rotation_angle: 30.0,
            ..aligned_base()
        };
        let (_, _, _, dropped) = import_one(linear_dim(rotated_text, (0.0, 0.0), (10.0, 0.0)));
        assert_eq!(dropped, 1);

        // 52 と 53 と文字テンプレートが同時に落ちても、計上は 1 件。
        let everything = EntityType::RotatedDimension(RotatedDimension {
            dimension_base: DimensionBase {
                text: "M10".to_string(),
                text_rotation_angle: 30.0,
                ..aligned_base()
            },
            definition_point_2: DxfPoint::new(0.0, 0.0, 0.0),
            definition_point_3: DxfPoint::new(10.0, 0.0, 0.0),
            extension_line_angle: 15.0,
            ..Default::default()
        });
        assert_eq!(import_one(everything).3, 1, "寸法ごとに 1 件だけ数える");
    }

    /// 実測値（group 42）は「書いた CAD が表示していた値」。mcad は座標から再計算した
    /// 値を採るので、食い違う寸法は**値だけ黙って変わる**ことを避けるため取り込まず
    /// スキップする（種別は対応しているので `dropped_dimension_details` ではなく
    /// `skipped_dimensions` へ計上）。
    /// 省略（クレート既定の `0.0`）は比較しない＝取り込む。許容内の微差（相対
    /// `DIM_MEASUREMENT_REL_EPS` = 1e-6 の内側）も取り込む。
    #[test]
    fn actual_measurement_mismatch_is_skipped_not_mismeasured() {
        for (measurement, expected_skipped) in [
            (0.0, 0),         // 省略扱い（クレート既定）→ 比較しない
            (10.0, 0),        // 一致
            (10.0 + 1e-9, 0), // 許容内の微差
            (12.0, 1),        // 食い違い → 取り込まずスキップ
        ] {
            let base = DimensionBase {
                actual_measurement: measurement,
                ..aligned_base()
            };
            let (geom, skipped_entities, skipped_dimensions, dropped) =
                import_one(linear_dim(base, (0.0, 0.0), (10.0, 0.0)));
            assert_eq!(
                geom.is_some(),
                expected_skipped == 0,
                "42 = {measurement} の取り込み可否"
            );
            assert_eq!(
                (skipped_entities, skipped_dimensions, dropped),
                (0, expected_skipped, 0),
                "42 = {measurement}"
            );
        }

        // 直径寸法の group 42 は**直径**。半径と比べてはいけない。
        let diameter = EntityType::DiameterDimension(DiameterDimension {
            dimension_base: DimensionBase {
                actual_measurement: 10.0,
                definition_point_1: DxfPoint::new(-5.0, 0.0, 0.0),
                ..Default::default()
            },
            definition_point_2: DxfPoint::new(5.0, 0.0, 0.0),
            ..Default::default()
        });
        let (geom, skipped_entities, skipped_dimensions, dropped) = import_one(diameter);
        match geom {
            Some(EntityGeom::DimDiameter(d)) => {
                assert!((d.radius - 5.0).abs() < EPS, "radius: {}", d.radius);
                approx_point(d.center, Point2::new(0.0, 0.0));
            }
            other => panic!("直径寸法として取り込まれていない: {other:?}"),
        }
        assert_eq!(
            (skipped_entities, skipped_dimensions, dropped),
            (0, 0, 0),
            "直径 10 と group 42 = 10 は一致している"
        );
    }

    /// 回転寸法の平行判定（計測 2 点が group 50 の方向と平行）を通っても、group 42
    /// が測定倍率（DIMLFAC 相当）で実距離と食い違えば、値が黙って変わらないよう
    /// 取り込まずスキップする。平行判定だけでは防げないケースの回帰。
    #[test]
    fn parallel_dimension_with_mismatched_measurement_is_skipped_not_mismeasured() {
        // 水平（50 省略 = 0）・計測 2 点も水平 → 平行判定は通る。
        // 実距離は 10 だが group 42 = 20（DIMLFAC = 2 相当）で食い違う。
        let base = DimensionBase {
            dimension_type: DimensionType::RotatedHorizontalOrVertical,
            actual_measurement: 20.0,
            ..aligned_base()
        };
        let (geom, skipped_entities, skipped_dimensions, dropped) =
            import_one(linear_dim(base, (0.0, 0.0), (10.0, 0.0)));
        assert!(
            geom.is_none(),
            "平行判定を通ったのに測定倍率の食い違いで取り込まれた"
        );
        assert_eq!(
            (skipped_entities, skipped_dimensions, dropped),
            (0, 1, 0),
            "食い違いは skipped_dimensions へ計上し、dropped_dimension_details には計上しない"
        );
    }

    /// 退化した寸法（計測 2 点が同一 / 半径 0）はスキップして数える。長さ寸法は
    /// 法線が決まらず `offset` を定義できず、半径・直径は
    /// [`EntityGeom::validate`] が半径 0 を拒否する。種別自体は対応しているため
    /// `skipped_dimensions` へ計上する（`skipped_entities` ではない）。
    #[test]
    fn degenerate_dimensions_are_skipped_and_counted() {
        let (geom, skipped_entities, skipped_dimensions, _) =
            import_one(linear_dim(aligned_base(), (1.0, 1.0), (1.0, 1.0)));
        assert!(geom.is_none());
        assert_eq!((skipped_entities, skipped_dimensions), (0, 1));

        // 中心と円周点が同じ = 半径 0。
        let radial = EntityType::RadialDimension(RadialDimension::default());
        assert_eq!(import_one(radial).2, 1);

        let diameter = EntityType::DiameterDimension(DiameterDimension::default());
        assert_eq!(import_one(diameter).2, 1);
    }

    /// 3 点角度寸法・座標寸法は mcad にモデルが無いのでスキップして数える
    /// （M11 タスク69 で対応）。2 直線角度寸法・弧長寸法は `dxf` 0.6.1 が
    /// エンティティとして返さないため、そもそもここへ来ない（数えられない）。
    #[test]
    fn angular_and_ordinate_dimensions_are_skipped_and_counted() {
        for specific in [
            EntityType::AngularThreePointDimension(AngularThreePointDimension::default()),
            EntityType::OrdinateDimension(OrdinateDimension::default()),
        ] {
            let (geom, skipped_entities, skipped_dimensions, dropped) = import_one(specific);
            assert!(geom.is_none());
            assert_eq!(
                (skipped_entities, skipped_dimensions, dropped),
                (1, 0, 0),
                "種別自体が未対応なので skipped_entities（skipped_dimensions ではない）"
            );
        }
    }

    /// 不正な TextGeom（空文字列・非正の文字高さ）も、Shape の不正ジオメトリと同じ
    /// 規律で読込全体を失敗させずスキップしてカウントする。
    ///
    /// `DxfText::default()` は `value` が空文字列・`text_height` が 0.0 で、
    /// [`EntityGeom::validate`] の両条件に触れる。
    #[test]
    fn invalid_text_entity_is_skipped_and_counted() {
        let mut drawing = Drawing::new();
        while drawing.remove_layer(0).is_some() {}
        drawing.add_layer(DxfLayer {
            name: "0".to_string(),
            ..Default::default()
        });

        let mut empty_text = DxfEntity::new(EntityType::Text(DxfText::default()));
        empty_text.common.layer = "0".to_string();
        drawing.add_entity(empty_text);

        // 内容はあるが文字高さが 0 の TEXT も不正。
        let mut zero_height = DxfEntity::new(EntityType::Text(DxfText {
            value: "ok".to_string(),
            text_height: 0.0,
            ..Default::default()
        }));
        zero_height.common.layer = "0".to_string();
        drawing.add_entity(zero_height);

        let summary = import_dxf(&drawing).unwrap();
        assert_eq!(summary.skipped_entities, 2);
        assert_eq!(summary.document.entity_count(), 0);
    }

    /// 位置基準（justification）が Left/Baseline 以外の TEXT は、`location`
    /// （group code 10）が文字位置を持たないため import せずスキップして計上する。
    ///
    /// 誤配置を防ぐための意図的な取りこぼしであり、逆算にフォントメトリクスが必要で
    /// io 層では実装できないという依存方向の制約が根拠（詳細は
    /// [`is_text_justification_supported`] の doc）。mcad 自身の export は常に
    /// Left/Baseline なので、この経路は**外部 CAD が作った DXF でしか通らない**。
    /// そのため往復テストでは検出できず、`dxf::Drawing` へ直接エンティティを
    /// 注入して確認する。
    ///
    /// 同じ図面に Left/Baseline の TEXT も混ぜ、そちらは従来どおり import され
    /// 位置・内容が保たれることを同時に固定する（判定が広すぎて正常な TEXT まで
    /// 落とす回帰を防ぐ）。
    #[test]
    fn text_with_unsupported_justification_is_skipped_and_counted() {
        let mut drawing = Drawing::new();
        while drawing.remove_layer(0).is_some() {}
        drawing.add_layer(DxfLayer {
            name: "0".to_string(),
            ..Default::default()
        });

        // ジオメトリとしては有効（非空の value・正の text_height）にしておき、
        // スキップの原因が justification だけであることを担保する。
        let base = DxfText {
            location: DxfPoint::new(1.0, 2.0, 0.0),
            text_height: 1.5,
            value: "label".to_string(),
            ..Default::default()
        };

        // 水平 justification が Left 以外（Center / Right）。文字位置は
        // second_alignment_point 側にあるので location は使えない。
        for horizontal in [
            HorizontalTextJustification::Center,
            HorizontalTextJustification::Right,
        ] {
            let mut entity = DxfEntity::new(EntityType::Text(DxfText {
                horizontal_text_justification: horizontal,
                second_alignment_point: DxfPoint::new(10.0, 20.0, 0.0),
                ..base.clone()
            }));
            entity.common.layer = "0".to_string();
            drawing.add_entity(entity);
        }

        // 垂直 justification が Baseline 以外（Middle）。水平が Left でもスキップ対象。
        let mut vertical_middle = DxfEntity::new(EntityType::Text(DxfText {
            vertical_text_justification: VerticalTextJustification::Middle,
            second_alignment_point: DxfPoint::new(10.0, 20.0, 0.0),
            ..base.clone()
        }));
        vertical_middle.common.layer = "0".to_string();
        drawing.add_entity(vertical_middle);

        // 既定（Left / Baseline）の TEXT は従来どおり import される。
        let mut supported = DxfEntity::new(EntityType::Text(base.clone()));
        supported.common.layer = "0".to_string();
        drawing.add_entity(supported);

        let summary = import_dxf(&drawing).unwrap();
        assert_eq!(
            summary.skipped_entities, 3,
            "Center / Right / 垂直 Middle の 3 件がスキップされるはず"
        );
        assert_eq!(
            summary.document.entity_count(),
            1,
            "Left/Baseline の 1 件だけが復元されるはず"
        );

        let (_, entity) = summary.document.entities().next().unwrap();
        let EntityGeom::Text(text) = &entity.geom else {
            panic!("Text エンティティとして復元されるはず: {:?}", entity.geom);
        };
        assert_eq!(text.content, "label");
        // alignment point ではなく location が anchor になる（Left/Baseline なので正しい）。
        approx_point(text.anchor, Point2::new(1.0, 2.0));
    }

    /// タスク25: Text は DXF `TEXT` エンティティとして export され、長さ寸法・半径寸法は
    /// DXF に対応するプリミティブがなくスキップされる（DESIGN.md M6 設計判断5・
    /// タスク分割表#25）。ここは export 側のフィールドマッピングのみを見る
    /// （往復は `round_trip_preserves_cjk_and_ascii_text`）。
    #[test]
    fn export_writes_text_entity_and_skips_dimensions() {
        use mcad_core::{DimAnnotation, DimDiameter, DimLinear, DimRadial, EntityGeom, TextGeom};

        let mut doc = Document::new();
        let layer = doc.current_layer();
        // Shape 1 件は DXF へ書き出される。
        doc.apply(Command::AddEntity(Entity::new(
            Shape::Line(LineSeg::new(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0))),
            layer,
            Style::inherited(),
        )))
        .unwrap();
        // Text は DXF TEXT エンティティとして書き出される。
        doc.apply(Command::AddEntity(Entity::new(
            EntityGeom::Text(TextGeom {
                anchor: Point2::new(3.0, 4.0),
                content: "hi".into(),
                height: 2.5,
                angle: FRAC_PI_2,
            }),
            layer,
            Style::inherited(),
        )))
        .unwrap();
        // 長さ寸法・半径寸法・直径寸法は DXF 非対応でスキップされる（直径寸法は
        // M9 タスク47-2 の新設バリアント。ワイルドカード腕でスキップ側へ乗る）。
        doc.apply(Command::AddEntity(Entity::new(
            EntityGeom::DimLinear(DimLinear {
                p1: Point2::new(0.0, 0.0),
                p2: Point2::new(2.0, 0.0),
                offset: 1.0,
                annotation: DimAnnotation::default(),
            }),
            layer,
            Style::inherited(),
        )))
        .unwrap();
        doc.apply(Command::AddEntity(Entity::new(
            EntityGeom::DimRadial(DimRadial {
                center: Point2::new(0.0, 0.0),
                radius: 2.0,
                leader_angle: 0.0,
                annotation: DimAnnotation::default(),
            }),
            layer,
            Style::inherited(),
        )))
        .unwrap();
        doc.apply(Command::AddEntity(Entity::new(
            EntityGeom::DimDiameter(DimDiameter {
                center: Point2::new(0.0, 0.0),
                radius: 2.0,
                angle: 0.0,
                annotation: DimAnnotation::default(),
            }),
            layer,
            Style::inherited(),
        )))
        .unwrap();

        let export = export_dxf(&doc);
        // 3 件（長さ・半径・直径寸法）がスキップされ、Shape 1 件 + TEXT 1 件が図面へ入る。
        assert_eq!(export.skipped_entities, 3);
        assert_eq!(export.drawing.entities().count(), 2);

        let text_entity = export
            .drawing
            .entities()
            .find_map(|e| match &e.specific {
                EntityType::Text(t) => Some(t),
                _ => None,
            })
            .expect("TEXT entity must be present in the exported drawing");
        approx_point(from_dxf_point(&text_entity.location), Point2::new(3.0, 4.0));
        assert!((text_entity.text_height - 2.5).abs() < EPS);
        assert!((text_entity.rotation - 90.0).abs() < ANGLE_EPS);
        assert_eq!(text_entity.value, "hi");
    }

    /// タスク61: 表（`EntityGeom::Table`、M10）の DXF 分解 export。
    /// 2×2 の表は罫線 `4（外枠）+ 1（行境界）+ 1（列境界）= 6` 本の LINE と、
    /// 非空セルぶんの TEXT へ分解される（DESIGN.md M10 詳細設計8）。表自体は
    /// `skipped_entities` に計上しない（分解して書けているため、Text と同じ
    /// 「対応済み」扱い）。
    #[test]
    fn export_decomposes_table_into_lines_and_texts() {
        let mut doc = Document::new();
        let layer = doc.current_layer();
        doc.apply(Command::AddEntity(Entity::new(
            EntityGeom::Table(TableGeom {
                anchor: Point2::new(10.0, 20.0),
                col_widths_mm: vec![30.0, 20.0],
                row_heights_mm: vec![8.0, 8.0],
                text_height_mm: 3.5,
                cells: vec!["a".into(), "b".into(), "c".into(), "d".into()],
            }),
            layer,
            Style::inherited(),
        )))
        .unwrap();

        let export = export_dxf(&doc);
        assert_eq!(export.skipped_entities, 0);

        for entity in export.drawing.entities() {
            assert!(
                matches!(entity.specific, EntityType::Line(_) | EntityType::Text(_)),
                "table export must only produce LINE/TEXT, got {:?}",
                entity.specific
            );
        }
        let line_count = export
            .drawing
            .entities()
            .filter(|e| matches!(e.specific, EntityType::Line(_)))
            .count();
        let text_count = export
            .drawing
            .entities()
            .filter(|e| matches!(e.specific, EntityType::Text(_)))
            .count();
        assert_eq!(line_count, 6, "4 outer + 1 row divider + 1 col divider");
        assert_eq!(text_count, 4, "all 4 cells are non-empty");

        let text_values: Vec<&str> = export
            .drawing
            .entities()
            .filter_map(|e| match &e.specific {
                EntityType::Text(t) => Some(t.value.as_str()),
                _ => None,
            })
            .collect();
        for expected in ["a", "b", "c", "d"] {
            assert!(
                text_values.contains(&expected),
                "missing cell text {expected:?} in {text_values:?}"
            );
        }
    }

    /// タスク61: 空セルの表は TEXT を1つも出さない（`expand_table` と同じ
    /// 「空セルは描かない」規則）。罫線だけは出る。
    #[test]
    fn export_table_with_empty_cells_produces_no_text() {
        let mut doc = Document::new();
        let layer = doc.current_layer();
        doc.apply(Command::AddEntity(Entity::new(
            EntityGeom::Table(TableGeom {
                anchor: Point2::new(0.0, 0.0),
                col_widths_mm: vec![30.0],
                row_heights_mm: vec![8.0],
                text_height_mm: 3.5,
                cells: vec![String::new()],
            }),
            layer,
            Style::inherited(),
        )))
        .unwrap();

        let export = export_dxf(&doc);
        assert_eq!(export.skipped_entities, 0);
        let text_count = export
            .drawing
            .entities()
            .filter(|e| matches!(e.specific, EntityType::Text(_)))
            .count();
        assert_eq!(text_count, 0);
        // 1行1列でも外枠4本は出る。
        let line_count = export
            .drawing
            .entities()
            .filter(|e| matches!(e.specific, EntityType::Line(_)))
            .count();
        assert_eq!(line_count, 4);
    }

    /// タスク61: 表の DXF 分解は紙 mm × k（ワールド）で座標を書く。尺度 1:2
    /// （`k = 2.0`）で 1行1列・列幅30mm・行高さ8mm・anchor (0,0) の表の外枠は
    /// ワールドで幅60mm・高さ16mmになる（`text_height_scales_by_sheet_scale_on_export`
    /// と同じ契約）。
    #[test]
    fn export_table_lines_scale_by_sheet_scale() {
        use mcad_core::Scale;

        let mut doc = Document::new();
        let mut sheet = doc.sheet().clone();
        sheet.scale = Scale::new(1, 2).unwrap(); // 1:2、k = 2.0
        doc.apply(Command::SetSheet(sheet)).unwrap();

        let layer = doc.current_layer();
        doc.apply(Command::AddEntity(Entity::new(
            EntityGeom::Table(TableGeom {
                anchor: Point2::new(0.0, 0.0),
                col_widths_mm: vec![30.0],
                row_heights_mm: vec![8.0],
                text_height_mm: 3.5,
                cells: vec!["x".into()],
            }),
            layer,
            Style::inherited(),
        )))
        .unwrap();

        let export = export_dxf(&doc);

        // 外枠の対角（右上隅）はワールドで (60.0, 16.0) になっているはず。
        let max_x = export
            .drawing
            .entities()
            .filter_map(|e| match &e.specific {
                EntityType::Line(l) => Some(l.p1.x.max(l.p2.x)),
                _ => None,
            })
            .fold(f64::MIN, f64::max);
        let max_y = export
            .drawing
            .entities()
            .filter_map(|e| match &e.specific {
                EntityType::Line(l) => Some(l.p1.y.max(l.p2.y)),
                _ => None,
            })
            .fold(f64::MIN, f64::max);
        assert!((max_x - 60.0).abs() < EPS, "got {max_x}");
        assert!((max_y - 16.0).abs() < EPS, "got {max_y}");

        // セル文字の高さは `3.5 * 2.0 = 7.0`（Text export と同じ尺度契約）。
        let text_entity = export
            .drawing
            .entities()
            .find_map(|e| match &e.specific {
                EntityType::Text(t) => Some(t),
                _ => None,
            })
            .expect("TEXT entity must be present");
        assert!((text_entity.text_height - 7.0).abs() < EPS);
    }

    /// タスク61 import: `ACAD_TABLE`(dxf 0.6.1 に対応する `EntityType` バリアントは
    /// 存在しない。実測: `dxf::entities::EntityType` の生成ソースにも spec にも
    /// テーブル系のバリアントは無い)を含む最小 DXF 文字列を import しても、
    /// 他のエンティティ（LINE）は正しく読める。
    ///
    /// **実測で判明した制約**: `dxf` 0.6.1 は型名を認識できないエンティティ
    /// （`EntityType::from_type_string` が `None` を返す名前）を、パーサ内部で
    /// **`drawing.entities()` に一切現れない形で読み飛ばす**
    /// （`dxf-0.6.1/src/entity.rs` の `read_entity` 内 `None => { // swallow
    /// unsupported entity }`）。そのため `import_dxf` の `skipped_entities` は
    /// **`drawing.entities()` を走査して初めて数えられる**ので、ACAD_TABLE の
    /// ようにクレートが型として知らないエンティティは `import_dxf` に渡る前に
    /// 消えており、`skipped_entities` へは計上され**ない**（0 のまま）。
    /// これは DIMENSION・SPLINE 等（`EntityType` にバリアントがあり
    /// `dxf_entity_to_geom` の `_ => None` 腕でスキップする）とは異なる、
    /// クレートのパーサ自体による無警告の読み飛ばしである。
    #[test]
    fn import_ignores_acad_table_and_still_reads_other_entities() {
        let dxf_text = "0\nSECTION\n2\nENTITIES\n0\nLINE\n8\n0\n10\n0.0\n20\n0.0\n30\n0.0\n11\n1.0\n21\n0.0\n31\n0.0\n0\nACAD_TABLE\n8\n0\n0\nENDSEC\n0\nEOF\n";
        let mut cursor = std::io::Cursor::new(dxf_text.as_bytes());
        let drawing = Drawing::load(&mut cursor).unwrap();

        // クレート自体が ACAD_TABLE をパーサ内部で読み飛ばすため、
        // `drawing.entities()` の時点で既に LINE 1件しか現れない。
        assert_eq!(drawing.entities().count(), 1);

        let summary = import_dxf(&drawing).unwrap();
        assert_eq!(summary.document.entity_count(), 1);
        // 実測どおり、この経路では skipped_entities は増えない（doc 参照）。
        assert_eq!(summary.skipped_entities, 0);
    }

    #[test]
    fn unknown_layer_reference_is_added_on_the_fly() {
        // LAYER テーブルには存在しないレイヤー名を直接参照するエンティティ。
        //
        // 注意: `Drawing::add_entity` はビルダー API 経由で呼ぶと内部で
        // `ensure_layer_is_present` を呼び、参照するレイヤー名を自動的に
        // LAYER テーブルへ追加してしまう（このクレートを使ってプログラム的に
        // 組み立てる分には「テーブル未登録のレイヤー参照」は起こらない）。
        // 実際の DXF ファイル読込ではテーブルとエンティティは独立して解析される
        // ため、テーブルに存在しないレイヤー名を参照するファイルがありうる。
        // これを模すため、`add_entity` でいったんデフォルトレイヤー "0" 上に
        // 追加した後、エンティティの層参照だけを "ghost" へ書き換える
        // （LAYER テーブルには "0" しか残らない）。
        let mut drawing = Drawing::new();
        while drawing.remove_layer(0).is_some() {}
        let entity = DxfEntity::new(EntityType::ModelPoint(ModelPoint::new(DxfPoint::new(
            1.0, 2.0, 0.0,
        ))));
        drawing.add_entity(entity);
        for e in drawing.entities_mut() {
            e.common.layer = "ghost".to_string();
        }

        let summary = import_dxf(&drawing).unwrap();
        assert_eq!(summary.skipped_entities, 0);
        assert_eq!(summary.document.entity_count(), 1);
        assert_eq!(summary.document.layer_count(), 2); // デフォルト "0" + "ghost"
        let (_, e) = summary.document.entities().next().unwrap();
        let layer = summary.document.layer(e.layer).unwrap();
        assert_eq!(layer.name, "ghost");
    }

    #[test]
    fn invalid_geometry_entity_is_skipped_and_counted() {
        let mut drawing = Drawing::new();
        while drawing.remove_layer(0).is_some() {}
        drawing.add_layer(DxfLayer {
            name: "0".to_string(),
            ..Default::default()
        });

        // 半径が負の CIRCLE は不正なジオメトリなので無視される。
        let mut bad_circle = DxfEntity::new(EntityType::Circle(DxfCircle {
            center: DxfPoint::origin(),
            radius: -1.0,
            ..Default::default()
        }));
        bad_circle.common.layer = "0".to_string();
        drawing.add_entity(bad_circle);

        let summary = import_dxf(&drawing).unwrap();
        assert_eq!(summary.skipped_entities, 1);
        assert_eq!(summary.document.entity_count(), 0);
    }

    /// import: LAYER テーブルの出現順（`enumerate()` の `index`）がそのまま
    /// `order` になることの回帰テスト（タスク41、モジュール doc「レイヤーの
    /// 重ね順は保存されない」参照）。テーブル出現順と名前のアルファベット順が
    /// 一致しない並びを使い、名前順に引きずられていないことを確認する。
    #[test]
    fn import_assigns_order_from_layer_table_appearance() {
        let mut drawing = Drawing::new();
        while drawing.remove_layer(0).is_some() {}
        for name in ["zeta", "0", "alpha"] {
            drawing.add_layer(DxfLayer {
                name: name.to_string(),
                ..Default::default()
            });
        }

        let summary = import_dxf(&drawing).unwrap();
        let doc = summary.document;
        let ordered: Vec<_> = doc
            .layers_in_order()
            .into_iter()
            .map(|(_, l)| l.name.clone())
            .collect();
        assert_eq!(ordered, vec!["zeta", "0", "alpha"]);
    }

    /// export: レイヤーは「デフォルトレイヤーが先頭、残りは
    /// [`Document::layers_in_order`] の順（`order` 昇順、奥→手前）」で LAYER
    /// テーブルへ書き出される（タスク41、Codex adversarial review [high] 指摘対応で
    /// 先頭固定に変更）。挿入順（"0" → "second" → "third"）とも
    /// `layers_in_order`（"third" → "0" → "second"）とも異なる並びになることを
    /// 確認する: デフォルト "0" は `order` 上は "third" より奥（数値が小さい）だが、
    /// 先頭固定のためテーブル上は "third" より先に出る。
    #[test]
    fn export_writes_layers_in_layers_in_order_sequence() {
        let mut doc = Document::new();
        let default = doc.default_layer();
        let second = doc
            .apply(Command::AddLayer(Layer::new("second", Rgb::WHITE)))
            .unwrap()
            .layers[0];
        let third = doc
            .apply(Command::AddLayer(Layer::new("third", Rgb::WHITE)))
            .unwrap()
            .layers[0];

        // 挿入順は 0, second, third だが、重ね順は third, 0, second にする。
        let mut default_props = doc.layer(default).unwrap().clone();
        default_props.order = 1;
        doc.apply(Command::SetLayerProps {
            id: default,
            props: default_props,
        })
        .unwrap();
        let mut second_props = doc.layer(second).unwrap().clone();
        second_props.order = 2;
        doc.apply(Command::SetLayerProps {
            id: second,
            props: second_props,
        })
        .unwrap();
        let mut third_props = doc.layer(third).unwrap().clone();
        third_props.order = 0;
        doc.apply(Command::SetLayerProps {
            id: third,
            props: third_props,
        })
        .unwrap();

        let layers_in_order: Vec<String> = doc
            .layers_in_order()
            .into_iter()
            .map(|(_, l)| l.name.clone())
            .collect();
        assert_eq!(layers_in_order, vec!["third", "0", "second"]);

        // 書き出し順は「デフォルト "0" が先頭、残りは layers_in_order 順
        // （"third" → "second"、"0" を除いたもの）」。
        let export = export_dxf(&doc);
        let exported_names: Vec<String> = export.drawing.layers().map(|l| l.name.clone()).collect();
        assert_eq!(exported_names, vec!["0", "third", "second"]);
    }

    /// export → import の往復で、**デフォルトレイヤーの `order` が最小でない**
    /// （デフォルトより奥のレイヤーがある）図面でも、往復後に同じレイヤー
    /// （名前で判定）がデフォルトレイヤーのままであることを固定する
    /// （Codex adversarial review [high] 指摘の回帰テスト）。
    ///
    /// 修正前の実装は、export が `layers_in_order`（`order` 昇順）の順で
    /// LAYER テーブルを書いていたため、デフォルトより奥のレイヤーがあると
    /// そちらがテーブルの先頭に来てしまい、import は「テーブル先頭 = デフォルト」
    /// という規則で読むため、**最背面のレイヤーがデフォルトへ化け、元のデフォルトは
    /// 通常レイヤーになる**という実害のあるバグがあった
    /// （既存の `export_writes_layers_in_layers_in_order_sequence` は再 import
    /// していなかったためこの回帰を検出できなかった）。
    #[test]
    fn round_trip_preserves_default_layer_identity_when_default_is_not_backmost() {
        let mut doc = Document::new();
        let default = doc.default_layer(); // 名前は "0"

        // デフォルトより奥（order が小さい）レイヤーを作る。
        let behind = doc
            .apply(Command::AddLayer(Layer::new("behind", Rgb::WHITE)))
            .unwrap()
            .layers[0];
        let mut behind_props = doc.layer(behind).unwrap().clone();
        behind_props.order = -10;
        doc.apply(Command::SetLayerProps {
            id: behind,
            props: behind_props,
        })
        .unwrap();

        // デフォルトより手前のレイヤーも1枚。
        let front = doc
            .apply(Command::AddLayer(Layer::new("front", Rgb::WHITE)))
            .unwrap()
            .layers[0];
        let mut front_props = doc.layer(front).unwrap().clone();
        front_props.order = 10;
        doc.apply(Command::SetLayerProps {
            id: front,
            props: front_props,
        })
        .unwrap();

        // 前提: "behind" がデフォルトより奥にいる（これが回帰の引き金）。
        let layers_in_order: Vec<String> = doc
            .layers_in_order()
            .into_iter()
            .map(|(_, l)| l.name.clone())
            .collect();
        assert_eq!(layers_in_order, vec!["behind", "0", "front"]);

        let export = export_dxf(&doc);
        let summary = import_dxf(&export.drawing).unwrap();
        let mut imported = summary.document;

        // デフォルトレイヤーは名前で見て "0" のままであること
        // （import 側は削除できない特別なレイヤーとして "0" を扱う）。
        let imported_default_name = &imported.layer(imported.default_layer()).unwrap().name;
        assert_eq!(
            imported_default_name,
            &doc.layer(default).unwrap().name,
            "デフォルトレイヤーの同一性(名前)が往復で入れ替わってはならない"
        );
        assert_eq!(imported_default_name, "0");

        // 削除できないのが本当に "0" であることも確認する（"behind" は削除できる）。
        assert!(
            imported
                .apply(Command::RemoveLayer(imported.default_layer()))
                .is_err()
        );
    }

    #[test]
    fn save_and_load_file() {
        let dir = std::env::temp_dir().join("mcad-io-test");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("roundtrip.dxf");

        let doc = full_document();
        save_dxf(&doc, &path).unwrap();
        let summary = load_dxf(&path).unwrap();
        assert_eq!(summary.skipped_entities, 0);
        assert_eq!(summary.document.entity_count(), doc.entity_count());
        assert_layers_match(&doc, &summary.document);

        fs::remove_file(&path).ok();
    }

    /// ヘッダバージョン（[`export_dxf`] 参照）が決める文字列 codec の実態を、
    /// 生成された .dxf ファイルの**バイト列**で固定する。
    ///
    /// 往復（`round_trip_preserves_cjk_and_ascii_text`）だけでは「エンコード・
    /// デコードが対称に働いた」ことしか分からず、ファイルに何が書かれるかは
    /// 固定されない。他の CAD ソフトが読める形になっているかを担保するため、
    /// ここでは生ファイルを直接見る。
    ///
    /// # 以前このテストが固定していたこと（R2000 時代）
    ///
    /// export が R2000 だった間は、`dxf` クレートが ASCII 範囲外の文字を
    /// `\U+XXXX`（4桁大文字16進）へエスケープしていたため（`text_as_ascii =
    /// header.version <= AcadVersion::R2004`）、このテストは
    /// 「ファイル全体が純 ASCII であること」「`寸法テスト` が
    /// `\U+5BF8\U+6CD5\U+30C6\U+30B9\U+30C8` として現れること」を固定していた。
    ///
    /// # 今固定していること（R2007）
    ///
    /// R2007 では UTF-8 のまま書かれ、エスケープ経路を通らない。この経路へ移した
    /// 理由（R2004 以下の codec が往復でデータを壊す 3 欠陥）は [`export_dxf`] の
    /// コメント、再発検知は `round_trip_preserves_pathological_text` を参照。
    /// ここでは「group code 1 の値行に元の文字列がそのまま UTF-8 で現れる」ことと
    /// 「`\U+` エスケープが使われていない」ことの両方を確認し、ヘッダを下げる
    /// 変更があれば必ず落ちるようにする。
    ///
    /// # このテストの位置づけ（何を証明していないか）
    ///
    /// 保存は `dxf` 0.6.1 が行い、検証はその**生バイト列**に対して行う。したがって
    /// 証明できるのは「このクレートがこのヘッダバージョンで期待どおりのバイトを
    /// 書く」ことだけであり、**外部の DXF リーダーがこのファイルを受理することの
    /// 証明ではない**。目的は codec 回帰（ヘッダを下げる・クレートを上げるなどで
    /// エスケープ経路へ戻る変化）の検知であって、相互運用性の保証ではない。
    /// 相互運用性は手動確認（LibreCAD 実機）で担保している
    /// （DESIGN.md M6 設計判断5）。
    #[test]
    fn save_dxf_writes_cjk_text_as_utf8() {
        let dir = std::env::temp_dir().join("mcad-io-test");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cjk_text.dxf");

        const CONTENT: &str = "寸法テスト";

        let mut doc = Document::new();
        let layer = doc.current_layer();
        doc.apply(Command::AddEntity(Entity::new(
            EntityGeom::Text(TextGeom {
                anchor: Point2::new(0.0, 0.0),
                content: CONTENT.into(),
                height: 1.0,
                angle: 0.0,
            }),
            layer,
            Style::inherited(),
        )))
        .unwrap();

        let skipped = save_dxf(&doc, &path).unwrap();
        assert_eq!(skipped, 0);

        // R2007 の DXF は UTF-8 テキストなので、そのままデコードできる。
        let text = String::from_utf8(fs::read(&path).unwrap()).unwrap();

        // group code 1（TEXT の値）の直後の行が、エスケープされていない元の文字列。
        let value_lines: Vec<&str> = text
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .windows(2)
            .filter(|w| w[0].trim() == "1")
            .map(|w| w[1])
            .collect();
        assert!(
            value_lines.contains(&CONTENT),
            "group code 1 の値行に UTF-8 のままの {CONTENT:?} が見つからない: {value_lines:?}"
        );

        // `\U+XXXX` エスケープ経路（R2004 以下）に入っていないこと。ヘッダを
        // 下げるとここで落ちる。
        assert!(
            !text.contains("\\U+"),
            "R2007 では \\U+XXXX エスケープが使われてはならない"
        );
        assert_eq!(
            export_dxf(&doc).drawing.header.version,
            dxf::enums::AcadVersion::R2007,
            "ヘッダバージョンは R2007（理由は export_dxf のコメント）"
        );

        fs::remove_file(&path).ok();
    }

    /// `dxf` 0.6.1 の R2004 以下の文字列 codec が**実際に壊した** 4 ケースの
    /// ファイル往復テスト。export のヘッダバージョンを R2007 未満へ下げると
    /// **必ずこのテストが落ちる**（それが存在理由）。
    ///
    /// R2004 以下では書き出しが `escape_unicode_to_ascii`、読込が
    /// `un_escape_ascii_to_unicode` を通る。それぞれの壊れ方（2026-07-25 実測）:
    ///
    /// 1. 非BMP文字 `😀`（U+1F600）: `\U+1F600` と書かれるが、デコーダが 16 進を
    ///    4 桁ちょうどしか消費しないため `ὠ`（U+1F60）+ `0` の 2 文字に化ける。
    /// 2. 末尾バックスラッシュ `path\`: 末尾の `\` がエスケープ開始と誤認され、
    ///    未完のシーケンスが flush されずに消える。
    /// 3. 中間のバックスラッシュ `C:\temp\a`: 上と同じ経路。単体では壊れなかったが
    ///    エスケープ開始文字を含む代表ケースとして固定する。
    /// 4. リテラル `\U+0041`（`\` + `U+0041` の 7 文字）: 書き出し側が
    ///    バックスラッシュを二重化しないため、読込で `A` 1 文字に潰れる。
    ///
    /// R2007（UTF-8 経路）ではエスケープを通らないため 4 ケースすべてが完全一致する。
    /// メモリ内往復（`export_dxf` → `import_dxf`）ではこの codec を通らないので、
    /// **必ずファイル経由**で往復させること。
    ///
    /// # このテストの位置づけ（何を証明していないか）
    ///
    /// 書き出しも読み込みも同じ `dxf` 0.6.1 が行うため、証明できるのは
    /// **同一ライブラリ内でエンコードとデコードが対称に働く**ことだけである。
    /// **外部の DXF リーダーがこのファイルを受理することの証明ではない**
    /// （対称に壊れれば往復は一致してしまう。それを補うため、ファイルに何が
    /// 書かれるかは `save_dxf_writes_cjk_text_as_utf8` が生バイトで別途固定する）。
    /// 目的は codec 回帰の検知であって相互運用性の保証ではなく、相互運用性は
    /// 手動確認（LibreCAD 実機）で担保している（DESIGN.md M6 設計判断5）。
    #[test]
    fn round_trip_preserves_pathological_text() {
        let dir = std::env::temp_dir().join("mcad-io-test");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pathological_text.dxf");

        let contents = [
            "OK 😀 done",  // 1. 非BMP（U+1F600）
            "path\\",      // 2. 末尾バックスラッシュ
            "C:\\temp\\a", // 3. 中間バックスラッシュ
            "\\U+0041",    // 4. リテラル `\U+XXXX`（7 文字。`A` に化けてはいけない）
        ];

        let mut doc = Document::new();
        let layer = doc.current_layer();
        for (i, content) in contents.iter().enumerate() {
            doc.apply(Command::AddEntity(Entity::new(
                EntityGeom::Text(TextGeom {
                    anchor: Point2::new(i as f64, 0.0),
                    content: (*content).into(),
                    height: 1.0,
                    angle: 0.0,
                }),
                layer,
                Style::inherited(),
            )))
            .unwrap();
        }

        assert_eq!(save_dxf(&doc, &path).unwrap(), 0);
        let summary = load_dxf(&path).unwrap();
        assert_eq!(summary.skipped_entities, 0);
        assert_eq!(summary.document.entity_count(), contents.len());

        for (expected, (_, entity)) in contents.iter().zip(summary.document.entities()) {
            let EntityGeom::Text(actual) = &entity.geom else {
                panic!("Text エンティティとして復元されるはず: {:?}", entity.geom);
            };
            assert_eq!(
                *expected, actual.content,
                "文字列が往復で壊れた（ヘッダバージョンを下げていないか確認すること）"
            );
        }

        fs::remove_file(&path).ok();
    }

    /// タスク25b: TEXT の往復（CJK・ASCII の両方）。
    ///
    /// **ファイル経由**で往復させるのが要点。文字列のエンコード・デコードは
    /// `dxf` クレートのコードペア書き出し・読み込みで起きるため、`export_dxf` →
    /// `import_dxf` のメモリ内往復では codec をまったく通らず、CJK が正しく
    /// 書かれ読み戻されるかを検証できない。境界ケース（非BMP・バックスラッシュ）は
    /// `round_trip_preserves_pathological_text` が担当する。
    #[test]
    fn round_trip_preserves_cjk_and_ascii_text() {
        let dir = std::env::temp_dir().join("mcad-io-test");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("text_roundtrip.dxf");

        let texts = [
            TextGeom {
                anchor: Point2::new(1.5, -2.25),
                content: "寸法テスト".into(),
                height: 3.0,
                angle: FRAC_PI_2,
            },
            TextGeom {
                anchor: Point2::new(-4.0, 8.0),
                content: "Ascii Label 123".into(),
                height: 0.5,
                angle: 0.0,
            },
        ];

        let mut doc = Document::new();
        let layer = doc.current_layer();
        for text in &texts {
            doc.apply(Command::AddEntity(Entity::new(
                EntityGeom::Text(text.clone()),
                layer,
                Style::inherited(),
            )))
            .unwrap();
        }

        assert_eq!(save_dxf(&doc, &path).unwrap(), 0);
        let summary = load_dxf(&path).unwrap();
        assert_eq!(summary.skipped_entities, 0);
        assert_eq!(summary.document.entity_count(), texts.len());

        for (expected, (_, entity)) in texts.iter().zip(summary.document.entities()) {
            let EntityGeom::Text(actual) = &entity.geom else {
                panic!("Text エンティティとして復元されるはず: {:?}", entity.geom);
            };
            assert_eq!(expected.content, actual.content);
            approx_point(expected.anchor, actual.anchor);
            assert!(
                (expected.height - actual.height).abs() < EPS,
                "height mismatch: {} vs {}",
                expected.height,
                actual.height
            );
            // angle はラジアン→度→ラジアンで往復するので丸め誤差を許容する。
            assert!(
                (expected.angle - actual.angle).abs() < ANGLE_EPS,
                "angle mismatch: {} vs {}",
                expected.angle,
                actual.angle
            );
        }

        fs::remove_file(&path).ok();
    }

    /// タスク35c: export ヘッダの `$INSUNITS` は常に mm（値 4）。
    /// DESIGN.md M8 設計判断1、モジュール doc「単位（$INSUNITS）」。
    #[test]
    fn export_writes_insunits_millimeters() {
        let doc = Document::new();
        let export = export_dxf(&doc);
        assert_eq!(
            export.drawing.header.default_drawing_units,
            DxfUnits::Millimeters
        );
    }

    /// タスク35c: TEXT 高さの尺度契約（export 側）。尺度 1:2（`k = 2.0`）の図面で
    /// 紙 mm 3.5 の Text は DXF に `3.5 * 2.0 = 7.0` として書かれる（現行の
    /// 無変換転写は 1:1 でのみ偶然正しかった。DESIGN.md M8 設計判断4a）。
    #[test]
    fn text_height_scales_by_sheet_scale_on_export() {
        use mcad_core::Scale;

        let mut doc = Document::new();
        let mut sheet = doc.sheet().clone();
        sheet.scale = Scale::new(1, 2).unwrap(); // 1:2、k = den/num = 2.0
        doc.apply(Command::SetSheet(sheet)).unwrap();

        let layer = doc.current_layer();
        doc.apply(Command::AddEntity(Entity::new(
            EntityGeom::Text(TextGeom {
                anchor: Point2::new(0.0, 0.0),
                content: "h".into(),
                height: 3.5,
                angle: 0.0,
            }),
            layer,
            Style::inherited(),
        )))
        .unwrap();

        let export = export_dxf(&doc);
        let text_entity = export
            .drawing
            .entities()
            .find_map(|e| match &e.specific {
                EntityType::Text(t) => Some(t),
                _ => None,
            })
            .expect("TEXT entity must be present");
        assert!(
            (text_entity.text_height - 7.0).abs() < EPS,
            "expected 3.5 * k(2.0) = 7.0, got {}",
            text_entity.text_height
        );
    }

    /// タスク35c: TEXT 高さの尺度契約（往復）。1:2 で紙 mm 3.5 → DXF 7.0 →
    /// import（尺度の概念がないので 1:1 とみなす）→ 紙 mm 7.0。
    ///
    /// 「紙 mm としての意味」は往復しないが、**モデル空間での幾何サイズ
    /// （ワールド高さ）は正しく保存される**ことをここで固定する: 元の図面での
    /// ワールド高さは `3.5 * 2.0 = 7.0`、import 後の図面（尺度 1:1）での
    /// ワールド高さは `7.0 * 1.0 = 7.0` で一致する（DESIGN.md M8 設計判断4a）。
    #[test]
    fn text_geometric_height_survives_round_trip_across_scale_reinterpretation() {
        use mcad_core::Scale;

        let mut doc = Document::new();
        let mut sheet = doc.sheet().clone();
        sheet.scale = Scale::new(1, 2).unwrap();
        doc.apply(Command::SetSheet(sheet)).unwrap();

        let layer = doc.current_layer();
        doc.apply(Command::AddEntity(Entity::new(
            EntityGeom::Text(TextGeom {
                anchor: Point2::new(0.0, 0.0),
                content: "h".into(),
                height: 3.5,
                angle: 0.0,
            }),
            layer,
            Style::inherited(),
        )))
        .unwrap();

        let export = export_dxf(&doc);
        let summary = import_dxf(&export.drawing).unwrap();
        assert_eq!(summary.skipped_entities, 0);

        let (_, entity) = summary.document.entities().next().unwrap();
        let EntityGeom::Text(text) = &entity.geom else {
            panic!("Text エンティティのはず: {:?}", entity.geom);
        };
        // import 後の図面は尺度 1:1（既定）なので、「紙 mm」の値がそのまま
        // ワールド高さになる。
        assert!(
            (text.height - 7.0).abs() < EPS,
            "geometric size must survive: expected 7.0, got {}",
            text.height
        );
        assert_eq!(summary.document.sheet().scale, mcad_core::Scale::ONE);
    }

    /// タスク35c: レイヤーの線種は export/import で往復する。
    #[test]
    fn layer_linetype_round_trips() {
        let mut doc = Document::new();
        let default = doc.default_layer();
        let mut props = doc.layer(default).unwrap().clone();
        props.linetype = Linetype::DashDot;
        doc.apply(Command::SetLayerProps { id: default, props })
            .unwrap();

        let export = export_dxf(&doc);
        let dxf_layer = export
            .drawing
            .layers()
            .find(|l| l.name == "0")
            .expect("default layer must be present");
        assert_eq!(dxf_layer.line_type_name, "DASHDOT");

        let summary = import_dxf(&export.drawing).unwrap();
        let imported_default = summary
            .document
            .layer(summary.document.default_layer())
            .unwrap();
        assert_eq!(imported_default.linetype, Linetype::DashDot);
    }

    /// タスク35c: レイヤーの線幅は `dxf` 0.6.1 の API 制約で export できない
    /// （`LineWeight` に任意値を作れる公開コンストラクタがない。モジュール doc
    /// 「線幅・線種は best-effort」参照）。export したレイヤーは常に raw `0`
    /// （既定）になり、re-import では「未指定」として `WidthMm::DEFAULT`
    /// （0.35mm）へ復元され、クランプとしては計上されないことを固定する
    /// （往復しないことを意図的に記録するテスト）。
    #[test]
    fn layer_line_weight_cannot_be_exported_and_import_resets_to_default() {
        let mut doc = Document::new();
        let default = doc.default_layer();
        let mut props = doc.layer(default).unwrap().clone();
        props.width_mm = WidthMm::new(1.4).unwrap();
        doc.apply(Command::SetLayerProps { id: default, props })
            .unwrap();

        let export = export_dxf(&doc);
        let dxf_layer = export
            .drawing
            .layers()
            .find(|l| l.name == "0")
            .expect("default layer must be present");
        assert_eq!(
            dxf_layer.line_weight.raw_value(),
            0,
            "layer line weight cannot be written by dxf 0.6.1's public API"
        );

        // raw 0 は「未指定」として扱われ（mcad 自身が export した DXF は必ず
        // ここを通るため、クランプとしては計上しない。モジュール doc参照）、
        // 既定 0.35mm に戻る。1.4mm という明示値だったことは失われる。
        let summary = import_dxf(&export.drawing).unwrap();
        let imported_default = summary
            .document
            .layer(summary.document.default_layer())
            .unwrap();
        assert_eq!(imported_default.width_mm, WidthMm::DEFAULT);
        assert_eq!(
            summary.clamped_line_widths, 0,
            "raw 0 is treated as unspecified, not an out-of-range explicit value"
        );
    }

    /// タスク35c: エンティティ個別の線幅・線種の上書き（`Style::width_mm` /
    /// `Style::linetype`）は export/import で往復する（`lineweight_enum_value` /
    /// `line_type_name` はレイヤーと違い任意値を書ける生フィールドのため）。
    #[test]
    fn entity_style_width_and_linetype_override_round_trips() {
        let mut doc = Document::new();
        let layer = doc.current_layer();
        doc.apply(Command::AddEntity(Entity::new(
            Shape::Line(LineSeg::new(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0))),
            layer,
            Style {
                color: None,
                width_mm: Some(WidthMm::new(0.7).unwrap()),
                linetype: Some(Linetype::DashDotDot),
            },
        )))
        .unwrap();

        let export = export_dxf(&doc);
        let entity = export.drawing.entities().next().expect("entity present");
        assert_eq!(entity.common.line_type_name, "DIVIDE");
        assert_eq!(entity.common.lineweight_enum_value, 70);

        let summary = import_dxf(&export.drawing).unwrap();
        assert_eq!(summary.clamped_line_widths, 0);
        let (_, imported) = summary.document.entities().next().unwrap();
        assert_eq!(imported.style.linetype, Some(Linetype::DashDotDot));
        assert_eq!(
            imported.style.width_mm.map(WidthMm::mm),
            Some(WidthMm::new(0.7).unwrap().mm())
        );
    }

    /// タスク35c: `Style::width_mm`/`Style::linetype` が `None`（ByLayer）の
    /// エンティティは、DXF の `BYLAYER`（線種既定値）・raw `-1`（線幅）で往復する。
    #[test]
    fn entity_style_bylayer_width_and_linetype_round_trips() {
        let mut doc = Document::new();
        let layer = doc.current_layer();
        doc.apply(Command::AddEntity(Entity::new(
            Shape::Line(LineSeg::new(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0))),
            layer,
            Style::inherited(),
        )))
        .unwrap();

        let export = export_dxf(&doc);
        let entity = export.drawing.entities().next().expect("entity present");
        assert_eq!(entity.common.line_type_name, "BYLAYER");
        assert_eq!(entity.common.lineweight_enum_value, -1);

        let summary = import_dxf(&export.drawing).unwrap();
        assert_eq!(summary.clamped_line_widths, 0);
        let (_, imported) = summary.document.entities().next().unwrap();
        assert_eq!(imported.style.linetype, None);
        assert_eq!(imported.style.width_mm, None);
    }

    /// タスク35c: 範囲外の DXF lineweight（10mm 相当の raw 1000）は
    /// `WidthMm` の範囲（0.05..=5.0mm）へクランプして取り込み、
    /// `ImportSummary::clamped_line_widths` に計上する（黙って丸めない）。
    #[test]
    fn out_of_range_entity_lineweight_is_clamped_and_counted() {
        let mut drawing = Drawing::new();
        while drawing.remove_layer(0).is_some() {}
        drawing.add_layer(DxfLayer {
            name: "0".to_string(),
            ..Default::default()
        });

        let mut entity = DxfEntity::new(EntityType::Line(DxfLine {
            p1: DxfPoint::new(0.0, 0.0, 0.0),
            p2: DxfPoint::new(1.0, 0.0, 0.0),
            ..Default::default()
        }));
        entity.common.layer = "0".to_string();
        entity.common.lineweight_enum_value = 1000; // 10.0mm、範囲外
        drawing.add_entity(entity);

        let summary = import_dxf(&drawing).unwrap();
        assert_eq!(summary.skipped_entities, 0);
        assert_eq!(summary.clamped_line_widths, 1);
        let (_, imported) = summary.document.entities().next().unwrap();
        assert_eq!(
            imported.style.width_mm.map(WidthMm::mm),
            Some(WidthMm::MAX_MM)
        );
    }

    /// タスク35c: 未知の DXF 線種名（他 CAD 由来）は `Linetype::Continuous` へ
    /// フォールバックする（DESIGN.md M8 設計判断5）。無視してカウントするのでは
    /// なく確定的に解釈するため `skipped_entities`/`clamped_line_widths` は
    /// 増えない。
    #[test]
    fn unknown_dxf_line_type_name_falls_back_to_continuous() {
        let mut drawing = Drawing::new();
        while drawing.remove_layer(0).is_some() {}
        drawing.add_layer(DxfLayer {
            name: "0".to_string(),
            ..Default::default()
        });

        let mut entity = DxfEntity::new(EntityType::Line(DxfLine {
            p1: DxfPoint::new(0.0, 0.0, 0.0),
            p2: DxfPoint::new(1.0, 0.0, 0.0),
            ..Default::default()
        }));
        entity.common.layer = "0".to_string();
        entity.common.line_type_name = "ACAD_ISO02W100".to_string(); // 未知の名前
        drawing.add_entity(entity);

        let summary = import_dxf(&drawing).unwrap();
        assert_eq!(summary.skipped_entities, 0);
        assert_eq!(summary.clamped_line_widths, 0);
        let (_, imported) = summary.document.entities().next().unwrap();
        assert_eq!(imported.style.linetype, Some(Linetype::Continuous));
    }

    /// タスク35c: 未知の DXF レイヤー線種名は `Linetype::Continuous` へ
    /// フォールバックする（レイヤーはエンティティと違い ByLayer 概念がないため
    /// 常に確定値になる）。
    #[test]
    fn unknown_dxf_layer_line_type_name_falls_back_to_continuous() {
        let mut drawing = Drawing::new();
        while drawing.remove_layer(0).is_some() {}
        drawing.add_layer(DxfLayer {
            name: "0".to_string(),
            line_type_name: "SOME_OTHER_CAD_LINETYPE".to_string(),
            ..Default::default()
        });

        let summary = import_dxf(&drawing).unwrap();
        let layer = summary
            .document
            .layer(summary.document.default_layer())
            .unwrap();
        assert_eq!(layer.linetype, Linetype::Continuous);
    }
}
