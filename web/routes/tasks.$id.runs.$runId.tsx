import { createFileRoute } from "@tanstack/react-router";
import { RunLogScreen } from "../features/runs/run-log-view";

// R26 /tasks/:id/runs/:runId（P3-12）。画面は features/runs に置き、ここは配置だけ。loader は置かず fetch を待たない。
export const Route = createFileRoute("/tasks/$id/runs/$runId")({
  component: Screen,
});

function Screen() {
  const params = Route.useParams();
  return <RunLogScreen taskId={params.id} runId={params.runId} />;
}
