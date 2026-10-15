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

## 再実行 4（2026-10-15）
- `GET /api/v1/secrets`（読み取りのみ）: `jev_api_key` は依然 `updated_at: null`・`fingerprint: null` = 未登録。本番 config（~/.config/celeris/config.toml）に `[llm_sources.jev]` も無い（grep で 0 件）。
- Jev 実 API 統合は **未検証**（キー値は扱っていない、検証済み扱いしていない）。credential 待ち→通知→自動再開の実経路は、本番 config に Jev source を人が追加するまで起こせない（手順は「再実行 3」）。
- decision-log への検証済み/未検証の記録は、スモーク実施後。

## 再実行 5（2026-10-15）
- 運用セッションの調査: 分類段は本番 daemon に配線されていない（crates/celeris/src/daemon.rs:22 のコメントのまま、Config に Jev 欄なし）。人は配線の unit 追加を承認済み。この unit のブランチには配線が無く、実経路のスモークは起こせない。
- Jev 実 API 統合は **未検証**（キー値は扱っていない）。配線・release・昇格・config 投入・鍵登録の後に再実行する。

## 再実行 6（2026-10-15 22:15Z、jev-wire 昇格・鍵登録後）
読み取りのみ（本番 DB・config は変えていない。キー値は扱っていない）。
- 鍵: `GET /api/v1/secrets` → `jev_api_key` updated_at 2026-10-15T22:12:39Z、fingerprint あり（値は見ていない）。登録済みなので credential 待ちは起きない想定。
- 本番 config: `[routing] mode = "decision_assisted"` と `[jev] enabled = true` が入っている。稼働 release 2c07df44ebef（jev-wire 込み。`/local/celeris/state/releases/2c07df44ebef` が存在）。
- 実経路の観測（この live-smoke 自身の dispatch、`GET /api/v1/tasks/<root>/events` の `routing_decided`、22:14:16Z）:
  `lane_assist = {mode: decision_assisted, classifier_status: {status: not_attempted}, real_api_verified: false, base_lane: standard, final_lane: standard, basis: fixed_fallback}`。
- 解釈: この unit は decision_answered（22:14:14Z）で blocked→ready になり 2 秒後に dispatch された。分類は tick の直前に走る（`classify_tick`）ので、ready になった同じ tick で dispatch されると board が空のまま = 分類は試みられていない。reload 後に分類が走ったことの証拠ではない。ほかに reload 後に dispatch された WU はまだ無い（観測時点で 1 分）。
- 待ち→通知→登録→一度だけ再開の実経路: 鍵を先に登録したため**起きていない**（試験でのみ示されている: cw-e2e / jev-e2e の fake server）。実経路としては**未検証**。

### 検証済み / 未検証
| 項目 | 状態 | 根拠 |
|---|---|---|
| 鍵が登録済み（fingerprint あり） | 検証済み | secrets API |
| 本番 config が decision_assisted + jev enabled | 検証済み | config.toml の該当節（読み取り） |
| 本番 release が jev-wire 込み | 検証済み | releases/2c07df44ebef の存在、運用セッションの回答 |
| 実 Jev API への分類 1〜数件の成功 | **未検証** | 観測できた routing_decided は not_attempted のみ |
| fell_back_to_fixed の有無と理由 | **未検証** | 分類が試みられた記録がまだ無い |
| credential 待ち→通知→登録→再開（実経路） | **未検証** | 鍵先行登録のため発生せず |

## 次の一手（人 / 運用セッション）
分類を試みる dispatch が 1 件要る。最小は、ready になってから 1 tick 以上おいて dispatch される安い task（または既存 WU）を 1 件走らせ、その `routing_decided.lane_assist` を読む。`classifier_status` が `succeeded` なら実 API 検証済み、`fell_back_to_fixed{reason}` なら理由を記録。本 unit を再実行するとき、pilot など後続 WU が ready になる前に確認できる。有料の本測定は始めない。

## 再実行 7（reload が config.jev を書き戻さない不具合の修正）
- 原因（運用セッション調査）: `reload_providers`（crates/celeris/src/daemon/admin.rs）が `config.jev` を書き戻さず、tick_loop の `classify_built_for != Some(&config.jev)` が一致したまま分類段が作り直されなかった。
- 修正: admin.rs に `config.jev = new_config.jev;`。試験 `reload_writes_back_the_jev_section_so_the_next_tick_builds_the_stage`（[jev] enabled=false で起動→true に書き換え→reload→`ClassifyTick::build` が Some）。
- ログ: tick_loop.rs が分類段の（再）作成・無効化を info 1 行、classify_stage.rs が分類ごとの結果（`jev_ok` 等 / `fell_back_to_fixed` と reason）を info 1 行出す。鍵の値・入力本文は出さない。
- 作業ツリーの base に jev-wire が無かったため、親ブランチ celeris/01M4Z1QJX5FYBXTPSEF0WKB1CK を merge してから修正した。
- 検証: `cargo fmt --all -- --check` exit 0 / `cargo clippy --workspace -- -D warnings` exit 0 / `bash scripts/dev/test-parallel.sh` exit 0（passed 5366, failed 0）/ 対象試験 24 passed。
- 実経路（実 Jev 分類の成否・fell_back_to_fixed の理由・待ち→通知→登録→再開）はこの run では観測していない = **未検証**のまま（上の表を維持）。release・昇格は運用セッション。昇格後、ready から 1 tick 以上おいて dispatch された WU の `routing_decided.lane_assist.classifier_status` と、daemon ログの `jev: classification ...` 行を確認する。
