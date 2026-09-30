import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";

// R02 /inbox（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/inbox")({
  component: Screen,
});

function Screen() {
  return <ScreenFrame title="受信箱" route="/inbox" />;
}
