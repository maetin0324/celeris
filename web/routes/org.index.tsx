import { createFileRoute } from "@tanstack/react-router";
import { OrgScreen } from "../features/org/org-screen";
import { optionalString } from "../lib/search";

export const Route = createFileRoute("/org/")({
  validateSearch: (search: Record<string, unknown>): { project?: string; selected?: string } => ({
    project: optionalString(search.project),
    selected: optionalString(search.selected),
  }),
  component: OrgScreen,
});
