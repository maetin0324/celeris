---
title: アーキ v4 G ta-s2 — after で test-affected の S2 の 2・3 回目を測り、S1・S2 の 3 回の中央値を A・E と並べる
tasks: [01M7MZTKBKXKEVBWS87PS5J9NK]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# ta-s2（WorkUnit）

after は `git merge-base HEAD main`（この WorkUnit の `$CELERIS_WU_BASE` と同じ crates/ の tree）。before は A の baseline.md §5.5 の値。
command は `/usr/bin/time -v bash scripts/dev/test-affected.sh`（A §7 C9）、probe は comment の probe。手順は [rules.md](rules.md) の「## TA」、
TSV の列と valid の規則は同じ file の「## TSV」、file は [scenarios.env](scenarios.env)。各回の行は [ta-s2.tsv](ta-s2.tsv)。
S1 の 1・2 回目は [ta-s1.md](ta-s1.md)、S1 の 3 回目と S2 の 1 回目は [ta-mix.md](ta-mix.md)。

## 0. 結論

- S2 の 2 回目（707.76 s）と 3 回目（653.34 s）は、どちらも exit 0、開始時の 1 分 load が 4 以下（3.48・3.86）で valid だった。測り直しは 0 回。
- S2 の valid な 3 回は 722.11・707.76・653.34 s で、wall の中央値は 707.76 s。A の中央値 655.59 s に対して +52.17 s（+8.0 %）で、
  A の +10 %（721.15 s）を超えない（余裕 13.39 s）。user の中央値は 3,093.42 s で、A の 3,104 s に対して −0.3 %。
- E の 1 回（769.32 s）に対して、S2 の中央値は −61.56 s（−8.0 %）。
- S1 の中央値は 906.03 s で、A の 893.41 s に対して +1.4 %（ta-mix と同じ）。
- 試験の数（nextest の passed）は、S1 が 8,481（A 8,436、+45）、S2 が 3,938（A 3,917、+21。E は 3,936 で +19）。skipped は S1 22・S2 11 で A と同じ。
- 10 % を超えたかどうかの印は compare の leaf が付ける。S2 の差の内訳は §3。

## 1. S1・S2 の 3 回の中央値と A・E の比較

| | after run 1 | after run 2 | after run 3 | after の中央値 | A の run 1・2・3 | A の中央値 | 差（A の中央値比） | E の 1 回 |
|---|---|---|---|---|---|---|---|---|
| S1 の wall | 911.49 s | 901.20 s | 906.03 s | **906.03 s** | 893.41・878.66・900.34 s | 893.41 s | +12.62 s（+1.4 %） | — |
| S1 の user | 4,662.13 s | 4,416.82 s | 4,492.85 s | 4,492.85 s | 4,550・4,354・4,352 s | 4,354 s | +138.85 s（+3.2 %） | — |
| S1 の nextest の Summary の秒 | 497.035 s | 513.087 s | 505.865 s | 505.865 s | 483.7・503.2・511.2 s | 503.2 s | +2.67 s（+0.5 %） | — |
| S1 の nextest の passed | 8,481 | 8,481 | 8,481 | 8,481 | 8,436（3 回とも） | 8,436 | +45 | — |
| S2 の wall | 722.11 s | **707.76 s** | **653.34 s** | **707.76 s** | 695.36・655.59・647.32 s | 655.59 s | +52.17 s（+8.0 %） | 769.32 s |
| S2 の user | 3,566.71 s | 3,093.42 s | 2,990.88 s | 3,093.42 s | 3,705・3,104・2,961 s | 3,104 s | −10.58 s（−0.3 %） | 3,629.05 s |
| S2 の nextest の Summary の秒 | 422.878 s | 454.890 s | 407.091 s | 422.878 s | 406.636・417.994・406.069 s | 406.636 s | +16.24 s（+4.0 %） | 443.874 s |
| S2 の test build（`Finished`） | 2m 16s | 1m 22s | 1m 25s | 1m 25s | 2m 12s・1m 16s・1m 18s | 1m 18s | +7 s | 2m 19s |
| S2 の wall − build − nextest | 163.2 s | 170.9 s | 161.2 s | 163.2 s | 156.7・161.6・163.3 s | 161.6 s | +1.6 s（+1.0 %） | 186.4 s |
| S2 の nextest の passed | 3,938 | 3,938 | 3,938 | 3,938 | 3,917（3 回とも） | 3,917 | +21 | 3,936（+19） |

