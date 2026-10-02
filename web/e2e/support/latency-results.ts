import { appendFileSync } from "node:fs";

/** The gate writes one JSON object per completed measurement to the run artifact. */
export function recordLatency(value: Record<string, unknown>) {
  const file = process.env.WEB_LATENCY_RESULTS;
  if (file) appendFileSync(file, `${JSON.stringify(value)}\n`);
}
