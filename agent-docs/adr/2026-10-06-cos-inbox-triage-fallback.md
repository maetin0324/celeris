# ADR 2026-10-06: CoS 受信箱の不在退避（D6）の検出と回収

- 状態: 採用（実装済み）
- 関係: [2026-10-05-cos-chat-home](2026-10-05-cos-chat-home.md) D6「通知の一本化と退避」、[2026-10-06-cos-inbox-triage](2026-10-06-cos-inbox-triage.md)（store 契約）、[2026-10-06-cos-inbox-triage-ingest](2026-10-06-cos-inbox-triage-ingest.md)

## 決定

1. 検出は dispatcher の tick で SQLite だけを見る（LLM 無し）。`cos_inbox_items` の `pending/running` 行を `chat_runs` と `cos_notification_routes` に LEFT JOIN して読む（`idx_cos_inbox_items_state` に乗る）。派生 inbox の build は退避する行が在る pass に 1 回だけ。
2. 不在の条件（上から順に判定）: `cos.enabled=false` → 即退避。item の run が `failed`（reason に quota/login/no usable account を含めば「全候補 quota 切れ・ログイン不可」）、`stopped/interrupted`、`completed`（成功終了でも未処理）、run 行が無い（crash）→ 退避。それ以外（pending・run 稼働中・容量待ち・`accepting_new_work=false`・disk guard）は item の `created_at` から `unavailable_after_secs` を超えたら退避。run の wall-clock 上限とは独立。
3. 退避は store の `cos_triage_outbox_claim(item,"fallback",body)` を通す（escalation と同じ triple 一意の arbitration）。claim が取れなければ escalation か解決が先に勝ったので何もしない。取れたら item を `fallback` にし、受信箱 thread に keyed system message（`cos-fallback:<item_id>`）で「人へ委ね済み・再送も代答もしない」を残す。`cos_triage_claim` は `pending` しか取らないので、CoS 復帰後も同じ revision を再び CoS に渡さない。
4. 元の待ちが既に派生 inbox に無ければ通知せず `resolved` にする。notice は情報なので常に退避対象（web path は `/notifications`）。
5. 本文は escalation と同じ JSON envelope に `route=fallback`・`headline`（「CoS 不在のため直接通知」）・`unavailable_reason`・要点・既存選択肢・推奨（元の待ちの推奨があればそれを「CoS の推奨ではない」と明記、無ければ「CoS の推奨なし」）・`web_path`（resolve API の導出と同じ）・定型文 `text`（1900 字以内）を入れる。自動 approve はしない。
6. 再起動時の回収: 検査は DB だけなので未解決行は次の tick で拾う。claim 済みで state の記録が crash で失われた行（route=fallback かつ pending/running）は claim をやり直さず state だけ `fallback` にする。route=escalation の行は resolve API の持ち物なので触らない。
7. 退避の outbox は `notifications` 行であり notice ではないので、送信失敗が新しい CoS 待ちを作らない。
