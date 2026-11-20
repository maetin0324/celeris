---
title: アーキ v4 H R4 measure — 測定の script・base.txt・metrics.tsv の before と進捗 file を置く
tasks: [01M7TWBX8YYDKHV6C2JWDJS1VR]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# measure（WorkUnit）

完了日: 2026-11-19

## 0. 結論

- [measure.sh](measure.sh) を POSIX sh で書いた。`measure.sh <rev>` は 3 行 `<metric> <value>`、`measure.sh files <metric> <rev>` は file の一覧を出す。
- [base.txt](base.txt) に開始時の `git merge-base HEAD main` の sha を書いた（main の commit「integrate wu/close (phase close)」。計画時の値と同じ）。
- before の値は rusqlite_prod_files 75・sql_string_prod_files 50・conn_open_files 47 で、計画時の値と一致した。
  [metrics.tsv](metrics.tsv) の before に写し、after は `-` にした。
- 進捗 [2026-11-19-arch-v4-h-r4.md](../2026-11-19-arch-v4-h-r4.md) を status: running で作った。
- source（`crates/`・`scripts/`・`web/`）と policy の file は変えていない。

## 1. measure.sh の数え方

- rusqlite_prod_files・sql_string_prod_files は [struct-store.md](../2026-11-19-arch-v4-h/struct-store.md) §6 の C4 の 2 つの
  `git grep` を写した。除外も同じ: `X`（tests・benches・examples の dir、`tests.rs`・`*_test(s).rs`・`test_support.rs`・`*_fixture.rs`）、
  `P`（`crates/campaign`・`routing-eval`・`jev-classifier`）、`crates/task-core/src/store/`。
- conn_open_files は同じ除外で `git grep -lE 'Connection::open|TransactionBehavior|busy_timeout'` の file。H の final-report の
  提案 12 の補助の指標。`busy_timeout` の設定欄の名前（task-api の `lib.rs`・`types.rs`・`state.rs`・`handlers/system.rs`、
  celeris の `config/` ほか）も数えるので、接続を開く file の数そのものではない。
- rev は `git rev-parse --verify <rev>^{commit}` で sha にしてから `git grep <sha>` に渡す（作業ツリーは読まない。branch 名に `/` があっても
  sed の置換が壊れない）。repo の root へ `cd` してから数えるので、どの dir から呼んでも同じ値になる。
- 引数の誤り・未知の metric・解決できない rev は usage を stderr に出して exit 2。

## 2. 検査（提出時の HEAD で実行）

| 条件 | コマンド | 結果 |
|---|---|---|
| measure.sh <base> が 3 行を出し metrics.tsv の before と一致 | `dash measure.sh "$(cat base.txt)"` と tsv の before の突き合わせ | exit 0。75・50・47 で 3 行とも一致 |
| H の C4 の command と同じ値 | struct-store.md §6 の 2 行を base に当てる | 75・50（measure.sh と同じ） |
| files の行数が数と一致 | `dash measure.sh files <metric> <base> \| wc -l` | 75・50・47 |
| base.txt が merge-base で HEAD の祖先 | `git merge-base HEAD main`、`git merge-base --is-ancestor <base.txt> HEAD` | 同じ sha、exit 0 |
| 進捗の front matter と hash | `sh scripts/dev/progress-index.sh --check` | exit 0 |

base で task-api の conn_open_files に入る file は `chat/attachments.rs`（`Connection::open` 3 行）・`cos/triage_view.rs`
（`Connection::open` と `busy_timeout`）・`lib.rs`・`types.rs`・`state.rs`・`handlers/system.rs` の 6 つ。`cos/inbox.rs` の `triage_db` は
DB の path を返すだけで、この指標に入らない（WorkUnit triage が消しても数は変わらない）。

## 3. 未解決事項

- 無し。after は WorkUnit after が `measure.sh HEAD` で埋める。
