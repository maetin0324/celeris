import { Dialog } from "radix-ui";
import { type ReactElement, type ReactNode, useId } from "react";
import { cn } from "../../lib/utils";
import { Icon } from "./icon";

export type DrawerProps = {
  /** Radix が閉じた後に focus を戻す操作。focus 可能な単一要素を渡す。 */
  trigger: ReactElement;
  title: string;
  description?: string;
  children: ReactNode;
  open?: boolean;
  defaultOpen?: boolean;
  onOpenChange?: (open: boolean) => void;
  className?: string;
};

/** 狭い画面では全幅、広い画面では右端の side panel。 */
export function Drawer({ trigger, title, description, children, className, ...rootProps }: DrawerProps) {
  const descriptionId = useId();
  return (
    <Dialog.Root {...rootProps}>
      <Dialog.Trigger asChild>{trigger}</Dialog.Trigger>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-overlay transition-opacity duration-150 motion-reduce:transition-none" />
        <Dialog.Content
          aria-describedby={description ? descriptionId : undefined}
          className={cn(
            "fixed inset-y-0 right-0 flex w-full max-w-drawer flex-col overflow-y-auto overscroll-contain border-l border-border bg-surface p-4 text-foreground shadow-dialog transition-transform duration-150 state-closed:translate-x-full motion-reduce:transition-none md:p-6",
            className,
          )}
        >
          <div className="flex min-w-0 items-start justify-between gap-2">
            <div className="min-w-0">
              <Dialog.Title className="break-words text-section font-semibold">{title}</Dialog.Title>
              {description ? (
                <Dialog.Description id={descriptionId} className="mt-2 break-words text-label text-muted-foreground">
                  {description}
                </Dialog.Description>
              ) : null}
            </div>
            <Dialog.Close
              aria-label="閉じる"
              className="inline-flex min-h-11 min-w-11 shrink-0 items-center justify-center rounded-md text-foreground hover:bg-accent focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
            >
              <Icon name="close" />
            </Dialog.Close>
          </div>
          <div className="mt-4 min-w-0 flex-1">{children}</div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

export const SidePanel = Drawer;
