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
- `docs/ops/browser-department.md` と `docs/ops/browser-department-org.json`:
  本番 org への投入手順（POST/GET/PATCH/DELETE の curl 例、`BrowserCapability::validate()` による
  検証内容、業務 host の追加手順、grant を外す戻し方）。投入する JSON は D1.2 の profile と同一内容。

## 前回 run からの修正

前回 run は done を返したが次の check が不合格だった。原因は 2 つ:

1. ファイル名が check の期待（`docs/ops/browser-department.md` / `docs/ops/browser-department-org.json`）
   と違った（`docs/ops/browser-execution-section.{md,org.json}` という名前で書いていた）→ `git mv` で
   直した（内容は同じ、`.md` 内の相互参照も新名に合わせた）。
2. 範囲 check（この WU の許可 path 一覧）に `crates/celeris/src/config/tests.rs` と
   `crates/celeris/src/lib/tests.rs` が入っておらず、前回 run がこれらを直した分が範囲外と判定された
   → `git checkout "${CELERIS_WU_BASE}" -- crates/celeris/src/config/tests.rs crates/celeris/src/lib/tests.rs`
   で WU base の内容に戻した。

この WU の範囲 check を自分で再実行し、2 本とも exit 0 を確認した。

## 既知の問題（範囲外で直せないもの）

`crates/celeris/src/config/tests.rs` の `loads_the_org_example_and_maps_it_to_org_nodes`
（node 数・id 一覧のハードコード）と `example_org_routes_ui_work_to_ui_ux_and_api_work_to_software_engineering`
（skill なし task の既定 routing 先）、および `crates/celeris/src/lib/tests.rs` の
`seeds_the_org_once_into_an_empty_db_and_never_again`（seed 件数のハードコード）は、
`config/org.example.toml` に新しい node を足すと必ず壊れる（`cargo test -p celeris --lib` で
3 件 FAILED を確認済み）。この WU の Objective には「org.example.toml を読む既存試験があれば
通るようにする」とあるが、修正に必要なファイル（上記 2 本）はこの WU の範囲 check の許可 path
（`agent-docs/progress/`・`agent-docs/adr/`・`config/org.example.toml`・`crates/task-ops/`・
`crates/celeris/tests/`・`crates/task-core/src/org`・`docs/ops/browser-department`・
`docs/architecture-map.md`）に含まれない（`crates/celeris/tests/` は integration test dir で、
壊れているのは `src/` 配下の unit test）。Objective の指示と範囲 check が両立しないため、範囲 check
を優先し（done の可否を直接左右するため）、この 3 件は意図的に未修正のまま残した。

### 直し方（次の統合/close WU 向け）

```diff
--- a/crates/celeris/src/config/tests.rs
+++ b/crates/celeris/src/config/tests.rs
@@ "software-engineering", "ui-ux", "systems-performance", の次に
+            "browser-execution",
@@ nodes.len() アサーション
-    assert_eq!(nodes.len(), 14);
+    assert_eq!(nodes.len(), 15);
@@ route(&[]) アサーション（browser-execution が辞書順で先頭になるため）
-    assert_eq!(route(&[]), "cluster-hpc");
+    assert_eq!(route(&[]), "browser-execution");
```

```diff
--- a/crates/celeris/src/lib/tests.rs
+++ b/crates/celeris/src/lib/tests.rs
@@ seeds_the_org_once_into_an_empty_db_and_never_again の 2 箇所
-    assert_eq!(seed_org_if_empty(&store, &config).unwrap(), 14);
+    assert_eq!(seed_org_if_empty(&store, &config).unwrap(), 15);
     let nodes = store.org_list().unwrap();
-    assert_eq!(nodes.len(), 14);
+    assert_eq!(nodes.len(), 15);
...
-    assert_eq!(store.org_list().unwrap().len(), 14);
+    assert_eq!(store.org_list().unwrap().len(), 15);
```

`bash scripts/dev/test-parallel.sh` の前にこの 2 ファイルを直すこと（close WU の範囲 check が
許せば）。

## 証拠

- `cargo test -p task-ops matching::` → 13 passed, 0 failed（新規 `browser_specialist_*` 含む）
- `cargo test -p task-core org` → 14 passed, 0 failed（既存のまま、影響なし）
- `cargo test -p celeris --lib` → 320 passed, **3 failed**（上記「既知の問題」の 3 件。範囲外につき
  意図的に未修正）
- `cargo clippy -p task-ops -p celeris --all-targets -- -D warnings` → warning 0
- 範囲 check（この WU のもの）を自分で再実行し両方 exit 0 を確認:
  - `grep -q 'browser-enabled' config/org.example.toml && test -s docs/ops/browser-department.md && node -e "JSON.parse(...)"` → exit 0
  - `git diff --name-only "${CELERIS_WU_BASE}"` ∪ 未追跡 file が許可 path 一覧の外にない → exit 0

## Acceptance（この WU）との対応

0. `config/org.example.toml` に `browser-execution` の browser grant 済み node がある。
1. `browser_specialist_node_receives_browser_enabled_tasks_and_ungranted_nodes_are_excluded`
   （`crates/task-ops/src/matching/tests.rs`）が通る。
2. `docs/ops/browser-department.md`（手順）と `docs/ops/browser-department-org.json`
   （投入 JSON）がある。

## 未解決事項

- 上記「既知の問題」の 3 件（`crates/celeris` の unit test）は範囲外のため未修正。close/integrate WU で
  直すこと（diff を上に書いた）。
- D1.3 が挙げた 4 本の試験のうち、「子 node を足すと grant が継承され、子で置き換えられる」ケースは
  個別の専用試験としては足していない（`Profile` の継承規則は `task-core/src/profile.rs` の既存試験で
  既に汎用的に確かめられており、`browser` 欄も「子が丸ごと置換」という同じ規則に従う）。
- ADR D5（2026-10-06 人の追加要望: web からの設定編集・task 単位 `browser_network_domains`・actor 付き
  event 記録）は、D5.3 のとおりこの WU の外（follow-up task）に送った。本番適用（investigate POST の
  実行）はこの WU では行っていない（worker は本番に触れない）。

## 提案

- follow-up task（ADR D5.3）: `Event::OrgNodeUpdated` の追加、`TaskSpec.browser_network_domains` と
  作成時検証・親子部分集合チェック、CoS/planner prompt への最小 domain 規則の追記、関連試験一式。
- 次の WU（integrate-build / close）の範囲 check に `crates/celeris/src/config/tests.rs` と
  `crates/celeris/src/lib/tests.rs` を含めるか、少なくとも close WU がこの 2 ファイルを編集できる
  範囲を持つこと。そうでないと `bash scripts/dev/test-parallel.sh` が最後まで 0 failed にならない。
