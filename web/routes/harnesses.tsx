import { createFileRoute } from "@tanstack/react-router";
import { HarnessesScreen } from "../features/ops/sources-screen";

export const Route = createFileRoute("/harnesses")({ component: HarnessesScreen });
