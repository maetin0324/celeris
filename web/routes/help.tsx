import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";

// R36 /help（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/help")({
  component: Screen,
});

function Screen() {
  return <ScreenFrame title="ヘルプ" route="/help" />;
}
