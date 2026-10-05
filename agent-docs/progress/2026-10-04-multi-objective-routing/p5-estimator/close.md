---
tasks: [01M4651ZZP8FJKGG6W2WPFNKBH]
unit: close
status: done
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

## 原票の転記（2026-10-05）

人（Fable）が本番 host で実行した実 sidecar の原票（commit `3c1a0f5c`、`docs/reports/model-routing-routellm-shadow/`）を
report §3 に転記した（実行コマンド・exit・件数・上限消費・失敗・latency・依存・weights・機材。制約違反数・coverage は
未計測と明記）。手順書 §10.2 の license 行・§10.3 の状態（approved）・§10.5 の検証済み構成を直し、ADR 付記の状態行を
更新した。新しい実装はしていない。再検査は task の進捗の「原票の転記後の再検査」。

## 未解決事項

- Celeris 本体の opt-in shadow による coverage・制約違反数は未計測（人の別判断）。
