# ADR 2026-10-16: 元の待ちが閉じた escalated item は resolved にする

---
tasks: [01M51B9AFNAQ8FVXM8WAJ5B3H4]
---

- 日付: 2026-10-16
- 状態: 採用・実装済み
- 関連: 2026-10-06-cos-inbox-triage-fallback、2026-10-15-credential-wait-skip-triage、2026-10-11-breaking-changes-by-default

## 背景
人の inbox（GET /api/v1/inbox）が 0 件なのに GET /api/v1/cos/inbox?state=escalated が 257 件あった。
`cos_triage_reconcile` の stale 判定が `state IN ('pending','running')` だけで、人に渡した（escalated）後に元の待ちが閉じても
item が残り、受信箱 thread と digest に未処理として出続けた。また credential_wait は reconcile 対象から外れており
（直送、上記 ADR）、同じ理由で escalated のまま残った。

## 決定
- stale 判定（`resolve_stale`）の対象 state を `pending`／`running`／`escalated` にする。現在の未解決 snapshot に
  (source_key, source_revision) が無ければ `resolved`。開いたままの待ちは escalated のまま。
- `cos_triage_resolve_stale` を追加し、reconcile から外れている credential_wait は dispatcher が挿入せずに解消だけ行う
  （挿入は従来どおり `direct_escalate`。CoS run は起きない）。
- 受信箱 thread・digest は `cos_triage_open_items`（state が escalated/fallback/pending/running）から読むので、
  resolved になれば自動で消える。画面・digest の変更は不要。
- `fallback` は対象外（CoS が判断できず人に渡した別経路。今回の症状ではない）。
- events は書き換えない。migration は不要（次の reconcile が既存行を片付ける）。

## 試験
`task-core chat::triage_tests`: `cos_chat_triage_reconcile_resolves_escalated_when_source_closed`（閉じた待ち→resolved、
開いたまま→escalated）、`cos_chat_triage_resolve_stale_does_not_insert`。
