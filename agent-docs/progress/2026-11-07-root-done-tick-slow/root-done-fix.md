---
title: root_done_tick の遅さの修正（WU root-done-fix）
tasks: [01M6VQH1GY87Q79J406DFNQP6G]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# root_done_tick の遅さの修正（WU root-done-fix）
完了日: 2026-11-07

## 結論: 直した
- 原因: delivery が待ちの状態（Reviewing・MergeQueued・Merging・Preparing）の間、root_done_summaries の new の行は毎 tick 事実（`collect_facts`）を作り直して捨てていた。事実づくりの中の `progress_markdown` が、進捗ファイル 1,549 件を 1 件ずつ `git show` していた（1 行・1 tick あたり git の process 1,549 回、5.2〜6.8 秒）。WU root-done-probe が本番 DB の写しで再現し、この WU で同じ写しを使って修正の前後を交互に測った。
- 修正（ADR [2026-11-07-root-done-tick-fix](../../adr/2026-11-07-root-done-tick-fix.md)）:
  1. new の行は先に `delivery_get` だけを読み、待ちの間は事実を作らずに戻る（待ちの条件と上限 3600 秒は変えない）。
  2. `progress_markdown` は `git grep` 1 回で `tasks:` の行にその task id を持つ file だけを候補にし、候補だけを `git show` する。返す file は今までと同じ（名前順で最初に front matter の `tasks:` にその id を持つ file）。
- 写しでの結果（中央値）: 待ちの行がある tick の cos.root_done_tick は 5,456 ms → 0.2 ms。待ちが終わって事実を 1 回作る tick は 5,504 ms → 312 ms（git の起動 1,549 回 → 2 回）。
- 本番の値は未確認。promote の後に operator が下の「本番への移行」の値を見る。config・migration の変更は無い。

## 変更（commit は件名。基準は `$CELERIS_WU_BASE`）
- `root-done-fix: ADR — delivery の待ちの間は事実を作らず、進捗ファイルは git grep で探す` — `agent-docs/adr/2026-11-07-root-done-tick-fix.md`。実装より前の commit。
- `root-done-fix: delivery の待ちの間は事実を作らず、進捗ファイルは git grep で候補を絞る`
  - `crates/task-dispatch/src/root_done_probe.rs`: `facts_unless_delivery_waits`（D1。delivery を 1 回読み、待ちなら `None`、待ちでなければ読んだ delivery で事実を作る）。`collect_facts` は delivery を読んでから `facts_with_delivery` を呼ぶ形に分けた（`delivery_get` は 1 行 1 回のまま）。`progress_markdown` は `git grep -l -z -E --no-color -e '^[[:space:]]*tasks:.*<id>' <head> -- agent-docs/progress` の候補だけを show する（D2・D3）。計測の項目に `cos.root_done.delivery_wait_rows`（待ちで戻った行の数）と `cos.root_done.facts.delivery_get_calls` を足した（D4）。
  - `crates/task-dispatch/src/dispatcher/cos_chat/root_done.rs`: new の行の分岐で `facts_unless_delivery_waits` を呼ぶ。待ちの判定をこの file から移したので、dispatcher の module の行数は 4 行減った（repo health の size.responsibility-growth を増やさない）。
- この進捗ファイルの commit。

