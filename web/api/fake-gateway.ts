// 単体試験用の gateway の写し: server/relay.js と同じく same-origin の /api/x を daemon の /api/v1/x へ前置して
// fixture を引く。要求 path（gateway が受けた path）を記録し、daemon に無い path は 404 を返す。
export type FakeGatewayCall = { method: string; path: string; daemonPath: string; body: unknown };

export function fakeGateway(daemon: Record<string, unknown>) {
  const calls: FakeGatewayCall[] = [];
  const fetcher: typeof fetch = async (input, init) => {
    const url = new URL(String(input), "http://gateway.test");
    const daemonPath = url.pathname.startsWith("/api/") ? `/api/v1${url.pathname.slice("/api".length)}` : url.pathname;
    const body = typeof init?.body === "string" ? (JSON.parse(init.body) as unknown) : undefined;
    calls.push({ method: init?.method ?? "GET", path: url.pathname, daemonPath, body });
    if (!(daemonPath in daemon)) return new Response(JSON.stringify({ error: "not_found" }), { status: 404 });
    return new Response(JSON.stringify(daemon[daemonPath]), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  };
  return { calls, fetcher };
}
