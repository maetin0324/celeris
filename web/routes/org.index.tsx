import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";
import { optionalString } from "../lib/search";

// R07 /org（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/org/")({
  validateSearch: (search: Record<string, unknown>): { project?: string; selected?: string } => ({
    project: optionalString(search.project),
    selected: optionalString(search.selected),
  }),
  component: Screen,
});

function Screen() {
  return <ScreenFrame title="組織" route="/org" />;
}
