import { createFileRoute } from "@tanstack/react-router";
import { KnowledgeScreen } from "../features/knowledge/knowledge-screen";
export const Route = createFileRoute("/knowledge/")({ component: KnowledgeScreen });
