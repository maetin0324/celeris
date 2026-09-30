import { createFileRoute } from "@tanstack/react-router";
import { ProjectDetailScreen } from "../features/projects/project-detail-view";

// R10 /projects/:id（P4-02 は表示。操作は P4-03〜P4-05）。
export const Route = createFileRoute("/projects/$id/")({
  component: Screen,
});

function Screen() {
  const params = Route.useParams();
  return <ProjectDetailScreen projectId={params.id} />;
}
