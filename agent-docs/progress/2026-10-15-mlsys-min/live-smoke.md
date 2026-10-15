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

## 再実行 3（2026-10-15）
- `GET /api/v1/secrets`（読み取りのみ）: `jev_api_key` は依然 `updated_at: null`・`fingerprint: null` = 未登録。Jev 実 API 統合は **未検証**（キー値は扱っていない）。
- 人が許可した「Jev を要求する task を先に走らせて credential 待ちを起こす」は、現状では起こせない: 待機の入口は `[llm_sources.<id>] wait_for_credential = true` + `credential_ref`（cw-enter）で、本番 config に Jev の source 宣言が無い（本番 config は worker が変更不可）。
- 本番への移行（人が実施）: `~/.config/celeris/config.toml` に Jev の `[llm_sources.jev]`（`wait_for_credential = true`、`credential_ref = "jev_api_key"`）を追加し reload。その後、decision-assisted の task を 1 件走らせると、その work unit だけが待機→受信箱/Discord 通知（/accounts リンク付き）。人が GUI『アカウント』で登録→自動再開→この unit を再実行して分類 1〜数件のスモーク。
- decision-log への「検証済み/未検証」の記録は、スモーク実施後に行う（現時点は未検証）。
