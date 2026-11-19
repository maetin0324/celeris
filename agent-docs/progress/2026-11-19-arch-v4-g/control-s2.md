---
title: アーキ v4 G control-s2 — test_affected:S2 の before の測り直し（対象なし）
tasks: [01M7MZTKBKXKEVBWS87PS5J9NK]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# control-s2（WorkUnit）

## 結論: 対象なし

[flags.tsv](flags.tsv) の `test_affected	S2` の行は `flag=ok`（A の中央値 655.59 s、after の中央値 707.76 s、比 1.0796 = +8.0 %。
印の閾値は A の +10 % = 721.15 s）。印が無いので、この leaf は before（bc665168）の S2 を測らない（[compare.md](compare.md) §0 と同じ判断）。

- [control-s2.tsv](control-s2.tsv) は header だけ（列は [rules.md](rules.md) の「## TSV」）。この leaf が測った回は無く、各回の値・内訳は無い。
- before の worktree（rules.md「## BEFORE」の `$TMPDIR/wt-before`）は作っていない。control-s1 も作っておらず、`git worktree list` に wt-before は無い
  （消す対象も無かった）。
- この leaf は source を変えていない。`git status --porcelain -- crates` と `git diff --stat "$CELERIS_WU_BASE" -- crates` は空（提出時の HEAD で実行）。

## A・E との差（既存の値。この leaf は測っていない）

objective の「E の 1 回 769.32 s が host の揺れか dispatcher の変更由来か」を、measure 段の after の 3 回（[ta-mix.md](ta-mix.md)・[ta-s2.md](ta-s2.md)）と
E の [metrics.md](../2026-11-18-arch-v4-e/metrics.md) の (5) の値で並べた。before を同じ host で測り直していないので、下の読みは確定ではない。

| | A（before）の中央値 | after の run 1・2・3 | after の中央値 | after − A | E の 1 回 | after の中央値 − E |
|---|---|---|---|---|---|---|
| wall | 655.59 s | 722.11・707.76・653.34 s | 707.76 s | +52.17 s（+8.0 %） | 769.32 s | −61.56 s（−8.0 %） |
| user | 3,104 s | 3,566.71・3,093.42・2,990.88 s | 3,093.42 s | −10.58 s（−0.3 %） | 3,629.05 s | — |
| nextest の Summary の秒 | 406.636 s | 422.878・454.890・407.091 s | 422.878 s | +16.24 s（+4.0 %） | 443.874 s | — |
| nextest の passed | 3,917 | 3,938（3 回とも） | 3,938 | +21 | 3,936 | +2 |

- after の tree（`git merge-base HEAD main`、crates/ の tree は scenarios.env の `AFTER_CRATES_TREE`）は、E が測った commit
  （件名 `arch-v4 E counts: metrics.md に C3・E1〜E3・…`）を祖先に持つ（`git merge-base --is-ancestor` が 0）。after は E の dispatcher の変更
  （wu-region・cos-region）を全部含み、そのうえに F などの変更（E の commit から after まで crates/ の 39 file、S2 の 5 crate の中では 12 file）がある。
- 同じ dispatcher の変更を含む after の 3 回は 653.34〜722.11 s で、3 回とも E の 769.32 s より短い。user の中央値は A と同じ（−0.3 %）。
- E の 1 回と、起点の状態が近い after の run 1（どちらも作り直しの多い起点。test build は E 2m 19s、after run 1 2m 16s）の差は +47.21 s。
  内訳は test build +3 s、nextest +21.0 s（443.874 対 422.878 s）、残り（health・doc test など。769.32 − 139 − 443.874 = 186.4 s 対 163.2 s）+23.2 s。
- 読み: E の 769.32 s が after の 3 回より長いのは、dispatcher のコードではなく、1 回だけの測定の起点の状態と host の負荷の違いによる見込みが高い。
  ただし before を同じ時間帯に測っていないので未確認。after の +8.0 % がコード由来か host 由来かの切り分けは cause の leaf が扱う。

## 再現（判定）

```sh
awk -F'\t' '$1=="test_affected" && $2=="S2" {print $6}' agent-docs/progress/2026-11-19-arch-v4-g/flags.tsv   # ok
git merge-base --is-ancestor "$(git log --format=%h --grep='arch-v4 E counts: metrics.md に C3' "$(git merge-base HEAD main)" | head -1)" "$(git merge-base HEAD main)"; echo $?   # 0
git worktree list | grep -c wt-before   # 0
```
