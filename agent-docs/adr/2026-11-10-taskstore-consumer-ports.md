---
tasks: [01M6TYVJT2HG9NY5FW9FMCZS9A]
---
# ADR 2026-11-10: TaskStore の consumer 駆動の port — 最初の consumer は celeris-mcp

- 状態: 採用（設計）。この ADR の WorkUnit（ports-design）ではコードを変えない。実装は同じ task の mcp-port・store-prune・table-writers、前後計測は record。
- 計測と検査: `agent-docs/progress/2026-11-10-taskstore-consumer-ports/measure.py`（標準ライブラリだけ）、着手時の値 `baseline.json`、削除の対象 `prune-list.json`、直す file `port-fix-files.json`。基準は着手時の `$CELERIS_WU_BASE`（件名「integrate wu/final-record (phase reclose)」）。

## 文脈
- `TaskStore`（`crates/task-core/src/store/task_store.rs`）は fn 宣言 193・supertrait 16 を持ち、supertrait を含めると 1 つの `dyn TaskStore` から 308 method が見える（計測 `agent-docs/progress/2026-11-10-taskstore-consumer-ports/baseline.json`、台本 `measure.py --json`。B0 と同じ command で 193・16）。
- 健全性v2 B0（[ADR 2026-11-07 健全性 v2 の基準](2026-11-07-health-v2-baseline.md) D4）は R3 に、consumer ごとの port、task-api が直接書く表の task-core への集約、未参照の store 関数 9 件の削除を割り当てた。
- 実装は既に `<method>_impl`（領域の file）と `impl TaskStore for SqliteStore` の 1 行転送（`task_store_impl.rs`）に分かれている。分けるべきなのは宣言（consumer が依存する面）であって、SQL ではない。

## 決定

### D1. 最初の consumer は celeris-mcp
- celeris-mcp の試験でない src が呼ぶ TaskStore 直の method は 9 個（`event_rows_for`, `get`, `list_page`, `milestone_list`, `org_get`, `org_list`, `org_upsert`, `project_get`, `project_list`）。objective の見込みは 10 個だったが、計測では 9 個だった（`get`・`insert`・`list` は HashMap などと名前が重なるので、受け手に `store` を含む呼び出しだけを数えた）。supertrait の method は 6 個（`mcp_call_record`, `mcp_client_by_token_hash`, `mcp_client_get`, `mcp_client_touch_last_used`, `node_session_active`, `report_list`）で、どれも既に狭い trait（`McpClientStore`・`McpCallStore`・`NodeSessionStore`・`ReportStore`）にある。
- celeris-mcp の src は R2・R4 の write-set に入らない。consumer の規模が小さく、移した結果（TaskStore を名指す file の数）を数で確かめられる。
- task-ops・task-dispatch・task-api・celeris は呼ぶ method が 50〜95 個あり、1 回目の対象にしない（D7）。

### D2. port の形: 能力ごとの trait へ宣言を移す
- 宣言を TaskStore から能力ごとの trait へ**移す**。各 fn の宣言は workspace に 1 か所（検査 `measure.py --check-single-decl`）。
- TaskStore はその trait を supertrait に持つ。`dyn TaskStore`・`T: TaskStore` で呼ぶ既存の consumer はそのまま動く（supertrait の method は import なしで呼べる。rustc で確かめた）。
- `impl <trait> for SqliteStore` は既存の `<method>_impl` への 1 行転送にする。SQL・transaction の本体は写さない。
- 1 回目に作る trait（file は `crates/task-core/src/store/` の下に 1 trait 1 file。`task_store.rs` には置かない。B0 の command が TaskStore の宣言の減りを数えられるようにするため）:
  - `OrgDirectory`（組織・案件・途中目標の名簿）: `org_list`・`org_get`・`org_upsert`・`project_list`・`project_get`・`milestone_list`
  - `TaskQuery`（task の一覧と event 行の読み出し）: `list_page`・`event_rows_for`
