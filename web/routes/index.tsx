import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";
import { optionalString } from "../lib/search";

// R01 /（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/")({
  validateSearch: (search: Record<string, unknown>): { scope?: string } => ({
    scope: optionalString(search.scope),
  }),
  component: Screen,
});

function Screen() {
  return <ScreenFrame title="ホーム" route="/" />;
}
