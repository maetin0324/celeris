import type { RequestContext } from "celeris:host";
import { hostSelftest } from "./host_selftest";

export type DomainHandler = (input: unknown, ctx: RequestContext) => Promise<unknown>;

// 鍵は bundle.config.json の domains と同じにする（src/domains/domains.test.ts が確かめる）。
export const domains: Record<string, DomainHandler> = {
  host_selftest: hostSelftest,
};
