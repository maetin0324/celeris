---
unit: jev-calls
task: 01M530Y2MXJFVQBWJSP21F02DE
status: done
completed: 2026-10-16
---
# jev-calls: Jev 呼び出しを一意に記録し費用を保持、欠測を 0 にしない

## 実際の起点
`decision_log_wire.rs` ではなく `crates/task-dispatch/src/dispatcher/lane_assist.rs`（`record_jev_usage`）。
旧実装は `jev_call(subject, 1, …)` の固定 call_no で、`JevOk` のときだけ書いたため、再分類は上書き、失敗（fallback）は欠落していた。

## 変更
- `ClassificationBoard::put` が呼び出しごとに一意の `call_id`（起動時刻 + 連番）と subject 内の `call_no` を付け、
  未記録の呼び出しを `take_unrecorded` で古い順に渡す（dispatcher が読む前に置き換わっても落ちない）。
- `record_jev_usage` は呼び出しごとに `decision:jev:<subject>:<outcome>:<call_id>:<call_no>` の行を書く
  （outcome = `ok` / `fallback:<reason>`）。2 回目以降は `is_retry`。`NotAttempted` は書かない。
- 費用は応答が返した値だけ `list_cost_usd`（定価換算）へ。返らなければ NULL（0 にしない）。実支出 `cash_cost_usd`・quota には書かない。
- `decision_log_record_usage` の upsert は費用 2 欄を COALESCE（後から欠測で上書きして取得済み費用を消さない）。
- 試験: `jev_calls_are_kept_per_call_and_missing_cost_stays_null`（失敗・成功・再分類が 3 行残る、欠測は NULL、再 tick で増えない）。

## 検証の区分
- 実装済み: 上記。
- mock 検証済み: task-dispatch lib 試験（934 passed）、`bash scripts/dev/test-parallel.sh` exit 0（5518 passed, 0 failed）、
  `cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。
- 実経路検証済み: なし（Jev 実 API・本番 DB は触っていない）。
- 承認待ち: なし。

## 未解決・提案
- 失敗時に Jev が費用を返すかは `jev-classifier` の `FallbackToFixed` が費用を持たないため未取得（NULL）。応答 body に usage があれば別途拡張。
- 一意な識別（campaign 識別の API 受け口）は api-ident、export 側の接続は他 unit の担当。
