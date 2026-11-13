---
tasks: [01M791DFQGCDG46CCR5KWNDDQA]
---
# ADR 2026-11-12: Discord は人か operator の対応が要ると決まった件だけに送る（古い revision を閉じ、推奨が人の選択肢でなければ CoS が答える。policy_version 13）

- 状態: 採用。D1・D2・D5 は実装済み、D3・D4 は運用の規則（[進捗](../progress/2026-11-12-discord-person-or-operator.md)）。
- 関係: [ADR 2026-11-12 cos-triage-scope](2026-11-12-cos-triage-scope.md)（人に残す件の列挙 D1、operator 宛て D2）、
  [ADR 2026-11-11 退避の時刻](2026-11-11-cos-fallback-timing.md)、[ADR 2026-10-29 認可の退避](2026-10-29-cos-authorization-fallback-after-triage.md)、
  [ADR-0133](0133-inbox-and-notifications.md)、[ADR 2026-10-27 外部 operator](2026-10-27-external-operator.md)、`docs/ops/external-operator.md`。

## 背景

人の依頼（2026-11-13 08:2x JST）:「人間や operator の外部セッションが対応する必要があるものだけ通知に出るように」。人が Discord
（08:21 JST）を見て画面を開くと、件は既に答えられていた。

直近 7 日（2026-11-05T23:30Z〜11-12T23:30Z）の CoS 経路の Discord 送信は 155 件で、人が答えたのは 1 件、外部 operator が答えた件が
69 件、CoS が後で閉じた件が 4 件だった（分類は task の artifacts の `discord-7d-report.md`）。

- 事例 1（task 01M782BS の認可、policy 12）: 推奨の案 2 は人が承認した node-h の枠 20 の内（0.167 node-h、合計 19.70）で、人だけが
  決める部分は「欠測のまま閉じるか（研究上の判断）」と「上限を上げるか（承認を超える資源）」だった。CoS は件全体を人に回し、
  Discord の 47 秒後に operator が案 2 で答え、人の判断は後で新しい認可として聞くよう worker に指示した。
- 事例 2（知らせ 01M764DJ8X、rev 2 と rev 3）: 昇格の直後に同じ知らせの 2 つの revision が別々の件になり、同じ run に入った。CoS は
  rev 3 を見届けたが、rev 2 の resolve は 409 で閉じられず、run の終了後に退避の行ができた。Discord は送る直前の再確認で取り下げられ
  たが、item は `fallback` のまま残り（統括 CoS の窓の札・CoS への要約の依頼も残る）、人の待ちに見えた。7 日間で同じ形の退避の行は
  古い revision に 4 回、閉じた待ちに 5 回できた。

## 決定

### D1 古い revision は新しい revision の判定で閉じ、退避も escalate も取れない

- 新しい revision の有無の判定は `task_core::cos_revision::superseded`（同じ元の待ちに、後の `created_at` の item がある。同時刻は id で
  決める）1 つにする。resolve の API（409）、経路の取り合い（`cos_triage_outbox_claim`）、送る直前の再確認（notifier）が同じ関数を使う。
- item の結果（answered・observed・escalated・fallback・resolved）を記録すると、同じ元の待ちの古い revision で `pending`・`running`・
  `escalated`・`fallback` のものを `resolved`（reason「新しい revision の判定で閉じた」）にし、その未送信の通知を取り下げる
  （`error = superseded`）。送った通知は送ったまま。
- 経路の取り合いは、古い revision を `resolved` にして経路を取らない（通知の行・札・要約の依頼ができない）。
- 退避は、知らせが既読か束が育った（`task_core::feed::notice_revision_open` が `Some(false)`）なら経路を取らずに閉じる。知らせが
  見つからないときは閉じたと決めず、今までどおり退避する（notifier はそれを送らない）。
- 送る直前の再確認で元の待ちが閉じていた退避は、通知の取り下げと同じ transaction で item を閉じる（`fallback` を閉じる sweep は無い）。
  escalated の item は今までどおり triage の stale sweep が閉じる。
- 判定と閉じる SQL は `task_core::cos_revision` に置く（`cos_fallback` と同じ形。`cos_inbox_items`・`cos_notification_routes`・
  `notifications` の SQL だけ）。書く関数（`close_if_superseded`・`after_judgment`）は crate の中だけで、chat の triage store が
  自分の transaction の中で呼ぶ。状態の正本は `cos_inbox_items` の 1 つのまま。
- 再確認から POST までの秒単位の間に閉じた件は防げない（7 日間で 1 件）。送った後に閉じた件の web の札は「既に答えられたか、失効」と出る。

