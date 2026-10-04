import { createFileRoute } from "@tanstack/react-router";
import { ReportsScreen } from "../features/reports/reports-screen";
import { optionalNumber, optionalString } from "../lib/search";

export const Route = createFileRoute("/reports")({
  validateSearch: (
    search: Record<string, unknown>,
  ): { project?: string; level?: number; filter?: string; report?: string } => ({
    project: optionalString(search.project),
    level: optionalNumber(search.level),
    filter: optionalString(search.filter),
    report: optionalString(search.report),
  }),
  component: ReportsScreen,
});
