import { browserActionResponse, runDecisionAction } from "~/celeris/browser-waits.server";
import { getCelerisClient } from "~/celeris/client.server";
import type { Route } from "./+types/browser.waits.$waitId.decision";

/** `POST /browser/waits/:waitId/decision`（ADR-0080 D5）。`approve_once` / `deny` を本人の attestation 付きで中継する。 */
export function loader() {
  return browserActionResponse({ ok: false, code: "method_not_allowed" });
}

export async function action({ request, params }: Route.ActionArgs): Promise<Response> {
  return runDecisionAction(getCelerisClient(), request, params.waitId);
}
