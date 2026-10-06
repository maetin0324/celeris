---
title: domain-policy — D2.0（task 毎の browser allowed_domains と browser 設定管理 API）の実装突き合わせ
tasks: [01M470CJRXMPWS39S14PN7XPFP]
status: done
updated: 2026-10-06
completed: 2026-10-06
---

# domain-policy — D2.0 の実装と試験の突き合わせ

WorkUnit `close`。[2026-10-05-browser-department-web-live-view](../../adr/2026-10-05-browser-department-web-live-view.md)
D2.0（2026-10-06 追記）の各要求と、実装・試験名（`browser_allowed_domains_` / `browser_settings_`）の対応表を書く。
各 WU の進捗は `domain-policy/` 配下（origin・task-req・core-fix・enforce・prompts・settings-api）。

## 要求 → 実装・試験の対応表

### (a) task 毎の allowed_domains（origin 形式の検証・欠落/空の作成拒否・親子包含・seed）

| 要求 | 実装 | 試験（名前） |
|---|---|---|
| origin 形式（scheme・host・port、wildcard は `*.` 接頭のみ）の検証 | `task_core::browser::AllowedOrigin::parse`（`crates/task-core/src/browser.rs`）。`*` 全体・public suffix wildcard・userinfo・path・query・fragment・不正 scheme を拒否 | `browser_allowed_domains_reject_invalid_origins`（task-core/src/browser/policy_tests.rs） |
| 包含判定（apex と既定 port を含め scheme・host・port で判定） | `origin_covers` / `normalize` | `browser_allowed_domains_containment_respects_apex_and_default_port` |
| 交差が文字列一致でない（scheme・host・port 込み、wildcard は包含） | `EffectiveBrowserPolicy::derive` を origin 対応に拡張 | `browser_allowed_domains_intersection_checks_scheme_host_and_port`、`browser_allowed_domains_derive_minimizes_overlapping_intersections` |
| browser-enabled の新規 task で欠落・空は作成拒否（全経路） | `Task.requirements.browser.allowed_domains` を正本に。`NewTaskSpec`・`PlanUnitSpec`・`ExecutionChildSpec`・`ConsoleAction::CreateTask`・`TaskStore` 保存時（`insert_tx`）で同じ決定的検証。grant との交差は作成時にしない | 作成経路 3 経路: (1) API `POST /tasks`（＋`PATCH /tasks/{id}` で browser skill 後付けも）→ `browser_allowed_domains_api_create_rejects_missing_empty_and_invalid`・`browser_allowed_domains_api_patch_cannot_add_skill_without_origins`（task-api/tests/operations.rs）、(2) CoS の起票 → `browser_allowed_domains_cos_create_rejects_missing_empty_and_invalid`（task-ops/src/actions/tests.rs）、(3) execution plan の子 task（plan unit・execution-plan/2 の children・旧 plan.json・旧 delegate・store 直入）→ `browser_allowed_domains_plan_child_requires_explicit_subset`（task-ops/src/tree/tests.rs）・`browser_allowed_domains_execution_plan_children_cannot_exceed_parent`（task-ops/src/delegate/tests.rs）・`browser_allowed_domains_store_rejects_invalid_children_without_events`（task-core/src/store/tests.rs） |
| 子 task の集合は親の部分集合（超えたら拒否） | `origin_covers` で scheme・host・port を判定。親が browser-enabled でない場合子は持てない | 同上 (3) の試験＋`browser_allowed_domains_store_accepts_child_with_narrower_origin`（親の wildcard に含まれる単一 origin の子は受ける） |
| grant の host 形式を origin へ移行（host への丸めで許可を広げない） | 読み込み時に HTTPS 既定 port 443 へだけ写す（旧 `example.com` → `https://example.com`。HTTP・他 port へは広がらない）。ADR: `2026-10-05-browser-allowed-origins.md` | 移行の照合は `browser_policy::url_origin_allowed`・`origin_host_port`（core-fix 葉。shim の `browser_cli.py` も origin 正規表現に揃えた） |
| seed を loopback の origin のみに修正し検証 | `config/org.example.toml` と `docs/ops/browser-department-org.json` を `["http://localhost:3000", "http://127.0.0.1:3000"]` に変更。両 seed を `include_str!` で読む検証試験 | `browser_allowed_domains_seed_grants_validate`（task-core/src/browser/policy_tests.rs） |

### (b) 実効許可 = task ∩ grant（grant 外は broker と egress の両方で拒否・grant 縮小は即時）

