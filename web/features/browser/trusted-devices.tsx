import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import type { FormEvent } from "react";
import { useId, useState } from "react";
import { EmptyState, FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge, type BadgeTone } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { ConfirmDialog } from "../../components/ui/confirm-dialog";
import { DataList, type DataListItem } from "../../components/ui/data-list";
import { Input } from "../../components/ui/input";
import { Section } from "../../components/ui/panel";
import {
  BrowserGatewayError,
  type OwnerSession,
  ownerSessionQuery,
  registerTrustedDevice,
  revokeTrustedDevice,
  type TrustedDevice,
  trustedDevicesQuery,
} from "./browser-query";
import { formatIdentityExpiry } from "./identities-screen";
import { OwnerSessionNotice, ownerNoticeReason } from "./owner-session-notice";

// ADR 2026-10-07-browser-trusted-devices: 本人（owner）の端末を「信頼できる端末」として登録し、web・daemon の
// 再起動後も password login ＋ 端末 cookie で owner session を作り直す。画面は登録（名前を付ける）・一覧・失効だけ。
// 端末の秘密は httpOnly cookie にあり、画面にも JS にも出ない。

export type DeviceState = "active" | "revoked" | "expired";

/** `now`（UNIX 秒）の時点の状態。失効を期限切れより先に見る。 */
export function trustedDeviceState(device: TrustedDevice, now: number): DeviceState {
  if (device.revoked_at !== null) return "revoked";
  if (device.expires_at <= now) return "expired";
  if (device.absolute_expires_at !== null && device.absolute_expires_at <= now) return "expired";
  return "active";
}

const STATE_LABEL: Record<DeviceState, string> = { active: "有効", revoked: "失効済み", expired: "期限切れ" };
const STATE_TONE: Record<DeviceState, BadgeTone> = { active: "success", revoked: "neutral", expired: "warning" };

export function trustedDeviceStateLabel(device: TrustedDevice, now: number): string {
  const state = trustedDeviceState(device, now);
  if (state === "revoked" && device.revoked_reason === "reuse") return "失効済み（使い回しを検知）";
  return STATE_LABEL[state];
}

const DEVICE_ERROR_TEXT: Record<string, string> = {
  csrf_failed: "送信元を確認できませんでした。画面を読み込み直してください。",
  unauthenticated: "ログインが切れています。",
  owner_unavailable: "この構成では本人を識別できません。",
  not_owner: "この操作は本人のセッションでだけ行えます。",
  invalid_input: "名前は 1〜64 文字で入力してください。",
  device_invalid: "名前は 1〜64 文字で入力してください。",
  device_limit: "登録できる端末は上限に達しています。一覧から使わない端末を失効させてください。",
  device_not_found: "この端末は見つかりません。",
  probe_mode: "確認用の起動中は端末を扱えません。",
  attestation_unavailable: "web の署名鍵が設定されていません。",
  browser_action_pending: "前の操作がまだ終わっていません。少し待ってやり直してください。",
  celeris_unavailable: "Celeris に接続できません。",
};

/** 固定コードの文言だけを出す（daemon の応答本文は出さない）。 */
export function trustedDeviceErrorMessage(error: unknown): string {
  const code = error instanceof BrowserGatewayError ? error.code : "request_failed";
  return DEVICE_ERROR_TEXT[code] ?? "操作に失敗しました。";
}

/** 端末の名前の既定値（OS とブラウザの大まかな名前）。人が書き換えられる。 */
export function defaultDeviceName(userAgent: string): string {
  const os = /iPhone|iPad/.test(userAgent)
    ? "iPhone/iPad"
    : /Android/.test(userAgent)
      ? "Android"
      : /Mac OS X/.test(userAgent)
        ? "Mac"
        : /Windows/.test(userAgent)
          ? "Windows"
          : /Linux/.test(userAgent)
            ? "Linux"
            : "端末";
  const browser = /Edg\//.test(userAgent)
    ? "Edge"
    : /Firefox\//.test(userAgent)
      ? "Firefox"
      : /Chrome\//.test(userAgent)
        ? "Chrome"
        : /Safari\//.test(userAgent)
          ? "Safari"
          : "";
  return browser ? `${os} の ${browser}` : os;
}

