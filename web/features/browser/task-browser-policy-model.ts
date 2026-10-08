import { apiGet, apiMutate, isApiError } from "../../api/client";
import type { BrowserAction, Status, Timeline } from "../../api/generated/types";

// GET/PUT browser/policy は schema の型一覧にない。BrowserTaskPolicy の wire contract。
export type TaskBrowserPolicy = {
  policy_id: string;
  revision: number;
  domain_mode: "common_hosts" | "separate_origins";
  navigation_origins: string[];
  network_domains: string[];
  allowed_actions: BrowserAction[];
  approval_actions: BrowserAction[];
  credential_policy_ids: string[];
  artifact_policy_id?: string | null;
};
export const POLICY_ACTIONS: Record<BrowserAction, string> = {
  navigate: "ページを開く",
  click: "クリック",
  snapshot: "ページの構造を読む",
  extract: "内容を抽出",
  screenshot: "画面を撮影",
  download: "ダウンロード",
  scroll: "スクロール",
  credential_use: "資格情報を使う",
};
export const policyKey = (taskId: string) => ["browser", "task-policy", taskId] as const;
const policyPath = (taskId: string) => `/api/tasks/${encodeURIComponent(taskId)}/browser/policy`;
export function taskBrowserPolicyQuery(taskId: string) {
  return {
    queryKey: policyKey(taskId),
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      apiGet<{ policy: TaskBrowserPolicy | null }>(policyPath(taskId), signal),
  };
}
export function saveTaskBrowserPolicy(taskId: string, policy: TaskBrowserPolicy) {
  return apiMutate<{ updated: boolean }>("PUT", policyPath(taskId), policy);
}
export function browserPrerequisite(status: Status, timeline?: Timeline) {
  if (status !== "blocked") return null;
  const events = (timeline?.items ?? []).filter((item) => item.kind === "event").sort((a, b) => b.seq - a.seq);
  const transition = events.find((item) => item.event.type === "transitioned");
  if (transition && transition.event.type === "transitioned" && transition.event.reason !== "browser_prerequisite")
    return null;
  for (const item of events) {
    if (item.event.type === "browser_prerequisite_resumed") return null;
    if (item.event.type === "browser_prerequisite_blocked") return item.event;
  }
  return null;
}
export function policyEditable(status: Status, prerequisite: boolean) {
  return status === "draft" || status === "ready" || (status === "blocked" && prerequisite);
}
export function policyOriginLabel(policy: TaskBrowserPolicy) {
  if (policy.policy_id === "web-human") return "人が web で編集（retry での引き継ぎを含む）";
  if (policy.policy_id === "auto") return "自動付与の識別子（過去の手動編集・retry の有無は未記録）";
  return "既存 policy（出自は未記録）";
}
export const policyLines = (value: string) => [
  ...new Set(
    value
      .split(/[\n,]/)
      .map((v) => v.trim())
      .filter(Boolean),
  ),
];
export function editedPolicy(
  current: TaskBrowserPolicy | null,
  domains: string,
  credentials: string,
  actions: BrowserAction[],
): TaskBrowserPolicy {
  return {
    ...current,
    policy_id: "web-human",
    revision: (current?.revision ?? 0) + 1,
    domain_mode: current?.domain_mode ?? "common_hosts",
    navigation_origins: current?.navigation_origins ?? [],
    network_domains: policyLines(domains),
    credential_policy_ids: policyLines(credentials),
    allowed_actions: actions,
    approval_actions: (current?.approval_actions ?? []).filter((action) => actions.includes(action)),
  };
}
export function policySaveError(error: unknown) {
  if (!isApiError(error)) return "保存できませんでした。接続を確認して再取得してください。";
  if (error.kind === "forbidden") return "この操作を行う権限がありません。";
  if (error.kind === "conflict") return "タスクの状態が変わりました。再取得して編集できる状態か確認してください。";
  if (error.kind === "validation")
    return "許可サイト・操作・credential policy の入力を確認してください。HTTPS の origin と登録済みの policy ID を指定してください。";
  if (["timeout", "network", "server"].includes(error.kind))
    return "保存結果を確認できません。再送する前に再取得して保存内容を確認してください。";
  return "保存できませんでした。再取得して確認してください。";
}
