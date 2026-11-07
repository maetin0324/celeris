---
status: proposed
tasks: [01M6VQH1GY87Q79J406DFNQP6G]
---
# root_done_tick の段別計測（slow tick の log に cos.root_done.* を出す）

## 背景
本番（release 66d9c1dae567、2026-11-07 18:24Z 起動）の slow tick 234 件（18:30〜19:25Z）のうち 154 件で、最も長い段は tick_cos_chat_launch だった。その 1 件あたりの平均は 4,787 ms で、そのうち cos.root_done_tick が 4,743 ms を占める。最大は 19:18:12Z の 27,537 ms。前の task 01M6QZ6KYW は本番 DB の写しで root_done_tick を 1 ms 未満と測っており、写しでは再現しなかった（`agent-docs/progress/2026-11-06-cos-chat-launch-scaling.md`）。

`root_done_tick` は 2 つの部分からなる（`crates/task-dispatch/src/dispatcher/cos_chat/root_done.rs`）。
- 取り込み: cursor を読み、`events_since` で最大 8 頁 × 512 件の event を読む。done 候補ごとに `store.get`・`events_for`・`root_done_insert` を呼び、最後に cursor を書く。
- 前進: `root_done_open()` の行（state が new か queued）ごとに `root_done_advance` を呼ぶ。new の行は毎 tick `root_done_facts` を作る（`get`・`events_for`・`delivery_get`・`children`・`progress_markdown`）。delivery が Reviewing・MergeQueued・Merging・Preparing の間は、最大 `DELIVERY_WAIT_SECS`（3600 秒）まで new のまま待つ。

写しで再現しない理由の候補は 2 つある。
1. 写し（`20261107-182359-pre-66d9c1dae567.sqlite3`）では root_done_summaries の 583 行が全部 notified か fallback で、new・queued の行が 0 件である。前進の部分は写しでは 1 回も動かない。本番では 18:24Z の後に root が done になり、delivery の待ちの間に new の行が残った可能性がある。写しの行にも created_at から updated_at まで約 1 時間かかったものがある（例: 10:38→11:40Z、06:34→07:35Z）。
2. 本番だけにある同時アクセス（API・web・SSE の同じ store の書き込み接続 `Mutex` と読み取りプール）による待ち。

`progress_markdown` は delivery の head の tree で `agent-docs/progress` の `.md` を名前順に `git show` し、front matter の `tasks:` にその task を持つ最初の file を探す。1 file ごとに git の process を 1 つ起こす。この repo の HEAD では 1,610 file あり、当日の file は名前順で 1,500 番目より後ろに並ぶ。この host で全 file を 1 回ずつ `git show` すると 7.7 秒かかった（shell の loop、事前の計測）。これは候補 1 の費用の見積もりで、本番でこの段が遅いことはまだ確かめていない。

## 決定
- D1: `root_done_tick` の中に段別の計測（`RootDoneProbe`）を置く。測る段は次のとおり。
  - 取り込み: `feed_cursor_get`、`events_since`（呼び出し回数・読んだ行数・時間）、done 候補の数、候補ごとの `store.get`・`events_for`（時間と最大件数）・`root_done_insert`、`feed_cursor_set`。
  - 前進: `root_done_open`（時間と行数）、`root_done_facts`（呼び出し回数）と、その中の `get`・`events_for`（時間と最大件数）・`delivery_get`・`children`・`progress_markdown`（時間と git の起動回数）。前進のうち事実づくり以外（chat への投稿・状態の更新など）は `advance_other` にまとめる。
- D2: store の呼び出しは「待ち」と「処理」を分けて記録する。待ちは、その thread が書き込み接続の `Mutex` を取るまでの時間と、読み取りプールから接続を借りるまでの時間の合計である。task-core の store が thread ごとの累積値（`task_core::store_lock_wait()`）に足す。処理は呼び出しの経過時間から待ちを引いた残りである。SQLite の file lock を他の接続・process が持つときの `busy_timeout` の待ちは SQL の実行の内側にあるので、処理の側に入る。この限界は進捗にも書く。
- D3: 計測した値を log に出すのは、`root_done_tick` 全体が閾値（100 ms）を超えた tick だけとする。そのときだけ `cos.root_done.<段>.<量>` の項目を tick_cos_chat_launch の内訳（`TickReport::step_us`）に足す。slow tick の警告（`tick_steps.rs` の "slow tick phases"）はこの列を `steps_us` に出すので、本番の slow tick の行にそのまま載る。時間の項目は µs、行数・回数の項目は名前の末尾を `_rows`・`_calls`・`_max` にして区別する。事実づくりが閾値を超えた行は、別に "slow root-done row" の警告（task・state・各段の µs・events_for の件数・git の起動回数）を 1 行出す。
- D4: 閾値を下回る tick には項目を足さない。計測の費用は、固定の大きさの struct への加算と、store の呼び出しごとの `Instant::now()` の 2 回だけで、確保は増えない。store の待ちの累積は `Mutex` の取得の前後に `Instant::now()` を 2 回呼ぶだけである。閾値は `CosChatLaunch` の欄に持ち、試験だけが 0 や最大値に差し替える。
- D5: 本番 DB の写しで測るときは、写し（`$TMPDIR` の複製）の上で 1 つの root の行を new に戻し、その delivery を Preparing にする。こうして本番の「delivery の待ちの間に new の行が残る」状態を写しの上に作り、`crates/celeris/tests/tick_prod_copy_steps.rs` で `root_done_tick` を測る。元の backup・本番 DB・state dir には書かない。
- D6: この ADR は計測だけを決める。原因の修正（例: `progress_markdown` の git 起動の回数を減らす）は、計測の結果を見てから別の ADR か付記で決める（同じ task の WorkUnit root-done-fix）。

## 試験
- 閾値を 0 にした dispatcher で `tick_cos_chat_launch` と `tick` を回し、`cos.root_done.*` の項目が `step_us` に出ることを確かめる。閾値を最大にしたときは出ないことを確かめる。時計・sleep・負荷は使わない。
- store の待ちの累積は、別の thread が書き込み接続を持っている間に書き込みを呼び、待ちが正になることを確かめる。待っている thread の数（`writer_waiters`）を見てから lock を放すので、負荷で順序が崩れても結果は変わらない。
