import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";

// R24 /tasks/:id/files（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/tasks/$id/files")({
  component: Screen,
});

function Screen() {
  const params = Route.useParams();
  return <ScreenFrame title={`作業ツリーと成果物 ${params.id}`} route="/tasks/:id/files" />;
}
