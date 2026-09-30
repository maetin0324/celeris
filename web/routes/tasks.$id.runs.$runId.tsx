import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";

// R26 /tasks/:id/runs/:runId（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/tasks/$id/runs/$runId")({
  component: Screen,
});

function Screen() {
  const params = Route.useParams();
  return <ScreenFrame title={`run ログ ${params.id} / ${params.runId}`} route="/tasks/:id/runs/:runId" />;
}
