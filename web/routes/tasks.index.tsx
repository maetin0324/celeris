import { createFileRoute, useRouterState } from "@tanstack/react-router";
import { TasksListScreen } from "../features/tasks/task-list-view";
import { optionalBoolean, optionalString } from "../lib/search";

// R21 /tasks（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/tasks/")({
  validateSearch: (
    search: Record<string, unknown>,
  ): { q?: string; order?: string; limit?: string; genre?: string; show_support?: boolean } => ({
    q: optionalString(search.q),
    order: optionalString(search.order),
    limit: optionalString(search.limit),
    genre: optionalString(search.genre),
    show_support: optionalBoolean(search.show_support),
  }),
  component: Screen,
});

function Screen() {
  const search = useRouterState({ select: (state) => state.location.searchStr });
  const params = new URLSearchParams(search);
  return (
    <TasksListScreen
      q={params.get("q") ?? undefined}
      status={params.getAll("status")}
      order={params.get("order") ?? undefined}
      limit={params.get("limit") ?? undefined}
    />
  );
}
