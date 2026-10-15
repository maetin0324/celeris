---
unit: mock-e2e
status: done
completed: 2026-10-15
---
# mock-e2e

実 LLM・実キー・外部ネットワークなしの mock E2E。`crates/task-dispatch/src/dispatcher/tests/mlsys_mock_e2e.rs`（3 件。`credential_e2e.rs` の harness を `pub(super)` にして再利用。本体コードの変更なし、欠陥は見つからなかった）。

## 通した流れ
1. キーなし: 分類段が置く `key_missing`＋credential 宣言 → その unit だけ `waiting_for_auth`（兄弟は Ready）、受信箱 item 1 件（/accounts リンク・「自動で続行」）。
2. ダミーキー登録 → mock 認証（`Scripted::Valid`）→ 同じ unit が一度だけ再開（`credential_resumed` event 1 件、再 process しても増えない、item は消える）。
3. 再開後、分類 JevOk（注入）→ lane は fixed 方策を下げず、`real_api_verified=false`。
4. 分類→方策→実行→ログ→export: task を実際に dispatch → `routing_decided.lane_assist`、`decision_log_runs` に提案・実行・成果が入り、campaign_id/arm を付けた `decision_log_export_campaign` が decision/run/outcome の 3 record を出す。再 export は byte 一致、ダミー鍵の値は出力に無い。
5. 故障時 fallback: timeout / breaker_open / rate_limited / invalid_response → `fell_back_to_fixed{reason}`、lane は fixed 実行と同じ、`basis=fixed_fallback`、`real_api_verified=false`。run・ログは通常どおり完了。

## 証拠
- `cargo test -p task-dispatch mlsys_mock_e2e`: 3 passed。
- test-parallel・clippy の結果は下の「最終検査」。

## 実装済み／未検証
- 検証済み（mock）: 上記の流れ、Jev adapter の fake server 試験（jev-adapter 節）、policy_close 7 件。
- **未検証**: 実キー・実 Jev API（応答形・認証 header）、実 LLM での worker 実行、実タスク性能。この結果は配線の確認であり実タスク性能ではない。
- 未配線（既知）: ready な WU を分類して board を埋める tick 連携と `JevClassifier` の構築・`ApiKeyProvider` 実装（classify-stage 未解決事項のまま）。Jev の HTTP を通した分類段と dispatcher の同一試験は無い（段は `RequestClassifier` fake、HTTP は fake server で別々に試験）。
- decision_log の `routing_mode`・`classifier` 欄は dispatch 経由では Null のまま（export の record で確認）。lane_assist 側が正。次 unit 以降で書き込みを接続する提案。
- campaign_id/arm はこの試験では書き手の代わりに付与（campaign runner 経由の自動付与は未接続）。

## 最終検査
- `bash scripts/dev/test-parallel.sh`: exit 0、nextest 5349 passed / 0 failed / 13 ignored、doctest ok、`test-parallel: ok`。
- `cargo clippy --workspace -- -D warnings`: exit 0。
