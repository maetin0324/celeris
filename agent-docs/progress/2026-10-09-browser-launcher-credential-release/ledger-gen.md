---
tasks: [01M4FRN13Z206RAWBZ0NHZQEGX]
---

# ledger-gen: launcher credential evidence

- `ConformanceEvidence` に `runtime`（`daemon` / `launcher`）を追加。旧台帳で省略された値は daemon として読む。
- `CredentialInjection` / `IdentityRestore` の認定では isolation・egress の全証拠が launcher runtime として成功していることを要求する。P4-B の既存測定は runtime 属性を尊重する。
- `scripts/browser-conformance.py` に `--credential-runtime daemon|launcher` を追加。launcher は `CELERIS_BROWSER_LAUNCHER_SOCKET` が無い場合 `launcher_unavailable` と理由を出し、出力台帳では選択 backend の credential 証拠と passed claim を消す（入力台帳は変更しない）。launcher 実行時は `CELERIS_LAUNCHER_TESTS=require` を設定し、結果行に runtime を記録する。
- launcher 偽証拠・daemon のみ・runtime 混在を確かめる `browser_ledger_launcher_` 3 件を追加。

## 検証

- `cargo test -p task-worker browser_ledger_launcher_ --lib`: 3 passed
- `cargo test -p task-core p4b_cases_count_only_with_measured_evidence --lib`: 1 passed
- `python3 -m unittest scripts.tests.test_browser_conformance_credential`: 7 passed (launcher unavailable clearing output claim covered)
- `git diff --check`: passed
- scope snapshot (`sh "$CELERIS_WU_SCOPE_PATHS"`): 許可範囲内。task-core browser backend、task-worker ledger tests / fixtures、生成器と tests、progress。

launcher host 実測はこの葉では実施していない。
