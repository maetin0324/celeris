import { createFileRoute } from "@tanstack/react-router";
import { TaskCreateScreen } from "../features/tasks/create-screen";

export const Route = createFileRoute("/tasks/new")({
  component: TaskCreateScreen,
});
