---
tasks: [01M4MAZNNYT4JVRYKCVZ1TF1VF]
---
# API and CLI stranded integration surface

## Status

解消済み（2026-10-11）。前回の「未完了」の理由だった view 型・API route・daemon 配線・CLI・schema/生成型は、次の WorkUnit の記録で実装されている。

- view（TaskSummary/TaskDetail の `integrations`・`promotion`、受信箱 `Unintegrated`）: [views](views.md)
- API route（`GET /api/v1/integrations`、`GET /api/v1/tasks/{id}/integrations`、`POST …/{repo}/{merge|discard|hold|reopen}`）: [routes](routes.md)、[routes-wire](routes-wire.md)（CoS 登録・daemon の `releases_dir` 配線・summary/detail の取り込み状態配線）
- celerisctl（`integration list/merge/discard/hold/reopen`）: [cli](cli.md)
- daemon 配線込みの結合試験（取り残し→一覧・詳細→route 操作→events）: [e2e](e2e.md)
- 突き合わせと残りの差分: [ADR 2026-10-11-stranded-done-branches の付記](../../adr/2026-10-11-stranded-done-branches.md)、[close](close.md)

前回の「API handler の操作に必要な repository path/default branch/release directory の wiring が未特定」は、routes の記録どおり、task の worktree marker と project repository の default branch・`[selfdeploy] releases_dir`（`ApiState.integration_releases_dir`）で解消した。

## 確認（前回からの継続分）

- `task-ops/src/stranded.rs`: 状態遷移と既存 integration request への merge route。
- `task-ops/src/inbox.rs`, `human_inbox.rs`: Unintegrated item の生成と kind。
- `task-ops/src/view.rs`: TaskSummary/TaskDetail の所有位置。
- `celerisctl/src/main.rs`: `integration` は `cron`/`models` と同じく DB を開く前に処理し、CoS run 下の mutation は拒否される。

## 未解決

- 受信箱の discard/hold は item の `native` に入っていない（`native` は merge だけ）。web は `operation_path` に操作名を足して呼ぶ必要がある。詳細は [close](close.md) の未解決 1。
- ADR D4 の「merge は git 到達確認を通す」と API の merge（既に integrated なら記録、未取り込みなら統合依頼の記録）の差。人の判断が要る（close の未解決 2）。
