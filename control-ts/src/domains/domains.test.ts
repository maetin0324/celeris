import type { SelftestEchoRequest } from "celeris:host";
import { describe, expect, it } from "vitest";
import config from "../../bundle.config.json";
import "../main";
import * as stub from "../testing/host-stub";
import { domains } from "./index";

// 差し替えが celeris:host の宣言に合っていることを型検査で確かめる。
const _stubMatchesHost: typeof import("celeris:host") = stub;

const ctx = { token: "01M6DJ6QCMCWT80MTYJ7SNE0XK" };
const request: SelftestEchoRequest = {
  task_id: "01M6DJ6QCMCWT80MTYJ7SNE0XK",
  note: "n",
  tokens: "18446744073709551615",
  priority: "normal",
  at: "2026-11-02T00:00:00Z",
};

describe("host_selftest", () => {
  it("passes the request context to the host and returns the reply", async () => {
    let seen: unknown;
    stub.setSelftestEcho(async (c, req) => {
      seen = c;
      return { ...req, observation: { kind: "noted", length: 1 } };
    });
    const out = await globalThis.__celerisDispatch("host_selftest", request, ctx);
    expect(seen).toEqual(ctx);
    expect(out).toMatchObject({ host_api: stub.HOST_API_VERSION, reply: { tokens: "18446744073709551615" } });
  });

  it("does not swallow a host error", async () => {
    stub.setSelftestEcho(async () => {
      throw new stub.HostError("denied", "out of scope");
    });
    await expect(globalThis.__celerisDispatch("host_selftest", request, ctx)).rejects.toMatchObject({
      name: "HostError",
      code: "denied",
    });
  });
});

describe("dispatch", () => {
  it("rejects an unknown domain", async () => {
    await expect(globalThis.__celerisDispatch("nope", {}, ctx)).rejects.toThrow("unknown domain: nope");
  });

  it("has the domains that bundle.config.json declares", () => {
    expect(Object.keys(domains).sort()).toEqual(Object.keys(config.domains).sort());
  });
});
