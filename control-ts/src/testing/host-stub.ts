// 試験だけで celeris:host の代わりに使う（vitest.config.mjs の alias）。bundle には入らない。
import type {
  CosDigestInput,
  CosDigestListRunsReply,
  CosDigestListRunsRequest,
  CosDigestReadRunRequest,
  HostErrorCode,
  RequestContext,
  SelftestEchoReply,
  SelftestEchoRequest,
} from "celeris:host";

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

// cos_digest の op（付記 S4-12）。試験が set… で答えを差し替える。差し替えなければ denied。
type ListRuns = (ctx: RequestContext, request: CosDigestListRunsRequest) => Promise<CosDigestListRunsReply>;
type ReadRun = (ctx: RequestContext, request: CosDigestReadRunRequest) => Promise<CosDigestInput>;
let listRuns: ListRuns = async () => {
  throw new HostError("denied", "cosDigestListRuns is not set in this test");
};
let readRun: ReadRun = async () => {
  throw new HostError("denied", "cosDigestReadRun is not set in this test");
};

export function setCosDigestListRuns(next: ListRuns): void {
  listRuns = next;
}

export function setCosDigestReadRun(next: ReadRun): void {
  readRun = next;
}

export function cosDigestListRuns(ctx: RequestContext, request: CosDigestListRunsRequest): Promise<CosDigestListRunsReply> {
  return listRuns(ctx, request);
}

export function cosDigestReadRun(ctx: RequestContext, request: CosDigestReadRunRequest): Promise<CosDigestInput> {
  return readRun(ctx, request);
}
