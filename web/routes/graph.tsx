import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";
import { optionalBoolean, optionalNumber, optionalString } from "../lib/search";

// R35 /graph（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/graph")({
  validateSearch: (search: Record<string, unknown>): { root?: string; depth?: number; include_terminal?: boolean } => ({
    root: optionalString(search.root),
    depth: optionalNumber(search.depth),
    include_terminal: optionalBoolean(search.include_terminal),
  }),
  component: Screen,
});

function Screen() {
  return <ScreenFrame title="依存グラフ" route="/graph" />;
}
