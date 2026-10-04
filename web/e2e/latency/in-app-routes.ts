import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { fixtureFor } from "../support/fake-daemon.mjs";

// S1 で nav にリンクの無い経路（ADR-0081 付記「リンクの無い経路」）を、アプリ内の遷移で測るための表。
// 親画面を開き、起動の完了を待ってから、その画面の実リンク（main 内の a[href=fixture]）を click する。
// fixtures は親画面にリンクを描かせるための偽 daemon の応答（e2e/support/screens.ts は変えない）。
// 表に無い経路は、どの画面にも SPA のリンクが無い（/org/cos の「話す」は文書の再読み込みになる
// 素の a、files・changes は ?tab= のリンクだけ）ので、起動完了の後に history.pushState と
// popstate でアプリ内遷移させる。

const schema = JSON.parse(
  readFileSync(path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../api/generated/schema.json"), "utf8"),
);

const task = {
  id: "T1",
  title: "一件目",
  status: "ready",
  kind: "execute",
  category: "work",
  tier: "standard",
  actions: [],
  attempts: 0,
  children: 0,
  conversation: false,
  created_at: "2026-09-30T00:00:00Z",
  depends_on: [],
  labels: [],
  max_retries: 0,
  pending_children: 0,
  priority: 0,
  priority_label: "normal",
  updated_at: "2026-09-30T00:00:00Z",
};

const project = {
  id: "P1",
  title: "一件目",
  request: "一件目 の依頼",
  status: "active",
  created_at: "2026-09-30T00:00:00Z",
  updated_at: "2026-09-30T00:00:00Z",
};

function taskDetail() {
  const value = fixtureFor(schema.$defs.TaskDetail) as { task: Record<string, unknown>; runs: unknown[] };
  value.task = { ...value.task, id: "T1" };
  const run = fixtureFor(schema.$defs.RunSummary) as Record<string, unknown>;
  value.runs = [{ ...run, run_id: "R1", adapter: "claude-code", finished_at: "2026-09-30T00:00:00Z" }];
  return value;
}

function projectDetail() {
  const value = fixtureFor(schema.$defs.ProjectDetail) as { project: Record<string, unknown> };
  value.project = { ...value.project, ...project };
  return value;
}

export type InAppRoute = { parent: string; fixtures?: Record<string, unknown> };

/** fixture（遷移先の URL）→ その URL への実リンクを持つ親画面。 */
export const inAppRoutes: Record<string, InAppRoute> = {
  "/tasks/T1": {
    parent: "/tasks",
    fixtures: { "/api/v1/tasks": { items: [task], total: 1, next_cursor: null, counts_by_status: { ready: 1 } } },
  },
  "/tasks/T1/runs/R1": { parent: "/tasks/T1", fixtures: { "/api/v1/tasks/T1": taskDetail() } },
  "/tasks/new": { parent: "/help" },
  "/projects/P1": { parent: "/projects", fixtures: { "/api/v1/projects": { items: [project] } } },
  "/projects/P1/docs": { parent: "/projects/P1", fixtures: { "/api/v1/projects/P1": projectDetail() } },
  "/projects/P1/docs/maintenance": { parent: "/projects/P1/docs" },
  "/knowledge/inbox": { parent: "/knowledge" },
  "/knowledge/skills": { parent: "/knowledge" },
};
