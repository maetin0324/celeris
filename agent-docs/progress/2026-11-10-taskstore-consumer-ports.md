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
- store-prune・table-writers: 済み（下の各節）。
- mcp-port: 済み。計画 v2 の形（1 行転送）では repo-health の `size.responsibility-growth` が落ちたので plan_issue で返し、計画 v3 の案 (a) で直した（下の「mcp-port の check の不合格と直し」）。
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

## store-prune の結果
詳細は [store-prune.md](2026-11-10-taskstore-consumer-ports/store-prune.md)。
- `prune-list.json` の 17 名を全部消した（TaskStore method 4・その `_impl` 4・B0 割り当て 9）。着手時の grep で呼び手が見つかった名前は無く、list から外した名前も無い。使い手が無くなった型 `ProjectPlanApply`・`ProjectPlanMilestoneChange`・`StoredRouteJudgment` も消した。消した試験は無い（17 名を呼ぶ試験が無かった）。
- `task_store.rs` の fn 宣言 193 → 189。store module の本体の行（size rule の `module_lines`）19676 → 19396、物理行 40947 → 40618、`SqliteStore` の method 729 → 713。
- `measure.py --check-pruned …/prune-list.json` → exit 0（`remaining=0`）、`--check-single-decl` → exit 0（`fns=304`）。

## mcp-port の結果（計画 v2）
- 移した 6 method: `OrgDirectory`（`crates/task-core/src/store/org_directory.rs`）へ `org_get`・`project_list`・`project_get`・`milestone_list`、`TaskQuery`（`crates/task-core/src/store/task_query.rs`）へ `list_page`・`event_rows_for`。どちらも `TaskStore` の supertrait。宣言は `task_store.rs` から消した（写していない）。SQL の本体は計画 v3 で各 trait の file の `impl … for SqliteStore` へ移した（下の「mcp-port の check の不合格と直し」）。SQL・transaction・migration・event は変えていない。
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
- 振る舞いは変えていない。6 method はどれも読み出しだけ（`with_read_conn`）で、state+event を同時に書く操作は今までどおり 1 つの store method の 1 transaction の中にある。port の trait は method の境界を変えない（ADR D5）。移した本体は既存の試験がそのまま確かめる（`store/tests.rs` の `list_page_` で始まる 6 件と `event_rows_for_returns_one_tasks_rows_with_ids_after_seq`、celeris-mcp・task-api の試験）。test-affected（下の証拠）が既存の transaction・replay・migration の試験を含めて合格した。

## 証拠コマンドと結果（mcp-port、提出時の HEAD で実行）
- `grep -rlw TaskStore crates/celeris-mcp/src` → `resources.rs`・`tools/org.rs`・`tools/tasks.rs` の 3 file
- `grep -nE 'fn (org_get|project_list|project_get|milestone_list|list_page|event_rows_for)\b' crates/task-core/src/store/task_store.rs` → 0 行
- `python3 agent-docs/progress/2026-11-10-taskstore-consumer-ports/measure.py --check-single-decl` → exit 0（trait 19、fn 304、重複 0）
- `git diff --name-only $CELERIS_WU_BASE | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R3 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → exit 0（`write-set R3: ok`）
- `grep -rnE 'fn (org_get|project_get|project_list|milestone_list|list_page|event_rows_for)_impl' crates/task-core/src` → 0 行
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 8179 passed・23 skipped、web の vitest 712 passed、tsc -b 合格、ts-verify 20 passed）。実行は commit「mcp-port: 6 method の本体を impl OrgDirectory・impl TaskQuery for SqliteStore へ移し…」の時点で、その後の commit は agent-docs だけを変える
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/check-health.sh` → exit 0（base は auto の 217bc325。verdict pass、fail 級 0、new 0、worsened 1 は下の未解決）
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"` → exit 0（verdict pass、fail 級 0、new 0、worsened 0。store の本体の行 19371）

## mcp-port の check の不合格と直し
- 計画 v2 の run の後に celeris が走らせた WU の check は 2 つ落ちた。
  - 範囲 check: `agent-docs/adr/2026-11-10-taskstore-consumer-ports.md` が範囲の外だった。ADR に足した付記を消し（WU base の版に戻した）、内容は下の「ADR の付記の案」としてこの進捗に移した。
  - repo-health の `size.responsibility-growth`（error）: `crates/task-core/src/store` の本体の行（空行・注釈・use・mod 宣言を除く）が WU base（store-prune の後）の 19396 から 19406 へ 10 行増えた。増えたのは新しい trait 2 つの構造の行（trait の見出しと閉じ括弧 4 行、impl の見出しと閉じ括弧 4 行、`TaskStore` の supertrait 2 行）で、1 行転送の形のままでは trait 1 つにつき 5 行は必ず増える。plan_issue で返した。
