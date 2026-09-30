import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";

// R30 /providers（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/providers")({
  component: Screen,
});

function Screen() {
  return <ScreenFrame title="プロバイダ" route="/providers" />;
}
