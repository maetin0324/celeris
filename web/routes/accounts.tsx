import { createFileRoute } from "@tanstack/react-router";
import { AccountsScreen } from "../features/ops/accounts-screen";
import { optionalString } from "../lib/search";

// R31 /accounts。loader は置かず fetch を待たない。
// ?secret=<id> は受信箱の credential 待ち item からの行き先で、secrets section の該当 secret にフォーカスする。
export const Route = createFileRoute("/accounts")({
  validateSearch: (search: Record<string, unknown>): { secret?: string } => ({
    secret: optionalString(search.secret),
  }),
  component: Screen,
});

function Screen() {
  const { secret } = Route.useSearch();
  return <AccountsScreen focusSecret={secret} />;
}
