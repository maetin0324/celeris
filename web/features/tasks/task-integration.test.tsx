import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { configureApiClient } from "../../api/client";
import type { InboxItem } from "../../api/generated/types";
import {
  InboxIntegrationActions,
  integrationActionPath,
  integrationLabel,
  TaskIntegrationActions,
} from "./task-integration";

const integration = {
  actor: "human",
  branch: "work",
  checked_at: "",
  head: "abc",
  repo: "web",
  repo_id: "repo-1",
  state: "unintegrated" as const,
};

describe("stranded task integrations", () => {
  it("distinguishes unintegrated and discarded states", () => {
    expect(integrationLabel("unintegrated")).toBe("未取り込み");
    expect(integrationLabel("discarded")).toBe("捨てた");
    expect(renderToStaticMarkup(<TaskIntegrationActions taskId="T1" integration={integration} />)).toContain(
      "未取り込み",
    );
    expect(integrationLabel("discarded")).toContain("捨てた");
  });

  it("renders task detail and inbox actions with the task API integration route", async () => {
    const calls: Array<{ path: string; body: unknown }> = [];
    configureApiClient({
      fetcher: async (input, init) => {
        calls.push({ path: String(input), body: init?.body ? JSON.parse(String(init.body)) : undefined });
        return new Response("{}", { status: 200 });
      },
    });
    const detail = renderToStaticMarkup(<TaskIntegrationActions taskId="T 1" integration={integration} />);
    const inboxItem = {
      age_secs: 0,
      answer: { body_schema: {}, method: "POST", path: "/api/v1/tasks/T%201/integrations/repo-1/merge" },
      blocked_by: [],
      blocking: {},
      created_at: "",
      id: "unintegrated-T1-repo-1",
      kind: "unintegrated",
      links: [],
      options: [],
      task: { id: "T 1", title: "task" },
      title: "未取り込み",
    } as unknown as InboxItem;
    const inbox = renderToStaticMarkup(<InboxIntegrationActions item={inboxItem} />);
    expect(detail).toContain("merge");
    expect(inbox).toContain("reopen");
    await (await import("../../api/client")).apiMutate("POST", integrationActionPath("T 1", "repo-1", "merge"), {});
    expect(calls).toEqual([{ path: "/api/v1/tasks/T%201/integrations/repo-1/merge", body: {} }]);
    configureApiClient({ fetcher: (input, init) => fetch(input, init) });
  });
});
