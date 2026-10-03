import { type ComponentPropsWithoutRef, type ReactNode, useId } from "react";
import { cn } from "../../lib/utils";

// 画面の中の枠（P2-02）。見出しは h2 以下にし、h1 は画面の見出しだけにする。
// 既存の API（{title, children}）と markup（section の aria-label・h2）は互換のまま、色と寸法を token にした。
export function Panel({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <section aria-label={title} className="min-w-0 rounded-lg border border-border bg-surface p-4">
      <h2 className="text-section font-semibold text-foreground">{title}</h2>
      {children ? <div className="mt-2 text-label text-foreground">{children}</div> : null}
    </section>
  );
}

export type SectionLevel = 2 | 3 | 4;

export type SectionProps = Omit<ComponentPropsWithoutRef<"section">, "title"> & {
  title: ReactNode;
  /** 見出しの level。h1 は画面の見出しだけなので 2〜4。既定は 2。 */
  level?: SectionLevel;
  description?: ReactNode;
  /** 見出しの右上に置く操作。狭い幅では見出しの下へ折り返す。 */
  actions?: ReactNode;
};

const headingClass: Record<SectionLevel, string> = {
  2: "text-section font-semibold",
  3: "text-body font-semibold",
  4: "text-label font-semibold",
};

// 見出し・説明・操作を持つ区画。入れ子の枠を増やさないよう、枠線は持たず余白と見出しで分ける。
export function Section({ title, level = 2, description, actions, className, children, ...props }: SectionProps) {
  const id = useId();
  const headingId = `${id}-title`;
  const descriptionId = description ? `${id}-description` : undefined;
  const Heading = `h${level}` as "h2" | "h3" | "h4";
  return (
    <section
      aria-labelledby={headingId}
      aria-describedby={descriptionId}
      className={cn("min-w-0", className)}
      {...props}
    >
      <div className="flex flex-col gap-2 md:flex-row md:items-start md:justify-between">
        <div className="min-w-0">
          <Heading id={headingId} className={cn("break-words text-foreground", headingClass[level])}>
            {title}
          </Heading>
          {description ? (
            <p id={descriptionId} className="mt-1 text-label text-muted-foreground">
              {description}
            </p>
          ) : null}
        </div>
        {actions ? (
          <div data-slot="section-actions" className="flex shrink-0 flex-wrap items-center gap-2">
            {actions}
          </div>
        ) : null}
      </div>
      {children ? <div className="mt-3 min-w-0">{children}</div> : null}
    </section>
  );
}
