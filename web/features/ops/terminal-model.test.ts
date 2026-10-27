import { describe, expect, it } from "vitest";
import { closeReasonLabel, ctrlChar, nextTab, targetLabel, terminalErrorLabel, terminalWsUrl } from "./terminal-model";

describe("terminal model", () => {
  it("builds a same-origin websocket url", () => {
    expect(terminalWsUrl({ protocol: "http:", host: "localhost:7720" }, "local", 80, 24)).toBe(
      "ws://localhost:7720/terminal/ws?target=local&cols=80&rows=24",
    );
    expect(terminalWsUrl({ protocol: "https:", host: "c.ts.net" }, "cluster:sirius", 100, 40)).toBe(
      "wss://c.ts.net/terminal/ws?target=cluster%3Asirius&cols=100&rows=40",
    );
  });

  it("maps control keys", () => {
    expect(ctrlChar("c")).toBe("\x03");
    expect(ctrlChar("D")).toBe("\x04");
    expect(ctrlChar("[")).toBe("\x1b");
    expect(ctrlChar("1")).toBeNull();
    expect(ctrlChar("ab")).toBeNull();
  });

  it("labels reasons, errors and targets", () => {
    expect(closeReasonLabel("idle_timeout")).toContain("上限");
    expect(closeReasonLabel("gateway_shutdown")).toContain("昇格");
    expect(closeReasonLabel(undefined)).toBe("終了しました");
    expect(terminalErrorLabel("cluster_not_connected")).toContain("接続");
    expect(terminalErrorLabel("terminal_entry_not_allowed")).toContain("LAN");
    expect(terminalErrorLabel("weird")).toContain("weird");
    expect(targetLabel("local", "host1")).toBe("このマシン（host1）");
    expect(targetLabel("cluster:pegasus")).toBe("pegasus");
  });

  it("opens several tabs for the same target", () => {
    const one = nextTab([], "local", "このマシン", 1);
    const two = nextTab(one, "local", "このマシン", 2);
    expect(two.map((t) => t.key)).toEqual(["local#1", "local#2"]);
  });
});
