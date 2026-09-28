import { type ReactNode, useState } from "react";
import { MarkdownViewer } from "~/components/MarkdownViewer";
import { Badge } from "~/components/ui/badge";
import { buttonClass } from "~/components/ui/button";
import { Icon, type IconName } from "~/components/ui/Icon";
import { TONE_ICON_WRAP, type Tone } from "~/components/ui/tone";
import { type FileDiff, prettyRaw, type RunLogEvent, type RunLogEventKind } from "~/lib/run-log";
import { cn } from "~/lib/utils";

/**
 * run ログの会話形式の表示（docs/adr/0013-run-log-conversation-view.md D2）。harness を知らない:
 * 入力は `~/lib/run-log.ts` の `RunLogEvent[]` だけ。長い本文は既定で畳み、開いたときだけ描く。
 * 各イベントに「JSON」（元の行）と「コピー」を付ける。LLM の出力は信用しない（DESIGN §8.3）ので、
 * 本文は Markdown（生の HTML はエスケープ）か `<pre>` のテキストとしてだけ出す。
 */

export const KIND_LABEL: Record<RunLogEventKind, string> = {
  message: "発言",
  thinking: "思考",
  tool: "ツール",
  command: "コマンド",
  file_change: "ファイル変更",
  error: "エラー",
  usage: "使用量",
  system: "システム",
  unknown: "未対応の形式",
};

const KIND_ICON: Record<RunLogEventKind, IconName> = {
  message: "message",
  thinking: "sparkles",
  tool: "settings",
  command: "terminal",
  file_change: "file",
  error: "alert",
  usage: "activity",
  system: "info",
  unknown: "help",
};

const KIND_TONE: Record<RunLogEventKind, Tone> = {
  message: "primary",
  thinking: "info",
  tool: "teal",
  command: "neutral",
  file_change: "warning",
  error: "danger",
  usage: "success",
  system: "neutral",
  unknown: "neutral",
};

/** これより長い本文は既定で畳む。 */
const FOLD_LINES = 8;
const FOLD_CHARS = 600;

function isLong(text: string): boolean {
  return text.length > FOLD_CHARS || text.split("\n").length > FOLD_LINES;
}

function lineCount(text: string): number {
  return text.length === 0 ? 0 : text.split("\n").length;
}

const preClass =
  "max-h-[32rem] overflow-auto rounded-lg border border-border bg-surface-2 p-3 font-mono text-xs leading-relaxed text-fg whitespace-pre-wrap [overflow-wrap:anywhere]";

/** 開いたときだけ中身を描く折りたたみ（`<details>` は閉じていても子を描くので使わない）。 */
function Fold({
  label,
  defaultOpen = false,
  children,
  testId,
}: {
  label: ReactNode;
  defaultOpen?: boolean;
  children: () => ReactNode;
  testId?: string;
}) {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <div data-testid={testId}>
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className="inline-flex min-h-8 items-center gap-1 rounded-md py-1 text-sm font-medium text-fg-muted hover:text-fg"
        data-testid="event-fold-toggle"
      >
        <Icon name={open ? "chevronDown" : "chevronRight"} />
        {label}
      </button>
      {open && <div className="mt-1.5">{children()}</div>}
    </div>
  );
}

/** 長ければ畳む。短ければそのまま出す。 */
function TextBlock({ label, text, tone }: { label: string; text: string; tone?: "danger" }) {
  const body = () => (
    <pre className={cn(preClass, tone === "danger" && "border-danger-border text-danger-soft-fg")}>{text}</pre>
  );
  if (text.length === 0) return <p className="text-sm text-fg-subtle">{label}: （出力なし）</p>;
  if (!isLong(text)) {
    return (
      <div>
        <p className="mb-1 text-xs font-medium text-fg-subtle">{label}</p>
        {body()}
      </div>
    );
  }
  return <Fold label={`${label}（${lineCount(text)} 行）`}>{body}</Fold>;
}

