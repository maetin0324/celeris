---
title: 健全性v2 R5 map-api-b — task-api tests d〜r の試験関数に契約を付け、組を判断する
tasks: [01M7388T31FEX7W469M2TJS236]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 R5 map-api-b — task-api tests d〜r の試験関数に契約を付け、組を判断する

完了日: 2026-11-10

WorkUnit `map-api-b`。fragment `api-b`（scope `^crates/task-api/tests/[d-r]`）の commit 済みの雛形 [`mapping/api-b.json`](mapping/api-b.json) を、[`fill-guide.md`](fill-guide.md) の手順どおりに埋めた。`--scaffold` は走らせていない。`scan` 欄・試験 id・組の members は変えていない。試験（crates/ の下）・`contracts.py`・ほかの fragment の `mapping/*.json` は変えていない。基準は `$CELERIS_WU_BASE`。

## 結論
- scope の 44 file・320 件の試験の全部に `capability` と契約（1 行）を付けた。`--check-fragment` は exit 0。
- 320 件のうち 316 件は試験ごとの上書き（`fns`）で個別の契約を書いた。上書きの無い 4 件は試験が 1 件だけの file（`legacy_routing_events`・`project_auto_integrate`・`request_lock_counts`・`routing_stage`）で、file の契約がその試験の契約。
- 不変条件の試験（`invariant=true`）は 156 件。どの組の members にも入っていない（api-b の組は全部 `duplicate_setup` で、members は helper）。
- api-b が持つ組は 8 件（全部 `duplicate_setup`、`equivalent_scenario` は 0 件）。採用 6 件・不採用 2 件。
- `legacy_routing_events.rs` は DB に残る旧い shape の event を読む replay の契約として扱い、契約にそう書いた（候補にしていない）。
- `request_lock_counts.rs` は lock 数の回帰固定（不変条件の試験）で、組にも候補にも入っていない。

## capability ごとの試験の数

| capability | 試験 | 出所 |
|---|---|---|
| `project-controller` | 49 | B0 |
| `task-lifecycle` | 26 | B0 |
| `notifications` | 22 | B0 |
| `decisions-plan-gate` | 18 | B0 |
| `execution-plan-adoption` | 18 | B0 |
| `org-projects-reports` | 18 | extra |
| `accounts-quota` | 17 | B0 |
| `task-read-views` | 14 | extra |
| `project-docs` | 11 | extra |
| `browser-policy-broker-approval` | 11 | B0 |
| `project-repos-workspace` | 11 | extra |
| `task-files-artifacts` | 10 | extra |
| `task-genres-roles` | 10 | extra |
| `http-api-auth-principals` | 9 | B0 |
| `knowledge-base` | 9 | extra |
| `celeris-mcp` | 8 | B0 |
| `daemon-snapshot-config-api` | 7 | extra |
| `routing-eval` | 7 | B0 |
| `cos-chat-threads-runs` | 6 | B0 |
| `delivery-merge-candidate` | 6 | B0 |
| `routing-audit-outcome` | 4 | B0 |
| `routing-lane-route` | 4 | B0 |
| `llm-proxy` | 4 | B0 |
| `local-machine-settings` | 4 | extra |
| `worker-memory-read` | 4 | extra |
| `model-catalog` | 4 | extra |
| `recursive-task-tree` | 3 | B0 |
| `pre-review-sync-auto-resolve-integration-repair` | 3 | B0 |
| `workunit-scheduling-stage-integration` | 2 | B0 |
| `budget-pricing` | 1 | B0 |
| 計 | 320 | |

file の値と違う capability を試験ごとに付けた主なもの:
- `execution.rs` の plan gate・phase gate の 5 件は `decisions-plan-gate`、decompose の 2 件は `recursive-task-tree`、retry の 1 件は `task-lifecycle`。
- `operations.rs` と `organization.rs` の browser の allowed_domains・browser 設定の 11 件は `browser-policy-broker-approval`、bearer 必須の 4 件と `reports.rs` の 1 件は `http-api-auth-principals`。
- `project_plan.rs` は routing の指定 2 件が `routing-lane-route`、`is_root_task` が `recursive-task-tree`、subtree の停止が `task-lifecycle`。
- `files.rs` の WU の check-log の 2 件は `workunit-scheduling-stage-integration`。

