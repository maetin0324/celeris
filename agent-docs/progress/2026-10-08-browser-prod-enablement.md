---
title: 本番で browser 実行を使える状態にする（適合台帳の release 組み込み・前提 gate・site policy の DB 正本化・task policy 自動付与・点検コマンド）
tasks: [01M4CDNAYX6J68WTX7SKF0DJ64]
status: done
updated: 2026-10-08
completed: 2026-10-08
---
# 本番 browser 実行の有効化（close-out）

ADR `agent-docs/adr/2026-10-08-browser-prod-enablement.md`（状態: 実装済み）。葉ごとの進捗は `2026-10-08-browser-prod-enablement/<key>.md`
（adr・ledger-gate・site-policy-api・ledger-release・task-policy-auto・preflight・web-site-policy・web-task-policy）。
運用手順は `docs/ops/browser-prod-enablement.md`。

## 証拠（統合後 HEAD `a48a440c`、2026-10-08）

- `bash scripts/dev/test-parallel.sh` → exit 0。passed 4761、failed 0、ignored 14、tmp_leftovers 0（nextest 190 秒、doctest 9.8 秒）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo test -p task-core -p task-ops -p task-api -p task-dispatch -p task-worker -p celeris -p celerisctl -p celeris-credentiald browser_` → exit 0、316 passed、0 failed。
- 5 つの接頭辞の合格数（上の実行の ok 行を数えた）: `browser_ledger_gate_` 12、`browser_site_policy_db_` 6、`browser_policy_autoattach_` 6、`browser_ledger_release_` 10、`browser_doctor_` 13。
- `for t in scripts/selfdeploy/tests/browser_ledger_release_*.sh; do sh "$t"; done` → `browser_ledger_release_stages.sh` 1 本、exit 0。
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → exit 0（ledger-release 葉で出ていた `docs/ops/browser-prod.md` の broken reference は preflight 葉で解消済み）。
- コード修正が要る失敗は見つからなかった。

## 未解決事項

### 本番での有効化（人・運用セッション。coding run は本番を触らない）
1. この branch を main に取り込み、release を prepare・verify・promote する（`docs/ops/browser-prod-enablement.md` §1）。
2. 昇格後、運用セッションで `celerisctl browser doctor --config <config.toml>` を流し、NG が無い（exit 0）ことを確かめる。NG は同文書 §3 の表で直す。
   既存 release の台帳だけ作り直すなら `bash scripts/selfdeploy/browser-ledger.sh <sha12>`。
3. 台帳生成は release 全体を失敗にしない（非 blocking）。実 agent-browser・実 Chromium・userns での生成と、credential 注入に要る P4-B 証拠は本番 host で未実施。

### manaba 監視 task 01M4CD11W51ADC4HHMMEKRQM71 の再開に人がすること
- web で owner session を立てる（信頼デバイスを登録済みなら再起動後も自動復帰）。
- web の `/browser/settings` で manaba の site policy（ログイン先）と browser-execution grant の credential_use・credential policy を設定（再起動不要）。
- 初回だけ credential form に ID/password を入力する。
- run 中の承認（click・download・credential_use の都度承認）の承認ボタンを押す。
- task 詳細の browser policy を確認する（台帳不足で止まっていた task は台帳が有効になると自動で再開する。再開しなければ retry）。

### 実装側の残り
- ADR D4 の `BrowserTaskPolicySet { source }` event と task 詳細の出自欄は未実装（task-policy-auto・web-task-policy 葉の記録どおり）。web は `policy_id`（`auto`・`web-human`）で表示している。
- `TaskDetail.browser.prerequisite` 欄は未追加（web は既存 event から表示）。
- ADR D1.3 の「`promoted.json` の `browser_ledger` を `GET /releases` 経由で web の release 一覧に出す」は未実装。promote.sh・release.sh は欄を書くが、`crates/celeris/src/releases.rs`・`crates/task-api/src/releases.rs` が読まず、web にも表示が無い（台帳の状態は doctor・readiness・`/browser/settings` で見る）。
- ADR D3 の `Event::BrowserSitePolicyChanged` は作らず、別表 `browser_site_policy_events`（migration 0060）に追記している（site-policy-api 葉の判断）。web の `event-kinds.ts` に `browser_site_policy_changed` は無く、site policy 一覧は SSE で更新されない。
- web readiness 型は生成 schema に未収録で局所型を使っている。
- `POST /org`・`PATCH /org/{id}` の profile 経由の `credential_policy_ids` は実在検査していない（doctor が未知 ID を出す）。
- ledger-release の runner-release（`scripts/browser-conformance.py --celeris-release`）には葉の進捗ファイルが無い。

## 提案
- 出自 event（`BrowserTaskPolicySet`）と `TaskDetail.browser.prerequisite` を後続の core/API 葉で足す。
- `GET /releases` の release 要素に `browser_ledger`（`promoted.json`・`manifest.json` から）を足し、web の release 一覧に出す。
- site policy の変更を SSE で web に届けるなら、`browser_site_policy_events` を通知 feed に流す口か、ADR を別表方式に改める付記を入れる。
- 本番昇格後の doctor 結果と manaba 再開の実測を、運用セッションがこの file か新しい進捗に追記する。
