# ADR 2026-10-06: CoS 受信箱の投入・通知 route・切替の永続化

---
tasks: [01M47VN95D8QQ65ATZVGHFFXA7]
---

## 決定

ADR 2026-10-05 CoS chat D3/D6 の store 契約を次のように固定する。

- 入力の同一性は `(source_kind, source_key, source_revision)`。revision は元の判断待ちの version、なければ発生 event id。title や既読の変更は revision に含めない。CoS operation id が付いた入力は捨てる。
- cursor は既存 `feed_cursor` の `cos_triage:<source>` に保存する。各 batch の item INSERT と cursor 更新を一つの IMMEDIATE transaction にする。再投入は既存行に触れない。cursor は呼出側が同じ source 内で単調増加させる。起動時の reconciliation は現在の未解決待ち集合と保存済み triple だけを比較し、events を読まない。
- item は `pending → running → answered / observed / escalated / fallback / resolved`。停止した run の item は `running → pending` として回収できる。別 revision は別行。状態更新時には元の待ちが今も未解決かを呼出側が確認する。
- claim は作成順に 20 件までを一 transaction で選び、受信箱 thread の system message に item id を列挙する。同じ transaction で run の `input_message_id` を設定する。item ごとの resolve は独立。
- 通知は `cos_notification_routes` の triple UNIQUE を arbitration point とする。escalation と fallback は同じ claim 関数を使い、既存 `notifications` に 1 行だけ作る。送信直前に元の待ちを再照合し、解決済みなら未送信通知を取り下げる。sent 履歴は消さない。外部 POST と DB commit の完全な原子性は保証できず、notifier は既存の再送規則を使う。
- 切替は `feed_cursor` に route 版 `cos_v1` と source cursor を保存する transaction。旧未送信通知の source を item に対応付ける `cos_legacy_notification_links` を追加し、`ok=0,error='superseded'` とする。sent 行はそのまま。初回 reconciliation 完了も `feed_cursor` に記録し、導入時の未解決待ちは一度だけ照合する。

## 設計上の境界

store は元の判断待ちが解決済みかを推測しない。dispatcher は現在の未解決集合を渡し、notifier は送信直前に元 source を再確認する。時刻は呼出側から渡す。イベント全履歴の tick ごとの走査はしない。
