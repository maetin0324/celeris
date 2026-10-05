import { readFileSync } from "node:fs";
import http from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";

const schema = JSON.parse(
  readFileSync(path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../api/generated/schema.json"), "utf8"),
);
const reservedPorts = new Set([7700, 7701, 7710, 7711, 7712]);
const delayValues = new Set([0, 5000, 10000]);

function resolve(node) {
  return node.$ref ? schema.$defs[node.$ref.split("/").at(-1)] : node;
}

export function fixtureFor(node, seen = new Set()) {
  node = resolve(node);
  if (Object.hasOwn(node, "const")) return node.const;
  if (node.enum) return node.enum[0];
  if (node.anyOf || node.oneOf) return fixtureFor((node.anyOf ?? node.oneOf)[0], seen);
  if (node.allOf) return Object.assign({}, ...node.allOf.map((part) => fixtureFor(part, seen)));
  const kind = Array.isArray(node.type) ? (node.type.find((item) => item !== "null") ?? "null") : node.type;
  if (kind === "object" || node.properties) {
    if (seen.has(node)) throw new Error("recursive required fixture schema");
    const next = new Set(seen).add(node);
    return Object.fromEntries((node.required ?? []).map((key) => [key, fixtureFor(node.properties[key], next)]));
  }
  if (kind === "array") return [];
  if (kind === "string") return node.format === "date-time" ? "2026-01-01T00:00:00Z" : "fixture";
  if (kind === "number" || kind === "integer") return Math.max(0, node.minimum ?? 0);
  if (kind === "boolean") return false;
  if (kind === "null") return null;
  throw new Error(`unsupported required fixture schema: ${JSON.stringify(node)}`);
}

export function validateFixture(value, original, location = "$", errors = []) {
  const node = resolve(original);
  if (node.anyOf || node.oneOf) {
    if (!(node.anyOf ?? node.oneOf).some((part) => validateFixture(value, part).length === 0))
      errors.push(`${location}: no matching variant`);
    return errors;
  }
  if (node.allOf) {
    for (const part of node.allOf) validateFixture(value, part, location, errors);
    return errors;
  }
  if (Object.hasOwn(node, "const") && value !== node.const) errors.push(`${location}: const`);
  if (node.enum && !node.enum.includes(value)) errors.push(`${location}: enum`);
  const kinds = Array.isArray(node.type) ? node.type : [node.type];
  const valid = kinds.some(
    (kind) =>
      (kind === "object" && value !== null && typeof value === "object" && !Array.isArray(value)) ||
      (kind === "array" && Array.isArray(value)) ||
      (kind === "integer" && Number.isInteger(value)) ||
      (kind === "number" && typeof value === "number") ||
      (kind === "string" && typeof value === "string") ||
      (kind === "boolean" && typeof value === "boolean") ||
      (kind === "null" && value === null),
  );
  if (node.type && !valid) {
    errors.push(`${location}: type`);
    return errors;
  }
  if (typeof value === "number" && node.minimum !== undefined && value < node.minimum)
    errors.push(`${location}: minimum`);
  if (typeof value === "string" && node.pattern && !new RegExp(node.pattern).test(value))
    errors.push(`${location}: pattern`);
  if (Array.isArray(value) && node.items)
    value.forEach((item, i) => {
      validateFixture(item, node.items, `${location}[${i}]`, errors);
    });
  if (value !== null && typeof value === "object" && !Array.isArray(value)) {
    for (const key of node.required ?? []) if (!Object.hasOwn(value, key)) errors.push(`${location}.${key}: required`);
    for (const [key, item] of Object.entries(value)) {
      if (node.properties?.[key]) validateFixture(item, node.properties[key], `${location}.${key}`, errors);
      else if (node.additionalProperties === false) errors.push(`${location}.${key}: unexpected`);
      else if (typeof node.additionalProperties === "object")
        validateFixture(item, node.additionalProperties, `${location}.${key}`, errors);
    }
  }
  return errors;
}

export const defaultFixtures = Object.fromEntries(
  ["health", "inbox", "daemon", "console"].flatMap((key) => {
    const value = fixtureFor(schema.properties[key]);
    const errors = validateFixture(value, schema.properties[key]);
    if (errors.length) throw new Error(`${key}: ${errors.join(", ")}`);
    return [
      [`/api/v1/${key}`, value],
      [`/${key}`, value],
    ];
  }),
);

// Screenshot and mobile-audit data. The normal profile stays intentionally small:
// parity tests supply their own exact rows and counts.
const richPath =
  "src/very-long-workspace-name/feature-with-a-long-description/components/task-detail/overview-panel.tsx";
const richNow = "2026-10-04T09:00:00Z";

export function richFixtures() {
  const summary = (id, title, extra = {}) => ({
    ...fixtureFor(schema.$defs.TaskSummary),
    id,
    title,
    status: "ready",
    kind: "execute",
    tier: "standard",
    category: "feature",
    priority_label: "normal",
    created_at: richNow,
    updated_at: richNow,
    ...extra,
  });
  const title = "複数の画面にまたがる長いタスク名と依存関係を確認する作業 ".repeat(4).trim();
  const tasks = [
    summary("T1", title, { children: 2, project_id: "P1" }),
    summary("T2", "子タスク: 画面の状態を調べる", { parent_id: "T1", depends_on: ["T1"], project_id: "P1" }),
    summary("T3", "子タスク: 検証結果をまとめる", { parent_id: "T1", depends_on: ["T2"], project_id: "P1" }),
    ...Array.from({ length: 21 }, (_, i) =>
      summary(`T${i + 4}`, `一覧の追加タスク ${i + 4}: 表示密度を確かめる`, { project_id: "P1" }),
    ),
    summary("T25", "レビュー結果を確認して判断する", { status: "reviewing", project_id: "P1" }),
  ];
  const detail = fixtureFor(schema.$defs.TaskDetail);
  detail.task = {
    ...detail.task,
    id: "T1",
    title,
    objective: "長い目的文を複数行で表示し、親子関係・依存関係・作業ツリーと成果物を確認する。\n".repeat(6).trim(),
    status: "ready",
    created_at: richNow,
    updated_at: richNow,
    project_id: "P1",
  };
  detail.workspace_dir = `/workspace/${richPath}`;
  detail.children = tasks.slice(1, 3).map(({ id, title, kind, status }) => ({ id, title, kind, status, actions: [] }));
  detail.dependents = [detail.children[0]];
  detail.runs = [
    {
      ...fixtureFor(schema.$defs.RunSummary),
      run_id: "R1",
      role: "worker",
      adapter: "codex",
      model: "standard",
      started_at: richNow,
      finished_at: richNow,
    },
  ];
  const reviewing = structuredClone(detail);
  reviewing.task = {
    ...reviewing.task,
    id: "T25",
    title: "レビュー結果を確認して判断する",
    objective: "検証結果を読み、承認するか理由を添えて却下する。",
    status: "reviewing",
    kind: "execute",
  };
  reviewing.actions = ["approve", "reject"];
  reviewing.children = [];
  reviewing.dependents = [];
  const changes = fixtureFor(schema.$defs.ChangesView);
  changes.task_id = "T1";
  changes.gh = true;
  changes.merge_method = "squash";
  changes.repos = [
    {
      ...fixtureFor(schema.$defs.RepoChangesView),
      repo: "web",
      branch: "celeris/T1",
      default_branch: "main",
      ahead: 2,
      files: Array.from({ length: 15 }, (_, i) => ({
        path: i ? `${richPath.replace("overview-panel", `panel-${i}`)}` : richPath,
        status: "M",
        additions: i + 2,
        deletions: 1,
      })),
      stat: { files: 15, additions: 135, deletions: 15 },
      dirty: false,
      missing: false,
      origin: false,
    },
  ];
  const project = {
    ...fixtureFor(schema.$defs.Project),
    id: "P1",
    title: "画面品質の確認",
    request: "一覧と成果物を確認する",
    status: "active",
    created_at: richNow,
    updated_at: richNow,
  };
  const milestone = {
    ...fixtureFor(schema.$defs.MilestoneView),
    id: "M1",
    project_id: "P1",
    seq: 1,
    title: "画面の検証を終える",
    status: "reached",
    created_at: richNow,
    updated_at: richNow,
  };
  const org = {
    items: [
      { id: "cos", name: "CoS", kind: "secretary", created_at: richNow, updated_at: richNow },
      {
        id: "software-engineering",
        name: "ソフトウェア開発",
        kind: "department",
        parent_id: "cos",
        created_at: richNow,
        updated_at: richNow,
      },
      {
        id: "ui-ux",
        name: "UI/UX",
        kind: "section",
        parent_id: "software-engineering",
        created_at: richNow,
        updated_at: richNow,
      },
    ],
  };
  const docsTree = {
    ...fixtureFor(schema.$defs.DocsTree),
    project_id: "P1",
    repo: "agent-platform",
    root: "docs",
    default_branch: "main",
    items: [{ path: "docs/guide.md", title: "画面確認の案内" }],
    truncated: false,
  };
  const docPage = {
    ...fixtureFor(schema.$defs.DocPage),
    project_id: "P1",
    repo: "agent-platform",
    root: "docs",
    default_branch: "main",
    path: "docs/guide.md",
    title: "画面確認の案内",
    raw: "# 画面確認の案内\n\n状態と判断画面を確認します。",
    html: "",
    history: [],
    too_large: false,
  };
  const artifact = (idx) => ({
    idx,
    run_id: "R1",
    ts: richNow,
    exists: true,
    forbidden: false,
    size: 128 + idx,
    artifact: {
      kind: "file",
      name: `report-${idx}.md`,
      path: `reports/${richPath}/report-${idx}.md`,
      sha256: "0".repeat(64),
    },
  });
  const progressLine = (seq, kind, text, tool) => ({ seq, at: richNow, kind, text, ...(tool ? { tool } : {}) });
  // 報告・承認・review 待ちの task の実行と routing。形は schema の required から組み（fixtureFor）、
  // 画面が読む欄（headline・body・decision・phase・plan・runs など）だけを正常な値で埋める。
  const report = (id, kind, headline, extra) => ({
    ...fixtureFor(schema.$defs.Report),
    id,
    kind,
    headline,
    node_id: "cos",
    level: 0,
    created_at: richNow,
    ...extra,
  });
  const reportRows = {
    RP1: report("RP1", "result", "画面の検証結果を報告します", {
      body: "## 検証の結果\n\n- 360・390・412・1440 px の撮影を確認しました。\n- 長い path と長文の折り返しに崩れはありません。\n\n受信箱の認可を判断してください。",
      task_id: "T1",
      project_id: "P1",
      sources: ["RP2", "RP3"],
    }),
    RP2: report("RP2", "progress", "子タスクの状態を確認しました", {
      body: "T2 と T3 の状態を確認し、依存の順を保ったまま進めています。",
      level: 2,
      node_id: "ui-ux",
      task_id: "T2",
      project_id: "P1",
    }),
    RP3: report("RP3", "question", "承認の判断をお願いします", {
      body: "cluster-hpc への依頼を認めてよいか、受信箱で判断をお願いします。",
      level: 1,
      node_id: "software-engineering",
      task_id: "T4",
      project_id: "P1",
    }),
  };
  const approval = (id, decision, question, extra) => ({
    ...fixtureFor(schema.$defs.Approval),
    id,
    decision,
    question,
    node_id: "ui-ux",
    task_id: "T1",
    project_id: "P1",
    created_at: "2026-10-03T09:00:00Z",
    decided_at: "2026-10-03T10:00:00Z",
    answer: null,
    ...extra,
  });
  const approvalRows = [
    approval("01J8APPROVAL0000000000001", "once", "ビルドの検査を走らせてよいですか", { answer: "はい。今回だけ。" }),
    approval("01J8APPROVAL0000000000002", "standing", "web の画像を書き出してよいですか", {
      decided_at: richNow,
      answer: "同じ依頼は今後も認めます。",
    }),
  ];
  const workUnit = (seq, key, status) => ({
    ...fixtureFor(schema.$defs.WorkUnitView),
    id: `WU${seq}`,
    key,
    seq,
    status,
    created_at: richNow,
    updated_at: richNow,
  });
  const execution = fixtureFor(schema.$defs.TaskExecutionView);
  execution.phase = "verifying";
  execution.plan = {
    ...fixtureFor(schema.$defs.ExecutionPlanView),
    id: "PL1",
    task_id: "T1",
    version: 1,
    origin: "planner",
    status: "active",
    created_at: richNow,
    work_units: [workUnit(1, "fix-screens", "done"), workUnit(2, "verify-screens", "running")],
  };
  execution.gate = {
    ...fixtureFor(schema.$defs.ExecutionGateDecision),
    mode: "compound",
    source: "policy",
    score: 6,
    threshold: 5,
    rule_id: "signals-v1",
    policy_version: "1",
    shadow: false,
  };
  execution.runs = detail.runs;
  execution.metrics = { ...execution.metrics, work_units_total: 2, work_units_done: 1, has_plan: true };
  const routing = {
    ...fixtureFor(schema.properties.task_routing),
    task_id: "T1",
    assignee: "ui-ux",
    runs: [
      {
        ...fixtureFor(schema.$defs.RoutingAudit),
        task_id: "T1",
        run_id: "R1",
        adapter: "codex",
        lane: "standard",
        model: "standard",
        org_node: "ui-ux",
        rule_id: "rule-standard",
      },
    ],
  };
  const reviewRoutes = {
    "/api/v1/reports": { items: Object.values(reportRows) },
    ...Object.fromEntries(
      Object.entries(reportRows).map(([id, row]) => [
        `/api/v1/reports/${id}`,
        { report: row, sources_expanded: (row.sources ?? []).map((source) => reportRows[source]).filter(Boolean) },
      ]),
    ),
    "/api/v1/approvals": { items: approvalRows },
    "/api/v1/standing-rules": {
      items: [
        {
          ...fixtureFor(schema.$defs.StandingRule),
          id: "SR1",
          rule: "web の画像を書き出す依頼は今後も認める",
          node_id: "ui-ux",
          created_at: "2026-10-03T10:00:00Z",
        },
      ],
    },
    "/api/v1/tasks/T1/execution": execution,
    "/api/v1/tasks/T1/routing": routing,
  };
  return {
    ...reviewRoutes,
    "/api/v1/tasks": {
      items: tasks,
      total: tasks.length,
      next_cursor: null,
      counts_by_status: { ready: tasks.length - 1, reviewing: 1 },
    },
    "/api/v1/graph": {
      nodes: tasks.slice(0, 8).map(({ id, title, kind, status, parent_id }) => ({
        id,
        title,
        kind,
        status,
        ...(parent_id ? { parent_id } : {}),
      })),
      edges: [
        { from: "T1", to: "T2", kind: "depends_on" },
        { from: "T2", to: "T3", kind: "depends_on" },
      ],
    },
    "/api/v1/tasks/T1": detail,
    "/api/v1/tasks/T25": reviewing,
    "/api/v1/org": org,
    "/api/v1/projects/P1/docs": docsTree,
    "/api/v1/projects/P1/docs/page": docPage,
    "/api/v1/projects/P1/docs/maintenance": {
      audit: {
        revision: "fixture",
        documents: [{ path: "docs/guide.md", title: "画面確認の案内", findings: ["見出しの整理を確認してください"] }],
      },
      proposal: { actions: [{ operation: "rewrite", path: "docs/guide.md" }], rationale: ["案内の見出しを揃える"] },
      policy: { mode: "observe" },
    },
    "/api/v1/tasks/T1/timeline": {
      task_id: "T1",
      items: [{ kind: "delegation", at: richNow, run_id: "R1", tasks: detail.children }],
    },
    "/api/v1/tasks/T1/changes": changes,
    "/api/v1/tasks/T1/changes/web/diff": (url) => ({
      repo: "web",
      path: url.searchParams.get("path") ?? richPath,
      diff: `--- a/${richPath}\n+++ b/${richPath}\n@@ -1,2 +1,3 @@\n-old text\n+${"長い差分の行".repeat(50)}\n context\n`,
      truncated: false,
    }),
    "/api/v1/tasks/T1/tree": (url) => ({
      repo: "web",
      path: url.searchParams.get("path") ?? "",
      repos: [{ name: "web", kind: "git", dir: `/workspace/${richPath}` }],
      entries: [
        { kind: "dir", name: "src", path: "src" },
        ...Array.from({ length: 12 }, (_, i) => ({
          kind: "file",
          name: `long-file-${i}.tsx`,
          path: `${richPath}/long-file-${i}.tsx`,
          size: 120 + i,
        })),
      ],
    }),
    "/api/v1/tasks/T1/tree/file": (url) => ({
      repo: "web",
      path: url.searchParams.get("path") ?? richPath,
      size: 64,
      binary: false,
      too_large: false,
      text: "長いファイルの本文\n".repeat(12),
    }),
    "/api/v1/tasks/T1/artifacts": { task_id: "T1", items: Array.from({ length: 12 }, (_, i) => artifact(i)) },
    "/api/v1/projects": { items: [project] },
    "/api/v1/projects/P1": {
      project,
      milestones: [milestone],
      milestones_frozen: 1,
      tasks: [
        {
          ...fixtureFor(schema.$defs.ProjectTaskView),
          id: "T1",
          title,
          status: "ready",
          depends_on: [],
          conversation: false,
        },
      ],
    },
    "/api/v1/console": {
      items: [
        {
          kind: "human",
          at: richNow,
          cursor: "c1",
          message_id: "M1",
          node_id: "cos",
          text: "画面群の長文・多数行・長い path を確認してください。",
        },
        {
          kind: "reply",
          at: richNow,
          cursor: "c2",
          message_id: "M2",
          node_id: "cos",
          text: "確認を始めます。",
          state: "done",
          steps: [
            { kind: "tool_use", tool: "read_file", text: `Read ${richPath}` },
            { kind: "tool_result", tool: "read_file", text: "12 行を読みました" },
          ],
        },
        {
          kind: "progress",
          at: richNow,
          cursor: "c3",
          title: "画面を検証する",
          tier: "standard",
          progress: {
            task_id: "T1",
            run_id: "R1",
            count: 8,
            tool_count: 2,
            started_at: richNow,
            updated_at: richNow,
            first: [progressLine(1, "tool_use", `Read ${richPath}`, "read_file")],
            last: [progressLine(8, "tool_result", "表示を確認しました", "read_file")],
          },
        },
      ],
      next_cursor: null,
    },
  };
}

export function richFiles() {
  return {
    "/api/v1/tasks/T1/runs/R1/stdout.jsonl": {
      body: `${[
        { type: "user", message: { role: "user", content: [{ type: "text", text: "画面を確認してください" }] } },
        {
          type: "assistant",
          message: {
            role: "assistant",
            content: [
              { type: "text", text: "確認を始めます" },
              { type: "tool_use", id: "tool-1", name: "read_file", input: { path: richPath } },
            ],
          },
        },
        {
          type: "user",
          message: {
            role: "user",
            content: [{ type: "tool_result", tool_use_id: "tool-1", content: "12 行を読みました" }],
          },
        },
        ...Array.from({ length: 24 }, (_, i) => ({
          type: "assistant",
          message: {
            role: "assistant",
            content: [{ type: "text", text: `検証ログ ${i + 1}: ${"長い行".repeat(12)}` }],
          },
        })),
      ]
        .map((line) => JSON.stringify(line))
        .join("\n")}\n`,
      type: "text/plain; charset=utf-8",
    },
    ...Object.fromEntries(
      Array.from({ length: 12 }, (_, i) => [
        `/api/v1/tasks/T1/artifacts/${i}`,
        { body: `# report-${i}\n\n画面の検証結果\n`, type: "text/markdown" },
      ]),
    ),
  };
}

// P4-10/P4-11: knowledge domain fixtures. Keep this block together for parallel merge.
export const knowledgeFixtures = {
  "/api/v1/knowledge/tree": {
    initialized: true,
    root: "/fake/knowledge",
    items: [{ path: "projects/demo.md", title: "Demo knowledge" }],
    inbox_count: 1,
    truncated: false,
  },
  "/api/v1/knowledge/page": {
    path: "projects/demo.md",
    title: "Demo knowledge",
    raw: "# Demo knowledge\n\nKnowledge body",
    html: "",
    root: "/fake/knowledge",
    history: [],
    etag: "etag-1",
    too_large: false,
  },
  "/api/v1/knowledge/inbox": {
    initialized: true,
    root: "/fake/knowledge",
    items: [
      {
        id: "K1",
        path: "_inbox/K1.md",
        target: "projects/new.md",
        title: "New knowledge",
        body: "# New knowledge",
        html: "",
        target_exists: false,
      },
    ],
  },
  "/api/v1/skills": {
    initialized: true,
    root: "/fake/knowledge",
    items: [{ name: "demo", description: "Demo skill", mounted_by: [] }],
  },
  "/api/v1/skills/demo": {
    name: "demo",
    skill_md: "# Demo skill\n\nDescription",
    files: ["references/guide.md"],
    mounted_by: [],
  },
};

// ADR-0133: 受信箱（GET /inbox/items・answer）と通知（GET /notifications・unread-count・read・read-all）。
// 状態を持ち、答えた項目は消え、既読は残る。変わったら SSE の合図（inbox_changed・notifications_changed）を送る。
// `fixtures` に同じ path があればそちらを返す（試験ごとの上書き）。並列の merge のためこの塊はまとめておく。
const inboxTask = (id, title, status = "blocked") => ({ id, title, kind: "execute", status, actions: [] });
const inboxOption = (key, label, effect, needs_note = false) => ({ key, label, effect, needs_note });
const inboxAnswer = (id) => ({
  method: "POST",
  path: `/api/v1/inbox/items/${id}/answer`,
  body_schema: { option: "string", note: "string?" },
});

export function inboxItemsFixture() {
  const item = (id, kind, title, extra) => ({
    id,
    kind,
    title,
    detail: null,
    options: [],
    recommended: null,
    due_at: null,
    blocking: { tasks: [], units: [], summary: "" },
    blocked_by: [],
    answer: inboxAnswer(id),
    created_at: "2026-10-01T09:00:00Z",
    age_secs: 3600,
    links: [],
    project_id: null,
    task: null,
    ...extra,
  });
  return [
    item("decision:D1", "decision", "認証方式を決める", {
      detail: "gateway の session と daemon token のどちらで web を守るか。",
      options: [
        inboxOption("session", "gateway の session", "web は gateway の cookie だけで入る"),
        inboxOption("token", "daemon token", "web が token を持つ"),
      ],
      recommended: "session",
      due_at: "2026-10-05T09:00:00Z",
      blocking: { tasks: [inboxTask("T1", "web の認証")], units: ["auth"], summary: "葉 auth を止めている" },
      project_id: "P1",
      task: inboxTask("T1", "web の認証"),
      links: [{ label: "タスク", href: "/tasks/T1" }],
    }),
    item("plan_gate:T2", "plan_gate", "計画を承認する: 受信箱の画面", {
      options: [
        inboxOption("approve", "承認する", "計画どおりに葉を走らせる"),
        inboxOption("replan", "計画をやり直す", "planner に差し戻す", true),
        inboxOption("withdraw", "取り下げる", "task を止める", true),
      ],
      recommended: "approve",
      blocked_by: ["decision:D1"],
      blocking: { tasks: [inboxTask("T2", "受信箱の画面")], units: [], summary: "task 全体を止めている" },
      project_id: "P1",
      task: inboxTask("T2", "受信箱の画面"),
      age_secs: 7200,
    }),
    item("failed:T3", "failed", "失敗: nightly の検査", {
      detail: "cargo test が 2 件落ちた。",
      options: [
        inboxOption("retry", "やり直す", "同じ内容で複製して走らせる"),
        inboxOption("reopen", "再開する", "同じ worktree で続ける"),
        inboxOption("cancel", "取り消す", "この task を終える", true),
      ],
      recommended: "retry",
      blocking: { tasks: [inboxTask("T3", "nightly の検査", "failed")], units: [], summary: "親 task を止めている" },
      task: inboxTask("T3", "nightly の検査", "failed"),
      age_secs: 86400,
    }),
    item("authorization:A1", "authorization", "認可: cluster-hpc への依頼", {
      detail: "software-engineering が cluster-hpc に job の投入を頼む。",
      options: [
        inboxOption("approve", "認可する", "今回だけ許す"),
        inboxOption("approve_always", "今後ずっと認可する", "同じ依頼を以後は自動で許す"),
        inboxOption("deny", "断る", "依頼を取り下げる", true),
      ],
      recommended: "approve",
      blocking: { tasks: [inboxTask("T4", "ベンチを流す")], units: [], summary: "1 task を止めている" },
      task: inboxTask("T4", "ベンチを流す"),
      age_secs: 600,
    }),
    item("knowledge_review:K1", "knowledge_review", "知識の候補を確かめる: New knowledge", {
      options: [inboxOption("accept", "採用する", "正本に入れる"), inboxOption("reject", "捨てる", "候補を消す")],
      links: [{ label: "知識の候補", href: "/knowledge/inbox" }],
      age_secs: 120,
    }),
  ];
}

export function noticesFixture() {
  const notice = (id, kind, title, summary, count, last_at, extra) => ({
    id,
    kind,
    group_key: `${kind}:${id}`,
    title,
    summary,
    count,
    first_at: "2026-10-01T00:00:00Z",
    last_at,
    read_at: null,
    links: [],
    project_id: null,
    target: null,
    task_id: null,
    ...extra,
  });
  return [
    notice("N1", "task_done", "3 件の task が完了", "web の認証ほか 2 件", 3, "2026-10-03T12:00:00Z", {
      project_id: "P1",
      links: [{ label: "タスク", href: "/tasks/T1" }],
    }),
    notice("N2", "report", "日次の報告", "受信箱 5 件・失敗 1 件", 1, "2026-10-03T09:00:00Z"),
    notice("N3", "bad_news", "nightly の検査が失敗", "cargo test が 2 件落ちた", 2, "2026-10-02T21:00:00Z", {
      task_id: "T3",
      target: { kind: "task", id: "T3" },
    }),
    notice("N4", "release", "release aaaaaaaaaaaa を昇格", "本番へ反映済み", 1, "2026-10-01T18:00:00Z", {
      read_at: "2026-10-01T19:00:00Z",
    }),
  ];
}

const FAKE_NOW = "2026-10-04T00:00:00Z";

/** 受信箱・通知の要求を処理する。扱わない path なら false。 */
function handleInboxNotifications(state, req, res, url, body, sendEvent) {
  const json = (status, value) => {
    res.writeHead(status, { "content-type": "application/json" });
    res.end(JSON.stringify(value));
  };
  const parts = url.pathname
    .replace(/^\/api\/v1\//, "")
    .split("/")
    .map(decodeURIComponent);
  if (parts[0] === "inbox" && parts[1] === "items") {
    if (parts.length === 2 && req.method === "GET") {
      const kind = url.searchParams.get("kind");
      const project = url.searchParams.get("project");
      const items = state.items.filter(
        (item) => (!kind || item.kind === kind) && (!project || item.project_id === project),
      );
      const by_kind = {};
      for (const item of items) by_kind[item.kind] = (by_kind[item.kind] ?? 0) + 1;
      return json(200, { items, counts: { total: items.length, by_kind }, suppressed: {} });
    }
    const item = state.items.find((row) => row.id === parts[2]);
    if (parts.length === 3 && req.method === "GET") return item ? json(200, item) : json(404, { error: "not_found" });
    if (parts.length === 4 && parts[3] === "answer" && req.method === "POST") {
      if (!item) return json(404, { error: "not_found" });
      const option = item.options.find((row) => row.key === body?.option);
      if (!option) return json(422, { error: "invalid_option" });
      if (option.needs_note && !(typeof body.note === "string" && body.note.trim()))
        return json(422, { error: "note_required" });
      state.items = state.items.filter((row) => row !== item);
      state.answers.push({ id: item.id, ...body });
      sendEvent("inbox_changed", {});
      return json(200, { item_id: item.id, removed: true, result: { option: option.key } });
    }
    return json(405, { error: "method_not_allowed" });
  }
  if (parts[0] === "notifications") {
    const unreadOf = (rows) => rows.filter((row) => !row.read_at);
    if (parts.length === 1 && req.method === "GET") {
      const kind = url.searchParams.get("kind");
      const project = url.searchParams.get("project");
      const before = url.searchParams.get("before");
      const limit = Number(url.searchParams.get("limit") ?? 50);
      if (!Number.isInteger(limit) || limit < 1 || limit > 500) return json(400, { error: "invalid_limit" });
      const rows = state.notices
        .filter(
          (row) =>
            (url.searchParams.get("unread") !== "true" || !row.read_at) &&
            (!kind || row.kind === kind) &&
            (!project || row.project_id === project) &&
            (!before || row.last_at < before),
        )
        .sort((a, b) => b.last_at.localeCompare(a.last_at));
      const items = rows.slice(0, limit);
      return json(200, {
        items,
        unread: unreadOf(state.notices).length,
        next_before: rows.length > limit ? items.at(-1).last_at : null,
      });
    }
    if (parts[1] === "unread-count" && parts.length === 2 && req.method === "GET") {
      const unread = unreadOf(state.notices);
      const by_kind = {};
      for (const row of unread) by_kind[row.kind] = (by_kind[row.kind] ?? 0) + 1;
      return json(200, { unread: unread.length, events: unread.reduce((n, row) => n + row.count, 0), by_kind });
    }
    if (parts[1] === "read-all" && parts.length === 2 && req.method === "POST") {
      const marked = unreadOf(state.notices).filter(
        (row) =>
          (!body?.kind || row.kind === body.kind) &&
          (!body?.project || row.project_id === body.project) &&
          (!body?.before || row.last_at < body.before),
      );
      for (const row of marked) row.read_at = FAKE_NOW;
      if (marked.length) sendEvent("notifications_changed", {});
      return json(200, { marked: marked.length });
    }
    if (parts.length === 3 && parts[2] === "read" && req.method === "POST") {
      const row = state.notices.find((notice) => notice.id === parts[1]);
      if (!row) return json(404, { error: "not_found" });
      if (!row.read_at) {
        row.read_at = FAKE_NOW;
        sendEvent("notifications_changed", {});
      }
      return json(200, { id: row.id, read_at: row.read_at });
    }
    return json(405, { error: "method_not_allowed" });
  }
  return false;
}

// files は daemon の path（`/api/v1/tasks/...`）→ `{ body, type?, disposition? }`。
// token を与えると、`Authorization: Bearer <token>` の無い要求に 401 を返す（P1-07 の中継の検査）。
export function createFakeDaemon({
  host = "127.0.0.1",
  port = 0,
  delayMs = 0,
  fixtures = {},
  token = null,
  files = {},
  profile = "default",
  // 状態の変種（web/e2e/support/states.ts）。fault は path の前置きに一致する要求を status で返し、
  // hold は一致する要求を releaseHeld() まで保留する（読み込み中を時計でなく出来事で解く）。
  // streamStatus は `/api/v1/stream` を最初から 200 以外にする（再接続も失敗する）。
  fault = null,
  hold = null,
  streamStatus: initialStreamStatus = 200,
  inboxItems = null,
  notices = null,
} = {}) {
  if (host !== "127.0.0.1" && host !== "::1" && host !== "localhost") throw new Error("fake daemon requires loopback");
  if (!Number.isInteger(port) || port < 0 || port > 65535 || reservedPorts.has(port))
    throw new Error("fake daemon refuses reserved port");
  if (!delayValues.has(delayMs)) throw new Error("JSON delay must be 0, 5000 or 10000 ms");
  if (!new Set(["default", "rich"]).has(profile)) throw new Error(`unknown fixture profile: ${profile}`);
  fixtures = { ...knowledgeFixtures, ...defaultFixtures, ...(profile === "rich" ? richFixtures() : {}), ...fixtures };
  files = { ...(profile === "rich" ? richFiles() : {}), ...files };
  const requests = [];
  const clients = new Set();
  const consoleClients = new Set();
  let delay = delayMs;
  let postDelay = 0;
  let streamStatus = initialStreamStatus;
  let timer;
  let faultRule = fault;
  let holdRule = hold;
  const held = [];
  const inbox = {
    items: inboxItems ?? inboxItemsFixture(),
    notices: notices ?? noticesFixture(),
    answers: [],
  };
  const streamPaths = new Set(["/events", "/api/v1/events", "/api/v1/stream", "/api/v1/console/stream"]);
  const matches = (rule, pathname) =>
    rule !== null &&
    !streamPaths.has(pathname) &&
    (rule.paths ?? ["/api/v1"]).some((prefix) => pathname === prefix || pathname.startsWith(`${prefix}/`));
  const server = http.createServer((req, res) => {
    const pathname = new URL(req.url ?? "/", `http://${host === "::1" ? "[::1]" : host}`).pathname;
    if (matches(holdRule, pathname)) {
      held.push(() => handle(req, res));
      return;
    }
    handle(req, res);
  });
  const handle = (req, res) => {
    const pathname = new URL(req.url ?? "/", `http://${host === "::1" ? "[::1]" : host}`).pathname;
    const record = {
      path: pathname,
      method: req.method,
      at: Date.now(),
      aborted: false,
      authorization: req.headers.authorization ?? null,
      cookie: req.headers.cookie ?? null,
    };
    requests.push(record);
    let finished = false;
    res.on("finish", () => {
      finished = true;
    });
    res.on("close", () => {
      if (!finished) record.aborted = true;
      clients.delete(res);
    });
    if (token !== null && req.headers.authorization !== `Bearer ${token}`) {
      res.writeHead(401, { "content-type": "application/json" });
      res.end(JSON.stringify({ error: "unauthorized" }));
      return;
    }
    if (matches(faultRule, pathname)) {
      res.writeHead(faultRule.status, { "content-type": "application/json" });
      res.end(JSON.stringify({ error: faultRule.status === 403 ? "forbidden" : "unavailable" }));
      return;
    }
    if (pathname === "/api/v1/stream" && streamStatus !== 200) {
      res.writeHead(streamStatus, { "content-type": "application/json" });
      res.end(JSON.stringify({ error: "too_many_streams" }));
      return;
    }
    // ops: daemon/providers (P4-12)
    if (pathname === "/api/v1/replay" || pathname.startsWith("/api/v1/providers") || pathname === "/api/v1/reload") {
      if (!server.opsProviders)
        server.opsProviders = {
          items: [
            {
              id: "claude-main",
              adapter: "claude-code",
              concurrency: 2,
              env_keys: [],
              tiers: ["standard"],
              model: "sonnet",
              stats: {
                by_day: [],
                done: 0,
                error: 0,
                input_tokens: 0,
                lease_expired: 0,
                output_tokens: 0,
                question: 0,
                requeue: 0,
                runs: 0,
              },
            },
          ],
        };
      const ops = server.opsProviders;
      const chunks = [];
      req.on("data", (chunk) => chunks.push(chunk));
      req.on("end", () => {
        const text = Buffer.concat(chunks).toString("utf8");
        record.body = text;
        const json = (status, body) => {
          res.writeHead(status, { "content-type": "application/json" });
          res.end(JSON.stringify(body));
        };
        let input = {};
        try {
          input = text ? JSON.parse(text) : {};
        } catch {
          return json(400, { error: "bad json" });
        }
        const parts = pathname.replace("/api/v1/", "").split("/");
        if (pathname === "/api/v1/replay" && req.method === "POST") return json(200, { mismatches: [], tasks: 3 });
        if (pathname === "/api/v1/reload" && req.method === "POST") return json(200, { reloaded: true });
        if (parts[0] !== "providers") return json(404, { error: "not found" });
        const found = ops.items.find((item) => item.id === parts[1]);
        if (parts.length === 1 && req.method === "GET") return json(200, { items: ops.items });
        if (parts.length === 1 && req.method === "POST") {
          if (!input.id || ops.items.some((item) => item.id === input.id))
            return json(422, { detail: "id が不正か重複しています" });
          const created = { concurrency: 1, env_keys: [], tiers: [], stats: ops.items[0]?.stats, ...input };
          ops.items.push(created);
          return json(200, created);
        }
        if (!found) return json(404, { error: "not found" });
        if (parts.length === 2 && req.method === "PATCH") return json(200, Object.assign(found, input));
        if (parts.length === 2 && req.method === "DELETE") {
          ops.items.splice(ops.items.indexOf(found), 1);
          return json(200, {});
        }
        if (parts[2] === "check" && req.method === "POST") {
          found.last_check = { at: "2026-09-30T00:00:00Z", result: "ok", detail: null };
          return json(200, { checked_at: "2026-09-30T00:00:00Z", result: "ok", detail: null });
        }
        return json(404, { error: "not found" });
      });
      return;
    }
    // end ops: daemon/providers (P4-12)
    if (pathname === "/events" || pathname === "/api/v1/events" || pathname === "/api/v1/stream") {
      record.query = new URL(req.url ?? "/", "http://x").search;
      record.lastEventId = req.headers["last-event-id"] ?? null;
      res.writeHead(200, {
        "content-type": "text/event-stream",
        "cache-control": "no-cache",
        connection: "keep-alive",
      });
      res.write(": connected\n\n");
      clients.add(res);
      return;
    }
    if (pathname === "/api/v1/console/stream") {
      record.query = new URL(req.url ?? "/", "http://x").search;
      res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
      const since = new URL(req.url ?? "/", "http://x").searchParams.get("since");
      res.write(
        `event: hello\ndata: ${JSON.stringify({ cursor: since ?? "h0", now: "2026-01-01T00:00:00Z", scope: "all" })}\n\n`,
      );
      consoleClients.add(res);
      res.on("close", () => consoleClients.delete(res));
      return;
    }
    if (
      req.method === "POST" &&
      (pathname === "/api/v1/console/instruct" || pathname === "/api/v1/console/new-conversation")
    ) {
      const chunks = [];
      req.on("data", (chunk) => chunks.push(chunk));
      req.on("end", () => {
        record.body = Buffer.concat(chunks).toString("utf8");
        const respond = () => {
          if (pathname.endsWith("/instruct")) {
            res.writeHead(202, { "content-type": "application/json" });
            res.end(JSON.stringify({ message_id: "M1", node_id: "cos", task_id: "T1" }));
          } else {
            res.writeHead(204);
            res.end();
          }
        };
        if (postDelay) setTimeout(respond, postDelay);
        else respond();
      });
      return;
    }
    // ops: releases (P4-16)
    // GET /api/v1/releases は状態を持つ。POST /api/v1/releases/{sha12}/promote は 202 で昇格を起こし、
    // `pendingMs` の間は promoting、その後 `mode`（succeed | fail）の結果を行に書く。
    // 制御: POST /__fake/releases `{ mode?, pendingMs? }`。
    if (pathname === "/__fake/releases" || pathname.startsWith("/api/v1/releases")) {
      if (!server.fakeReleases) {
        server.fakeReleases = {
          mode: "succeed",
          pendingMs: 0,
          current: "aaaaaaaaaaaa",
          promotedAt: { aaaaaaaaaaaa: "2026-09-29T00:00:00Z", bbbbbbbbbbbb: null },
          promoting: null,
          failed: null,
        };
      }
      const rel = server.fakeReleases;
      const json = (status, value) => {
        res.writeHead(status, { "content-type": "application/json" });
        res.end(JSON.stringify(value));
      };
      if (req.method === "POST") {
        const chunks = [];
        req.on("data", (chunk) => chunks.push(chunk));
        req.on("end", () => {
          const promote = /^\/api\/v1\/releases\/([0-9a-f]{12})\/promote$/.exec(pathname);
          if (pathname === "/__fake/releases") {
            const body = JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}");
            if (body.mode) rel.mode = body.mode;
            if (typeof body.pendingMs === "number") rel.pendingMs = body.pendingMs;
            return json(200, { mode: rel.mode, pendingMs: rel.pendingMs });
          }
          if (!promote || !(promote[1] in rel.promotedAt)) return json(404, { error: "not found" });
          const sha = promote[1];
          if (rel.promoting) return json(409, { error: "別の昇格が走っています" });
          const startedAt = new Date().toISOString();
          rel.promoting = sha;
          rel.failed = null;
          setTimeout(() => {
            if (rel.mode === "fail")
              rel.failed = { sha, failed_at: new Date().toISOString(), error: "promote.sh が exit 1 で終わりました" };
            else {
              rel.current = sha;
              rel.promotedAt[sha] = new Date().toISOString();
            }
            rel.promoting = null;
          }, rel.pendingMs);
          return json(202, { sha12: sha, log: `${sha}/promote.log`, started_at: startedAt, script_from: "current" });
        });
        return;
      }
      const item = (sha) => ({
        sha12: sha,
        gate_ok: true,
        is_current: rel.current === sha,
        is_previous: rel.current !== sha && rel.promotedAt[sha] !== null,
        promoting: rel.promoting === sha,
        promoted_at: rel.promotedAt[sha],
        promote_failed:
          rel.failed && rel.failed.sha === sha ? { failed_at: rel.failed.failed_at, error: rel.failed.error } : null,
        built_at: "2026-09-28T00:00:00Z",
        ref: "main",
      });
      return json(200, {
        current: rel.current,
        previous: null,
        running: { release: rel.current, role: "active", instance_id: "I1" },
        instances: [],
        items: ["bbbbbbbbbbbb", "aaaaaaaaaaaa"].map(item),
      });
    }
    // accounts/clusters (P4-13..15)
    // accounts・secrets・llm sources・mcp clients・clusters を状態付きで返す。ログイン・接続の途中状態（login_pending /
    // connect_pending）はサーバが持ち、GET で返す。secret の値は保存せず fingerprint だけ持つ。
    if (/^\/api\/v1\/(accounts|secrets|llm\/sources|mcp\/clients|clusters)(\/|$)/.test(pathname)) {
      if (!server.fakeAc) {
        server.fakeAc = {
          accounts: [{ id: "main", adapter: "claude-code", logged_in: true, login_pending: false }],
          secrets: [],
          clusters: [
            {
              id: "pegasus",
              host: "pegasus.example",
              auth: "totp",
              connected: false,
              connect_pending: false,
              work_dir: null,
            },
          ],
        };
      }
      const ac = server.fakeAc;
      const chunks = [];
      req.on("data", (chunk) => chunks.push(chunk));
      req.on("end", () => {
        const text = Buffer.concat(chunks).toString("utf8");
        const json = (status, value) => {
          res.writeHead(status, { "content-type": "application/json" });
          res.end(JSON.stringify(value));
        };
        let input = {};
        try {
          input = text ? JSON.parse(text) : {};
        } catch {
          return json(400, { error: "bad json" });
        }
        const parts = pathname.replace("/api/v1/", "").split("/");
        const view = (a) => ({
          dir: `/accounts/${a.id}`,
          in_use: 0,
          stats: {
            done: 0,
            error: 0,
            input_tokens: 0,
            lease_expired: 0,
            output_tokens: 0,
            question: 0,
            requeue: 0,
            runs: 0,
            by_day: [],
          },
          ...a,
        });
        if (parts[0] === "accounts") {
          const acc = ac.accounts.find((a) => a.id === parts[1]);
          if (parts.length === 1 && req.method === "GET")
            return json(200, { items: ac.accounts.map(view), max_runs_per_account: 1 });
          if (parts.length === 1 && req.method === "POST") {
            if (!input.id || ac.accounts.some((a) => a.id === input.id))
              return json(422, { detail: "id が不正か重複しています" });
            const created = {
              id: input.id,
              adapter: input.adapter ?? "claude-code",
              logged_in: false,
              login_pending: false,
            };
            ac.accounts.push(created);
            return json(200, view(created));
          }
          if (!acc) return json(404, { error: "not found" });
          if (parts.length === 2 && req.method === "DELETE") {
            ac.accounts.splice(ac.accounts.indexOf(acc), 1);
            return json(200, {});
          }
          if (parts[2] === "check" && req.method === "POST")
            return json(200, { checked_at: "2026-09-30T00:00:00Z", result: "ok" });
          if (parts[2] === "login" && parts.length === 3 && req.method === "POST") {
            acc.login_pending = true;
            return json(200, {
              kind: "paste_code",
              url: "https://login.example/auth",
              expires_at: "2026-09-30T01:00:00Z",
              user_code: "ABCD-1234",
            });
          }
          if (parts[2] === "login" && parts.length === 3 && req.method === "DELETE") {
            acc.login_pending = false;
            return json(200, {});
          }
          if (parts[2] === "login" && parts[3] === "code" && req.method === "POST") {
            if (!input.code) return json(422, { detail: "code が空です" });
            acc.login_pending = false;
            acc.logged_in = true;
            return json(200, { result: "ok" });
          }
        }
        if (parts[0] === "secrets") {
          if (parts.length === 1 && req.method === "GET") return json(200, { items: ac.secrets });
          if (parts.length === 2 && req.method === "PUT") {
            if (!input.value) return json(422, { detail: "値が空です" });
            ac.secrets = ac.secrets.filter((s) => s.id !== parts[1]);
            const secret = {
              id: parts[1],
              fingerprint: `fp-${input.value.length}`,
              updated_at: "2026-09-30T00:00:00Z",
              used_by: [],
            };
            ac.secrets.push(secret);
            return json(200, { id: secret.id, fingerprint: secret.fingerprint, updated_at: secret.updated_at });
          }
          if (parts.length === 2 && req.method === "DELETE") {
            ac.secrets = ac.secrets.filter((s) => s.id !== parts[1]);
            return json(200, {});
          }
        }
        if (parts[0] === "llm" && req.method === "GET")
          return json(200, {
            sources: [
              {
                id: "claude-oauth",
                kind: "oauth",
                enabled: true,
                last_hour_completion_tokens: 0,
                last_hour_prompt_tokens: 0,
                last_hour_requests: 3,
                accounts: [{ id: "main", logged_in: true }],
              },
            ],
          });
        if (parts[0] === "mcp" && parts.length === 2 && req.method === "GET")
          return json(200, {
            items: [{ id: "c1", name: "editor", created_at: "2026-09-01T00:00:00Z", scopes: ["tasks:read"] }],
          });
        if (parts[0] === "mcp" && parts[3] === "calls" && req.method === "GET")
          return json(200, {
            items: [
              {
                id: "k1",
                client_id: parts[2],
                at: "2026-09-30T00:00:00Z",
                tool: "tasks_list",
                ok: true,
                latency_ms: 12,
              },
            ],
          });
        if (parts[0] === "clusters") {
          const cl = ac.clusters.find((c) => c.id === parts[1]);
          const row = (c) => ({
            concurrency: 1,
            delete_on_push: false,
            env_keys: [],
            has_setup: false,
            rsync_excludes: [],
            sync: "rsync",
            ...c,
          });
          if (parts.length === 1 && req.method === "GET") return json(200, { items: ac.clusters.map(row) });
          if (!cl) return json(404, { error: "not found" });
          if (parts[2] === "connect" && parts.length === 3 && req.method === "POST") {
            cl.connect_pending = true;
            return json(200, { kind: "needs_code", prompt: "Verification code:", expires_at: "2026-09-30T01:00:00Z" });
          }
          if (parts[2] === "connect" && parts.length === 3 && req.method === "DELETE") {
            cl.connect_pending = false;
            return json(200, {});
          }
          if (parts[2] === "connect" && parts[3] === "code" && req.method === "POST") {
            if (!input.code) return json(422, { detail: "code が空です" });
            cl.connect_pending = false;
            cl.connected = true;
            return json(200, { ok: true });
          }
          if (parts[2] === "settings" && req.method === "PUT") {
            cl.work_dir = input.work_dir ?? null;
            cl.work_dir_source = input.work_dir ? "db" : null;
            return json(200, { cluster_id: cl.id, updated_at: "2026-09-30T00:00:00Z", work_dir: cl.work_dir });
          }
        }
        return json(404, { error: "not found" });
      });
      return;
    }
    if (
      (pathname.startsWith("/api/v1/inbox/items") || /^\/api\/v1\/notifications(\/|$)/.test(pathname)) &&
      !Object.hasOwn(fixtures, pathname)
    ) {
      const chunks = [];
      req.on("data", (chunk) => chunks.push(chunk));
      req.on("end", () => {
        record.body = Buffer.concat(chunks).toString("utf8");
        let body = {};
        try {
          body = record.body ? JSON.parse(record.body) : {};
        } catch {
          body = null;
        }
        if (body === null) {
          res.writeHead(400, { "content-type": "application/json" });
          return res.end(JSON.stringify({ error: "invalid_json" }));
        }
        handleInboxNotifications(inbox, req, res, new URL(req.url ?? "/", "http://x"), body, sendEvent);
      });
      return;
    }
    const file = files[pathname];
    if (file) {
      // run のファイル・成果物（P1-08）。単一の `bytes=a-b` の Range だけを扱う。
      // body は関数でもよい（実行中の run の追記を試す。P3-12）。
      const body = Buffer.from(typeof file.body === "function" ? file.body() : file.body);
      const headers = { "content-type": file.type ?? "text/plain", "accept-ranges": "bytes" };
      if (file.disposition) headers["content-disposition"] = file.disposition;
      const range = /^bytes=(\d+)-(\d*)$/.exec(req.headers.range ?? "");
      if (range) {
        const start = Number(range[1]);
        const end = Math.min(range[2] ? Number(range[2]) : body.length - 1, body.length - 1);
        if (start >= body.length || end < start) {
          res.writeHead(416, { "content-range": `bytes */${body.length}` });
          return res.end();
        }
        res.writeHead(206, { ...headers, "content-range": `bytes ${start}-${end}/${body.length}` });
        return res.end(body.subarray(start, end + 1));
      }
      // `offset`（P3-12 の追い掛け）: offset == size は空本体、offset > size は 416。
      const offsetParam = new URL(req.url ?? "/", "http://x").searchParams.get("offset");
      if (offsetParam !== null) {
        const offset = Number(offsetParam);
        if (offset > body.length) {
          res.writeHead(416, { "content-range": `bytes */${body.length}` });
          return res.end();
        }
        res.writeHead(200, headers);
        return res.end(body.subarray(offset));
      }
      res.writeHead(200, headers);
      return res.end(body);
    }
    // Knowledge mutations use the same fake daemon and record the submitted body.
    if (
      /^\/api\/v1\/(knowledge\/(page|inbox\/[^/]+\/(accept|reject))|skills\/[^/]+)$/.test(pathname) &&
      req.method !== "GET"
    ) {
      const chunks = [];
      req.on("data", (chunk) => chunks.push(chunk));
      req.on("end", () => {
        record.body = Buffer.concat(chunks).toString("utf8");
        res.writeHead(req.method === "DELETE" ? 204 : 200, { "content-type": "application/json" });
        res.end(
          req.method === "DELETE" ? undefined : JSON.stringify({ path: pathname, sha: "fake-sha", unchanged: false }),
        );
      });
      return;
    }
    const raw = fixtures[pathname] ?? fixtures[pathname.replace(/^\/api\/v1/, "")];
    const value = typeof raw === "function" ? raw(new URL(req.url ?? "/", "http://x")) : raw;
    const respond = () => {
      if (res.destroyed) return;
      res.writeHead(value === undefined ? 404 : 200, { "content-type": "application/json" });
      res.end(JSON.stringify(value === undefined ? { error: "not found" } : value));
    };
    if (delay) setTimeout(respond, delay);
    else respond();
  };
  const sendEvent = (event, data = {}) => {
    const frame = `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`;
    for (const client of clients) client.write(frame);
  };
  return {
    requests,
    sendEvent,
    // 受信箱・通知の状態（ADR-0133）。試験が項目を差し替えたら合図を送る。
    inbox,
    setInboxItems(items) {
      inbox.items = items;
      sendEvent("inbox_changed", {});
    },
    setNotices(notices) {
      inbox.notices = notices;
      sendEvent("notifications_changed", {});
    },
    // `/api/v1/stream` の応答を 200 以外（503 など）にする（P1-09）。
    setStreamStatus(value) {
      streamStatus = value;
    },
    // Console（P3-01）: `/api/v1/console/stream` の接続へ block を流す・切る。POST の応答を遅らせる。
    sendConsoleBlock(block) {
      const frame = `event: console.block\ndata: ${JSON.stringify(block)}\n\n`;
      for (const client of consoleClients) client.write(frame);
    },
    dropConsoleClients() {
      for (const client of consoleClients) client.destroy();
    },
    // 状態の変種: 失敗の規則を替える・保留を解く・SSE の接続を切る。
    setFault(rule) {
      faultRule = rule;
    },
    releaseHeld() {
      holdRule = null;
      for (const run of held.splice(0)) run();
    },
    get heldCount() {
      return held.length;
    },
    dropStreamClients() {
      for (const client of clients) client.destroy();
    },
    setPostDelay(value) {
      postDelay = value;
    },
    get consoleClients() {
      return consoleClients.size;
    },
    get streamClients() {
      return clients.size;
    },
    setDelay(value) {
      if (!delayValues.has(value)) throw new Error("JSON delay must be 0, 5000 or 10000 ms");
      delay = value;
    },
    async start() {
      await new Promise((resolve, reject) => {
        server.once("error", reject);
        server.listen(port, host, resolve);
      });
      timer = setInterval(() => sendEvent("daemon", fixtureFor(schema.$defs.DaemonSnapshot)), 2000);
      return `http://${host === "::1" ? "[::1]" : host}:${server.address().port}`;
    },
    async close() {
      clearInterval(timer);
      for (const run of held.splice(0)) run();
      for (const client of [...clients, ...consoleClients]) client.destroy();
      await new Promise((resolve, reject) => server.close((error) => (error ? reject(error) : resolve())));
    },
  };
}
