---
title: "root 完了時の CoS まとめ通知の設計（ADR 2026-10-15-root-done-cos-summary）"
tasks: [01M4YES0YP2F68ZG10RYANHYHB]
status: done
updated: 2026-10-15
---
# root 完了時の CoS まとめ通知の設計

設計は ADR 2026-10-15-root-done-cos-summary（write-adr 葉が書く）。この進捗ファイルの『事実』は facts 葉
（task 01M4YETK5J7Q554ATEB4DSHJP6）がコード（base `612287e5521f`）と本番 DB の読み取りで集めたもの。実装はしない。

## 事実

### (a) NotificationKind と (kind,key) の重複排除

- `crates/task-core/src/notify.rs:67-107` の `NotificationKind` は 15 種（`ALL` は `:152`）:
  `cos_escalation`・`cos_fallback`・`inbox_new`・`digest`・`milestone_ready`（R5a で廃止、旧行の読み取り用）・
  `approval_pending`・`question_blocked`・`bad_news`・`secretary_reply`・`task_ready`（key = `task id:遷移番号`）・
  `cluster_login_needed`・`task_failed`（key = `task id:遷移番号`）・`phase_checkpoint`・`decision_requested`・`plan_approval`。
- 重複排除は `notifications` 表の `UNIQUE(kind, key)`（`crates/task-core/migrations/0008_notifications.sql:28`
  `idx_notifications_kind_key`）。`notification_upsert_pending`（`crates/task-core/src/notify.rs:345-371`）は
  `INSERT OR IGNORE` で、既にあれば `None`。判定は tick ごとに何度走ってもよく、再起動しても行が DB に残るので二重にならない
  （試験 `upsert_is_idempotent_per_kind_and_key`、`:462`）。送信結果は `notification_mark`（`ok`=1/NULL/0、
  最大 `MAX_NOTIFY_ATTEMPTS`=3）。
- notice store（ADR-0133、`agent-docs/adr/0133-inbox-and-notifications.md`、migration 0041）は別系統:
  `feed_notices`（`group_key` ごとに未読 1 行、`UNIQUE(group_key) WHERE read_at IS NULL`、出来事が足されると `count`+1）と
  `feed_sources`（`source_key` 主キー。例 `event:<events.id>`・`release:created:<sha12>`・`delivery:<task>:<head>:<state>`）。
  出来事の重複排除は `feed_sources` の主キー。notice は Discord に直接は出ない（下の (d)）。
- notice の種（`crates/task-core/src/feed.rs:84` `NoticeKind`）: `task_done`・`report`・`bad_news`・`secretary_reply`・
  `delivery`・`release`・`cron_run`・`auto_recovered`・`requeue_limit_near`・`disk`。
  - `task_done`（`crates/task-ops/src/notify_feed.rs:269-352`）: events の `Transitioned{to: Done}` から。`parent_id` あり・
    木の子（`is_tree_child`）・support task は除く。group は `task_done:project:<id>`。**delivery の有無で skip しない**。
  - `delivery`（同 `:505-589`）: delivery が `Ready`/`Blocked` のとき、group `delivery:project:<id>`、source key
    `delivery:<task>:<head>:<state>`。`release`: delivery に `release` が付いたら `release:created:<sha12>`。

### (b) scan_task_ready / scan_task_failed（`crates/celeris/src/notify.rs`）

- `scan_task_ready`（`:537-597`）: `status=done` を `UpdatedDesc` で `TASK_SCAN` 件読み、`updated_at >= since`・
  `milestone_id` なし・support task でない・**木の子でない**（`:555-560`、ADR-0079 D6/D11）ものについて、
  最後の `WorkerFinished{role: None}` の `done: ` 以降を要約に使う。**`store.delivery_get(task.id)` があれば skip**（`:573-576`、
  「検証後の delivery 通知にまとめる」）。候補は `TaskReady`・key = `transition_key`（`:526-535`、`task id:done 遷移の event seq`）・
  本文「仕事『題名』が完了しました。成果を確認してください。<要約><web_base_url>/tasks/<id>」。
- delivery 側の候補（同 `scan_at` の `:245-277`）: `delivery.notification`（chat message id）があり state が `Ready`、または
  `Blocked` で detail が `[needs-human]` 始まりなら `SecretaryReply`（key `message:<id>`、本文「デプロイ準備完了: …/releases」）。