function DiffView({ diffs }: { diffs: FileDiff[] }) {
  const total = diffs.reduce((n, d) => n + d.hunks.reduce((m, h) => m + h.lines.length, 0), 0);
  const body = () => (
    <div className="space-y-2">
      {diffs.map((d, i) => (
        // biome-ignore lint/suspicious/noArrayIndexKey: 同じパスが並ぶことがあり、並びは変わらない
        <div key={i} className="overflow-hidden rounded-lg border border-border" data-testid="event-diff">
          <p className="flex flex-wrap items-center gap-2 border-b border-border bg-surface-2 px-3 py-1.5 font-mono text-xs text-fg-muted">
            {d.kind && (
              <Badge tone={d.kind === "delete" ? "danger" : d.kind === "add" ? "success" : "warning"}>{d.kind}</Badge>
            )}
            <span className="min-w-0 [overflow-wrap:anywhere]">{d.path || "（パス不明）"}</span>
          </p>
          {d.hunks.length > 0 && (
            <pre className="max-h-[32rem] overflow-auto bg-surface font-mono text-xs leading-relaxed">
              {d.hunks.map((h, hi) => (
                // biome-ignore lint/suspicious/noArrayIndexKey: hunk は並び替えない
                <span key={hi} className="block">
                  {h.header && <span className="block bg-info-soft px-3 text-info-soft-fg">{h.header}</span>}
                  {h.lines.map((l, li) => (
                    <span
                      // biome-ignore lint/suspicious/noArrayIndexKey: 行は並び替えない
                      key={li}
                      className={cn(
                        "block min-w-max px-3 whitespace-pre",
                        l.startsWith("+") && "bg-success-soft text-success-soft-fg",
                        l.startsWith("-") && "bg-danger-soft text-danger-soft-fg",
                      )}
                    >
                      {l || " "}
                    </span>
                  ))}
                </span>
              ))}
            </pre>
          )}
        </div>
      ))}
    </div>
  );
  if (total === 0) return body();
  return (
    <Fold label={`差分（${diffs.length} ファイル・${total} 行）`} defaultOpen={total <= 24}>
      {body}
    </Fold>
  );
}

function JsonInput({ input }: { input: unknown }) {
  if (input === undefined) return null;
  const text = JSON.stringify(input, null, 2);
  if (text === "{}" || text === undefined) return null;
  return <Fold label="入力">{() => <pre className={preClass}>{text}</pre>}</Fold>;
}

function StatusBadge({ pending, isError, done = "完了" }: { pending?: boolean; isError?: boolean; done?: string }) {
  if (isError) return <Badge tone="danger">エラー</Badge>;
  if (pending)
    return (
      <Badge tone="primary" dot pulse>
        実行中
      </Badge>
    );
  return <Badge tone="success">{done}</Badge>;
}

function EventTitle({ ev }: { ev: RunLogEvent }): ReactNode {
  switch (ev.kind) {
    case "message":
      return ev.role === "user" ? "入力" : "発言";
    case "thinking":
      return ev.text.length === 0 ? "思考（本文は記録されていない）" : "思考";
    case "tool":
      return <span className="font-mono font-semibold">{ev.name}</span>;
    case "command":
      return "コマンド";
    case "file_change":
      return `ファイル変更（${ev.diffs.length} 件）`;
    case "error":
      return "エラー";
    case "usage":
      return ev.title;
    case "system":
      return ev.label;
    case "unknown":
      return `未対応の形式: ${ev.label}`;
  }
}

function EventBody({ ev }: { ev: RunLogEvent }): ReactNode {
  switch (ev.kind) {
    case "message":
      return ev.text.length > 0 ? <MarkdownViewer content={ev.text} bare /> : null;
    case "thinking":
      if (ev.text.length === 0) return null;
      return (
        <Fold label={`思考を表示（${lineCount(ev.text)} 行）`}>
          {() => <p className="whitespace-pre-wrap text-sm italic text-fg-muted [overflow-wrap:anywhere]">{ev.text}</p>}
        </Fold>
      );
    case "tool":
      return (
        <div className="space-y-1.5">
          {ev.diffs && ev.diffs.length > 0 && <DiffView diffs={ev.diffs} />}
          <JsonInput input={ev.input} />
          {ev.result !== undefined && (
            <TextBlock label="結果" text={ev.result} tone={ev.isError ? "danger" : undefined} />
          )}
        </div>
      );
    case "command":
      return (
        <div className="space-y-1.5">
          <pre className={cn(preClass, "max-h-40")}>
            <span className="select-none text-fg-subtle">$ </span>
            {ev.command}
          </pre>
          {ev.output !== undefined && (ev.output.length > 0 || ev.status === "completed" || ev.status === "failed") && (
            <TextBlock label="出力" text={ev.output} />
          )}
        </div>
      );
    case "file_change":
      return <DiffView diffs={ev.diffs} />;
    case "error":
      return (
        <div className="space-y-1.5">
          <p className="whitespace-pre-wrap text-sm text-danger-soft-fg [overflow-wrap:anywhere]">{ev.text}</p>
          {ev.detail && <TextBlock label="詳細" text={ev.detail} />}
        </div>
      );
    case "usage":
      return (
        <div className="space-y-2">
          {ev.stats.length > 0 && (
            <dl
              className="grid grid-cols-2 gap-x-4 gap-y-1.5 sm:grid-cols-3 lg:grid-cols-4"
              data-testid="event-usage-stats"
            >
              {ev.stats.map((s) => (
                <div key={s.label} className="min-w-0">
                  <dt className="text-xs text-fg-subtle">{s.label}</dt>
                  <dd className="font-mono text-sm text-fg [overflow-wrap:anywhere]">{s.value}</dd>
                </div>
              ))}
            </dl>
          )}
          {ev.text && <Fold label="最終の報告を表示">{() => <MarkdownViewer content={ev.text ?? ""} bare />}</Fold>}
        </div>
      );
    case "system":
    case "unknown":
      return null;
  }
}

