# web ADR-W2: /projects/:id の計画・途中目標の intent を ADR-0079 の後継 API に写す

---
tasks: [01M3SY0ZF3NNAMBNGTTZPEMJYK]
---

- Date: 2026-09-30
- Status: Accepted
- 関連: ADR-0081（web SPA）、ADR-0079 D13（途中目標は凍結した履歴、書き込みの入口は 410）

## 文脈

feature-parity.md の R10 は `project_plan` `project_plan_decide` `milestone_*` を intent として挙げるが、
celeris は ADR-0079 D13 以降それらの書き込み入口（`POST /projects/{id}/plan`・`/project-plan/{v}/decide`・
`/milestones…`）に 410 を返す。旧 gui/ は中継を外した。R28（`/plans/new`）と同じく後継に写すかを決める。

## 決定

web/ の `/projects/:id` は、410 の入口を呼ばずに docs/celeris-api-v1.md の「後継」列に写す。

| intent | 呼ぶもの |
|---|---|
| `project_plan` | `POST /tasks`（`project_id` 付きの root task。段階の名指しは `stages_hint`） |
| `project_plan_decide` | `POST /tasks/{root}/execution/plan-gate`（`approve` / `replan` / `withdraw`） |
| `milestone_create` | `POST /tasks`（`project_id` と 1 段の `stages_hint`） |
| `milestone_status` / `milestone_decide` | `POST /tasks/{id}/execution/phase-gate`（`continue` / `replan` / `withdraw`） |
| `milestone_pause` / `milestone_resume` / `milestone_cancel` | `POST /tasks/{id}/{pause,resume,cancel}` |

凍結した途中目標（`milestones_frozen`）は読み取り専用の件数と一覧だけを出す。
`project_plan_proposed` / `project_plan_decided` のイベントは ADR-0081 D6 の表どおり `projects/detail/<id>` と
`projects/plan/<id>` を stale にする（`web/api/realtime/invalidation-map.ts`、変更なし）。

## 結果

旧 GUI に無い操作（計画の承認・段の判定）を案件の画面から行える。API は変えない（ADR-0081 H1）。
