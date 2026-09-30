import { createFileRoute } from "@tanstack/react-router";
import { ProvidersScreen } from "../features/ops/providers-screen";

// R30 /providers。
export const Route = createFileRoute("/providers")({ component: ProvidersScreen });
