import type { IntegrationRepairView } from "../../api/generated/types";
import { type IntegrationRepairTone, integrationRepairDisplay } from "./integration-repair";

// 実装失敗（赤の「失敗」）と区別するため、青系の枠と専用ラベルで出す。
const frame: Record<IntegrationRepairTone, string> = {
  info: "border-sky-300 bg-sky-50",
  success: "border-sky-300 bg-sky-50",
  warning: "border-indigo-300 bg-indigo-50",
};

export function IntegrationRepairPanel({ view }: { view: IntegrationRepairView | null | undefined }) {
  const display = integrationRepairDisplay(view);
  if (!display) return null;
  return (
    <section
      className={`min-w-0 rounded border p-3 ${frame[display.tone]}`}
      data-testid="integration-repair"
      data-state={display.state}
      aria-label={display.heading}
    >
      <h2 className="flex flex-wrap items-center gap-2 text-base font-semibold">
        <span>{display.heading}</span>
        <span className="rounded bg-sky-200 px-1.5 py-0.5 text-xs font-medium text-sky-900">{display.label}</span>
      </h2>
      <dl className="mt-2 min-w-0 grid grid-cols-[minmax(6rem,10rem)_minmax(0,1fr)] gap-x-3 gap-y-1 text-sm">
        {display.rows.map((row) => (
          <div key={row.label} className="contents">
            <dt className="text-neutral-600">{row.label}</dt>
            <dd className="break-all">{row.value}</dd>
          </div>
        ))}
      </dl>
    </section>
  );
}
