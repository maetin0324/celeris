// search / filter の正規化（ADR-0081 D5）。同じ条件が同じ key になるようにする。
// 空の値（undefined・null・空文字・空配列）を落とし、文字列を trim し、配列は重複を除いて並べ、object の key を並べる。

export type FilterValue = string | number | boolean | null | undefined | readonly (string | number)[];
export type Filters = Readonly<Record<string, FilterValue>>;
export type NormalizedFilters = Readonly<Record<string, string | number | boolean | (string | number)[]>>;

export function normalizeFilters(filters: Filters | undefined): NormalizedFilters {
  const out: Record<string, string | number | boolean | (string | number)[]> = {};
  if (!filters) return out;
  for (const name of Object.keys(filters).sort()) {
    const value = filters[name];
    if (value === undefined || value === null) continue;
    if (typeof value === "string") {
      const trimmed = value.trim();
      if (trimmed !== "") out[name] = trimmed;
    } else if (Array.isArray(value)) {
      const items = [...new Set(value.map((v) => (typeof v === "string" ? v.trim() : v)).filter((v) => v !== ""))].sort(
        (a, b) => String(a).localeCompare(String(b)),
      );
      if (items.length > 0) out[name] = items;
    } else if (typeof value === "number") {
      if (Number.isFinite(value)) out[name] = value;
    } else {
      out[name] = value as boolean;
    }
  }
  return out;
}
