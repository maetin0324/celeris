---
unit: live-smoke
status: blocked-on-human
updated: 2026-10-15
---
# live-smoke

## 状況（2026-10-15、再実行）
- Jev 実 API 統合: **未検証**（実接続スモーク未実施。キー値は一切扱っていない）。
- 本番は 57d4d30421cf に昇格済み。読み取りのみで確認: `GET /api/v1/secrets` に `jev_api_key`（provider jev）が出ており `updated_at: null`・`fingerprint: null`、つまり**未登録**。
- worker は GUI『アカウント』で登録できず、キー値も扱わない方針。本番 host・本番 DB を変えない制約のため、credential 待ちを本番で起こすこともしていない。

## 人への依頼
1. GUI『アカウント』で `jev_api_key` を登録する（チャットにキーを書かない）。
2. Jev を要求する work unit（decision-assisted の実験）が走ると credential 待ち→受信箱/Discord 通知→登録→自動再開になる。登録後にこの unit を再実行すると、分類 1〜数件の最小スモークを行い、結果を decision-log に「検証済み/未検証」で区別して記録する。
