import { createFileRoute } from "@tanstack/react-router";
import { TaskChangesScreen } from "../features/changes/changes-view";

// R25 /tasks/:id/changes（P3-13）。画面は features/changes に置き、ここは配置だけ。loader は置かず fetch を待たない。
export const Route = createFileRoute("/tasks/$id/changes")({
  component: Screen,
});

function Screen() {
  const params = Route.useParams();
  return <TaskChangesScreen taskId={params.id} />;
}
