import { createFileRoute, useRouterState } from "@tanstack/react-router";
import { TasksListScreen } from "../features/tasks/task-list-view";
import { optionalBoolean, optionalString } from "../lib/search";

function statusSearch(value: unknown): string | undefined {
  const statuses = (Array.isArray(value) ? value : [value])
    .filter((item): item is string => typeof item === "string" && item.length > 0)
    .flatMap((item) => item.split(",").filter(Boolean));
  return statuses.length ? statuses.join(",") : undefined;
}

function limitSearch(value: unknown): number | undefined {
  const number = Number(value);
  return value !== undefined && Number.isInteger(number) && number > 0 ? number : undefined;
}

// R21 /tasks（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/tasks/")({
  validateSearch: (
    search: Record<string, unknown>,
  ): { q?: string; status?: string; order?: string; limit?: number; genre?: string; show_support?: boolean } => ({
    q: optionalString(search.q),
    status: statusSearch(search.status),
    order: optionalString(search.order),
    limit: limitSearch(search.limit),
    genre: optionalString(search.genre),
    show_support: optionalBoolean(search.show_support),
  }),
  component: Screen,
});

function Screen() {
  const search = useRouterState({ select: (state) => state.location.searchStr });
  const validated = Route.useSearch();
  const params = new URLSearchParams(search);
  return (
    <TasksListScreen
      q={params.get("q") ?? undefined}
      status={validated.status?.split(",") ?? []}
      order={params.get("order") ?? undefined}
      limit={params.get("limit") ?? undefined}
    />
  );
}
