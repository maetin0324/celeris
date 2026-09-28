import { browserActionResponse, runCredentialAction } from "~/celeris/browser-waits.server";
import { getCelerisClient } from "~/celeris/client.server";
import type { Route } from "./+types/browser.waits.$waitId.credential";

/**
 * `POST /browser/waits/:waitId/credential`（ADR-0080 D5）。本人が username/password を登録する resource route。
 * GET / echo / readback は設けない。応答は固定コードだけで、入力値を返さない。
 */
export function loader() {
  return browserActionResponse({ ok: false, code: "method_not_allowed" });
}

export async function action({ request, params }: Route.ActionArgs): Promise<Response> {
  return runCredentialAction(getCelerisClient(), request, params.waitId);
}
