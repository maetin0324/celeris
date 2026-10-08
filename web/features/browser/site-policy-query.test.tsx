import { QueryClient, QueryClientProvider, QueryObserver } from "@tanstack/react-query";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { BrowserSitePolicyRecord, OrgNode } from "../../api/generated/types";
import { orgKeys } from "../../api/queries/keys";
import { BrowserGatewayError } from "./browser-query";
import { BrowserReadinessPanel, browserReadinessQuery, readinessView } from "./browser-readiness-panel";
import { SettingsForm } from "./browser-settings-screen";
import { SitePolicyForm } from "./site-policies-panel";
import {
  deleteSitePolicy,
  saveBrowserSettings,
  saveSitePolicy,
  sitePoliciesQuery,
  sitePolicyError,
} from "./site-policy-query";

const body = {
  exact_origin: "https://courses.example.com",
  login_url: "https://courses.example.com/login",
  password_selector: "#password",
  submit_selector: null,
};
const policy: BrowserSitePolicyRecord = {
  policy_id: "manaba",
  ...body,
  source: "api",
  created_at: "2026-10-08T00:00:00Z",
  updated_at: "2026-10-08T00:00:00Z",
};
afterEach(() => vi.unstubAllGlobals());

describe("site policy requests", () => {
  it("adds, replaces and deletes a policy and refetches the active list after each write", async () => {
    let items: BrowserSitePolicyRecord[] = [];
    const fetcher = vi.fn(async (_path: string, init: RequestInit) => {
      if (init.method === "GET") return Response.json({ items });
      if (init.method === "DELETE") {
        items = [];
        return Response.json({});
      }
      const sent = JSON.parse(String(init.body));
      items = [{ ...policy, ...sent }];
      return Response.json({ created: items.length === 0, policy: items[0] });
    });
    vi.stubGlobal("fetch", fetcher);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const observer = new QueryObserver(client, sitePoliciesQuery());
    const unsubscribe = observer.subscribe(() => {});
    await observer.refetch();
    await saveSitePolicy(client, "manaba", body, "csrf");
    expect(fetcher.mock.calls[1]).toEqual([
      "/browser/site-policies/manaba",
      expect.objectContaining({
        method: "PUT",
        credentials: "same-origin",
        body: JSON.stringify({ ...body, csrf: "csrf" }),
      }),
    ]);
    expect(observer.getCurrentResult().data?.items[0]?.login_url).toBe(body.login_url);
    const edited = { ...body, login_url: `${body.login_url}/new`, submit_selector: "#submit" };
    await saveSitePolicy(client, "manaba", edited, "csrf");
    expect(JSON.parse(String(fetcher.mock.calls[3]?.[1].body))).toEqual({ ...edited, csrf: "csrf" });
    expect(observer.getCurrentResult().data?.items[0]?.submit_selector).toBe("#submit");
    await deleteSitePolicy(client, "manaba", "csrf");
    expect(fetcher.mock.calls[5]?.[1].method).toBe("DELETE");
    expect(observer.getCurrentResult().data?.items).toEqual([]);
    expect(fetcher.mock.calls.filter(([, init]) => init.method === "GET")).toHaveLength(4);
    unsubscribe();
    client.clear();
  });
  it("patches credential_use and policy IDs and refreshes the organisation", async () => {
    const patch = { credential_use: true, credential_policy_ids: ["manaba"] };
    const fetcher = vi.fn(async (_path: string, init: RequestInit) =>
      Response.json(init.method === "PATCH" ? { id: "browser-execution" } : { items: [] }),
    );
    vi.stubGlobal("fetch", fetcher);
    const client = new QueryClient();
    const observer = new QueryObserver(client, {
      queryKey: orgKeys.list(),
      queryFn: () => fetch("/api/org", { method: "GET" }).then((r) => r.json()),
    });
    const unsubscribe = observer.subscribe(() => {});
    await observer.refetch();
    await saveBrowserSettings(client, patch, "csrf");
    expect(fetcher.mock.calls[1]?.[0]).toBe("/browser/settings");
    expect(JSON.parse(String(fetcher.mock.calls[1]?.[1].body))).toEqual({ ...patch, csrf: "csrf" });
    expect(fetcher.mock.calls).toHaveLength(3);
    await saveBrowserSettings(client, { credential_use: false, credential_policy_ids: [] }, "csrf");
    expect(JSON.parse(String(fetcher.mock.calls[3]?.[1].body))).toMatchObject({
      credential_use: false,
      credential_policy_ids: [],
    });
    unsubscribe();
    client.clear();
  });
  it("does not send without owner CSRF or with an invalid ID", async () => {
    const fetcher = vi.fn();
    vi.stubGlobal("fetch", fetcher);
    const client = new QueryClient();
    await expect(saveSitePolicy(client, "manaba", body, "")).rejects.toThrow("not_owner");
    await expect(deleteSitePolicy(client, "../escape", "csrf")).rejects.toThrow("policy ID");
    expect(fetcher).not.toHaveBeenCalled();
    client.clear();
  });
  it("keeps failed mutations from refetching or retrying and explains conflicts", async () => {
    const fetcher = vi.fn(async () => Response.json({ code: "site_policy_in_use" }, { status: 409 }));
    vi.stubGlobal("fetch", fetcher);
    const client = new QueryClient();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    await expect(deleteSitePolicy(client, "manaba", "csrf")).rejects.toMatchObject({ code: "site_policy_in_use" });
    expect(fetcher).toHaveBeenCalledTimes(1);
    expect(invalidate).not.toHaveBeenCalled();
    expect(sitePolicyError(new BrowserGatewayError(409, "site_policy_in_use"))).toContain("選択を外して保存");
    client.clear();
  });
});

