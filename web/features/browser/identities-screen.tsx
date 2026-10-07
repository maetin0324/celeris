import { useQuery } from "@tanstack/react-query";
import type { CSSProperties, FormEvent } from "react";
import { useId, useState } from "react";
import { EmptyState, FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge, type BadgeTone } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { ConfirmDialog } from "../../components/ui/confirm-dialog";
import { DataList, type DataListItem } from "../../components/ui/data-list";
import { fieldClassName, Input } from "../../components/ui/input";
import { Section } from "../../components/ui/panel";
import { cn } from "../../lib/utils";
import {
  BrowserGatewayError,
  type BrowserIdentity,
  browserIdentitiesQuery,
  createBrowserIdentity,
  deleteBrowserIdentity,
  ownerSessionQuery,
  restoreBrowserIdentity,
  revokeBrowserIdentity,
} from "./browser-query";
import { OwnerSessionNotice, ownerNoticeReason } from "./owner-session-notice";

// D3.4: identity は project ごとに origin・id・generation・期限・状態を出し、active にだけ失効・復元を付ける。
// 全件に削除（ConfirmDialog）。登録は state JSON を password 型の textarea 相当で受け、送信後は必ず空にする —
// 検証の失敗・送信の失敗のいずれでも画面に残さない（credential form と同じ扱い）。

export function identityStateLabel(state: string): string {
  if (state === "active") return "有効";
  if (state === "revoked") return "失効済み";
  return state;
}

export function identityStateTone(state: string): BadgeTone {
  return state === "active" ? "success" : "neutral";
}

export function canRevokeIdentity(identity: Pick<BrowserIdentity, "state">): boolean {
  return identity.state === "active";
}

export function canRestoreIdentity(identity: Pick<BrowserIdentity, "state">): boolean {
  return identity.state === "active";
}

/** epoch 秒・ISO 文字列のどちらでも timezone に依らず同じ文字列にする（formatDeadline と同じ書式）。 */
export function formatIdentityExpiry(value: unknown): string {
  const ms = typeof value === "number" ? value * 1000 : typeof value === "string" ? Date.parse(value) : Number.NaN;
  if (!Number.isFinite(ms)) return String(value);
  return new Date(ms)
    .toISOString()
    .replace("T", " ")
    .replace(/\.\d+Z$/, "Z");
}

export type IdentityStateJson = { entries: unknown[] };
export type ParsedIdentityState = { ok: true; state: IdentityStateJson } | { ok: false; error: string };

/** gateway と同じ条件（entries 配列を持つ object）を手元で確かめ、無駄な送信をしない。 */
export function parseIdentityStateJson(text: string): ParsedIdentityState {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return { ok: false, error: "JSON の形式が正しくありません。" };
  }
  if (typeof parsed !== "object" || parsed === null || !Array.isArray((parsed as { entries?: unknown }).entries)) {
    return { ok: false, error: "entries の配列を含む JSON にしてください。" };
  }
  return { ok: true, state: parsed as IdentityStateJson };
}

const IDENTITY_ERROR_TEXT: Record<string, string> = {
  csrf_failed: "送信元を確認できませんでした。画面を読み込み直してください。",
  unauthenticated: "ログインが切れています。",
  owner_unavailable: "この構成では本人を識別できません。",
  not_owner: "この操作は本人のセッションでだけ行えます。",
  invalid_input: "入力を確認してください。",
  identity_exists: "同じ id の本人情報が既にあります。",
  identity_not_found: "この本人情報は見つかりません。",
  identity_not_active: "有効な本人情報だけ失効・復元できます。",
  identity_not_restorable: "いまは復元できません。状態を確認してください。",
  payload_too_large: "入力が大きすぎます。",
  browser_action_pending: "前の操作がまだ終わっていません。少し待ってやり直してください。",
  celeris_unavailable: "Celeris に接続できません。",
  request_failed: "操作に失敗しました。",
};

/** 固定コードの文言だけを出す（celeris の応答本文は出さない）。 */
export function identityErrorMessage(error: unknown): string {
  const code = error instanceof BrowserGatewayError ? error.code : "request_failed";
  return IDENTITY_ERROR_TEXT[code] ?? "操作に失敗しました。";
}

type ActionOutcome = { ok: boolean; message: string };

function OutcomeText({ outcome }: { outcome: ActionOutcome | null }) {
  if (!outcome) return null;
  return (
    <p
      role={outcome.ok ? "status" : "alert"}
      className={outcome.ok ? "text-success-foreground" : "text-danger-foreground"}
    >
      {outcome.message}
    </p>
  );
}

