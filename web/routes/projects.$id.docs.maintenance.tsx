import { createFileRoute } from "@tanstack/react-router";
import { ProjectDocsMaintenanceScreen } from "../features/projects/project-docs-maintenance-screen";

export const Route = createFileRoute("/projects/$id/docs/maintenance")({ component: Screen });

function Screen() {
  return <ProjectDocsMaintenanceScreen projectId={Route.useParams().id} />;
}