## extra_capabilities（B0 に無い capability、11 件）
- `daemon-snapshot-config-api` — daemon の snapshot・watch stream・`GET /config`・`/clusters`・`/schema` の読み取り API（`handlers/system.rs`・`handlers/clusters.rs`・`schema.rs`・`sse.rs`）
- `project-docs` — 案件の文書 repo の page・成果物の昇格・文書整理（`task-api/src/docs.rs`・`task-ops/src/docs.rs`）
- `task-files-artifacts` — run の log と成果物の file API（`task-api/src/files.rs`）
- `task-genres-roles` — task の genre・role の検証と既定（`celeris/src/config/harness.rs`・`handlers/tasks.rs`・`task-core/src/model.rs`）
- `knowledge-base` — 知識ベースの page・tree・候補（`task-api`・`task-ops`・`task-core` の `knowledge.rs`）
- `task-read-views` — task の一覧・詳細・events・graph の view（`task-ops/src/view.rs`・`task-api/src/query.rs`）
- `local-machine-settings` — ローカルマシン設定（`task-core/src/local_machine.rs`・`store/local_machine.rs`・`handlers/clusters.rs`）
- `worker-memory-read` — 組織 node の記憶 file の読み取り（`task-api/src/memory.rs`・`task-ops/src/memory.rs`）
- `model-catalog` — model catalog の発見・上書き（`task-core/src/model_catalog.rs`・`store/model_catalog.rs`・`task-api/src/model_catalog.rs`）
- `org-projects-reports` — 組織・案件・途中目標・報告の API（`task-core/src/org.rs`・`store/{org,projects}.rs`・`report.rs`・`handlers/{org,projects}.rs`・`task-api/src/reports.rs`）
- `project-repos-workspace` — 案件の repo と作業場所（`task-core/src/repos.rs`・`store/repos.rs`・`task-api/src/repos.rs`）

ほかの fragment（`api-c` の `skills.rs`、`cli` の celerisctl など）で同じ領域の試験があれば、この id を使える（`--check-fragment` は `mapping/*.json` の全部を読む）。

## 組の判断（8 件。採用 6・不採用 2）
組は helper の名前と file で書く（組の id は [`mapping/api-b.json`](mapping/api-b.json) の `groups` にある）。

採用（6 件）:

| helper（file 数） | action | 案 | mutation の対象 |
|---|---|---|---|
| `env`（docs・execution・knowledge・repos・skills の 5） | delete | 本体が `common::admin_env()` と同じ。局所の `env()` を消し `admin_env()` を呼ぶ | `crates/task-api/src/middleware.rs` |
| `pa`（organization・project_plan・repos の 3） | delete | `common::patch_admin()` と同じ要求を作る。局所の `pa()` を消し `patch_admin()` を呼ぶ | `crates/task-api/src/handlers/projects.rs` |
| `pu`（docs・knowledge・skills の 3） | merge | common に `put_admin(path, body)` を足し、局所の `pu()` を消す | `crates/task-api/src/knowledge.rs` |
| `bearer_for`（project_brief・project_controller・project_roadmap の 3） | merge | `common/cos_ops.rs` の `cos_bearer` の後半と同じ 25 行。thread id を受ける helper を切り出して呼ぶ | `crates/task-api/src/cos/mod.rs` |
| `run_bearer`（general_cos_consult・single_human_window の 2） | merge | 上の helper に policy の版を引数で渡す bearer 発行を置き、写しを消す | `crates/task-api/src/cos/mod.rs` |
| `seed_secretary`（memory・project_plan の 2） | merge | 同じ秘書の組織 node を作る fixture。`post_admin()` で書き直して common に置く | `crates/task-api/src/handlers/org.rs` |

不採用（2 件）:
- `env`（general_cos_consult・project_cos_guard の 2）: 本体は同じだが、中の `WEB_TOKEN` は file ごとに値の違う定数を指す。common へ移すには token を引数にする必要があり、2 file では build も読みやすさも変わらない。
- `rfc3339`（general_cos_consult・project_wake_human_chat・relayed_instructions・single_human_window・unblocks の 5）: time の Rfc3339 書式を呼ぶだけの 2 行の変換で、fixture を持たない。

各組の `keep_tests` は users から file ごとに 1 件ずつ選んだ。整理は helper の置き換えなので、users の試験はどれも消さない。

## 確かめた結果（提出時の HEAD で実行）
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/contracts.py --check-fragment agent-docs/progress/2026-11-10-health-v2-r5/mapping/api-b.json` → exit 0（`ok`）
- `git diff --name-only "$CELERIS_WU_BASE"` → `agent-docs/progress/2026-11-10-health-v2-r5/mapping/api-b.json` と この進捗の 2 file だけ（crates/ と試験の変更なし）
- `git diff --name-only "$CELERIS_WU_BASE" | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → exit 0
- `sh scripts/dev/progress-index.sh --check` → exit 0
- cargo の検査（test-affected・clippy・fmt）は走らせていない。この WorkUnit は crates/ を変えていない。

## 変更
- `wu/map-api-b: mapping/api-b.json に 320 件の契約と 8 組の判断を書く` — 上のとおり。

## 未解決
- 採用した組のうち 3 件は api-b の scope の外の file を含む（`env` の `skills.rs`、`pu` の `skills.rs`、`run_bearer` の `single_human_window.rs`。`rfc3339` は不採用）。組の持ち主は先頭の member の path で決まるので api-b が判断した。実際の整理（dedupe 段）ではこれらの file も変える。
- `memory.rs`・`project_plan.rs` の `p()` も `common::post_admin()` と同じ写しだが、その組は先頭の member が `approvals.rs` なので api-a が判断する。

## 提案
- `env`・`pa`・`p` のように common に同じ本体の helper が既にある写しは、dedupe 段で最初に消すと mutation の確認が少なくて済む（要求の形が変わらない）。