- `scan_task_failed`（`:599-649`）: `status=failed`・`updated_at >= since`・support でない・木の子でない root。分類と理由は
  `task_ops::derive::classify_task_failure`、delivery に `release` があれば「成果は main に取り込み済み（release <sha12>）だが
  レビューで不合格」を足す。key は `transition_key`。cancelled は対象外（`statuses: [Failed]` だけ）。
- **重要: 現行 daemon は `scan`/`schedule` を呼ばない。** 呼び出しは試験（`crates/celeris/tests/notify.rs`）だけで、
  `crates/celeris/src/daemon/tick_loop.rs` には無い（`git grep -n "notify::schedule\|notify::scan" crates/celeris/src` は
  notify.rs 自身のみ）。tick_loop は起動時に `notify::triage::cutover`（`:66`）を呼び、送信は
  `notify::select_routes_batch`（`notify.rs:1156-1170`、「Only the two CoS outbox kinds may reach the webhook」）で
  `CosEscalation`/`CosFallback` だけを選ぶ（`tick_loop.rs:380-406`）。それ以外の未送信行は `discard_pending` で
  `ok=0, error=NOT_CONFIGURED` にして退ける（`tick_loop.rs:383-399`）。`schedule_routes`（`inbox_new`・`digest`）も daemon からは
  呼ばれていない。つまり **`scan_task_ready`・`scan_task_failed` は死んだコード経路**（本番 DB の (e) と一致）。

### (c) CosFallback（`crates/celeris/src/notify/triage.rs`）

- `render`（`:22-101`）: `notify.web_base_url` が無い・http(s) でない・userinfo/query/fragment 付きなら
  `NO_WEB_URL`（`:14`）でエラー（リンク無しで成功扱いにしない）。本文は notifications 行の JSON packet
  （`summary`・`options`・`recommended`・`recommendation_reason`・`blocking`・`reason`・`web_path`）から決定的に組む。
  見出しは `CosEscalation`=「CoS から判断のお願い」、`CosFallback`=「CoS 不在のため直接通知」（`:39-43`）、fallback は
  「不在理由」を足す（`:85-87`）。末尾に `<base><web_path>` と「回答はリンク先で」。`web_path` は `/` 始まり・`//` 不可・改行不可。
  1 通 `CONTENT_MAX_CHARS` に切る。
- `source_unresolved`（`:108-161`）/`withdraw_if_resolved`（`:163-176`）: 送信直前に `cos_inbox_items` の元 source が
  最新 revision かつ未解決か（notice は未読で `count` が一致、他は派生 inbox に同じ id・created_at）を読み取り専用接続で確かめ、
  解決済みなら store が取り下げる。
- `cutover`（`:178-232`）: 起動時 1 回。`feed_cursor.cos_route_version = cos_v1` が無ければ、派生 inbox の現在の待ちと
  旧未送信 notifications（cos_* 以外）を CoS 入力（`legacy_notification`）にし、同じ transaction で旧行を superseded にする。
- 退避の判定と本文は dispatcher 側（`crates/task-dispatch/src/dispatcher/cos_chat/fallback.rs`、ADR 2026-10-06-cos-inbox-triage-fallback）:
  `cos.enabled=false`・run の failed/stopped/interrupted/completed 取り残し・run 行なし・`unavailable_after_secs` 超過で、
  `cos_triage_outbox_claim(item,"fallback",body)`（`cos_notification_routes` の triple UNIQUE が arbitration）。
  `web_path` は task があれば `/tasks/<id>`、無ければ `/inbox`、notice は `/notifications`（`fallback.rs:178-181,386`）。
  受信箱スレッドに keyed system message `cos-fallback:<item_id>` を残す。LLM は呼ばない。

### (d) CoS chat 系 ADR: run の起動経路・スレッドの種類・リンク

- スレッド（`agent-docs/adr/2026-10-05-cos-chat-home.md` D1、migration 0050）: `chat_threads.kind` は `human`/`inbox`/`legacy`
  の CHECK 制約付き。`inbox` は部分 UNIQUE で 1 本（システム受信箱スレッド）。本番は human 8・inbox 1
  （`01M4B11RX2K4CMYDHGVEX0Q11D`「受信箱」）・legacy 4（archived）。root task 専用の kind は無い（足すなら migration で CHECK を変える）。
  `chat_threads.project_id` はあるが task id の列は無い。
