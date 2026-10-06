import { createFileRoute } from "@tanstack/react-router";
import { ChatHome } from "../features/chat/home/chat-home";
import { optionalString } from "../lib/search";

export const Route = createFileRoute("/")({
  validateSearch: (search: Record<string, unknown>): { thread?: string } => ({
    thread: optionalString(search.thread),
  }),
  component: ChatHome,
});
