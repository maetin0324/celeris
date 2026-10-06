import { createFileRoute } from "@tanstack/react-router";
import { BrowserSettingsScreen } from "../features/browser/browser-settings-screen";

export const Route = createFileRoute("/browser/settings")({ component: BrowserSettingsScreen });