- `get` は移さない。具体型で `get` を呼ぶ file の候補（上限）は 266 で、`crates/task-worker/src` の 3 file（`browser_tests.rs`・`workspace_prune/tests.rs`・`workspace_targets/tests.rs`、`browser_tests.rs` は R2 の write-set）を含む。celeris-mcp の `tools/tasks.rs` だけが `get` のために TaskStore を import し続ける。
- celeris-mcp は TaskStore を名指さずに、使う trait だけを import する: `auth.rs` は `&dyn McpClientStore`、`resources.rs`・`tools/org.rs` は `OrgDirectory`、`tools/projects.rs` は `OrgDirectory`・`TaskQuery`、`tools/tasks.rs` は `TaskStore`（`get` だけ）・`TaskQuery`・`ReportStore`。
- celeris-mcp が `task_ops::gate` などへ `&SqliteStore` を渡す所（`&dyn TaskStore` への暗黙の変換）は task-ops の port で、この段では変えない。

### D3. 退けた案
- **consumer 名の trait（`McpStore` など）に写す案**: 同じ method を consumer ごとに宣言し直すと、同じ契約の宣言が consumer の数だけ増える（汎用規約 2・4）。consumer が増えるたびに trait の合成が増え、method の意味の所有者が分からなくなる。能力ごとの trait なら、別の consumer も同じ宣言を共有できる。
- **TaskStore を機械的に supertrait へ割る案**: 193 宣言を領域ごとの trait に移して TaskStore を supertrait の和にしても、consumer は `dyn TaskStore` のままで、見える method 数（308）も依存も変わらない。万能 store を名前を変えて作り直すだけで、親 task の禁止（「supertrait の寄せ集めで同じ万能 store を作り直すだけは不可」）に当たる。移すのは、それに依存だけする consumer がいる method に限る。

