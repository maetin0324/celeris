---
unit: executor
task: 01M530Y2MXJFVQBWJSP21F02DE
status: done
completed: 2026-10-16
---
# executor: CelerisExecutor::body から識別を送り runs[].usage と decision-log を整合

## 実装済み
- `TrialRequest` に `campaign_id` / `manifest_sha256` を追加（runner が manifest から渡す）。
- `CelerisExecutor::body`（pub）が `campaign`（campaign_id / case_id=問題 ID / arm=A|B|C / repetition / manifest_hash /
  routing_setting=routing_override.mode）と `campaign_idempotency_key`（label と同じ文字列）を載せる。api-ident の `NewTaskBody` と同形。
- 二重計上しない集計の方針: 外側 Executor が読む trial の token・費用は `runs[].usage`（task 木の worker run）だけ。
  Jev 呼び出し（`decision:jev:…` 行、定価換算）と CoS・介入は decision-log 側の別 subject で、run の usage に足さない。
  export は run_id・call_id で一意なので再投影しても増えない（jev-calls の試験）。共有オーバーヘッドの記録は cos-record の担当。

## mock検証済み
- `crates/campaign/tests/celeris_exec.rs::body_carries_campaign_identity_and_idempotency_key`。
- `crates/task-api/tests/campaign_ident.rs::executor_body_is_accepted_and_identity_is_stored`: executor の body を実 task-api router に
  POST（識別の直挿入なし）→ 201・DB に識別、再送 200 同 task、別 repetition は別 task。
- `bash scripts/dev/test-parallel.sh` exit 0（5521 passed, 0 failed）、`cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。

## 実経路検証済み
- なし（一時 DB の in-process 試験まで。本番は触っていない）。

## 承認待ち
- なし。

## 未適用 patch / 未解決
- identity 側の未適用 patch は着手時の worktree に見当たらず（api-ident で受け口を新規実装済み）。取り込み対象なし。
- 子・孫・再試行・review への継承と export --verify-connected 相当の通し試験は propagate / cos-record の担当。
