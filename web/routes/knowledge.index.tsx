import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";

// R14 /knowledge（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/knowledge/")({
  component: Screen,
});

function Screen() {
  return <ScreenFrame title="知識" route="/knowledge" />;
}
