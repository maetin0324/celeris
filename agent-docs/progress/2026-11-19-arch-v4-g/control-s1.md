---
title: アーキ v4 G control-s1 — test_affected:S1 の before の測り直し（対象なし）
tasks: [01M7MZTKBKXKEVBWS87PS5J9NK]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# control-s1（WorkUnit）

## 結論: 対象なし

[flags.tsv](flags.tsv) の `test_affected	S1` の行は `flag=ok`（A の中央値 893.41 s、after の中央値 906.03 s、比 1.0141 = +1.4 %。
印の閾値は A の +10 % = 982.75 s）。印が無いので、この leaf は before（bc665168）の S1 を測らない（[compare.md](compare.md) §0 と同じ判断）。

- [control-s1.tsv](control-s1.tsv) は header だけ（列は [rules.md](rules.md) の「## TSV」）。各回の値・内訳・A との差は無い。
- before の worktree（rules.md「## BEFORE」の `$TMPDIR/wt-before`）は作っていない。`git worktree list` に wt-before は無い。
- `--list` の crate の数の照合（S1 は task-core を変える）も、測らないので行っていない。参考: control-build の run が before の worktree で
  `env -u CELERIS_WU_BASE bash scripts/dev/test-affected.sh --list` の S1（14 crate ＋ `web: typecheck+unit` ＋ `ts-verify: test-ts-verify`）が
  A の ta-list.txt と一致することを確かめている（rules.md「## BEFORE」の 4）。
- この leaf は source を変えていない。`git status --porcelain -- crates` と `git diff --stat "$CELERIS_WU_BASE" -- crates` は空（提出時の HEAD で実行）。

## 再現（判定）

```sh
awk -F'\t' '$1=="test_affected" && $2=="S1" {print $6}' agent-docs/progress/2026-11-19-arch-v4-g/flags.tsv   # ok
```
