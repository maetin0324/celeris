---
tasks: [01M4Q53EHAF7BS9V3QMM1WY352]
---
# merge-main: 先行 task（provider 廃止）を含む main の取り込み

## 状況

- task branch 開始 HEAD: `c883b7c77d0f7396fc4aba672225b18b2056273c`
- 取り込み対象 main: `101aced564aac5b2ee023a435ae81a82df582794`
- `git merge --no-ff --no-edit main` を実行し、merge を完了。
- `crates/task-dispatch/src/dispatcher/dispatch_run.rs`: main の RoutePolicy 選択、pick、HarnessSelected/LlmRouteSelected、lane RoutingDecided の流れを維持。worker subject と warn→None 継続を含む `record_route_features` の参照を lane decision 内に加え、RoutingRecord.features にだけ結び付けた。新 event/decision は追加していない。
- `review_spawn.rs` と main で削除された `cheap_local_first.rs` / `routing_and_quota.rs` は main 側を採用。
- 機械的な store/API/tests/docs/web 衝突は main の内容を保持し、task branch の `record_route_features` store method を main の TaskStore 実装へ併合。
- migration 競合: main の 67/70/71/72 を保持し、0069_route_records を 0073_route_records に振り直した。SCHEMA_VERSION は 73。

## 検査

- `git merge-base --is-ancestor main HEAD`: exit 0
- `rg -n '^(<<<<<<<|=======|>>>>>>>)' --glob '!target/**' crates agent-docs docs web`: 衝突 marker なし
- `git diff --check HEAD`: exit 0
- `sh "$CELERIS_WU_SCOPE_PATHS"`: exit 0
- `cargo check --workspace --all-targets`: exit 0（当初は main の routing_replay module file が merge tree に無く、restore 後に RoutingRecord.features の fixture 2 箇所を補って再実行）
- 前回 run 後 check の修復: `route_store_schema_is_pinned_and_has_all_tables` が旧 SCHEMA_VERSION `69` を pin していた。main の migration 追加に合わせて `73` に更新。
- `cargo test -p task-core --lib route_ && cargo test -p task-dispatch --lib route_select`: exit 0（task-core 43 件、task-dispatch 19 件）
- 残課題: なし
