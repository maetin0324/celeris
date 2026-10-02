import { createFileRoute } from "@tanstack/react-router";
import { ClustersScreen } from "../features/ops/clusters-screen";

// R33 /clusters。loader は置かず fetch を待たない。
export const Route = createFileRoute("/clusters")({ component: ClustersScreen });
