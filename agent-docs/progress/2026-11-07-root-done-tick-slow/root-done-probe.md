---
title: root_done_tick の段別計測と本番 DB の写しでの測定（WU root-done-probe）
tasks: [01M6VQH1GY87Q79J406DFNQP6G]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# root_done_tick の段別計測と本番 DB の写しでの測定（WU root-done-probe）
完了日: 2026-11-07

## 結論
- 本番 DB の写しで、本番と同じ遅さ（cos.root_done_tick が 1 tick 9.9〜18.6 秒）を再現した。条件は「root_done_summaries に new の行があり、その root の delivery が待ちの状態（Reviewing・MergeQueued・Merging・Preparing）にある」こと。
- 時間のほぼ全部（99.6〜99.9%）は `root_done_facts` の中の `progress_markdown` だった。delivery の head の tree で `agent-docs/progress` の `.md` を名前順に 1 file ずつ `git show` するので、1 行・1 tick あたり git の process を 1,549 回起こす。new の行は delivery の待ちの間（最大 `DELIVERY_WAIT_SECS` = 3600 秒）毎 tick この処理を繰り返す。
- store の待ち（書き込み接続の Mutex・読み取りプール）は全段の合計で 1 tick 10〜28 µs で、写しでは lock 待ちは原因ではなかった。store の処理で最も大きいのは `events_for`（388〜528 件で 12〜62 ms）だった。
- 費用は「待ちの状態の new 行の数 × 進捗ファイルの数 × git 1 回の時間」に比例する。git 1 回の時間は host の負荷で 4.0〜11.4 ms と揺れた。本番の release 準備（build・試験）は root の完了の直後に delivery の待ちの中で走るので、その間は git 1 回がさらに遅くなりうる（本番では未確認）。
- 写しそのまま（new・queued の行が 0 件）では root_done_tick は 0.18〜1.1 ms で、前の task 01M6QZ6KYW の「写しでは 1 ms 未満」と一致する。backup を取った 18:23Z の時点では待ちの行が無く、本番では起動後に root が done になって待ちの行ができたと考えられる。本番の 18:30〜19:25Z に待ちの行があったことは DB では確かめていない（未確認）。下の「本番への移行」に、promote 後に見る値を書いた。
- 修正はこの WU の範囲外とした（ADR D6。WU root-done-fix が行う）。直し方の案は「提案」に書いた。

## 変更（commit は件名。基準は `$CELERIS_WU_BASE`）
- `root-done-probe: root_done_tick の段別計測の ADR` — `agent-docs/adr/2026-11-07-root-done-tick-phase-timing.md`。実装より前の commit。
- `root-done-probe: root_done_tick の段別計測を slow tick の内訳に出す（cos.root_done.*、store の待ちと処理を分ける）`
  - task-core: `store_lock_wait()`（`crates/task-core/src/store/lock_wait.rs`）。thread ごとに、書き込み接続の `Mutex` を取るまでの時間と読み取りプールから接続を借りるまでの時間を累積する。`SqliteStore::lock()` と `with_read_conn()` が足す。試験の観測用に `SqliteStore::writer_waiters()`（Mutex を待っている thread の数）を足した。
  - task-dispatch: `RootDoneProbe`（`crates/task-dispatch/src/dispatcher/cos_chat/root_done_probe.rs`）。`root_done_tick` が 100 ms（`ROOT_DONE_PROBE_THRESHOLD`）以上かかった tick だけ、`cos.root_done.*` の項目を tick_cos_chat_launch の内訳の末尾に足す。`TickReport::step_us` に載るので、slow tick の警告 "slow tick phases" の `steps_us` に出る。事実づくりが閾値を超えた行は、別に "slow root-done row" の警告を 1 行出す（task・state・段ごとの µs・events_for の件数・git の起動回数）。閾値より速い tick は今までと同じ列で、加わる費用は store の呼び出しごとの `Instant::now()` 2 回と固定の大きさの struct への加算だけ。
- この進捗ファイルの commit。

### 出る項目（時間は µs。`_rows`・`_calls`・`_max` で終わる項目は件数）
- 取り込み: `cos.root_done.ingest_us`、`cursor_get.{wait,work}_us`、`events_since_calls`、`events_since_rows`、`events_since.{wait,work}_us`、`candidate_rows`、`candidate_get.{wait,work}_us`、`candidate_events_for.{wait,work}_us`、`candidate_events_for_max`、`insert_calls`、`insert.{wait,work}_us`、`cursor_set.{wait,work}_us`
- 前進: `cos.root_done.advance_us`、`open_rows`、`open.{wait,work}_us`、`facts_calls`、`facts.get.{wait,work}_us`、`facts.events_for.{wait,work}_us`、`facts.events_for_max`、`facts.delivery_get.{wait,work}_us`、`facts.children.{wait,work}_us`、`facts.repo_get.{wait,work}_us`、`facts.progress_us`、`facts.progress_git_calls`、`facts.row_max_us`、`advance_other_us`（前進のうち open と事実づくり以外。chat への投稿・状態の更新など）
- 待ち（`wait_us`）は接続を取るまでの時間、処理（`work_us`）は呼び出しの経過時間から待ちを引いた残り。SQLite の file lock を他の接続・process が持つときの `busy_timeout` の待ちは SQL の実行の内側にあるので、処理の側に入る（ADR D2 の限界）。

