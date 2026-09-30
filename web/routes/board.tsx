import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";
import { optionalBoolean } from "../lib/search";

// R13 /board（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/board")({
  validateSearch: (search: Record<string, unknown>): { show_support?: boolean } => ({
    show_support: optionalBoolean(search.show_support),
  }),
  component: Screen,
});

function Screen() {
  return <ScreenFrame title="ボード" route="/board" />;
}
