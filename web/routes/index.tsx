import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";
import { ConsoleView, normalizeScope } from "../features/console/console-view";
import { optionalString } from "../lib/search";

// R01 /（Console。中身は features/console）。
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
      <ConsoleView scope={scope} label={scope === "all" ? "CoS" : scope} />
    </ScreenFrame>
  );
}