## 回帰試験（時計は試験の時計だけ、sleep・CPU の負荷なし）
- `root_done_row_waiting_on_delivery_skips_facts_and_reads_progress_with_two_git_calls`（`crates/task-dispatch/src/dispatcher/tests/cos_chat_root_done.rs`）: 進捗ファイルを 3 件と 60 件持つ git repo を作り、Reviewing の delivery を持つ root の new 行で `tick_cos_chat_launch` を 3 回まわす。3 回とも `open_rows` 1・`delivery_wait_rows` 1・`facts.delivery_get_calls` 1・`facts_calls` 0・`facts.progress_git_calls` 0 で、行は new のまま。delivery を Ready にした次の pass は `facts_calls` 1・`progress_git_calls` 2 で、行は fallback に進み、通知の本文に「本番への移行: あり」（名前順で最後の、その task の進捗ファイルを読んだ）が出る。3 件と 60 件で回数が同じことで、進捗ファイルの数と待ちの tick 数に比例しないことを固定する。
- `progress_markdown_shows_only_files_whose_tasks_line_has_the_id`（`crates/task-dispatch/src/root_done_probe.rs`）: 別の task の file 40 件・本文だけで id に触れる file・front matter の外に `tasks: [<id>]` を書いた file・`.txt`・`tasks:` にその id を持つ file 2 件（並列 WU の配置の `2026-11-07-wu/a.md` と、後ろの file）を置く。名前順で最初の該当 file を返し、git の起動は 3 回（grep 1・front matter の外の file の show 1・該当 file の show 1）。該当が無い id は `None` で git 1 回、git が読めない head は `None` で git 1 回。
- 修正前の code では、待ちの pass の `facts_calls` は 1、`progress_git_calls` は 1 + 進捗ファイルの数になるので、上の試験は落ちる（読みで確認。修正前の code では走らせていない）。

## 本番 DB の写しでの before/after（交互に測った）
手順: backup `/local/celeris/state/backups/20261107-182359-pre-66d9c1dae567.sqlite3` を run の `$TMPDIR` に複製し（元の file・本番 DB・state dir には書いていない）、測るたびにそこから新しい写しを作った。写しの上で root 01M6S99ADJGBSD0CAJQDKSSMQ5（done_seq 368）の行を new（created_at は作った時刻）に戻し、その delivery の state を変えた（WU root-done-probe の「one」と同じ操作）。
- wait: delivery を reviewing にした（本番の「delivery の待ちの間に new の行が残る」状態）。3 tick を測る。
- ready: delivery を ready にした（待ちが終わり、事実を作る tick）。1 tick を測る。

試験 binary は `crates/celeris/tests/tick_prod_copy_steps.rs`（`cargo test -p celeris --test tick_prod_copy_steps --no-run` で作り、修正前・修正後の 2 つを `$TMPDIR` に写した）。修正前は `$CELERIS_WU_BASE`（この WU の変更なし）、修正後は commit「root-done-fix: delivery の待ちの間は事実を作らず、進捗ファイルは git grep で候補を絞る」の時点。修正前 → 修正後 → 修正後 → 修正前 の順に走らせた（wait、ready の順）。進捗ファイルを読む repo は DB の project_repos のとおり `/home/rmaeda/workspace/agent-platform`（git の読み取りのみ）で、delivery の head の tree の `.md` は 1,570 件。cluster・browser・通知・remote・LLM・外部接続は config に無い。CPU の負荷はかけていない。測った時刻の host の load average は 19→3 と下がっていった。

### wait（待ちの行が 1 行。ms。tick 0 は起動直後の照合を含む）
| 版（順番） | tick | tick 全体 | tick_cos_chat_launch | cos.root_done_tick | facts.progress | progress_git_calls |
|---|---|---|---|---|---|---|
| 修正前（1） | 0 | 11,976 | 8,455 | 6,830 | 6,814 | 1,549 |
| 修正前（1） | 1〜3 | 6,194〜6,509 | 5,184〜5,516 | 5,175〜5,507 | 5,166〜5,498 | 1,549 |
| 修正後（2） | 0 | 8,173 | 1,589 | 0.9 | （出ない） | （出ない） |
| 修正後（2） | 1〜3 | 1,016〜3,599 | 8.4〜9.2 | 0.2〜0.3 | （出ない） | （出ない） |
| 修正後（3） | 0 | 5,276 | 1,810 | 0.9 | （出ない） | （出ない） |
| 修正後（3） | 1〜3 | 1,024〜1,034 | 8.5〜9.3 | 0.2〜0.3 | （出ない） | （出ない） |
| 修正前（4） | 0 | 11,652 | 8,127 | 6,549 | 6,534 | 1,549 |
| 修正前（4） | 1〜3 | 6,437〜6,519 | 5,437〜5,501 | 5,428〜5,492 | 5,419〜5,483 | 1,549 |

