import { describe, expect, it } from "vitest";
import "../main";
import { type CompareCandidate, type RoutePriorityInput, decide, parseInput, routePriority } from "./route_priority";

const ctx = { token: "01M6EJYCFFSJXYBVH87Q8FSNYB" };
const c = (source: string, model: string): CompareCandidate => ({ source, model });

const OPUS = c("pool", "opus");
const SONNET = c("anthropic", "sonnet");
const QWEN = c("local", "qwen");
const COS = c("anthropic", "opus-cos");

function input(over: Partial<RoutePriorityInput> = {}, table: Partial<RoutePriorityInput["table"]> = {}): RoutePriorityInput {
  return {
    harness: "claude_code",
    lane: "standard",
    cos: false,
    execution_class: null,
    ...over,
    table: {
      route_defaults: [SONNET, OPUS],
      lane_route_defaults: [
        { lane: "frontier", candidates: [OPUS] },
        { lane: "standard", candidates: [QWEN, SONNET] },
      ],
      class_defaults: [{ execution_class: "coding", candidates: [OPUS, SONNET] }],
      cos_route_candidates: [COS],
      ...table,
    },
  };
}

describe("route_priority basis", () => {
  it("cos takes the cos column before a declared class", () => {
    expect(decide(input({ cos: true, execution_class: "coding" }))).toEqual({ basis: "cos", candidates: [COS] });
  });

  it("an empty cos column is none and does not fall to a lower column", () => {
    expect(decide(input({ cos: true }, { cos_route_candidates: [] }))).toEqual({ basis: "none", candidates: [] });
  });

  it("a declared class replaces the lane and harness columns", () => {
    expect(decide(input({ execution_class: "coding" }))).toEqual({
      basis: "execution_class",
      candidates: [OPUS, SONNET],
    });
  });

  it("an empty class row is none and does not fall to the lane", () => {
    const t = { class_defaults: [{ execution_class: "coding", candidates: [] }] };
    expect(decide(input({ execution_class: "coding" }, t))).toEqual({ basis: "none", candidates: [] });
  });

  it("an unknown class falls to the lane column", () => {
    expect(decide(input({ execution_class: "review" }))).toEqual({ basis: "lane", candidates: [QWEN, SONNET] });
  });

  it("an unknown lane falls to the harness column", () => {
    expect(decide(input({ lane: "cheap" }))).toEqual({ basis: "harness", candidates: [SONNET, OPUS] });
  });

  it("the lane match is exact (case differs → harness)", () => {
    expect(decide(input({ lane: "Standard" })).basis).toBe("harness");
  });

  it("an empty lane row is none and does not fall to the harness column", () => {
    const t = { lane_route_defaults: [{ lane: "standard", candidates: [] }] };
    expect(decide(input({}, t))).toEqual({ basis: "none", candidates: [] });
  });

  it("a null lane looks up the empty-string row", () => {
    const t = { lane_route_defaults: [{ lane: "", candidates: [QWEN] }] };
    expect(decide(input({ lane: null }, t))).toEqual({ basis: "lane", candidates: [QWEN] });
    expect(decide(input({ lane: null })).basis).toBe("harness");
  });

  it("an empty harness column is none", () => {
    expect(decide(input({ lane: "cheap" }, { route_defaults: [] }))).toEqual({ basis: "none", candidates: [] });
  });

  it("prototype names are plain strings, not object keys", () => {
    expect(decide(input({ lane: "__proto__" })).basis).toBe("harness");
    expect(decide(input({ execution_class: "constructor" })).basis).toBe("lane");
    const t = { lane_route_defaults: [{ lane: "__proto__", candidates: [QWEN] }] };
    expect(decide(input({ lane: "__proto__" }, t))).toEqual({ basis: "lane", candidates: [QWEN] });
  });

  it("keeps order and duplicates as given", () => {
    const list = [OPUS, SONNET, OPUS];
    expect(decide(input({ lane: "cheap" }, { route_defaults: list })).candidates).toEqual(list);
  });

  it("the first row wins when a lane appears twice", () => {
    const t = {
      lane_route_defaults: [
        { lane: "standard", candidates: [QWEN] },
        { lane: "standard", candidates: [OPUS] },
      ],
    };
    expect(decide(input({}, t)).candidates).toEqual([QWEN]);
  });

  it("returns fresh candidate objects with only source and model", async () => {
    const i = input();
    const out = await routePriority(i);
    expect(Object.keys(out)).toEqual(["basis", "candidates"]);
    expect(Object.keys(out.candidates[0] ?? {})).toEqual(["source", "model"]);
    expect(out.candidates[0]).not.toBe(i.table.lane_route_defaults[1]?.candidates[0]);
  });

  it("is reachable through the dispatcher", async () => {
    await expect(globalThis.__celerisDispatch("route_priority", input(), ctx)).resolves.toEqual({
      basis: "lane",
      candidates: [QWEN, SONNET],
    });
  });
});

describe("route_priority input", () => {
  const bad = (mutate: (o: Record<string, any>) => void): unknown => {
    const o = JSON.parse(JSON.stringify(input())) as Record<string, any>;
    mutate(o);
    return o;
  };

  it.each([
    ["non-object input", () => [], /input must be an object/],
    ["null input", () => null, /input must be an object/],
    ["extra field", () => bad((o) => (o.extra = 1)), /unknown field in input: extra/],
    ["missing lane", () => bad((o) => delete o.lane), /missing field in input: lane/],
    ["missing execution_class", () => bad((o) => delete o.execution_class), /missing field in input: execution_class/],
    ["missing table", () => bad((o) => delete o.table), /missing field in input: table/],
    ["cos as string", () => bad((o) => (o.cos = "true")), /input.cos must be a boolean/],
    ["harness as number", () => bad((o) => (o.harness = 1)), /input.harness must be a string/],
    ["lane as number", () => bad((o) => (o.lane = 0)), /input.lane/],
    ["class as array", () => bad((o) => (o.execution_class = [])), /input.execution_class/],
    ["extra table field", () => bad((o) => (o.table.frontier = [])), /unknown field in input.table: frontier/],
    ["missing table field", () => bad((o) => delete o.table.class_defaults), /missing field in input.table: class_defaults/],
    ["column not an array", () => bad((o) => (o.table.route_defaults = {})), /table.route_defaults must be an array/],
    ["extra candidate field", () => bad((o) => (o.table.route_defaults[0].via = "proxy")), /unknown field in table.route_defaults\[0\]: via/],
    ["missing model", () => bad((o) => delete o.table.cos_route_candidates[0].model), /missing field in table.cos_route_candidates\[0\]: model/],
    ["model as null", () => bad((o) => (o.table.route_defaults[1].model = null)), /table.route_defaults\[1\].model must be a string/],
    ["lane row extra field", () => bad((o) => (o.table.lane_route_defaults[0].harness = "x")), /unknown field in table.lane_route_defaults\[0\]: harness/],
    ["lane row lane null", () => bad((o) => (o.table.lane_route_defaults[0].lane = null)), /lane_route_defaults\[0\].lane must be a string/],
    ["class row missing candidates", () => bad((o) => delete o.table.class_defaults[0].candidates), /missing field in table.class_defaults\[0\]: candidates/],
  ])("rejects %s", async (_name, make, error) => {
    const value = make();
    expect(() => parseInput(value)).toThrow(error);
    await expect(routePriority(value)).rejects.toThrow(error);
  });
});