type Outcome = { ok: boolean; message: string };

function OutcomeText({ outcome }: { outcome: Outcome | null }) {
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

/** 「この端末を信頼する」form。owner のときだけ置く。 */
export function TrustDeviceForm({ csrf }: { csrf: string }) {
  const id = useId();
  const [name, setName] = useState(() =>
    defaultDeviceName(typeof navigator === "undefined" ? "" : navigator.userAgent),
  );
  const [pending, setPending] = useState(false);
  const [outcome, setOutcome] = useState<Outcome | null>(null);

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (pending) return;
    const trimmed = name.trim();
    if (!trimmed || [...trimmed].length > 64) {
      setOutcome({ ok: false, message: DEVICE_ERROR_TEXT.invalid_input ?? "" });
      return;
    }
    setPending(true);
    setOutcome(null);
    try {
      await registerTrustedDevice(trimmed, csrf);
      setOutcome({ ok: true, message: "この端末を信頼できる端末として登録しました。" });
    } catch (error) {
      setOutcome({ ok: false, message: trustedDeviceErrorMessage(error) });
    } finally {
      setPending(false);
    }
  }

  return (
    <form onSubmit={handleSubmit} autoComplete="off" className="space-y-3" data-testid="browser-trust-device">
      <div className="space-y-1">
        <label htmlFor={`${id}-name`} className="block text-label text-foreground">
          端末の名前
        </label>
        <Input
          id={`${id}-name`}
          name="name"
          type="text"
          required
          maxLength={64}
          autoComplete="off"
          value={name}
          onChange={(event) => setName(event.target.value)}
        />
        <p className="text-label text-muted-foreground">
          登録すると、web や daemon
          の再起動の後も、この端末でログインし直すだけで本人として操作できます。最後に使ってから 90 日で失効します。
        </p>
      </div>
      <Button type="submit" size="sm" disabled={pending}>
        この端末を信頼する
      </Button>
      <OutcomeText outcome={outcome} />
    </form>
  );
}

/**
 * owner の画面に出す信頼端末の案内。CLI 承認で owner になり、まだ端末 cookie が無いときは登録 form を、
 * 登録済みの端末なら短い表示と一覧への link を出す。owner でなければ何も出さない（OwnerSessionNotice が出る）。
 */
/** 案内の出し方。form = CLI 承認の owner で端末 cookie が無い、trusted = 登録済みの端末、null = owner でない。 */
export function trustDeviceMode(owner: OwnerSession | undefined): "form" | "trusted" | null {
  if (!owner?.isOwner || !owner.csrfToken) return null;
  return owner.deviceId || owner.trustedDevice === true ? "trusted" : "form";
}

export function TrustedDeviceOffer({ owner }: { owner: OwnerSession | undefined }) {
  const mode = trustDeviceMode(owner);
  if (!owner?.csrfToken || mode === null) return null;
  const trusted = mode === "trusted";
  return (
    <Section
      title={trusted ? "信頼できる端末" : "この端末を信頼する"}
      description={
        trusted
          ? "この端末は登録済みです。再起動の後もログインし直すだけで本人として使えます。"
          : "本人として承認したこの端末を登録すると、次からは本人確認のコマンドが要りません。"
      }
      data-testid="browser-trusted-device-offer"
    >
      {trusted ? null : <TrustDeviceForm csrf={owner.csrfToken} />}
      <Link to="/browser/devices" className="inline-flex min-h-11 items-center text-link underline underline-offset-2">
        登録した端末の一覧
      </Link>
    </Section>
  );
}

