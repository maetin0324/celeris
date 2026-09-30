import { createFileRoute } from "@tanstack/react-router";
import { ProjectDetailScreen } from "../features/projects/project-detail-view";
import { PlanOps, ProjectOps, RepoOps } from "../features/projects/project-ops";

// R10 /projects/:id（P4-02 は表示、P4-03〜P4-05 は操作）。
export const Route = createFileRoute("/projects/$id/")({
  component: Screen,
});

function Screen() {
  const params = Route.useParams();
  return (
    <ProjectDetailScreen
      projectId={params.id}
      ops={(detail) => (
        <>
          <ProjectOps detail={detail} />
          <PlanOps detail={detail} />
          <RepoOps detail={detail} />
        </>
      )}
    />
  );
}
