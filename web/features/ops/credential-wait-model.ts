// credential 待ち（ADR 2026-10-15-mlsys-min-decision-assisted D4）の表示の規則。
// 状態は受信箱 item（GET /inbox/items の kind=credential_wait）と secret の保存有無から決める。値は一切扱わない。

import type { InboxItem, SecretView } from "../../api/generated/types";
import { credentialSecretId, isReregistration } from "../inbox/inbox-model";

/** 登録直後に検証の結果が出るまでの猶予。これを過ぎても item が「更新してください」なら再登録待ちに戻す。 */
export const VERIFY_GRACE_MS = 60_000;

export type CredentialPhase = "awaiting_key" | "verifying" | "reregister";

export const PHASE_LABEL: Record<CredentialPhase, string> = {
  awaiting_key: "登録待ち",
  verifying: "検証中（成功すると自動で続行）",
  reregister: "再登録待ち（キーが無効か権限不足）",
};

export type CredentialWaitRow = {
  secretId: string;
  item: InboxItem;
  phase: CredentialPhase;
  tasks: InboxItem["blocking"]["tasks"];
  units: string[];
};

export function credentialPhase(input: {
  item: InboxItem;
  secret: Pick<SecretView, "fingerprint"> | undefined;
  savedAgoMs: number | undefined;
}): CredentialPhase {
  if (input.savedAgoMs !== undefined && input.savedAgoMs < VERIFY_GRACE_MS) return "verifying";
  if (isReregistration(input.item)) return "reregister";
  return input.secret?.fingerprint ? "verifying" : "awaiting_key";
}

export function credentialWaitRows(
  items: readonly InboxItem[],
  secrets: readonly SecretView[],
  savedAgoMs: Readonly<Record<string, number>>,
): CredentialWaitRow[] {
  const rows: CredentialWaitRow[] = [];
  for (const item of items) {
    const secretId = credentialSecretId(item);
    if (!secretId) continue;
    rows.push({
      secretId,
      item,
      phase: credentialPhase({
        item,
        secret: secrets.find((s) => s.id === secretId),
        savedAgoMs: savedAgoMs[secretId],
      }),
      tasks: item.blocking.tasks,
      units: item.blocking.units,
    });
  }
  return rows;
}