- S1 の run 1・2 は ta-s1、S1 の run 3 と S2 の run 1 は ta-mix、太字の S2 の run 2・3 はこの leaf で測った値。S1・S2 とも valid な回は 3 回ずつそろった。
- A の値は baseline.md §5.5 と A の log（`ta-S2-1.log`・`ta-S2-2r.log`・`ta-S2-3r.log`）から取った。A の S2 の nextest の 3 回の中央値は 406.636 s。
  ta-mix.md の表の「418.0 s」は、wall が中央値になった A の run 2 の nextest の値である。
- E の値は [E の metrics.md](../2026-11-18-arch-v4-e/metrics.md) の (5) の表（1 回、開始時の 1 分 load 3.91）。E の test build の後の残り（186.4 s）は
  769.32 − 139 − 443.874 で出した。
- 試験の数の差（+45・+21）は B〜F が足した試験で、E が書いた +19 に F までの分が加わった値。どの試験が増えたかの一覧は作っていない（未確認）。

## 2. 各回の値と内訳（この leaf の 2 回）

| 回 | wall | user | sys | exit | 開始時の load（1・5・15 分） | 待った秒 | 最大 RSS | 測った時刻（UTC） |
|---|---|---|---|---|---|---|---|---|
| S2 run 2 | 707.76 s | 3,093.42 s | 744.41 s | 0 | 3.48 / 7.32 / 8.25 | 110 | 3,558,748 kB | 07:53:20Z〜08:05:08Z |
| S2 run 3 | 653.34 s | 2,990.88 s | 728.69 s | 0 | 3.86 / 9.17 / 9.17 | 30 | 3,626,596 kB | 08:05:38Z〜08:16:31Z |

内訳（`raw/ta-summary.py` で log から取った。build の秒と Compiling の行の数は log の `Finished \`test\`` の行と `Compiling` の行から取った）:

- 2 回とも:
  - test-affected の対象: celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch の 5 crate（A と同じ）。web と ts-verify は対象外。
  - health: `repo-health: verdict=pass exit=0 findings=695 new=0 worsened=0`、`repo_health_gate: all checks passed`（A は findings=697）。
  - check-slot: `枠 0/2 を使う。--test-threads 12`（A の S2 の 3 回と ta-mix の S2 の 1 回目も同じ行）。
  - nextest: `Starting 3938 tests across 181 binaries (11 tests and 133 binaries skipped)`、3,938 passed・0 failed・11 skipped。
  - doc test: 14 crate、3 passed・0 failed・2 ignored。
  - test build: Compiling 5 行（task-dispatch と逆依存の 4 crate）。A の run 2・3 と同じ組。
- S2 run 2: test build 1m 22s、nextest 454.890 s（8 slow）。
- S2 run 3: test build 1m 25s、nextest 407.091 s（7 slow）。
- 2 回の `src_tree`（`git rev-parse HEAD:crates`）は scenarios.env の `AFTER_CRATES_TREE` と一致した（値は TSV の `src_tree` の列）。host home-dev、24 core。
  `CARGO_TARGET_DIR`・`CARGO_INCREMENTAL`・`CARGO_PROFILE_DEV_DEBUG`・rustc-wrapper は Celeris と host が渡した値のまま。

## 3. S2 の差の内訳と、比べるときの注意

- 起点の状態: A の run 2・3 は、S2 の probe を入れて戻した直後の状態から測った（Compiling 5 行）。この leaf の run 2・3 も同じ Compiling 5 行で、
  起点の状態は A の run 2・3 と同じ。run 1（ta-mix）は S1 の直後で Compiling 14 行、A の run 1 は Compiling 273 行で、どちらも作り直しが多い。
