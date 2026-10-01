import { signLiveAssertion } from "~/browser-attestation.server";
import { checkOwner, exactSameOrigin, verifyOwnerCsrfToken } from "~/browser-owner.server";
import { loadBrowserRuns } from "./browser";
import type { CelerisClient } from "./client.server";
import { CelerisError } from "./errors";

const ID = /^[0-9A-Za-z_-]{1,64}$/;
export interface ControlStatus {
  phase: "agent_running" | "pausing" | "paused" | "human_control" | "stopped";
  version: number;
  lease_expires_at: number | null;
  in_flight: number;
  auth_section: boolean;
}
export type ControlCommand =
  | { kind: "pause" | "takeover" | "renew" | "stop"; ttl_secs?: number }
  | { kind: "resume"; fresh_snapshot: boolean; policy_origin_ok: boolean };

function response(body: unknown, status = 200): Response {
  return Response.json(body, { status, headers: { "Cache-Control": "no-store", "Referrer-Policy": "no-referrer" } });
}
function error(code: string, status: number): Response {
  return response({ ok: false, code }, status);
}
function failure(e: unknown): Response {
  if (e instanceof CelerisError) {
    const codes = new Set([
      "version_conflict",
      "not_converged",
      "auth_section_active",
      "invalid_phase",
      "lease_expired",
      "not_lease_holder",
    ]);
    return error(codes.has(e.code) ? e.code : "rejected", e.status);
  }
  return error("celeris_unavailable", 503);
}
export async function runControlRequest(
  client: CelerisClient,
  request: Request,
  ids: { taskId?: string; runId?: string; sessionId?: string },
): Promise<Response> {
  const { taskId, runId, sessionId } = ids;
  if (!taskId || !runId || !sessionId || ![taskId, runId, sessionId].every((id) => ID.test(id)))
    return error("not_found", 404);
  const owner = await checkOwner(request);
  if (!owner.ok) return error(owner.code, owner.status);
  const path = `/tasks/${taskId}/browser/control/${runId}/${sessionId}`;
  try {
    const runs = await loadBrowserRuns(client, taskId, request.signal);
    if (!runs.some((run) => run.task_id === taskId && run.run_id === runId && run.session_id === sessionId))
      return error("not_found", 404);
    if (request.method === "GET") {
      const status = await client.get<ControlStatus>(path, { signal: request.signal });
      return response({ ok: true, status });
    }
    if (request.method !== "POST") return error("method_not_allowed", 405);
    if (!exactSameOrigin(request)) return error("csrf_failed", 403);
    if (Number(request.headers.get("content-length") ?? 0) > 4096) return error("invalid_input", 422);
    const body = (await request.json()) as {
      csrf?: unknown;
      command?: ControlCommand;
      expected_version?: number;
      idempotency_key?: string;
    };
    if (!verifyOwnerCsrfToken(owner.config, owner.sessionHash, body.csrf)) return error("csrf_failed", 403);
    const command = body.command;
    if (!command || !["pause", "takeover", "renew", "resume", "stop"].includes(command.kind))
      return error("invalid_input", 422);
    if (
      !Number.isSafeInteger(body.expected_version) ||
      (body.expected_version ?? -1) < 0 ||
      typeof body.idempotency_key !== "string" ||
      !ID.test(body.idempotency_key)
    )
      return error("invalid_input", 422);
    const status = await client.get<ControlStatus>(path, { signal: request.signal });
    if (status.auth_section) return error("auth_section_active", 409);
    const assertion = signLiveAssertion({
      taskId,
      runId,
      browserSessionId: sessionId,
      ownerSessionId: owner.sessionHash,
      originOk: true,
    });
    if (!assertion) return error("attestation_unavailable", 503);
    const result = await client.post<ControlStatus>(
      path,
      { assertion, command, expected_version: body.expected_version, idempotency_key: body.idempotency_key },
      { signal: request.signal },
    );
    return response({ ok: true, status: result });
  } catch (e) {
    return failure(e);
  }
}
