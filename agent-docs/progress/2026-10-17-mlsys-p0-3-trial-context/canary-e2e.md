---
title: "MLSys P0-3 canary-e2e: 実 task-api router を通す canary 試験と最終記録"
tasks: [01M54HYGQPFNRX6PNPJJG475EB]
status: done
updated: 2026-10-17
completed: 2026-10-17
---
# canary-e2e（WU）

要約は親の進捗 `agent-docs/progress/2026-10-17-mlsys-p0-3-trial-context.md` 末尾「最終記録（canary-e2e）」に書いた。

## 変更
- 新規 `crates/celeris/tests/trial_context_canary_e2e.rs`（既存 e2e の配置 `crates/celeris/tests/`。`campaign_wiring_e2e.rs` と同じ流儀）。
- コードの変更は無い（fix-context `02c9b1f6` と preflight-kb `ef121e56` の接続を通常経路で確かめる試験だけ）。

## 証拠
- `cargo nextest run -p celeris --test trial_context_canary_e2e` → 3 passed（約 5 秒）
- 変異確認: (a) 試行の materials に通常 KB の canary を混ぜる → `A1 の workspace に NORMAL_KB_CANARY_7f3a` で fail、
  (b) `run_context.rs` の `trial_scope` を常に「試行でない」にする → 3 件とも `RunContext に NORMAL_KB_CANARY_7f3a` で fail。どちらも元に戻して pass を確認。
- `bash scripts/dev/test-parallel.sh` → exit 0（passed 5629, failed 0, ignored 15）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
