import { createFileRoute } from "@tanstack/react-router";
import { PlanCreateScreen } from "../features/tasks/create-screen";

export const Route = createFileRoute("/plans/new")({
  component: PlanCreateScreen,
});
