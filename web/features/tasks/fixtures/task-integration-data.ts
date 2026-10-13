import type { InboxItem, IntegrationView } from "../../../api/generated/types";

// The public repo name deliberately differs from the durable repo ID.
export const integration: IntegrationView = {
  actor: "human",
  branch: "task/work",
  checked_at: "2026-10-11T00:00:00Z",
  head: "abcdef0123456789",
  repo: "web 日本語",
  repo_id: "repo-1",
  state: "unintegrated",
};

export const inboxItem: InboxItem = {
  age_secs: 0,
  answer: {
    body_schema: {},
    method: "POST",
    path: "/api/v1/inbox/items/unintegrated-T1-repo-2/answer",
    native: { method: "POST", path: "/api/v1/tasks/T%201/integrations/another%20repo/merge" },
  },
  blocked_by: [],
  blocking: { summary: "", tasks: [], units: [] },
  created_at: "2026-10-11T00:00:00Z",
  id: "unintegrated-T1-repo-2",
  kind: "unintegrated",
  links: [],
  options: [],
  task: { id: "T 1", title: "task", kind: "execute", status: "done", actions: [] },
  title: "未取り込み",
  detail: "repository another repo",
};