### D2 人宛ての escalate は人に残す選択肢を名指し、推奨がその外なら CoS が先に答える

- escalation packet に `reserved_options`（人だけが選べる `options[].key`。cos-triage-scope D1 の分類に当たる選択肢）を足す。
  `to = human` では必須。`recommended` がその外なら API が 422 `cos_answer_unreserved_recommendation` で断り、経路も通知も作らない。
  CoS は推奨を answer し（認可は `within_approved_frame`、ほかは決まった category）、人に残す部分を `answer.note` に「後で人に聞く問い」
  として書く。worker はその区切りで新しい問いを出し、その問いは推奨が人の選択肢になるので人に回る。
- 推奨を出せない件（`recommended = null`）、推奨が人の選択肢の件は今までどおり人に送る。人が `human_required` を付けた件・override の
  待ち・照合（cos-triage-scope D4）が人に残すと決めた件は件全体が人のもので、`reserved_options` は空でもよい。
- `to = operator` では `reserved_options` を空にする（入れると 422。人の選択肢を operator に渡さない）。
- 判定は API の決定的な検査で、どの選択肢が人のものかは CoS が書く。CoS が推奨を人の選択肢に入れれば送られる。監査の payload に
  `reserved_options` が残るので、review と台帳で確かめられる。
- 試験: 事例 1 の形は Discord の行ができない（`answer_first_case1_in_frame_recommendation_is_answered_and_not_sent`）。枠を超える推奨・
  研究上の判断の推奨・`human_required` の件は人に送られる（`answer_first_reserved_matters_still_reach_the_person`）。credential の待ちは
  CoS を通らない直経路で送られる（既存の `credential_wait_notify`）。

### D3 operator の外部 session の対応が要る件は Discord に送らない

- `to = operator` の件は今までどおり通知の行を作らず、Discord に送らない。operator session は
  `GET /api/v1/cos/inbox?state=escalated` の `addressee = "operator"` を見て拾う（`docs/ops/external-operator.md` §1。operator session の
  受信箱の見張りが新しい件で起きる）。
- 理由: Discord は人の手を止める割り込みで、operator 宛ての件は人の判断が要らない。印を分けて同じ Discord に送ると、人は「自分が
  見なくてよい通知」を毎回読むことになり、今回の苦情と同じ形になる。
- operator session が動いていないと件は残る。それは operator の運用（見張りの起動）で扱い、人への Discord にはしない。人が知るべき
  滞留は、件の停滞の知らせ（`human_wait_aging` など）を CoS が判定する既存の経路で上がる。

### D4 operator session が人宛ての件を先に答える場合

- operator は人宛て（`to = human`、`addressee = human`）の件を自分の判断では答えない。答えてよいのは、人の発言を取り次ぐとき
  （`instructed_by` に人の発言の id。ADR 2026-10-27-relayed-instructions）だけ。
- 事例 1 のように、人宛ての件の一部が CoS の範囲（枠の内の推奨）だったと operator が気づいたときは、件に答えず、CoS への指示
  （統括 CoS のチャット）か CoS の方針の直しの task で扱う。送った Discord は取り消せないので、人が開いた画面で既に答え済みになる
  ことを防ぐには、CoS が人に送る前に答える（D2）しかない。
- 例外として operator が先に答えた場合、web の札は「既に答えられたか、失効」と出て、events の `by = operator:claude-code` が残る。
  追いかけの Discord は送らない（割り込みを増やさない）。

### D5 policy_version 13

- skill cos-inbox-triage を version 13 にし（§0・§3 の規則、§6 の `reserved_options`、§7 の送る直前の再確認と古い revision）、
  `[cos.triage] policy_version` の既定を `"13"` にする。CoS の常設の文脈（ADR 2026-11-11-cos-standing-context D5）の方針の要点にも
  同じ規則を書く。
- 本番は運用者が config の `[cos.triage] policy_version` を `"13"` にする（進捗の「本番への移行」節）。版 13 の skill の同期の前に config
  だけを上げると、CoS の claim の門（ADR 2026-11-11-cos-skill-sync）が CoS を止める。

### D6 撤去

- notifier（`celeris::notify::triage::source_unresolved`）の新しい revision の SQL（`created_at` だけを比べ、同時刻の扱いが API と違った）と、
  知らせを 500 件ずつ読む loop。D1 の 2 つの関数に置き換えた。
- task-api の `triage_view::superseded` の SQL。store の関数を呼ぶだけにした。
- `reserved_options` の無い人宛ての escalate（版 12 以前の packet の形）。API が 422 で断る。互換の分岐は置かない。
