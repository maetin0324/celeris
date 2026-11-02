// 試験だけで celeris:host の代わりに使う（vitest.config.mjs の alias）。bundle には入らない。
import type { HostErrorCode, RequestContext, SelftestEchoReply, SelftestEchoRequest } from "celeris:host";

export const HOST_API_VERSION = "1.0.0-stub";

export class HostError extends Error {
  readonly code: HostErrorCode;
  constructor(code: HostErrorCode, message: string) {
    super(message);
    this.name = "HostError";
    this.code = code;
  }
}

export function hostVersion(): string {
  return HOST_API_VERSION;
}

type Echo = (ctx: RequestContext, request: SelftestEchoRequest) => Promise<SelftestEchoReply>;
let echo: Echo = async () => {
  throw new HostError("denied", "selftestEcho is not set in this test");
};

export function setSelftestEcho(next: Echo): void {
  echo = next;
}

export function selftestEcho(ctx: RequestContext, request: SelftestEchoRequest): Promise<SelftestEchoReply> {
  return echo(ctx, request);
}