- 一次対応（同 D3「一次対応の起動とルーティング」、`2026-10-06-cos-inbox-triage.md`・`-ingest.md`）: dispatcher が派生 inbox
  （events cursor）と notice store（notices cursor、`source_revision = count`）から `cos_inbox_items` へ
  `(source_kind,source_key,source_revision)` で upsert（`feed_cursor` の `cos_triage:<source>` と同じ IMMEDIATE transaction）、
  受信箱スレッドに system message を置き、受信箱スレッドに生きた run が無ければ `cos_triage_claim`（最大 20 件）で
  `launch.rs` の `start_thread(ClaimSource::Triage)` が run を起こす（`crates/task-dispatch/src/dispatcher/cos_chat/launch.rs:166,283-299`）。
  人の発言は `ClaimSource::Queue`。`secretary_reply` notice と CoS operation 由来の入力は取り込まない。
- run の起動（`2026-10-06-cos-chat-run-dispatch.md`）: dispatcher が保存しない一時 Task と `CosChatContext` を組み（`context.cos_chat`
  の有無だけで判定）、tick の通常 dispatch 後に `tick_cos_chat_launch`。chat run は task の events・lease を作らず `chat_runs`・
  `chat_events` が正本。credential は env `CELERIS_COS_RUN_CREDENTIAL` のみ。要約は worker の checkpoint、dispatcher は LLM を呼ばない。
  account 選択・容量は ADR-0089（`max_cos_runs`）。
- 受信箱スレッドの発言（`2026-10-07-cos-inbox-thread-conversation.md` D1）: triage run の終端ごとに dispatcher が SQLite だけから
  assistant 発言を 1 件決定的に作る（冪等 key `client_message_id = cos-triage-digest:<run_id>`）。CoS の最終文は使わない
  （triage run は出力 message を持たない）。D2 で fallback の handoff 文、D3 で人が受信箱スレッドに書ける。
  keyed system message は `chat_system_message_add_once`（`crates/task-core/src/chat/store.rs:890`）。
- Discord とリンク（cos-chat-home D3「Discord 通知」）: worker は webhook 秘密を持たず、resolve API が outbox（notifications 行）を作り
  notifier が送る。web_path はサーバが対象から導く同一アプリ内 path と照合し、絶対リンクは `[notify] web_base_url` + web_path
  （ADR 本文の `gui_base_url` は旧名。現行コードは `web_base_url` で、`gui_base_url` は読み込みエラー、
  `crates/celeris/src/notify/tests.rs:301-304`）。本番 config は `web_base_url = "http://192.168.1.103:7721"`。
  web でスレッドを開く path は `/?thread=<thread_id>`（`web/e2e/chat/queue.spec.ts:16`）、task は `/tasks/<id>`、
  変更は `/tasks/<id>?tab=changes`、release は `/releases`。
- 本番 `[cos]`: `enabled = true`、`harness = "claude-code"`、route は claude_oauth / claude-opus-5-5 の 1 件、`max_turns = 70`、
  `max_wall_secs = 900`。`[cos.triage]`: `unavailable_after_secs = 120`、`min_confidence = 0.85`、`human_required` 6 種。

### (e) 本番 DB の読み取り（root done 後に何が積まれたか）

開き方: `sqlite3 -readonly /local/celeris/data/db/celeris.sqlite3`（書き込み・migration・daemon 操作なし。2026-10-15 00:2x UTC、
events の最大 id 555195）。

1. 種類別の notifications（`select kind,count(*),max(created_at) from notifications group by kind`）:
   `task_ready` 20 件・最終 **2026-10-02T04:21Z**、`task_failed` 15 件・最終 2026-10-09T13:37Z、`inbox_new` 最終 10-07、`digest` 最終 10-07。
   `cos_escalation` 234 件（最終 10-14T16:46Z）・`cos_fallback` 216 件（最終 10-14T06:29Z）、最初の cos_* は 2026-10-07T11:12Z。
   `notification_scan_state.scanned_at` は 2026-10-03T04:00:56Z で止まっている（`schedule` が呼ばれていない証拠）。
   `feed_cursor.cos_route_version = cos_v1`。
2. 最近 done の root（`status='done' and parent_id is null`、support・日次整理・報告のまとめを除く）3 件の時系列:

