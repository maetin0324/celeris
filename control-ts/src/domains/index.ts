import type { RequestContext } from "celeris:host";
import { cosDigestPass } from "./cos_digest";
import { executionGate } from "./execution_gate";
import { hostSelftest } from "./host_selftest";
import { laneRules } from "./lane_rules";
import { projectController } from "./project_controller";
import { routePriority } from "./route_priority";

export type DomainHandler = (input: unknown, ctx: RequestContext) => Promise<unknown>;

// 鍵は bundle.config.json の domains と同じにする（src/domains/domains.test.ts が確かめる）。
export const domains: Record<string, DomainHandler> = {
  // pass は入力を取らない（読み出しは op で行う。付記 S4-12）。
  cos_digest: (_input, ctx) => cosDigestPass(ctx),
  execution_gate: executionGate,
  host_selftest: hostSelftest,
  lane_rules: laneRules,
  // 入力は Rust の ProjectSnapshot。DB も system も書かない（ADR 2026-11-04-project-controller D7）。
  project_controller: async (input) => projectController(input),
  route_priority: routePriority,
};
