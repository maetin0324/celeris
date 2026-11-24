// publish manifest の画面の規則（ADR 2026-11-23-project-observability-publish D3・D4）。
// 読むのは `GET /tasks/{id}/publish/{manifest_id}?version=`（task_api::publish::manifest_status）だけで、署名の状態・違反・根拠の無い
// claim は daemon が読むたびに導いた値をそのまま出す（画面は正本を持たない）。署名は既存の決定の回答（`POST /decisions/{id}/answer`）。
// Celeris は署名を受けて外へ何も送らない。公開は人が Celeris の外で行う（D3.5）。この画面に公開の操作は無い。

import { apiGet, isApiError } from "../../api/client";
import type { ManifestView, ProjectOverviewLink, SignoffState } from "../../api/generated/types";
import type { Session } from "../../lib/session";
import { problemCode, problemDetail } from "../chat/data/problem";
import { warningText } from "../projects/project-overview-panel";

// ---- 文書の項目（task_core::publish の形。wire は unknown[] なので読む側で形を決める） ----

export type ManifestReference =
  | { kind: "commit"; repo: string; sha: string }
  | { kind: "url"; url: string }
  | { kind: "task"; task_id: string }
  | { kind: "knowledge"; path: string; sha256: string }
  | { kind: "chat"; thread_id: string; message_id: string; revision: number; sha256: string }
  | { kind: "artifact"; task_id: string; name: string; sha256: string };

export type ManifestConfidence = "confirmed" | "estimated" | "contradicted" | "needs_decision";
export type ManifestClaim = { id: string; statement: string; confidence: ManifestConfidence; evidence: string[] };
export type ManifestStance = "support" | "refute" | "unknown";
export type ManifestEvidence = {
  id: string;
  stance: ManifestStance;
  statement: string;
  references: ManifestReference[];
};
export type ManifestReproduction = {
  id: string;
  claims: string[];
  commands: string[];
  environment: { repo: string; commit: string; host_class: string };
  data: string[];
  expected: string;
};
export type ManifestLimitation = { id: string; statement: string; claims: string[] };
export type ManifestOpenQuestion = { id: string; statement: string; blocking: boolean };

export const asClaims = (view: ManifestView) => view.claims as ManifestClaim[];
export const asEvidence = (view: ManifestView) => view.evidence as ManifestEvidence[];
export const asReproduction = (view: ManifestView) => view.reproduction as ManifestReproduction[];
export const asLimitations = (view: ManifestView) => view.limitations as ManifestLimitation[];
export const asOpenQuestions = (view: ManifestView) => view.open_questions as ManifestOpenQuestion[];

/** D3.2 の読む順（claims → evidence → 再現条件 → limitations・未解決 → 人の署名）。画面の節もこの順。 */
export const MANIFEST_SECTIONS = [
  ["claims", "1. 主張（claims）"],
  ["evidence", "2. 根拠（evidence）"],
  ["reproduction", "3. 再現条件"],
  ["limitations", "4. limitations・未解決"],
  ["signoff", "5. 人の署名"],
] as const;

export const CONFIDENCE_LABEL: Record<ManifestConfidence, string> = {
  confirmed: "確認済み",
  estimated: "推定",
  contradicted: "矛盾・反証",
  needs_decision: "要判断",
};

export const STANCE_LABEL: Record<ManifestStance, string> = { support: "支持", refute: "反証", unknown: "未確認" };

export const SIGNOFF_LABEL: Record<SignoffState, string> = {
  unsigned: "未署名",
  signed: "署名済み",
  rejected: "署名しないと回答済み",
  stale: "失効（署名の後に file が変わった）",
  superseded: "失効（より新しい版がある）",
};

/** 確認の dialog と署名の節に常に出す文（D3.5）。 */
export const PUBLISH_SIGN_NOTICE = "Celeris は外に送らない。公開は人が行う。";
export const PUBLICATION_REQUIRES_HUMAN = "publication_requires_human";
export const MANIFEST_STALE = "publish_manifest_stale";
export const MANIFEST_INVALID = "publish_manifest_invalid";

// ---- 取得 ----

export function manifestPath(taskId: string, manifestId: string, version?: number): string {
  const base = `/api/tasks/${encodeURIComponent(taskId)}/publish/${encodeURIComponent(manifestId)}`;
  return version === undefined ? base : `${base}?version=${version}`;
}

export const manifestKeys = {
  all: ["publish-manifest"] as const,
  one: (taskId: string, manifestId: string, version?: number) =>
    ["publish-manifest", taskId, manifestId, version ?? "latest"] as const,
};

export function manifestQuery(taskId: string, manifestId: string, version?: number) {
  return {
    queryKey: manifestKeys.one(taskId, manifestId, version),
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      apiGet<ManifestView>(manifestPath(taskId, manifestId, version), signal),
  };
}

// ---- 強い警告（色だけでなく文字で出す。D4） ----

export type ManifestWarning = { code: string; text: string; claim?: string };

