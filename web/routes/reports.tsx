import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";
import { optionalString } from "../lib/search";

// R17 /reports（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/reports")({
  validateSearch: (search: Record<string, unknown>): { project?: string; level?: string; filter?: string } => ({
    project: optionalString(search.project),
    level: optionalString(search.level),
    filter: optionalString(search.filter),
  }),
  component: Screen,
});

function Screen() {
  return <ScreenFrame title="報告" route="/reports" />;
}
