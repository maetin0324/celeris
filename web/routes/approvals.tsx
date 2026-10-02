import { createFileRoute } from "@tanstack/react-router";
import { ApprovalsScreen } from "../features/approvals/approvals-screen";

export const Route = createFileRoute("/approvals")({ component: ApprovalsScreen });
