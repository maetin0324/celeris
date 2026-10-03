import { AlertDialog } from "radix-ui";
import { type ReactElement, useRef, useState } from "react";
import { Button, buttonVariants } from "./button";

export type ConfirmDialogProps = {
  trigger: ReactElement;
  title: string;
  /** 操作する対象の人が読める名前。 */
  target: string;
  /** 実行後に何が変わるか。影響範囲もここに書く。 */
  consequence: string;
  /** 元に戻せるか、戻せる場合はその方法。 */
  reversibility: string;
  /** 実行後の結果をどこで確認できるか。 */
  followUp: string;
  /** 動詞と対象を含める。例:「タスク A を削除」 */
  confirmLabel: string;
  onConfirm: () => Promise<void> | void;
  cancelLabel?: string;
};

/** 送信完了まで開いたままにし、失敗時は理由を示して再確認できるようにする。 */
export function ConfirmDialog({
  trigger,
  title,
  target,
  consequence,
  reversibility,
  followUp,
  confirmLabel,
  onConfirm,
  cancelLabel = "戻る",
}: ConfirmDialogProps) {
  const [open, setOpen] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const submitting = useRef(false);
  const cancelRef = useRef<HTMLButtonElement>(null);

  async function confirm() {
    if (submitting.current) return;
    submitting.current = true;
    setPending(true);
    setError(null);
    try {
      await onConfirm();
      setOpen(false);
    } catch (reason) {
      setError(
        reason instanceof Error && reason.message
          ? reason.message
          : "操作を完了できませんでした。状態を確認してください。",
      );
    } finally {
      submitting.current = false;
      setPending(false);
    }
  }

  return (
    <AlertDialog.Root
      open={open}
      onOpenChange={(nextOpen) => {
        if (submitting.current) return;
        setOpen(nextOpen);
        if (nextOpen) setError(null);
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
            <AlertDialog.Title className="break-words text-section font-semibold">{title}</AlertDialog.Title>
            <AlertDialog.Description className="break-words text-body text-foreground">
              対象: {target}。{consequence}
            </AlertDialog.Description>
            <p className="break-words text-label text-muted-foreground">元に戻す方法: {reversibility}</p>
            <p className="break-words text-label text-muted-foreground">結果の確認: {followUp}</p>
          </div>
          {error ? (
            <p role="alert" className="rounded-md bg-danger p-3 text-danger-foreground">
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
              {cancelLabel}
            </AlertDialog.Cancel>
            <Button variant="destructive" disabled={pending} onClick={confirm}>
              {confirmLabel}
            </Button>
          </div>
        </AlertDialog.Content>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  );
}
