import { queryOptions, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { apiGet, apiMutate, isApiError } from "../../api/client";
import type {
  ConfigEventList,
  ConfigFieldsView,
  ConfigFieldView,
  ConfigPatchResult,
  ConfigRuleView,
} from "../../api/generated/types";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { Input } from "../../components/ui/input";
import { Notice } from "../../components/ui/notice";
import { Section } from "../../components/ui/panel";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "../../components/ui/table";

// ADR 2026-10-22-dynamic-config: config.toml の欄を見て、動的な欄を再起動なしに変える画面。
// 起動時のみの欄は値と「再起動が要る」旨だけを出し、secret の値は daemon が伏せた "<redacted>" のまま出さない。

const settingsKeys = {
  fields: ["config", "fields"] as const,
  events: ["config", "events"] as const,
};

// path は api client の規約どおり /api/...（gateway が daemon の /api/v1/... へ前置する）。
export const configFieldsQuery = queryOptions({
  queryKey: settingsKeys.fields,
  queryFn: ({ signal }) => apiGet<ConfigFieldsView>("/api/config/fields", signal),
});

export const configEventsQuery = queryOptions({
  queryKey: settingsKeys.events,
  queryFn: ({ signal }) => apiGet<ConfigEventList>("/api/config/events", signal),
});

export function patchConfigField(key: string, value: unknown): Promise<ConfigPatchResult> {
  return apiMutate<ConfigPatchResult>("PATCH", "/api/config/fields", { changes: [{ key, value }] });
}

export const applyUnitLabel: Record<string, string> = {
  next_tick: "次の tick から",
  next_dispatch: "次の dispatch（次に起動する run）から",
  next_request: "API の次の要求から",
  restart: "再起動で効く",
  elsewhere: "別の画面（DB が正）",
};

export const classLabel: Record<string, string> = {
  dynamic: "動的",
  startup: "起動時のみ",
  seed: "種（DB が正）",
};

/** 分類の表を最長一致で引く（daemon の `config::fields::rule_for` と同じ規則）。 */
export function ruleFor(rules: readonly ConfigRuleView[], key: string): ConfigRuleView | undefined {
  let best: ConfigRuleView | undefined;
  for (const rule of rules) {
    const hit = key === rule.key || key.startsWith(`${rule.key}.`);
    if (hit && (!best || rule.key.length > best.key.length)) best = rule;
  }
  return best;
}

/** 入力欄の文字列を JSON の値にする。JSON として読めなければ文字列。空欄は null（欄を消して既定値に戻す）。 */
export function parseInput(text: string): unknown {
  const trimmed = text.trim();
  if (trimmed === "") return null;
  try {
    return JSON.parse(trimmed);
  } catch {
    return text;
  }
}

export function showValue(value: unknown): string {
  if (value === null || value === undefined) return "（既定値）";
  return typeof value === "string" ? value : JSON.stringify(value);
}

function inputText(value: unknown): string {
  if (value === null || value === undefined) return "";
  return JSON.stringify(value);
}

export function restartMessage(key: string, rule: ConfigRuleView | undefined): string | null {
  if (!rule || rule.class === "dynamic") return null;
  if (rule.class === "startup") {
    return `${key} は起動時にしか決められない欄です（${rule.reason}）。config.toml を書き換えて celeris を再起動すると効きます。`;
  }
  return `${key} は DB が正の欄です（${rule.reason}）。この画面では変えません。`;
}

function problemDetail(error: unknown): string {
  if (isApiError(error)) {
    const body = error.body as { detail?: string } | undefined;
    return body?.detail ?? error.message;
  }
  return error instanceof Error ? error.message : String(error);
}

type Save = (key: string, value: unknown) => void;

function DynamicRow({ field, onSave, pending }: { field: ConfigFieldView; onSave: Save; pending: boolean }) {
  const [text, setText] = useState(inputText(field.value));
  const id = `config-${field.key}`;
  return (
    <TableRow>
      <TableCell className="break-all">
        <label htmlFor={id}>{field.key}</label>
        <div className="text-label text-muted-foreground">{field.reason}</div>
      </TableCell>
      <TableCell className="min-w-48">
        {field.secret ? (
          <span className="text-muted-foreground">秘密の値は表示しません（秘密の画面で扱います）</span>
        ) : (
          <form
            className="flex flex-wrap items-center gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              onSave(field.key, parseInput(text));
            }}
          >
            <Input id={id} value={text} onChange={(e) => setText(e.target.value)} disabled={pending} />
            <Button type="submit" disabled={pending}>
              保存
            </Button>
          </form>
        )}
      </TableCell>
      <TableCell>{applyUnitLabel[field.apply_unit] ?? field.apply_unit}</TableCell>
    </TableRow>
  );
}

