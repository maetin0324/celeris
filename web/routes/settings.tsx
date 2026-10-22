import { createFileRoute } from "@tanstack/react-router";
import { SettingsScreen } from "../features/ops/settings-screen";

// ADR 2026-10-22-dynamic-config: /settings（config.toml の欄と変更の履歴）。
export const Route = createFileRoute("/settings")({ component: SettingsScreen });
