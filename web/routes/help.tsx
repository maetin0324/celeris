import { createFileRoute } from "@tanstack/react-router";
import { HelpScreen } from "../features/help/help-screen";

export const Route = createFileRoute("/help")({ component: HelpScreen });
