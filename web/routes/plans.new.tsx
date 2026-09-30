import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";

// R28 /plans/new（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/plans/new")({
  component: Screen,
});

function Screen() {
  return <ScreenFrame title="計画の作成" route="/plans/new" />;
}
