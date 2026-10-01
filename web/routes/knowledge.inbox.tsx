import { createFileRoute } from "@tanstack/react-router";
import { KnowledgeInboxScreen } from "../features/knowledge/knowledge-screen";
export const Route = createFileRoute("/knowledge/inbox")({ component: KnowledgeInboxScreen });