function DeviceCard({
  device,
  now,
  current,
  csrf,
}: {
  device: TrustedDevice;
  now: number;
  current: boolean;
  csrf: string;
}) {
  const state = trustedDeviceState(device, now);
  const items: DataListItem[] = [
    { key: "created", label: "登録日時", value: formatIdentityExpiry(device.created_at) },
    {
      key: "used",
      label: "最終使用",
      value: device.last_used_at === null ? "未使用" : formatIdentityExpiry(device.last_used_at),
    },
    { key: "expires", label: "期限", value: formatIdentityExpiry(device.expires_at) },
    {
      key: "state",
      label: "状態",
      value: <Badge tone={STATE_TONE[state]}>{trustedDeviceStateLabel(device, now)}</Badge>,
    },
  ];
  return (
    <div
      className="space-y-2 border-b border-border py-3 last:border-b-0"
      data-testid="browser-trusted-device"
      data-device-id={device.id}
      data-device-state={state}
    >
      <p className="flex flex-wrap items-center gap-2 font-semibold">
        <span className="break-all">{device.name}</span>
        {current ? <Badge tone="info">この端末</Badge> : null}
      </p>
      <DataList items={items} />
      {state === "active" ? (
        <ConfirmDialog
          trigger={
            <Button type="button" variant="destructive" size="sm" aria-label={`${device.name} を失効`}>
              失効
            </Button>
          }
          title="この端末を失効させますか"
          target={device.name}
          consequence="この端末からの自動復帰がすぐに使えなくなります。この端末で作った本人のセッションも終わります。"
          reversibility="失効は元に戻せません。使い続けるには、本人確認のコマンドで承認してから登録し直してください。"
          followUp="一覧の状態が「失効済み」になったことで確認できます。"
          confirmLabel={`${device.name} を失効`}
          onConfirm={async () => {
            try {
              await revokeTrustedDevice(device.id, csrf);
            } catch (error) {
              throw new Error(trustedDeviceErrorMessage(error));
            }
          }}
        />
      ) : null}
    </div>
  );
}

/** 一覧の本体。react-query に依存しないので試験できる。 */
export function TrustedDevicesPanel({
  devices,
  now,
  limit,
  currentDeviceId,
  csrf,
}: {
  devices: readonly TrustedDevice[];
  now: number;
  limit: number;
  currentDeviceId: string | null;
  csrf: string;
}) {
  const active = devices.filter((device) => trustedDeviceState(device, now) === "active").length;
  return (
    <Section
      title="登録した端末"
      description={`有効な端末 ${active} 台（上限 ${limit} 台）。最後に使ってから 90 日で期限が切れます。`}
    >
      {devices.length === 0 ? (
        <EmptyState message="登録した端末はありません。" />
      ) : (
        <div className="flex flex-col" data-testid="browser-trusted-device-list">
          {devices.map((device) => (
            <DeviceCard key={device.id} device={device} now={now} current={device.id === currentDeviceId} csrf={csrf} />
          ))}
        </div>
      )}
    </Section>
  );
}

function OwnerDevices({ owner }: { owner: OwnerSession }) {
  const devices = useQuery(trustedDevicesQuery());
  return (
    <div className="space-y-6">
      <TrustedDeviceOffer owner={owner} />
      <FetchFrame query={devices} subject="登録した端末">
        {devices.data ? (
          <TrustedDevicesPanel
            devices={devices.data.devices}
            now={devices.data.now}
            limit={devices.data.limit}
            currentDeviceId={devices.data.currentDeviceId ?? owner.deviceId ?? null}
            csrf={owner.csrfToken ?? ""}
          />
        ) : null}
      </FetchFrame>
    </div>
  );
}

export function TrustedDevicesScreen() {
  const owner = useQuery(ownerSessionQuery());
  return (
    <ScreenFrame
      title="信頼できる端末"
      route="/browser/devices"
      breadcrumb={[{ label: "ブラウザ", link: { to: "/browser" } }, { label: "信頼できる端末" }]}
      description="本人として使う端末の登録・一覧・失効を行います。"
    >
      <FetchFrame query={owner} subject="本人確認">
        {owner.data ? (
          ownerNoticeReason(owner.data) !== null ? (
            <OwnerSessionNotice owner={owner.data} />
          ) : (
            <OwnerDevices owner={owner.data} />
          )
        ) : null}
      </FetchFrame>
    </ScreenFrame>
  );
}
