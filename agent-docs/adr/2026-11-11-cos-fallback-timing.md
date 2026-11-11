# ADR 2026-11-11: CoS 不在の直接通知（CosFallback）は一次対応の後だけ送る（全種類）

---
tasks: [01M756VNCZAB8DSPKB1RCPJXTB]
---

- 状態: 採用（実装済み）
- 関係: [2026-10-05-cos-chat-home](2026-10-05-cos-chat-home.md) D6「通知の一本化と退避」（付記）、
  [2026-10-06-cos-inbox-triage](2026-10-06-cos-inbox-triage.md)・[2026-10-06-cos-inbox-triage-fallback](2026-10-06-cos-inbox-triage-fallback.md) 決定 2（付記）、
  [2026-10-29-cos-authorization-fallback-after-triage](2026-10-29-cos-authorization-fallback-after-triage.md)（D3 を置き換える）、
  [2026-10-15-root-done-cos-summary](2026-10-15-root-done-cos-summary.md)（待ち時間の規則を置き換える）

## 1. 文脈

利用者の指摘（2026-11-11）「一次対応が終わってないのに通知来てるような気もする」。本番（operator の読み取り確認）では
2026-11-04 以降の CosFallback 26 件がすべて「CoS の未応答が 120 秒を超えた」。9 件は件の run が動いている間、9 件は同じ
thread の run の後ろで待っている間、8 件は thread も CoS の枠も空いているのに一次対応が始まらないまま送られた。
fallback の後の CoS の判定は「already fallback」で捨てられた（24 件、うち 4 件は observe）。受信箱の一次対応 run の p90 は
139 秒で、到着から 120 秒の期限は一次対応の途中で切れる。root 完了のまとめも同じ規則で 131 件中 74 件が定型文だった。

## 2. 決定

D1. 認可の件と同じ規則を全種類（question・decision・failed・unintegrated・notice・root 完了のまとめなど）に当てる。
CosFallback を作るのは、CoS が escalate せずに件を手放したとき（run が failed・stopped/interrupted の再投入後も未処理・
completed で未処理・run 行なし）、`cos.enabled=false`、または安全網に掛かったときだけ。到着時刻から数える期限は消す。
安全網 `[cos.triage] fallback_after_secs` は次の時刻から数える。
- 件の run が動いている（queued・running・stopping）: その run の `started_at`。
- 同じ thread の別の run が動いていて件がその後ろで待つ: その run の `started_at`。
- thread に動いている run が無い: 件を拾えるようになった時刻（到着と thread の直前の run の `finished_at` の遅い方）。
  容量待ち・`accepting_new_work=false`（standby・draining）・disk guard を含む。

値の決め方: 未設定なら `[cos] max_wall_secs + 300`（既定 900 + 300 = 1200 秒）。run は `max_wall_secs` で worker に止められて
「run が未処理のまま終わった」で送られるので、安全網は止まらない run と始まらない一次対応だけを拾えばよい。300 秒は止められた
run の回収の tick と終端の記録の遅れの余裕。明示する値は `max_wall_secs` より大きくなければ読み込みエラー。

D2. fallback を取った後、notifier が送る前（`notifications.sent_at IS NULL` かつ `ok IS NULL`）に CoS が answer・observe・
escalate したら、CoS の判定を採る。fallback の通知は `ok=0`・`error="withdrawn: CoS judged before the send"` で取り下げる
（行は消さない）。escalate なら CoS の文面の escalation が同じ route 行（`cos_notification_routes` を `route=escalation` に
書き換え）を取り、1 回だけ送られる。送信済みの後の CoS の判定は件を変えず、何も送り直さない。resolve API は 409
（`cos_inbox_revision_conflict`、「既に退避で人へ送った。判定は記録し、送り直さない」）を返し、判定と理由は rejected の
CoS 操作の監査行に残る。POST の最中（送信中）の行は取り下げられない（記録は送信済みになる）。fallback が書いた hand-off の行・
統括 CoS への要約依頼はそのまま残る（記録として）。

