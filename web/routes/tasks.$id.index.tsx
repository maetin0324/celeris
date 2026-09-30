import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";
import { optionalString } from "../lib/search";

// R23 /tasks/:id（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/tasks/$id/")({
  validateSearch: (search: Record<string, unknown>): { tab?: string } => ({
    tab: optionalString(search.tab),
  }),
  component: Screen,
});

function Screen() {
  const params = Route.useParams();
  return <ScreenFrame title={`タスクの詳細 ${params.id}`} route="/tasks/:id" />;
}
