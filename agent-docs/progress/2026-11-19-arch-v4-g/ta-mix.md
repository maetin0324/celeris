---
title: アーキ v4 G ta-mix — after で test-affected の S1 の 3 回目と S2 の 1 回目を測る
tasks: [01M7MZTKBKXKEVBWS87PS5J9NK]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# ta-mix（WorkUnit）

after は `git merge-base HEAD main`（この WorkUnit の `$CELERIS_WU_BASE` と同じ crates/ の tree）。before は A の baseline.md §5.5 の値。
command は `/usr/bin/time -v bash scripts/dev/test-affected.sh`（A §7 C9）、probe は comment の probe。手順は [rules.md](rules.md) の「## TA」、
TSV の列と valid の規則は同じ file の「## TSV」、file は [scenarios.env](scenarios.env)。各回の行は [ta-mix.tsv](ta-mix.tsv)。
S1 の 1・2 回目は [ta-s1.md](ta-s1.md)・[ta-s1.tsv](ta-s1.tsv)。S2 の 2・3 回目は ta-s2 の leaf が測る。

## 0. 結論

- S1 の 3 回目と S2 の 1 回目は、どちらも exit 0、開始時の 1 分 load が 4 以下で valid だった（測り直しは 0 回）。
- S1 の valid な 3 回（911.49・901.20・906.03 s）がそろった。wall の中央値は 906.03 s で、A の中央値 893.41 s に対して +12.62 s（+1.4 %）。
  A の +10 %（982.75 s）を超えない。user の中央値は 4,492.85 s で、A の 4,354 s に対して +3.2 %。
- S2 の 1 回目は 722.11 s で、A の中央値 655.59 s に対して +10.1 %（A の +10 % の 721.15 s を 0.96 s 超える）、A の run 1（695.36 s）に対して +3.8 %。
  差の大半は test build の段で、直前の状態が違うことによる（§3）。S2 の判定は ta-s2 の 2・3 回目とそろえた中央値で compare の leaf が行う。

| | run 1 | run 2 | run 3 | 中央値 | A の run 1・2・3 | A の中央値 | 差 |
|---|---|---|---|---|---|---|---|
| S1 の wall | 911.49 s | 901.20 s | **906.03 s** | **906.03 s** | 893.41・878.66・900.34 s | 893.41 s | +12.62 s（+1.4 %） |
| S1 の user | 4,662.13 s | 4,416.82 s | 4,492.85 s | 4,492.85 s | 4,550・4,354・4,352 s | 4,354 s | +3.2 % |
| S1 の nextest の Summary の秒 | 497.035 s | 513.087 s | 505.865 s | 505.865 s | 483.7・503.2・511.2 s | 503.2 s | +0.5 % |
| S1 の wall − nextest | 414.5 s | 388.1 s | 400.2 s | 400.2 s | 409.7・375.5・389.1 s | 389.1 s | +2.8 % |
| S2 の wall | **722.11 s** | （ta-s2） | （ta-s2） | — | 695.36・655.59・647.32 s | 655.59 s | run 1 だけで +10.1 % |
| S2 の user | 3,566.71 s | | | — | 3,705・3,104・2,961 s | 3,104 s | run 1 だけで +14.9 % |
| S2 の nextest の Summary の秒 | 422.878 s | | | — | 406.6・418.0・406.1 s | 418.0 s | run 1 だけで +1.2 % |

S1 の run 1・2 は ta-s1 の値、太字がこの leaf で測った値。

## 1. 各回の値と内訳

| 回 | wall | user | sys | exit | 開始時の load（1・5・15 分） | 待った秒 | 最大 RSS | 測った時刻（UTC） |
|---|---|---|---|---|---|---|---|---|
| S1 run 3 | 906.03 s | 4,492.85 s | 1,025.50 s | 0 | 0.77 / 4.64 / 8.32 | 0 | 3,629,052 kB | 07:14:57Z〜07:30:03Z |
| S2 run 1 | 722.11 s | 3,566.71 s | 798.99 s | 0 | 3.71 / 8.92 / 9.05 | 60 | 3,612,248 kB | 07:31:03Z〜07:43:05Z |

内訳（`raw/ta-summary.py` で log から取った。build の秒と Compiling の行の数は log の `Finished \`test\`` の行と `Compiling` の行から取った）:

- S1 run 3:
  - health: `repo-health: verdict=pass exit=0 findings=695 new=0 worsened=0`、`repo_health_gate: all checks passed`（ta-s1 と同じ。A は findings=697）。
  - test build: `Finished` 2m 09s、Compiling 24 行（ta-s1 の run 1・2 は 2m 09s・2m 12s、A は 2m 14s・2m 10s・2m 16s）。
  - nextest: `Starting 8481 tests across 312 binaries (22 tests and 2 binaries skipped)`、8,481 passed・0 failed・22 skipped・9 slow、505.865 s。
  - doc test: 14 crate、3 passed・0 failed・2 ignored。
  - web: vitest 101 file・705 passed、node --test 110 passed・0 failed。`web install` の段は無い。
  - ts-verify: `test-ts-verify: 20 passed, 0 failed`、script bundle の SKIP 0 件。
