import { createFileRoute } from "@tanstack/react-router";
import { ManifestScreen } from "../features/publish/manifest-view";
import { optionalNumber } from "../lib/search";

// /tasks/:id/publish/:manifestId（ADR 2026-11-23-project-observability-publish D4）。画面は features/publish に置き、ここは配置だけ。
// `version` を省けば最新の版（daemon の GET と同じ）。
export const Route = createFileRoute("/tasks/$id/publish/$manifestId")({
  validateSearch: (search: Record<string, unknown>): { version?: number } => ({
    version: optionalNumber(search.version),
  }),
  component: Screen,
});

function Screen() {
  const params = Route.useParams();
  const search = Route.useSearch();
  return <ManifestScreen taskId={params.id} manifestId={params.manifestId} version={search.version} />;
}
