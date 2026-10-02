import { createFileRoute } from "@tanstack/react-router";
import { ArtifactsScreen } from "../features/artifacts/artifacts-view";
import { optionalString } from "../lib/search";

// R20 /artifacts（P3-11）。画面は features/artifacts に置き、ここは配置だけ。loader は置かず fetch を待たない。
export const Route = createFileRoute("/artifacts")({
  validateSearch: (search: Record<string, unknown>): { project?: string } => ({
    project: optionalString(search.project),
  }),
  component: Screen,
});

function Screen() {
  const search = Route.useSearch();
  return <ArtifactsScreen project={search.project} />;
}