### D4. 具体型で呼ぶ所の import と、移す method の条件
- 宣言を移すと、具体型 `SqliteStore`（`Arc<SqliteStore>`・`&SqliteStore` を含む）で呼ぶ file に新しい trait の import が要る（E0599）。`TaskStore::<method>(…)` の UFCS は trait 名を直す（E0782）。
- 直す file は、作業ツリーで 8 method を実際に移し、`cargo check --workspace --all-targets --keep-going` の誤りが 0 になるまで import を足して決めた（4 回で 0、試した変更は戻した）。結果は `agent-docs/progress/2026-11-10-taskstore-consumer-ports/port-fix-files.json`。
- 合計 64 file（celeris 11・celeris-mcp 5・celerisctl 3・task-api 23・task-core 5・task-dispatch 5・task-ops 12、うち試験 36）。`crates/task-worker` は 0 file。
- R2 の write-set に入る file は 8（どれも `crates/task-api/tests/` の import 1 行）。R4 の write-set に入る file は 3（`task-dispatch/src/dispatcher/cos_chat/projects.rs`・`cos_chat/review.rs`・`project_wake.rs`、import 1 行）。R4 は R2・R3 の後に走る（B0 D2）ので同時には変えない。R2 と並ぶときは既存の `use task_core::{…}` の並びに足し、衝突は両側保持で解く。
- 条件: 移すのは `crates/task-worker/src` の file を直さずに済む method だけにする。8 method はこの条件を満たす。
- method ごとの直す file（`crates/` を省く）:
- `org_list`（OrgDirectory、14 file）: `celeris-mcp/src/resources.rs`, `celeris-mcp/src/tools/org.rs`, `celeris/src/browser_doctor.rs`, `celeris/src/browser_doctor_tests.rs`, `celeris/src/lib/tests.rs`, `celeris/tests/variant_propagation.rs`, `celerisctl/src/commands/org.rs`, `celerisctl/tests/org_migrate_v2.rs`, `task-api/src/console.rs`, `task-api/src/conversation.rs`, `task-api/src/handlers/org.rs`, `task-api/src/skills.rs`, `task-ops/src/add/tests.rs`, `task-ops/src/conversation/tests.rs`
- `org_get`（OrgDirectory、10 file、R2 の write-set 2 件）: `celeris-mcp/src/tools/org.rs`, `celeris-mcp/tests/mcp_integration.rs`, `celeris/src/lib/tests.rs`, `task-api/src/console.rs`, `task-api/src/conversation.rs`, `task-api/src/handlers.rs`, `task-api/src/handlers/org.rs`, `task-api/src/memory.rs`, `task-api/tests/browser_site_policy_db.rs`, `task-api/tests/cos_ops_admin_config.rs`
- `org_upsert`（OrgDirectory、26 file、UFCS・注釈 `task-core/src/approval/tests.rs`, `task-core/src/report/tests.rs`・R2 の write-set 1 件）: `celeris-mcp/src/tools/org.rs`, `celeris-mcp/tests/mcp_integration.rs`, `celeris/src/browser_doctor_tests.rs`, `celeris/src/knowledge_curation/tests.rs`, `celeris/src/knowledge_gc/tests.rs`, `celeris/src/knowledge_maint/tests.rs`, `celeris/src/lib/tests.rs`, `celeris/src/reports/tests.rs`, `celeris/tests/daily_curation.rs`, `celeris/tests/notify.rs`, `celeris/tests/trial_context_canary_e2e.rs`, `task-api/src/handlers/org.rs`, `task-api/tests/approvals.rs`, `task-core/src/approval/tests.rs`, `task-core/src/report/tests.rs`, `task-dispatch/src/approvals/tests.rs`, `task-dispatch/src/reports/tests.rs`, `task-ops/src/actions/tests.rs`, `task-ops/src/add/tests.rs`, `task-ops/src/approval/tests.rs`, `task-ops/src/conversation/tests.rs`, `task-ops/src/delegate/tests.rs`, `task-ops/src/edit/tests.rs`, `task-ops/src/notify_feed/tests.rs`, `task-ops/src/retry/tests.rs`, `task-ops/tests/browser_policy_autoattach.rs`
- `project_list`（OrgDirectory、8 file、R4 の write-set 2 件・R2 の write-set 1 件）: `celeris-mcp/src/tools/projects.rs`, `celerisctl/src/commands/projects.rs`, `task-api/src/console.rs`, `task-api/src/handlers/projects.rs`, `task-api/tests/cos_ops_projects_cron.rs`, `task-dispatch/src/dispatcher/cos_chat/projects.rs`, `task-dispatch/src/dispatcher/cos_chat/review.rs`, `task-ops/src/actions/tests.rs`
- `project_get`（OrgDirectory、17 file、R4 の write-set 1 件・R2 の write-set 4 件）: `celeris-mcp/src/tools/projects.rs`, `celeris-mcp/tests/mcp_integration.rs`, `task-api/src/changes.rs`, `task-api/src/docs.rs`, `task-api/src/handlers/authority.rs`, `task-api/src/handlers/project_controller.rs`, `task-api/src/handlers/projects.rs`, `task-api/src/handlers/roadmap.rs`, `task-api/src/repos.rs`, `task-api/tests/cos_operations_domains.rs`, `task-api/tests/cos_ops_projects_cron.rs`, `task-api/tests/project_auto_integrate.rs`, `task-api/tests/project_cos_guard.rs`, `task-core/src/store/roadmap_tests.rs`, `task-dispatch/src/dispatcher/project_wake.rs`, `task-ops/src/actions/tests.rs`, `task-ops/src/project_snapshot.rs`
- `milestone_list`（OrgDirectory、6 file、R2 の write-set 1 件）: `celeris-mcp/src/tools/projects.rs`, `task-api/src/console.rs`, `task-api/src/handlers/projects.rs`, `task-api/tests/project_plan.rs`, `task-ops/src/add/tests.rs`, `task-ops/src/lifecycle/tests.rs`
- `list_page`（TaskRead、8 file、UFCS・注釈 `task-core/src/store/mod.rs`, `task-core/src/store/query.rs`）: `celeris-mcp/src/tools/projects.rs`, `celeris-mcp/src/tools/tasks.rs`, `celeris/src/knowledge_maint/tests.rs`, `task-api/src/console.rs`, `task-api/src/handlers/projects.rs`, `task-core/src/store/mod.rs`, `task-core/src/store/query.rs`, `task-ops/src/lifecycle/tests.rs`
- `event_rows_for`（TaskRead、6 file）: `celeris-mcp/src/tools/tasks.rs`, `task-api/src/console.rs`, `task-api/src/docs.rs`, `task-api/src/handlers/task_io.rs`, `task-api/src/timeline.rs`, `task-ops/src/replay/tests.rs`
- このほか、宣言と転送を動かす `task-core/src/store/task_store.rs`・`task_store_impl.rs`、新しい trait の file 2 つ、再 export の `task-core/src/store/mod.rs`・`task-core/src/lib.rs` を変える。

