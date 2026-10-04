import type { IntegrationRepairView } from "../../api/generated/types";
import { Badge } from "../../components/ui/badge";
import { DataList } from "../../components/ui/data-list";
import { integrationRepairDisplay, integrationRepairTone } from "./integration-repair";

// 実装失敗（danger の「失敗」）と区別するため、info の枠と専用ラベルで出す。
// id は木の「integration repair」行からの移動先（overview-view の TaskTree）。
export function IntegrationRepairPanel({
  view,
  anchorId,
}: {
  view: IntegrationRepairView | null | undefined;
  /** 木からの移動先にするときの id（task 詳細だけ。inbox では複数並ぶので付けない）。 */
  anchorId?: string;
}) {
  const display = integrationRepairDisplay(view);
  if (!display) return null;
  return (
    <section
      id={anchorId}
      className="min-w-0 rounded-lg border border-border bg-info p-4 text-info-foreground"
      data-testid="integration-repair"
      data-state={display.state}
      aria-label={display.heading}
    >
      <h2 className="flex flex-wrap items-center gap-2 text-section font-semibold">
        <span className="min-w-0 break-words">{display.heading}</span>
        <Badge tone={integrationRepairTone(display.tone)}>{display.label}</Badge>
      </h2>
      <DataList
        className="mt-2 rounded-md bg-surface px-3"
        items={display.rows.map((row) => ({
          label: row.label,
          value: <span className="break-all">{row.value}</span>,
        }))}
      />
    </section>
  );
}
