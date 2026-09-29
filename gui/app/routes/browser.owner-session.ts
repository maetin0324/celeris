import { browserActionResponse, runOwnerChallengeAction } from "~/celeris/browser-waits.server";
import type { Route } from "./+types/browser.owner-session";

/**
 * `POST /browser/owner-session`（ADR-0080 D6）。この cookie session の本人確認 challenge を発行する。
 * 本人はローカルで `celerisctl browser owner-session approve <challenge>` を実行して確定する。
 */
export function loader() {
  return browserActionResponse({ ok: false, code: "method_not_allowed" });
}

export async function action({ request }: Route.ActionArgs): Promise<Response> {
  return runOwnerChallengeAction(request);
}
