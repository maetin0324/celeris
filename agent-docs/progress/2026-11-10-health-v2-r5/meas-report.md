---
title: R5 meas-report — 再計測の表と ADR D7 の更新
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-12
completed: 2026-11-12
---
# R5 meas-report — 再計測の表と ADR D7 の更新

完了日: 2026-11-12。基準は `$CELERIS_WU_BASE`。

## 変更と commit（件名）

- 「phase 1: meas-report: 再計測の生成表で R5 の比較を置き換え、ADR D7 と手順を更新する」: 5 原票の summary block を R5 の進捗に載せた。6 crate の build/edit と 4 crate の run・flaky を記載。celerisctl の除外理由・load・比、workspace doc test の費用、未確認のばらつきを明記した。解決済みの ADR 不一致・worker 未計測・run 交互実行の提案を撤去した。singleton を keep に戻した後の束数 85 → 17 も反映した。
- 検査器・baseline・test-affected の対象規則は変更していない。

## 証拠（提出時の HEAD で実行）

- `measure.sh summary --check`（R5 の進捗と 5 原票。完全なコマンドは remeasure.md）→ exit 0、生成 block と一致。
- `sh scripts/dev/check-doc-links.sh` → exit 0、ok。
- `sh scripts/dev/check-adr-numbers.sh` → exit 0、427 files、ok。
- `sh scripts/dev/progress-index.sh --check` → exit 0、ok。
- `cargo clippy --workspace -- -D warnings` → exit 0、58.58 秒。
- `cargo fmt --all -- --check` → exit 0。
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/adr-check.py agent-docs/adr/2026-11-10-test-target-bundles.md` → exit 0、4 crate・17 束・85 target が一致。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → exit 0、変更は R5 の ADR と進捗 4 path のみ。

- `bash scripts/dev/test-affected.sh` → exit 0、docs のみで crate の試験対象なし、repo health pass。

## 未解決事項

- 無し（計測の限界は remeasure.md と R5 の進捗に記載）。

## 提案

- 無し。
