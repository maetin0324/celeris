# Web stranded integration status

tasks: [web-status]

## Status

Stopped before implementation because the integration API contract required by this unit is incomplete in the current worktree. Per the task constraints, the web client must not infer a route or reuse the unrelated ADR-0043 `task_integrations` changes route.

## Available contract

- Generated `TaskSummary` and `TaskDetail` include `integrations?: IntegrationView[] | null` and `promotion?: PromotionView | null`.
- `IntegrationView` exposes repo id/name, state (`unintegrated | integrated | discarded | held`), reason, actor, branch, head, and checked time.
- Inbox `unintegrated` items include `operation_path: string`, but the generated API does not define an operation-specific request body for the stranded flow.
- ADR `agent-docs/adr/2026-10-11-stranded-done-branches.md` requires merge/discard/hold/reopen, with required notes for discard and hold.

## Missing contract

The current generated schema/types only describe `IntegrateBody` for the legacy `/tasks/{id}/changes/{repo}/integrate` route, with methods `merge | pr | discard`. There is no generated contract for the stranded integration operation that supports `merge | discard | hold | reopen`, including its route, payload, and result. Consequently the inbox `operation_path` cannot safely be turned into the required actions, and using the legacy route would violate the task's explicit separation from ADR-0043 integrations.

No web implementation or tests were added. Resume this unit after the integration-api child publishes the stranded operation contract and regenerates web API types.
