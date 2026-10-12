import type { RunSummary } from "~/celeris/types";

/**
 * run の選択（`RunSummary.selection`）を画面に出す。harness・source・model・account は API が記録した値だけを
 * 使い、旧 run（`legacy_unresolved`）は推測で埋めない（ADR 2026-10-11-llm-source-harness-selection）。
 */

/** 選択の記録が無い旧 run の表示。 */
export const LEGACY_SELECTION_TEXT = "記録なし（旧 run）";

export type RunSelectionParts = { harness: string; source: string; model: string; account: string };

type RunSelection = RunSummary["selection"];

/** 選択を 4 つの値に分ける。旧 run・記録が無いときは undefined。 */
export function runSelectionParts(selection: RunSelection | null | undefined): RunSelectionParts | undefined {
  if (selection?.status !== "selected") return undefined;
  return {
    harness: selection.harness_id,
    source: selection.source_id,
    model: selection.model_id,
    account: selection.account_id,
  };
}

/** 選択を 1 行の文字にする（`harness / source / model / 口座 account`）。旧 run は LEGACY_SELECTION_TEXT。 */
export function runSelectionText(selection: RunSelection | null | undefined): string {
  const parts = runSelectionParts(selection);
  if (!parts) return LEGACY_SELECTION_TEXT;
  return `${parts.harness} / ${parts.source} / ${parts.model} / 口座 ${parts.account}`;
}
