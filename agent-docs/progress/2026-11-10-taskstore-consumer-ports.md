---
title: 健全性v2 R3 — TaskStore の consumer 駆動分割と domain/storage 境界
tasks: [01M6TYVJT2HG9NY5FW9FMCZS9A]
status: running
updated: 2026-11-10
---
# 健全性v2 R3 — TaskStore の consumer 駆動分割と domain/storage 境界

基準は着手時の `$CELERIS_WU_BASE`（WU ports-design、件名「integrate wu/final-record (phase reclose)」）。設計判断は [ADR 2026-11-10 TaskStore の consumer 駆動の port](../adr/2026-11-10-taskstore-consumer-ports.md)。

## 現在地
- ports-design: 計測台本・着手時の計測・削除の対象・ADR を置いた。crates/ は変えていない。
- store-prune・table-writers・mcp-port: 済み（下の各節）。mcp-port は計画 v2 のとおり 6 method を移した（ADR の付記 2026-11-10）。
- 次: record（前後計測）。

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

## 証拠コマンドと結果（提出時の HEAD で実行）
- `python3 agent-docs/progress/2026-11-10-taskstore-consumer-ports/measure.py --json $TMPDIR/m.json` → exit 0、key 5 つあり
- `… measure.py --check-writers cos_inbox_items,chat_client_requests` → exit 1（`task-api/src/cos/triage_view.rs`・`task-api/src/chat/attachments.rs`）
- `… measure.py --check-single-decl` → exit 0（trait 17、fn 308、重複 0）
- `… measure.py --check-pruned agent-docs/progress/2026-11-10-taskstore-consumer-ports/prune-list.json` → exit 1（17 名が残る。store-prune の後に 0 になる）
- `git diff --name-only $CELERIS_WU_BASE -- crates` → 0 行

## store-prune の結果
詳細は [store-prune.md](2026-11-10-taskstore-consumer-ports/store-prune.md)。
- `prune-list.json` の 17 名を全部消した（TaskStore method 4・その `_impl` 4・B0 割り当て 9）。着手時の grep で呼び手が見つかった名前は無く、list から外した名前も無い。使い手が無くなった型 `ProjectPlanApply`・`ProjectPlanMilestoneChange`・`StoredRouteJudgment` も消した。消した試験は無い（17 名を呼ぶ試験が無かった）。
- `task_store.rs` の fn 宣言 193 → 189。store module の本体の行（size rule の `module_lines`）19676 → 19396、物理行 40947 → 40618、`SqliteStore` の method 729 → 713。
- `measure.py --check-pruned …/prune-list.json` → exit 0（`remaining=0`）、`--check-single-decl` → exit 0（`fns=304`）。

## mcp-port の結果（計画 v2）
- 移した 6 method: `OrgDirectory`（`crates/task-core/src/store/org_directory.rs`）へ `org_get`・`project_list`・`project_get`・`milestone_list`、`TaskQuery`（`crates/task-core/src/store/task_query.rs`）へ `list_page`・`event_rows_for`。どちらも `TaskStore` の supertrait で、`impl … for SqliteStore` は既存の `<method>_impl` への 1 行転送だけ（SQL・transaction・migration・event は変えていない）。宣言は `task_store.rs` から消した（写していない）。
- `org_list`・`org_upsert`・`get` は `TaskStore` に残した（理由は提案節）。
- 計測（`measure.py --json`、提出時の HEAD で実行。前は着手時の `baseline.json`）:

| 指標 | 前 | 後 |
|---|---:|---:|
| celeris-mcp の src の `TaskStore` の言及（`taskstore_mentions`） | 6 | 3 |
| celeris-mcp の src で `TaskStore` を名指す file | 5（auth.rs・resources.rs・tools/org.rs・tools/projects.rs・tools/tasks.rs） | 3（resources.rs・tools/org.rs・tools/tasks.rs） |
| celeris-mcp が直に呼ぶ `TaskStore` の method | 9 | 3（`get`・`org_list`・`org_upsert`） |
| `auth.rs` から見える method 数 | 308（`dyn TaskStore`） | 7（`dyn McpClientStore`） |
| `tools/projects.rs` から見える method 数 | 308 | 6（`OrgDirectory` 4・`TaskQuery` 2） |
| `resources.rs`・`tools/org.rs`・`tools/tasks.rs` から見える method 数 | 308 | 304（`TaskStore` のまま。304 は store-prune の後の値） |
| `TaskStore` の fn 宣言（B0 の command） | 193（store-prune の後 189） | 183 |
| `TaskStore` の supertrait | 16 | 18 |
| celeris-mcp の試験で SQLite を開く数（`sqlite_store_opens_in_tests`） | 7 | 7 |

