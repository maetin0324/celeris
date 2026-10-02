import { type QueryKey, useQueryClient } from "@tanstack/react-query";
import { useRef, useState } from "react";
import { ApiError, type ApiMethod, apiMutate } from "../../api/client";

export type ActionResult = {
  id: string;
  ok: boolean;
  message: string;
  status?: number;
  uncertain?: boolean;
  response?: unknown;
};
export type ActionTarget = { id: string; path: string; body?: unknown; method?: Exclude<ApiMethod, "GET"> };

function errorMessage(error: unknown): Pick<ActionResult, "message" | "status" | "uncertain"> {
  if (!(error instanceof ApiError)) return { message: "操作に失敗しました" };
  if (error.kind === "timeout" || error.kind === "network" || error.kind === "aborted")
    return { message: "結果を確認できません。再取得して状態を確認してください。", uncertain: true };
  if (error.status === 409) return { message: "状態が変わりました。最新の状態を確認してください。", status: 409 };
  const body = error.body;
  const detail =
    typeof body === "string"
      ? body
      : body && typeof body === "object"
        ? ((body as Record<string, unknown>).detail ??
          (body as Record<string, unknown>).message ??
          (body as Record<string, unknown>).error)
        : null;
  return { message: typeof detail === "string" ? detail : "操作に失敗しました", status: error.status };
}

/** 後続画面は useActionResult(key).run(targets) を呼ぶ。targets は直列、結果は項目別に保持。
 * pending 中は同じ hook から再送不可。409 は key を invalidate、422 の文言は results[id] に残る。
 * 自動再送はしない。timeout は結果不明として扱う。 */
export function useActionResult(key: QueryKey) {
  const queryClient = useQueryClient();
  const pendingRef = useRef(false);
  const [pending, setPending] = useState(false);
  const [results, setResults] = useState<Record<string, ActionResult>>({});
  async function run(targets: readonly ActionTarget[]): Promise<ActionResult[]> {
    if (pendingRef.current || targets.length === 0) return [];
    pendingRef.current = true;
    setPending(true);
    const outcomes: ActionResult[] = [];
    try {
      for (const target of targets) {
        let result: ActionResult;
        try {
          const response = await apiMutate(target.method ?? "POST", target.path, target.body);
          result = { id: target.id, ok: true, message: "操作が完了しました", response };
        } catch (error) {
          result = { id: target.id, ok: false, ...errorMessage(error) };
        }
        outcomes.push(result);
        setResults((previous) => ({ ...previous, [target.id]: result }));
        if (result.status === 409 || result.ok) await queryClient.invalidateQueries({ queryKey: key });
      }
    } finally {
      pendingRef.current = false;
      setPending(false);
    }
    return outcomes;
  }
  return { run, pending, results };
}

export function ActionResultView({ result, fieldId }: { result?: ActionResult; fieldId?: string }) {
  if (!result) return null;
  return (
    <p id={fieldId} role={result.ok ? "status" : "alert"} className={result.ok ? "text-green-800" : "text-red-800"}>
      {result.message}
    </p>
  );
}
