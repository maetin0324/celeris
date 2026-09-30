import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";

// R19 /approvals（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/approvals")({
  component: Screen,
});

function Screen() {
  return <ScreenFrame title="承認" route="/approvals" />;
}
