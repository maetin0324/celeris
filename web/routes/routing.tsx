import { createFileRoute } from "@tanstack/react-router";
import { RoutingScreen } from "../features/ops/routing-screen";

export const Route = createFileRoute("/routing")({ component: RoutingScreen });
