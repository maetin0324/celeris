import { createFileRoute, useRouterState } from "@tanstack/react-router";
import { GraphScreen } from "../features/tasks/graph-view";
import { optionalBoolean, optionalNumber, optionalString } from "../lib/search";

// R35 /graph（P2-02 は見出しと枠だけ。loader は置かず fetch を待たない）。
export const Route = createFileRoute("/graph")({
  validateSearch: (search: Record<string, unknown>): { root?: string; depth?: number; include_terminal?: boolean } => ({
    root: optionalString(search.root),
    depth: optionalNumber(search.depth),
    include_terminal: optionalBoolean(search.include_terminal),
  }),
  component: Screen,
});

function Screen() {
  const search = useRouterState({ select: (state) => state.location.searchStr });
  const params = new URLSearchParams(search);
  const rawDepth = params.get("depth");
  const depth = rawDepth === null || rawDepth === "" ? undefined : Number(rawDepth);
  return (
    <GraphScreen
      root={params.get("root") ?? undefined}
      depth={depth !== undefined && Number.isInteger(depth) && depth >= 0 ? depth : undefined}
    />
  );
}
