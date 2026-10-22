import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import { configureApiClient } from "../../api/client";
import { fakeGateway } from "../../api/fake-gateway";
import type { RoutingEvidenceList, RoutingStageList } from "../../api/generated/types";
import { RoutingScreen, RoutingView, routingEvidenceQuery, routingStagesQuery } from "./routing-screen";

vi.mock("@tanstack/react-router", () => ({
  Link: ({ to, children, ...props }: { to: string; children: React.ReactNode }) => (
    <a href={to} {...props}>
      {children}
    </a>
  ),
}));

const stages: RoutingStageList = {
  stages: [
    {
      genre: "coding",
      harness_id: "codex",
      stage: "shadow",
      since: "2026-10-12T10:00:00Z",
      evidence_version: "ev-4",
      latest_evidence_version: "ev-5",
      last_change: {
        actor: "human",
        from: null,
        reason: "初期化",
        task_id: "T1",
        to: "shadow",
        ts: "2026-10-12T10:00:00Z",
        window: { days: 7, minimum_n: 30 },
      },
    },
  ],
};
const evidence: RoutingEvidenceList = {
  groups: [
    {
      genre: "coding",
      harness_id: "codex",
      evidence_version: "ev-5",
      rows: [
        {
          harness_version: "1.2",
          model_id: "model-x",
          n: 40,
          pass: 38,
          pass_lower: 0.82,
          pass_upper: 0.98,
          qualifies: true,
          quota_per_run: [],
          source_id: "source-a",
          timeout_rate: 0.025,
          usd_per_pass: 0.02,
          wall_p90_s: 12,
        },
        {
          // 定価の無い source・結果の無い cell は null（ADR 2026-10-11 付記 2026-10-13）。
          harness_version: "1.2",
          model_id: "model-y",
          n: 12,
          pass: 6,
          pass_lower: 0.25,
          pass_upper: 0.75,
          qualifies: false,
          quota_per_run: [],
          source_id: "source-b",
          timeout_rate: 0,
          usd_per_pass: null,
          wall_p90_s: null,
        },
      ],
    },
  ],
};

describe("RoutingView", () => {
  it("段階・根拠表・直近変更を fixture から表示し、承認経路を説明する", () => {
    const html = renderToStaticMarkup(<RoutingView stages={stages} evidence={evidence} />);
    expect(html).toContain("coding");
    expect(html).toContain("ev-4");
    expect(html).toContain("ev-5");
    expect(html).toContain("初期化");
    expect(html).toContain("source-a / model-x");
    expect(html).toContain("38/40");
    expect(html).toContain("12s / $0.02");
    expect(html).toContain("— / —");
    expect(html).toContain("celerisctl または API");
    expect(html).not.toContain("承認する");
  });
});

describe("RoutingScreen against the gateway", () => {
  afterEach(() => configureApiClient({ fetcher: (input, init) => fetch(input, init) }));

  it("requests /api/routing/* once-prefixed (daemon /api/v1/routing/*) and renders the values", async () => {
    const gateway = fakeGateway({ "/api/v1/routing/stages": stages, "/api/v1/routing/evidence": evidence });
    configureApiClient({ fetcher: gateway.fetcher });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    await Promise.all([client.fetchQuery(routingStagesQuery), client.fetchQuery(routingEvidenceQuery)]);
    expect(gateway.calls.map((c) => [c.method, c.path, c.daemonPath])).toEqual([
      ["GET", "/api/routing/stages", "/api/v1/routing/stages"],
      ["GET", "/api/routing/evidence", "/api/v1/routing/evidence"],
    ]);

    const html = renderToStaticMarkup(
      <QueryClientProvider client={client}>
        <RoutingScreen />
      </QueryClientProvider>,
    );
    expect(html).toContain("段階（1）");
    expect(html).toContain("source-a / model-x");
    expect(html).toContain("38/40");
  });
});
