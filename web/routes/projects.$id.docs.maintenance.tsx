import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";

// R12 /projects/:id/docs/maintenance（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/projects/$id/docs/maintenance")({
  component: Screen,
});

function Screen() {
  const params = Route.useParams();
  return <ScreenFrame title={`文書の保守 ${params.id}`} route="/projects/:id/docs/maintenance" />;
}
