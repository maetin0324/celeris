import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";
import { optionalString } from "../lib/search";

// R20 /artifacts（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/artifacts")({
  validateSearch: (search: Record<string, unknown>): { project?: string } => ({
    project: optionalString(search.project),
  }),
  component: Screen,
});

function Screen() {
  return <ScreenFrame title="成果物" route="/artifacts" />;
}
