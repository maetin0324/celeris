import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";
import { ConsoleView, normalizeScope } from "../features/console/console-view";
import { ConsoleRegion } from "../features/home/console-region";
import { HomeEntries } from "../features/home/home-entries";
import { optionalString } from "../lib/search";

// R01 /（Console。中身は features/console）。会話は features/home の枠の中で scroll し、ページは viewport に収める。
export const Route = createFileRoute("/")({
  validateSearch: (search: Record<string, unknown>): { scope?: string } => ({
    scope: optionalString(search.scope),
  }),
  component: Screen,
});

function Screen() {
  const scope = normalizeScope(Route.useSearch().scope);
  return (
    <ScreenFrame title="ホーム" route="/">
      <HomeEntries />
      <ConsoleRegion>
        <ConsoleView scope={scope} label={scope === "all" ? "CoS" : scope} contained />
      </ConsoleRegion>
    </ScreenFrame>
  );
}
