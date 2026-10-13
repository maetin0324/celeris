# Views WorkUnit progress

---
tasks: [01M4MRE949FE9DWVT26NDF0T8V]
---

- `task_ops::view::IntegrationView` and optional `integrations` / `promotion` fields were added to `TaskSummary` and `TaskDetail`; callers can attach durable rows through `attach_integrations` when an optional `task_ops::stranded::IntegrationContext` is available. Without context, fields are omitted.
- `AttentionItem::Unintegrated` now carries the repository name, state, and native operation base. Human inbox options no longer contain the old description-only `/integration/{merge,discard,hold}` URLs. The native merge path is `/api/v1/tasks/{task_id}/integrations/{repo_name}/merge`; matching discard, hold, and reopen routes are fixed as `/api/v1/tasks/{task_id}/integrations/{repo_name}/{discard|hold|reopen}` for the route WorkUnit.
- The API state/config wiring and route handlers remain for the later API WorkUnits. `IntegrationContext.default_branch` is reserved for that wiring/operation context; view construction does not infer repository config.
- Schema generation and generated GUI/web types are pending; GUI offline installation currently fails because pnpm cannot open its default cache database in this sandbox.
