import {
  AlertCircle,
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  ChevronUp,
  Clipboard,
  Ellipsis,
  ExternalLink,
  Folder,
  House,
  Inbox,
  Info,
  Kanban,
  ListChecks,
  type LucideIcon,
  Menu,
  RefreshCw,
  X,
} from "lucide-react";
import type { ComponentProps } from "react";

const icons = {
  alert: AlertCircle,
  check: Check,
  "chevron-down": ChevronDown,
  "chevron-left": ChevronLeft,
  "chevron-right": ChevronRight,
  "chevron-up": ChevronUp,
  close: X,
  copy: Clipboard,
  external: ExternalLink,
  folder: Folder,
  home: House,
  inbox: Inbox,
  info: Info,
  kanban: Kanban,
  "list-checks": ListChecks,
  menu: Menu,
  "more-horizontal": Ellipsis,
  refresh: RefreshCw,
} as const satisfies Record<string, LucideIcon>;

export type IconName = keyof typeof icons;
export type IconSize = "sm" | "default";

type DecorativeIconProps = {
  name: IconName;
  size?: IconSize;
  className?: string;
  "aria-hidden"?: true;
  "aria-label"?: never;
};

/** 装飾用 icon。accessible name は可視テキスト側に置く。 */
export function Icon({ name, size = "default", className, ...props }: DecorativeIconProps) {
  const Glyph = icons[name];
  const dimension = size === "sm" ? "var(--spacing-icon-sm)" : "var(--spacing-icon)";
  return (
    <Glyph
      data-icon="inline-start"
      aria-hidden="true"
      focusable="false"
      width={dimension}
      height={dimension}
      strokeWidth={2}
      className={className}
      {...props}
    />
  );
}

type IconOnlyButtonProps = Omit<ComponentProps<"button">, "children" | "aria-label"> & {
  name: IconName;
  label: string;
  size?: IconSize;
};

/** Icon だけで示す操作。読み上げ名とモバイル用 hit target を必須にする。 */
export function IconOnlyButton({
  name,
  label,
  size = "default",
  className,
  type = "button",
  ...props
}: IconOnlyButtonProps) {
  return (
    <button
      type={type}
      aria-label={label}
      data-icon-only="true"
      className={`inline-flex min-h-11 min-w-11 items-center justify-center ${className ?? ""}`.trim()}
      {...props}
    >
      <Icon name={name} size={size} />
    </button>
  );
}
