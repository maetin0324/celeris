import type { QueryClient } from "@tanstack/react-query";
import { ApiError, apiGet } from "../../api/client";
import type {
  BrowserSettingsPatch,
  OrgNode,
  SitePolicyList,
  SitePolicyPutBody,
  SitePolicyPutResult,
} from "../../api/generated/types";
import { orgKeys } from "../../api/queries/keys";
import { BrowserGatewayError, browserActionGate } from "./browser-query";

export const sitePolicyKeys = { all: ["browser", "site-policies"] as const };
export function sitePoliciesQuery() {
  return {
    queryKey: sitePolicyKeys.all,
    queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<SitePolicyList>("/api/browser/site-policies", signal),
  };
}

async function send<T>(path: string, method: string, body: object, csrf: string): Promise<T> {
  if (!csrf) throw new BrowserGatewayError(403, "not_owner");
  return browserActionGate.run(async () => {
    const response = await fetch(path, {
      method,
      credentials: "same-origin",
      cache: "no-store",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ ...body, csrf }),
    });
    const value = await response.json();
    if (!response.ok) {
      if (path === "/browser/settings" && response.status === 422)
        throw new ApiError("validation", { method, path, status: response.status, body: value });
      throw new BrowserGatewayError(response.status, value.code ?? "request_failed");
    }
    return value as T;
  });
}

function policyPath(policyId: string) {
  if (!/^[A-Za-z0-9._-]{1,64}$/.test(policyId) || policyId === "." || policyId === "..")
    throw new Error("policy ID は英数字・.・_・- の 1〜64 文字で入力してください。");
  return `/browser/site-policies/${policyId}`;
}
export async function saveSitePolicy(client: QueryClient, policyId: string, body: SitePolicyPutBody, csrf: string) {
  const result = await send<SitePolicyPutResult>(policyPath(policyId), "PUT", body, csrf);
  await client.invalidateQueries({ queryKey: sitePolicyKeys.all });
  return result;
}
export async function deleteSitePolicy(client: QueryClient, policyId: string, csrf: string) {
  await send(policyPath(policyId), "DELETE", {}, csrf);
  await client.invalidateQueries({ queryKey: sitePolicyKeys.all });
}
export async function saveBrowserSettings(client: QueryClient, patch: BrowserSettingsPatch, csrf: string) {
  const result = await send<OrgNode>("/browser/settings", "PATCH", patch, csrf);
  await client.invalidateQueries({ queryKey: orgKeys.all });
  return result;
}
export function sitePolicyError(error: unknown): string {
  if (error instanceof BrowserGatewayError) {
    if (error.code === "site_policy_in_use")
      return "この設定は実行課または承認待ちが参照しています。credential policy の選択を外して保存し、承認待ちを解消してから削除してください。";
    if (error.status === 403)
      return "本人確認または送信元を確認できません。画面を再取得して本人確認をやり直してください。";
    if (error.status === 422)
      return "入力を確認してください。origin・ログイン URL・selector、登録済み policy ID と identity の対応を確認してください。";
    if (error.status === 404) return "設定が見つかりません。一覧を再取得してください。";
  }
  return "保存・削除の結果を確認できません。一覧を再取得してからやり直してください。";
}
