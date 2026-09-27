# `mcad-io` のテスト fixture

| ファイル | 用途 | テスト |
|---|---|---|
| `v1.mcad`〜`v8.mcad` | `.mcad` の後方互換 | `tests/backward_compat.rs` |
| `v7_legacy_boms.mcad` | v7 以前の旧部品表を表として読む(推測しない) | `tests/backward_compat.rs` |
| `v8_before_col_aligns.mcad` | `col_aligns` が無いタスク93以前の v8 を左揃え補完で読む | `tests/backward_compat.rs` |
| `synthetic_dimensions.dxf` | 他 CAD が書いた DXF の DIMENSION import | `tests/dxf_dimensions.rs` |

## `synthetic_dimensions.dxf`

**合成データ**。リポジトリは GitHub で公開されているため、ユーザーが実際に
LibreCAD で描いた図面はそのまま同梱できない。座標は単純な値
（斜辺 `(0,0)`–`(120,30)`、鉛直線 `(150,0)`–`(150,40)`、円 中心 `(60,80)` 半径 `25`
など）へ差し替えてあるが、**構造（group code の並び・subclass marker・anonymous
block）は LibreCAD（libdxfrw 0.6.3）が実際に書いたファイルのまま踏襲している**。
`ENTITIES` は `LINE` 4・`CIRCLE` 1・`DIMENSION` 6 を持ち、寸法 6 件は整列・
回転（水平/鉛直）・半径・直径・2 直線角度を 1 件ずつ含む。M11 タスク68 以降、
2 直線角度以外の 5 件がすべて取り込まれる（回転寸法は `DimDirection::Rotated`）。

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

`v1.mcad` 〜 `v8.mcad` は各フォーマットバージョンのスキーマが表現できる要素を
一通り含む実ファイル。テストは `crates/mcad-io/tests/backward_compat.rs`。

各版のスキーマの一次情報は `crates/mcad-io/src/mcad_file.rs` のモジュール doc と
DTO 定義（`FileDocument` / `FileLayer` / `EntityGeomV5` / `EntityGeomV6` /
`EntityGeomV7` / `DimLinearV6` / `TableGeomV7` / `SheetMetaV7` 等）。fixture を手で
書くときは必ずそちらを見て、その版のスキーマに存在しないキーを混ぜないこと
（**v6 以前に `DimLinear.direction` を、v7 以前に表の `kind`・`col_aligns`・`sheet` の
`use_bom` を書くと読込が失敗する** — 凍結 DTO が未知フィールドを拒否する）。

**壊れた入力(不正 JSON・偽装 `Table` 等)のテストはここへ置かない。** それらは
「正規のサンプル」という fixture ディレクトリの位置づけと紛らわしいので、
`mcad_file.rs` の `#[cfg(test)]` に残す。

## 次の版を追加するとき

手順は `backward_compat.rs` のモジュール doc「v9 を追加するとき」を参照。

`v7.mcad` は M11 タスク68 で追加。同じ斜辺 `(0,0)`–`(120,30)` に整列・水平回転・
鉛直回転の 3 通りの長さ寸法を付けてあり、向きによって表示値が実距離 123.693…・
水平投影 120・鉛直投影 30 と変わることを `backward_compat.rs` が固定している。

`v8.mcad` と `v7_legacy_boms.mcad` は M12 タスク81 で追加(手作り)。

- `v8.mcad` は部品表(`"kind": "Bom"`、ロック済みレイヤー上)・表(`"kind": "Table"`)・
  `"use_bom": false` を持つ。M12 タスク93 で列ごとの揃え(`col_aligns`)を足した(v8 に
  相乗り): 部品表は部品表プリセットの既定(部番・個数・材質・質量[kg] = 中央、品番・名称・
  備考 = 左)、表は中央・右。**現行版の書き出しと JSON として完全に一致する**ように
  書いてある(`v8_fixture_is_exactly_what_the_exporter_writes` が固定。キーの過不足・
  整数と小数の書き分けまで一致させること)。M12 タスク91 で構築線(`XLine`、基点
  `(10,20)`・単位方向 `(0.6,0.8)`・線種上書き `DashDot`)を 1 本足した(v8 に相乗り)。
- `v7_legacy_boms.mcad` は v0.11.0 以前の旧 5 列部品表と v0.11.1 の様式A/C 7 列部品表を
  1 つずつ持つ v7。どちらも表として読まれ(中身から部品表を推測しない)、読込が
  「部品表は 1 図面に 1 つ」に引っかからないことを固定する。

`v8_before_col_aligns.mcad` は M12 タスク93 で追加。タスク93 で `col_aligns` を
`TableGeom` へ足す**前**（タスク81〜92 の間）の実際のビルドが書く v8 を固定したもの
（`kind` はあるが `col_aligns` が無い。タスク93 直前のコミット（M12 タスク81「表の種類と
`.mcad` v8」）の `v8.mcad` そのもの）。**手で書き換えないこと**
（過去のビルドが実際に書いた形を保つのが目的）。読込は列数ぶんの左揃えで補って受理し
（Codex レビュー指摘。その時期の v8 は中央・右揃えを表現できなかったので情報を失わない）、
書き出すと `col_aligns` 付きの現行 v8 になることを `v8_before_col_aligns_fixture_is_backfilled_as_left`
が固定する。
