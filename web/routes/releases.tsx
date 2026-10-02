import { createFileRoute } from "@tanstack/react-router";
import { ReleasesScreen } from "../features/ops/releases-screen";

// R34 /releases（画面は features/ops/releases-screen.tsx）。
export const Route = createFileRoute("/releases")({ component: ReleasesScreen });
