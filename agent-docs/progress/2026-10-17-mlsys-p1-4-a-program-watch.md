---
title: "[最終接続修正 P1-4] A の実行ファイル固定と watch 標準化"
tasks: [01M54M65D14ZETW8FXXMB7CTDT]
status: done
updated: 2026-10-17
completed: 2026-10-17
---

# P1-4: A の実行ファイルを preflight と実起動で一致させ、標準起動を watch に統一

## 変更
- `preflight::resolve_executable` / `resolve_program_in`（絶対 path・symlink 解決・実行可・`--version`）。`Host::program_available` は `Host::resolve_program` に置換（旧経路は削除）。
  preflight の `Report.a_program` に path と版を出す。`gate` は Report を返し、`main` の `a_executor` は `ClaudeCodeBuilder::from_resolved` で**同じ物**を起動する（`ClaudeCodeBuilder::default()` の PATH 上 `claude` は廃止）。未指定の既定（`claude`）も同じ解決を通る。
- `HarnessExecutor::submit` が `harness-state/<key>/launch.json`（program・version・argv（prompt は長さのみ）・sandboxed）を残す。
- runbook・`run-final.md`: 標準起動を `watch` に、`run` は単発走査と区別。§4.1 に監視・超過の限界を明記。

## 検証（無課金・stub のみ）
- 実装済み＋stub の実 process で検証: `crates/campaign/tests/a_program_pin_and_watch.rs`（4 件）。
  - PATH に別の `claude`（DECOY）を置いても、`--a-program` の stub だけが走る（DECOY の印なし）。launch.json に解決 path・版・argv。
  - 既定（`claude`）は PATH の解決結果が検査と実起動で一致。解決不能・版なしは拒否。
  - watch を `max_iterations` で止める（監視 thread なし）→ 新 executor で再起動 → 同じ 3 process を追跡、launch 3 回のまま（二重投入なし）、観測 usage 5,110 tokens が保たれ、上限 1,000 超過で kill・cancelled・全 process 消滅・台帳全終端・`settled`。
- 実経路（実 claude・実 task-api）では未検証。

## 未解決・提案
- preflight は版が manifest の `harness_version` と一致するかは見ない（人が確認）。

## 再試行 (attempt 2) の修正
- 前回は `crates/campaign/tests/preflight_contract.rs` の `RouterHost` が旧 `Host::program_available` のままで test-parallel の compile が落ちた。`resolve_program` に直した。
- 証拠: `bash scripts/dev/test-parallel.sh` exit 0（5625 passed, 0 failed）、`cargo clippy --workspace -- -D warnings` exit 0（`--all-targets` も 0）、`cargo fmt --all -- --check` exit 0、`cargo nextest run -p celeris-campaign a_program` 2 passed。
