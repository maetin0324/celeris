import { afterEach, expect, it } from "vitest";
import schema from "../../api/generated/schema.json";
// @ts-expect-error Local .mjs test support has no declaration.
import { createFakeDaemon, defaultFixtures, validateFixture } from "./fake-daemon.mjs";

let daemon: ReturnType<typeof createFakeDaemon> | undefined;
afterEach(async () => {
  if (daemon) await daemon.close();
  daemon = undefined;
});

it("provides responses that conform to the committed API schema", () => {
  for (const name of ["health", "inbox", "daemon"] as const) {
    expect(validateFixture(defaultFixtures[`/api/v1/${name}`], schema.properties[name])).toEqual([]);
  }
});

it("rejects non-loopback and reserved production or staging ports", () => {
  expect(() => createFakeDaemon({ host: "0.0.0.0" })).toThrow("loopback");
  for (const port of [7700, 7701, 7710, 7711, 7712]) expect(() => createFakeDaemon({ port })).toThrow("reserved port");
  expect(() => createFakeDaemon({ delayMs: 100 })).toThrow("JSON delay");
});

it("records JSON requests, sends SSE immediately, and marks aborted requests", async () => {
  daemon = createFakeDaemon();
  const url = await daemon.start();
  const health = await fetch(`${url}/health`);
  expect(health.status).toBe(200);
  expect(await health.json()).toEqual(defaultFixtures["/health"]);
  daemon.setDelay(5000);
  const stream = await fetch(`${url}/events`);
  expect(stream.headers.get("content-type")).toContain("text/event-stream");
  if (!stream.body) throw new Error("SSE body missing");
  const reader = stream.body.getReader();
  await reader.read();
  daemon.sendEvent("task.event", { task_id: "t1" });
  const frame = new TextDecoder().decode((await reader.read()).value);
  expect(frame).toContain("event: task.event");
  expect(frame).toContain("t1");
  const tick = new TextDecoder().decode((await reader.read()).value);
  expect(tick).toContain("event: daemon");
  const snapshot = JSON.parse(tick.match(/^data: (.+)$/m)?.[1] ?? "null");
  expect(validateFixture(snapshot, schema.$defs.DaemonSnapshot)).toEqual([]);
  await reader.cancel();
  const controller = new AbortController();
  const pending = fetch(`${url}/inbox`, { signal: controller.signal });
  await new Promise((resolve) => setTimeout(resolve, 30));
  controller.abort();
  await expect(pending).rejects.toThrow();
  await new Promise((resolve) => setTimeout(resolve, 30));
  expect(
    daemon.requests.some((request: { path: string; aborted: boolean }) => request.path === "/inbox" && request.aborted),
  ).toBe(true);
});
