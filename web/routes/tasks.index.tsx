import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";
import { optionalBoolean, optionalString } from "../lib/search";

// R21 /tasks（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/tasks/")({
  validateSearch: (
    search: Record<string, unknown>,
  ): { q?: string; order?: string; genre?: string; show_support?: boolean } => ({
    q: optionalString(search.q),
    order: optionalString(search.order),
    genre: optionalString(search.genre),
    show_support: optionalBoolean(search.show_support),
  }),
  component: Screen,
});

function Screen() {
  return <ScreenFrame title="タスク" route="/tasks" />;
}
