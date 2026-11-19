---
title: アーキ v4 H R7b — 修正の波及 C10 を B0 の後の窓に当てる
tasks: [01M7MZTKBPZPQE4XN451F8X84F]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# c10-window（WorkUnit r7-rules）

A の C10 の snippet（A の baseline.md §7 C10。H の [final-report.md](final-report.md) の「再現コマンド」節が写す）を変えずに、
B0（main `bc665168c9caf1898adc4c611404bbee056daf9a`、2026-11-17T11:48Z）から今の main（件名 `integrate wu/close (phase close)` の commit、
2026-11-19T10:37Z）までの窓に、no-merges と with-merges の両方で当てた。数は [c10-window.tsv](c10-window.tsv)（列 metric scope before after checker note）。
規則の付記は [ADR 2026-11-19-arch-v4-measure-rules-addendum](../../adr/2026-11-19-arch-v4-measure-rules-addendum.md) の D5。

## 0. 結論

**未達（窓の件数が不足）**。B0 の後の窓で production を変えた通常の修正（no-merges）は **1 件**で、比べる下限の 30 件に満たない。
with-merges でも 9 件で、9 件はアーキ v4 の子 task・WorkUnit の変更を束ねた merge なので、通常の修正の代わりにならない。
修正の波及（共通受入条件 (4) の #13）の before/after は、この窓では比べられない。

- B0 の後 2 週間の窓は **2026-12-01T00:00:00Z** に閉じる（SINCE 2026-11-17T00:00:00Z の 14 日後）。閉じた後に下の §5 の command を同じ規則で当て直す。
- 2026-12-01 の時点で no-merges が 30 件に届くかは未確認。main の first-parent は 11-18 以降ほとんど merge commit で（§3）、
  通常の修正が branch の merge で入る限り no-merges の件数は増えにくい。届かなかったときの扱い（窓を延ばす、with-merges で数える等）は人が決める。
- 本番への昇格・release はしていない。この unit は文書だけを変えた（crates/・scripts/・web/ と policy の file は変えていない）。

## 1. 規則

- snippet は A の baseline.md §7 C10 の heredoc を `sed` でそのまま `$TMPDIR/c10.py` に取り出した（26 行）。手を加えていない。
- production の判定は B0 の `scripts/dev/source-size-report.py` と `.toml`（`git show bc665168:<path>`）の `path_class`（exclusive の規則）。checker の欄は `ssr@bc665168`。
- region は `crates/<c>` か top-level の dir、module は `crates/<c>/src/<第 1 要素>`（lib.rs・main.rs は `crates/<c>/src`）か親 dir（A の §6 の 7）。
- after の窓は A の §7 C10 の「after」の行のとおり REV を範囲にし、SINCE は区間の始まりより前の 2026-11-17T00:00:00Z にした。
  H の loc-cost は REV を `bc665168..HEAD`（H の branch。docs の commit が 1 件多い）にしたが、この unit は objective のとおり `bc665168..main` にした。
  production を変えた commit の数は両方とも同じ（no-merges 1・with-merges 9）。

## 2. 結果

| 窓 | 数え方 | commit | production を変えた | 複数 region | 複数 module |
|---|---|---|---|---|---|
| A の窓（`bc665168`、SINCE 2026-10-28T00:00:00Z）の再現 | no-merges | 492 | 194 | 91（46.9 %） | 134（69.1 %） |
| 同 | with-merges | 653 | 246 | 122（49.6 %） | 175（71.1 %） |
| B0 の後（`bc665168..main`、SINCE 2026-11-17T00:00:00Z） | no-merges | 11 | **1** | 0（0.0 %） | 0（0.0 %） |
| 同 | with-merges | 40 | **9** | 5（55.6 %） | 5（55.6 %） |

- A の窓の再現は A の値と全列で一致した（no-merges 28 s、with-merges 55 s）。snippet の取り出しに誤りは無い。
- no-merges の 1 件は C の前提の commit `PathClass に任意の欄 invariants を足し、往復試験と schema を更新する`（task-core の 1 module）。
  ほかの非 merge の 10 件は文書だけの commit で、production を変えていない。
- with-merges の 9 件（commit ごとの内訳は snippet の集計の直前に print を 1 行足した写しで出した）:
  - C: 上の PathClass の commit（1 region・1 module）、`integrate wu/prompts (phase impl)`（3 region・8 module）、`integrate wu/cases (phase verify)`（1・1）。
  - E: `integrate wu/wu-region (phase impl)`・`integrate wu/cos-region (phase impl)`（どちらも task-dispatch の 1 module）。
  - F: `integrate wu/lifecycle-tx (phase impl)`（2 region・4 module）・`integrate wu/knowledge-pin (phase impl)`（2・3）。
  - main を E の branch（`celeris/01M7MZTKB6GQ241F6JJYS…`）に取り込んだ merge（4 region・11 module）と、F の branch（`celeris/01M7MZTKB8A4…`）に取り込んだ merge（3 region・3 module）。
