import { describe, expect, it } from "vitest";
import type { RunSummary } from "~/celeris/types";
import { LEGACY_SELECTION_TEXT, runSelectionParts, runSelectionText } from "~/lib/run-selection";

type Selection = RunSummary["selection"];

const selected: Selection = {
  status: "selected",
  harness_id: "claude-code",
  source_id: "claude-oauth",
  model_id: "claude-opus-5-5",
  account_id: "acc-1",
  basis: {},
};

describe("run selection (ADR 2026-10-11-llm-source-harness-selection)", () => {
  it("splits a recorded selection into harness, source, model and account", () => {
    expect(runSelectionParts(selected)).toEqual({
      harness: "claude-code",
      source: "claude-oauth",
      model: "claude-opus-5-5",
      account: "acc-1",
    });
    expect(runSelectionText(selected)).toBe("claude-code / claude-oauth / claude-opus-5-5 / 口座 acc-1");
  });

  it("shows a legacy run as unrecorded instead of guessing", () => {
    const legacy: Selection = { status: "legacy_unresolved" };
    expect(runSelectionParts(legacy)).toBeUndefined();
    expect(runSelectionText(legacy)).toBe(LEGACY_SELECTION_TEXT);
    expect(runSelectionText(undefined)).toBe(LEGACY_SELECTION_TEXT);
  });
});