function IdentityRegisterForm({ projectId, csrf }: { projectId: string; csrf: string }) {
  const idBase = useId();
  const [identityId, setIdentityId] = useState("");
  const [origin, setOrigin] = useState("");
  const [confirmedBy, setConfirmedBy] = useState("");
  const [stateText, setStateText] = useState("");
  const [pending, setPending] = useState(false);
  const [outcome, setOutcome] = useState<ActionOutcome | null>(null);

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (pending) return;
    const parsed = parseIdentityStateJson(stateText);
    // 検証に失敗しても必ず空にする。再入力は求めるが、画面には残さない。
    setStateText("");
    if (!parsed.ok) {
      setOutcome({ ok: false, message: parsed.error });
      return;
    }
    setPending(true);
    try {
      await createBrowserIdentity(
        {
          identity_id: identityId,
          project_id: projectId,
          origin,
          demand_confirmed_by: confirmedBy,
          state: parsed.state,
        },
        csrf,
      );
      setIdentityId("");
      setOrigin("");
      setConfirmedBy("");
      setOutcome({ ok: true, message: "登録しました。使う前に本人の復元操作が要ります。" });
    } catch (error) {
      setOutcome({ ok: false, message: identityErrorMessage(error) });
    } finally {
      setPending(false);
    }
  }

  return (
    <form onSubmit={handleSubmit} autoComplete="off" className="space-y-3" data-testid="browser-identity-register">
      <div className="space-y-1">
        <label htmlFor={`${idBase}-id`} className="block text-label text-foreground">
          id
        </label>
        <Input
          id={`${idBase}-id`}
          name="identity_id"
          type="text"
          required
          pattern="[0-9A-Za-z_-]{1,64}"
          maxLength={64}
          autoComplete="off"
          value={identityId}
          onChange={(event) => setIdentityId(event.target.value)}
        />
      </div>
      <div className="space-y-1">
        <label htmlFor={`${idBase}-origin`} className="block text-label text-foreground">
          サイト（origin）
        </label>
        <Input
          id={`${idBase}-origin`}
          name="origin"
          type="text"
          required
          placeholder="https://example.com"
          autoComplete="off"
          value={origin}
          onChange={(event) => setOrigin(event.target.value)}
        />
      </div>
      <div className="space-y-1">
        <label htmlFor={`${idBase}-confirmed`} className="block text-label text-foreground">
          確認した人
        </label>
        <Input
          id={`${idBase}-confirmed`}
          name="demand_confirmed_by"
          type="text"
          required
          placeholder="human:あなたの id"
          autoComplete="off"
          value={confirmedBy}
          onChange={(event) => setConfirmedBy(event.target.value)}
        />
        <p className="text-label text-muted-foreground">
          この site が認証を求めたことを確認した人の参照。自動では作れません。
        </p>
      </div>
      <div className="space-y-1">
        <label htmlFor={`${idBase}-state`} className="block text-label text-foreground">
          state JSON
        </label>
        <textarea
          id={`${idBase}-state`}
          name="state"
          required
          rows={4}
          autoComplete="off"
          autoCapitalize="none"
          autoCorrect="off"
          spellCheck={false}
          data-1p-ignore=""
          data-lpignore="true"
          className={cn(fieldClassName, "font-mono")}
          style={{ WebkitTextSecurity: "disc" } as CSSProperties}
          value={stateText}
          onChange={(event) => setStateText(event.target.value)}
        />
        <p className="text-label text-muted-foreground">
          <code className="break-all font-mono">
            {'{"entries":[{"origin":"https://...","kind":"cookie","name":"...","value":"..."}]}'}
          </code>{" "}
          の形式。7 日で失効し、送信後は画面に残りません。
        </p>
      </div>
      <Button type="submit" size="sm" disabled={pending}>
        登録する
      </Button>
      <OutcomeText outcome={outcome} />
    </form>
  );
}

