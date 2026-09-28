import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { loadBrowserRuns } from "~/celeris/browser";
import type { CelerisClient } from "~/celeris/client.server";
import { CelerisError, CelerisUnavailable } from "~/celeris/errors";
import type { BrowserRun, EventRow, RunSummary } from "~/celeris/types";
import { BrowserRunsPanel } from "~/components/BrowserRunsPanel";
import { activeBrowserRunIds, safeBrowserLiveUrl } from "~/lib/browser";

function browser(overrides: Partial<BrowserRun> = {}): BrowserRun {
  return {
    task_id: "T1",
    run_id: "R1",
    session_id: "isolated-r1",
    state: "RUNNING",
    live_view_url: "https://browser.example/dashboard",
    ...overrides,
  } as BrowserRun;
}
function row(seq: number, overrides: Partial<BrowserRun> = {}): EventRow {
  return {
    id: seq,
    seq,
    task_id: "T1",
    ts: "2026-09-28T00:00:00Z",
    event: { type: "browser_updated", browser: browser(overrides) },
  };
}

it.each([
  "javascript:alert(1)",
  "http://127.0.0.1:9222",
  "https://user:password@example.com",
  "https://example.com/?token=secret",
  "https://example.com/#token",
  "//example.com",
  " https://example.com",
  "https://example.com/\npath",
  "https://example.com/\\path",
])("rejects unsafe dashboard URL %s", (url) => {
  expect(safeBrowserLiveUrl(url)).toBeNull();
});

it("allows an operator HTTPS dashboard without credentials or capability tokens", () => {
  expect(safeBrowserLiveUrl("https://browser.example/dashboard")).toBe("https://browser.example/dashboard");
});

it("fails closed for terminal task/run and unknown run", () => {
  const run = { run_id: "R1" } as RunSummary;
  expect(activeBrowserRunIds([run], "running")).toEqual(["R1"]);
  expect(activeBrowserRunIds([run], "cancelled")).toEqual([]);
  expect(activeBrowserRunIds([run], "done")).toEqual([]);
  expect(activeBrowserRunIds([{ ...run, finished_at: "now" }], "running")).toEqual([]);
  expect(activeBrowserRunIds([{ ...run, outcome: "done" }], "running")).toEqual([]);
  expect(activeBrowserRunIds([], "running")).toEqual([]);
});

it("renders only the server-authorized same-origin live path, never the raw dashboard URL", () => {
  const link = { R1: { state: "link", href: "/browser/live/T1/R1" } } as const;
  const html = renderToStaticMarkup(<BrowserRunsPanel runs={[browser()]} liveViews={link} />);
  expect(html).toContain('href="/browser/live/T1/R1"');
  expect(html).toContain('rel="noopener noreferrer"');
  expect(html).toContain("Open Browser Live View");
  expect(html).toContain("isolated-r1");
  expect(html).not.toContain("browser.example");
  // 同一 origin の `/browser/live/...` 以外は href にしない
  const bad = { R1: { state: "link", href: "https://browser.example/dashboard" } } as const;
  expect(renderToStaticMarkup(<BrowserRunsPanel runs={[browser()]} liveViews={bad} />)).not.toContain("href=");
  for (const reason of ["not_owner", "owner_unavailable", "not_running", "relay_unavailable"] as const) {
    const out = renderToStaticMarkup(
      <BrowserRunsPanel runs={[browser()]} liveViews={{ R1: { state: "disabled", reason } }} />,
    );
    expect(out).not.toContain("href=");
    expect(out).not.toContain("browser.example");
  }
  expect(renderToStaticMarkup(<BrowserRunsPanel runs={[browser()]} liveViews={{}} />)).toContain("実行中のみ");
});

describe("dedicated browser lifecycle feed", () => {
  it("accepts only the explicit N-1 unknown browser event response as absent capability", async () => {
    const error = new CelerisError({
      status: 400,
      code: "invalid_query",
      detail: "unknown event type `browser_updated`",
    });
    const get = vi.fn().mockRejectedValue(error);
    expect(await loadBrowserRuns({ get } as unknown as CelerisClient, "T1", new AbortController().signal)).toEqual([]);
  });
  it.each([
    new CelerisUnavailable("http://127.0.0.1"),
    new CelerisError({ status: 500, code: "internal", detail: "unknown event type `browser_updated`" }),
    new CelerisError({ status: 400, code: "invalid_query", detail: "unknown event type `another_type`" }),
    new CelerisError({ status: 400, code: "invalid_query", detail: "bad limit" }),
    new Error("unknown event type `browser_updated`"),
  ])("preserves unrelated browser feed errors: %s", async (error) => {
    const get = vi.fn().mockRejectedValue(error);
    await expect(loadBrowserRuns({ get } as unknown as CelerisClient, "T1", new AbortController().signal)).rejects.toBe(
      error,
    );
  });
  it("paginates independently from general events and takes the last state for each run", async () => {
    const get = vi
      .fn()
      .mockResolvedValueOnce({ has_more: true, items: [row(1)] })
      .mockResolvedValueOnce({
        has_more: false,
        items: [row(8000, { state: "COMPLETED" }), row(9000, { run_id: "R2" })],
      });
    const runs = await loadBrowserRuns({ get } as unknown as CelerisClient, "T1", new AbortController().signal);
    expect(runs.map((r) => [r.run_id, r.state])).toEqual([
      ["R1", "COMPLETED"],
      ["R2", "RUNNING"],
    ]);
    expect(get.mock.calls[0][1].query).toEqual({ types: "browser_updated", after_seq: -1, limit: 5000 });
    expect(get.mock.calls[1][1].query.after_seq).toBe(1);
  });
  it("ignores forged progress and events for another task", async () => {
    const get = vi.fn().mockResolvedValue({
      has_more: false,
      items: [
        row(1, { task_id: "T2" }),
        { ...row(2), event: { type: "worker_progress", msg: "https://attacker.example", run_id: "R1" } },
      ],
    });
    expect(await loadBrowserRuns({ get } as unknown as CelerisClient, "T1", new AbortController().signal)).toEqual([]);
  });
  it("rejects partial history rather than displaying a stale RUNNING link", async () => {
    const get = vi
      .fn()
      .mockResolvedValueOnce({ has_more: true, items: [row(1)] })
      .mockRejectedValueOnce(new Error("unavailable"));
    await expect(
      loadBrowserRuns({ get } as unknown as CelerisClient, "T1", new AbortController().signal),
    ).rejects.toThrow("unavailable");
  });
});