D3. `triage_launch` は待ちのある thread を毎 tick SQLite から読む（プロセス内の「待ちがあるかも」の印 `triage.dirty` を消す）。
件を拾わなかった thread には理由（`not_accepting_new_work`・`disk_not_ready`・`cos_disabled`・`cos_skill_drift`（ADR 2026-11-11-cos-skill-sync D3 の門）・`thread_run_live`・`no_cos_slot`・
`thread_run_live_in_db`）を付け、理由が変わったときだけ log（info）に残す。standby → active の切り替え直後、他の process の
run が SQLite に残っていた thread でも、空いた次の tick で拾う。本番の「空いた thread で 120 秒以上拾われない」8 件の原因は
決めきれていない（推測: claim が DB 上の動いている run を見て拒まれたとき `dirty` が消え、新しい件が来るまで拾われなかった）。
この推測の型は試験で再現し、`dirty` の撤去で直した。

D4. root 完了のまとめにも D1 を当てる。CoS の run が書いている間は送らず、run が安全網を超えて動き続けたときだけ定型の短文
にする。run が始まっていない入力は、thread の run の後ろなら その run の開始から、空いていれば入力が積まれた時刻か直前の run の
終わりの遅い方から数える。root 完了の時刻からは数えない。

D5. config の欄の意味が変わるので名前を変える。`[cos.triage] unavailable_after_secs`・`authorization_unavailable_after_secs`
は消し、書いてあれば読み込みエラー（unknown field）。新しい欄は `fallback_after_secs`（省略可）。

D6. web のブラウザ通知（未読の知らせの数が増えるたびに出していた `NotificationsWatcher`）を消す。知らせの増加では出さない。
人への知らせは Discord（CoS の escalate と退避）だけにする。web は未読の badge を出し続ける。CoS の判定に合わせる案は、
画面が CoS の判定を知る経路（Web Push・service worker）が無く、同じ判定を二重に持つことになるので採らない。

## 3. 実装

- `crates/task-ops/src/cos_first_response.rs`: `FirstResponseWait`（`OwnRun`・`BehindRun`・`Pickable`、`queued`・
  `queued_behind`・`overdue`）と `TriageSkip`（`gate`・`as_str`）。純粋な関数で、dispatcher は SQLite から時刻を読んで渡す。
- `crates/task-dispatch/src/dispatcher/cos_chat/fallback.rs`: `unavailable_reason(c, enabled, fallback_after_secs, now)`。
  走査は thread の動いている run の開始と直前の run の終わりも読む。`FallbackDeadlines`・`waits_for_first_response` は消した。
- `crates/task-dispatch/src/dispatcher/cos_chat/triage.rs`・`launch.rs`: `triage_launch` の skip 理由（`launch_skips`）、
  `dirty` の撤去。
- `crates/task-dispatch/src/dispatcher/cos_chat/root_done.rs`: D4。
- `crates/task-core/src/cos_fallback.rs`: `unsent`・`withdraw_unsent`・`judged_from`・`take_route`・`WITHDRAWN`（D2 の SQL）。
  `crates/task-core/src/chat/triage.rs` の `cos_triage_resolve_tx`・`cos_triage_outbox_claim` がこれを同じ transaction で呼ぶ。
  `crates/task-api/src/cos/inbox.rs`（`terminal_conflict`）: 未送信の fallback の件の resolve を受ける。
- `crates/celeris/src/config/cos.rs`: `fallback_after_secs`・`CosConfig::fallback_after_secs()`・`FALLBACK_MARGIN_SECS`。
- web: `web/features/reports/notifications-watcher.tsx`・`notification-rule.ts`（と試験）・`notifications-enable.tsx` を消した。
- 試験: `crates/task-dispatch/src/dispatcher/tests/cos_chat_triage_fallback_timing.rs`（注入した試験時計）、
  `cos_chat_root_done.rs` の `root_done_summary_waits_*`、`crates/task-core/src/chat/triage_tests.rs` の
  `cos_triage_judgment_before_the_send_withdraws_the_fallback`、`crates/celeris/src/config/cos_tests.rs`、
  `web/e2e/parity/reports.spec.ts` の parity-x。

## 4. 本番への移行

本番 config の `[cos.triage] unavailable_after_secs = 120` を消す（残すと起動できない）。`authorization_unavailable_after_secs`
を書いていれば `fallback_after_secs` に名前を変える（未設定なら既定 1200 秒）。手順は進捗ファイル
`agent-docs/progress/2026-11-11-cos-fallback-timing.md` の「本番への移行」節。
