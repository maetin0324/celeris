---
title: 健全性v2 R3 — TaskStore の consumer 駆動分割と domain/storage 境界
tasks: [01M6TYVJT2HG9NY5FW9FMCZS9A]
status: running
updated: 2026-11-10
---
# 健全性v2 R3 — TaskStore の consumer 駆動分割と domain/storage 境界

基準は着手時の `$CELERIS_WU_BASE`（WU ports-design、件名「integrate wu/final-record (phase reclose)」）。設計判断は [ADR 2026-11-10 TaskStore の consumer 駆動の port](../adr/2026-11-10-taskstore-consumer-ports.md)。

## 現在地
- ports-design（この段）: 計測台本・着手時の計測・削除の対象・ADR を置いた。crates/ は変えていない。
- 次: mcp-port（celeris-mcp を `OrgDirectory`・`TaskQuery` へ移す）、store-prune（`prune-list.json` の 17 名）、table-writers（`cos_inbox_items`・`chat_client_requests` の書き手を task-core の chat module へ）、record（前後計測）。

## 着手時の計測（提出時の HEAD で実行）
台本 [measure.py](2026-11-10-taskstore-consumer-ports/measure.py)、値 [baseline.json](2026-11-10-taskstore-consumer-ports/baseline.json)。
- TaskStore の fn 宣言 193、supertrait 16（B0 と同じ command）。supertrait を含めて `dyn TaskStore` から見える method 308、宣言の重複 0。
- consumer ごとの TaskStore 直の method 数（試験でない src）: task-dispatch 95・task-ops 78・task-api 58・celeris 50・celerisctl 12・celeris-mcp 9・task-worker 4。
- celeris-mcp: `TaskStore` の言及 6（src）、method 9（`event_rows_for`・`get`・`list_page`・`milestone_list`・`org_get`・`org_list`・`org_upsert`・`project_get`・`project_list`）。objective の見込み 10 と 1 個違う（ADR D1）。
- 呼び手の無い TaskStore method 4（`milestone_transition_status`・`project_plan_apply`・`project_plan_decide_apply`・`project_set_text`）、試験だけが呼ぶ method 14。
- 表の書き手（試験以外）: `cos_inbox_items` は task-api の `cos/triage_view.rs` と task-core の `chat/override_op.rs`・`chat/triage.rs`、`chat_client_requests` は task-api の `chat/attachments.rs` と task-core の `chat/attachments.rs`。

## 直す file の確定（cargo check の試し）
- 作業ツリーで 8 method（`get` を除く）の宣言を `OrgDirectory`・`TaskQuery` 相当の trait へ移し、`cargo check --workspace --all-targets --keep-going` の誤り（E0599・E0782）が 0 になるまで import を足した。4 回で 0 になった。試した変更は `git checkout -- crates` で戻した。
- 結果 [port-fix-files.json](2026-11-10-taskstore-consumer-ports/port-fix-files.json): 64 file（task-api 23・task-ops 12・celeris 11・task-core 5・celeris-mcp 5・task-dispatch 5・celerisctl 3）、task-worker 0、R2 の write-set 8、R4 の write-set 3。

## table-writers の結果
詳細は [table-writers.md](2026-11-10-taskstore-consumer-ports/table-writers.md)。
- `cos_inbox_items` の印は task-core `chat::triage::cos_triage_resolve_tx(&Transaction, …)` が書き、task-api `cos/operations.rs` は `cos_operation_apply` の transaction でこれを呼ぶ。`chat_client_requests` の `attachment_ref` の記録は task-core `ChatAttachmentStore::add_ref_request` が pin と同じ transaction で書く。
- `measure.py --check-writers cos_inbox_items,chat_client_requests` → exit 0。原子性の試験 `cos_chat_triage_mark_and_audit_commit_together`（task-api `tests/cos_triage.rs`）を足した。
- 範囲外: task-api に残る読みの SQL（`cos/triage_view.rs` の読み 3 関数、`chat/attachments.rs` の SELECT）。

## 証拠コマンドと結果（提出時の HEAD で実行）
- `python3 agent-docs/progress/2026-11-10-taskstore-consumer-ports/measure.py --json $TMPDIR/m.json` → exit 0、key 5 つあり
- `… measure.py --check-writers cos_inbox_items,chat_client_requests` → exit 1（`task-api/src/cos/triage_view.rs`・`task-api/src/chat/attachments.rs`）
- `… measure.py --check-single-decl` → exit 0（trait 17、fn 308、重複 0）
- `… measure.py --check-pruned agent-docs/progress/2026-11-10-taskstore-consumer-ports/prune-list.json` → exit 1（17 名が残る。store-prune の後に 0 になる）
- `git diff --name-only $CELERIS_WU_BASE -- crates` → 0 行

## 変更（commit の件名）
- `ports-design: TaskStore の consumer 計測台本・着手時の計測・削除の対象・port の ADR`

## 未解決
- 試験だけが呼ぶ TaskStore method 14 個は消すかを決めていない（`prune-list.json` の `held_test_only`）。本番の代わりの経路を確かめた method から、後の段で契約の廃止を書いて消す。
- `browser_task_policies` の書き手は task-core の 2 file だけで、task-api は書いていない（B0 D4 の「task-api が直接書く 3 表」のうち 1 表は既に task-core に寄っている）。

## 提案
- `get` を `TaskQuery` へ移すのは、R2 が `crates/task-worker/src/browser_tests.rs` を変え終えた後の別の段にする（具体型で `get` を呼ぶ file の候補は 266）。
