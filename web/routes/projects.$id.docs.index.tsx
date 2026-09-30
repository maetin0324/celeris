import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";

// R11 /projects/:id/docs（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/projects/$id/docs/")({
  component: Screen,
});

function Screen() {
  const params = Route.useParams();
  return <ScreenFrame title={`案件の文書 ${params.id}`} route="/projects/:id/docs" />;
}
