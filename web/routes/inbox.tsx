import { createFileRoute } from "@tanstack/react-router";
import { InboxScreen } from "../features/inbox/inbox-screen";

export const Route = createFileRoute("/inbox")({ component: InboxScreen });