### 試験
- `root_done_probe_parts_reach_the_tick_steps_only_over_the_threshold`（task-dispatch、`dispatcher::tests::cos_chat_triage::root_done`）: 閾値を最大にした pass では `cos.root_done.*` が出ないこと、閾値 0 の pass では取り込み・前進の全段と件数（events_since_calls 1、candidate_rows 1、insert_calls 1、open_rows 1、facts_calls 1、progress_git_calls 0）が出ること、通常の `Dispatcher::tick()` の `TickReport::step_us`（"slow tick phases" が出す列）にも載ることを確かめる。閾値は欄の差し替えで、時計・sleep・負荷は使わない。
- `writer_mutex_wait_is_counted_for_the_waiting_thread_only`（task-core、`store::lock_wait`）: 別の thread が書き込み接続を持っている間の書き込みで、待ちが 20 ms 以上になり、待たなかった thread の累積は増えないことを確かめる。`writer_waiters()` が 1 になるのを見てから lock を放すので、負荷で順序が崩れても結果は変わらない。
- `store_span_counts_calls_and_splits_wait_from_work`・`parts_are_prefixed_and_unique`（task-dispatch、`root_done_probe`）。

## 本番 DB の写しでの測定
手順: backup `/local/celeris/state/backups/20261107-182359-pre-66d9c1dae567.sqlite3` を `$TMPDIR` に複製した（元の file・本番 DB・state dir には書いていない）。その複製から 3 つの版を作り、`crates/celeris/tests/tick_prod_copy_steps.rs`（`TICK_PROD_DB=<版> cargo nextest run -p celeris --test tick_prod_copy_steps --run-ignored all --no-capture`）で測った。試験はさらに一時 dir へ複製して開く。cluster・browser・通知・remote・LLM・外部接続は config に無い。CPU の負荷はかけていない。
- copy: 写しそのまま。root_done_summaries は 583 行（notified 468・fallback 115）、new・queued は 0 行。cursor は events の末尾（838,369）。
- one: copy の上で、agent-platform の root 1 件（task 01M6S99ADJGBSD0CAJQDKSSMQ5、done_seq 368）の行を new に戻し（created_at を現在時刻）、その delivery の state を reviewing にした。本番の「delivery の待ちの間に new の行が残る」状態を作るためで、Preparing ではなく Reviewing にしたのは、tick が release の準備を起こす経路に触れないようにするためである。
- three: 同じ操作を 3 件（01M6S99ADJGBSD0CAJQDKSSMQ5、01M6RGSDEKJECE8QNFS1JC5P0B、01M6S325NYYJS9YWNHDH757E08）に行った。

delivery の head の tree の進捗ファイルは 1,564〜1,584 件で、この 3 件の進捗ファイルは名前順の後ろにある（1 件目は 1,549 回目の `git show` で見つかる）。進捗ファイルを読む repo は DB の project_repos のとおり `/home/rmaeda/workspace/agent-platform`（git の読み取りのみ）。

### 段別の値（ms。tick 0 は起動直後の照合を含む）
| 版 | tick | tick 全体 | tick_cos_chat_launch | cos.root_done_tick | open_rows | facts.progress | progress_git_calls | facts.events_for（件数の最大） | store の待ちの合計 |
|---|---|---|---|---|---|---|---|---|---|
| copy | 0 | 13,268 | 2,315 | 1.1 | （出ない） | — | — | — | — |
| copy | 1〜3 | 1,272〜1,550 | 8〜13 | 0.18〜0.36 | （出ない） | — | — | — | — |
| one | 0 | 26,158 | 20,451 | 17,719 | 1 | 17,693 | 1,549 | 23.1（388） | 0.012 |
| one | 1 | 17,561 | 15,802 | 15,789 | 1 | 15,758 | 1,549 | 27.6（388） | 0.010 |
| one | 2 | 15,894 | 13,767 | 13,754 | 1 | 13,738 | 1,549 | 14.1（388） | 0.011 |
| one | 3 | 11,352 | 9,902 | 9,891 | 1 | 9,877 | 1,549 | 11.8（388） | 0.014 |
| three | 0 | 23,612 | 20,101 | 18,498 | 3 | 18,431 | 4,628 | 61.9（528） | 0.026 |
| three | 1 | 19,674 | 18,641 | 18,632 | 3 | 18,600 | 4,628 | 28.4（528） | 0.028 |

