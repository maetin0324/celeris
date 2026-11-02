// Generated from docs/api/script-host/host-v1.schema.json by control-ts/scripts/gen-host-types.mjs. Do not edit.
declare module "celeris:host" {
export type Capability = "selftest:echo_task" | "cos_digest:list_runs" | "cos_digest:read_run";

export type CosDigestInput = {
  "items": Array<CosTriageItemReport>;
  "reason": string | null;
  "run_id": string;
  "run_state": string;
  "thread": string;
};

export type CosDigestInputResult = {
  "kind": "ok";
  "value": CosDigestInput;
} | {
  "error": HostError;
  "kind": "error";
};

export type CosDigestListRunsReply = {
  "runs": Array<CosDigestRunRef>;
};

export type CosDigestListRunsReplyResult = {
  "kind": "ok";
  "value": CosDigestListRunsReply;
} | {
  "error": HostError;
  "kind": "error";
};

export type CosDigestListRunsRequest = {

};

export type CosDigestReadRunRequest = {
  "run_id": string;
  "thread": string;
};

export type CosDigestRunRef = {
  "run_id": string;
  "run_state": string;
  "thread": string;
};

export type CosTriageDecision = {
  "background": string | null;
  "on_answer": string | null;
  "options": Array<CosTriageOption>;
  "recommendation_reason": string | null;
  "recommended": string | null;
  "summary": string;
  "web_path": string;
};

export type CosTriageItemReport = {
  "created_at": string;
  "decision": CosTriageDecision | null;
  "detail": string;
  "item_id": string;
  "operation_id": string | null;
  "reason": string | null;
  "route": string | null;
  "run_id": string | null;
  "source_key": string;
  "source_kind": string;
  "source_revision": string;
  "state": string;
  "summary": string;
  "thread_id": string;
};

export type CosTriageOption = {
  "detail": string | null;
  "key": string;
  "label": string;
};

export type HostErrorCode = "exception" | "timeout" | "busy" | "invalid_output" | "denied" | "terminated" | "generation_gone";

export type SelftestEchoReply = {
  "at": Timestamp;
  "note": string | null;
  "observation": SelftestObservation;
  "priority": SelftestPriority;
  "task_id": string;
  "tokens": U64String;
};

export type SelftestEchoReplyResult = {
  "kind": "ok";
  "value": SelftestEchoReply;
} | {
  "error": HostError;
  "kind": "error";
};

export type SelftestEchoRequest = {
  "at": Timestamp;
  "note": string | null;
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

export function cosDigestListRuns(ctx: RequestContext, request: CosDigestListRunsRequest): Promise<CosDigestListRunsReply>;

export function cosDigestReadRun(ctx: RequestContext, request: CosDigestReadRunRequest): Promise<CosDigestInput>;
}
