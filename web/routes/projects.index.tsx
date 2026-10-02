import { createFileRoute } from "@tanstack/react-router";
import { ProjectsListScreen } from "../features/projects/project-list-screen";
import { optionalBoolean } from "../lib/search";

// R09 /projects（P4-01）。
export const Route = createFileRoute("/projects/")({
  validateSearch: (search: Record<string, unknown>): { archived?: boolean } => ({
    archived: optionalBoolean(search.archived),
  }),
  component: Screen,
});

function Screen() {
  const search = Route.useSearch();
  return <ProjectsListScreen archived={search.archived === true} />;
}
