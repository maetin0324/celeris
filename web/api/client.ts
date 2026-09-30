// API client（P2-01、ADR-0081 D3/D5）。same-origin の `/api/*` だけを呼ぶ。
// timeout 15 s、呼び出し側の AbortSignal、session の AbortSignal のどれかで打ち切る。
// ここでは再試行しない。GET の再試行は QueryClient の retry（query-client.ts）だけが持ち、変更系は再送しない。

export const DEFAULT_TIMEOUT_MS = 15_000;

export type ApiErrorKind =
  | "timeout" // 15 s を超えた
  | "aborted" // 呼び出し側か session の終了で止めた
  | "network" // 応答が来なかった
  | "unauthorized" // 401
  | "forbidden" // 403
  | "not_found" // 404
  | "conflict" // 409
  | "validation" // 400 / 422
  | "rate_limited" // 429
  | "server" // 5xx
  | "http" // その他の非 2xx
  | "parse"; // 2xx だが JSON として読めない

export class ApiError extends Error {
  readonly kind: ApiErrorKind;
  readonly status: number | undefined;
  readonly method: string;
  readonly path: string;
  /** daemon が返した本文（422 の文言など）。読めなければ undefined。 */
  readonly body: unknown;

  constructor(
    kind: ApiErrorKind,
    init: { method: string; path: string; status?: number; body?: unknown; cause?: unknown },
  ) {
    super(`${init.method} ${init.path}: ${kind}${init.status === undefined ? "" : ` (${init.status})`}`, {
      cause: init.cause,
    });
    this.name = "ApiError";
    this.kind = kind;
    this.status = init.status;
    this.method = init.method;
    this.path = init.path;
    this.body = init.body;
  }
}

export function isApiError(error: unknown): error is ApiError {
  return error instanceof ApiError;
}

export function kindForStatus(status: number): ApiErrorKind {
  if (status === 401) return "unauthorized";
  if (status === 403) return "forbidden";
  if (status === 404) return "not_found";
  if (status === 409) return "conflict";
  if (status === 400 || status === 422) return "validation";
  if (status === 429) return "rate_limited";
  if (status >= 500) return "server";
  return "http";
}

export type ApiMethod = "GET" | "POST" | "PUT" | "PATCH" | "DELETE";

export type ApiRequest = {
  method?: ApiMethod;
  /** JSON にして送る。 */
  body?: unknown;
  signal?: AbortSignal;
  timeoutMs?: number;
};

type ClientConfig = {
  fetcher: typeof fetch;
  onUnauthorized: () => void;
};

const config: ClientConfig = {
  fetcher: (input, init) => fetch(input, init),
  onUnauthorized: () => {},
};

/** 起動時とテストで差し替える。401 の処理（cache の破棄と login への移動）は session 側が渡す。 */
export function configureApiClient(next: Partial<ClientConfig>): void {
  Object.assign(config, next);
}

let sessionController = new AbortController();

/** session の終了（logout・失効・切替）で進行中の要求をすべて止める。以後の要求は新しい session に属する。 */
export function abortSessionRequests(): void {
  sessionController.abort(new DOMException("session ended", "AbortError"));
  sessionController = new AbortController();
}

/** same-origin の `/api/` 配下だけを許す。絶対 URL・`//host`・`..` を含む path は投げる。 */
export function assertApiPath(path: string): void {
  if (!path.startsWith("/api/") || path.startsWith("//") || path.includes("\\") || /(^|\/)\.\.(\/|$|\?)/.test(path)) {
    throw new TypeError(`API path must be a same-origin /api/ path: ${path}`);
  }
}

async function readBody(response: Response): Promise<unknown> {
  const text = await response.text();
  if (text === "") return undefined;
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return text;
  }
}

export async function apiFetch<T>(path: string, request: ApiRequest = {}): Promise<T> {
  assertApiPath(path);
  const method = request.method ?? "GET";
  const timeoutMs = request.timeoutMs ?? DEFAULT_TIMEOUT_MS;
  const controller = new AbortController();
  let timedOut = false;
  const timer = setTimeout(() => {
    timedOut = true;
    controller.abort(new DOMException("timeout", "TimeoutError"));
  }, timeoutMs);
  const sessionSignal = sessionController.signal;
  const sources = request.signal ? [request.signal, sessionSignal] : [sessionSignal];
  const onAbort = () => controller.abort(new DOMException("aborted", "AbortError"));
  for (const source of sources) {
    if (source.aborted) onAbort();
    else source.addEventListener("abort", onAbort, { once: true });
  }

  const headers: Record<string, string> = { Accept: "application/json" };
  if (request.body !== undefined) headers["Content-Type"] = "application/json";
  try {
    let response: Response;
    try {
      response = await config.fetcher(path, {
        method,
        headers,
        body: request.body === undefined ? undefined : JSON.stringify(request.body),
        credentials: "same-origin",
        cache: "no-store",
        signal: controller.signal,
      });
    } catch (cause) {
      if (timedOut) throw new ApiError("timeout", { method, path, cause });
      if (controller.signal.aborted) throw new ApiError("aborted", { method, path, cause });
      throw new ApiError("network", { method, path, cause });
    }
    if (!response.ok) {
      const body = await readBody(response).catch(() => undefined);
      const kind = kindForStatus(response.status);
      if (kind === "unauthorized") config.onUnauthorized();
      throw new ApiError(kind, { method, path, status: response.status, body });
    }
    if (response.status === 204) return undefined as T;
    let text: string;
    try {
      text = await response.text();
    } catch (cause) {
      const kind: ApiErrorKind = timedOut ? "timeout" : controller.signal.aborted ? "aborted" : "network";
      throw new ApiError(kind, { method, path, status: response.status, cause });
    }
    if (text === "") return undefined as T;
    try {
      return JSON.parse(text) as T;
    } catch (cause) {
      throw new ApiError("parse", { method, path, status: response.status, cause });
    }
  } finally {
    clearTimeout(timer);
    for (const source of sources) source.removeEventListener("abort", onAbort);
  }
}

export function apiGet<T>(path: string, signal?: AbortSignal): Promise<T> {
  return apiFetch<T>(path, { method: "GET", signal });
}

/** 変更系。自動では再送しない（timeout でも結果不明として呼び出し側へ返す）。 */
export function apiMutate<T>(method: Exclude<ApiMethod, "GET">, path: string, body?: unknown, signal?: AbortSignal) {
  return apiFetch<T>(path, { method, body, signal });
}
