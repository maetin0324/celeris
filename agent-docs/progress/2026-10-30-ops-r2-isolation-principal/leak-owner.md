---
title: 漏洩検査が owned_by_run_uid の残る path の same_uid を外さない（WU leak-owner）
tasks: [01M66JYQ249SZHNXK0AEVTZNEC]
status: done
updated: 2026-10-30
---
# 漏洩検査が owned_by_run_uid の残る path の same_uid を外さない（WU leak-owner）

完了日: 2026-10-30

ADR は `agent-docs/adr/2026-10-30-ops-r2-isolation-principal.md`。前の attempt の最終 review（criterion 3）の差し戻しへの対応のうち、漏洩検査の側を直した。preflight の L1 の側は WU l1-owner が直す。

## 変更
- `leak-owner: 漏洩検査は owned_by_run_uid の残る path の same_uid を外さない`
  — `scripts/campaign/check-ops-r1-leak.py`・`scripts/campaign/test_check_ops_r1_leak.py`・`docs/ops/ops-r1-leak-check.md` を変えた。`crates/` は変えていない。

## 判定規則（変更後）
- probe で判定した path も、host 側の stat で path と祖先のうち run の uid が持ち主のもの（root の run なら全部）を `owned_by_run_uid` に書く（最大 5 件）。
- `same_uid_limit: false`・`same_uid_cleared_by: runtime_probe_and_l1` にするのは、runtime の probe と L1 の probe が両方 `readable=false`、かつ `owned_by_run_uid` が空の path だけ。
- `owned_by_run_uid` が空でなければ、両方の probe が拒否でも `same_uid` の limit を残す。canary が無ければ `same_uid_without_canary` の hit（exit 1）。
- probe JSON の形（`celeris.runtime-probe/1`）は変えていない。

## 試験
- `test_same_uid_chmod_000_keeps_same_uid_even_if_both_probes_deny`: 試験の uid が持つ dir と file を 0o000 にし、実際に `open` と `listdir` が PermissionError になることを確かめたうえで、runtime と L1 の両方の probe が拒否でも `same_uid_limit: true`・`same_uid` の limit が残り、canary なしでは `same_uid_without_canary` の hit で exit 1 になる。
- `test_runtime_and_l1_denied_clears_same_uid_only_without_owned_path`: `--run-uid` に別の uid を渡すと `owned_by_run_uid` が空になり `same_uid_cleared_by: runtime_probe_and_l1` になる。同じ probe で run の uid が持ち主のままなら外れない。
- 両試験は root で走らせると skip する（root の run は全部の path を持ち主として扱い、mode 000 も読める）。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
- `python3 -I scripts/campaign/test_check_ops_r1_leak.py` → exit 0（Ran 35 tests, OK）
- `bash scripts/dev/test-affected.sh` → exit 0（Rust の試験対象なし。scripts・docs の検査は合格）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/progress-index.sh --check` → exit 0

## 未解決事項
- `agent-docs/progress/2026-10-30-ops-r2-isolation-principal/leak-runtime.md` と isolate.md・ADR 付記・`docs/ops/ops-r2-isolation.md` の記述は、後段の WU docs-align が新しい規則に合わせる（この WU の範囲外）。
