---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: done
title: run-switch: 通常 run を llm_route で起動・記録
updated: 2026-10-11
---
# run-switch: 通常 run を llm_route で起動・記録

## 完了

- 通常 dispatcher run は `select_route` で route を選び、`route_adapter` と `route_launch_target` から adapter・model・account と `SelectedLlmRoute` を組み立てる。route または adapter が使えない場合は refusal を記録して dispatcher を抜け、worker を起動しない。
- `RunRequest.extras.llm_route` に選択 route を渡し、`RunEntry.route` に `(source_id, account_id)` を記録する。`runs` の開始行の記録後に `record_route_selection` を呼ぶ。provider 欄が必要な既存 audit / run 表示には source id を使う。
- provider 選択・adapter 解決・tier からの model 解決に依存した通常 run 経路と、その前提で成立しなくなった既存試験を整理した。
- `route_select_dispatch_run_` 試験で選択 harness/model/account の起動入力と DB/event 記録、候補不足・adapter 不在時の fail-closed を確認する。

## 検証

- `cargo test -p task-dispatch route_select_dispatch_run_` — exit 0、2 passed。
- `cargo test -p task-dispatch` — exit 0、902 unit tests、11 integration tests、4 unified-kill tests passed。途中にテスト自身が出す merge conflict と user namespace 不可の診断ログがあったが、該当試験を含め全件 pass。
- `cargo clippy --workspace -- -D warnings` — exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` — exit 0、変更 path はこの WorkUnit の dispatcher・試験範囲内。
- `git diff --check` — exit 0。
- `bash scripts/dev/test-parallel.sh` はこの WorkUnit では未実行。

## 未解決

- なし。

## 本番への移行

- 実行時の harness と source/model/account は `llm_route` の registry と admission が選ぶ。選択不能または harness adapter が未登録なら通常 run は defer されるため、本番 config では利用する harness/source/model/account と harness adapter 宣言を揃えること。
