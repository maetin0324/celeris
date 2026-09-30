import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";

// R34 /releases（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/releases")({
  component: Screen,
});

function Screen() {
  return <ScreenFrame title="リリース" route="/releases" />;
}
