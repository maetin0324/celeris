# artifact_exists はディレクトリの成果物を中身 1 件以上で合格にする

- 日付: 2026-10-18
- 状態: 実装済み
- tasks: [01M56DYJFG15PWKAYK2PN977G3]

## 背景
`crates/task-dispatch/src/review.rs` の `Check::ArtifactExists` は `is_file()` だけを見ていたため、
`artifacts/screenshots/` のようなディレクトリを指す条件は中身があっても「not found」で落ちた
（実例: Pairs PRA-29b 01M56D4B90JG46H289BAX51NX3、before/after 32 枚ずつあるのに不合格）。
CoS も人もディレクトリ名で条件を書く。

## 決定
- D1: 解決した path がディレクトリなら、その下の通常ファイル（再帰。symlink は辿らず数えない）が
  1 件以上で合格とする。0 件（空、または空の子ディレクトリだけ）は「empty directory」で不合格。
  存在しない名前は従来どおり「not found」で不合格。
- D2: 合格の reason は `path=<rel> dir files=<件数> sha256=<合成ハッシュ> [<相対 path を辞書順に , で連結>]`。
  合成ハッシュは辞書順に並べた各ファイルの `"<相対 path>\0<sha256>\n"` を連結したものの sha256。
  中身や名前が変われば値が変わる。
- D3: ファイルの挙動（`path=<rel> sha256=<sha>`）は変えない。

## 却下した案
- 条件の作成時（POST/PATCH /tasks・CoS の起票）に拒否する: 作成時には成果物はまだ無く、
  名前がファイルかディレクトリかを判別できない（拡張子の有無は手がかりにならない）。
  人の書き方を直させるより、検証側で中身を確かめる方が意図に合う。

## 試験
`crates/task-dispatch/src/review/tests.rs` の
`artifact_exists_directory_passes_with_files_and_fails_when_empty`。
旧い「ディレクトリは not found」を確かめる試験は無かった（消すものなし）。