- copy では閾値（100 ms）を超えないので `cos.root_done.*` は出ない（通常の tick に項目を足さないことの確認）。
- one と three の取り込み（`ingest_us`）は 0.12〜0.54 ms、`root_done_open` は 0.08〜0.33 ms、`advance_other` は 0.65〜1.0 ms、`get`・`delivery_get`・`children`・`repo_get` はそれぞれ 1 ms 未満だった。
- three の行ごとの最大（`facts.row_max_us`）は 6.7〜6.8 秒。3 行分で 18.5 秒になり、行数に比例している。
- git 1 回あたりは one で 6.4〜11.4 ms、three で 4.0 ms だった。同じ回数でも時間が揺れるのは host の負荷の差と見る（原因の process は未確認）。
- 生データ: run の artifacts の `m-copy-1.jsonl`・`m-one-1.jsonl`・`m-three-1.jsonl`（repo には入れない）。写しは run の `$TMPDIR` にあり、run の終わりに消える。

### 事前の見積もり（ADR の背景）
この repo の HEAD の 1,610 件を shell の loop で 1 回ずつ `git show` すると 7.7 秒かかった。dispatcher の中の値（1,549 回で 9.9〜17.7 秒）と同じ大きさである。

## 原因候補の絞り込み
- 再現した段: `cos.root_done.facts.progress_us`（`progress_markdown`）。写しの上で待ちの状態の new 行を 1 件作ると、cos.root_done_tick は毎 tick 10〜18 秒になり、本番の平均 4.7 秒・最大 27.5 秒と同じ大きさになった。
- 外れた候補: store の lock 待ち（写しでは 1 tick の合計 10〜28 µs）、`events_since` の頁読み（1 ms 未満）、`root_done_open` の行数（1〜3 行で 1 ms 未満）、`events_for` の件数（388〜528 件で 62 ms 以下）。本番の lock 待ちは写しでは測れないので、本番の `*.wait_us` で確かめる。
- 本番の値との対応: 写しの行の created_at から updated_at までに約 1 時間かかったもの（10:38→11:40Z、06:34→07:35Z、04:53→05:55Z、23:36→00:16Z）は、delivery の待ちで new のまま残った行である。その間は毎 tick この費用がかかっていたはずで、前日の release 8024a00d18c2 の起動直後に出た 12〜19 秒の tick とも矛盾しない（本番の log との突き合わせは未確認）。

## 未解決
- 本番の 18:30〜19:25Z に待ちの状態の new 行が何行あったかは確かめていない（本番 DB は読まない。この計測が入った release の slow tick の行で `cos.root_done.open_rows` と `facts.progress_git_calls` を見れば決まる）。
- SQLite の `busy_timeout` の待ち（他の接続・process の書き込みの file lock）は `work_us` の側に入り、`wait_us` とは分けられない。`progress_markdown` 以外の段の `work_us` が本番だけで大きければ、この待ちを疑う。
- dispatch_ready の段別は WU dispatch-ready-probe の範囲で、この WU では測っていない。
- 並行する WU dispatch-ready-probe も store の待ちの計測を task-core に足すと、`crates/task-core/src/store/mod.rs` の `lock()`・`with_read_conn()` で衝突しうる。統合のときに 1 つにまとめる。

## 提案（WU root-done-fix 向け。決めるのは ADR の付記か新しい ADR）
- 案 1: delivery の待ちの間は `progress_markdown` を呼ばない。待ちの判定に要るのは `delivery_state` だけなので、先に `delivery_get` だけを読み、待ちが続くなら事実を作らずに戻る。待ちが終わった tick に 1 回だけ全部の事実を作る。費用は待ちの間 0 回、行ごとに 1 回になる。
- 案 2: 進捗ファイルの探し方を git の起動 1〜2 回にする。`git grep -l -e "<task id>" <head> -- agent-docs/progress` で候補を絞るか、`git cat-file --batch` で 1 process から全 file を読む。file 数への比例は残るが、process の起動の費用が消える。
- 案 3: (task, head) ごとの結果を `CosChatLaunch` の process 内に覚える。daemon の再起動で消えても次の tick に 1 回作り直すだけで、状態の真実は DB のまま。
- 案 1 と案 2 を合わせると、待ちの間は 0 回、待ちの後も git 1〜2 回になる。試験は git の起動回数（`progress_git_calls`）と待ちの tick の事実づくり回数（`facts_calls`）で固定でき、時計を使わずに「進捗ファイルの数・tick 数に比例しない」ことを示せる。

## 本番への移行
- config の変更は無い。migration も無い。
- promote 後に operator が見る値（journal の "slow tick phases" の `steps_us`、slow tick の行だけに出る）:
  - `cos.root_done_tick` の中央値と最大。この計測だけの release では遅さは変わらない。
  - `cos.root_done.facts.progress_us` が `cos.root_done_tick` のほぼ全部なら、原因は写しと同じ（`progress_markdown`）。そのとき `cos.root_done.open_rows` と `cos.root_done.facts.progress_git_calls`（1 行あたり約 1,550）も見る。
  - `cos.root_done.*.wait_us` の合計が大きければ、本番の store の lock 待ちも効いている。
  - "slow root-done row" の警告の task・state で、どの root が待ちの状態に残っているかが分かる。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
（検査の後に追記する）