- 直した file は 45。宣言と trait の 7 file（`task_store.rs`・`task_store_impl.rs`・新しい `org_directory.rs`・`task_query.rs`、再 export の `store/mod.rs`・`lib.rs`、doc comment の `store/query.rs`）と、import だけの 38 file（task-api 21・task-ops 5・celeris-mcp 5・task-dispatch 3・celeris 2・celerisctl 1・task-core 1、うち試験 15）。import の要否は `cargo check --workspace --all-targets --keep-going` の E0599 と unused import の警告で決めた。R2 の write_set に入る file は 7（`crates/task-api/tests/` の import）、R4 の write_set に入る file は 3（`task-dispatch/src/dispatcher/cos_chat/projects.rs`・`cos_chat/review.rs`・`project_wake.rs`）。task-dispatch の src の差分は import の行だけ。
- fixture は簡素にならなかった。`authenticate` は `&dyn McpClientStore` を受けるので、fake で試すなら 7 method（前は 308）を実装すれば足りる。しかし `auth/tests.rs` の fixture は `SqliteStore::open_in_memory()` の 1 行で、token の hash で client を引く SQL も一緒に確かめている。fake に替えると 7 method 分の実装が増え、その SQL を確かめなくなるので替えなかった。減ったのは fake を作るときに要る method の数（上の表）で、fixture の行ではない。
- 振る舞いは変えていない。state+event を同時に書く操作は今までどおり 1 つの store method の 1 transaction の中にあり、port の trait は method の境界を変えない（ADR D5）。test-affected（下の証拠）が既存の transaction・replay・migration の試験を含めて合格した。

## 証拠コマンドと結果（mcp-port、提出時の HEAD で実行）
- `grep -rlw TaskStore crates/celeris-mcp/src` → `resources.rs`・`tools/org.rs`・`tools/tasks.rs` の 3 file
- `grep -nE 'fn (org_get|project_list|project_get|milestone_list|list_page|event_rows_for)\b' crates/task-core/src/store/task_store.rs` → 0 行
- `python3 agent-docs/progress/2026-11-10-taskstore-consumer-ports/measure.py --check-single-decl` → exit 0（trait 19、fn 304、重複 0）
- `git diff --name-only $CELERIS_WU_BASE | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R3 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → exit 0（`write-set R3: ok`）
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 8179 passed・23 skipped、web の vitest 712 passed、tsc -b 合格、ts-verify 20 passed）。実行は commit「mcp-port: org_get・…の宣言を store/ の OrgDirectory・TaskQuery へ移し…」の時点で、その後の commit は agent-docs だけを変える
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/check-health.sh` → exit 0（verdict pass、fail 級 0、new 0、worsened 1 は下の未解決）

## 変更（commit の件名）
- `ports-design: TaskStore の consumer 計測台本・着手時の計測・削除の対象・port の ADR`
- `mcp-port: org_get・project_list・project_get・milestone_list・list_page・event_rows_for の宣言を store/ の OrgDirectory・TaskQuery へ移し、celeris-mcp を狭い trait に依存させる`
- `mcp-port: ADR の付記と進捗に port の結果を書く`

## 未解決
- 試験だけが呼ぶ TaskStore method 14 個は消すかを決めていない（`prune-list.json` の `held_test_only`）。本番の代わりの経路を確かめた method から、後の段で契約の廃止を書いて消す。
- `browser_task_policies` の書き手は task-core の 2 file だけで、task-api は書いていない（B0 D4 の「task-api が直接書く 3 表」のうち 1 表は既に task-core に寄っている）。

- check-health の `dup.state`（warning、worsened）: 表 `t` の書き手として `measure.py` の docstring（23・24 行目の `INSERT … INTO t` の説明）が数えられた。ports-design で入った誤検出で、mcp-port の変更とは関係ない。fail 級ではない。

## 提案
- **残した 3 method と次の工程への引き継ぎ**（mcp-port）:
  - `org_upsert`・`org_list`: `OrgDirectory` へ移すと、R3 の write_set の外の 4 file を直す必要がある。`crates/task-core/src/approval/tests.rs`・`crates/task-core/src/report/tests.rs` は UFCS の `TaskStore::org_upsert(&store, …)` を呼ぶ（supertrait へ移すと E0782 で、trait 名を直す必要がある）。`crates/celerisctl/tests/org_migrate_v2.rs`（`org_list`）と `crates/celeris/tests/trial_context_canary_e2e.rs`（`org_upsert`）は具体型で呼び、file 名の数字が write_set の正規表現 `[a-z_/]+` に合わない。次の段はこの 4 path を write_set に入れてから 2 method を `OrgDirectory` へ移す。移した後、celeris-mcp の `resources.rs`・`tools/org.rs` は `TaskStore` を名指さなくなり、名指す file は `tools/tasks.rs` だけになる。
  - `get`: 具体型で呼ぶ `crates/task-worker/src` の試験 3 file（`browser_tests.rs`・`workspace_prune/tests.rs`・`workspace_targets/tests.rs`）を直す必要がある。`browser_tests.rs` は R2 の write_set なので、R2 の後に `TaskQuery` へ移す（下の項）。
  - write_set を変えるには B0 の `slices.json` の変更が要り、検査器・baseline の変更として別の review の対象になる。
- `get` を `TaskQuery` へ移すのは、R2 が `crates/task-worker/src/browser_tests.rs` を変え終えた後の別の段にする（具体型で `get` を呼ぶ file の候補は 266）。