### D5. transaction の規則
- state と event を同時に書く操作は、1 つの store method の中で 1 つの transaction に入れる（既存の `*_tx` と `transition` の形）。port の trait は method の境界を変えないので、原子性は今と同じ。
- port の trait に、呼び手が transaction を開いて複数の method を組み合わせる API（unit-of-work の型）を足さない。要る操作は store method として task-core に置く。
- `cos_inbox_items`・`chat_client_requests` の書き手は task-core の chat module（`crates/task-core/src/chat/`）に置く。書く関数は `&rusqlite::Transaction` を受け、CoS の操作の監査（`/cos/operations`）と同じ transaction に入る。task-api は SQL を書かず、その関数を呼ぶ。今の task-api の書き手は `task-api/src/cos/triage_view.rs`（cos_inbox_items）と `task-api/src/chat/attachments.rs`（chat_client_requests）で、`measure.py --check-writers cos_inbox_items,chat_client_requests` が exit 1 で出す。集約の後は exit 0 になる。
- migration・replay は変えない。port は宣言の置き場所だけを変え、SQL・schema・event の kind に触れない。

### D6. 指標
前後を `measure.py --json` で数え、`baseline.json`（この ADR の時点）と比べる。

| 指標 | 着手時 | 1 回目の目標 | 数え方 |
|---|---:|---:|---|
| TaskStore の fn 宣言 | 193 | 185 | B0 の command（`grep -c '^    fn '`） |
| celeris-mcp の src の `TaskStore` の言及 | 6 | 1 | `consumers.celeris-mcp.taskstore_mentions` |
| celeris-mcp が名指す store trait | McpCallStore, NodeSessionStore, ReportStore, TaskStore | TaskStore は `tools/tasks.rs` だけ | `consumers.celeris-mcp.store_traits_named` |
| mcp の file から見える method 数 | `TaskStore` を名指す file は 308 | `OrgDirectory` 6・`TaskQuery` 2・`McpClientStore` 7 | `store_traits` の fn_count の和 |
| 試験の fixture（SQLite を開く数） | 7 | 減らすかは mcp-port が判断 | `consumers.celeris-mcp.sqlite_store_opens_in_tests` |
| fake で試験するのに要る method 数 | 308（`dyn TaskStore`） | `resources` の一覧は 6（`OrgDirectory`） | 同上 |
| 8 method を変えたときに直す file | — | 64（`port-fix-files.json`） | cargo check の結果 |

- celeris-mcp と task-core は別の crate だが、celeris-mcp は task-core に依存し続ける。compile の単位の逆依存は減らない。減るのは、TaskStore の宣言を変えたときに celeris-mcp で読む・直す file（`TaskStore` を名指す file が 5 から 1）と、fake の試験で実装する method の数。

### D7. 削除の対象と順序
- 削除の対象は `prune-list.json` に書いた 17 名（呼び手の無い TaskStore method 4・その `_impl` 4・B0 の R3 割り当て 9）。検査は `measure.py --check-pruned prune-list.json`（残れば exit 1）。
- 試験だけが呼ぶ TaskStore method 14 個は、契約の廃止を書ける根拠を確かめていないので消さない（`prune-list.json` の `held_test_only` に理由を書いた）。
- 2 回目以降の consumer（task-ops・task-dispatch・task-api・celeris・celerisctl）は、1 回目の指標を見てから選ぶ。この ADR はそれを決めない。

## 結果
- celeris-mcp は TaskStore の 308 method の代わりに、使う能力の trait だけに依存する（`get` の 1 か所を除く）。
- 宣言は 1 か所、SQL は `_impl` に 1 か所のまま。state+event の原子性・migration・replay は変わらない。
