# ADR 2026-10-16: 元の待ちが閉じた escalated item は resolved にする

---
tasks: [01M51B9AFNAQ8FVXM8WAJ5B3H4]
---

- 日付: 2026-10-16
- 状態: 採用・実装済み
- 2026-10-22 付記（[2026-10-22-cos-fixed-threads](2026-10-22-cos-fixed-threads.md)）: `cos_notification_routes` は変更なし。escalation の通知本文に CoS の名（`cos`）を足した。
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

## 付記 2026-10-16: `InboxKind::ALL` の取りこぼし（unintegrated）
本番で上の修正後も、escalated のうち unintegrated 46 件が残った（元 task は全て done で integration_merged／discarded 済み）。
原因: `InboxKind::ALL: [InboxKind; 16]` に `Unintegrated` が無く（enum は 17 種）、`triage_reconcile` は ALL から `by_kind` を作る。
unintegrated が現在 0 件だと by_kind に空の枠が無く、`cos_triage_reconcile("unintegrated", [])` が呼ばれず resolve_stale が働かない。
- 決定: ALL を 17 要素にして `Unintegrated` を加える。
- 再発防止: `task-ops human_inbox` の試験で、全 variant を網羅する `match`（新 variant を足すとコンパイルが落ちる）から作った一覧と ALL を突き合わせる。
- 影響: `task-api inbox_notifications` の kind 検証は ALL を使うので、`kind=unintegrated` が有効な絞り込みになる（これまで 400 だった。不具合の解消）。
  `human_inbox_every_inbox_kind_appears_in_common_form` は ALL を回すので、fixture の unintegrated 1 件が検証対象に入る。
- 試験: dispatcher の triage 試験に、閉じた escalated unintegrated item が reconcile で resolved になるものを足す。
- 本番への移行: migration 不要。昇格後の startup reconcile が既存の 46 件を resolved にする。

## 付記 2026-10-16b: event 取り込みでも、触れた task の stale item を resolved にする
上の 2 つの修正後も、daemon 起動後に escalated になり同時刻に人が取り込んで閉じた unintegrated 2 件が 45 分後も残った。
`resolve_stale` は `triage_reconcile`（起動時と credential_gate の `request_cos_triage_reconcile`）でしか走らず、
`triage_events`（events_since → touched_tasks → sources_for_touched → cos_triage_ingest_batch）は挿入しかしないため。
- 決定: `triage_events` で触れた task があるとき、構築済みの現在の派生 inbox を使い、`cos_triage_resolve_stale_touched` を呼ぶ。
  判定は `resolve_stale`（reconcile と同じ関数）を共有する。二重の規則は作らない。
- 範囲: cos_inbox_items に task 列は無いので、触れた task の id を `source_key` が含む item に限る
  （failed-/unintegrated-/question-/phase_gate- など key に task id を持つ種別）。触れていない task の item と、現在の inbox に残っている item は変えない。
  key に task id を持たない種別（decision・authorization 等）は次の startup reconcile が片付ける。
- 対象 state は reconcile と同じ pending／running／escalated。events は書き換えない。migration 不要。
- 試験: dispatcher の triage 試験に、(a) reconcile 後に閉じた escalated item が次の tick で resolved、(b) 触れていない task の item と開いたままの item は変わらない、を足す。
