import { AlertDialog } from "radix-ui";
import { type ReactElement, useId, useRef, useState } from "react";
import type { OverrideMode } from "../../../api/generated/types";
import { Button, buttonVariants } from "../../../components/ui/button";
import { fieldClassName } from "../../../components/ui/input";
import { OVERRIDE_LABELS, type OverrideOutcome } from "./card-model";

// CoS 代答の取消・差し戻しの確認。ConfirmDialog と同じ作法（戻るが初期 focus、送信中は閉じない）に、
// API が必須とする理由の入力を足す。競合（409）などの結果はカード側に残すので、入力の不備のときだけ開いたままにする。

const CONSEQUENCE: Record<OverrideMode, string> = {
  revoke: "まだ使われていない回答・認可を失効させ、人の判断待ちに戻します。",
  return:
    "回答を置き換え済みにして人の判断待ちを作り直します。既に後続が動いていれば対象のタスクを一時停止します。戻せない外部への影響は修正のタスクを起票します。",
};

export type OverrideDialogProps = {
  mode: OverrideMode;
  target: string;
  disabled?: boolean;
  onConfirm: (reason: string) => Promise<OverrideOutcome>;
};

export function OverrideDialog({ mode, target, disabled, onConfirm }: OverrideDialogProps) {
  const id = useId();
  const reasonId = `${id}-reason`;
  const errorId = `${id}-error`;
  const [open, setOpen] = useState(false);
  const [pending, setPending] = useState(false);
  const [reason, setReason] = useState("");
  const [error, setError] = useState<string | null>(null);
  const submitting = useRef(false);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const label = OVERRIDE_LABELS[mode];

  async function confirm() {
    if (submitting.current) return;
    if (reason.trim() === "") {
      setError("理由を書いてください。");
      return;
    }
    submitting.current = true;
    setPending(true);
    setError(null);
    try {
      const outcome = await onConfirm(reason);
      // 入力の不備だけはその場で直させる。それ以外（成功・競合・通信失敗）はカードに結果を出して閉じる。
      if (!outcome.ok && !outcome.conflict && !outcome.stale) setError(outcome.message);
      else {
        setOpen(false);
        setReason("");
      }
    } finally {
      submitting.current = false;
      setPending(false);
    }
  }

  const trigger: ReactElement = (
    <Button variant={mode === "revoke" ? "destructive" : "secondary"} size="sm" disabled={disabled}>
      {label}
    </Button>
  );

  return (
    <AlertDialog.Root
      open={open}
      onOpenChange={(next) => {
        if (submitting.current) return;
        setOpen(next);
        if (next) setError(null);
      }}
    >
      <AlertDialog.Trigger asChild>{trigger}</AlertDialog.Trigger>
      <AlertDialog.Portal>
        <AlertDialog.Overlay className="fixed inset-0 bg-overlay" />
        <AlertDialog.Content
          onOpenAutoFocus={(event) => {
            event.preventDefault();
            cancelRef.current?.focus();
          }}
          onEscapeKeyDown={(event) => {
            if (submitting.current) event.preventDefault();
          }}
          className="fixed inset-x-4 top-1/2 mx-auto flex max-h-full w-auto max-w-form -translate-y-1/2 flex-col gap-4 overflow-y-auto rounded-lg border border-border bg-surface p-4 text-foreground shadow-dialog md:p-6"
        >
          <div className="flex flex-col gap-2">
            <AlertDialog.Title className="break-words text-section font-semibold">
              CoS の代答を{label}しますか
            </AlertDialog.Title>
            <AlertDialog.Description className="break-words text-body text-foreground">
              対象: {target}。{CONSEQUENCE[mode]}
            </AlertDialog.Description>
            <p className="break-words text-label text-muted-foreground">
              元に戻す方法: 新しく開いた判断待ちに答え直します。監査の履歴は消えません。
            </p>
            <p className="break-words text-label text-muted-foreground">
              結果の確認: このカードと受信箱で確認できます。
            </p>
          </div>
          <div className="flex flex-col gap-1">
            <label htmlFor={reasonId} className="text-label font-medium">
              理由（必須）
            </label>
            <textarea
              id={reasonId}
              className={`${fieldClassName} min-h-11`}
              rows={2}
              value={reason}
              disabled={pending}
              onChange={(event) => setReason(event.target.value)}
              aria-invalid={error ? true : undefined}
              aria-describedby={error ? errorId : undefined}
            />
          </div>
          {error ? (
            <p id={errorId} role="alert" className="rounded-md bg-danger p-3 text-danger-foreground">
              {error}
            </p>
          ) : null}
          {pending ? (
            <p role="status" className="text-label text-muted-foreground">
              処理中です。完了までお待ちください。
            </p>
          ) : null}
          <div className="flex flex-wrap justify-end gap-2">
            <AlertDialog.Cancel ref={cancelRef} disabled={pending} className={buttonVariants({ variant: "secondary" })}>
              戻る
            </AlertDialog.Cancel>
            <Button variant="destructive" disabled={pending} onClick={() => void confirm()}>
              代答を{label}
            </Button>
          </div>
        </AlertDialog.Content>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  );
}