| 要求 | 実装 | 試験（名前） |
|---|---|---|
| 実効許可は task 集合 ∩ grant 集合 | `task_core::browser::task_run_policy`（保存 policy ∩ requirements）→ 既存 `EffectiveBrowserPolicy::derive`（∩ grant） | `browser_allowed_domains_intersection_is_task_and_grant`・`browser_allowed_domains_scheme_and_port_mismatch_leave_empty_intersection`（task-core/src/browser/policy_tests.rs） |
| grant 外を policy broker で拒否 | `task_worker::browser_policy::{prepare_for_task, admit}`（起動前判定、空なら process を起動せず失敗） | `browser_allowed_domains_broker_denies_outside_task_and_grant`・`browser_allowed_domains_empty_intersection_is_refused_before_run`（task-worker/src/browser_policy.rs） |
| grant 外を egress で拒否 | `PreparedBrowserPolicy::egress_allow`（CONNECT は scheme なしのため port〈443/80〉で scheme 判定） | `browser_allowed_domains_egress_allow_is_task_and_grant`（browser_policy.rs）・`browser_allowed_domains_egress_denies_outside_task_and_grant`（task-worker/src/browser_egress/tests.rs、CONNECT を DNS 前に拒否） |
| grant 縮小は既存 task にも即時効く | dispatcher が run ごとに org の現 grant を読み（`run_extras`）、`RunContext.browser_policy` に狭めた policy を入れる。実行中の run には流さず次の run から（ADR 2026-10-05-browser-allowed-origins.md「強制と grant 縮小の伝え方」） | `browser_allowed_domains_grant_shrink_applies_to_existing_task`（task-worker/src/browser_policy.rs）・`browser_allowed_domains_grant_shrink_reaches_next_run_context`（task-dispatch/src/dispatcher/tests/browser_allowed_domains.rs） |

### (d) browser 設定の管理 API（検証・失敗時無変更・actor 付き event・schema 再生成）

| 要求 | 実装 | 試験（名前） |
|---|---|---|
| 管理 API（`browser.allowed_domains`・harness・budget・credential/identity 対応を編集） | `PATCH /api/v1/org/{id}/browser-settings`（`crates/task-api/src/handlers/org.rs`）。`{id}` は node id（`browser-execution`）。body は `allowed_domains`・`credential_policy_ids`・`credential_identity_ids`・`harnesses`・`budget` の任意組合せ。既存の `require_admin` と変更系共通 guard（Origin・CSRF・Content-Type）を通す | `browser_settings_patch_updates_profile`（task-api/tests/organization.rs） |
| `*` 全体・public suffix wildcard・userinfo/path/query/fragment・不正 scheme を拒否 | `AllowedOrigin::parse` を通した上での reject。汎用 `PATCH /api/v1/org/{id}` の browser profile 変更も同じ検証 | `browser_settings_rejects_global_and_public_suffix_wildcards`・`browser_settings_rejects_userinfo_path_query_fragment`・`browser_settings_rejects_invalid_scheme`（ftp・file・非 loopback の http）・`browser_settings_rejects_empty_domains_and_unknown_credential_policy`・`browser_settings_generic_org_patch_uses_same_validation` |
| 失敗時は DB と event を変えない | 拒否系試験で org 行・監査 event・task events・org 行数が不変であることを比較 | 上の拒否系 5 件（各件で不変をアサート） |
| 成功時は actor 付きで events に追記（機密値は入れない） | org 行と `org_browser_events` を 1 transaction で書く。actor・変更前後の設定を記録し、監査値は credential policy ID と identity ID のみ | `browser_settings_event_records_actor_and_before_after`・`browser_settings_event_has_no_secret_values`・`browser_settings_requires_admin_and_same_origin` |
| API schema と web の生成型を再生成 | `UPDATE_SCHEMA=1` で `docs/api/v1/api-v1.schema.json`・`docs/protocol/worker-protocol.schema.json` を再生成し `web/api/generated`（types.ts に `BrowserSettingsPatch`）へ反映。`node web/scripts/gen-types.mjs --check` で整合 | `committed_schema_matches_generated`（task-api・task-worker）・`gen-types --check`（settings-api 葉で exit 0） |

### (e) planner と CoS の prompt・skill（最小 origin・親を超えない規則と例）

| 要求 | 実装 | 試験（名前） |
|---|---|---|
| 「最小の origin だけを渡す、親を超えない」規則と例（`["https://billing.example.com"]`、`*.example.com` や grant 全体のコピーをしない） | planner: `crates/task-worker/src/claude_code/prompt.rs` の `tree_plan_shape_section`（execution-plan/3）と `parallel_phases_section`（execution-plan/2）に共通の `BROWSER_ALLOWED_DOMAINS_GUIDANCE` を差し込み。CoS 起票: `crates/task-worker/src/preamble.rs::actions_instructions` の `create_task` 例に `requirements.browser.allowed_domains` と規則を足す | `browser_allowed_domains_prompt_tree_planner_has_minimal_origin_rule`・`browser_allowed_domains_prompt_v2_planner_has_minimal_origin_rule`（task-worker/src/claude_code/tests.rs）・`browser_allowed_domains_prompt_cos_create_task_has_minimal_origin_rule`（task-worker/src/preamble/tests.rs） |

### 試験の合計

- `browser_allowed_domains_`: 23 件（task-core 9〈policy_tests 7・store 2〉・task-ops 3〈tree・delegate・actions〉・task-api 2・task-worker 8〈broker/egress 5・prompt 3〉・task-dispatch 1）。
- `browser_settings_`: 9 件（task-api/tests/organization.rs。attempt 2 で 9 件に分割した）。
- 両者とも確定系（userns・実 browser 不要、時計も使わない）。

