# `.mcad` 後方互換 fixture

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
