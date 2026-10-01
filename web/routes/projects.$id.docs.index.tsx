import { createFileRoute } from "@tanstack/react-router";
import { ProjectDocsScreen } from "../features/projects/project-docs-screen";

export const Route = createFileRoute("/projects/$id/docs/")({
  validateSearch: (search: Record<string, unknown>): { path?: string; q?: string; edit?: "1" } => ({
    path: typeof search.path === "string" ? search.path : undefined,
    q: typeof search.q === "string" ? search.q : undefined,
    edit: search.edit === "1" || search.edit === 1 ? ("1" as const) : undefined,
  }),
  component: Screen,
});

function Screen() {
  const { id } = Route.useParams();
  const search = Route.useSearch();
  return <ProjectDocsScreen projectId={id} {...search} />;
}
