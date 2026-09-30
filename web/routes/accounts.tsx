import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";

// R31 /accounts（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/accounts")({
  component: Screen,
});

function Screen() {
  return <ScreenFrame title="アカウント" route="/accounts" />;
}
