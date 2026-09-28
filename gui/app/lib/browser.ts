import type { RunSummary, Status } from "~/celeris/types";

/** Dashboard URLs are operator-configured HTTPS endpoints, never browser page URLs. */
export function safeBrowserLiveUrl(value: string | null | undefined): string | null {
  if (!value || value.trim() !== value) return null;
  try {
    const url = new URL(value);
    if (url.protocol !== "https:" || url.username || url.password || url.search || url.hash) return null;
    // biome-ignore lint/suspicious/noControlCharactersInRegex: reject URL parser control-character normalization
    if (/[\u0000-\u0020\u007f\\]/.test(value)) return null;
    return url.href;
  } catch {
    return null;
  }
}

export function activeBrowserRunIds(runs: RunSummary[], status: Status): string[] {
  if (status !== "running") return [];
  return runs.filter((run) => !run.finished_at && !run.end && !run.outcome).map((run) => run.run_id);
}
