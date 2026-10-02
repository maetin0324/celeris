import type { Tier } from "../../api/generated/types";

export const TIERS: readonly Tier[] = ["frontier", "standard", "cheap"];
export const ADAPTERS = ["fake", "claude-code", "codex", "acp", "paperqa", "local-deep-research"] as const;

export type ProviderInput = { id?: string; adapter?: string; tiers?: Tier[]; concurrency?: number; model?: string };

/** フォームの値から送る body を作る。空欄は送らない。 */
export function buildProviderBody(
  form: { id?: string; adapter?: string; concurrency: string; model: string; tiers: readonly Tier[] },
  mode: "create" | "patch",
): ProviderInput {
  const body: ProviderInput = { tiers: [...form.tiers] };
  if (mode === "create") {
    body.id = form.id?.trim() ?? "";
    body.adapter = form.adapter ?? "";
  }
  const c = form.concurrency.trim();
  if (c !== "" && Number.isInteger(Number(c)) && Number(c) >= 0) body.concurrency = Number(c);
  if (form.model.trim() !== "") body.model = form.model.trim();
  return body;
}
