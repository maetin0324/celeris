import { createFileRoute } from "@tanstack/react-router";
import { ChatHome } from "../features/chat/home/chat-home";
import { optionalString } from "../lib/search";

export const Route = createFileRoute("/")({
  // ADR 2026-11-23-project-observability-publish D4: `message` は読み込み済みの発言へ移って強調する。
  validateSearch: (search: Record<string, unknown>): { thread?: string; message?: string } => ({
    thread: optionalString(search.thread),
    message: optionalString(search.message),
  }),
  component: ChatHome,
});
