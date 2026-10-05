---
title: org-node — ブラウザ実行課の org 定義・matching 試験・投入 JSON と手順
tasks: [01M46W97H391DSFW1XJ745W0G9]
status: done
updated: 2026-10-05
completed: 2026-10-05
---

# org-node — ブラウザ実行課の org 定義・matching 試験・投入 JSON と手順

WorkUnit `org-node`。[2026-10-05-browser-department-web-live-view](../../adr/2026-10-05-browser-department-web-live-view.md)
D1（部署）・D1.3（適用と試験）に従って実装した。

## やったこと

- `config/org.example.toml`: `[[org]] id = "browser-execution"`（ブラウザ実行課、section、親 `engineering`、
  genre `coding`）を足した。`profile.browser`（`allowed_domains = ["localhost", "127.0.0.1"]`、
  `credential_policy_ids = []`）、`profile.harnesses`（`allowed = ["coding"]`、`default = "coding"`）、
  `profile.budget`（`max_lane = "standard"`、`max_attempts = 2`）、D1.2 の 3 行の policy を含む。
- `crates/task-ops/src/matching/tests.rs`: `browser_specialist_node_receives_browser_enabled_tasks_and_ungranted_nodes_are_excluded`
  を足した。grant が無い間は `browser-enabled` task が `Unroutable`、grant を足すと `browser-execution` に
  割り当たり、同じ coding harness を持つ `software-engineering` は通常 task のまま変わらず、grant を外すと
  再び `Unroutable` に戻ることを確かめる。既存の `browser_matching_requires_administrator_capability_grant`
  （grant の無効化で候補から外れる挙動）はそのまま残している。
- `crates/celeris/src/config/tests.rs`: `browser_specialist_org_node_receives_browser_enabled_tasks_from_the_example_org`
  を足した。seed の `org.example.toml` をそのまま `Config::load` → `validate()` → `org_nodes()` に通し、
  `browser-execution` の grant（`parent_id`・`genre`・`allowed_domains`）を確認したうえで、`decide()` で
  `browser-enabled` task がこの node に割り当たること、他の node には grant が付いていないことを確かめる。
  既存の `loads_the_org_example_and_maps_it_to_org_nodes`（node 数・id 一覧）と
  `example_org_routes_ui_work_to_ui_ux_and_api_work_to_software_engineering`
  （skill なし task の既定 routing 先）を新しい node 数・id 構成に合わせて更新した。
- `crates/celeris/src/lib/tests.rs`: `seeds_the_org_once_into_an_empty_db_and_never_again` の
  node 数アサーションを 2 箇所とも 14→15 に直した（1 箇所目は前 run で直っていたが、同じ関数内の
  2 箇所目 `store.org_list().unwrap().len()` が未更新で落ちていた。今回修正）。
- `docs/ops/browser-execution-section.md` と `docs/ops/browser-execution-section.org.json`:
  本番 org への投入手順（POST/GET/PATCH/DELETE の curl 例、`BrowserCapability::validate()` による
  検証内容、業務 host の追加手順、grant を外す戻し方）。投入する JSON は D1.2 の profile と同一内容。

## 証拠

- `cargo test -p task-ops matching::` → 13 passed（新規 `browser_specialist_*` 含む）
- `cargo test -p celeris --lib config::` → 114 passed（新規 `browser_specialist_org_node_*` 含む）
- `cargo test -p celeris --lib tests::` → 324 passed（`seeds_the_org_once_into_an_empty_db_and_never_again` 含む）
- `bash scripts/dev/test-parallel.sh` → nextest 3944 passed / 0 failed / 13 ignored、doctest 0 failed
- `cargo clippy --workspace -- -D warnings` → warning 0
- 範囲 check: `git diff --stat "${CELERIS_WU_BASE:-HEAD}"` は ADR・`config/org.example.toml`・
  `crates/celeris/src/config/tests.rs`・`crates/celeris/src/lib/tests.rs`・
  `crates/task-ops/src/matching/tests.rs`・`docs/ops/browser-execution-section.{md,org.json}` の 7 file のみ

## Acceptance（この WU）との対応

0. `config/org.example.toml` に `browser-execution` の browser grant 済み node がある。
1. `browser_specialist_node_receives_browser_enabled_tasks_and_ungranted_nodes_are_excluded`
   （task-ops）と `browser_specialist_org_node_receives_browser_enabled_tasks_from_the_example_org`
   （celeris、seed 経由）が通る。
2. `docs/ops/browser-execution-section.md`（手順）と `docs/ops/browser-execution-section.org.json`
   （投入 JSON）がある。

## 未解決事項

- D1.3 が挙げた 4 本の試験のうち、「子 node を足すと grant が継承され、子で置き換えられる」ケースは
  個別の専用試験としては足していない（`Profile` の継承規則は `task-core/src/profile.rs` の既存試験で
  既に汎用的に確かめられており、`browser` 欄も「子が丸ごと置換」という同じ規則に従う。追加するなら
  `task-core::profile::tests` 側が適切で、`task-ops`/`celeris` の matching 試験の範囲ではない）。
- ADR D5（2026-10-06 人の追加要望: web からの設定編集・task 単位 `browser_network_domains`・actor 付き
  event 記録）は、D5.3 のとおりこの WU の外（follow-up task）に送った。本番適用（investigate POST の
  実行）はこの WU では行っていない（worker は本番に触れない）。

## 提案

- follow-up task（ADR D5.3）: `Event::OrgNodeUpdated` の追加、`TaskSpec.browser_network_domains` と
  作成時検証・親子部分集合チェック、CoS/planner prompt への最小 domain 規則の追記、関連試験一式。
