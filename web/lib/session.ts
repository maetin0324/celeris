// 認証の境界（P1-06）。gateway のローカル session（GET /api/session）だけで決め、daemon の health は見ない。
// 保護データの cache は登録制にし、session が切れたら全部捨ててから /login?next= へ移る。

export type Session = { authenticated: boolean; authRequired: boolean };

const protectedCaches = new Set<() => void>();

/** 保護データを持つ cache の破棄関数を登録する。戻り値で登録を外す。 */
export function registerProtectedCache(clear: () => void): () => void {
  protectedCaches.add(clear);
  return () => protectedCaches.delete(clear);
}

export function clearProtectedCaches(): void {
  for (const clear of protectedCaches) clear();
}

/** `next` は同一オリジンの絶対パスだけ（gateway の safeNextPath と同じ規則）。 */
export function safeNextPath(value: unknown): string {
  if (typeof value !== "string" || !value.startsWith("/") || value.startsWith("//")) return "/";
  if (value.includes("\\") || [...value].some((c) => c.charCodeAt(0) < 0x20 || c.charCodeAt(0) === 0x7f)) return "/";
  try {
    const url = new URL(value, "http://celeris-web.invalid");
    if (url.origin !== "http://celeris-web.invalid") return "/";
    return `${url.pathname}${url.search}${url.hash}`;
  } catch {
    return "/";
  }
}

export function loginHref(next: string): string {
  return `/login?next=${encodeURIComponent(safeNextPath(next))}`;
}

export async function fetchSession(fetcher: typeof fetch = fetch): Promise<Session> {
  const response = await fetcher("/api/session", { headers: { Accept: "application/json" }, cache: "no-store" });
  if (!response.ok) return { authenticated: false, authRequired: true };
  return (await response.json()) as Session;
}

/** 401 を受けたときの共通処理。cache を捨てて login へ移る。 */
export function onUnauthenticated(
  current: string,
  go: (href: string) => void = (href) => window.location.assign(href),
) {
  clearProtectedCaches();
  go(loginHref(current));
}

/** 保護 API の fetch。401 なら session 失効として扱う。 */
export async function protectedFetch(input: string, init?: RequestInit): Promise<Response> {
  const response = await fetch(input, init);
  if (response.status === 401) onUnauthenticated(`${window.location.pathname}${window.location.search}`);
  return response;
}