- 計画 v3（案 (a)）で直した。6 method の本体を各 trait の file の `impl OrgDirectory for SqliteStore`（`store/org_directory.rs`）・`impl TaskQuery for SqliteStore`（`store/task_query.rs`）へ写さずに移し、領域の file の `_impl` と 1 行の転送を消した。前例は `store/model_catalog.rs` の `impl ModelCatalogStore for SqliteStore`。
  - 置き場所: 各 trait の method は複数の領域の file に分かれていた（`OrgDirectory` は `org.rs` と `projects.rs`、`TaskQuery` は `tasks.rs` と `events.rs`）。1 つの trait の impl を 1 か所に置くので、既にある impl block を使える trait の file を選んだ。領域の file に置くと impl の見出しと閉じ括弧が増えるので、trait の file の方が行が少ない。
  - 消した `_impl`（6 個）: `org.rs` の `org_get_impl`、`projects.rs` の `project_get_impl`・`project_list_impl`・`milestone_list_impl`、`tasks.rs` の `list_page_impl`、`events.rs` の `event_rows_for_impl`。どれも 1 行の転送からしか呼ばれていなかった。
  - 可視性を変えた helper は `events.rs` の `read_event_rows`（private → `pub(super)`）の 1 つだけ。`org_row`・`project_row`・`milestone_row`・`row_to_task` は既に `pub(super)`、`query.rs` の cursor・絞り込みの部品は既に `pub(crate)` だった。helper は写していない。`tasks.rs` から使わなくなった import を消した。
  - SQL の文字列と本体の処理は 1 文字も変えていない（`git diff` で移動だけ）。
- store の行の前後（`check-health.sh` の report の `size.responsibility-growth`）:

| 時点 | store の本体の行（`crates/task-core/src/store`） | `SqliteStore` の method |
|---|---:|---:|
| 着手時（ports-design の base） | 19676 | 729 |
| WU base（store-prune の後） | 19396 | 713 |
| 宣言の移動の後（計画 v2、1 行転送） | 19406 | 713 |
| HEAD（計画 v3、本体を trait の impl へ） | 19371 | 707 |

## ADR の付記の案（record で ADR に写す）
- D2 の 8 method のうち `org_list`・`org_upsert` は `TaskStore` に残した。`get` も D2 のとおり残した。移したのは 6 method で、`OrgDirectory`（`crates/task-core/src/store/org_directory.rs`）は `org_get`・`project_list`・`project_get`・`milestone_list` の 4 個、`TaskQuery`（`crates/task-core/src/store/task_query.rs`）は `list_page`・`event_rows_for` の 2 個を持つ。
- 6 method の本体は `impl OrgDirectory for SqliteStore`・`impl TaskQuery for SqliteStore` に直接置き、D2 の「`<method>_impl` への 1 行転送」はやめた。新しい trait 2 つの見出しと impl の行だけで store の本体の行が WU base より 10 行増え、`size.responsibility-growth` が落ちたため。前例は `store/model_catalog.rs`。消した `_impl` は 6 個で、store の本体の行は WU base の 19396 から 19371 になった。
- 残した理由は提案節の「残した 3 method」。celeris-mcp の src で `TaskStore` を名指す file は `resources.rs`（`org_list`）・`tools/org.rs`（`org_list`・`org_upsert`）・`tools/tasks.rs`（`get`）の 3 つになった。D4 の 64 file の見込みは 8 method の場合で、6 method では import を足した file が 38（うち celeris-mcp 5）だった。

## 変更（commit の件名）
- `ports-design: TaskStore の consumer 計測台本・着手時の計測・削除の対象・port の ADR`
- `mcp-port: org_get・project_list・project_get・milestone_list・list_page・event_rows_for の宣言を store/ の OrgDirectory・TaskQuery へ移し、celeris-mcp を狭い trait に依存させる`
- `mcp-port: ADR の付記と進捗に port の結果を書く`
- `mcp-port: ADR の付記を進捗へ移し、repo-health の不合格と plan_issue を書く`
- `mcp-port: 6 method の本体を impl OrgDirectory・impl TaskQuery for SqliteStore へ移し、_impl と 1 行の転送を消す`
- `mcp-port: 進捗に本体の移動と store の行の前後を書く`

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
