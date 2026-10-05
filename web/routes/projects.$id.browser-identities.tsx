import { createFileRoute } from "@tanstack/react-router";
import { BrowserIdentitiesScreen } from "../features/browser/identities-screen";

export const Route = createFileRoute("/projects/$id/browser-identities")({ component: Screen });

function Screen() {
  const { id } = Route.useParams();
  return <BrowserIdentitiesScreen projectId={id} />;
}
