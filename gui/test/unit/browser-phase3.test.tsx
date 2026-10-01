import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, expect, it, vi } from "vitest";
import { CelerisClient } from "~/celeris/client.server";
import type { CelerisError } from "~/celeris/errors";
import type { BrowserRun } from "~/celeris/types";
import { BrowserControl, ControlActionGate, controlMessage } from "~/components/BrowserControl";
import { BrowserRunsPanel } from "~/components/BrowserRunsPanel";
import { type MockCeleris, serveBrowserPhase3, startMockCeleris } from "../mock-celeris/server";

const run = { task_id: "T1", run_id: "R1", session_id: "S1", state: "RUNNING" } as BrowserRun;
let mock: MockCeleris | undefined;
afterEach(async () => {
  await mock?.close();
  mock = undefined;
});

it("shows an authentication interval and disables control for non-owner sessions", () => {
  const html = renderToStaticMarkup(
    <BrowserRunsPanel runs={[run]} liveViews={{ R1: { state: "disabled", reason: "auth_interval" } }} />,
  );
  expect(html).toContain("認証を扱う区間");
  expect(html).toContain("本人のセッション");
  expect(controlMessage("version_conflict")).toContain("状態が変わりました");
  expect(controlMessage("not_converged")).toContain("収束していません");
  expect(renderToStaticMarkup(<BrowserControl run={run} csrfToken="csrf" authInterval />)).toContain(
    "制御状態を読み込み中",
  );
});

it("coalesces a double click into one request with one idempotency key", async () => {
  const gate = new ControlActionGate();
  let finish!: () => void;
  const pending = new Promise<void>((resolve) => {
    finish = resolve;
  });
  const send = vi.fn(async (_key: string) => pending);
  const first = gate.run(send);
  const second = gate.run(send);
  expect(first).toBe(second);
  await Promise.resolve();
  expect(send).toHaveBeenCalledTimes(1);
  expect(send.mock.calls[0][0]).toMatch(/^[\w-]{36}$/);
  finish();
  await first;
});

it("mock control and identity endpoints preserve version, lease and metadata without cookies", async () => {
  mock = await startMockCeleris();
  serveBrowserPhase3(mock);
  const client = new CelerisClient({ baseUrl: mock.baseUrl });
  const path = "/tasks/T1/browser/control/R1/S1";
  expect((await client.get<{ phase: string }>(path)).phase).toBe("paused");
  const takeover = await client.post<{ phase: string; lease_expires_at: number }>(path, {
    command: { kind: "takeover", ttl_secs: 60 },
    expected_version: 2,
    idempotency_key: "same-key",
  });
  expect(takeover.phase).toBe("human_control");
  expect(takeover.lease_expires_at).toBeGreaterThan(0);
  await expect(
    client.post(path, { command: { kind: "pause" }, expected_version: 1, idempotency_key: "other-key" }),
  ).rejects.toMatchObject({ code: "version_conflict" } satisfies Partial<CelerisError>);
  await expect(
    client.post(path, { command: { kind: "pause" }, expected_version: 2, idempotency_key: "other-key" }),
  ).rejects.toMatchObject({ code: "not_converged" } satisfies Partial<CelerisError>);
  const identities = await client.get<{ identities: unknown[] }>("/browser/identities", {
    query: { project_id: "P1" },
  });
  expect(JSON.stringify(identities)).not.toMatch(/cookie|ciphertext|sealed/);
  expect(identities.identities).toHaveLength(1);
  expect(await client.post("/browser/identities/I1/revoke")).toMatchObject({ identity: { state: "revoked" } });
  expect(await client.delete("/browser/identities/I1")).toMatchObject({ identity: { state: "deleted" } });
});
