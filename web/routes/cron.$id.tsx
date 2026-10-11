import { createFileRoute } from "@tanstack/react-router";
import { CronDetailScreen } from "../features/cron/cron-screen";
export const Route = createFileRoute("/cron/$id")({ component: Screen });
function Screen() {
  return <CronDetailScreen key={Route.useParams().id} id={Route.useParams().id} />;
}
