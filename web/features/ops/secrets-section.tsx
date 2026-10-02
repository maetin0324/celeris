import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { apiGet } from "../../api/client";
import type { LlmSourcesView, SecretList } from "../../api/generated/types";
import { accountKeys } from "../../api/queries/keys";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { Button } from "../../components/ui/button";

const inputClass = "block w-full min-h-11 rounded border p-2";

/** secret の値は state の input にだけ置き、送る直前に空にする。query key・mutation key・応答表示には載せない。 */
function SecretForm({ run, pending }: { run: ReturnType<typeof useActionResult>["run"]; pending: boolean }) {
  const [id, setId] = useState("");
  const [value, setValue] = useState("");
  return (
    <form
      className="rounded border p-3 space-y-2"
      aria-label="secret を設定"
      onSubmit={(event) => {
        event.preventDefault();
        const sentId = id.trim();
        const sent = value;
        setValue("");
        void run([
          {
            id: `put:${sentId}`,
            path: `/api/secrets/${encodeURIComponent(sentId)}`,
            method: "PUT",
            body: { value: sent },
          },
        ]);
      }}
    >
      <div className="grid gap-2 sm:grid-cols-2">
        <label className="block">
          secret id
          <input className={inputClass} required value={id} onChange={(e) => setId(e.target.value)} />
        </label>
        <label className="block">
          secret 値
          <input
            className={inputClass}
            type="password"
            autoComplete="new-password"
            required
            value={value}
            onChange={(e) => setValue(e.target.value)}
          />
        </label>
      </div>
      <Button type="submit" disabled={pending}>
        secret を保存
      </Button>
    </form>
  );
}

export function SecretsSection() {
  const secrets = useQuery({
    queryKey: accountKeys.list({ section: "secrets" }),
    queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<SecretList>("/api/secrets", signal),
  });
  const llm = useQuery({
    queryKey: accountKeys.list({ section: "llm" }),
    queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<LlmSourcesView>("/api/llm/sources", signal),
  });
  const sender = useActionResult(accountKeys.all);
  return (
    <section className="space-y-3 min-w-0" aria-label="secret と LLM source">
      <h2 className="text-lg font-semibold">secret</h2>
      <FetchFrame query={secrets}>
        {secrets.data && (
          <ul className="space-y-2">
            {secrets.data.items.map((item) => (
              <li key={item.id} className="rounded border p-2 space-y-1" aria-label={`secret ${item.id}`}>
                <span className="font-semibold break-words">{item.id}</span>{" "}
                <span className="text-sm">{item.fingerprint ? `fingerprint ${item.fingerprint}` : "未設定"}</span>
                <div>
                  <Button
                    disabled={sender.pending}
                    onClick={() => {
                      if (window.confirm(`secret ${item.id} を削除しますか`))
                        void sender.run([
                          {
                            id: `del:${item.id}`,
                            path: `/api/secrets/${encodeURIComponent(item.id)}`,
                            method: "DELETE",
                          },
                        ]);
                    }}
                  >
                    secret を削除
                  </Button>
                </div>
                <ActionResultView result={sender.results[`del:${item.id}`]} />
              </li>
            ))}
          </ul>
        )}
      </FetchFrame>
      <SecretForm run={sender.run} pending={sender.pending} />
      {Object.entries(sender.results)
        .filter(([key]) => key.startsWith("put:"))
        .map(([key, result]) => (
          <ActionResultView key={key} result={result} />
        ))}
      <h2 className="text-lg font-semibold">LLM source</h2>
      <FetchFrame query={llm}>
        {llm.data && (
          <ul className="space-y-2">
            {llm.data.sources.map((source) => (
              <li
                key={source.id}
                className="rounded border p-2 text-sm break-words"
                aria-label={`LLM source ${source.id}`}
              >
                {source.id}（{source.kind}）/ {source.enabled ? "有効" : "無効"} / 直近 1 時間{" "}
                {source.last_hour_requests} 件
                {source.accounts.length > 0 && ` / アカウント ${source.accounts.map((a) => a.id).join(", ")}`}
              </li>
            ))}
          </ul>
        )}
      </FetchFrame>
    </section>
  );
}