| root | done 遷移（events.id / ts） | delivery | 積まれた notice（feed_sources） | CoS 入力（cos_inbox_items） | notifications（Discord） |
|---|---|---|---|---|---|
| 01M4M8D4AQ（routing 作り直し） | 555008 / 10-14T16:59:33Z | Ready、release 612287e5521f、`notification` あり | `event:555008` → task_done group rev 34。同時刻に delivery group（count 49）・release group rev 27 | task_done rev 34 **observed**（16:59:42Z「受理は人が task 画面で」）、release rev 27 observed。delivery group は 10-13T18 以降の item 0 件 | **0 件**（16:59〜17:15） |
| 01M4WSXTXW（verify の daemon が評価試行を消す） | 552045 / 10-14T12:23:04Z | Ready、release 64158056af89 | `event:552045` → task_done rev 32、release rev 25 | task_done rev 32 observed、release rev 25 observed | **0 件**（12:23〜12:40） |
| 01M4X19V0R（ローカルマシン設定） | 554585 / 10-14T15:57:37Z | **無し**（`delivery_skipped` 2 回） | `event:554585` → task_done rev 33 | task_done rev 33 observed、`unintegrated` 項目 escalated（16:01:22Z） | `cos_escalation` 1 件（16:01:40Z、ok=1、route escalation、source `unintegrated`） |

   使った SQL（要点）:
   `select id,seq,ts,json_extract(json,'$.type') from events where task_id=? and json_extract(json,'$.type') in ('transitioned','delivery_skipped',…)`、
   `select * from feed_sources where source_key in ('event:555008','event:552045','event:554585')`（3 件とも task_done group
   `01M4NERHYGGQVXMT1DF92TNSGP`）、`select source_revision,state,created_at,reason from cos_inbox_items where source_key=?`、
   `select n.kind,n.created_at,n.ok,r.route,r.source_kind from notifications n left join cos_notification_routes r on r.notification_id=n.id where n.created_at between …`。
3. 結論（事実）:
   - **root done で `TaskReady` は鳴っていない**（daemon が `scan_task_ready` を呼ばない。最終 task_ready は 10-02）。delivery の有無に
     依らない。`scan_task_ready` の「delivery があれば skip」は今は効いていない分岐。
   - delivery 経由の root でも **delivery 通知（`SecretaryReply`「デプロイ準備完了」）も Discord に出ない**（同じく `scan_at` が
     呼ばれない）。delivery/release/task_done は notice になり、CoS が observe して受信箱スレッドの digest に 1 行残るだけ。
   - Discord に出るのは CoS が escalate したもの（例 未取り込み root の `unintegrated`）か fallback だけ。root 完了そのものは人に
     能動的には届かない。
   - 付随して見えた事実: delivery notice group（count 49）は 10-13T18 以降 CoS 入力が 0 件。同じ `last_at` を持つ task_done group
     （id が大きい）で notices cursor `(last_at,id)` が進み、同時刻の delivery group が cursor 以下として飛ばされた可能性がある
     （推測。未検証）。

### (f) 決定的に集められる事実の出所

| 事実 | 正本 | 読み方（API / SQL / file） |
|---|---|---|
| 題名・目的 | `tasks`（`title`・`objective` 列） | `GET /api/v1/tasks/{id}` |
| 段階・WorkUnit・計画版 | `execution_plans`（`status=active/completed`、json）・`work_units` | `GET /api/v1/plans`・`GET /api/v1/tasks/{id}/tree` |
| 子 task と要約 | `tasks`（`parent_id`・`root_id`、子の `title`・status）、子の最終 `worker_finished.outcome` | `GET /api/v1/tasks/{id}/tree`・`/timeline` |
| 段の統合 commit | events `phase_integrated`（`phase`・`merged[].commit`・`head`）・`work_unit_committed` | `GET /api/v1/tasks/{id}/events` |
| 統合（main 取り込み） | events `integration_merged`（`head_sha`・`actor`）、`deliveries`（`base`・`head`・`target_sha`・`reviewed_sha`・`merge_candidate_sha`・state・detail） | `GET /api/v1/deliveries`・`/tasks/{id}/integrations` |
| release | `deliveries.json.release`（sha12）、release 本体は selfdeploy の releases dir | `GET /api/v1/releases` |
| 成果物 path | events `artifact_produced`（`name`・`path`・`sha256`）、task workspace の `artifacts/` | `GET /api/v1/tasks/{id}/artifacts` |
| 受け入れ結果 | events `review_verdict`（`criterion_idx`・`pass`・`reason`）、`work_unit_check_finished`・`integration_check_finished` | `GET /api/v1/tasks/{id}/events`・`/runs/{run}/result` |
| 完了の要約（worker 申告） | events `worker_finished.outcome`（`done: …`）— 自己申告なので単独では使わない | 同上 |
| 本番への移行節・未解決・提案 | 進捗ファイル `agent-docs/progress/YYYY-MM-DD-<slug>.md`（front matter `tasks:` で task に紐付く）の「本番への移行」「未解決事項」「提案」節 | リポジトリの統合 commit（`deliveries.head`）の tree を `git show <head>:<path>` で読む |
| 未取り込み・配送の止まり | events `delivery_skipped`・`integration_requested`/`answered`、派生 inbox の `unintegrated` | `GET /api/v1/cos/inbox`・派生 inbox |
| done 遷移の一意 id | events `transitioned{to:done}` の `seq`（`transition_key`）と global `events.id` | — |

