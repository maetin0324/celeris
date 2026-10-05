# Phase 3 (p3-model-fix2): model 段統合の E0063 修正 — feedback/tests.rs の RoutingRecord literal

完了日: 2026-10-05

## 背景

- model 段の統合（integrate-model）で `crates/task-core/src/model_router/feedback/tests.rs:15` の `RoutingRecord` 全欄 literal に escalation 葉（`crates/task-core/src/model_policy.rs:550`、`escalation: Option<crate::retry_policy::EscalationAudit>`）の欄が無く `E0063` で落ちる（原因の詳細は `p3-model-fix.md`）。
- 前の model-fix 葉は escalation 未統合の base（`103d9f88`）で走ったため修正を当てられず、進捗記録だけにした。
- この葉（`model-fix2`）は task branch の HEAD（`3a2fcf9a` 以降、escalation・events・model-fix 統合済み）から分岐するため、実際に 1 行の修正を当てた。

## 修正

- `crates/task-core/src/model_router/feedback/tests.rs` の `RoutingRecord { ... }` literal で `optimizer: None,` の次に `escalation: None,` を 1 行足した。
- `model_policy.rs` で `RoutingRecord` に `escalation` 欄があることを確認済み（merge 不要、base に既に取り込み済み）。
- 他のファイルは変えない。

## 証拠

| 段階 | コマンド | 結果 |
| --- | --- | --- |
| 型検査 | `cargo clippy --workspace --all-targets -- -D warnings` | exit 0（`Finished dev profile`） |
| feedback 試験 | `cargo test -p task-core feedback` | `test result: ok. 3 passed; 0 failed` |
| escalation 試験 | `cargo test -p task-core retry_policy` | `test result: ok. 6 passed; 0 failed` |
| context 試験 | `cargo test -p task-core context` | `test result: ok. 5 passed; 0 failed` |
| task-core lib 全体 | `cargo test -p task-core --lib` | `test result: ok. 736 passed; 0 failed` |

## 残タスク

- integrate-model はこの修正を取り込んだ上で統合後に test-parallel と workspace 全試験を流す。
