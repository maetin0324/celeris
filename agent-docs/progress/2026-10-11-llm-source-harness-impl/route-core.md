---
tasks: [01M4MAYXS5C7438RAB6TRZK60W]
status: done
title: task-core llm_route 選択境界
updated: 2026-10-11
---

# route-core

## 実装

`crates/task-core/src/llm_route.rs` を追加し、`task_core::llm_route` として公開した。既存コードは `lib.rs` の module 宣言以外変更していない。

- source/model/harness/API format/execution class/account の ID と Registry 宣言型を追加した。Registry は `llm_routes`、harness ごとの `route_defaults`、CoS の `cos_route_candidates` を保持する。
- `select_harness` は task の明示 harness、genre/role 等の最適合規則、execution class 既定の順に決める。未登録 ID・同じ優先度の複数候補・該当なしをエラーにし、task 文面や LLM は参照しない。
- `RouteRequest` に lane を保持しつつ、`enumerate_candidates` は lane を参照せず宣言 source×model を列挙し、model hint があれば絞る。
- `constrain_for_harness` は source kind と API format または明示 route を照合し、非適合候補と理由を返す。
- `admit_source` は account/source の snapshot quota、cooldown、同時実行数から予約 owner `(source_id, account_id)` または拒否理由を返す。
- `RoutePolicy` と `default_route_policy` は宣言優先順の先頭適格候補だけを選び、空や未定義順序では fail closed にする。CoS も別途宣言された候補列を同関数で選べる。

## 検証

- `cargo test -p task-core llm_route::tests --lib`: 5 passed（選択決定性・同段重複・model hint・lane 非入力・互換性・quota/cooldown/source/account 同時実行・既定なし・CoS fallback）。
- `cargo test -p task-core --lib`: 905 passed, 0 failed。
- 前回 run 後の check で `cargo clippy -p task-core --all-targets -- -D warnings` が `cloned_ref_to_slice_refs` を報告したため、試験の `&[c.clone()]` を `std::slice::from_ref(&c)` に修正した。再実行した `cargo test -p task-core --lib && cargo clippy -p task-core --all-targets -- -D warnings` は exit 0（905 passed、clippy clean）。
- `git diff --check`: exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"`: `agent-docs/progress/2026-10-11-llm-source-harness-impl/route-core.md`、`crates/task-core/src/lib.rs`、`crates/task-core/src/llm_route.rs` のみ。

## 境界

task schema への harness 欄追加と config 宣言 loader は別 unit の担当。本 module は `HarnessRequest` を境界入力として提供し、既存 `Task` や provider 選択の挙動を変更しない。
