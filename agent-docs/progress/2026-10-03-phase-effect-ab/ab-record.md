---
title: phase_effect_ab: 評価文書への前後比較の記録（ab-record）
tasks: [01M3WZJ9C8R9WN6A737G0X1G6N]
wu: ab-record
status: done
updated: 2026-10-03
completed: 2026-10-03
---
# phase_effect_ab: 評価文書への前後比較の記録（ab-record）

docs/progress/merge-train-evaluation.md に '## 前後比較' 節を足した。3 scenario × 4 指標の導入前・導入後・差の表、本番 DB の導入前の値との並べ表、模擬の限界、release 後の再計測手順（30 delivery 後）を書いた。'## 結論'（不要）は変えず、比較を踏まえた一文を足した。crates/ は触っていない。

## 証拠

| 条件 | コマンド | 結果 |
|---|---|---|
| ab-metric の 6 行 | `cargo test -p task-dispatch phase_effect_ab -- --nocapture` | exit 0、4 passed / 0 failed |
| crates/ に変更なし | `git diff --name-only 2d20adf7 HEAD -- crates` | 0 行 |
| 文書のリンク | `sh scripts/dev/check-doc-links.sh` | ok |

## 未解決事項

- 本番での導入後の値は、Phase 1〜5 を含む release の後に人が測る（手順は評価文書に書いた）。