function EventBadge({ ev }: { ev: RunLogEvent }): ReactNode {
  switch (ev.kind) {
    case "tool":
      return <StatusBadge pending={ev.pending} isError={ev.isError} />;
    case "command":
      if (ev.exitCode === null || ev.exitCode === undefined) {
        return ev.status === "in_progress" ? <StatusBadge pending /> : null;
      }
      return <Badge tone={ev.exitCode === 0 ? "success" : "danger"}>exit {ev.exitCode}</Badge>;
    case "file_change":
      return ev.status ? <Badge tone={ev.status === "failed" ? "danger" : "neutral"}>{ev.status}</Badge> : null;
    case "usage":
      return ev.isError ? <Badge tone="danger">エラー</Badge> : null;
    default:
      return null;
  }
}

/** 元の JSON の表示とコピー（イベントごと）。 */
function RawToggle({ raw, open, onToggle }: { raw: string[]; open: boolean; onToggle: () => void }) {
  const [copied, setCopied] = useState(false);
  async function copy() {
    try {
      await navigator.clipboard.writeText(raw.join("\n"));
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // Clipboard API が無い・許可が無い等。何もしない（`CopyButton` と同じ）。
    }
  }
  // スマホでは文字を隠してアイコンだけにし、見出しの行に収める（名前は aria-label で読める）。
  return (
    <span className="flex shrink-0 items-center">
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={open}
        aria-label={open ? "元の JSON を閉じる" : "元の JSON を表示"}
        title="元の JSON"
        className={buttonClass({ variant: "ghost", size: "xs" })}
        data-testid="event-json-toggle"
      >
        <Icon name="code" />
        <span className="hidden sm:inline">JSON{raw.length > 1 ? ` ${raw.length}` : ""}</span>
      </button>
      <button
        type="button"
        onClick={copy}
        aria-label={copied ? "コピーしました" : "元の JSON をコピー"}
        title="元の JSON をコピー"
        className={buttonClass({ variant: "ghost", size: "xs" })}
        data-testid="event-json-copy"
      >
        <Icon name={copied ? "check" : "copy"} />
        <span className="hidden sm:inline">{copied ? "コピーしました" : "コピー"}</span>
      </button>
    </span>
  );
}

function CompactEvent({ ev, index }: { ev: Extract<RunLogEvent, { kind: "system" | "unknown" }>; index: number }) {
  const [rawOpen, setRawOpen] = useState(false);
  const preview = ev.kind === "unknown" ? ev.raw[0] : ev.detail;
  return (
    <li data-testid="run-log-event" data-event-kind={ev.kind} data-event-index={index} className="px-1">
      <div className="flex min-w-0 items-center gap-x-2 text-sm text-fg-subtle">
        <Icon name={KIND_ICON[ev.kind]} className="size-3.5" />
        <span
          className={cn(
            "max-w-[60%] shrink-0 truncate font-medium",
            ev.kind === "unknown" ? "text-warning-soft-fg" : "text-fg-muted",
          )}
        >
          <EventTitle ev={ev} />
        </span>
        {ev.kind === "system" && ev.count > 1 && <span className="font-mono text-xs">×{ev.count}</span>}
        {preview && (
          <span className="min-w-0 flex-1 truncate font-mono text-xs" title={preview}>
            {preview}
          </span>
        )}
        <span className="ml-auto shrink-0">
          <RawToggle raw={ev.raw} open={rawOpen} onToggle={() => setRawOpen((v) => !v)} />
        </span>
      </div>
      {rawOpen && (
        <pre className={cn(preClass, "mt-1.5")} data-testid="event-raw-json">
          {prettyRaw(ev.raw)}
        </pre>
      )}
    </li>
  );
}

