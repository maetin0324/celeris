# ADR-0082: ADR-0081 D6 の表に無い task.event 12 種の invalidate 範囲

---
tasks: [01M3RQSXY0306GYMM19G5VTEQE]
---

- Date: 2026-09-30
- Status: Accepted
- 関連: ADR-0081 D6（SSE イベント種類ごとの invalidate 範囲）

## 文脈

ADR-0081 D6 の表は task.event を 36 種として書いたが、`web/api/generated/schema.json` の EventRow には 48 種ある。
`web/api/realtime/invalidation-map.ts` は `Record<EventKind, …>` で全種を要求し、schema と突き合わせるテストもあるので、残り 12 種にも範囲を決める必要がある。
EventRow に project_id を足す API 変更はしない（ADR-0081 H1）。

## 決定

記号は D6 と同じ（T=task detail+timeline、L=tasks/list・inbox・board、P=project 集計、N=reports・approvals・daemon/rest、R=runs、E=execution）。

| 種類 | 範囲 | 理由 |
|---|---|---|
| `browser_updated` `browser_wait_opened` `browser_wait_resolved` | T R N | run 中の browser 状態。承認待ちの出入りがあるので N を含む。一覧・project は変えない |
| `work_unit_spec_overridden` `unit_gate_overridden` | T E R L P | execution plan の変更。D6 の plan 系にならう |
| `child_task_created` `child_adopted` | T E R L P と子 task の detail | 木の形が変わる。子 task の id は payload にある |
| `decision_requested` `decision_answered` `decision_withdrawn` | T L N | inbox・承認の件数が変わる |
| `plan_approval_requested` | T L N E | 承認待ちに入る plan。execution も更新 |
| `stall_detected` | T R L | run の停滞表示。project 集計は変えない |

project が要る種類は D6 と同じく Query cache から解決し、不明なら project の集計 key を stale にする。

## 結果

schema に種類が増えると型検査と `invalidation-map.test.ts` が落ち、この表か D6 に追記するまで先へ進めない。