- S2 run 1:
  - test-affected の対象: `test-affected: crates: celeris celeris-campaign celerisctl llm-proxy task-dispatch`（A と同じ 5 crate）。web と ts-verify は対象外。
  - health: `repo-health: verdict=pass exit=0 findings=695 new=0 worsened=0`、`repo_health_gate: all checks passed`（A は findings=697）。
  - test build: `Finished` 2m 16s、Compiling 14 行（task-core と逆依存の crate を作り直した）。
  - nextest: `Starting 3938 tests across 181 binaries (11 tests and 133 binaries skipped)`、3,938 passed・0 failed・11 skipped・9 slow、422.878 s。
    A は 3,917 passed（+21。B〜F が足した試験）。
  - doc test: 14 crate、3 passed・0 failed・2 ignored（A の S2 も 14 crate の `test result:` の行がある）。
- 2 回の `src_tree`（`git rev-parse HEAD:crates`）は scenarios.env の `AFTER_CRATES_TREE` と一致した（値は TSV の `src_tree` の列）。host home-dev、24 core。
  `CARGO_TARGET_DIR`・`CARGO_INCREMENTAL`・`CARGO_PROFILE_DEV_DEBUG`・rustc-wrapper は Celeris と host が渡した値のまま。

## 2. 計る前の確認

- S2 の `--list`（comment の probe を入れた状態）を、S1 の 3 回目を測る前に 1 回、driver の中で S2 の 1 回目の直前にもう 1 回走らせた。
  2 回とも celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch の 5 crate で、[scope.md](scope.md) §2 の after の対象
  （scope の run の `raw/ta-list.txt` の S2 の節）と `diff` で一致した。
- S1 の `--list` も driver の中で走らせ、scope の `ta-list.txt` の S1 の節（14 crate ＋ `web: typecheck+unit` ＋ `ts-verify: test-ts-verify`）と一致した。
- driver の写しには「`--list` が scope の出力と違えば測らずに止まる」確認を足した（rules.md「## TA」の 2 を driver で強制するため）。他は ta-s1 の driver と同じ。
- 各回の probe は `git diff --stat -- crates` で 4 file・4 行の差分（4 insertions・4 deletions）だった。
- `web/node_modules`・`control-ts/node_modules` は driver が計る前に `pnpm install --offline --frozen-lockfile` で入れた（計らない）。

## 3. S2 の 1 回目の差の内訳と、比べるときの注意

- S2 の 1 回目は S1 の 3 回目の直後に測った。S1 の probe（task-core の `store/org.rs` を含む）を戻した後なので、S2 の test build は task-core と
  その逆依存を作り直し、`Finished` が 2m 16s（Compiling 14 行）だった。
- A の S2 は、run 1 が同じく 2m 12s（Compiling 273 行）、S2 の後に続けて測った run 2・3（attempt 2）が 1m 16s・1m 18s（Compiling 5 行）だった。
- wall から build と nextest を引いた残り（health・doc test・bundle の作成など）は、この回 163.2 s、A の run 1・2・3 で 156.7・161.6・163.3 s で、ほぼ同じ。
- このため、この回の A の中央値（run 2）に対する +66.5 s のうち約 60 s は build の段の差（136 s 対 76 s）で、直前の状態による。nextest の差は +4.9 s（+1.2 %）。
  A の run 1（直前の状態が似ている回）と比べると +26.8 s（+3.8 %）。
- user の +14.9 %（A の中央値比）も、作り直す crate の数の差を含む。A の run 1（3,705 s）と比べると −3.7 %。
- ta-s2 の 2・3 回目は、S2 の probe を入れて戻した状態から続けて測れば A の run 2・3 と同じ条件になる。ta-s2 の worktree の target dir の状態は未確認。

## 4. probe の戻しと再現

- 各回の後に driver が `git checkout -- <file>` で戻し、`git status --porcelain -- crates` が 0 行であることを `raw/timing-raw.log` に書いた
  （2 回とも `porcelain=0`）。測り終えた後の `git status --porcelain` も 0 行だった（md・TSV を書く前）。
- 再現（repo の root で。`<A>` は成果物 dir）:
  `RAW=<A>/raw nohup bash <A>/raw/ta-chain.sh > <A>/raw/chain-nohup.out 2>&1 < /dev/null &`
  （`ta-chain.sh` は `ta-driver.sh S1 3` → `ta-driver.sh S2 1` を続けて走らせ、`raw/chain.done` を作る）→ foreground の sleep で `chain.done` を待つ →
  `python3 -I <A>/raw/ta-summary.py <A>/raw/ta-<S>-<回>.log <A>/raw/ta-<S>-<回>.time`。
  `--list` の確認は `raw/expect-list-{S1,S2}.txt`（scope の `ta-list.txt` の各節）と `raw/ta-list-<S>.txt` の `diff`。
- 生の記録（この run の成果物 dir の `raw/`）: `ta-S1-3.log`・`ta-S1-3.time`・`ta-S1-3.probe.diff`・`ta-S2-1.log`・`ta-S2-1.time`・`ta-S2-1.probe.diff`・
  `ta-S1.runs`・`ta-S2.runs`・`ta-list-S1.txt`・`ta-list-S2.txt`・`ta-list-S2-precheck.txt`・`ta-list-{S1,S2}.diff`・`expect-list-{S1,S2}.txt`・
  `timing-raw.log`・`chain.out`・`ta-driver.sh`・`ta-chain.sh`・`ta-summary.py`・`probe-comment.sh`。
