---
title: アーキ v4 G ta-s1 — after で test-affected の S1 を 2 回測る
tasks: [01M7MZTKBKXKEVBWS87PS5J9NK]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# ta-s1（WorkUnit）

after は `git merge-base HEAD main`（この WorkUnit の `$CELERIS_WU_BASE` と同じ crates/ の tree）。before は A の baseline.md §5.5 の S1 の値。
command は `/usr/bin/time -v bash scripts/dev/test-affected.sh`（A §7 C9）。probe は comment の probe（S1 の 4 file の最初の `//` の行の末尾に
` probe-<date +%s%N>`）。手順は [rules.md](rules.md) の「## TA」、TSV の列と valid の規則は同じ file の「## TSV」、file は [scenarios.env](scenarios.env)。
各回の行は [ta-s1.tsv](ta-s1.tsv)。S1 の 3 回目は ta-mix の leaf が測る。

## 0. 結論

- 2 回とも exit 0、開始時の 1 分 load が 4 以下で valid（測り直しは 0 回）。
- wall は 911.49 s・901.20 s。A の中央値 893.41 s に対して +2.0 %・+0.9 %。A の +10 % は 982.75 s。
  3 回目がどの値でも、3 回の中央値は 901.20〜911.49 s の間に入るので、+10 % を超えない。
- user の CPU 時間は 4,662.13 s・4,416.82 s（A の中央値 4,354 s に対して +7.1 %・+1.4 %）。
- 試験の数は nextest 8,481 passed・22 skipped（A は 8,436 passed・22 skipped。+45）。web と ts-verify の数は A と同じ。

| | run 1 | run 2 | A の run 1・2・3 | A の中央値 | A の +10 % |
|---|---|---|---|---|---|
| wall | 911.49 s | 901.20 s | 893.41・878.66・900.34 s | 893.41 s | 982.75 s |
| user | 4,662.13 s | 4,416.82 s | 4,550・4,354・4,352 s | 4,354 s | — |
| nextest の Summary の秒 | 497.035 s | 513.087 s | 483.7・503.2・511.2 s | 503.2 s | — |
| wall − nextest（health・web・ts-verify・build・doc test の合計） | 414.5 s | 388.1 s | 409.7・375.5・389.1 s | 389.1 s | — |

## 1. 各回の値と内訳

| run | wall | user | sys | exit | 開始時の load（1・5・15 分） | 待った秒 | 最大 RSS |
|---|---|---|---|---|---|---|---|
| 1 | 911.49 s | 4,662.13 s | 1,070.22 s | 0 | 2.51 / 7.43 / 10.72 | 0 | 3,644,292 kB |
| 2 | 901.20 s | 4,416.82 s | 1,014.29 s | 0 | 3.98 / 9.30 / 10.32 | 40 | 3,622,732 kB |

内訳（2 回とも同じ数。`raw/ta-summary.py` で log から取った）:

- health（test-affected の最初の check-health）: `repo-health: verdict=pass exit=0 findings=695 new=0 worsened=0`、`repo_health_gate: all checks passed`。
  A の S1 の log は findings=697。
- nextest: `Starting 8481 tests across 312 binaries (22 tests and 2 binaries skipped)`、8,481 passed・0 failed・22 skipped
  （slow は run 1 で 10、run 2 で 11）。Summary の秒は上の表。
- doc test: 14 crate、3 passed・0 failed・2 ignored（A と同じ）。
- web: typecheck（tsc -b）と unit。vitest 101 file・705 passed、node --test 110 passed・0 failed（A と同じ）。`web install` の段は 2 回とも無い。
- ts-verify: `test-ts-verify: 20 passed, 0 failed`、script bundle の SKIP 0 件（A と同じ）。
- 測った時刻は 2026-11-19T06:38:14Z〜07:09:07Z（UTC）。host home-dev、24 core、cargo 1.98.1、cargo-nextest 0.9.146。
  `CARGO_TARGET_DIR`・`CARGO_INCREMENTAL`・`CARGO_PROFILE_DEV_DEBUG`・rustc-wrapper は Celeris と host が渡した値のまま。
- 2 回の `src_tree`（`git rev-parse HEAD:crates`）は scenarios.env の `AFTER_CRATES_TREE` と一致した（値は TSV の `src_tree` の列）。

## 2. 計る前の確認

- `--list`（comment の probe を入れた状態）: 14 crate（celeris・celeris-campaign・celeris-credentiald・celeris-mcp・celeris-script・celerisctl・
  e2e・llm-proxy・routing-eval・task-api・task-core・task-dispatch・task-ops・task-worker）＋ `web: typecheck+unit` ＋ `ts-verify: test-ts-verify`。
  [scope.md](scope.md) §2 の after の対象（scope の run の `raw/ta-list.txt` の S1）と同じだった。
- 各回の probe は `git diff --stat -- crates` で 4 file・4 行の差分（4 insertions・4 deletions）だった。

## 3. A と条件が違う点

- S1 の file は 4 個（`org_write.rs` を足した。理由は [scope.md](scope.md) §1）。A は 3 個。`--list` の対象は A と同じ。
- worktree に `web/node_modules`・`control-ts/node_modules` が無かったので、計る前に各 dir で `pnpm install --offline --frozen-lockfile` を走らせた
  （計らない。rules.md「## TA」の 1）。A の S1 run 1 は test-affected の中の `web install` の段を含み、A の run 2・3 は含まない（A の log で確かめた）。
- 開始時の 5 分 load は 7.43・9.30 で、直前の build-test の測定と自分の run 1 の余波で高かった（1 分 load は 4 以下）。A の S1 run 1 の
  5 分 load は 8.64 で、同じ程度だった。A の run 2・3 の 5 分 load は記録が無い。
- user の +7.1 %（run 1）の原因は未確認。nextest の試験が 45 増えたこと（B〜F が足した試験）が一部を占めるかは、この 2 回からは分けられない。

## 4. probe の戻しと再現

- 各回の後に `git checkout -- $S1_FILES` で戻し、`git status --porcelain -- crates` が空（0 行）であることを driver が log に書いた。測り終えた後の
  `git status --porcelain` も空だった。
- 再現（repo の root で。`<A>` は成果物 dir）:
  `. agent-docs/progress/2026-11-19-arch-v4-g/scenarios.env; RAW=<A>/raw nohup bash <A>/raw/ta-driver.sh S1 1 2 > <A>/raw/driver-S1.out 2>&1 < /dev/null &`
  → `raw/driver-S1.done` ができるまで foreground の sleep で待つ → `python3 -I <A>/raw/ta-summary.py <A>/raw/ta-S1-<回>.log <A>/raw/ta-S1-<回>.time`。
- 生の記録（この run の成果物 dir の `raw/`）: `ta-S1-{1,2}.log`・`ta-S1-{1,2}.time`・`ta-S1-{1,2}.probe.diff`・`ta-S1.runs`・`ta-list-S1.txt`・
  `timing-raw.log`・`prep-{web,control-ts}.log`・`ta-driver.sh`・`ta-summary.py`・`probe-comment.sh`。
