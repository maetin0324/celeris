// 発言の送信の再試行（ADR 2026-10-05-cos-chat-home D2「重複送信は client_message_id で同じ応答に戻す」・D5）。
// 結果が分からない失敗（network・timeout・5xx）だけを同じ要求（同じ client_message_id）で送り直す。
// 4xx（409 の本文不一致・422・413・429 など）は送り直さずに返す。

import { isApiError } from "../../../api/client";
import { randomId } from "../../../lib/random-id";

export const SEND_ATTEMPTS = 3;
export const SEND_RETRY_DELAY_MS = 1_000;

export function isRetryableSendError(error: unknown): boolean {
  if (!isApiError(error)) return false;
  return error.kind === "network" || error.kind === "timeout" || error.kind === "server";
}

/** client_message_id を 1 度だけ作る（再試行・再送ではこの値を使い回す）。 */
export function newClientId(prefix = "c"): string {
  return `${prefix}-${randomId()}`;
}

export async function sendWithRetry<T>(
  send: () => Promise<T>,
  options: { attempts?: number; delayMs?: number } = {},
): Promise<T> {
  const attempts = Math.max(1, options.attempts ?? SEND_ATTEMPTS);
  const delayMs = options.delayMs ?? SEND_RETRY_DELAY_MS;
  for (let attempt = 1; ; attempt += 1) {
    try {
      return await send();
    } catch (error) {
      if (attempt >= attempts || !isRetryableSendError(error)) throw error;
      await new Promise<void>((resolve) => setTimeout(resolve, delayMs * attempt));
    }
  }
}
