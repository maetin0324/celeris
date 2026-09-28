import { redirect } from "react-router";
import { clearSessionCookie, getAuthConfig, readValidSession } from "~/auth.server";
import { ownerSessionHash, revokeOwnerSession } from "~/browser-owner.server";
import type { Route } from "./+types/logout";

/** `POST /logout`: セッションクッキーを消して `/login` へ（docs/adr/0008 D1）。GET は `/` へ返す。 */

export function loader() {
  throw redirect("/");
}

export async function action({ request }: Route.ActionArgs) {
  const config = getAuthConfig();
  if (!config.enabled) throw redirect("/");
  // ADR-0080 D6: logout でこの session の browser owner grant と Live View の接続を失効させる。
  const session = await readValidSession(config, request);
  if (session) revokeOwnerSession(ownerSessionHash(session.id));
  throw redirect("/login", { headers: { "Set-Cookie": await clearSessionCookie(config, request) } });
}
