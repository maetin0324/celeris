import { createFileRoute } from "@tanstack/react-router";
import { SourcesScreen } from "../features/ops/sources-screen";

export const Route = createFileRoute("/sources/")({ component: SourcesScreen });
