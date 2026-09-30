import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";

// R15 /knowledge/inbox（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/knowledge/inbox")({
  component: Screen,
});

function Screen() {
  return <ScreenFrame title="知識の候補" route="/knowledge/inbox" />;
}
