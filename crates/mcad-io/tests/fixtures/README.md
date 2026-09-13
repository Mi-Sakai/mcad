# `mcad-io` のテスト fixture

| ファイル | 用途 | テスト |
|---|---|---|
| `v1.mcad`〜`v6.mcad` | `.mcad` の後方互換 | `tests/backward_compat.rs` |
| `synthetic_dimensions.dxf` | 他 CAD が書いた DXF の DIMENSION import | `tests/dxf_dimensions.rs` |

## `synthetic_dimensions.dxf`

**合成データ**。リポジトリは GitHub で公開されているため、ユーザーが実際に
LibreCAD で描いた図面はそのまま同梱できない。座標は単純な値
（斜辺 `(0,0)`–`(120,30)`、鉛直線 `(150,0)`–`(150,40)`、円 中心 `(60,80)` 半径 `25`
など）へ差し替えてあるが、**構造（group code の並び・subclass marker・anonymous
block）は LibreCAD（libdxfrw 0.6.3）が実際に書いたファイルのまま踏襲している**。
`ENTITIES` は `LINE` 4・`CIRCLE` 1・`DIMENSION` 6 を持ち、寸法 6 件は整列・
回転（水平/鉛直）・半径・直径・2 直線角度を 1 件ずつ含む。

定義点（group 10/11/13〜16 等）の解釈が正しいことは、差し替え前の実ファイル
（ユーザーが LibreCAD で描いた図面）で検証済み（DESIGN.md M11 章参照）。この
fixture 自体の役割は「今後の実装変更で読込結果が変わらないこと」を固定する回帰
テストであり、実測値の出所としては扱わない。半径/直径寸法の期待値は本ファイル内
の `CIRCLE`（中心 `(60,80)`・半径 `25`）と突き合わせて `tests/dxf_dimensions.rs`
側で計算し直している。

見た目のブロック（寸法線・矢先・文字）は LibreCAD が anonymous block `*D1`〜`*D6`
として書くが、mcad は `BLOCKS` を読まないため、この fixture では各ブロックの中身を
`LINE` 1〜2 本まで簡略化してある（矢先・文字ジオメトリまで再現する必要はない）。

手で書き換えないこと（座標・構造を変えるとテストの期待値と回帰の意味が両方
変わる）。別の CAD の出力を足したいときは新しいファイルとして追加する。

## `.mcad` 後方互換 fixture

`v1.mcad` 〜 `v6.mcad` は各フォーマットバージョンのスキーマが表現できる要素を
一通り含む実ファイル。テストは `crates/mcad-io/tests/backward_compat.rs`。

各版のスキーマの一次情報は `crates/mcad-io/src/mcad_file.rs` のモジュール doc と
DTO 定義（`FileDocument` / `FileLayer` / `EntityGeomV5` 等）。fixture を手で書くときは
必ずそちらを見て、その版のスキーマに存在しないキーを混ぜないこと。

**壊れた入力(不正 JSON・偽装 `Table` 等)のテストはここへ置かない。** それらは
「正規のサンプル」という fixture ディレクトリの位置づけと紛らわしいので、
`mcad_file.rs` の `#[cfg(test)]` に残す。

## v7 を追加するとき

`.mcad` フォーマットが v7 へ上がったら:

1. 現行版の mcad で保存したファイル(または既存 fixture と同じ流儀で手作りした
   JSON)を `v7.mcad` として追加する。新フィールドを含めること。
2. `backward_compat.rs` に `v7_fixture_loads_with_expected_content` を1ケース足す
   (新フィールドの既定値補完・ラウンドトリップを確認)。
3. `all_fixtures_declare_their_own_version_and_reexport_as_current` は
   `1..=FORMAT_VERSION` を回すだけなので、`FORMAT_VERSION` を7へ上げれば
   自動的に v7 も対象に入る。
4. 旧版(v1〜v6)が変わらず読めることは、既存の `v1_*`〜`v6_*` テストがそのまま
   回帰網として働く(変更不要)。
