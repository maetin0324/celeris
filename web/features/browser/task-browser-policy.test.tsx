import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError, configureApiClient } from "../../api/client";
import type { Event, TaskDetail, Timeline, TimelineItem } from "../../api/generated/types";
import { browserKeys } from "./browser-query";
import { BrowserPrerequisiteNotice, TaskBrowserPolicyView } from "./task-browser-policy";
import {
  browserPrerequisite,
  editedPolicy,
  policyEditable,
  policyOriginLabel,
  policySaveError,
  saveTaskBrowserPolicy,
  type TaskBrowserPolicy,
  taskBrowserPolicyQuery,
} from "./task-browser-policy-model";
import { TaskBrowserSection } from "./task-browser-section";

vi.mock("@tanstack/react-router", () => ({
  Link: ({ to, children, ...props }: { to: string; children: React.ReactNode }) => (
    <a href={to} {...props}>
      {children}
    </a>
  ),
}));
afterEach(() => configureApiClient({ fetcher: (input, init) => fetch(input, init) }));
const policy: TaskBrowserPolicy = {
  policy_id: "auto",
  revision: 1,
  domain_mode: "common_hosts",
  navigation_origins: [],
  network_domains: ["https://manaba.example.ac.jp"],
  allowed_actions: ["navigate", "click", "credential_use"],
  approval_actions: ["navigate"],
  credential_policy_ids: ["manaba"],
  artifact_policy_id: "artifacts-safe",
};
const eventItem = (seq: number, event: Event): TimelineItem => ({
  kind: "event",
  seq,
  event,
  at: "2026-10-08T00:00:00Z",
});
const timeline = (...items: TimelineItem[]) => ({ items }) as Timeline;
const blocked = eventItem(2, {
  type: "browser_prerequisite_blocked",
  code: "missing",
  message: "browser の適合台帳が未配置",
});

describe("task browser policy", () => {
  it("shows sites, Japanese actions, credential policies, approval rules and auto provenance even before a run", () => {
    const out = renderToStaticMarkup(
      <TaskBrowserPolicyView policy={policy} editable taskId="T1" onSaved={async () => {}} />,
    );
    for (const value of [
      "https://manaba.example.ac.jp",
      "クリック",
      "manaba",
      "資格情報を使う",
      "自動付与",
      "過去の手動編集",
      "毎回",
      "ブラウザ policy を編集",
    ])
      expect(out).toContain(value);
    expect(out).not.toContain("password\u003d");
  });
  it("mounts policy in the task overview before any browser run exists", () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    client.setQueryData(taskBrowserPolicyQuery("T1").queryKey, { policy });
    client.setQueryData(browserKeys.owner, { available: true, isOwner: true, csrfToken: "csrf" });
    client.setQueryData(browserKeys.runs("T1"), { items: [] });
    client.setQueryData(browserKeys.waits("T1"), { items: [] });
    const detail = {
      task: { id: "T1", status: "ready", skills: ["browser-enabled"] },
      runs: [],
    } as unknown as TaskDetail;
    const out = renderToStaticMarkup(
      <QueryClientProvider client={client}>
        <TaskBrowserSection detail={detail} />
      </QueryClientProvider>,
    );
    expect(out).toContain('data-testid="task-browser-policy"');
    expect(out).toContain("ブラウザ policy を編集");
    expect(out).toContain("まだブラウザ実行はありません");
    client.clear();
  });
  it("shows missing policy and read-only state without offering editing", () => {
    const out = renderToStaticMarkup(
      <TaskBrowserPolicyView policy={null} editable={false} taskId="T1" onSaved={async () => {}} />,
    );
    expect(out).toContain("未設定");
    expect(out).not.toContain("ブラウザ policy を編集");
    expect(policyOriginLabel({ ...policy, policy_id: "web-human" })).toContain("人が web で編集");
    expect(policyOriginLabel({ ...policy, policy_id: "legacy" })).toContain("出自は未記録");
  });
  it("loads from the existing GET endpoint", async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValue(new Response(JSON.stringify({ policy }), { status: 200 }));
    configureApiClient({ fetcher });
    await expect(taskBrowserPolicyQuery("T1").queryFn({ signal: new AbortController().signal })).resolves.toEqual({
      policy,
    });
    expect(fetcher.mock.calls[0]?.[0]).toBe("/api/tasks/T1/browser/policy");
  });
  it("PUT sends the edited policy itself, marks human editing and retains restrictions outside the form", async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValue(new Response('{"updated":true}', { status: 200 }));
    configureApiClient({ fetcher });
    const edited = editedPolicy(
      policy,
      " https://manaba.example.ac.jp\nhttps://files.example.ac.jp\nhttps://files.example.ac.jp",
      "manaba\nother",
      ["click", "credential_use"],
    );
    await saveTaskBrowserPolicy("T1", edited);
    const [path, init] = fetcher.mock.calls[0] ?? [];
    expect(path).toBe("/api/tasks/T1/browser/policy");
    expect(init?.method).toBe("PUT");
    expect(JSON.parse(String(init?.body))).toEqual({
      ...policy,
      policy_id: "web-human",
      revision: 2,
      network_domains: ["https://manaba.example.ac.jp", "https://files.example.ac.jp"],
      credential_policy_ids: ["manaba", "other"],
      allowed_actions: ["click", "credential_use"],
      approval_actions: [],
    });
    expect(init?.credentials).toBe("same-origin");
  });
  it("creates a policy when none is saved and sends no credential secret", () => {
    expect(editedPolicy(null, "https://example.com", "", ["navigate"])).toEqual({
      policy_id: "web-human",
      revision: 1,
      domain_mode: "common_hosts",
      navigation_origins: [],
      network_domains: ["https://example.com"],
      credential_policy_ids: [],
      allowed_actions: ["navigate"],
      approval_actions: [],
    });
  });
  it("does not retry a failed PUT and separates permission, validation, conflict and unknown results", async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValue(new Response("{}", { status: 409 }));
    configureApiClient({ fetcher });
    await expect(saveTaskBrowserPolicy("T1", policy)).rejects.toMatchObject({ kind: "conflict" });
    expect(fetcher).toHaveBeenCalledTimes(1);
    for (const [kind, text] of [
      ["forbidden", "権限"],
      ["validation", "入力"],
      ["conflict", "状態"],
      ["timeout", "保存結果"],
      ["network", "再送する前"],
    ] as const) {
      expect(policySaveError(new ApiError(kind, { method: "PUT", path: "/api/tasks/T1/browser/policy" }))).toContain(
        text,
      );
    }
  });
});

