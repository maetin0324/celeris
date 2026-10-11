import type { ProviderCheckResult, ProviderView, Tier } from "../../api/generated/types";
import type { ActionResult } from "../../components/actions/use-action-result";
import type { BadgeTone } from "../../components/ui/badge";

export const TIERS: readonly Tier[] = ["frontier", "standard", "cheap"];
export const ADAPTERS = ["fake", "claude-code", "codex", "acp", "paperqa", "local-deep-research"] as const;

/** 送る body。同時実行数（旧 concurrency）は送らない: 上限はアカウント画面で管理する（ADR 2026-10-08-account-concurrency-limits）。 */
export type ProviderInput = { id?: string; adapter?: string; tiers?: Tier[]; model?: string };

/** フォームの値から送る body を作る。空欄は送らない。 */
export function buildProviderBody(
  form: { id?: string; adapter?: string; model: string; tiers: readonly Tier[] },
  mode: "create" | "patch",
): ProviderInput {
  const body: ProviderInput = { tiers: [...form.tiers] };
  if (mode === "create") {
    body.id = form.id?.trim() ?? "";
    body.adapter = form.adapter ?? "";
  }
  if (form.model.trim() !== "") body.model = form.model.trim();
  return body;
}

export type ProviderFieldErrors = { id?: string };

/** 送る前の検査。項目ごとの文言を返す（空なら送ってよい）。並びは画面の入力順。 */
export function validateProviderForm(form: { id?: string }, mode: "create" | "patch"): ProviderFieldErrors {
  const errors: ProviderFieldErrors = {};
  if (mode === "create" && (form.id ?? "").trim() === "") errors.id = "id を入力してください。";
  return errors;
}

/** 403・401 の結果が 1 つでもあれば、その区画の変更操作を止める。 */
export function isDenied(results: Record<string, ActionResult>): boolean {
  return Object.values(results).some((r) => r.status === 403 || r.status === 401);
}

/** 権限が無いときの理由文。操作の名前を添えて、何が止まっているかを書く。 */
export function deniedMessage(actions: string): string {
  return `この操作を行う権限がありません。${actions}は止めています。管理者に権限を確認してください。`;
}

const checkLabel: Readonly<Record<ProviderCheckResult, string>> = {
  ok: "正常",
  auth_failed: "認証失敗",
  throttled: "利用制限中",
  spawn_failed: "起動失敗",
};

export function checkResultLabel(result: string): string {
  return Object.hasOwn(checkLabel, result) ? checkLabel[result as ProviderCheckResult] : result;
}

export type StateView = { tone: BadgeTone; label: string; detail: string };

/** 実行枠の状態を 1 語にまとめる。重いもの（失敗）から順に見る。badge には必ずこの文字を出す。 */
export function providerState(
  item: Pick<ProviderView, "cooldown" | "last_check">,
  format: (at: string) => string = (at) => at,
): StateView {
  const check = item.last_check;
  if (check && (check.result === "auth_failed" || check.result === "spawn_failed"))
    return {
      tone: "danger",
      label: checkResultLabel(check.result),
      detail: check.detail ?? "認証情報か起動設定を見直して、接続を確認し直してください",
    };
  if (item.cooldown)
    return {
      tone: "warning",
      label: "休止中",
      detail: `${item.cooldown.reason}（${format(item.cooldown.until)} まで）`,
    };
  if (check?.result === "throttled")
    return { tone: "warning", label: "利用制限中", detail: check.detail ?? "時間をおいて確認し直してください" };
  if (check?.result === "ok") return { tone: "success", label: "接続確認済み", detail: "使える状態です" };
  if (check) return { tone: "neutral", label: "未確認", detail: `確認結果 ${check.result}` };
  return { tone: "neutral", label: "未確認", detail: "まだ接続を確認していません" };
}
