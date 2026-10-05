---
tasks: [01M44NN2DCZXV0TZ5FXX5TMN58]
---
# Phase 1: surface 段の統合検査失敗（routing_old_events_deserialize_without_optimizer）の修正

## 原因

段 `surface` 統合後の `bash scripts/dev/test-parallel.sh` が 1 件だけ落ちた（3933 passed / 1 failed）。
失敗は `task-core` の `model_router::tests::routing_old_events_deserialize_without_optimizer`（`crates/task-core/src/model_router/tests.rs`）。

試験は「旧 event（`optimizer` 欄なし）と新 event（`optimizer: Some(trace)`）を `routing_audit` に通した結果が等しい」ことを `assert_eq!` で求めていた。
catalog-api 葉（commit `ec9c35d3`）は `crates/task-core/src/routing_audit.rs` で `a.optimizer = record.optimizer.clone()` を入れ、optimizer trace を task routing の投影に載せた。
ADR `2026-10-04-multi-objective-model-routing.md` §9（task routing の optional trace）どおりの意図的な変更であり、旧試験の「全体一致」の前提だけが崩れた。

## 直した箇所

- `crates/task-core/src/model_router/tests.rs`（試験名は変更なし）
  - 旧 `assert_eq!(routing_audit(old), routing_audit(new))` を次の形に分けた。
    - 旧の投影は全件 `optimizer.is_none()`、新の投影は全件 `optimizer.is_some()`（§9 の投影規則）。
    - 両方の `optimizer` を `None` に落としてから比べ、lane・rule_id・policy_version・reasons・features など他の欄が旧新で等しいことを確かめる。
- プロダクトコード（`routing_audit.rs` の投影、catalog-api の変更）は変えていない。

他の whole-audit 比較の確認: `crates/` 内の `routing_audit(` 呼び出し 11 箇所を走査し、全体一致を取っている箇所は `model_router/tests.rs` の 1 件だけだった（他は lane・件数・個別欄の検査）。

## 証拠

- `cargo test -p task-core routing_old_events_deserialize_without_optimizer` → exit 0、`1 passed; 0 failed`。
- `cargo test -p task-core -p task-ops -p task-api` → exit 0。test binary 56 個、passed 1675、failed 0（出力の `FAILED` / `panicked` 0 件）。
- `cargo fmt --all -- --check` → exit 0（差分なし）。
- `cargo clippy --workspace -- -D warnings` → exit 0。ただし test コードは対象外のため、
- `cargo clippy -p task-core --all-targets -- -D warnings` → exit 0（変更した test コードを含めて検査。`Checking task-core` 1 回、warning なし）。

## 未解決事項

- 全体の `bash scripts/dev/test-parallel.sh` の再実行は段 `close`（全体検査）で行う。この WU では task-core・task-ops・task-api の試験に限った。
- `task-dispatch` の `dispatcher/tests/routing_and_quota.rs` は `routing_audit` の個別欄（lane）を見ており、全体一致の比較ではないため今回は触っていない。close の全体検査で確かめる。

## 提案

- 投影の全体一致を試験で書くと、投影に欄を足すたびに旧 event 側の試験が落ちる。今後は「新欄は旧 event で None・新 event で Some」と欄ごとに書き、残りを比べる形を既定にする（この WU の書き方）。
