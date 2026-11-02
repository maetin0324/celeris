// cos_digest の pass trace を replay する fake host（境界 ADR 付記 S4-16）。試験と scripts/compare.mjs だけが使う。bundle には入らない。
// trace の 1 行 = 1 pass:
//   { name, ops: { op, request, reply: { kind: "ok", value } | { kind: "error", error: { code, message } } }[], output }
// fake host は TS の op 呼び出しに trace の ops から答える。照合は op の名前と request の JSON 直列化の byte 一致で行う。
// 次のどれかが起きた pass は不一致（problems が空でない）:
// - 要求が記録と違う・記録に無い op を呼んだ（fake host は denied を返す）
// - 記録の op を 1 回も呼ばなかった
// Node の型除去でも読めるよう、erasable な構文だけを使い、相対 import をしない（HostError は呼び出し側が渡す）。
import type {
  CosDigestInput,
  CosDigestListRunsReply,
  CosDigestListRunsRequest,
  CosDigestReadRunRequest,
  HostErrorCode,
  RequestContext,
} from "celeris:host";

export type TraceOpName = "cos_digest:list_runs" | "cos_digest:read_run";

export type TraceReply = { kind: "ok"; value: unknown } | { kind: "error"; error: { code: HostErrorCode; message: string } };

export interface TraceOp {
  op: TraceOpName;
  request: unknown;
  reply: TraceReply;
}

export interface PassTrace {
  name: string;
  ops: TraceOp[];
  output: unknown;
}

export type HostErrorCtor = new (code: HostErrorCode, message: string) => Error;

export interface FakeHost {
  cosDigestListRuns: (ctx: RequestContext, request: CosDigestListRunsRequest) => Promise<CosDigestListRunsReply>;
  cosDigestReadRun: (ctx: RequestContext, request: CosDigestReadRunRequest) => Promise<CosDigestInput>;
  /** TS が呼んだ op（名前と request の直列化）を呼んだ順に。 */
  calls: { op: TraceOpName; request: string }[];
  /** 不一致の理由。pass が終わった後に finish() を呼ぶと、呼ばれなかった記録の op も足される。 */
  problems: string[];
  finish: () => string[];
}

/** trace の ops から答える fake host を作る。`token` が渡されたら、別の token の呼び出しを不一致にする。 */
export function fakeHost(ops: readonly TraceOp[], HostError: HostErrorCtor, token?: string): FakeHost {
  const used = new Set<number>();
  const calls: { op: TraceOpName; request: string }[] = [];
  const problems: string[] = [];
  const answer = async (op: TraceOpName, ctx: RequestContext, request: unknown): Promise<unknown> => {
    const bytes = JSON.stringify(request);
    calls.push({ op, request: bytes });
    if (token !== undefined && ctx?.token !== token) {
      problems.push(`${op} called with another request token`);
      throw new HostError("denied", `${op}: token of another request`);
    }
    const at = ops.findIndex((o) => o.op === op && JSON.stringify(o.request) === bytes);
    if (at < 0) {
      problems.push(`unrecorded op: ${op} ${bytes}`);
      throw new HostError("denied", `${op}: request not in the trace: ${bytes}`);
    }
    used.add(at);
    const reply = (ops[at] as TraceOp).reply;
    if (reply.kind === "error") throw new HostError(reply.error.code, reply.error.message);
    // 応答は毎回写しを返す（TS が書き換えても次の答えに漏れない）。
    return JSON.parse(JSON.stringify(reply.value));
  };
  return {
    cosDigestListRuns: (ctx, request) => answer("cos_digest:list_runs", ctx, request) as Promise<CosDigestListRunsReply>,
    cosDigestReadRun: (ctx, request) => answer("cos_digest:read_run", ctx, request) as Promise<CosDigestInput>,
    calls,
    problems,
    finish: () => {
      ops.forEach((o, i) => {
        if (!used.has(i)) problems.push(`recorded op not called: ${o.op} ${JSON.stringify(o.request)}`);
      });
      return problems;
    },
  };
}

/** run 1 つの case（`{ input, commands }`）を、その run だけを持つ pass の trace に組み直す（付記 S4-16）。 */
export function passFromCase(name: string, input: CosDigestInput, commands: unknown): PassTrace {
  return {
    name,
    ops: [
      {
        op: "cos_digest:list_runs",
        request: {},
        reply: { kind: "ok", value: { runs: [{ thread: input.thread, run_id: input.run_id, run_state: input.run_state }] } },
      },
      {
        op: "cos_digest:read_run",
        request: { thread: input.thread, run_id: input.run_id },
        reply: { kind: "ok", value: input },
      },
    ],
    output: { runs: [{ run_id: input.run_id, commands }] },
  };
}
