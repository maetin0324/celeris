---
task: 01M54HYGQNYDP2MGXXS7AY7164
wu: dispatch
status: done
completed: 2026-10-17
---
# dispatch: route 選択の evidence_mode

- `RouteContext.evidence_mode`（none/pinned/latest）を `dispatch_run` が task 欄の routing_override から渡す。
- `fill_route_stage`: none は `latest_routing_evidence` も `routing_evidence_at` も呼ばない。pinned は指定版のみ（版なしなら最新版へ落とさず読まない）。latest は従来。
- 記録: route_decision に `evidence_mode` と、none では `evidence_table:"none"`（evidence_version は null）。proposal の policy_version は `evidence=none(mode=none)`。
- `SqliteStore::evidence_lookup_count()`（試験用観測値）を追加。
- 試験: `route_select::evidence_mode::none_never_reads_the_table_and_ignores_later_updates`（lookup 数不変・v2 差替えでも none 不変・pinned=v1・latest=v2）、`lane_assist::task_override_evidence_mode_none_is_recorded_without_a_table_version`（通常 dispatch、lookup 0）。
- 検査: test-parallel 5622 passed/0 failed、clippy・fmt exit 0。

## 未解決・提案
- `routing_guardrail.rs` の `latest_routing_evidence` は genre×harness の stage 昇降格（運用側の判定）で、task 単位の route 選択ではないため変更しない（latest 前提のまま）。
- API/campaign/e2e は他 WU の担当。
