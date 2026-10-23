---
title: replan の done 持ち越しと計画上限の修正
updated: 2026-10-23
completed: 2026-10-23
tasks: [01M5MAPDJVDBJ1FXMDBJABD8E2]
status: done
---
# replan の done 持ち越しと計画上限の修正

ADR: `agent-docs/adr/2026-10-23-replan-planner-portion-limits.md`

## 実装

[ADR](../adr/2026-10-23-replan-planner-portion-limits.md) に従い、`/3` の計測用 planner 分と採用する全体を分離。
拒否の内訳、run ID・試行番号付き rejected JSON、planner の説明を更新した。

## 検証

- 大量の done 履歴を持つ `/3` の回帰試験: 成功。
- dispatcher の子失敗 → replan → 採用 → done の回帰試験: 成功。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- 最終 `bash scripts/dev/test-affected.sh`: exit 0。6,313 pass / 0 fail / 18 skip、doc test も成功。

初回の `test-affected` は 6,311 pass / 1 fail。拒否の内訳を補足エラーとして追加したため、
既存の soft tree limit（採用後に limit 決定を要求するもの）の分類から外れた。
`is_tree_plan_limit_error` で内訳を補足として許容し、最終コードの再実行で当該試験を含めすべて成功した。

ログは run の成果物ディレクトリ
`/local/celeris/data/workspaces/01M5MAPDJVDBJ1FXMDBJABD8E2/artifacts/` の
`regression.log`、`adoption-regression.log`、`test-affected.log`（初回）、
`test-affected-final.log`（最終）、`clippy.log`、`fmt.log` に保存。

## attempt 2（review 差し戻し対応）

- ADR と進捗を `docs/` から `agent-docs/adr/`・`agent-docs/progress/` へ移し、進捗の front matter を先頭に置いた（title/updated/status/tasks）。コードは変更なし。

## 本番への移行

本番 config の変更・DB migration は不要。通常のコード配送で有効になる。
既存の固定名 rejected JSON は変更せず、以後の拒否から新しいファイル名で残す。
本 run では本番サービス・DB を操作しない。
