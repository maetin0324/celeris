---
title: "P1-6 final-accounting: 実 router 経由の最終集計の検証"
tasks: [01M54N11F9PWWD39MJ4J1BBJG5]
status: done
updated: 2026-10-17
completed: 2026-10-17
---
# final-accounting 検証

詳細は [docs/progress/2026-10-17-p1-6-final-accounting.md](../../docs/progress/2026-10-17-p1-6-final-accounting.md)。

- 修正: `final_accounting::unfinished_rows` が台帳の全履歴行を予約として数えていた → 鍵ごとの最終行だけを見る。
- 追加: `campaign_acceptance_e2e` に最終集計の通し 2 件。実 dispatcher/LLM は未検証。
- 証拠: 3 本の受け入れ試験 exit 0、`test-parallel.sh` 5635 passed、clippy・fmt exit 0。

## 提案

- `celeris-campaign` の CLI 引数経路（`--records`）を通す試験は bin が別 crate のため未整備。