function CardEvent({ ev, index }: { ev: Exclude<RunLogEvent, { kind: "system" | "unknown" }>; index: number }) {
  const [rawOpen, setRawOpen] = useState(false);
  const tone = ev.kind === "tool" && ev.isError ? "danger" : KIND_TONE[ev.kind];
  const body = <EventBody ev={ev} />;
  const isUser = ev.kind === "message" && ev.role === "user";
  return (
    <li data-testid="run-log-event" data-event-kind={ev.kind} data-event-index={index} className="flex min-w-0 gap-2.5">
      <span className={cn("mt-0.5 hidden size-7 shrink-0 place-items-center rounded-lg sm:grid", TONE_ICON_WRAP[tone])}>
        <Icon name={KIND_ICON[ev.kind]} className="size-3.5" />
      </span>
      <div
        className={cn(
          "min-w-0 flex-1 rounded-xl border px-3 py-2.5",
          ev.kind === "error" ? "border-danger-border bg-danger-soft" : "border-border bg-surface",
          isUser && "bg-surface-2",
          ev.kind === "thinking" && "border-dashed",
        )}
      >
        <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
          <span className={cn("grid size-6 shrink-0 place-items-center rounded-md sm:hidden", TONE_ICON_WRAP[tone])}>
            <Icon name={KIND_ICON[ev.kind]} className="size-3.5" />
          </span>
          {ev.kind === "tool" && <span className="text-xs font-medium text-fg-subtle">{KIND_LABEL.tool}</span>}
          <span className="min-w-0 flex-1 text-sm font-medium text-fg [overflow-wrap:anywhere]">
            <EventTitle ev={ev} />
          </span>
          <EventBadge ev={ev} />
          <RawToggle raw={ev.raw} open={rawOpen} onToggle={() => setRawOpen((v) => !v)} />
        </div>
        {ev.kind === "tool" && ev.summary && (
          <p
            className="mt-1 line-clamp-3 font-mono text-xs text-fg-muted [overflow-wrap:anywhere]"
            title={ev.summary}
            data-testid="event-tool-summary"
          >
            {ev.summary}
          </p>
        )}
        {body && <div className="mt-2 min-w-0">{body}</div>}
        {rawOpen && (
          <pre className={cn(preClass, "mt-2")} data-testid="event-raw-json">
            {prettyRaw(ev.raw)}
          </pre>
        )}
      </div>
    </li>
  );
}

export function RunLog({ events }: { events: RunLogEvent[] }) {
  if (events.length === 0) {
    return <p className="text-sm text-fg-subtle">まだ出力がありません。</p>;
  }
  return (
    <ol className="space-y-2.5" data-testid="run-log">
      {events.map((ev, i) =>
        ev.kind === "system" || ev.kind === "unknown" ? (
          // biome-ignore lint/suspicious/noArrayIndexKey: イベントは追尾で末尾に足されるだけで並び替えない
          <CompactEvent key={i} ev={ev} index={i} />
        ) : (
          // biome-ignore lint/suspicious/noArrayIndexKey: 同上
          <CardEvent key={i} ev={ev} index={i} />
        ),
      )}
    </ol>
  );
}

/** 見出しに出す種類ごとの件数（色だけに頼らず文字で出す）。 */
export function RunLogKindCounts({ counts }: { counts: Partial<Record<RunLogEventKind, number>> }) {
  const order: RunLogEventKind[] = [
    "message",
    "thinking",
    "tool",
    "command",
    "file_change",
    "error",
    "usage",
    "system",
    "unknown",
  ];
  return (
    <ul className="flex flex-wrap gap-1.5" data-testid="run-log-counts" aria-label="種類ごとの件数">
      {order
        .filter((k) => (counts[k] ?? 0) > 0)
        .map((k) => (
          <li key={k}>
            <Badge tone={k === "error" ? "danger" : k === "unknown" ? "warning" : "neutral"}>
              {KIND_LABEL[k]} {counts[k]}
            </Badge>
          </li>
        ))}
    </ul>
  );
}
