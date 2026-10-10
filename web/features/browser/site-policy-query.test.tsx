import { QueryClient, QueryClientProvider, QueryObserver } from "@tanstack/react-query";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { BrowserSitePolicyRecord, OrgNode } from "../../api/generated/types";
import { orgKeys } from "../../api/queries/keys";
import { BrowserGatewayError } from "./browser-query";
import { BrowserReadinessPanel, browserReadinessQuery, readinessView } from "./browser-readiness-panel";
import { SettingsForm } from "./browser-settings-screen";
import { SitePolicyForm, sitePolicyBody } from "./site-policies-panel";
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

describe("site policy body (username selector and post-login read)", () => {
  const fields = {
    origin: " https://idp.example.ac.jp ",
    login: "https://idp.example.ac.jp/idp/login",
    password: 'input[name="j_password"]',
    submit: "",
    username: ' input[name="j_username"] ',
    postLogin: false,
    readOrigins: "",
    actions: [] as Array<"snapshot" | "extract" | "screenshot" | "download" | "click">,
    acknowledged: false,
  };
  it("sends the username selector and no post-login read unless opted in", () => {
    expect(sitePolicyBody(fields)).toEqual({
      body: {
        exact_origin: "https://idp.example.ac.jp",
        login_url: "https://idp.example.ac.jp/idp/login",
        password_selector: 'input[name="j_password"]',
        submit_selector: null,
        username_selector: 'input[name="j_username"]',
        post_login: null,
        consent: null,
      },
    });
    expect(sitePolicyBody({ ...fields, username: " " })).toMatchObject({ body: { username_selector: null } });
  });
  it("requires origins, actions and the LLM acknowledgement before sending a post-login read", () => {
    const on = { ...fields, postLogin: true };
    expect(sitePolicyBody(on)).toEqual({ error: "ログイン後に読み取る origin を 1 つ以上入れてください。" });
    const origins = { ...on, readOrigins: " https://lms.example.ac.jp\n\nhttps://lms2.example.ac.jp " };
    expect(sitePolicyBody(origins)).toEqual({ error: "ログイン後に許す操作を 1 つ以上選んでください。" });
    const picked = { ...origins, actions: ["click", "snapshot"] as typeof fields.actions };
    expect(sitePolicyBody(picked)).toMatchObject({ error: expect.stringContaining("LLM") });
    expect(sitePolicyBody({ ...picked, acknowledged: true })).toMatchObject({
      body: {
        post_login: {
          read_origins: ["https://lms.example.ac.jp", "https://lms2.example.ac.jp"],
          actions: ["snapshot", "click"],
        },
      },
    });
  });
  it("sends the pinned consent button only with a post-login read", () => {
    const base = {
      ...fields,
      postLogin: true,
      readOrigins: "https://lms.example.ac.jp",
      actions: ["snapshot"] as typeof fields.actions,
      acknowledged: true,
    };
    expect(
      sitePolicyBody({
        ...base,
        consentSelector: ' input[name="_eventId_proceed"] ',
        consentChoice: 'input[value="_shib_idp_doNotRememberConsent"]',
      }),
    ).toMatchObject({
      body: {
        consent: {
          selector: 'input[name="_eventId_proceed"]',
          choice_selector: 'input[value="_shib_idp_doNotRememberConsent"]',
        },
      },
    });
    expect(sitePolicyBody({ ...base, consentChoice: "#x" })).toMatchObject({
      error: expect.stringContaining("ボタン"),
    });
    expect(sitePolicyBody({ ...fields, consentSelector: "#accept" })).toMatchObject({
      error: expect.stringContaining("ログイン後の読み取りと組"),
    });
    expect(sitePolicyBody(base)).toMatchObject({ body: { consent: null } });
  });
  it("shows the username selector and the post-login opt-in when editing", () => {
    const html = renderToStaticMarkup(
      <SitePolicyForm
        policy={{
          ...policy,
          username_selector: "#user",
          post_login: { read_origins: ["https://lms.example.ac.jp"], actions: ["snapshot"] },
        }}
        onSave={async () => {}}
        onCancel={() => {}}
      />,
    );
    expect(html).toContain("username selector（任意。password 欄と同じ頁）");
    expect(html).toContain("#user");
    expect(html).toContain("https://lms.example.ac.jp");
    expect(html).toContain("個人情報を含みうる");
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