/** 画面の先頭にまとめる強い警告。unsupported の claim・未署名・失効・検査の違反・file の上書き・概要と同じ強い警告。 */
export function manifestWarnings(view: ManifestView): ManifestWarning[] {
  const out: ManifestWarning[] = [];
  for (const claim of view.unsupported_claims)
    out.push({ code: "unsupported_claim", text: `根拠の無い主張（unsupported）: ${claim}`, claim });
  if (!view.file_matches_record)
    out.push({
      code: "file_changed",
      text: "file が記録の sha256 と違う（記録の後に上書きされた）。検査済みとして扱わない。",
    });
  if (view.violations.length > 0)
    out.push({
      code: "manifest_invalid",
      text: `manifest が検査を通らない（違反 ${view.violations.length} 件）。署名できない。`,
    });
  const signoff = view.signoff.state;
  if (signoff === "unsigned") out.push({ code: "unsigned", text: "未署名。公開の前に人の署名が要る。" });
  else if (signoff === "stale")
    out.push({ code: "signoff_stale", text: "署名は失効した。署名の後に manifest の file が変わった。" });
  else if (signoff === "superseded")
    out.push({
      code: "signoff_superseded",
      text: `署名は失効した。より新しい版 v${latestVersion(view)} があり、署名し直しが要る。`,
    });
  else if (signoff === "rejected") out.push({ code: "signoff_rejected", text: "人が「署名しない」と回答した。" });
  for (const code of view.strong_warnings) out.push({ code, text: warningText(code) });
  return out;
}

export function latestVersion(view: ManifestView): number {
  return view.versions.at(-1) ?? view.version;
}

// ---- 参照の drill-down（概要の link と同じ部品で出すため、ProjectOverviewLink に写す。D1.4） ----

export function referenceLink(reference: ManifestReference, manifestTaskId: string): ProjectOverviewLink {
  switch (reference.kind) {
    case "commit":
      // commit の参照は task を持たない。manifest を作った task の変更の画面へ移す。
      return { kind: "git", repo: reference.repo, sha: reference.sha, task_id: manifestTaskId };
    case "url":
      return { kind: "literature", url: reference.url };
    case "task":
      return { kind: "task", task_id: reference.task_id };
    case "knowledge":
      return { kind: "literature", path: reference.path, sha256: reference.sha256 };
    case "chat":
      return { ...reference, kind: "chat" };
    case "artifact":
      return { kind: "artifact", task_id: reference.task_id, name: reference.name, sha256: reference.sha256 };
  }
}

// ---- 署名 ----

export type Signability = { ok: true; decisionId: string } | { ok: false; reason: string };

/** 署名の操作を出せるか。人の login（gateway の session）が無ければ出さない。最終の判定は daemon（403・409・422）。 */
export function signability(view: ManifestView, session: Session | undefined): Signability {
  if (!session?.authenticated) return { ok: false, reason: "署名は人の login のときだけできます。" };
  const signoff = view.signoff;
  if (signoff.state === "signed") return { ok: false, reason: "この版は署名済みです。" };
  if (signoff.state === "stale")
    return { ok: false, reason: "署名は失効しました。新しい版の manifest と署名の決定が要ります。" };
  if (signoff.state === "superseded")
    return { ok: false, reason: `より新しい版 v${latestVersion(view)} があります。新しい版に署名してください。` };
  if (signoff.state === "rejected")
    return { ok: false, reason: "「署名しない」と回答済みです。答えを変えるには task の決定の画面へ移ってください。" };
  if (!signoff.decision_id || signoff.decision_status !== "open")
    return { ok: false, reason: `署名の決定（${signoff.key}）がまだ出ていません。` };
  if (!view.file_matches_record || view.violations.length > 0)
    return { ok: false, reason: "manifest が検査を通らないので署名できません。" };
  return { ok: true, decisionId: signoff.decision_id };
}

/** 確認の dialog の対象の文。sha256 は記録の値（決定の question と同じ。D3.3）。 */
export function signTarget(view: ManifestView): string {
  return `${view.name}（sha256:${view.recorded_sha256}）`;
}

export type SignoffFailure = { message: string; stale: boolean };

/** 署名の回答の失敗を人の読める文にする。stale は読み直しを促す。 */
export function signoffFailure(error: unknown): SignoffFailure {
  if (!isApiError(error)) return { message: "署名を送れませんでした。", stale: false };
  if (error.kind === "timeout" || error.kind === "network" || error.kind === "aborted")
    return { message: "結果を確認できません。読み直して署名の状態を確かめてください。", stale: true };
  const code = problemCode(error.body);
  const detail = problemDetail(error.body);
  if (error.status === 403)
    return { message: `署名は人だけができます（${code ?? PUBLICATION_REQUIRES_HUMAN}）。`, stale: false };
  if (error.status === 502 && code === "daemon_auth")
    return {
      message: "署名を送れませんでした。daemon が要求の主体を拒みました（daemon_auth）。署名は人の主体だけができます。",
      stale: false,
    };
  if (error.status === 409 && code === MANIFEST_STALE)
    return { message: `manifest の file が署名の決定の sha256 と違います（${MANIFEST_STALE}）。`, stale: true };
  if (error.status === 409) return { message: "署名の決定の状態が変わりました。読み直してください。", stale: true };
  if (error.status === 422)
    return {
      message: `manifest が検査を通りません（${code ?? MANIFEST_INVALID}）${detail ? `: ${detail}` : ""}`,
      stale: true,
    };
  return { message: detail ?? `署名を送れませんでした（${error.status ?? error.kind}）。`, stale: false };
}
