import { useState } from "react";
import { cn } from "../../lib/utils";
import { Icon } from "./icon";

/** 長い ID（ULID・sha）を先頭 length 文字に縮める。縮めたときだけ末尾に「…」を付ける。 */
export function shortId(value: string, length = 8): string {
  return value.length <= length ? value : `${value.slice(0, length)}…`;
}

type ClipboardLike = { writeText: (text: string) => Promise<void> };

/** clipboard へ書く。clipboard が無い（非 secure context 等）・拒否されたときは false。 */
export async function copyText(value: string, clipboard: ClipboardLike | undefined): Promise<boolean> {
  if (!clipboard) return false;
  try {
    await clipboard.writeText(value);
    return true;
  } catch {
    return false;
  }
}

type CopyState = "idle" | "copied" | "failed";

const copyMessage: Record<CopyState, string> = { idle: "", copied: "コピーしました", failed: "コピーできません" };

export type ShortIdProps = {
  value: string;
  /** ID の種類（例「タスク ID」）。accessible name とコピー操作の名前に使う。 */
  label?: string;
  length?: number;
  /** false でコピー操作を出さない（表の中で行の link に含めるとき等）。 */
  copyable?: boolean;
  className?: string;
};

/** 等幅で省略表示する ID。全文は title と accessible name に持ち、隣の button でコピーする（44px）。 */
export function ShortId({ value, label = "ID", length = 8, copyable = true, className }: ShortIdProps) {
  const [state, setState] = useState<CopyState>("idle");
  const copy = async () => {
    const ok = await copyText(value, typeof navigator === "undefined" ? undefined : navigator.clipboard);
    setState(ok ? "copied" : "failed");
  };
  return (
    <span data-slot="short-id" className={cn("inline-flex min-w-0 max-w-full items-center gap-1", className)}>
      <code
        title={value}
        className="min-w-0 truncate rounded-sm bg-code px-1 font-mono text-label text-code-foreground"
      >
        {/* 読み上げは省略しない全文、見た目は省略形（code に aria-label は付けられないため 2 つに分ける）。 */}
        <span aria-hidden="true">{shortId(value, length)}</span>
        <span className="sr-only">{`${label} ${value}`}</span>
      </code>
      {copyable ? (
        <>
          <button
            type="button"
            aria-label={`${label}をコピー`}
            title={`${label}をコピー`}
            onClick={copy}
            onBlur={() => setState("idle")}
            className="inline-flex min-h-11 min-w-11 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
          >
            <Icon name={state === "copied" ? "check" : "copy"} size="sm" />
          </button>
          <span role="status" className="text-label text-muted-foreground empty:hidden">
            {copyMessage[state]}
          </span>
        </>
      ) : null}
    </span>
  );
}
