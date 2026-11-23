// chat の操作（カードの代答・取り消し、案の採否）が受けた problem の本文（`ApiError.body`）の読み方。

/** problem の本文の code（古い形の `error` も読む）。無ければ undefined。 */
export function problemCode(body: unknown): string | undefined {
  if (!body || typeof body !== "object") return undefined;
  const record = body as Record<string, unknown>;
  const code = record.code ?? record.error;
  return typeof code === "string" ? code : undefined;
}

/** problem の本文の detail。無ければ undefined。 */
export function problemDetail(body: unknown): string | undefined {
  if (!body || typeof body !== "object") return undefined;
  const detail = (body as Record<string, unknown>).detail;
  return typeof detail === "string" ? detail : undefined;
}
