# CoS triage fallback（CoS 不在の検出と LLM 無しの直接退避）

---
tasks: [01M47VN95D8QQ65ATZVGHFFXA7]
status: done
completed: 2026-10-06
---

## 完了

- [ADR 2026-10-06 cos-inbox-triage-fallback](../../../adr/2026-10-06-cos-inbox-triage-fallback.md) に検出条件・claim・回収の決定を書いた。
- `crates/task-dispatch/src/dispatcher/cos_chat/fallback.rs` を追加。ingest 葉の `triage_ingest` の最後から `triage_fallback` を呼ぶ（cos.enabled=false でも intake と同じ tick で走る）。設定は ingest 葉の `CosTriageSettings.unavailable_after_secs` と `CosChatLaunchConfig.enabled` を使い、celeris crate は触っていない。
- 読み取りは task-api の triage_view と同じく daemon DB に自前の接続を開く（task-core の store API は変えない）。このため `rusqlite` を task-dispatch の dev-dependency から dependency へ移した（Cargo.lock は不変）。
- ingest 葉の試験 fixture を `fixture_script(enabled, script)` に一般化し、`cos.enabled=false` の導入試験の期待を `pending=2` から `fallback=2` に直した（D6 で無効時は即退避）。

## 証拠

- `cargo test -p task-dispatch --lib cos_chat_triage` → 18 passed（fallback 11 件 + ingest 7 件）。fallback の試験: run 失敗・quota/ログイン不可・無効・期限超過（容量待ち）・成功終了で未処理・再起動と CoS 復帰で再通知/代答なし・claim 後 crash の回収・escalation との競合で pending 1・送信失敗が新しい待ちを作らない・revision ごとに outbox 1・不在理由の判定の単体。
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0。
- `bash scripts/dev/test-parallel.sh` → exit 0、4164 passed / 0 failed / 12 skipped。

## 未解決

- 送信直前の再照合（`cos_triage_outbox_sendable`）と Discord への描画は notifier 葉。退避本文の `text` をそのまま使える形にした。
- run が稼働中のまま期限を超えた item も退避する。その後に CoS が同じ item を resolve しようとすると item は終端なので resolve API 側で拒否される（resolve-api 葉の is_terminal）。

## 提案

- 退避した item を web の受信箱に「CoS 不在で直接通知済み」と出す表示は schema/web 側の後続で扱う。
