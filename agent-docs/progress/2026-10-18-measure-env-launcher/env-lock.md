---
title: "測定環境 launcher: 固定記録 env-lock と再開照合（env-lock）"
tasks: [01M56HCNS9SYAS7TAEA7Z3E4CZ]
unit: env-lock
status: done
updated: 2026-10-18
---
# env-lock

完了日: 2026-10-18。ADR `agent-docs/adr/2026-10-18-measure-env-launcher.md` D3 のうち、daemon に依らない部分を実装。

## 実装
- `crates/campaign/src/env_lock.rs`: `EnvLock`（A の絶対 path・sha256・`--version`、B/C の release sha12、arm ごとの
  lower_policy・evidence_table_id・models・run_harnesses・catalog、worker 実行ファイル、`campaign_sha256`）。
  `expected`（A の版が manifest の A `harness_version` を含むこと、B/C が同一の 12 桁 hex release であること、manifest の sha256 欄と
  再計算値の一致、必須入力の欠落を全件 Err）、`diff`（違う key 名を返す）、`establish`（無ければ原子的に作成、あれば照合、`--verify` は作らない）。
- CLI: `celeris-campaign env-lock --manifest <m> --out <dir> [--a-program <p>] [--worker-program <name>=<p>]... [--verify]`。
- 秘密値は持たない（環境変数・credential・token を読まない）。
- 試験 `crates/campaign/tests/env_lock.rs`（stub binary、実 LLM なし）: 作成と再開一致・秘密非混入、同一 path の binary 差替え
  （`--version` 同一でも `a.program.sha256` で拒否）、版不一致、B/C release 不正、manifest hash 不一致、worker 差替え、必須入力欠落。

## 証拠
- `cargo test -p celeris-campaign --test env_lock`: 6 passed
- `bash scripts/dev/test-parallel.sh`: exit 0（passed 5724 / failed 0）。`cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0

## 未解決事項・提案
- daemon 側の項目（`bc.daemon.*`、`instances_seen`、catalog / evidence の実値）は env-check WU が `Observed.catalog` と型の拡張で渡す
  （daemon-identity-api の `identity` が前提）。この WU では型だけ用意し、CLI 単体の catalog は空。
- 試行ごとの A 実体照合（harness_exec 起動直前の sha256 取り直し）は、`ResolvedProgram` を触る env-check/launcher WU との衝突を避けて未実装。
  `env_lock::pin_program` と `diff` を使えば足せる。
- 実装済み / 無 LLM 検証済み。実経路確認（実 daemon・実 claude）は未。
