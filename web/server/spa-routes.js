// SPA の画面の path（P2-02・R42）。gateway は ここに無い path に index.html を 404 で返し、shell の中の 404 を出す。
// web/routes/ の宣言と同じ集合であることを server/spa-routes.test.mjs が routeTree.gen.ts と照らして確かめる。
export const spaRoutePatterns = [
  "/",
  "/inbox",
  "/notifications",
  "/login",
  "/org",
  "/org/secretary",
  "/org/$id",
  "/projects",
  "/projects/$id",
  "/projects/$id/docs",
  "/projects/$id/docs/maintenance",
  "/projects/$id/browser-identities",
  "/board",
  "/knowledge",
  "/knowledge/inbox",
  "/knowledge/skills",
  "/reports",
  "/approvals",
  "/artifacts",
  "/tasks",
  "/tasks/new",
  "/tasks/$id",
  "/tasks/$id/files",
  "/tasks/$id/changes",
  "/tasks/$id/runs/$runId",
  "/plans/new",
  "/daemon",
  "/providers",
  "/accounts",
  "/clusters",
  "/releases",
  "/graph",
  "/browser",
  "/browser/settings",
  "/browser/runs/$taskId/$runId",
  "/help",
];

const matchers = spaRoutePatterns.map((pattern) => {
  const parts = pattern.split("/").filter(Boolean);
  return (segments) =>
    segments.length === parts.length && parts.every((part, i) => part.startsWith("$") || part === segments[i]);
});

/** `pathname` が宣言済みの画面か。末尾の `/` は無視する。 */
export function isSpaRoute(pathname) {
  const segments = pathname.split("/").filter(Boolean);
  if (segments.some((segment) => segment === "." || segment === "..")) return false;
  return matchers.some((match) => match(segments));
}
