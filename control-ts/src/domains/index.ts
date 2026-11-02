import type { RequestContext } from "celeris:host";
import { cosDigest } from "./cos_digest";
import { hostSelftest } from "./host_selftest";
import { laneRules } from "./lane_rules";
import { routePriority } from "./route_priority";

export type DomainHandler = (input: unknown, ctx: RequestContext) => Promise<unknown>;

// 鍵は bundle.config.json の domains と同じにする（src/domains/domains.test.ts が確かめる）。
export const domains: Record<string, DomainHandler> = {
  cos_digest: cosDigest,
  host_selftest: hostSelftest,
  lane_rules: laneRules,
  route_priority: routePriority,
};
