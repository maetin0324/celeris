import { createFileRoute } from "@tanstack/react-router";
import { SourceDetailScreen } from "../features/ops/sources-screen";

export const Route = createFileRoute("/sources/$sourceId")({ component: SourceDetailScreen });