- cos.root_done_tick の tick 1〜3（各版 6 tick）: 修正前 中央値 5,456 ms・最大 5,507 ms、修正後 中央値 0.2 ms・最大 0.3 ms。
- 修正後は cos.root_done_tick が閾値 100 ms を超えないので `cos.root_done.*` の内訳は出ない（ADR root-done-tick-phase-timing D3）。0.2〜0.9 ms は git の process 1 回（写しで 3.6 ms 以上）より短く、待ちの行で git を起こしていないことと合う。待ちの間に事実を作らないことの回数（`delivery_wait_rows` 1・`facts_calls` 0）は回帰試験で固定した。
- 修正後（2）の tick 2 の tick 全体 3,599 ms は tick_cos_chat_launch（8.4 ms）の外の段で、この修正とは関係しない（どの段かは見ていない）。
- 修正後の tick 全体 約 1.0 秒は、WU root-done-probe の「copy」（待ちの行なし）の 1,272〜1,550 ms と同じ大きさ。

### ready（待ちが終わった行が 1 行。ms）
| 版（順番） | tick | cos.root_done_tick | facts.progress | progress_git_calls | facts_calls |
|---|---|---|---|---|---|
| 修正前（1） | 0 | 6,679 | 6,651 | 1,549 | 1 |
| 修正前（1） | 1 | 5,519 | 5,506 | 1,549 | 1 |
| 修正後（2） | 0 | 1,181 | 1,143 | 2 | 1 |
| 修正後（2） | 1 | 310 | 297 | 2 | 1 |
| 修正後（3） | 0 | 340 | 305 | 2 | 1 |
| 修正後（3） | 1 | 313 | 300 | 2 | 1 |
| 修正前（4） | 0 | 5,456 | 5,403 | 1,549 | 1 |
| 修正前（4） | 1 | 5,491 | 5,478 | 1,549 | 1 |

- cos.root_done_tick（各版 4 tick）: 修正前 中央値 5,504 ms・最大 6,679 ms、修正後 中央値 312 ms・最大 1,181 ms（最初の run の tick 0。git の object を初めて読む tick）。
- 修正後の 300 ms のほぼ全部は `git grep` 1 回（1,570 file の blob を読む）。本番ではこの費用は 1 つの root につき、待ちが終わった tick と、CoS の run が終わった後の通知の tick の 2 回だけかかる。
- この計測の config には `web_base_url` が無いので、通知の短文が作れず、行は open のまま tick 1 でも事実を作り直した（tick 1 も `facts_calls` 1）。tracing の出力を取っていないので、tick 1 の行が new と queued のどちらだったかは未確認。本番は `web_base_url` が設定されていて、通知が作れた時点で行は閉じる。
- 生データ: run の artifacts の `m-wait-{1..4}-{before,after}.jsonl`・`m-ready-{1..4}-{before,after}.jsonl`（repo には入れない）。写しは run の `$TMPDIR` にあり、run の終わりに消える。

## 本番への移行
- config の変更は無い。migration も無い。
- 人が実行する手順（この WU は実行していない）:
  1. この修正が main に入った後、`scripts/selfdeploy/release.sh <git-ref>` で release を作る。
  2. `SD_ACTOR=human SD_REASON=<理由> SD_POLICY_REF=<参照> scripts/selfdeploy/promote.sh <sha12>` で promote する。
  3. promote の 1 時間後に、`journalctl --user -u celeris@<sha12> --since '<promote の時刻>' | grep 'slow tick phases'` で slow tick の行を集める。比べる対象は release 66d9c1dae567 の 18:30〜19:25Z の 234 件（tick_cos_chat_launch が最長の段 154 件、cos.root_done_tick の平均 4,743 ms、最大 27,537 ms）。
