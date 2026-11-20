---
title: アーキ v4 H R5 measure — Applied の形の 3 指標の measure.sh・base.txt・metrics.tsv の before と進捗 file を置く
tasks: [01M7VJY8NZ5G84ACBNQTPVV3R4]
status: done
updated: 2026-11-20
completed: 2026-11-20
---
# measure（WorkUnit）

完了日: 2026-11-20

## 0. 結論

- [measure.sh](measure.sh) を POSIX sh で書いた。`measure.sh <rev>` は 3 行 `<metric> <value>`、`measure.sh files <metric> <rev>` は
  数えたもの（applied_wide・applied_narrow は file の path、applied_functions は `<path> <fn>`）を sort して 1 行ずつ出す。
- [base.txt](base.txt) に開始時の `git merge-base HEAD main` の 40 桁を書いた（main の commit「integrate wu/close (phase close)」）。
- before は applied_wide 18・applied_narrow 14・applied_functions 55 で、H の値（[struct-store.tsv](../2026-11-19-arch-v4-h/struct-store.tsv) の after）と
  一致した。[metrics.tsv](metrics.tsv) の before に写し、after は `-` にした。
- 進捗 [2026-11-19-arch-v4-h-r5.md](../2026-11-19-arch-v4-h-r5.md) を status: running で作った。
- source（`crates/`・`scripts/`・`web/`）と policy の file は変えていない。

## 1. measure.sh の数え方

- applied_wide・applied_narrow は [struct-store.md](../2026-11-19-arch-v4-h/struct-store.md) §6 の「Applied の形（D の付記）」の 2 つの
  `git grep` を写した。scope は `crates/task-api/src`。
  - applied_wide: `git grep -lE "Option<&('[a-z_]+ )?(crate::cos::operations::)?OperationAudit>" <rev> -- crates/task-api/src`
  - applied_narrow: `git grep -l 'Option<&OperationAudit>' <rev> -- crates/task-api/src`
- applied_functions は同 §6 の「# 補助: file ごとの関数（type Audit の行は除く）」の `for` と awk をそのまま写した（awk の program は 1 文字も
  変えていない）。出力 `<path> <fn>` を `sort -u` した行を数える。
- rev は `git rev-parse --verify <rev>^{commit}` で sha にしてから `git grep <sha>`・`git show <sha>:<path>` に渡す（作業ツリーは読まない）。
  repo の root へ `cd` してから数えるので、どの dir から呼んでも同じ値になる。`LC_ALL=C` で sort の順を固定した。
- 引数の誤り・未知の metric・解決できない rev は usage か理由を stderr に出して exit 2。

## 2. 実行した command と出力（提出時の HEAD で実行。repo の root で、`D=agent-docs/progress/2026-11-19-arch-v4-h-r5`）

```text
$ git merge-base HEAD main
1680de5cd1b4c57d5df3ceeab59d03a3783a0f6c        # base.txt と同じ
$ git merge-base --is-ancestor "$(cat $D/base.txt)" HEAD
exit 0
$ dash $D/measure.sh "$(cat $D/base.txt)"
applied_wide 18
applied_narrow 14
applied_functions 55
exit 0
$ dash $D/measure.sh HEAD                       # 開始時の HEAD（R5 の変更の前）
applied_wide 18
applied_narrow 14
applied_functions 55
exit 0
$ for m in applied_wide applied_narrow applied_functions; do dash $D/measure.sh files $m <rev> | wc -l; done
base: 18 14 55 / HEAD: 18 14 55               # 行数は値と等しい
$ (cd crates && dash ../$D/measure.sh "$(cat ../$D/base.txt)")
applied_wide 18 / applied_narrow 14 / applied_functions 55   # sub dir からでも同じ
```

H の command との突き合わせ（struct-store.md §6 の 3 行を base に bash でそのまま当てた）:

```text
狭い command の file 数: 14、広い command の file 数: 18、awk の行数: 55
awk の出力（sort -u）と `measure.sh files applied_functions <base>` の diff: 差分 0
```

base と開始時の HEAD の集合の差（`diff` で比べた）: applied_wide の file の集合・applied_functions の `<path> <fn>` の集合とも差分 0。
base の後に main 以外から入った commit（H の各 WorkUnit・R1〜R4・R7 の統合）の増減は 0。

metrics.tsv の before との突き合わせ（`measure.sh <base>` の各行と tsv の before・checker）:

```text
applied_wide measure=18 tsv=18/d-wide
applied_narrow measure=14 tsv=14/d-narrow
applied_functions measure=55 tsv=55/h-awk
```

誤りの扱い:

```text
$ dash $D/measure.sh                  → usage を stderr、exit 2
$ dash $D/measure.sh nosuchrev        → 「rev を commit に解決できない: nosuchrev」、exit 2
$ dash $D/measure.sh files bogus HEAD → usage、exit 2
$ dash -n $D/measure.sh               → exit 0
```

base の applied_wide の 18 file（`measure.sh files applied_wide <base>`）: `approvals.rs`・`changes.rs`・`cos/operations.rs`・`cron_jobs.rs`・
`decisions.rs`・`docs.rs`・`execution.rs`・`handlers/clusters.rs`・`handlers/projects.rs`・`handlers/task_actions.rs`・`handlers/tasks.rs`・
`inbox_notifications.rs`・`integrate_task.rs`・`knowledge.rs`・`lifecycle.rs`・`model_catalog.rs`・`repos.rs`・`skills.rs`（どれも
`crates/task-api/src/` の下）。狭い command に入らない 4 file は `cron_jobs.rs`・`execution.rs`・`handlers/clusters.rs`・`inbox_notifications.rs`。

R5 の 6 関数（`measure.sh files applied_functions <base> | grep -E 'knowledge|lifecycle|skills'`）:

```text
crates/task-api/src/knowledge.rs accept_op
crates/task-api/src/knowledge.rs put_page_op
crates/task-api/src/knowledge.rs reject_op
crates/task-api/src/lifecycle.rs task_pause_op
crates/task-api/src/skills.rs delete_skill_op
crates/task-api/src/skills.rs put_skill_op
```

文書の検査:

```text
$ sh scripts/dev/progress-index.sh --check   → progress-index --check: ok（exit 0）
$ bash scripts/dev/check-doc-links.sh        → check-doc-links: ok（exit 0）
$ bash scripts/dev/test-affected.sh          → 「試験対象の変更なし（docs のみ等）」、repo_health_gate: all checks passed（exit 0）
$ cargo fmt --all -- --check                 → exit 0
$ cargo clippy --workspace -- -D warnings    → exit 0
```

範囲: `git diff --name-only $CELERIS_WU_BASE HEAD` と `sh "$CELERIS_WU_SCOPE_PATHS"` は、どちらも
`agent-docs/progress/2026-11-19-arch-v4-h-r5.md` と `agent-docs/progress/2026-11-19-arch-v4-h-r5/` の 4 file だけ（`crates/` の変更 0）。

`checkbashisms` はこの host に入っていないので走らせていない。dash で実行して上の値を得た。

## 3. 未解決事項

- 無し。after は WorkUnit after が `measure.sh HEAD` で埋める。
