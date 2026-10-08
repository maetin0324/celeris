import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useId, useState } from "react";
import { ApiError, apiGet, apiMutate } from "../../api/client";
import type { BrowserAction, BrowserSettingsPatch, OrgList, OrgNode, Tier } from "../../api/generated/types";
import { orgKeys } from "../../api/queries/keys";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Button } from "../../components/ui/button";
import { ConfirmDialog } from "../../components/ui/confirm-dialog";
import { fieldClassName, Input } from "../../components/ui/input";
import { Section } from "../../components/ui/panel";

const NODE_ID = "browser-execution";
const SETTINGS_PATH = `/api/org/${NODE_ID}/browser-settings`;
const lines = (value: string) =>
  value
    .split("\n")
    .map((item) => item.trim())
    .filter(Boolean);
const list = (value: string) =>
  value
    .split(/[\n,]/)
    .map((item) => item.trim())
    .filter(Boolean);
const displayTime = (value: string) =>
  Number.isNaN(Date.parse(value)) ? value : new Date(value).toLocaleString("ja-JP");

type Field = "origins" | "harnesses" | "budget" | "credentials" | "identity" | "approval";

/**
 * ADR 2026-10-08 D2: 承認対象に戻せる業務操作（既定はどちらも承認なし。credential_use は常に承認）。
 * 承認対象の操作は、agent が request-approval で承認待ちを開き、承認後に同じ session で一度だけ実行する。
 */
const APPROVAL_CHOICES: ReadonlyArray<{ action: BrowserAction; label: string; help: string }> = [
  {
    action: "click",
    label: "click（ページの要素を押す）",
    help: "許可 origin の中でも、押す前に毎回承認待ちになります。承認すると同じ session で一度だけ押します。",
  },
  {
    action: "download",
    label: "download（ファイルを保存する）",
    help: "保存する前に毎回承認待ちになります。承認すると同じ session で一度だけ保存します。",
  },
];
type Errors = Partial<Record<Field, string>>;

