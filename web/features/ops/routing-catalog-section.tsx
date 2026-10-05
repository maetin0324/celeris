import type { RoutingCatalogView } from "../../api/generated/types";
import {
  billingLabel,
  capabilitiesLabel,
  contextLabel,
  deploymentModelNote,
  deploymentPricingLabel,
  pricingLabel,
  qualityLabel,
  routingModeLabel,
  UNKNOWN,
} from "./routing-catalog";

/** 「不明」を含む値は濃い琥珀で目立たせる（白地で 7:1 以上。薄い色は本文に使わない）。 */
function Value({ text }: { text: string }) {
  return <span className={text.includes(UNKNOWN) ? "text-amber-900" : undefined}>{text}</span>;
}

function Row({ label, text }: { label: string; text: string }) {
  return (
    <div className="break-words">
      <dt className="inline font-semibold">{label}: </dt>
      <dd className="inline">
        <Value text={text} />
      </dd>
    </div>
  );
}

/** routing catalog の節: model（能力・品質・価格）と deployment（source × model）を別の一覧で出す。 */
export function RoutingCatalogList({ data }: { data: RoutingCatalogView }) {
  return (
    <div className="space-y-3 min-w-0">
      <p className="text-sm break-words">
        mode {routingModeLabel(data.mode)} / catalog 版 {data.catalog_version}
      </p>
      {data.warnings.length > 0 && (
        <ul className="text-sm text-amber-900 space-y-1" aria-label="catalog の警告">
          {data.warnings.map((w) => (
            <li key={w} className="break-words">
              警告: {w}
            </li>
          ))}
        </ul>
      )}
      <section className="space-y-2 min-w-0" aria-label="model の一覧">
        <h3 className="font-semibold">model（{data.models.length}）</h3>
        <p className="text-sm">モデルそのものの能力・品質・価格です。どの供給元で動くかは下の配置で示します。</p>
        {data.models.length === 0 ? (
          <p className="text-sm">model がありません。</p>
        ) : (
          <ul className="space-y-2">
            {data.models.map((m) => (
              <li key={m.id} className="rounded border p-2 text-sm" aria-label={`model ${m.id}`}>
                <p className="font-semibold break-words">
                  {m.id}（{m.family} / 版 {m.revision}）
                </p>
                <dl>
                  <Row label="能力" text={capabilitiesLabel(m)} />
                  <Row label="context" text={contextLabel(m.context_limits)} />
                  <Row label="品質" text={qualityLabel(m.quality)} />
                  <Row label="価格" text={pricingLabel(m.pricing)} />
                </dl>
              </li>
            ))}
          </ul>
        )}
      </section>
      <section className="space-y-2 min-w-0" aria-label="deployment の一覧">
        <h3 className="font-semibold">deployment（source × model、{data.deployments.length}）</h3>
        <p className="text-sm">供給元（LLM source）の上でどの model をどの名前で呼ぶかと、使える lane です。</p>
        {data.deployments.length === 0 ? (
          <p className="text-sm">deployment がありません。</p>
        ) : (
          <ul className="space-y-2">
            {data.deployments.map((d) => {
              const note = deploymentModelNote(d, data.models);
              return (
                <li key={d.id} className="rounded border p-2 text-sm" aria-label={`deployment ${d.id}`}>
                  <p className="font-semibold break-words">{d.id}</p>
                  <dl>
                    <Row label="source" text={d.source_ref} />
                    <Row label="model" text={d.model_profile_id} />
                    <Row label="上流の model 名" text={d.upstream_model} />
                    <Row label="課金" text={billingLabel(d.billing)} />
                    <Row
                      label="lane"
                      text={d.allowed_lanes.length ? d.allowed_lanes.map((l) => `celeris/${l}`).join(", ") : "なし"}
                    />
                    <Row label="価格" text={deploymentPricingLabel(d, data.models)} />
                  </dl>
                  {note && <p className="text-amber-900 break-words">{note}</p>}
                </li>
              );
            })}
          </ul>
        )}
      </section>
    </div>
  );
}
