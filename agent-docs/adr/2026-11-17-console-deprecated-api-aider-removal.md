---
tasks: [01M7J3NKVC2KQ4NGJ4GESHJV0B]
---
# ADR 2026-11-17: Console 互換・deprecated API・aider adapter を撤去する

- 状態: 採用（task 01M7J3NKVC2KQ4NGJ4GESHJV0B、LoC 削減 v3 の子 F）。人が 2026-11-17 に 3 つの廃止を認めた（運用 session の問いへの人の答え）。
- 置き換える前提: [ADR-0048](0048-console.md) の D1〜D3（`GET /console`・`GET /console/stream`・`POST /console/instruct`）と、[CoS チャット ADR](2026-10-05-cos-chat-home.md) の「全案件 Console stream は `/console` の監視用ビューとして残す」。どちらにも付記でこの ADR を指した。

## 文脈
- ホームは CoS チャット（ADR 2026-10-05）で、人が Console に書いた最後の message は 2026-10-21 だった（operator が本番 DB で確認）。その後に Console の node に書かれていたのは `crates/celeris/src/delivery.rs` の自動引き渡しの通知（「取り込み完了」「デプロイ準備完了」「取り込み・リリース準備の確認が必要」）だけだった。
- この通知は `messages`（node_id='cos'）に書かれ、Console の画面で読めるほか、daemon の起動時の写し（`SqliteStore::backfill_legacy_cos`）が CoS のスレッドへ写していた。写し先は migration 0092 が残した legacy スレッドがある scope ではそちら（archived で web に出ない）、無い scope では固定スレッドで、写るのは次の daemon の起動の後だった。
- `GET /api/v1/inbox`・`POST /reports/read`・`POST /reports/notified` は deprecated の印の付いた旧 API で、受信箱は `GET /api/v1/feed`・通知の既読は feed の操作に移っていた。
- aider adapter は本番の config に記述が無く、本番の runs に adapter=aider は 0 件（operator が確認）。

## 決定

### D1. Console の 3 つの入口と画面を消す
- task-api の `GET /api/v1/console`・`GET /api/v1/console/stream`・`POST /api/v1/console/instruct`、Console のブロックの型（`ConsolePage`・`ConsoleBlock`・`ConsoleHello`・`InstructBody`・`ConsoleInstructAccepted`）、CoS の操作表の `console_instruction_chain` を消す。
- web の `/console` と `/org/:id`（その人の Console）の画面、`features/console/`、gateway の `/console/stream` の中継、ナビの「Console」と組織画面の「Console を開く」の導線を消す。`/org/secretary` の転送も消す。
- task-core の `chat_legacy_console_messages`（Console の読み取り専用）と、task-ops の `console` module の Console だけが使う写像を消す。時刻の比較（`at_nanos`）は `GET /tasks/{id}/timeline` が使うので残す。
- `GET /tasks/{id}/runs/{run_id}/events` は run のログ画面が使う現行の API なので残す（`crates/task-api/src/run_events.rs` に移す）。
- celeris-mcp の `console_instruct`・`console_reply` は、名前は Console だが CoS チャットの入口（`chat_mcp_instruct`、ADR 2026-10-27-relayed-instructions D1）で、運用 session の中継に使う現行の経路なので残す。`POST /org/cos/messages`（`chat_console_enqueue`）も今回の承認の範囲の外なので残す。

### D2. 自動引き渡しの通知は書いた時点で CoS チャットへ写す
- 人に向けた通知（`needs_user`。秘書の node に書くもの）は、`messages` への追記の直後に CoS の固定スレッドへ写す（`SqliteStore::cos_messages_project`。起動時の写しと同じ関数で、同じ行を 2 回写さない）。通知の行き先は Console から CoS チャットになり、daemon の起動を待たない。
- 写し先はその scope の固定 CoS スレッド（案件の通知は案件の CoS スレッド）だけにする。migration 0092 が残した `legacy:global`・`legacy:project:<id>` があっても入れない（[ADR 2026-10-22-cos-fixed-threads](2026-10-22-cos-fixed-threads.md) の D1 の写しの規則を置き換え、付記で指した）。初回の review の指摘: 旧い規則のままだと、本番の主な案件（legacy と固定の両方のスレッドがある）では通知が archived の legacy スレッドに入り、web（open のスレッドだけを一覧する）で読めなかった。0092 の時点にあった旧 Console の行は 0092 を当てた open で写し終えているので、規則を変えて動く行は 0092 より後の通知だけである。
- 0092 の後から本 release の起動までに legacy スレッドへ写った通知は移さない（legacy は読むだけの履歴。`messages` の行と task・release の画面の配送の状態は残る）。
- `messages` の行は残す。`Delivery.notification` の冪等の印（固定の `MessageId`）と、`GET /org/{id}/messages` の読み口のため。
- 部署に戻す通知（`needs_user` でないもの）は部署の node の `messages` のまま（node の会話の材料で、Console の撤去と関係しない）。

### D3. 旧い報告の API を消す
- `POST /api/v1/reports/read`・`POST /api/v1/reports/notified` と、それだけが進めていた `DaemonSnapshot.reports` の `last_notified_at`・`notify_now` を消す。CoS の操作表と skill の記述も同じ変更で消す。
- `GET /api/v1/inbox` も廃止を認められたが残した。release の検証 `scripts/selfdeploy/verify.sh` の `probe_api` がこの path の 200 と JSON を求めており（候補の release を検証で落とす）、`scripts/selfdeploy/` は高 risk の path class（selfdeploy）で、人が認めた write set の外にある。ほかに `scripts/dev/daemon-soak/daemon_soak.py` の読み取り GET と、試験 14 か所（`crates/celeris/tests/integration_api.rs` 5・task-api の `browser_waits`・`decisions` 2・`execution` 2・`integration_repair`・`list_and_detail`・`unmapped_suite_a/approvals`・`inbox_notifications`・`request_lock_counts`）が旧い形を読む。
  - 撤去の条件: `verify.sh` の `probe_api` を `GET /api/v1/inbox/items` に替える変更（selfdeploy の設計 review を通す）と、上の試験・soak の書き換え。
  - 撤去の責任者: 親 01M7J3N68JRNTP5Q4G934QWF36 の closeout が人に後続 task（selfdeploy の write set を持つもの）を提案する。

### D4. aider adapter を消す
- task-worker の `aider.rs`、celeris の `[adapters.aider]` の欄と例の設定を消す。`[adapters.aider]` は読み込みエラーにする（`deny_unknown_fields`）。

## 結果
- 本番の config の書き換えは要らない（`[adapters.aider]` は本番に無い）。
- Console の画面は無くなる。全案件の流れを見る代わりの画面は作らない（新機能を足さない。LoC 削減 v3 の禁止事項）。