function originHint(value: string): string | null {
  if (value === "*" || /^https?:\/\/\*$/.test(value))
    return "全サイトを表す * は使えません。必要なサイトを指定してください。";
  if (/^https?:\/\/\*\.[^./:]+(?::\d+)?$/.test(value))
    return "公開サフィックスへの wildcard は使えません。より狭いドメインを指定してください。";
  if (/^(ftp|file):/i.test(value)) return "外向きのサイトは HTTPS を使ってください。";
  if (/^http:\/\//i.test(value) && !/^http:\/\/(localhost|127\.0\.0\.1|\[::1\])(?::\d+)?\/?$/i.test(value))
    return "HTTP は localhost・127.0.0.1・[::1] のみ使えます。";
  if (!/^https?:\/\//i.test(value)) return "https:// から始まる origin を入力してください。";
  return null;
}

function errorForApi(error: unknown): Errors {
  if (!(error instanceof ApiError) || error.kind !== "validation") return {};
  const body = error.body as { errors?: Array<{ field?: string; message?: string }>; detail?: string } | undefined;
  const message =
    body?.errors
      ?.map((item) => item.message)
      .filter(Boolean)
      .join("。") ||
    body?.detail ||
    "入力を確認してください。";
  const field = body?.errors?.[0]?.field ?? "";
  if (field.includes("allowed_domains") || /origin|scheme|wildcard|suffix|domain/i.test(message))
    return { origins: message };
  if (/harness/i.test(field + message)) return { harnesses: message };
  if (/budget|lane|attempt/i.test(field + message)) return { budget: message };
  if (/identity/i.test(field + message)) return { identity: message };
  if (/approval/i.test(field + message)) return { approval: message };
  if (/credential/i.test(field + message)) return { credentials: message };
  return {
    origins: message,
    harnesses: message,
    budget: message,
    credentials: message,
    identity: message,
    approval: message,
  };
}

function FieldError({ id, message }: { id: string; message?: string }) {
  return message ? (
    <p id={id} role="alert" className="text-label text-danger-foreground">
      {message}
    </p>
  ) : null;
}

function SettingsForm({ node, onSaved }: { node: OrgNode; onSaved: (node: OrgNode) => void }) {
  const id = useId();
  const queryClient = useQueryClient();
  const browser = node.profile?.browser;
  const [origins, setOrigins] = useState(browser?.allowed_domains ?? []);
  const [newOrigin, setNewOrigin] = useState("");
  const [harnesses, setHarnesses] = useState((node.profile?.harnesses?.allowed ?? []).join(", "));
  const [defaultHarness, setDefaultHarness] = useState(node.profile?.harnesses?.default ?? "");
  const [attempts, setAttempts] = useState(String(node.profile?.budget?.max_attempts ?? ""));
  const [lane, setLane] = useState(node.profile?.budget?.max_lane ?? "");
  const [approvalActions, setApprovalActions] = useState<BrowserAction[]>(browser?.approval_actions ?? []);
  const [policies, setPolicies] = useState((browser?.credential_policy_ids ?? []).join("\n"));
  const [identity, setIdentity] = useState(
    Object.entries(browser?.credential_identity_ids ?? {})
      .map(([policy, value]) => `${policy}=${value}`)
      .join("\n"),
  );
  const [errors, setErrors] = useState<Errors>({});

  function addOrigin() {
    const value = newOrigin.trim();
    if (!value) return;
    if (origins.includes(value)) {
      setErrors({ origins: "この origin は追加済みです。" });
      return;
    }
    setOrigins([...origins, value]);
    setNewOrigin("");
    setErrors({});
  }

  async function save() {
    setErrors({});
    const mappings: Record<string, string> = {};
    for (const row of lines(identity)) {
      const separator = row.indexOf("=");
      if (separator < 1 || separator === row.length - 1) {
        setErrors({ identity: "各行を credential policy ID=identity ID の形で入力してください。" });
        throw new Error("identity の対応を確認してください。");
      }
      mappings[row.slice(0, separator).trim()] = row.slice(separator + 1).trim();
    }
    const patch: BrowserSettingsPatch = {
      allowed_domains: origins,
      harnesses: { allowed: list(harnesses), default: defaultHarness.trim() || null },
      budget: { max_attempts: attempts ? Number(attempts) : null, max_lane: (lane || null) as Tier | null },
      approval_actions: APPROVAL_CHOICES.map((c) => c.action).filter((a) => approvalActions.includes(a)),
      credential_policy_ids: lines(policies),
      credential_identity_ids: mappings,
    };
    try {
      const updated = await apiMutate<OrgNode>("PATCH", SETTINGS_PATH, patch);
      onSaved(updated);
      await queryClient.invalidateQueries({ queryKey: orgKeys.all });
    } catch (error) {
      setErrors(errorForApi(error));
      if (error instanceof ApiError && error.kind === "forbidden")
        throw new Error("管理権限または送信元を確認してください。");
      if (error instanceof ApiError && error.kind === "validation")
        throw new Error("入力を確認してください。項目ごとに理由を表示しました。");
      throw new Error("保存できませんでした。状態を確認してからやり直してください。");
    }
  }

  const hint = newOrigin.trim() ? originHint(newOrigin.trim()) : null;
  return (
    <div className="space-y-6">
      <Section title="許可 origin" description="必要なサイトだけを登録します。変更は次の browser run から使われます。">
        <ul className="divide-y divide-border rounded-md border border-border" aria-label="許可 origin の一覧">
          {origins.map((origin, index) => (
            <li key={origin} className="flex min-w-0 items-center justify-between gap-2 px-3 py-2">
              <span className="min-w-0 break-all font-mono text-label">{origin}</span>
              <Button
                variant="secondary"
                className="shrink-0"
                onClick={() => setOrigins(origins.filter((_, i) => i !== index))}
                aria-label={`${origin} を削除`}
              >
                削除
              </Button>
            </li>
          ))}
        </ul>
        {origins.length === 0 ? (
          <p role="alert" className="text-label text-danger-foreground">
            origin は 1 件以上必要です。
          </p>
        ) : null}
        <div className="mt-3 flex flex-col gap-2 sm:flex-row sm:items-end">
          <label className="min-w-0 flex-1 space-y-1 text-label font-medium" htmlFor={`${id}-origin`}>
            追加する origin
            <Input
              id={`${id}-origin`}
              value={newOrigin}
              onChange={(event) => setNewOrigin(event.target.value)}
              placeholder="https://billing.example.com"
              aria-describedby={`${id}-origin-help`}
            />
          </label>
          <Button variant="secondary" onClick={addOrigin}>
            追加
          </Button>
        </div>
        <p id={`${id}-origin-help`} className="mt-2 text-label text-muted-foreground">
          外向きは HTTPS。HTTP は loopback のみ。wildcard は https://*.example.com の形で、親ドメインは含みません。port
          省略時は HTTPS 443 / HTTP 80 です。
        </p>
        {hint ? (
          <p role="status" className="text-label text-danger-foreground">
            {hint}
          </p>
        ) : null}
        <FieldError id={`${id}-origins-error`} message={errors.origins} />
      </Section>
      <Section title="実行環境と予算">
        <div className="grid gap-4 sm:grid-cols-2">
          <label htmlFor={`${id}-harnesses`} className="space-y-1 text-label font-medium">
            使える harness（カンマ区切り）
            <Input
              id={`${id}-harnesses`}
              value={harnesses}
              onChange={(event) => setHarnesses(event.target.value)}
              aria-describedby={`${id}-harness-error`}
            />
          </label>
          <label htmlFor={`${id}-default-harness`} className="space-y-1 text-label font-medium">
            既定の harness
            <Input
              id={`${id}-default-harness`}
              value={defaultHarness}
              onChange={(event) => setDefaultHarness(event.target.value)}
              aria-describedby={`${id}-harness-error`}
            />
          </label>
          <label htmlFor={`${id}-attempts`} className="space-y-1 text-label font-medium">
            最大試行回数
            <Input
              id={`${id}-attempts`}
              type="number"
              min="1"
              step="1"
              value={attempts}
              onChange={(event) => setAttempts(event.target.value)}
              aria-describedby={`${id}-budget-error`}
            />
          </label>
          <label className="space-y-1 text-label font-medium">
            モデルの上限
            <select
              className={fieldClassName}
              value={lane}
              onChange={(event) => setLane(event.target.value)}
              aria-describedby={`${id}-budget-error`}
            >
              <option value="">指定なし</option>
              <option value="cheap">cheap</option>
              <option value="standard">standard</option>
              <option value="frontier">frontier</option>
            </select>
          </label>
        </div>
        <FieldError id={`${id}-harness-error`} message={errors.harnesses} />
        <FieldError id={`${id}-budget-error`} message={errors.budget} />
      </Section>
      <Section
        title="毎回の承認が要る操作"
        description="既定では click・download は許可 origin の中なら承認なしで実行し、記録だけ残します（人の決定 2026-10-08）。承認対象に戻す操作だけ選びます。ID・password を使う login（credential_use）は常に毎回承認です。"
      >
        <ul className="space-y-2" aria-label="毎回の承認が要る操作">
          {APPROVAL_CHOICES.map((choice) => {
            const checked = approvalActions.includes(choice.action);
            return (
              <li key={choice.action} className="flex min-w-0 items-start gap-3">
                <input
                  id={`${id}-approval-${choice.action}`}
                  type="checkbox"
                  className="mt-1 size-5 shrink-0 accent-primary"
                  checked={checked}
                  onChange={(event) =>
                    setApprovalActions(
                      event.target.checked
                        ? [...approvalActions, choice.action]
                        : approvalActions.filter((a) => a !== choice.action),
                    )
                  }
                  aria-describedby={`${id}-approval-${choice.action}-help ${id}-approval-error`}
                />
                <label htmlFor={`${id}-approval-${choice.action}`} className="min-w-0 space-y-1 text-label font-medium">
                  {choice.label}
                  <span id={`${id}-approval-${choice.action}-help`} className="block font-normal text-muted-foreground">
                    {choice.help}
                  </span>
                </label>
              </li>
            );
          })}
        </ul>
        <p className="mt-2 text-label text-muted-foreground">
          承認待ちは受信箱の「ブラウザの承認」に出ます（何を・どの origin で・何のために）。期限は 30
          分で、期限内に承認されなければその task は失敗として止まります。
        </p>
        <FieldError id={`${id}-approval-error`} message={errors.approval} />
      </Section>
      <Section title="credential と identity の対応" description="秘密の値は入力せず、登録済みの ID だけを指定します。">
        <div className="grid gap-4 sm:grid-cols-2">
          <label className="space-y-1 text-label font-medium">
            使える credential policy ID（1 行に 1 件）
            <textarea
              className={fieldClassName}
              rows={3}
              value={policies}
              onChange={(event) => setPolicies(event.target.value)}
              aria-describedby={`${id}-policy-error`}
            />
          </label>
          <label className="space-y-1 text-label font-medium">
            policy と identity の対応（policy ID=identity ID）
            <textarea
              className={fieldClassName}
              rows={3}
              value={identity}
              onChange={(event) => setIdentity(event.target.value)}
              aria-describedby={`${id}-identity-error`}
            />
          </label>
        </div>
        <FieldError id={`${id}-policy-error`} message={errors.credentials} />
        <FieldError id={`${id}-identity-error`} message={errors.identity} />
      </Section>
      <div className="flex flex-wrap items-center gap-3">
        <ConfirmDialog
          trigger={<Button>変更内容を確認して保存</Button>}
          title="ブラウザ実行課の設定を保存"
          target={`許可 origin: ${origins.join("、") || "なし"}。harness: ${list(harnesses).join("、") || "なし"}（既定 ${defaultHarness || "なし"}）。最大試行 ${attempts || "指定なし"}、モデル上限 ${lane || "指定なし"}。毎回の承認が要る操作: ${approvalActions.length ? approvalActions.join("、") : "なし（credential_use のみ）"}。credential policy ${lines(policies).length} 件、identity 対応 ${lines(identity).length} 件`}
          consequence="登録内容を置き換えます。許可 origin の縮小は既存 task の次の run にも反映されます。"
          reversibility="この画面で値を戻して再保存できます。"
          followUp="保存後の設定と更新時刻をこの画面で確認できます。"
          confirmLabel="設定を保存"
          onConfirm={save}
        />
      </div>
    </div>
  );
}

export function BrowserSettingsScreen() {
  const query = useQuery({ queryKey: orgKeys.list(), queryFn: ({ signal }) => apiGet<OrgList>("/api/org", signal) });
  const [saved, setSaved] = useState<OrgNode | null>(null);
  const [sessionEvent, setSessionEvent] = useState<{ at: string; actor: string } | null>(null);
  const node = saved ?? query.data?.items.find((item) => item.id === NODE_ID);
  return (
    <ScreenFrame
      title="ブラウザ実行課の設定"
      route="/browser/settings"
      breadcrumb={[{ label: "ブラウザ", link: { to: "/browser" } }, { label: "設定" }]}
      description="許可するサイトと実行条件を管理します。"
    >
      <div className="space-y-6">
        <Link to="/browser" className="inline-flex min-h-11 items-center text-link underline underline-offset-2">
          ブラウザ実行へ戻る
        </Link>
        <FetchFrame query={query} subject="ブラウザ実行課の設定">
          {node?.profile?.browser ? (
            <>
              <SettingsForm
                key={node.updated_at}
                node={node}
                onSaved={(updated) => {
                  setSaved(updated);
                  setSessionEvent({ at: updated.updated_at, actor: "admin" });
                }}
              />
              {sessionEvent ? (
                <p role="status" className="text-label text-success-foreground">
                  設定を保存しました。更新後の値を表示しています。
                </p>
              ) : null}
              <Section title="変更の記録" description="この画面で保存した変更を表示します。">
                <p className="text-label">
                  現在の設定の更新時刻: <time dateTime={node.updated_at}>{displayTime(node.updated_at)}</time>
                </p>
                {sessionEvent ? (
                  <p className="text-label">
                    変更者: {sessionEvent.actor} · 保存時刻:{" "}
                    <time dateTime={sessionEvent.at}>{displayTime(sessionEvent.at)}</time>
                  </p>
                ) : (
                  <p className="text-label text-muted-foreground">この画面での保存はまだありません。</p>
                )}
              </Section>
            </>
          ) : query.data ? (
            <p role="alert">browser grant を持つ実行課が見つかりません。</p>
          ) : null}
        </FetchFrame>
      </div>
    </ScreenFrame>
  );
}