export function SettingsView({
  fields,
  events,
  onSave,
  pending = false,
  error = null,
  saved = null,
}: {
  fields: ConfigFieldsView;
  events: ConfigEventList;
  onSave: Save;
  pending?: boolean;
  error?: string | null;
  saved?: ConfigPatchResult | null;
}) {
  const [newKey, setNewKey] = useState("");
  const [newValue, setNewValue] = useState("");
  const dynamic = fields.fields.filter((f) => f.class === "dynamic");
  const fixed = fields.fields.filter((f) => f.class !== "dynamic");
  const newRule = newKey.trim() ? ruleFor(fields.rules, newKey.trim()) : undefined;
  const newRestart = newKey.trim() ? restartMessage(newKey.trim(), newRule) : null;
  return (
    <div className="min-w-0 space-y-4">
      <p className="break-all text-label text-muted-foreground">
        設定ファイル: {fields.config_path ?? "（ファイルから読んでいません）"}。変更は検証してから config.toml
        に書き戻し（コメントと並びは保ちます）、稼働中の daemon に入れます。
      </p>
      {error && (
        <Notice tone="danger" title="変更できませんでした">
          {error}
        </Notice>
      )}
      {saved && (
        <Notice tone="warning" title="変更しました">
          {saved.changes.length === 0
            ? "値は変わりませんでした。"
            : saved.changes
                .map(
                  (c) =>
                    `${c.key}: ${showValue(c.before)} → ${showValue(c.after)}（${applyUnitLabel[c.apply_unit] ?? c.apply_unit}）`,
                )
                .join(" / ")}
        </Notice>
      )}
      <Section
        title={`動的に変えられる欄（${dynamic.length}）`}
        description={
          '値は JSON で書きます（数・true/false・"文字列"・[配列]）。空欄で保存すると欄を消し、既定値に戻します。'
        }
      >
        <Table aria-label="動的な欄">
          <TableHeader>
            <TableRow>
              <TableHead>欄</TableHead>
              <TableHead>値</TableHead>
              <TableHead>反映</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {dynamic.map((f) => (
              <DynamicRow key={`${f.key}:${JSON.stringify(f.value)}`} field={f} onSave={onSave} pending={pending} />
            ))}
          </TableBody>
        </Table>
        <form
          className="mt-3 space-y-2"
          aria-label="欄を足す"
          onSubmit={(event) => {
            event.preventDefault();
            if (!newRestart) onSave(newKey.trim(), parseInput(newValue));
          }}
        >
          <div className="flex flex-wrap gap-2">
            <div className="min-w-48 flex-1">
              <label htmlFor="config-new-key">欄（例 execution.max_replans）</label>
              <Input
                id="config-new-key"
                value={newKey}
                onChange={(e) => setNewKey(e.target.value)}
                disabled={pending}
              />
            </div>
            <div className="min-w-48 flex-1">
              <label htmlFor="config-new-value">値（JSON）</label>
              <Input
                id="config-new-value"
                value={newValue}
                onChange={(e) => setNewValue(e.target.value)}
                disabled={pending}
              />
            </div>
          </div>
          {newRestart && (
            <p role="note" className="rounded border border-border bg-warning p-3 text-warning-foreground">
              {newRestart}
            </p>
          )}
          {newRule && !newRestart && (
            <p className="text-label text-muted-foreground">
              {classLabel[newRule.class]}・{applyUnitLabel[newRule.apply_unit] ?? newRule.apply_unit}（{newRule.reason}
              ）
            </p>
          )}
          <Button type="submit" disabled={pending || !newKey.trim() || newRestart !== null}>
            足す・変える
          </Button>
        </form>
      </Section>
      <Section
        title={`起動時にしか決められない欄（${fixed.length}）`}
        description="ここに並ぶ欄は画面からは変えません。変えるには config.toml を書き換えて celeris を再起動してください（再起動が要ります）。"
      >
        <Table aria-label="起動時のみの欄">
          <TableHeader>
            <TableRow>
              <TableHead>欄</TableHead>
              <TableHead>値</TableHead>
              <TableHead>分類</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {fixed.map((f) => (
              <TableRow key={f.key}>
                <TableCell className="break-all">
                  {f.key}
                  <div className="text-label text-muted-foreground">{f.reason}</div>
                </TableCell>
                <TableCell className="break-all">
                  {f.secret ? "秘密の値は表示しません" : showValue(f.value)}
                  {f.pending_restart && (
                    <div className="text-label">
                      <Badge tone="warning">再起動待ち</Badge> 稼働中:{" "}
                      {f.secret ? "（伏せています）" : showValue(f.running)}
                    </div>
                  )}
                </TableCell>
                <TableCell>
                  <Badge tone={f.class === "startup" ? "neutral" : "info"}>{classLabel[f.class] ?? f.class}</Badge>
                  <div className="text-label text-muted-foreground">
                    {f.class === "startup" ? "変更には再起動が要ります" : applyUnitLabel[f.apply_unit]}
                  </div>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </Section>
      <Section
        title={`変更の履歴（${events.items.length}）`}
        description="誰が・いつ・どの欄を・前後の値（新しい順）。"
      >
        {events.items.length === 0 ? (
          <p>変更の記録はありません。</p>
        ) : (
          <Table aria-label="変更の履歴">
            <TableHeader>
              <TableRow>
                <TableHead>時刻</TableHead>
                <TableHead>誰が</TableHead>
                <TableHead>欄</TableHead>
                <TableHead>前 → 後</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {events.items.map((e) => (
                <TableRow key={e.id}>
                  <TableCell className="break-all">{e.ts}</TableCell>
                  <TableCell className="break-all">{e.actor}</TableCell>
                  <TableCell className="break-all">{e.key}</TableCell>
                  <TableCell className="break-all">
                    {showValue(e.before)} → {showValue(e.after)}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </Section>
    </div>
  );
}

export function SettingsScreen() {
  const client = useQueryClient();
  const fields = useQuery(configFieldsQuery);
  const events = useQuery(configEventsQuery);
  const save = useMutation({
    mutationFn: ({ key, value }: { key: string; value: unknown }) => patchConfigField(key, value),
    onSettled: () => {
      void client.invalidateQueries({ queryKey: ["config"] });
    },
  });
  const data = fields.data && events.data ? { fields: fields.data, events: events.data } : undefined;
  const query = {
    data,
    isPending: fields.isPending || events.isPending,
    isFetching: fields.isFetching || events.isFetching,
    isError: fields.isError || events.isError,
    error: fields.error ?? events.error,
    dataUpdatedAt: Math.min(fields.dataUpdatedAt || 0, events.dataUpdatedAt || 0),
    refetch: async () => {
      await Promise.all([fields.refetch(), events.refetch()]);
    },
  };
  return (
    <ScreenFrame title="設定" route="/settings">
      <FetchFrame query={query}>
        {data && (
          <SettingsView
            fields={data.fields}
            events={data.events}
            onSave={(key, value) => save.mutate({ key, value })}
            pending={save.isPending}
            error={save.isError ? problemDetail(save.error) : null}
            saved={save.data ?? null}
          />
        )}
      </FetchFrame>
    </ScreenFrame>
  );
}
