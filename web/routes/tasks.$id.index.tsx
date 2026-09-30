import { createFileRoute } from "@tanstack/react-router";
import { parseTaskDetailTab } from "../features/tasks/task-detail-tabs";
import { TaskDetailScreen } from "../features/tasks/task-detail-view";
import { optionalString } from "../lib/search";

// R23 /tasks/:id（P3-08）。画面は features/tasks に置き、ここは配置だけ。loader は置かず fetch を待たない。
export const Route = createFileRoute("/tasks/$id/")({
  validateSearch: (search: Record<string, unknown>): { tab?: string } => ({
    tab: optionalString(search.tab),
  }),
  component: Screen,
});

function Screen() {
  const params = Route.useParams();
  const search = Route.useSearch();
  return <TaskDetailScreen taskId={params.id} tab={parseTaskDetailTab(search.tab)} />;
}
