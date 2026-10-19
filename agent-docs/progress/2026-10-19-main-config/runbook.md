---
tasks: [01M5A29H2319SHCSYS6EBYR90G]
status: done
completed: 2026-10-19
---
# runbook: 主比較 runbook の実値整合と最終再検査

## 実施
- `docs/ops/measure-run-final.md` を主比較（mlsys-main-1）の実値へ更新: §3a（承認済み release/binary/A 版/account 参照名/CAP_ENV/campaign_sha256/固定 hash/修正後 binary/予算は主副で 1 本・二重計上なし）、offline-lock による env-lock/3・campaign lock の生成と `--verify` 照合、実 K2 無課金 preflight 証跡（exit 0・paid_calls_made 0・4 probe）の参照、§4 の symlink 未解決記述を解決済みへ、§0/§10 の operator 未取得記述を証跡に合わせて更新、§7/§8 の停止・再開・採点・export を実在引数（`--env` のみ、`grade-out --cases`、`--case $R/...`）と固定 hash 一覧へ。
- host に jq が無いため、jq を使わず python3 で JSON を見る手順を明記（文書内に jq 依存は無い）。
- 運用手順としての試行開始（`watch`、人の決定待ち）と本タスクの無試行検証を文書で区別した。
- 選択機構（01M59KXY8345HZEE06FVZ3FWVP）未統合の部分は、セット別の最終 preflight だけ task 01M58VG5C4XGK4E70M6WJ6MEX0 に引き継ぐ（副比較の準備は待たない）。
- 新規実装なし。daemon・実 LLM・watch・本番 config/DB は触れていない。

## 証拠
- `python3 scripts/campaign/check-main-pins.py --root /local/celeris/data/workspaces/01M58VFRBA6HG0SSF3CG5BKM91/artifacts` → `check-main-pins: ok`、exit 0（実 K2 証跡 `pinned/main/evidence/k2-preflight.json` を内容検証）。

## 未解決・提案
- 本測定の開始（`watch`）は人の決定待ち。
