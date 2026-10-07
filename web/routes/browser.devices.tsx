import { createFileRoute } from "@tanstack/react-router";
import { TrustedDevicesScreen } from "../features/browser/trusted-devices";

export const Route = createFileRoute("/browser/devices")({ component: TrustedDevicesScreen });