function IdentityCard({ identity, projectId, csrf }: { identity: BrowserIdentity; projectId: string; csrf: string }) {
  const [pending, setPending] = useState(false);
  const [outcome, setOutcome] = useState<ActionOutcome | null>(null);

  async function act(send: () => Promise<unknown>, successMessage: string) {
    if (pending) return;
    setPending(true);
    setOutcome(null);
    try {
      await send();
      setOutcome({ ok: true, message: successMessage });
    } catch (error) {
      setOutcome({ ok: false, message: identityErrorMessage(error) });
    } finally {
      setPending(false);
    }
  }

  const items: DataListItem[] = [
    { key: "origin", label: "サイト", value: <span className="break-all font-mono">{identity.origin}</span> },
    { key: "id", label: "id", value: <span className="break-all font-mono">{identity.identity_id}</span> },
    { key: "generation", label: "generation", value: identity.generation },
    { key: "expires", label: "期限", value: formatIdentityExpiry(identity.expires_at) },
    {
      key: "state",
      label: "状態",
      value: <Badge tone={identityStateTone(identity.state)}>{identityStateLabel(identity.state)}</Badge>,
    },
  ];

  return (
    <div
      className="space-y-2 border-b border-border py-3 last:border-b-0"
      data-testid="browser-identity"
      data-identity-id={identity.identity_id}
      data-identity-state={identity.state}
    >
      <DataList items={items} />
      <div className="flex flex-wrap gap-2">
        {canRevokeIdentity(identity) ? (
          <Button
            type="button"
            variant="secondary"
            size="sm"
            disabled={pending}
            aria-label={`${identity.identity_id} を失効`}
            onClick={() =>
              void act(() => revokeBrowserIdentity(identity.identity_id, projectId, csrf), "失効しました。")
            }
          >
            失効
          </Button>
        ) : null}
        {canRestoreIdentity(identity) ? (
          <Button
            type="button"
            variant="secondary"
            size="sm"
            disabled={pending}
            aria-label={`${identity.identity_id} を復元`}
            onClick={() =>
              void act(
                () => restoreBrowserIdentity(identity.identity_id, projectId, identity.origin, csrf),
                "復元しました。",
              )
            }
          >
            復元
          </Button>
        ) : null}
        <ConfirmDialog
          trigger={
            <Button
              type="button"
              variant="destructive"
              size="sm"
              disabled={pending}
              aria-label={`${identity.identity_id} を削除`}
            >
              削除
            </Button>
          }
          title="本人情報を削除しますか"
          target={`${identity.origin}（${identity.identity_id}）`}
          consequence="この本人情報が使えなくなります。登録し直すまでこの site の自動操作はできません。"
          reversibility="削除は元に戻せません。必要なら登録をやり直してください。"
          followUp="一覧から消えたことで確認できます。"
          confirmLabel={`${identity.identity_id} を削除`}
          onConfirm={async () => {
            try {
              await deleteBrowserIdentity(identity.identity_id, projectId, csrf);
            } catch (error) {
              throw new Error(identityErrorMessage(error));
            }
          }}
        />
      </div>
      <OutcomeText outcome={outcome} />
    </div>
  );
}

/** owner 判定を済ませた後の本体（登録 form と一覧）。react-query に依存しないので試験できる。 */
export function BrowserIdentitiesPanel({
  identities,
  projectId,
  csrf,
}: {
  identities: readonly BrowserIdentity[];
  projectId: string;
  csrf: string;
}) {
  return (
    <div className="space-y-6">
      <Section title="本人情報の登録" description="site が求める認証状態を、人が確認したうえで一度だけ登録します。">
        <IdentityRegisterForm projectId={projectId} csrf={csrf} />
      </Section>
      <Section title="登録済みの本人情報">
        {identities.length === 0 ? (
          <EmptyState message="登録された本人情報はありません。" />
        ) : (
          <div className="flex flex-col" data-testid="browser-identity-list">
            {identities.map((identity) => (
              <IdentityCard key={identity.identity_id} identity={identity} projectId={projectId} csrf={csrf} />
            ))}
          </div>
        )}
      </Section>
    </div>
  );
}

export function BrowserIdentitiesScreen({ projectId }: { projectId: string }) {
  const owner = useQuery(ownerSessionQuery());
  const gated = owner.data === undefined || ownerNoticeReason(owner.data) !== null;
  const identities = useQuery({ ...browserIdentitiesQuery(projectId), enabled: !gated });
  return (
    <ScreenFrame
      title="ブラウザの本人情報"
      route="/projects/:id/browser-identities"
      breadcrumb={[
        { label: "案件", link: { to: "/projects" } },
        { label: "案件詳細", link: { to: "/projects/$id", params: { id: projectId } } },
        { label: "本人情報" },
      ]}
    >
      <FetchFrame query={owner} subject="本人確認">
        {owner.data ? (
          ownerNoticeReason(owner.data) !== null ? (
            <OwnerSessionNotice owner={owner.data} />
          ) : (
            <FetchFrame query={identities} subject="本人情報">
              {identities.data ? (
                <BrowserIdentitiesPanel
                  identities={identities.data.identities}
                  projectId={projectId}
                  csrf={owner.data.csrfToken ?? ""}
                />
              ) : null}
            </FetchFrame>
          )
        ) : null}
      </FetchFrame>
    </ScreenFrame>
  );
}