describe("settings forms", () => {
  it("fills all site policy fields when editing and leaves the policy ID immutable", () => {
    const html = renderToStaticMarkup(<SitePolicyForm policy={policy} onSave={async () => {}} onCancel={() => {}} />);
    expect(html).toContain('readOnly=""');
    for (const value of ["manaba", body.exact_origin, body.login_url, "#password"]) expect(html).toContain(value);
    expect(html).toContain("submit selector（任意）");
    expect(html).toContain("ログイン先を保存");
    expect(
      renderToStaticMarkup(<SitePolicyForm policy={null} onSave={async () => {}} onCancel={() => {}} />),
    ).not.toContain('readOnly=""');
  });
  it("shows granted policies including missing IDs and credential permission from allowed_actions", () => {
    const client = new QueryClient();
    client.setQueryData(sitePoliciesQuery().queryKey, { items: [policy] });
    const node = {
      profile: {
        browser: {
          allowed_domains: [body.exact_origin],
          allowed_actions: ["credential_use"],
          credential_policy_ids: ["manaba", "deleted"],
        },
      },
    } as OrgNode;
    const html = renderToStaticMarkup(
      <QueryClientProvider client={client}>
        <SettingsForm node={node} csrf="csrf" onSaved={() => {}} />
      </QueryClientProvider>,
    );
    expect(html.match(/checked=""/g)).toHaveLength(3);
    expect(html).toContain("使用するたび");
    expect(html).toContain("未登録・選択を外して保存");
    client.clear();
  });
});

describe("browser readiness", () => {
  it("reads daemon readiness and renders diagnostics with Japanese states", async () => {
    const client = new QueryClient();
    const data = { items: [{ status: "NG", check: "ledger", detail: "browser の適合台帳が未配置" }] };
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => Response.json(data)),
    );
    expect(await client.fetchQuery(browserReadinessQuery())).toEqual(data);
    expect(fetch).toHaveBeenCalledWith("/api/browser/readiness", expect.objectContaining({ method: "GET" }));
    const html = renderToStaticMarkup(
      <QueryClientProvider client={client}>
        <BrowserReadinessPanel />
      </QueryClientProvider>,
    );
    expect(html).toContain("対応が必要");
    expect(html).toContain(data.items[0]?.detail);
    expect(readinessView("new-status")).toEqual({ label: "未確認", tone: "neutral" });
    client.clear();
  });
});
