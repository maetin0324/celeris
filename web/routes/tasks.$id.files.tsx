import { createFileRoute } from "@tanstack/react-router";
import type { FilesSearch } from "../features/files/task-files-query";
import { TaskFilesScreen } from "../features/files/task-files-view";
import { optionalString } from "../lib/search";

// R24 /tasks/:id/files（P3-11）。画面は features/files に置き、ここは配置だけ。loader は置かず fetch を待たない。
export const Route = createFileRoute("/tasks/$id/files")({
  validateSearch: (search: Record<string, unknown>): FilesSearch => ({
    repo: optionalString(search.repo),
    path: optionalString(search.path),
    file: optionalString(search.file),
  }),
  component: Screen,
});

function Screen() {
  const params = Route.useParams();
  const search = Route.useSearch();
  return <TaskFilesScreen taskId={params.id} search={search} />;
}