- operator が見る値（`steps_us` の中）:
  - `cos.root_done_tick` の中央値と最大。slow tick（1 秒以上）の行にだけ出るので、slow tick の件数のうち tick_cos_chat_launch が最長の段の件数も数える。
  - 期待: 待ちの行があるだけの tick では cos.root_done_tick は 1 ms 前後で、slow tick の行に `cos.root_done.*` の内訳は出ない。1 つの root の完了ごとに 2 回だけ、cos.root_done_tick が約 0.3〜1.2 秒（写しの値）の tick があり、そのとき `cos.root_done.facts.progress_git_calls` は 2 前後（1 + 候補の数）。
- 判定の分かれ目:
  - cos.root_done_tick が 100 ms 以上の slow tick がほぼ無い → 直った。
  - 同じ task の "slow root-done row" の警告が続けて出て、`cos.root_done.facts_calls` が毎 tick 1 以上 → 行が閉じずに毎 tick 事実を作り直している。journal で "root-done summary step failed (retrying)"（key ごとに 1 回だけ出る）を探し、失敗の理由を見る。写しの計測では `web_base_url` が無いと通知が作れず、この形になった。
  - `cos.root_done.facts.progress_git_calls` が 2 を大きく超える → その task の id を `tasks:` の行に書いた進捗ファイルが多い（並列 WU の配置など）。件数を見て、候補を減らすかを決める。
  - `cos.root_done.*.wait_us` の合計が大きい → store の待ち（lock・disk・run queue）。同じ時刻の host の負荷と合わせて読む。
  - `cos.root_done_tick` が小さいのに tick_cos_chat_launch が大きい → root_done の外の段（`cos.*` の他の項目）を見る。

## 未解決
- 本番の 18:30〜19:25Z に待ちの状態の new 行が何行あったかは確かめていない（本番 DB は読まない）。修正が本番の遅さを消したかは、上の「本番への移行」の値で決まる。
- 行が閉じずに失敗を繰り返す場合（例: `web_base_url` が無い）、事実づくりは毎 tick 続く。修正で 1 回が 5.5 秒から約 0.3 秒になったが、回数は減っていない。本番でこの形が出るかは未確認。
- dispatch_ready の 1.9 秒（最大 6 秒）は WU dispatch-ready-probe の範囲で、この WU では直していない（主な段は `dispatch_ready.workspaces.worktree_base`。同 WU の進捗）。

## 提案
- 失敗を繰り返す root_done の行に、回数か時間の上限を置く（上限を過ぎたら fallback を諦めて行を閉じ、受信箱に知らせる）。本番で上の「判定の分かれ目」の 2 つ目が出たときに、ADR の付記か新しい ADR で決める。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
commit「root-done-fix: 修正の前後の計測と本番への移行を進捗に書く」の時点の HEAD で実行した（取り込み・rebase はしていない）。その後の commit はこの進捗ファイルの証拠の節だけを変える。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` → ADR・この進捗ファイル・`crates/task-dispatch/src/root_done_probe.rs`・`crates/task-dispatch/src/dispatcher/cos_chat/root_done.rs`・`crates/task-dispatch/src/dispatcher/tests/cos_chat_root_done.rs` の 5 path だけ。
- `bash scripts/dev/test-affected.sh` → exit 0（対象は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。nextest 3,505 件 passed・11 件 skipped（452 秒）。repo-health は verdict=pass・new 0・worsened 0。script bundle の SKIP 0 件）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/progress-index.sh --check` → ok
- `cargo nextest run -p task-dispatch -E 'test(root_done) | test(store_span) | test(progress_markdown)'` → 13 件 passed（新しい 2 件を含む）
- 写しの計測: `sh $TMPDIR/measure.sh wait reviewing 3 before after after before` と `sh $TMPDIR/measure.sh ready ready 1 before after after before`（各 run は `TICK_PROD_DB=$TMPDIR/prod/<variant>.sqlite3 TICK_PROD_TICKS=<n> <試験 binary> --ignored --nocapture --exact prod_copy_tick_steps`）→ 8 run とも exit 0。値は上の表。
