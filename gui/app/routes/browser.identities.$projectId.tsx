import { useRef } from "react";
import { Link, useFetcher } from "react-router";
import { browserOwnerView, checkOwner, exactSameOrigin, verifyOwnerCsrfToken } from "~/browser-owner.server";
import { getCelerisClient } from "~/celeris/client.server";
import { CelerisError } from "~/celeris/errors";
import type { Route } from "./+types/browser.identities.$projectId";

interface Identity {
  identity_id: string;
  project_id: string;
  origin: string;
  generation: number;
  expires_at: number;
  state: string;
}
const ID = /^[0-9A-Za-z_-]{1,64}$/;
const headers = { "Cache-Control": "no-store", "Referrer-Policy": "no-referrer" };
export async function loader({ params, request }: Route.LoaderArgs) {
  if (!ID.test(params.projectId)) throw new Response("Not found", { status: 404 });
  const owner = await browserOwnerView(request);
  if (!owner.isOwner) return { identities: [] as Identity[], owner, error: "本人のセッションで開いてください。" };
  try {
    const result = await getCelerisClient().get<{ identities: Identity[] }>("/browser/identities", {
      query: { project_id: params.projectId },
      signal: request.signal,
    });
    return { identities: result.identities.filter((i) => i.project_id === params.projectId), owner, error: null };
  } catch {
    return { identities: [] as Identity[], owner, error: "Identity を読み込めませんでした。" };
  }
}
export async function action({ params, request }: Route.ActionArgs): Promise<Response> {
  const fail = (code: string, status = 422) => Response.json({ ok: false, code }, { status, headers });
  if (!ID.test(params.projectId) || request.method !== "POST") return fail("invalid_input");
  const owner = await checkOwner(request);
  if (!owner.ok) return fail(owner.code, owner.status);
  if (!exactSameOrigin(request)) return fail("csrf_failed", 403);
  if (Number(request.headers.get("content-length") ?? 0) > 262144) return fail("invalid_input");
  const form = await request.formData();
  if (!verifyOwnerCsrfToken(owner.config, owner.sessionHash, form.get("csrf"))) return fail("csrf_failed", 403);
  const intent = form.get("intent");
  const identityId = form.get("identity_id");
  if (typeof identityId !== "string" || !ID.test(identityId)) return fail("invalid_input");
  const client = getCelerisClient();
  try {
    if (intent === "revoke" || intent === "delete") {
      const list = await client.get<{ identities: Identity[] }>("/browser/identities", {
        query: { project_id: params.projectId },
        signal: request.signal,
      });
      if (!list.identities.some((i) => i.project_id === params.projectId && i.identity_id === identityId))
        return fail("identity_not_found", 404);
      if (intent === "revoke")
        await client.post(`/browser/identities/${identityId}/revoke`, {}, { signal: request.signal });
      else await client.delete(`/browser/identities/${identityId}`, { signal: request.signal });
    } else if (intent === "register") {
      const origin = form.get("origin");
      const confirmed = form.get("demand_confirmed_by");
      const raw = form.get("state_json");
      if (
        typeof origin !== "string" ||
        !/^https:\/\/[^\s/?#]+$/.test(origin) ||
        typeof confirmed !== "string" ||
        !confirmed.trim() ||
        typeof raw !== "string"
      )
        return fail("invalid_input");
      let state: unknown;
      try {
        state = JSON.parse(raw);
      } catch {
        return fail("invalid_input");
      }
      if (!state || typeof state !== "object" || !Array.isArray((state as { entries?: unknown }).entries))
        return fail("invalid_input");
      await client.post(
        "/browser/identities",
        {
          identity_id: identityId,
          project_id: params.projectId,
          origin,
          demand_confirmed_by: confirmed.trim(),
          ttl_secs: 7 * 24 * 3600,
          state,
        },
        { signal: request.signal },
      );
    } else return fail("invalid_input");
    return Response.json({ ok: true, code: intent }, { headers });
  } catch (e) {
    return fail(e instanceof CelerisError ? e.code : "celeris_unavailable", e instanceof CelerisError ? e.status : 503);
  }
}

export default function BrowserIdentities({ loaderData, params }: Route.ComponentProps) {
  const fetcher = useFetcher<{ ok: boolean; code: string }>();
  const formRef = useRef<HTMLFormElement>(null);
  const busy = fetcher.state !== "idle";
  return (
    <main className="space-y-5 p-6">
      <Link to={`/projects/${params.projectId}`}>← プロジェクト</Link>
      <h1>Browser Identity</h1>
      <p>
        project: {params.projectId}。期限付き identity の metadata
        だけを表示します。利用は隔離環境の導入後に有効になります。
      </p>
      {loaderData.error && <p role="alert">{loaderData.error}</p>}
      {fetcher.data && (
        <p role="status">{fetcher.data.ok ? "更新しました。" : `操作できませんでした (${fetcher.data.code})`}</p>
      )}
      {loaderData.owner.isOwner && (
        <>
          <ul>
            {loaderData.identities.map((i) => (
              <li key={i.identity_id} className="space-y-2 border-b py-3">
                <p>
                  {i.origin} · {i.identity_id} · 世代 {i.generation} · 期限{" "}
                  {new Date(i.expires_at * 1000).toLocaleString()} · {i.state}
                </p>
                <fetcher.Form method="post">
                  <input type="hidden" name="csrf" value={loaderData.owner.csrfToken ?? ""} />
                  <input type="hidden" name="identity_id" value={i.identity_id} />
                  <button type="submit" name="intent" value="revoke" disabled={busy || i.state !== "active"}>
                    失効
                  </button>{" "}
                  <button type="submit" name="intent" value="delete" disabled={busy}>
                    削除
                  </button>
                </fetcher.Form>
              </li>
            ))}
          </ul>
          <h2>需要を確認した identity の登録</h2>
          <fetcher.Form
            ref={formRef}
            method="post"
            autoComplete="off"
            className="space-y-2"
            onSubmit={() => {
              setTimeout(() => {
                if (formRef.current) {
                  const field = formRef.current.elements.namedItem("state_json") as HTMLInputElement | null;
                  if (field) field.value = "";
                }
              }, 0);
            }}
          >
            <input type="hidden" name="csrf" value={loaderData.owner.csrfToken ?? ""} />
            <label>
              Identity ID <input required name="identity_id" pattern="[0-9A-Za-z_-]{1,64}" />
            </label>{" "}
            <label>
              Origin <input required name="origin" type="url" placeholder="https://example.com" />
            </label>{" "}
            <label>
              需要を確認した人 <input required name="demand_confirmed_by" />
            </label>{" "}
            <label>
              Browser state JSON（入力は非表示） <input required name="state_json" type="password" autoComplete="off" />
            </label>{" "}
            <button type="submit" name="intent" value="register" disabled={busy}>
              登録 (7 日)
            </button>
          </fetcher.Form>
        </>
      )}
    </main>
  );
}