## 未解決事項

- notices cursor の同時刻 tie で delivery group が CoS に渡っていない疑い（(e)-3）。本 task の範囲外。
- `scan`/`schedule`/`schedule_routes` と `TaskReady`・`TaskFailed` の生成コードは daemon から呼ばれない死んだ経路として残っている
  （破壊的変更の方針では撤去対象）。

## 提案

- ADR では「TaskReady の root 完了経路を置き換える」を、実態（既に鳴っていない死んだ経路）に合わせて「死んだ `scan_task_ready` と
  delivery の `SecretaryReply` 候補を消し、root 完了は新しい経路だけで鳴らす」と書く。`scan_task_failed` も同様に死んでいるので、
  failed/cancelled root の扱いは新経路に含めるか、TaskFailed を同じ撤去で消すかを決める。
- 重複排除は既存の `notifications UNIQUE(kind,key)` に乗せられる（例 kind=`root_done_summary`、key=`<task_id>:<done 遷移 seq>`。
  reopen 後の再 done も別 key になる）。
- 通知の kind は `root_done_summary`（key `<task_id>:<done 遷移の seq>`）を ADR D2 に採用した。

### D8 の推奨（failed／cancelled root）

- failed は新経路に含める: 同じ `RootDoneSummary` の仕組み（スレッド・事実ブロック・CoS・fallback）を再利用し、事実ブロックに
  失敗の分類と理由（`classify_task_failure`）・main 取り込み済みの有無を足す。kind は別（`root_failed_summary`）にせず、行の本文で
  区別するか `RootDoneSummary` を `RootSummary` に改名して outcome 欄を持たせる。`TaskFailed` と `scan_task_failed` は同じ変更で消す
  （どちらも死んだ経路）。
- cancelled は人が自分で止めたものなので通知しない（スレッドも作らない）。人が見たければ task 画面で足りる。
- 初回の実装は done だけ、failed は次の葉で足してよい。ただし `TaskFailed` を消すのは failed 対応と同じ葉にして、通知の空白期間を作らない。

### 実装の葉の分け方（提案）

1. store 葉: migration（`chat_threads` の kind `root_summary`・`task_id`、`TaskReady` 旧行の扱い、SCHEMA_VERSION）・`RootDoneSummary` kind・
   事実ブロックの組み立て（D4）と試験。
2. dispatch 葉: events cursor からの行作成・スレッド ensure・`ClaimSource::RootSummary` の run 起動・fallback（D6）。試験は stub の CoS run で、
   スレッド書込み後にだけ pending になること、再起動後の再開、重複排除。
3. notify 葉: `select_routes_batch` への追加・本文とリンク（D5）・`scan_task_ready` 等の旧経路と旧試験の撤去（D7）。
4. web 葉（必要なら）: 専用スレッドの表示（`/?thread=<id>`）が既存の一覧で出るかの確認。
5. close-out 葉: `bash scripts/dev/test-parallel.sh`、`cargo clippy --workspace -- -D warnings`、architecture-map の更新。
   起動試験は旧 `task_ready` 行を DB に実際に入れて migration を通す（grep で確かめる check を置く）。

## 本番への移行

- config 欄: 変更なし（ADR D9）。消す欄も無いので本番 config の書き換えは不要。`[notify] web_base_url`・`[cos] enabled = true` が本番に既にあることは確認済み（事実 (d)）。
- DB: 実装の release が migration（`chat_threads` の kind・task_id、`task_ready` 旧行）を持つ。migration は daemon 起動時に走り、データは失わない。
- release 昇格（人が実行する）: 実装の task が done になり delivery の release ができたら、`scripts/selfdeploy` の手順（verify → promote）で昇格する。
  この task は ADR だけで本番に変更は無い。
- 昇格後の確認: 次に root が done になった時、専用スレッドが作られ、Discord にスレッドと task へのリンク付きの通知が 1 通出ること。

## 証拠コマンド

- `grep -c '^## D[1-9] ' agent-docs/adr/2026-10-15-root-done-cos-summary.md` → 9
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → 下記の run で exit 0 を確認
- `git diff --stat $CELERIS_WU_BASE -- crates/` → 差分なし
