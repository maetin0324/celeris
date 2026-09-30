import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";

// R08 /org/:id（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/org/$id")({
  component: Screen,
});

function Screen() {
  const params = Route.useParams();
  return <ScreenFrame title={`組織の人 ${params.id}`} route="/org/:id" />;
}
