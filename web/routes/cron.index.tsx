import { createFileRoute } from "@tanstack/react-router";
import { CronScreen } from "../features/cron/cron-screen";
export const Route = createFileRoute("/cron/")({ component: CronScreen });
