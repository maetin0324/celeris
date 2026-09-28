import type { CelerisClient } from "~/celeris/client.server";
import { getCelerisClient } from "~/celeris/client.server";
import { celerisErrorResponse } from "~/celeris/errors";
import { redactLiveViewUrlText } from "~/lib/browser";
import type { Route } from "./+types/events";

/**
 * `/events`（SSE 中継、docs/DESIGN.md §6.4、docs/celeris-api-v1.md §4）。resource route（コンポーネントを持たない）。
 * celeris の `GET /stream` をそのまま中継する。バイト列は `live_view_url` の値（ADR-0080 D6）以外は加工しない。
 * celeris 側の 503 `too_many_streams`（`CelerisClient` は非 2xx を `CelerisError` にする）や接続不可
 * （`CelerisUnavailable`）は、同じ status の `Response` を返す（同じ経路の document route と違い resource route
 * には ErrorBoundary が無いので、`Response` を投げるのではなくそのまま返す。docs/adr/0004 D6）。
 */
export async function relayEvents(client: CelerisClient, request: Request): Promise<Response> {
  const url = new URL(request.url);
  const taskId = url.searchParams.get("task_id");
  const lastEventId = request.headers.get("Last-Event-ID");

  let upstream: Response;
  try {
    upstream = await client.stream({
      taskId: taskId ?? undefined,
      lastEventId: lastEventId ?? undefined,
      signal: request.signal,
    });
  } catch (e) {
    return celerisErrorResponse(e);
  }

  // ADR-0080 D6: `browser_updated` の dashboard URL は行単位で消してから流す（それ以外のバイトは加工しない）。
  return new Response(upstream.body ? redactLiveViewStream(upstream.body) : null, {
    status: upstream.status,
    headers: {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-store",
      "X-Accel-Buffering": "no",
    },
  });
}

/** SSE を行単位で読み、`live_view_url` の値だけを null にする。 */
export function redactLiveViewStream(body: ReadableStream<Uint8Array>): ReadableStream<Uint8Array> {
  const decoder = new TextDecoder();
  const encoder = new TextEncoder();
  let pending = "";
  return body.pipeThrough(
    new TransformStream<Uint8Array, Uint8Array>({
      transform(chunk, controller) {
        pending += decoder.decode(chunk, { stream: true });
        const nl = pending.lastIndexOf("\n");
        if (nl < 0) return;
        controller.enqueue(encoder.encode(redactLiveViewUrlText(pending.slice(0, nl + 1))));
        pending = pending.slice(nl + 1);
      },
      flush(controller) {
        pending += decoder.decode();
        if (pending) controller.enqueue(encoder.encode(redactLiveViewUrlText(pending)));
      },
    }),
  );
}

export async function loader({ request }: Route.LoaderArgs): Promise<Response> {
  return relayEvents(getCelerisClient(), request);
}
