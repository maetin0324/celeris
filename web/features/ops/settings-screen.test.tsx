import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import { configureApiClient } from "../../api/client";
import { fakeGateway } from "../../api/fake-gateway";
import type { ConfigEventList, ConfigFieldsView } from "../../api/generated/types";
import {
  configEventsQuery,
  configFieldsQuery,
  parseInput,
  patchConfigField,
  restartMessage,
  ruleFor,
  SettingsScreen,
  SettingsView,
} from "./settings-screen";

vi.mock("@tanstack/react-router", () => ({
  Link: ({ to, children, ...props }: { to: string; children: React.ReactNode }) => (
    <a href={to} {...props}>
      {children}
    </a>
  ),
}));

const rules: ConfigFieldsView["rules"] = [
  { key: "execution", class: "dynamic", apply_unit: "next_dispatch", reason: "次の判定から" },
  { key: "accounts", class: "startup", apply_unit: "restart", reason: "根ディレクトリ" },
  {
    key: "accounts.max_runs_per_account",
    class: "dynamic",
    apply_unit: "next_dispatch",
    reason: "同時 run 数の既定",
  },
  { key: "db", class: "startup", apply_unit: "restart", reason: "DB の path" },
  { key: "llm_proxy", class: "startup", apply_unit: "restart", reason: "listener" },
  { key: "cron", class: "seed", apply_unit: "elsewhere", reason: "定期実行の種" },
];

const fields: ConfigFieldsView = {
  config_path: "/home/u/.config/celeris/config.toml",
  rules,
  fields: [
    {
      key: "execution.max_replans",
      class: "dynamic",
      apply_unit: "next_dispatch",
      reason: "次の判定から",
      secret: false,
      value: 3,
      running: 3,
      pending_restart: false,
    },
    {
      key: "db",
      class: "startup",
      apply_unit: "restart",
      reason: "DB の path",
      secret: false,
      value: "moved.db",
      running: "celeris.db",
      pending_restart: true,
    },
    {
      key: "llm_proxy.sources.openai_compatible",
      class: "startup",
      apply_unit: "restart",
      reason: "listener",
      secret: false,
      value: [{ id: "qwen", api_key: "<redacted>" }],
      running: [{ id: "qwen", api_key: "<redacted>" }],
      pending_restart: false,
    },
    {
      key: "llm_proxy.token",
      class: "startup",
      apply_unit: "restart",
      reason: "listener",
      secret: true,
      value: "<redacted>",
      running: "<redacted>",
      pending_restart: false,
    },
  ],
};

const events: ConfigEventList = {
  items: [
    {
      id: 2,
      ts: "2026-10-22T10:00:00Z",
      actor: "admin",
      key: "execution.max_replans",
      class: "dynamic",
      apply_unit: "next_dispatch",
      before: 2,
      after: 3,
    },
  ],
};

describe("SettingsView", () => {
  const html = renderToStaticMarkup(<SettingsView fields={fields} events={events} onSave={() => {}} />);

  it("lets the dynamic field be edited and shows when it takes effect", () => {
    expect(html).toContain('id="config-execution.max_replans"');
    expect(html).toContain('value="3"');
    expect(html).toContain("次の dispatch（次に起動する run）から");
  });

  it("marks startup fields as needing a restart and shows pending restarts", () => {
    expect(html).toContain("変更には再起動が要ります");
    expect(html).toContain("再起動待ち");
    expect(html).toContain("稼働中: celeris.db");
    // 起動時のみの欄に入力欄は出さない。
    expect(html).not.toContain('id="config-db"');
  });

  it("never renders secret values", () => {
    expect(html).not.toContain("sk-");
    expect(html).toContain("秘密の値は表示しません");
  });

  it("shows the change history", () => {
    expect(html).toContain("変更の履歴（1）");
    expect(html).toContain("2 → 3");
    expect(html).toContain("admin");
  });
});

describe("settings helpers", () => {
  it("looks rules up by the longest dotted prefix", () => {
    expect(ruleFor(rules, "accounts.claude_dir")?.class).toBe("startup");
    expect(ruleFor(rules, "accounts.max_runs_per_account")?.class).toBe("dynamic");
    expect(ruleFor(rules, "executionx")).toBeUndefined();
  });

  it("tells the human a restart is needed before sending a startup field", () => {
    expect(restartMessage("db", ruleFor(rules, "db"))).toContain("再起動");
    expect(restartMessage("cron.seed", ruleFor(rules, "cron.seed"))).toContain("DB が正");
    expect(restartMessage("execution.max_replans", ruleFor(rules, "execution.max_replans"))).toBeNull();
  });

  it("parses input as JSON, falling back to a string, empty as null", () => {
    expect(parseInput("7")).toBe(7);
    expect(parseInput('["a","b"]')).toEqual(["a", "b"]);
    expect(parseInput("decision_assisted")).toBe("decision_assisted");
    expect(parseInput("  ")).toBeNull();
  });
});

describe("SettingsScreen against the gateway", () => {
  afterEach(() => configureApiClient({ fetcher: (input, init) => fetch(input, init) }));

  it("requests /api/config/* once-prefixed (daemon /api/v1/config/*) and renders the values", async () => {
    const gateway = fakeGateway({
      "/api/v1/config/fields": fields,
      "/api/v1/config/events": events,
    });
    configureApiClient({ fetcher: gateway.fetcher });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    await Promise.all([client.fetchQuery(configFieldsQuery), client.fetchQuery(configEventsQuery)]);
    expect(gateway.calls.map((c) => [c.method, c.path, c.daemonPath])).toEqual([
      ["GET", "/api/config/fields", "/api/v1/config/fields"],
      ["GET", "/api/config/events", "/api/v1/config/events"],
    ]);

    const html = renderToStaticMarkup(
      <QueryClientProvider client={client}>
        <SettingsScreen />
      </QueryClientProvider>,
    );
    expect(html).toContain("/home/u/.config/celeris/config.toml");
    expect(html).toContain('id="config-execution.max_replans"');
    expect(html).toContain("変更の履歴（1）");
  });

  it("sends PATCH to /api/config/fields", async () => {
    const result = { changes: [] };
    const gateway = fakeGateway({ "/api/v1/config/fields": result });
    configureApiClient({ fetcher: gateway.fetcher });
    await expect(patchConfigField("execution.max_replans", 4)).resolves.toEqual(result);
    expect(gateway.calls).toEqual([
      {
        method: "PATCH",
        path: "/api/config/fields",
        daemonPath: "/api/v1/config/fields",
        body: { changes: [{ key: "execution.max_replans", value: 4 }] },
      },
    ]);
  });
});
