import { createFileRoute } from "@tanstack/react-router";
import { isInboxKind } from "../features/inbox/inbox-model";
import { InboxScreen, type InboxSearch } from "../features/inbox/inbox-screen";
import { optionalString } from "../lib/search";

// /inbox（ADR-0133）。案件・種類の絞り込みは URL の search param（?project=&kind=）。loader は置かず fetch を待たない。
export const Route = createFileRoute("/inbox")({
  validateSearch: (search: Record<string, unknown>): InboxSearch => ({
    project: optionalString(search.project),
    kind: isInboxKind(search.kind) ? search.kind : undefined,
  }),
  component: Screen,
});

function Screen() {
  const search = Route.useSearch();
  return <InboxScreen search={search} />;
}
