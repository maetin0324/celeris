import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";

// R09 /projects（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/projects/")({
  component: Screen,
});

function Screen() {
  return <ScreenFrame title="案件" route="/projects" />;
}
