// Generated from docs/api/script-host/host-v1.schema.json by control-ts/scripts/gen-host-types.mjs. Do not edit.
declare module "celeris:host" {
export type Capability = "selftest:echo_task";

export type HostErrorCode = "exception" | "timeout" | "busy" | "invalid_output" | "denied" | "terminated" | "generation_gone";

export type HostResult = {
  "kind": "ok";
  "value": SelftestEchoReply;
} | {
  "error": HostError;
  "kind": "error";
};

export type SelftestEchoReply = {
  "at": Timestamp;
  "note": string;
  "observation": SelftestObservation;
  "priority": SelftestPriority;
  "task_id": string;
  "tokens": U64String;
};

export type SelftestEchoRequest = {
  "at": Timestamp;
  "note": string;
  "priority": SelftestPriority;
  "task_id": string;
  "tokens": U64String;
};

export type SelftestObservation = {
  "kind": "noted";
  "length": number;
} | {
  "kind": "empty";
};

export type SelftestPriority = "low" | "normal" | "high";

export type Timestamp = string;

export type U64String = string;

export type RequestContext = { token: string };

export const HOST_API_VERSION: string;

export class HostError extends Error {
  readonly code: HostErrorCode;
  constructor(code: HostErrorCode, message: string);
}

export function hostVersion(): string;

export function selftestEcho(ctx: RequestContext, request: SelftestEchoRequest): Promise<SelftestEchoReply>;
}
