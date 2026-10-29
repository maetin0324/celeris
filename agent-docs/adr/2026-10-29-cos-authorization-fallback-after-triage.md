# ADR 2026-10-29: 認可の件の Discord 通知は CoS の一次対応の後だけ送る

---
tasks: [01M62FCC7WC7EFEZRN56GFG5NA]
---

- 状態: 採用（実装済み）
- 関係: [2026-10-05-cos-chat-home](2026-10-05-cos-chat-home.md) D6「通知の一本化と退避」、
  [2026-10-06-cos-inbox-triage-fallback](2026-10-06-cos-inbox-triage-fallback.md) 決定 2（不在の条件）、
  [2026-10-07-cos-inbox-thread-conversation](2026-10-07-cos-inbox-thread-conversation.md) D1（停止した run の件の差し戻し）

## 1. 文脈

人の依頼（受信箱スレッド seq 4026）: 認可を求める Discord 通知が CoS の一次対応の途中で届くのは煩わしい。一次対応の後でも
人の対応が要る場合だけ送ってほしい。

これまで不在退避（CosFallback）は、CoS run が動いていても件の到着から `[cos.triage] unavailable_after_secs`（既定 120 秒）を
超えると人へ直接送っていた。案件 CoS は認可の件で report や task を読むので 120 秒を超えやすい。受信箱の件
01M61GFWAH3B68TPTY10YJH7GV（待ち 123 秒）と 01M60XB6E614Z9CMGRD24KFW63（待ち 122 秒）は一次対応の途中で送られた。

## 2. 決定

D1. 認可（`cos_inbox_items.source_kind = "authorization"`、`InboxKind::Authorization`）の件は、その件を claim した
CoS run が動いている間（`chat_runs.state` が `queued`・`running`・`stopping`）は `unavailable_after_secs` で退避しない。
認可の件を Discord に送るのは次の場合だけ:

- (a) CoS が escalate した（resolve API の escalation outbox。fallback は作らない）。
- (b) CoS run が `failed`・`stopped`・`interrupted` で終わった、または run 行が無い（crash）。
  `stopped`・`interrupted` の件は 2026-10-07-cos-inbox-thread-conversation D1 のとおりまず `pending` に戻して CoS に
  もう一度渡す。その run も resolve せずに終われば送る。
- (c) run が `completed` なのに件が未処理。
- (d) `cos.enabled=false`。
- 下の D2 の安全網に掛かった。

CoS が answer・observe で閉じた件は送らない（item が終端なので退避の走査に入らない）。

D2. 安全網 `[cos.triage] authorization_unavailable_after_secs`。件に動いている run が無いとき（容量待ち・
`accepting_new_work=false`・disk guard・差し戻し後の待ち）は件の到着（`created_at`）から、run が動いているときは run の開始
（`chat_runs.started_at`）から、この秒数を超えたら人へ直接送る。未設定なら `[cos] max_wall_secs + 300` 秒
（既定 900 + 300 = 1200 秒）とし、`unavailable_after_secs` より短くはしない。根拠: run は `max_wall_secs` で worker に
止められ (b) で送られるので、run が動いている間の上限は wall-clock 上限より長くてよい。300 秒は止められた run の回収の
tick と終端の記録が遅れる分の余裕。run が始まらない場合も同じ値で、120 秒より十分長く、一次対応 1 回分（最大
`max_wall_secs`）より長い。明示する値は `unavailable_after_secs` より大きくなければ config の読み込みエラーにする。

D3. 認可以外の件（question・decision・failed・unintegrated・notice など）の挙動は変えない。到着から
`unavailable_after_secs` で、run の途中でも退避する。理由: 人の依頼は認可の件に限られる。失敗・統合の取り残し・
知らせは、CoS が遅れても人が早く知る方がよい件を含み、範囲を広げるかは別に人が決める。

## 3. 実装

- `crates/task-dispatch/src/dispatcher/cos_chat/fallback.rs`: `unavailable_reason` が `FallbackDeadlines`
  （`after_secs`・`authorization_after_secs`）を受け、`waits_for_first_response(source_kind)` の件だけ D1・D2 で判定する。
  走査は `chat_runs.started_at` も読む。
- `CosTriageSettings.authorization_unavailable_after_secs`（task-dispatch）と
  `CosTriageConfig.authorization_unavailable_after_secs: Option<u64>`・`CosConfig::authorization_unavailable_after_secs()`
  （celeris の config）。
- 試験: `crates/task-dispatch/src/dispatcher/tests/cos_chat_triage_fallback_authorization.rs`（時刻は注入した試験時計、
  sleep なし）、`crates/celeris/src/config/cos_tests.rs` の `cos_triage_authorization_safety_net_default_and_override`。

## 4. 本番への移行

config の書き換えは要らない（未設定で既定 1200 秒）。変えたいときは `[cos.triage] authorization_unavailable_after_secs`
を `unavailable_after_secs` より大きい値で書く。
