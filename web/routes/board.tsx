import { createFileRoute, useLocation } from "@tanstack/react-router";
import { BoardScreen } from "../features/projects/board-screen";

export const Route = createFileRoute("/board")({
  validateSearch: (search: Record<string, unknown>) =>
    Object.fromEntries(
      Object.entries(search).filter(
        ([key, value]) =>
          ["project", "q", "label", "category", "tier", "priority", "assignee", "milestone", "show_support"].includes(
            key,
          ) &&
          (typeof value === "string" || typeof value === "number"),
      ),
    ),
  component: Screen,
});
function Screen() {
  useLocation();
  return <BoardScreen searchStr={window.location.search} />;
}
