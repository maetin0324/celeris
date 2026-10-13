// /models の model 行の型と供給元の表示名（models-screen が使う）。

// GET /api/llm/models の形（ADR 2026-10-06 opencode-go と model catalog の D1/D2/D5）。
export type ModelTier = "frontier" | "standard" | "cheap";
export type ModelOverride = {
  disabled: boolean;
  tier: ModelTier | null;
  alias: string | null;
  note: string | null;
};
export type LlmModel = {
  source: string;
  model_id: string;
  display_name: string | null;
  available: boolean;
  first_seen: string;
  last_seen: string;
  capabilities: Record<string, unknown>;
  override: ModelOverride | null;
  routing: { tiers: string[]; deployments: string[] };
};
/** 供給元 id の表示名。未知の id はそのまま出す。 */
export function modelSourceLabel(source: string): string {
  if (source === "claude-oauth") return "Claude（subscription）";
  if (source === "codex-oauth") return "Codex（subscription）";
  if (source === "opencode-go") return "OpenCode Go（subscription）";
  if (source.startsWith("openai-compatible:")) return `self-host ${source.slice("openai-compatible:".length)}`;
  return source;
}
