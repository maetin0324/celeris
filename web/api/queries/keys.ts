// domain 別の Query key factory（ADR-0081 D5）。key はここでだけ作る。
// key は ID と正規化した search / filter / page / offset を含む。バッジ用と画面用で別の key を作らない
// （バッジは同じ key の query に select を掛ける。badges.ts）。

import { type Filters, normalizeFilters } from "./normalize";

const f = normalizeFilters;

export const taskKeys = {
  all: ["tasks"] as const,
  lists: () => ["tasks", "list"] as const,
  list: (filters?: Filters) => ["tasks", "list", f(filters)] as const,
  detail: (taskId: string) => ["tasks", "detail", taskId] as const,
  timelines: (taskId: string) => ["tasks", "timeline", taskId] as const,
  timeline: (taskId: string, filters?: Filters) => ["tasks", "timeline", taskId, f(filters)] as const,
  runs: (taskId: string) => ["tasks", "runs", taskId] as const,
  runsOf: (taskId: string) => ["tasks", "run", taskId] as const,
  run: (taskId: string, runId: string) => ["tasks", "run", taskId, runId] as const,
  execution: (taskId: string) => ["tasks", "execution", taskId] as const,
  files: (taskId: string) => ["tasks", "files", taskId] as const,
  changes: (taskId: string) => ["tasks", "changes", taskId] as const,
  artifacts: (taskId: string) => ["tasks", "artifacts", taskId] as const,
};

export const projectKeys = {
  all: ["projects"] as const,
  lists: () => ["projects", "list"] as const,
  list: (filters?: Filters) => ["projects", "list", f(filters)] as const,
  detail: (projectId: string) => ["projects", "detail", projectId] as const,
  tasksOf: (projectId: string) => ["projects", "tasks", projectId] as const,
  tasks: (projectId: string, filters?: Filters) => ["projects", "tasks", projectId, f(filters)] as const,
  plan: (projectId: string) => ["projects", "plan", projectId] as const,
  docs: (projectId: string, path: string) => ["projects", "docs", projectId, path] as const,
};

/** 一覧系の domain（D5 の 5,000 / 10,000 / 60,000 の行）。`all` で domain 全体を指す。 */
function listDomain<const D extends string>(domain: D) {
  return {
    all: [domain] as const,
    list: (filters?: Filters) => [domain, f(filters)] as const,
  };
}

/**
 * 受信箱。`list` は旧 `GET /inbox`（互換）、`itemList` / `item` は ADR-0133 の `GET /inbox/items`。
 * どちらも `['inbox']` の下にあるので、`inboxKeys.all` の invalidate で両方が stale になる。
 */
export const inboxKeys = {
  ...listDomain("inbox"),
  items: () => ["inbox", "items"] as const,
  itemList: (filters?: Filters) => ["inbox", "items", f(filters)] as const,
  item: (itemId: string) => ["inbox", "item", itemId] as const,
};

/** 通知（ADR-0133 D3・D5）。束の一覧と未読数。 */
export const notificationKeys = {
  all: ["notifications"] as const,
  lists: () => ["notifications", "list"] as const,
  list: (filters?: Filters) => ["notifications", "list", f(filters)] as const,
  unreadCount: () => ["notifications", "unread-count"] as const,
};
export const boardKeys = listDomain("board");
export const reportKeys = listDomain("reports");
export const approvalKeys = listDomain("approvals");

export const daemonKeys = {
  all: ["daemon"] as const,
  /** REST の正本。 */
  rest: () => ["daemon", "rest"] as const,
  /** SSE の生 snapshot 専用。REST の代わりにしない。 */
  stream: () => ["daemon", "stream"] as const,
};

export const healthKeys = { all: ["health"] as const };

export const providerKeys = listDomain("providers");
export const accountKeys = listDomain("accounts");
export const clusterKeys = listDomain("clusters");
export const releaseKeys = listDomain("releases");
export const metricKeys = listDomain("metrics");

export const orgKeys = listDomain("org");
export const knowledgeKeys = listDomain("knowledge");
export const skillKeys = listDomain("skills");
export const configKeys = listDomain("config");
export const mcpKeys = listDomain("mcp");

export const consoleKeys = {
  all: ["console"] as const,
  conversation: (scope: string, conversationId: string) => ["console", scope, conversationId] as const,
};

export const queryKeys = {
  tasks: taskKeys,
  projects: projectKeys,
  inbox: inboxKeys,
  notifications: notificationKeys,
  board: boardKeys,
  reports: reportKeys,
  approvals: approvalKeys,
  daemon: daemonKeys,
  health: healthKeys,
  providers: providerKeys,
  accounts: accountKeys,
  clusters: clusterKeys,
  releases: releaseKeys,
  metrics: metricKeys,
  org: orgKeys,
  knowledge: knowledgeKeys,
  skills: skillKeys,
  config: configKeys,
  mcp: mcpKeys,
  console: consoleKeys,
} as const;