## 全体検査（2026-10-06、本 WU で実行）

- `bash scripts/dev/test-parallel.sh`: exit 0 — `CELERIS_TEST_SUMMARY {"passed": 3976, "failed": 0, "ignored": 13, "nextest_exit": 0, "doctest_exit": 0}`（binaries 127）。
- `cargo clippy --workspace -- -D warnings`: exit 0（warning 0）。
- 文書検査 3 本: `check-adr-numbers.sh`（142 files）・`check-doc-layout.sh scripts/dev/docs-layout.tsv`・`check-doc-links.sh` いずれも exit 0。
- 範囲 check: `git diff --name-only "${CELERIS_WU_BASE}"` の差分は本ファイルだけ（gui/ に差分なし）。

## web-ui・org-node 兄弟への申し送り

### 管理 API の経路

- 経路は **`PATCH /api/v1/org/{id}/browser-settings`**（`{id}` は node id の文字列で、browser 課は `browser-execution`）。
  既存の `PATCH /api/v1/org/{id}` は node id を固定した browser 専用経路と衝突するため、下位経路にした（ADR 2026-10-05-browser-allowed-origins.md「Browser settings API」）。
- body は `{"allowed_domains": [...], "credential_policy_ids": [...], "credential_identity_ids": [...], "harnesses": {"allowed": [...], "default": "..."}, "budget": {...}}` の任意組合せ（スキーマ `BrowserSettingsPatch`、`web/api/generated/types.ts` に再生成済み）。
- 権限: 既存の `require_admin`（管理権限）＋変更系共通 guard（Origin 完全一致・CSRF・`Content-Type: application/json`）。非 admin・他 origin は 401/403。
- 失敗時は org 行と event を一切変えない。成功時は `org_browser_events` に actor・変更前後の設定を追記（機密値は ID のみ）。
- web-ui はこの経路を使うこと（ADR D5.1-b）。generic `/api` relay 側での redact（`live_view_url` 除去）は既存のままでよい。

### origin 形式

- `allowed_domains` の各値は **origin の文字列**: `https://host[:port]`、または loopback（`localhost`・`127.0.0.1`・`[::1]`）に限り `http://host[:port]`。port 省略時は HTTPS=443・HTTP=80（正規形では既定 port を省く）。
- wildcard は DNS host の `*.` 接頭のみ（apex を含めない）。`*` 全体・public suffix wildcard・userinfo・`/` 以外の path・query・fragment・HTTPS/HTTP 以外の scheme は**すべて拒否**（422）。
- 初期値（seed）は `["http://localhost:3000", "http://127.0.0.1:3000"]` のみ。業務 host は人がこの API で origin 形式で追加する。
- 旧 host 形式（`example.com`）の読み替えは互換読み取りのみで `https://example.com`（既定 443）に写す（許可は広がらない）。web 画面が旧形式を再入力しても同じ規則で拒否される。
- web-ui は form に入力補助（scheme/host/port の分解、既定 port の表示）を置けるが、送る値は必ず上の正規形にすること。検証失敗の応答コードは固定（422、`{"code": ...}` 形式は task-api 共通）。

## 未解決事項（直さず残すもの）

- **egress は `host:port` の完全一致のまま**: wildcard origin（`*.example.com:443`）は egress では一致せず fail-closed に拒否される（core-fix・enforce 両葉の申し送り。以前からの挙動）。wildcard grant の task は egress 経由で接続できないため、実運用では wildcard ではなく単一 origin で指定するのが現状の安全側。
- **launcher 経路（`browser_launcher`）は prepared policy を受け取るだけで、本 phase では追加の origin 検証を足していない**（core-fix・enforce 両葉の申し送り）。
- **`crates/celeris` の seed 関連 unit test 3 件**（`loads_the_org_example_and_maps_it_to_org_nodes`・`example_org_routes_ui_work_to_ui_ux_and_api_work_to_software_engineering`・`seeds_the_org_once_into_an_empty_db_and_never_again`）は org-node 葉の範囲 check 外のため未修正。本 run の `test-parallel.sh` が 0 failed を出したため、統合 tree では既に直っているものとして扱う（この WU では確認していない）。
- **CoS 専用の外部 skill ファイルは無い**: `config/skills/` 配下に CoS 専用の skill が見つからず（指示は `preamble.rs` の Rust 文字列に直書き）、ADR (e) の「CoS の prompt・skill」は `preamble.rs` の 1 箇所で満たしたと判断した（prompts 葉の申し送り）。
- **`cargo test -p task-api` 全体をこの run では回していない**: settings-api 葉では sandbox の `browser_e2e`（実 process）が完了待ちになったため中断。実 browser の試験は sandbox 外の実行工程に委ねる（`crates/celeris/` 系・worker の browser 実 process 試験は sandbox では完走しない、既存の認識）。
- settings-api 葉は migration 番号を **0051** にした（全 refs で 0048〜0050 が使用済み）。この branch では 0048〜0050 を reserved とし、親 branch への統合時に実体の migration が入ったら予約を外す（settings-api.md の申し送り）。
