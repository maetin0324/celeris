import { createFileRoute } from "@tanstack/react-router";
import { BrowserRunsScreen } from "../features/browser/browser-runs-screen";

export const Route = createFileRoute("/browser/")({ component: BrowserRunsScreen });
