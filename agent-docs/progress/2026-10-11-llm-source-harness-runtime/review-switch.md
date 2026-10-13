---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: done
title: review-switch: reviewer run を select_route に切り替える
updated: 2026-10-11
---
# review-switch: reviewer run を select_route に切り替える

## 完了

- `review_spawn.rs` の `pick_reviewer` が provider 選択を使わず、`select_route` で reviewer run の経路を選ぶ。
  - harness 要求は `reviewer_harness_request`。role は `reviewer`、genre と repo の有無は対象 task から取る。task の `harness` 欄（worker run への人の指定）と browser 能力は使わない。
  - 選ばれた経路から `route_adapter`（harness adapter・model・OAuth account env）と `route_launch_target`（`SelectedLlmRoute`）を作る。
  - 選べない、または結合できない場合は `None` を返す（fail closed）。review は起動せず reviewing のまま次の tick を待つ。拒否は task ごとに 1 回だけ記録する（`route refused …` または `reviewer route cannot be launched: …`）。
- `ReviewerRun.llm_route` を追加し、reviewer の `RunRequest.llm_route` に `Some(..)` で渡す。
- `spawn_review` の記録:
  - `WorkerStarted`（role reviewer）: adapter id・選ばれた model・`provider` 欄に source_id・account。
  - `runs`: adapter・model・account。`run_index_start` の後に `record_route_selection` を呼ぶ（`harness_selected`・`llm_route_selected`・runs の selected_* 欄）。
- `ReviewEntry.provider` を `ReviewEntry.route: (SourceId, AccountId)` に置き換えた。
  - admission（`route_state`）が reviewer run の `(source, account)` を同時実行数に数える。
  - quota（`release_quota_if_tracked`・`resolve_quota_estimate`）・throttled event・snapshot の in_flight は source_id を使う。
  - reviewer の供給側失敗は provider policy に報告しない。OAuth source なら account 台帳、それ以外なら source の throttled 記録に向ける。
- reviewer の lane は `RouteInput.lane`（選択 metadata の `lane`）に載せるだけで、model 選択には使わない。reviewer の `RoutingDecided`（provider・tier 解決の記録）は出さない。
- `select_provider`・`select_provider_for` は lib から使われなくなった。provider_select.rs の削除は dispatch-prune の持ち物なので、`#[cfg(test)]` にとどめた（旧試験 cheap_local_first・routing_and_quota が呼んでいる）。dispatch-prune が試験と一緒に消す。
- 試験:
  - 新規 `tests/review_route.rs`:
    - `route_select_dispatch_review_runs_on_the_selected_harness_model_and_account`: reviewer の RunRequest.llm_route、WorkerStarted、events（harness_selected・llm_route_selected）、runs 行の選択欄を確かめる。
    - `route_select_dispatch_review_does_not_start_an_incompatible_reviewer`: harness と source が結合しない場合は起動せず、拒否を 1 回だけ記録する。
    - `route_select_dispatch_review_does_not_start_without_a_harness_adapter`: harness の adapter が無い場合は起動しない。
  - 共有の試験 dispatcher（`tests/mod.rs`）に `install_test_route` を足した（source `s1`・harness `h1`）。
  - 旧試験の書き直し:
    - reviewer の provider 期待値を `p1` から `s1` に変えた。
    - lane の 3 試験は `llm_route_selected.metadata.lane` を見る。
    - lead session の 3 試験は `IdAs`（claude-code id）の harness adapter を使う。
    - quota 試験は OAuth 経路を使う。
  - e2e `plan_scenarios` の設定に `[llm_sources]`・`[harnesses]`・`[harness_selection]`・`[route_defaults]` を足した（reviewer 条件の task が経路未宣言で待ち続けていたため）。

## 検証

- `cargo nextest run -p task-dispatch route_select_dispatch_review_` — 3 passed。
- `cargo nextest run -p task-dispatch --no-fail-fast` — 967 passed、0 failed。
- `cargo nextest run -p e2e --test plan_scenarios` — 2 passed。
- `cargo clippy --workspace --all-targets -- -D warnings` — exit 0。
- `bash scripts/dev/test-parallel.sh` — exit 100、5158 run: 5050 passed、108 failed、13 skipped。失敗はすべて browser・launcher・credentiald 系（`celeris-credentiald::{injection_ipc,broker,browser_doctor}`、`task-worker` と `task-api` の browser_*、`celeris browser_doctor`、`celerisctl commands::browser`）。原因は worker sandbox の SUN_LEN と userns で、harness-adapters の記録と同じ集合。この葉の変更箇所に失敗は無い。
- `sh "$CELERIS_WU_SCOPE_PATHS"` — 変更は task-dispatch と tests/e2e/tests/plan_scenarios.rs だけ。

## 未解決

- run-switch と同じ共有 file（`dispatcher.rs`・`route_select.rs`・`tests/mod.rs` の試験 dispatcher）に手を入れた。統合時に衝突したら両方を残す。試験 dispatcher の経路設定は `review_route::install_test_route` が 1 か所で持つ。
- 継続セッションの sticky 選択（Phase 67c）は reviewer では使わなくなった。`select_route` に sticky の入力が無いため。session は、選ばれた adapter id と account が前回と同じなら続く。
- 他の e2e 設定（scenarios・delegation など）は reviewer 条件を使わないので、`[llm_sources]` は足していない。config-remove・examples で新形に揃える。

## 本番への移行

- reviewer run は `[harnesses.*]`・`[llm_sources.*]`・`[harness_selection]`・`[route_defaults.*]` の宣言が無いと起動しない（review は reviewing のまま待つ）。
- reviewer 用の harness を分けたい場合は `[[harness_selection.rules]]` に `role = "reviewer"` の規則を書く。分けない場合は、role を問わない規則で選ばれる。
