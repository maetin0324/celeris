import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";

// R16 /knowledge/skills（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/knowledge/skills")({
  component: Screen,
});

function Screen() {
  return <ScreenFrame title="skills" route="/knowledge/skills" />;
}