- 同じ起点どうしで比べると、run 2 は A の run 2 より +52.17 s（+8.0 %）、run 3 は A の run 3 より +6.02 s（+0.9 %）。
- run 2 の差の大半は nextest の段（454.890 s。A の 3 回の最大 417.994 s より +36.9 s）。run 3 の nextest（407.091 s）は A と同じ幅に入る。
  run 2 と A の run 2 で、30 s を超えた SLOW の試験の顔ぶれと秒（dagbench_harness の 4 件 107〜125 s、project_controller_soak_72h の 3 件 84〜89 s、
  ops_r1_pipeline）はほぼ同じで、どの試験が run 2 を延ばしたかは log からは絞れていない（未確認）。
- test build は A の run 2・3 より 6〜7 s 長い（82・85 s 対 76・78 s）。wall から build と nextest を引いた残り（health・doc test・bundle の作成など）は
  A とほぼ同じ（161.2〜170.9 s 対 156.7〜163.3 s）。
- user の中央値は A と同じ（−0.3 %）で、CPU の仕事の量は増えていない。wall の差は nextest の 1 回の延びと build の数秒による。
- E の 1 回（769.32 s）は、test build 2m 19s（S1 の後のような作り直しの多い起点）と nextest 443.874 s を含む。after の run 1（同じく作り直しの多い起点）の
  722.11 s より 47.21 s 長い。E は 1 回だけで、開始の状態と load の推移が違うため、差の原因は確かめていない。

## 4. 計る前の確認と準備

- この worktree の target dir は、ta-mix の WorkUnit の target dir を引き継いだもの（`target-origin.json` の `adopted_from`）。worktree の path が違うため、
  計る前に `cargo nextest run --workspace --no-run --offline` を 1 回走らせ（計らない。A §7 C8 の「戻した状態で計らない build」と同じ）、
  この worktree の path で build した。2m 15.69s、Compiling 19 行、exit 0（`raw/warmup.log`・`raw/warmup.time`）。
- `--list`（comment の probe を入れた状態）は driver の中で run 2 の前に走らせ、scope の `ta-list.txt` の S2 の節（`raw/expect-list-S2.txt`）と
  `diff` で一致した（`raw/ta-list-S2.diff` は 0 byte）。
- 各回の probe は `git diff --stat -- crates` で 4 file・4 行の差分（4 insertions・4 deletions）だった（`raw/ta-S2-{2,3}.probe.diff`）。
- `web/node_modules`・`control-ts/node_modules` は driver が計る前に `pnpm install --offline --frozen-lockfile` で入れた（計らない。S2 は web を使わない）。
- 測っている間、repo の中の file は変えず、未追跡の file も置かなかった（TSV と md は測り終えてから書いた）。

## 5. probe の戻しと再現

- 各回の後に driver が `git checkout -- <file>` で戻し、`git status --porcelain -- crates` が 0 行であることを `raw/timing-raw.log` に書いた
  （2 回とも `porcelain=0`）。測り終えた後の `git status --porcelain` も 0 行だった（md・TSV を書く前）。
- 再現（repo の root で。`<A>` は成果物 dir）:
  `RAW=<A>/raw nohup bash <A>/raw/ta-chain.sh > <A>/raw/chain-nohup.out 2>&1 < /dev/null &`
  （`ta-chain.sh` は計らない warm-up の test build → `ta-driver.sh S2 2 3` を続けて走らせ、`raw/chain.done` を作る）→ foreground の sleep で `chain.done` を待つ →
  `python3 -I <A>/raw/ta-summary.py <A>/raw/ta-S2-<回>.log <A>/raw/ta-S2-<回>.time`。
  中央値は ta-s1.tsv・ta-mix.tsv・ta-s2.tsv の `valid=yes` の行の `seconds`（`metric=test_affected`）を scenario ごとに並べて取る。
- 生の記録（この run の成果物 dir の `raw/`）: `ta-S2-2.log`・`ta-S2-2.time`・`ta-S2-2.probe.diff`・`ta-S2-3.log`・`ta-S2-3.time`・`ta-S2-3.probe.diff`・
  `ta-S2.runs`・`ta-list-S2.txt`・`ta-list-S2.diff`・`expect-list-S2.txt`・`warmup.log`・`warmup.time`・`timing-raw.log`・`chain.out`・
  `ta-driver.sh`・`ta-chain.sh`・`ta-summary.py`・`probe-comment.sh`。driver・summary・probe の script は ta-mix の run の写し（ta-chain.sh だけをこの leaf で書いた）。
