import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";
import { ConsoleView, normalizeScope } from "../features/console/console-view";
import { ConsoleRegion } from "../features/home/console-region";
import { HomeEntries } from "../features/home/home-entries";
import { optionalString } from "../lib/search";

export const Route = createFileRoute("/console")({
  validateSearch: (search: Record<string, unknown>): { scope?: string } => ({
    scope: optionalString(search.scope),
  }),
  component: ConsoleScreen,
});

function ConsoleScreen() {
  const scope = normalizeScope(Route.useSearch().scope);
  return (
    <ScreenFrame title="Console" route="/console">
      <HomeEntries />
      <ConsoleRegion>
        <ConsoleView scope={scope} label={scope === "all" ? "CoS" : scope} contained />
      </ConsoleRegion>
    </ScreenFrame>
  );
}
