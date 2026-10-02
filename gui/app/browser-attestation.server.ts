import { createPrivateKey, type KeyObject, randomBytes, sign } from "node:crypto";
import { readFileSync, statSync } from "node:fs";
import path from "node:path";
import type { HumanAttestation } from "~/celeris/types";

/**
 * ADR-0080 D5: GUI 専用鍵（Ed25519）で署名する human attestation。
 * - 鍵は owner が初期設定する 0700 directory の 0600 ファイル（`CELERIS_GUI_ATTESTATION_KEY_FILE`、PKCS#8 PEM）
 * - actor/owner-session hash、task/wait/version、decision、policy hash、nonce、30 秒以内の expiry を束縛する
 * - 鍵が無い・権限が緩いときは署名しない（人の操作は `attestation_unavailable`）
 */

export const ATTESTATION_KEY_ENV = "CELERIS_GUI_ATTESTATION_KEY_FILE";
/** daemon の上限（30 秒）より短くして時計のずれを吸収する。 */
export const ATTESTATION_TTL_SECS = 20;
/** wait の `owner_id` と照合される actor。単一所有者の instance なので固定値（`CELERIS_GUI_OWNER_ID` で上書き）。 */
export function ownerActorId(env: NodeJS.ProcessEnv = process.env): string {
  return env.CELERIS_GUI_OWNER_ID?.trim() || "owner";
}

export type AttestationDecision = "approve_once" | "deny" | "revoke" | "register";

export interface AttestationInput {
  ownerSessionHash: string;
  taskId: string;
  waitId: string;
  version: number;
  decision: AttestationDecision;
  policyHash: string;
}

let cachedKey: KeyObject | null | undefined;

export function setAttestationKeyForTest(key: KeyObject | null | undefined): void {
  cachedKey = key;
}

/** 鍵ファイルを読む。権限が 0600 / directory が 0700 でなければ使わない。 */
export function loadAttestationKey(file = process.env[ATTESTATION_KEY_ENV]): KeyObject | null {
  if (!file) return null;
  try {
    const st = statSync(file);
    const dir = statSync(path.dirname(file));
    if ((st.mode & 0o077) !== 0 || (dir.mode & 0o077) !== 0) return null;
    const key = createPrivateKey(readFileSync(file));
    return key.asymmetricKeyType === "ed25519" ? key : null;
  } catch {
    return null;
  }
}

function attestationKey(): KeyObject | null {
  if (cachedKey === undefined) cachedKey = loadAttestationKey();
  return cachedKey;
}

export function attestationAvailable(): boolean {
  return attestationKey() !== null;
}

/** 署名した attestation。鍵が無ければ null。 */
export function signAttestation(input: AttestationInput, nowMs = Date.now()): HumanAttestation | null {
  const key = attestationKey();
  if (!key) return null;
  const payload = JSON.stringify({
    actor_id: ownerActorId(),
    owner_session_hash: input.ownerSessionHash,
    task_id: input.taskId,
    wait_id: input.waitId,
    version: input.version,
    decision: input.decision,
    policy_hash: input.policyHash,
    nonce: randomBytes(16).toString("hex"),
    expires_at: Math.floor(nowMs / 1000) + ATTESTATION_TTL_SECS,
  });
  const signature = sign(null, Buffer.from(payload, "utf8"), key).toString("hex");
  return { payload, signature };
}

/** Short-lived assertion for the task-scoped live relay. */
export function signLiveAssertion(
  input: {
    taskId: string;
    runId: string;
    browserSessionId: string;
    ownerSessionId: string;
    originOk: boolean;
  },
  nowMs = Date.now(),
): HumanAttestation | null {
  const key = attestationKey();
  if (!key) return null;
  const payload = JSON.stringify({
    task_id: input.taskId,
    run_id: input.runId,
    browser_session_id: input.browserSessionId,
    owner_session_id: input.ownerSessionId,
    owner_session: true,
    origin_ok: input.originOk,
    expires_at: Math.floor(nowMs / 1000) + ATTESTATION_TTL_SECS,
  });
  return { payload, signature: sign(null, Buffer.from(payload), key).toString("hex") };
}