- 1 件が WorkUnit 1 つ分か複数 task 分の変更を束ねるので、with-merges の 55.6 % を A の 49.6 % と並べて「悪化」とも「改善」とも言えない（比較不能）。

## 3. 窓の commit の形

main の first-parent の commit を commit 日（UTC）ごとに数えると、非 merge は 11-16 に 46 件・11-17 に 62 件（多くは B0 の前）、11-18 に 4 件・11-19 に 0 件、
merge は 11-18 に 13 件・11-19 に 11 件だった（`git log --first-parent --since=2026-10-28T00:00:00Z --format='%cI %P' main` を日ごとに集計）。
A の窓（10-28〜11-17）では非 merge が first-parent の 492 / 653 件（75 %）を占めたが、B0 の後の窓では 11 / 40 件（28 %）だった。
no-merges の件数は、修正の数だけでなく、変更が main に早送りで入るか merge commit で入るかで変わる。

## 4. 未達・要判断

| 項目 | 数値 | 理由 | 次の候補 |
|---|---|---|---|
| (4) #13 修正の波及 | before 194 件・46.9 %・69.1 %（no-merges）。after 1 件・0.0 %・0.0 %（no-merges）、9 件・55.6 %・55.6 %（with-merges） | B0 の後の窓（約 2.5 日）に通常の修正が 1 件しかない。with-merges の 9 件はアーキ v4 の変更の束 | 2026-12-01T00:00:00Z の後に §5 を当て直す。no-merges が 30 件に満たなければ、窓を延ばすか with-merges で数えるかを人が決める |

## 5. 再現コマンド

repo の root で実行する。`$TMPDIR` は run の一時 dir。

```sh
B=bc665168c9caf1898adc4c611404bbee056daf9a
BL=/local/celeris/data/workspaces/01M7MZTKAXM8GJGK8BPE13CQRM/artifacts/baseline.md   # A の run の成果物
git show $B:scripts/dev/source-size-report.py > $TMPDIR/ssr.py; git show $B:scripts/dev/source-size-report.toml > $TMPDIR/ssr.toml
sed -n '/^cat > \$TMPDIR\/c10.py <<.EOF.$/,/^EOF$/p' $BL | sed '1d;$d' > $TMPDIR/c10.py      # 26 行
# A の窓の再現（約 30 s と 55 s）
for m in no-merges with-merges; do python3 $TMPDIR/c10.py $TMPDIR/ssr.py $TMPDIR/ssr.toml 2026-10-28T00:00:00Z $m $B; done
#   no-merges: commits 492, production を変えた 194, 複数 region 91 (46.9%), 複数 module 134 (69.1%)
#   with-merges: commits 653, production を変えた 246, 複数 region 122 (49.6%), 複数 module 175 (71.1%)
# B0 の後の窓（1 s 前後）。2026-12-01T00:00:00Z の後も同じ command を当てる
M=$(git rev-parse main)
for m in no-merges with-merges; do python3 $TMPDIR/c10.py $TMPDIR/ssr.py $TMPDIR/ssr.toml 2026-11-17T00:00:00Z $m "$B..$M"; done
#   no-merges: commits 11, production を変えた 1, 複数 region 0 (0.0%), 複数 module 0 (0.0%)
#   with-merges: commits 40, production を変えた 9, 複数 region 5 (55.6%), 複数 module 5 (55.6%)
# commit ごとの内訳（集計の直前に print を 1 行足した写し）
sed "s/^    n+=1; mr+=/    print('C',h[:12],sorted({region(p) for p in prod}),len({module(p) for p in prod})); n+=1; mr+=/" $TMPDIR/c10.py > $TMPDIR/c10-list.py
for m in no-merges with-merges; do python3 $TMPDIR/c10-list.py $TMPDIR/ssr.py $TMPDIR/ssr.toml 2026-11-17T00:00:00Z $m "$B..$M"; done
```

## 6. 回帰条件

- A の窓の再現が 492・194・91・134（no-merges）と 653・246・122・175（with-merges）のままであること。崩れたら snippet の取り出しか B0 の分類の file が変わった。
- 2026-12-01T00:00:00Z の後に当て直した no-merges の production を変えた commit が 30 件以上なら、複数 region・複数 module の割合を A の 46.9 %・69.1 % と比べる。
