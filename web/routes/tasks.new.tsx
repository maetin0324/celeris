import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";

// R22 /tasks/new（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/tasks/new")({
  component: Screen,
});

function Screen() {
  return <ScreenFrame title="タスクの作成" route="/tasks/new" />;
}
