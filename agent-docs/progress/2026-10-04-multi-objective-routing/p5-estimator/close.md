---
tasks: [01M4651ZZP8FJKGG6W2WPFNKBH]
unit: close
status: blocked
updated: 2026-10-05
---

# Phase 5 close: 全体検査・report・ADR 付記・進捗

統合後 HEAD `18f60cef8b39` で全体検査した。新しい実装はしていない。変更は記録だけ:
ADR 末尾の付記「Phase 5 の実装済み範囲（close による全体検査）」、`docs/reports/model-routing-routellm-shadow.md`
（status `awaiting-human`、§3 に人の決定、§4 close 時点の確認）、`docs/ops/model-routing-migration.md` §10.3 に人の決定の記録
（block は `pending` のまま）、task の進捗 [../p5-estimator.md](../p5-estimator.md)、本ファイル。

## 証拠

[../p5-estimator.md](../p5-estimator.md) の証拠表（test-parallel 4034 passed・clippy/fmt exit 0・filterset 38 passed・
unittest 3・check-runbook ok・fake-shadow-check ok・real-sidecar-check は not run・文書検査 3 本 exit 0）。
範囲 check（scope）→ 範囲外 path なし。

## 未解決事項

- 人の決定 `routellm-weights-use` の実行（実 sidecar の start-stop と上限付き shadow）と原票が無い。
  決定は「close は原票を転記してから完了」なので、この run は人への質問で止まる。
- report §2 の食い違い 3 点（実評価の前に直す必要がある。提案は task の進捗参照）。
