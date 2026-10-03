---
tasks: [01M3YFCJKMNWQ13HRS52M5BSWW]
---
# ADR-0133: 受信箱（人の判断）と通知（知らせ）の 2 系統

- 日付: 2026-10-02
- 状態: **実装済み**（inbox-model / notify-store / notify-feed / api / outbound / gui-compat / verify の全葉
  完了。web 葉は人の決定 `ui-overlap = c` により UI/UX task `01M3XTCNKMQBCHKSZ7Y1GF6ZM4` へ `superseded`）。
  付記（rules-wire / notify-status / sync-main 葉、2026-10-02）: 1) D4 の自動片付け規則（inbox-rules、task
  `01M3YF3NS2EGTZD2BBWNPG1K28` の `788e5cc0`・`739cd209`）は `git cherry-pick -x` で内容を変えずに取り込み
  （このブランチの `966dd2b1`・`559bb4ab`）、`human_inbox` の自動で閉じるに結合済み。規則の本体は
  `task_ops::inbox` の `attention_suppression` だけで、この task は重複実装していない。試験:
  `crates/task-api/tests/inbox_notifications.rs` の `auto_close_drops_meaningless_items_and_keeps_failed_needing_a_decision`
  （置き換え済み failed 子・終端 task の attention が受信箱から消え、判断が要る failed は残り、`suppressed` に規則別件数）、
  task-ops の `inbox_cleanup_*`。2) D6 の `GET /api/v1/notify` への 4 設定値（`inbox_batch_secs`・
  `inbox_reminder_secs`・`digest_interval_secs`・`digest_max_lines`）と経路ごとの最終送信時刻は実装済み
  （試験 `crates/task-api/tests/notify.rs` の `get_notify_status_exposes_route_settings_and_last_successful_sends`）。
  3) migration は最新 main の取り込み時に全 celeris/* を再走査し `0041_feed_notices.sql`（版数 41）へ振り直した（D3.2 の付記）。
  詳細は [phase-inbox-notifications.md](../progress/2026-10-02-inbox-notifications.md)
- 関連: ADR-0033（報告・認可）、ADR-0037 / ADR-0050（Discord 通知）、ADR-0067 D4（承認の材料）、
  ADR-0070 D1（失敗の分類）、ADR-0074 D2.4（途中確認）、ADR-0079 D7 / D8（決定の要求・計画の承認）、
  ADR-0080 D5（browser の待ち）、ADR-0081（web/ SPA）、ADR-0121 D3（配送の取りこぼし）、
  **ADR-0131 D7（受信箱の決定論的な片付け規則。task `01M3YF3NS2EGTZD2BBWNPG1K28` の葉 inbox-rules）**、
  CLAUDE.md「ディスパッチャやストアに LLM 呼び出しを入れない」

## 状況

人に向けた出口が役割の重なったまま別々にある（2026-10-02 時点の HEAD）:

- `GET /api/v1/inbox`（`task_ops::inbox::Inbox`）が `approvals`・`questions`・`drafts`・`attention`
  （`AttentionItem` 7 種）・`browser_waits`・`decisions` を束ねる。
- 別に `/api/v1/approvals`（認可）、`/api/v1/decisions`、`/api/v1/tasks/{id}/{approve,accept,answer,retry,reopen}`、
  `/api/v1/tasks/{id}/execution/{plan-gate,phase-gate}`、`/api/v1/browser/waits`、`/api/v1/knowledge/inbox`、
  `/api/v1/reports`（既読・通知済み）、`/api/v1/notify`（Discord への送り出し、`NotificationKind` 11 種）がある。
- 同じ出来事が受信箱の attention と Discord 通知の両方に出る（例: `failed` は `AttentionItem::Failed` と
  `NotificationKind::TaskFailed`）。判断の要らない報告や、親が別の子で完了して意味を失った `failed` が受信箱に
  残り続ける。
- 報告の既読は `reports.read_at`、「通知済み」は API プロセスのメモリ（`last_notified_at`）で、通知の
  既読・束ねの概念は無い。`notifications` 表（migration 0008/0009/0019）は Discord への**送り出しの待ち行列**で、
  人が読む一覧ではない。

人の方針（2026-10-02）: **人の判断が要るものは受信箱、それ以外は通知**。1 つの出来事は片方にだけ出る。
画面は人の決定 `ui-overlap = c` により、web/ の受信箱・通知の画面は UI/UX task `01M3XTCNKMQBCHKSZ7Y1GF6ZM4`
の screens-work が作る。この task は API まで（gui/ は互換の最小限）。

## 決定

### 用語

- **受信箱（inbox）**: 人が何かを決めないと先へ進まないものだけ。各項目は「何を決めるか・選択肢・推奨・期限・
  止めている範囲・答え方」を持ち、**答えると消える**。DB に行を持たない**派生の一覧**（正は各領域の状態）。
- **通知（notification）**: 判断の要らない知らせ。既読を持ち、同種は束ねる。DB に行を持つ（D3）。
- **送り出し（outbound）**: 受信箱の新着と通知の要約を Discord 等の外へ出すこと（D6）。既存の
  `notifications` 表はこの待ち行列として残す。

### D1. 今の出口の対応表

各行は**ちょうど 1 つ**の系統に割り当てる。「受信箱」の行の `kind` は D2 の `InboxKind`、「通知」の行の
`kind` は D3 の `NoticeKind`。「経路」は答え・中身を持つ既存の endpoint（受信箱の答えは D5 の
`POST /inbox/items/{id}/answer` がここへ委ねる）。

#### D1.1 `GET /api/v1/inbox` の各種類

| 今の出口 | 割り当て | kind | 答え方（委ね先） / 備考 |
|---|---|---|---|
| `approvals[]`（Human check の Approval 子、`kind=Approval ∧ ready`） | 受信箱 | `acceptance_check` | `POST /tasks/{approval_id}/approve` / `reject` |
| `questions[]`（`approval_id = null`） | 受信箱 | `question` | `POST /tasks/{id}/answer` |
| `questions[]`（`approval_id` あり） | 受信箱 | `authorization` | `POST /approvals/{id}/decide`。同じ止まりを `question` として**二重に出さない**（認可の行に吸収） |
| `drafts[]`（`project_plan = null`） | 受信箱 | `draft_accept` | `POST /tasks/{id}/accept`（一括は親ごとに 1 項目、`drafts` を束ねる） |
| `drafts[]`（`project_plan` あり） | 受信箱 | `project_plan` | `POST /projects/{id}/project-plan/{version}/decide` |
| `attention[type=failed]`（ADR-0131 D7 の R1〜R4 で外れなかったもの） | 受信箱 | `failed` | `POST /tasks/{id}/retry` / `reopen` / `cancel`（ADR-0131 D7 の failed の cancel） |
| `attention[type=failed]`（R1〜R4 で外れたもの） | 通知 | `auto_recovered` | 受信箱には出ない。D3.3 の規則で 1 件の通知（束ね） |
| `attention[type=requeue_limit_near]` | 通知 | `requeue_limit_near` | まだ止まっていない警告。上限に達して `failed` になれば受信箱の `failed` |
| `attention[type=unroutable]` | 受信箱 | `unroutable` | 担当の付け替え（`PATCH /tasks/{id}` の hint）か `cancel` |
| `attention[type=cluster_unavailable]` | 受信箱 | `cluster_login` | 人の TOTP 再ログイン（クラスタ接続画面）。答えは「ログインした」の確認で、接続の回復で消える |
| `attention[type=phase_checkpoint]` | 受信箱 | `phase_gate` | `POST /tasks/{id}/execution/phase-gate`（continue / replan / withdraw） |
| `attention[type=plan_approval]` | 受信箱 | `plan_gate` | `POST /tasks/{id}/execution/plan-gate`（approve / replan / withdraw）。`decision_ids` は別の `decision` 項目として並び、この項目の `blocked_by` に載る |
| `attention[type=delivery_skipped]` | 受信箱 | `delivery_skipped` | 担当を付けて配送し直すか、配送しないことを選ぶ（ADR-0121 D3 の操作） |
| `browser_waits[]` | 受信箱 | `browser_wait` | `POST /tasks/{id}/browser/waits/{wait_id}/{credential,registered,decision}` |
| `decisions[]` | 受信箱 | `decision` | `POST /decisions/{id}/answer` |
| `counts.by_status` | どちらでもない | — | 出口ではなく集計。D5 の `GET /inbox` 互換のまま残す（新 API には載せない） |

#### D1.2 個別の endpoint

| 今の出口 | 割り当て | kind | 備考 |
|---|---|---|---|
| `GET /approvals?pending=true`（未決の認可） | 受信箱 | `authorization` | D1.1 の `questions[approval_id]` と同じ項目（1 件の認可 = 1 項目）。`pending=false` の履歴は出口ではない（領域の一覧として残す） |
| `GET /decisions`（未回答） | 受信箱 | `decision` | D1.1 の `decisions[]` と同じ項目。回答済み・取り下げ済みの一覧は領域の一覧として残す |
| `POST /tasks/{id}/approve` / `accept` / `answer` / `retry` / `reopen` | 受信箱（答える操作） | — | 出口ではなく**答える操作**。D5 の answer が委ねる先 |
| `GET /browser/waits`、`GET /tasks/{id}/browser/waits` | 受信箱 | `browser_wait` | D1.1 の `browser_waits[]` と同じ項目。task ごとの browser 認可方針（`/tasks/{id}/browser/policy`）は設定であり出口ではない |
| `GET /knowledge/inbox`（`_inbox/` の候補） | 受信箱 | `knowledge_review` | 候補 1 件ずつではなく「KB 取り込み待ち n 件」の **1 項目**に束ねる（答えは KB 画面で `accept` / `reject`、0 件で消える）。ADR-0131 D6 の日次整理 job が `enabled ∧ mode=apply` の間はこの項目を作らない（判断は日次整理の `decision` で来る） |
| `GET /reports`（`kind=progress`・`result`） | 通知 | `report` | 報告の本文は `/reports/{id}` に残る。既読は通知へ移す（D5） |
| `GET /reports`（`kind=bad_news`） | 通知 | `bad_news` | 判断が要るなら起こした側が `decision` を出す（悪い知らせ自体は知らせ） |
| `GET /reports`（`kind=question`） | 受信箱 | `question` | 止まっている task の `question` 項目に吸収（報告から通知を作らない） |
| `GET /reports`（`kind=proposal`） | 受信箱 | `draft_accept` / `project_plan` | 提案の実体は draft / 案件計画。その受信箱項目に吸収（報告から通知を作らない） |
| `POST /reports/read`、`POST /reports/notified` | 通知（既読操作） | — | D5 の段取りで通知の既読へ置き換える |
| `GET /api/v1/notify`、`POST /notify/test` | 送り出しの状態 | — | 出口ではなく D6 の送り出しの設定・状態。残す |

#### D1.3 `task_core::notify::NotificationKind`（Discord への送り出し）全種

送り出しは「受信箱の新着」「通知の要約」の 2 経路だけになる（D6）。既存の種類は次の系統の経路へ移す。
旧種類の値は既存の行を読むために enum に残すが、新しい行は D6 の 2 種（`inbox_new`・`digest`）だけを作る。

| `NotificationKind` | 割り当て | 新しい元 |
|---|---|---|
| `milestone_ready` | 通知 | 既に凍結（ADR-0079 D13）。新規生成なし、旧行の読み取りのみ |
| `approval_pending` | 受信箱 | `authorization` 項目の新着 |
| `question_blocked` | 受信箱 | `question` 項目の新着 |
| `bad_news` | 通知 | `bad_news` |
| `secretary_reply` | 通知 | `secretary_reply` |
| `task_ready` | 通知 | `task_done` |
| `cluster_login_needed` | 受信箱 | `cluster_login` 項目の新着 |
| `task_failed` | 受信箱 | `failed` 項目の新着（R1〜R4 で外れた失敗は送らない） |
| `phase_checkpoint` | 受信箱 | `phase_gate` 項目の新着 |
| `decision_requested` | 受信箱 | `decision` 項目の新着（24 時間の再通知は D6 の受信箱経路が引き継ぐ） |
| `plan_approval` | 受信箱 | `plan_gate` 項目の新着 |

#### D1.4 新しく通知になる出来事（今どこにも出ていないか、受信箱から外すもの）

`delivery`（配送の完了・失敗の自動回復）、`release`（release の作成・昇格の結果）、`cron_run`（定期実行の
結果の要約。ADR-0131）、`auto_recovered`、`requeue_limit_near`。D3.3 で規則を書く。

#### D1.5 重複しないことの規則

- 1 つの出来事（event の行 / 状態の遷移 / 報告 1 件）は純関数 `task_ops::route::classify(source) -> Route`
  （`Inbox(InboxKind) | Notice(NoticeKind) | None`）で**ちょうど 1 つ**に振り分ける。受信箱の項目と通知の
  両方を同じ出来事から作る経路を持たない（verify 葉の試験で全 source を列挙して確かめる）。
- 受信箱の項目が答えられて消えたことは通知にしない（人が自分でした操作なので）。自動で閉じたこと
  （ADR-0131 D7）は D3.3 の `auto_recovered`（失敗が別の子で置き換わった場合）だけを通知にし、他の規則
  （R2 取り消し済み・R3 終端）で外れた件数は応答の `suppressed` と日次要約（ADR-0131 D8）に出るだけ。

### D2. 受信箱項目の共通形

`task_ops::inbox::InboxItem`（新。既存の `Inbox` 構造体は互換のため残す）:

| 欄 | 型 | 内容 |
|---|---|---|
| `id` | string | 決定的な id `<kind>-<元の id>`（例 `decision-01M3…`、`plan_gate-01M3…-v2`、`failed-01M3…-<遷移番号>`）。URL にそのまま使える文字だけ |
| `kind` | `InboxKind` | D1 の 14 種: `decision`・`plan_gate`・`phase_gate`・`question`・`authorization`・`acceptance_check`・`draft_accept`・`project_plan`・`browser_wait`・`failed`・`unroutable`・`cluster_login`・`delivery_skipped`・`knowledge_review` |
| `title` | string | **何を決めるか**（1 行、決定的な定型文 + 元の問い） |
| `detail` | string? | 判断材料の短い本文（質問文・失敗の理由・計画の要約など。600 字で切る） |
| `options[]` | `{key, label, needs_note, effect}` | **選択肢**。`needs_note=true` は note 必須（replan 等）。`effect` は選んだら何が起きるかの 1 行 |
| `recommended` | string? | **推奨**の option key（決定の要求の `recommended`、計画の承認は `approve`、failed は `class=infra` なら `retry`・`work` なら推奨なし） |
| `due_at` | string? | **期限**（RFC 3339）。browser wait の期限、決定の要求の再通知時刻など、元に期限が無ければ `null` |
| `blocking` | `{tasks: [TaskRef], units: [string], root: TaskRef?, summary}` | **止めている範囲**（決定の `needed_before`、計画の承認なら root の木全体、質問なら その task） |
| `blocked_by` | string[] | この項目より先に答えるべき受信箱項目の id（plan_gate → その計画の decision） |
| `answer` | `{method, path, body_schema}` | **答え方の操作**。新 API の `POST /api/v1/inbox/items/{id}/answer` と、委ね先の既存 endpoint（D1 の表）を両方載せる |
| `task` / `project_id` | TaskRef? / string? | 元の task・案件（画面のリンク用） |
| `created_at` / `age_secs` | string / u64 | 積まれた時刻と経過 |
| `links` | `{label, href}[]` | 判断材料（成果物・報告・計画の見取り図）への API path |

選択肢は kind ごとに決定的に固定する（例 `failed`: `retry`・`reopen`・`cancel`。`plan_gate`: `approve`・
`replan`・`withdraw`。`decision`: 元の `options[]` をそのまま）。並びは `due_at` の近い順 → `kind` の固定順
（`decision`・`plan_gate`・`phase_gate`・`authorization`・`browser_wait`・`question`・`acceptance_check`・
`draft_accept`・`project_plan`・`failed`・`unroutable`・`cluster_login`・`delivery_skipped`・`knowledge_review`）
→ `created_at` の古い順。

### D3. 通知の型・保存・出来事から通知を作る規則

#### D3.1 型

`task_core::feed::Notice`（`task_core::notify::Notification` は送り出しの待ち行列のまま）:

- `kind: NoticeKind` — `task_done`・`report`・`bad_news`・`secretary_reply`・`delivery`・`release`・
  `cron_run`・`auto_recovered`・`requeue_limit_near` の 9 種（追加は ADR で）。
- `group_key: String` — **束ね key**。`<kind>:<範囲>`（D3.3 の表）。未読の束は group_key ごとに高々 1 行。
- `count: u32` — 束ねた出来事の件数。`title` は最新の 1 件、`summary` は「ほか n−1 件」を決定的に付ける。
- `read_at: Option<time>` — **既読**。既読にした束に同じ group_key の出来事が来たら新しい束（行）を作る。
- `first_at` / `last_at`、`project_id`・`task_id`（最新の 1 件）、`links`。

#### D3.2 保存（migration）

migration `0041_feed_notices.sql`（下の付記で 0040 から振り直し。全 celeris/* ブランチの `crates/task-core/migrations` を走査して 0038 まで
使用、0039 は ADR-0131 の cron-jobs が使う見込み。実装時に再走査し、使われていれば次の空き番号）:

```sql
CREATE TABLE feed_notices (
  id          TEXT PRIMARY KEY,          -- ULID
  kind        TEXT NOT NULL,
  group_key   TEXT NOT NULL,
  title       TEXT NOT NULL,
  summary     TEXT NOT NULL,
  project_id  TEXT,
  task_id     TEXT,
  links_json  TEXT NOT NULL DEFAULT '[]',
  count       INTEGER NOT NULL DEFAULT 1,
  first_at    TEXT NOT NULL,
  last_at     TEXT NOT NULL,
  read_at     TEXT
);
CREATE UNIQUE INDEX feed_notices_open_group ON feed_notices(group_key) WHERE read_at IS NULL;
CREATE INDEX feed_notices_unread ON feed_notices(read_at, last_at DESC);
CREATE TABLE feed_sources (               -- 冪等: 1 出来事は 1 回だけ数える
  source_key  TEXT PRIMARY KEY,           -- 例 'event:<seq>' / 'report:<id>' / 'cron_run:<id>'
  notice_id   TEXT NOT NULL REFERENCES feed_notices(id) ON DELETE CASCADE,
  recorded_at TEXT NOT NULL
);
CREATE TABLE feed_cursor (name TEXT PRIMARY KEY, value TEXT NOT NULL);  -- 走査位置（events の seq 等）
```

付記（notify-store 葉 → sync-main 葉で振り直し、2026-10-02）: 実装時の再走査で 0038（work_unit_sessions）・
0039（cron_jobs・write_sets）が他ブランチで使用中だったので当初 `0040_feed_notices.sql` にしたが、最新 main の
取り込み時（sync-main 葉）の再走査（`git for-each-ref refs/heads/celeris/` × `git ls-tree`）で 0040 も
behind_targets が 4 ブランチで使用中と分かり、`git mv` で **`0041_feed_notices.sql`（版数 41）** に振り直した
（main は 0037 まで）。版数の飛びを許すため、`task_core::store::migrations::RESERVED_VERSIONS = [38, 39, 40]` を置き、
`migrate` は「記録の無い版数を順に当てる」（予約は飛ばし記録しない）形にした。記録が連続する DB では従来と同じ。
統合で本物の 0038/0039/0040 が入ったら予約から外して `migration_sql` に足せば、版数 41 の DB にも後から当たる。
表には対象（`target_kind`・`target_id`）の列を足した。DB の版数は従来どおり `MAX(version)` で読む（`open_client`
の古い・新しいの判定も同じ）。予約版数は記録されないので、試験で「版数 N の DB」を作るとき
（`crates/celerisctl/tests/no_migrate.rs` の `db_at`、`SCHEMA_VERSION - 1` = 40 は予約）は `N` より大きい記録を
消してから `INSERT OR IGNORE` で `N` を記録する。

`feed_sources` の挿入と束の `count` の加算は同じ transaction で行う（daemon の再起動・二重走査で数が
増えない）。保持は既読から 30 日で削除（daemon の既存 GC の段で）。

#### D3.3 出来事から通知を作る規則（決定的、LLM なし）

daemon の tick の新しい段 `feed_notices`（`task_ops::feed::collect(store, now)`）が `feed_cursor` 以降の
events と reports を読み、D1.5 の `classify` で `Notice` になるものだけを束に足す。

| NoticeKind | 元の出来事 | group_key |
|---|---|---|
| `task_done` | root task（`parent_id = null`）の `Transitioned → done`。子 task・leaf の完了は通知しない | `task_done:project:<project_id or none>` |
| `report` | level 0 の報告で `kind ∈ {progress, result}` | `report:project:<project_id or none>` |
| `bad_news` | level 0 の報告で `kind = bad_news` | `bad_news:project:<project_id or none>` |
| `secretary_reply` | 案件の対話への assistant の返事（既存 `scan_secretary_reply` と同じ条件） | `secretary_reply:project:<project_id>` |
| `delivery` | delivery の merge 完了・verify の結果（ADR-0118/0121 の events） | `delivery:project:<project_id>` |
| `release` | release の作成・昇格の結果（`promoted.json` の観測、ADR-0040） | `release` |
| `cron_run` | cron が作った task の終端（ADR-0131 `Event::CronFired` を持つ task）と `skipped_*` の履歴 | `cron_run:<job_id>` |
| `auto_recovered` | ADR-0131 D7 の `attention_suppression` が R1 または R4 を返した `failed` task（初めてそうなった時） | `auto_recovered:root:<root_id>` |
| `requeue_limit_near` | 今の `RequeueLimitNear` と同じ判定が初めて真になった task | `requeue_limit_near` |

`auto_recovered` は `attention_suppression` の**出力を読むだけ**で判定を持たない（D4）。

### D4. 自動で閉じる規則の分担

- **意味を失った項目の片付け**（置き換え済み `failed` 子 R1、取り消された木の `failed` 子 R2、終端 task に残る
  attention R3、同じ unit の再試行で通った `failed` R4、`failed` の cancel）は、task
  `01M3YF3NS2EGTZD2BBWNPG1K28` の葉 **inbox-rules** が `task_ops::inbox`（`build_attention`）に実装し、純関数
  `attention_suppression(task, by_id) -> Option<SuppressRule>` と応答の `suppressed: {rule: count}` として公開する
  （ADR-0131 D7）。**この task はこれを再実装しない**。受信箱の `failed`・`unroutable` 等の候補は
  `build_attention` の出力（規則を適用した後）から作り、`auto_recovered` の判定もこの関数を呼ぶ。規則を足す・
  直すときは ADR-0131 側（inbox-rules）を直す。
- **この task が足すのは 2 つだけ**:
  1. **答えたら消える**: 受信箱は D2 の派生の一覧で、DB に項目の行を持たない。答える操作（D1 の委ね先）が状態を
     書き換えた後の `GET /inbox/items` には、その項目は構造的に現れない（各 kind の派生条件が「未決」だから）。
     `POST /inbox/items/{id}/answer` は委ね先が成功した後に 200 を返し、応答に `removed: true` を載せる。
  2. **分類**: D1 の対応表（`classify`）と D2 の共通形への写し。
- 既に各領域にある「取り下げ・取り消し・期限切れで消える」条件（決定の `withdrawn`、認可の `withdrawn`、
  browser wait の `revoked`/期限、draft の cancel）は今の派生条件のままで、新しい規則ではない。
- 実装順の依存: この task の inbox-model 葉は ADR-0131 の inbox-rules が main に入っていればその関数を使い、
  まだなら `build_attention` の今の出力をそのまま使う（規則の仮実装を置かない）。verify 葉が結合状況を記録する。

### D5. API

#### 受信箱（新）

| method | path | 内容 |
|---|---|---|
| GET | `/api/v1/inbox/items?project=&kind=` | `{items: InboxItem[], counts: {total, by_kind}, suppressed: {rule: count}}`。`suppressed` は ADR-0131 D7 の出力をそのまま渡す |
| GET | `/api/v1/inbox/items/{id}` | 1 件（無ければ 404 = 既に答えられた・閉じた） |
| POST | `/api/v1/inbox/items/{id}/answer` | `{option, note?, payload?}` → 委ね先の操作を呼ぶ（**管理系**、既存の require_admin）。成功 200 `{removed: true, item_id, result}`、既に無い 404、選択肢外 400、委ね先の 409 はそのまま |

`payload` は kind 固有の追加の入力だけ（`browser_wait` の資格情報の登録は秘密を扱うので answer では受けず、
`answer.path` に既存の credential endpoint を示す）。

#### 通知（新）

| method | path | 内容 |
|---|---|---|
| GET | `/api/v1/notifications?unread=&kind=&project=&limit=&before=` | `{items: Notice[], unread: u32, next_before?}`（`last_at` の新しい順） |
| GET | `/api/v1/notifications/unread-count` | `{unread, by_kind: {kind: n}}`（件数は束の数ではなく `count` の和も `events` に載せる。D7 の例） |
| POST | `/api/v1/notifications/{id}/read` | body `{}`。冪等（既読なら 200 のまま）。**管理系** |
| POST | `/api/v1/notifications/read-all` | body `{before?: rfc3339, kind?: NoticeKind, project?: id}`。返り値 `{marked: n}`。**管理系** |

#### 既存 endpoint の互換と廃止の段取り

1. **段 1（api 葉）**: 新 API を足す。既存は全部そのまま動く。`GET /api/v1/inbox` は応答の形を変えない。
2. **段 2（api 葉の中で同時）**: `GET /api/v1/inbox`、`POST /reports/read`、`POST /reports/notified` に
   `Deprecation: true` と `Link: <後継>; rel="successor-version"` の header を付ける。`POST /reports/read` は
   その報告を元にした通知も既読にする（新旧で既読がずれない）。`/reports/notified` は no-op + header。
3. **段 3（人の決定）**: web/ の screens-work と gui/ が新 API へ移ったこと（`rg '/api/v1/inbox[^/]'` が
   web/・gui/ で 0 件）を確かめ、30 日以上たってから人が廃止を決める。廃止は 410 Gone + 後継の Link。
- `/approvals`・`/decisions`・`/browser/waits`・`/knowledge/inbox`・`/reports`（`GET`）・
  `/tasks/{id}/{approve,accept,answer,retry,reopen}`・`/execution/{plan,phase}-gate` は**領域の endpoint として
  廃止しない**（履歴・詳細・答えの実体）。出口としての役割だけが受信箱・通知に移る。
- `/api/v1/notify` は送り出しの設定・状態として残し、D6 の欄を足す。

### D6. 外部送り出し（Discord）

送り出しは 2 経路だけ。どちらも既存の `notifications` 表（待ち行列）・webhook の秘密の規律（ADR-0037 D3）・
1 tick 1 通・429 の待ちをそのまま使う。

| 経路 | いつ | 束ね | 既定値 |
|---|---|---|---|
| **受信箱の新着**（`NotificationKind::InboxNew`） | 受信箱に**新しい項目 id** が現れたとき（前回送った id の集合との差）。即時 | 最初の新着から `inbox_batch_secs` の間に現れた項目を 1 通に束ねる（最大 5 行 + 「ほか n 件」+ リンク）。`key = inbox:<最初の項目 id>` | `inbox_batch_secs = 60`。未回答のまま `inbox_reminder_secs` たった項目は 1 回だけ再通知（束ねる）: `86400` |
| **通知の要約**（`NotificationKind::Digest`） | `digest_interval_secs` ごと。前回の要約以後に未読の通知が増えたときだけ | kind ごとに「件数 + 最新の題名」を 1 行（最大 `digest_max_lines` 行）。`key = digest:<区間の終わり>` | `digest_interval_secs = 3600`（`0` = 送らない）、`digest_max_lines = 10` |

- 既存の 11 種の個別送信は廃止し（D1.3）、上の 2 種だけを作る。`bad_news` も要約経路（判断が要るものは
  `decision` として受信箱経路で即時に出る）。
- 設定は `[notify]` に `inbox_batch_secs`・`inbox_reminder_secs`・`digest_interval_secs`・`digest_max_lines`
  を足す（`interval_secs`・`discord_webhook_secret`・`gui_base_url` は今のまま）。`GET /api/v1/notify` に
  この 4 値と、2 経路それぞれの最後の送信時刻を載せる。
- 判定は決定的（LLM なし）。文面は定型文。

### D7. 画面の分担

- web/ の受信箱・通知の 2 つの入口（判断を直接返す受信箱、既読・束ねの通知）は人の決定 `ui-overlap = c` により
  UI/UX task `01M3XTCNKMQBCHKSZ7Y1GF6ZM4` の **screens-work** が作る。この task は API まで（下の節の形を守る）。
- gui/（旧 GUI）は互換の最小限: 既存の受信箱画面は `GET /api/v1/inbox` のまま、ヘッダに通知の未読数
  （`GET /notifications/unread-count`）と通知一覧への入口を 1 つ足すだけ（葉 gui-compat）。

## UI/UX task が使う API

以下の形を api 葉が実装し、schema（`docs/api/v1/api-v1.schema.json`）に載せる。web/ はこの形だけに依存する。

### 受信箱の一覧

`GET /api/v1/inbox/items`

```json
{
  "items": [
    {
      "id": "decision-01M3YG7Q2Z9X4K8N5T0A1B2C3D",
      "kind": "decision",
      "title": "決定: 通知の要約の既定間隔をどうするか",
      "detail": "outbound 葉の既定値。短いほど早く知るが Discord が騒がしくなる。",
      "options": [
        {"key": "1h", "label": "1 時間", "needs_note": false, "effect": "digest_interval_secs = 3600"},
        {"key": "6h", "label": "6 時間", "needs_note": false, "effect": "digest_interval_secs = 21600"},
        {"key": "other", "label": "その他（note に書く）", "needs_note": true, "effect": "note の値を使う"}
      ],
      "recommended": "1h",
      "due_at": "2026-10-03T14:00:00Z",
      "blocking": {
        "tasks": [{"id": "01M3YFCJKMNWQ13HRS52M5BSWW", "title": "受信箱・通知・認可を整理し…"}],
        "units": ["outbound"],
        "root": {"id": "01M3YFCJKMNWQ13HRS52M5BSWW", "title": "受信箱・通知・認可を整理し…"},
        "summary": "unit outbound が待っている"
      },
      "blocked_by": [],
      "answer": {
        "method": "POST",
        "path": "/api/v1/inbox/items/decision-01M3YG7Q2Z9X4K8N5T0A1B2C3D/answer",
        "body_schema": {"option": "string (options[].key)", "note": "string? (needs_note なら必須)"},
        "native": {"method": "POST", "path": "/api/v1/decisions/01M3YG7Q2Z9X4K8N5T0A1B2C3D/answer"}
      },
      "task": {"id": "01M3YFCJKMNWQ13HRS52M5BSWW", "title": "受信箱・通知・認可を整理し…"},
      "project_id": "agent-platform",
      "created_at": "2026-10-02T14:00:00Z",
      "age_secs": 1800,
      "links": [{"label": "計画", "href": "/api/v1/tasks/01M3YFCJKMNWQ13HRS52M5BSWW/tree"}]
    },
    {
      "id": "failed-01M3YH0000000000000000FAIL-7",
      "kind": "failed",
      "title": "失敗: api 葉（review 不合格、attempt 3/3）",
      "detail": "cargo test -p task-api inbox_items が 1 件失敗",
      "options": [
        {"key": "retry", "label": "やり直す（複製）", "needs_note": false, "effect": "POST /tasks/{id}/retry"},
        {"key": "reopen", "label": "同じ task を開き直す", "needs_note": false, "effect": "POST /tasks/{id}/reopen"},
        {"key": "cancel", "label": "諦める", "needs_note": false, "effect": "failed → cancelled（ADR-0131 D7）"}
      ],
      "recommended": null,
      "due_at": null,
      "blocking": {"tasks": [{"id": "01M3YH0000000000000000FAIL", "title": "api"}], "units": [], "root": null, "summary": "この task だけ"},
      "blocked_by": [],
      "answer": {"method": "POST", "path": "/api/v1/inbox/items/failed-01M3YH0000000000000000FAIL-7/answer",
                 "body_schema": {"option": "string", "note": "string?"}},
      "task": {"id": "01M3YH0000000000000000FAIL", "title": "api"},
      "project_id": "agent-platform",
      "created_at": "2026-10-02T13:10:00Z",
      "age_secs": 4800,
      "links": []
    }
  ],
  "counts": {"total": 2, "by_kind": {"decision": 1, "failed": 1}},
  "suppressed": {"r1_parent_done": 3, "r3_terminal": 1}
}
```

### 受信箱に答える

`POST /api/v1/inbox/items/{id}/answer`

```json
{"option": "1h", "note": null}
```

応答 200:

```json
{"removed": true, "item_id": "decision-01M3YG7Q2Z9X4K8N5T0A1B2C3D", "result": {"delegated_to": "/api/v1/decisions/01M3YG7Q2Z9X4K8N5T0A1B2C3D/answer", "status": 200}}
```

**答えると消える保証**: 200 を返した時点で委ね先が状態を書き終えており、以後の `GET /inbox/items` に同じ id は
現れない（D4。kind ごとの結合試験で確かめる）。既に答えられた・自動で閉じた項目への answer は 404
（`type: inbox-item-gone`）で、画面は一覧から消すだけでよい。409 は委ね先の衝突（例: 計画の版が変わった）で、
画面は一覧を取り直す。`needs_note` の選択肢で note が空なら 400。

### 通知の一覧・未読数・既読化

`GET /api/v1/notifications?unread=true&limit=20`

```json
{
  "items": [
    {
      "id": "01M3YJ5N0T1C3S0000000000AA",
      "kind": "task_done",
      "group_key": "task_done:project:agent-platform",
      "title": "完了: 受信箱・通知・認可を整理し…",
      "summary": "完了: 受信箱・通知・認可を整理し…（ほか 2 件）",
      "count": 3,
      "project_id": "agent-platform",
      "task_id": "01M3YFCJKMNWQ13HRS52M5BSWW",
      "links": [{"label": "task", "href": "/api/v1/tasks/01M3YFCJKMNWQ13HRS52M5BSWW"}],
      "first_at": "2026-10-02T09:00:00Z",
      "last_at": "2026-10-02T15:20:00Z",
      "read_at": null
    }
  ],
  "unread": 4,
  "next_before": null
}
```

- **束ね**: 1 行 = 1 束（`group_key` ごとに未読の束は高々 1 つ）。`count` は束ねた出来事の数、`summary` は
  「ほか count−1 件」を含む。既読にした後に同じ `group_key` の出来事が来たら新しい行ができる。
- `GET /api/v1/notifications/unread-count` → `{"unread": 4, "events": 9, "by_kind": {"task_done": 1, "report": 2, "cron_run": 1}}`
  （`unread` は未読の束の数 = バッジに出す数、`events` は `count` の和）。
- 個別の既読化: `POST /api/v1/notifications/{id}/read`、body `{}` → `{"id": "…", "read_at": "…"}`（冪等）。
- 全件の既読化: `POST /api/v1/notifications/read-all`、body `{"before": "2026-10-02T15:30:00Z"}`（`kind`・
  `project` で絞れる。省略で全未読）→ `{"marked": 4}`。`before` は画面が一覧を取った時刻を渡し、取った後に
  来た通知を誤って既読にしない。
- 変更の購読: 既存の SSE（`/api/v1/stream`）に `inbox_changed`・`notifications_changed` の 2 種の軽い合図を
  足す（中身は持たず、画面は上の GET を取り直す）。

## 帰結

- 人が見る入口は 2 つになり、受信箱は「答えれば減る」一覧になる。判断の要らない知らせは既読で片付く。
- 受信箱は派生の一覧のままなので、答え・取り消しと受信箱がずれない（DB の正は各領域）。代わりに一覧の構築は
  毎回の派生で、`build_attention` の性能の注意（events の index）は今のまま引き継ぐ。
- Discord の通知は種類ごとの個別送信から 2 経路に減る。既存の `NotificationKind` の旧値は読み取り専用で残る。
- 自動で閉じる規則は ADR-0131 の 1 か所だけにあり、この ADR はそれを読むだけ。規則の変更が受信箱と通知の
  両方に同時に効く。
- 未解決: `knowledge_review` を 1 項目に束ねる形が使いやすいかは日次整理 job を有効にした後に見直す。

## 付記: 通知フィードの同期を差分にする（2026-10-03、release 3527c8e39ee2 の verify 退行）

**状況**: main 3527c8e3 の release は verify で 2 回とも ok=false（GUI 無応答・503、煙試験の task が 61 秒
ready のまま、staging の slow api request 31 件）。staging DB（本番の写し: task 697・events 172,994・
level 0 の報告 257・発言 200・delivery 54）の `feed_cursor` は events 2170 で止まっていた。

**原因**（測定。`crates/task-ops/tests/feed_measure.rs` を写しに対して実行、ローカルディスク）:
- 29d0ffc5 が tick ごとに `sync_notifications`（5982b3cb）を呼ぶようにした。その同期は
  1. events を**全部読み切るまで**回り、**1 行ごとに** `feed_cursor_set`（書き込み 1 回）していた。
     初回は 172,994 回の書き込みで 11.3 秒。NFS の staging では数分たっても 2170 行で、tick が dispatch に戻らない。
  2. 追いついた後も、tick ごとに報告（最大 1000 件）・発言（最大 10,000 件）・delivery の全件に
     `notice_record`（`BEGIN IMMEDIATE` の書き込み transaction）を開き、記録済みなら捨てていた（写しで tick
     ごとに約 500 回・100 ms。NFS ではその何十倍）。発言ごとに `delivery_get` も引いていた。
- その間、同じ DB の書き込み lock を tick が取り続けるので、API（別接続）も待たされた。

**決定**:
- D-a 走査位置: events は `id`、報告・発言は `created_at`（`feed_cursor` の `reports`・`messages`）を持ち、
  その後だけを読む。events は 1 回の同期で `EVENT_BUDGET`（2048 行）までに区切り、残りは次の tick が読む。
- D-b 記録済みかどうかは `NoticeStore::notice_sources_known`（読み取り接続、`feed_sources` の主キー）で一括して
  先に確かめる。新しい出来事の記録と走査位置の更新は `notice_record_batch` で**1 つの書き込み transaction**に
  まとめる（同期 1 回の書き込みは高々 1 回。新しい出来事も位置の前進も無い tick は書き込み接続を取らない）。
  NFS 上の DB では commit の回数がそのまま tick の時間になる（staging は 1 commit 約 0.1 秒だった）ため。
- D-c 通知の読み取り（`notice_list`・`notice_get`・`notice_unread_count`・`feed_cursor_get`）と
  `delivery_get`・`delivery_list` は読み取り接続（ADR-0064 D4）にする。受信箱の 1 回の構築で書き込み接続を
  取る回数は 300 task で 79 → 4 回になった（failed の task ごとの `delivery_get`）。
- D-d 回帰試験は時計ではなく件数で固定する: `SqliteStore::lock_counts`（書き込み・読み取り接続を取った回数）と
  `FeedSyncStats`（読んだ行・store に渡した出来事・書き込み transaction の数）。index の migration は足さない
  （報告・発言の表は小さく、費用の大半は書き込みの回数だったため）。

**修正後の測定**（同じ写し）: 1 回目 113 ms（events 2048 行・記録 419 件・書き込み transaction 1 回）、
追いつくまで 85 回（各回の書き込みは 1 回）、追いついた後は 1〜2 ms・書き込み接続 0 回。記録の結果
（`feed_sources` 502 件）は修正前と同じ。verify と同じ起動（`--mode verify`、写しの DB、煙試験は verify.sh の
smoke と同じ台本）を手元で再現し、修正後の debug build で煙試験 8.1 秒で done・slow api request 0 件。
（ローカルディスクでは修正前の 3527c8e3 も 12.6 秒で通る。退行は commit が遅い NFS の staging でだけ表に出る。）

**未解決**: 本番（active）の tick loop は 30 秒ごとに `notify::schedule_routes`（29d0ffc5）で受信箱全体を
構築する。受信箱の構築は task ごとに events を読む既存の形（写しで 0.2〜0.6 秒）で、verify（`verify` 役は
通知を回さない）の退行とは別。差分化するなら別の task にする。
