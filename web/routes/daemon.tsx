import { createFileRoute } from "@tanstack/react-router";
import { DaemonScreen } from "../features/ops/daemon-screen";

// R29 /daemon。
export const Route = createFileRoute("/daemon")({ component: DaemonScreen });