describe("browser prerequisite", () => {
  it("uses the latest sequenced event and stops showing it after resume or a different block", () => {
    expect(browserPrerequisite("blocked", timeline(blocked))?.code).toBe("missing");
    expect(browserPrerequisite("ready", timeline(blocked))).toBeNull();
    expect(
      browserPrerequisite(
        "blocked",
        timeline(eventItem(3, { type: "browser_prerequisite_resumed", code: "ok" }), blocked),
      ),
    ).toBeNull();
    expect(
      browserPrerequisite(
        "blocked",
        timeline(
          blocked,
          eventItem(4, { type: "transitioned", from: "ready", to: "blocked", reason: "waiting_for_auth" }),
        ),
      ),
    ).toBeNull();
    expect(browserPrerequisite("blocked", undefined)).toBeNull();
  });
  it("permits editing only in draft, ready or a confirmed prerequisite block", () => {
    for (const status of ["running", "reviewing", "done", "failed", "cancelled"] as const)
      expect(policyEditable(status, true)).toBe(false);
    expect(policyEditable("draft", false)).toBe(true);
    expect(policyEditable("ready", false)).toBe(true);
    expect(policyEditable("blocked", false)).toBe(false);
    expect(policyEditable("blocked", true)).toBe(true);
  });
  it("shows the ledger reason, browser settings and readable operations instructions", () => {
    const out = renderToStaticMarkup(
      <BrowserPrerequisiteNotice message="browser の適合台帳が未配置" policyMissing={false} />,
    );
    for (const value of [
      "適合台帳が未配置",
      'href="/browser/settings"',
      "運用手順を開く",
      "browser doctor",
      "browser-ledger.sh",
      "docs/ops/browser-prod.md",
      "自動で再開",
    ])
      expect(out).toContain(value);
    const missing = renderToStaticMarkup(<BrowserPrerequisiteNotice message="policy が未設定" policyMissing />);
    expect(missing).toContain("必要なサイトを入力");
  });
});
