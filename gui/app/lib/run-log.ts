/**
 * `stdout.jsonl`（各 run のワーカー標準出力）を、人が読む会話形式の表示のためのイベント列に変える純粋関数
 * （docs/adr/0013-run-log-conversation-view.md D1）。harness ごとの違いはこのファイルの adapter に閉じ込め、
 * 表示部品（`~/components/RunLog.tsx`）は harness を知らない。
 *
 * - claude-code: `--output-format stream-json`（`assistant` / `user` の tool_result / `system` / `result` …）
 * - codex: `exec --json`（`thread.*` / `turn.*` / `item.started|updated|completed`）
 * - opencode 等の ACP: JSON-RPC 2.0（`session/update` の `agent_message_chunk` / `tool_call` … と応答・エラー）
 *
 * 分類は表示のためだけ（成否・リトライ可否などの celeris の判断は再実装しない。ADR-GUI-0006 D2）。
 * **どのイベントも元の行を `raw` に全部持つ**。知らない行は `unknown` として残す。
 */

export interface DiffHunk {
  /** `+` / `-` / ` ` を頭に付けた行（unified diff の本体）。 */
  lines: string[];
  header?: string;
}

export interface FileDiff {
  path: string;
  /** `add` / `update` / `delete` 等（harness の値そのまま）。 */
  kind?: string;
  hunks: DiffHunk[];
}

export interface UsageStat {
  label: string;
  value: string;
}

interface EventBase {
  /** 元の行（1 行 = 1 つの JSON。結合したイベントは複数行）。 */
  raw: string[];
}

export type RunLogEvent = EventBase &
  (
    | { kind: "message"; text: string; role: "assistant" | "user" }
    | { kind: "thinking"; text: string }
    | {
        kind: "tool";
        id?: string;
        name: string;
        summary?: string;
        input?: unknown;
        result?: string;
        isError?: boolean;
        diffs?: FileDiff[];
        pending: boolean;
      }
    | { kind: "command"; id?: string; command: string; output?: string; exitCode?: number | null; status?: string }
    | { kind: "file_change"; id?: string; diffs: FileDiff[]; status?: string }
    | { kind: "error"; text: string; detail?: string }
    | { kind: "usage"; title: string; stats: UsageStat[]; text?: string; isError: boolean }
    | { kind: "system"; label: string; detail?: string; count: number }
    | { kind: "unknown"; label: string }
  );

export type RunLogEventKind = RunLogEvent["kind"];

type Obj = Record<string, unknown>;

function isObj(v: unknown): v is Obj {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}
function str(v: unknown): string | undefined {
  return typeof v === "string" ? v : undefined;
}
function num(v: unknown): number | undefined {
  return typeof v === "number" && Number.isFinite(v) ? v : undefined;
}
function oneLine(s: string, max = 160): string {
  const flat = s.replace(/\s+/g, " ").trim();
  return flat.length > max ? `${flat.slice(0, max)}…` : flat;
}

/** 数値を 3 桁区切りに（トークン数）。 */
function fmtInt(n: number): string {
  return n.toLocaleString("en-US");
}
function fmtDurationMs(ms: number): string {
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s} 秒`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} 分 ${s % 60} 秒`;
  return `${Math.floor(m / 60)} 時間 ${m % 60} 分`;
}

// ---------------------------------------------------------------------------------------------
// 差分
// ---------------------------------------------------------------------------------------------

/** 旧文字列と新文字列から素朴な差分（共通の先頭・末尾の行を除いて `-` と `+`）を作る。 */
export function simpleDiff(oldText: string, newText: string): DiffHunk {
  const a = oldText.length > 0 ? oldText.split("\n") : [];
  const b = newText.length > 0 ? newText.split("\n") : [];
  let head = 0;
  while (head < a.length && head < b.length && a[head] === b[head]) head++;
  let tail = 0;
  while (tail < a.length - head && tail < b.length - head && a[a.length - 1 - tail] === b[b.length - 1 - tail]) tail++;
  const ctxBefore = a.slice(Math.max(0, head - 2), head).map((l) => ` ${l}`);
  const removed = a.slice(head, a.length - tail).map((l) => `-${l}`);
  const added = b.slice(head, b.length - tail).map((l) => `+${l}`);
  const ctxAfter = a.slice(a.length - tail, Math.min(a.length, a.length - tail + 2)).map((l) => ` ${l}`);
  return { lines: [...ctxBefore, ...removed, ...added, ...ctxAfter] };
}

/** claude-code の `structuredPatch` / `bashEditDiff.files[].hunks`（`{oldStart, oldLines, newStart, newLines, lines}`）。 */
function hunksFromStructured(v: unknown): DiffHunk[] {
  if (!Array.isArray(v)) return [];
  const out: DiffHunk[] = [];
  for (const h of v) {
    if (!isObj(h) || !Array.isArray(h.lines)) continue;
    const lines = h.lines.filter((l): l is string => typeof l === "string");
    const header =
      num(h.oldStart) !== undefined
        ? `@@ -${h.oldStart},${h.oldLines ?? "?"} +${h.newStart ?? "?"},${h.newLines ?? "?"} @@`
        : undefined;
    out.push(header ? { lines, header } : { lines });
  }
  return out;
}

// ---------------------------------------------------------------------------------------------
// ツール入力の要約（主な引数だけ）
// ---------------------------------------------------------------------------------------------

const SUMMARY_KEYS = [
  "command",
  "file_path",
  "path",
  "notebook_path",
  "pattern",
  "url",
  "query",
  "description",
  "prompt",
  "skill",
  "subject",
  "plan",
];

export function summarizeToolInput(name: string, input: unknown): string | undefined {
  if (!isObj(input)) return typeof input === "string" ? oneLine(input) : undefined;
  if (name === "Bash" && typeof input.command === "string") {
    return oneLine(input.command);
  }
  if ((name === "Grep" || name === "Glob") && typeof input.pattern === "string") {
    const where = str(input.path) ?? str(input.glob);
    return oneLine(where ? `${input.pattern} (${where})` : input.pattern);
  }
  if (name === "TodoWrite" && Array.isArray(input.todos)) return `${input.todos.length} 件の TODO`;
  for (const key of SUMMARY_KEYS) {
    const v = input[key];
    if (typeof v === "string" && v.length > 0) return oneLine(v);
  }
  for (const v of Object.values(input)) {
    if (typeof v === "string" && v.length > 0) return oneLine(v);
  }
  return undefined;
}

/** tool_result の `content`（文字列、または `{type:"text", text}` 等の配列）を文字列にする。 */
function toolResultText(content: unknown): string {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content
      .map((c) => {
        if (isObj(c) && c.type === "text" && typeof c.text === "string") return c.text;
        if (isObj(c) && c.type === "image") return "[画像]";
        return JSON.stringify(c);
      })
      .join("\n");
  }
  if (content === undefined || content === null) return "";
  return JSON.stringify(content);
}

// ---------------------------------------------------------------------------------------------
// 変換の状態
// ---------------------------------------------------------------------------------------------

class Builder {
  events: RunLogEvent[] = [];
  /** tool_use id / codex item id / ACP toolCallId → events の添字。 */
  byId = new Map<string, number>();

  push(ev: RunLogEvent, id?: string): void {
    // 同じ見出しの system が続いたら 1 行にまとめる（thinking_tokens の連打・heartbeat 等）。
    const last = this.events.at(-1);
    if (ev.kind === "system" && last?.kind === "system" && last.label === ev.label) {
      last.count += 1;
      last.raw.push(...ev.raw);
      if (ev.detail !== undefined) last.detail = ev.detail;
      return;
    }
    this.events.push(ev);
    if (id !== undefined) this.byId.set(id, this.events.length - 1);
  }

  find(id: string | undefined): RunLogEvent | undefined {
    if (id === undefined) return undefined;
    const i = this.byId.get(id);
    return i === undefined ? undefined : this.events[i];
  }
}

// ---------------------------------------------------------------------------------------------
// claude-code（stream-json）
// ---------------------------------------------------------------------------------------------

function claudeToolDiffs(name: string, input: unknown): FileDiff[] | undefined {
  if (!isObj(input)) return undefined;
  const path = str(input.file_path) ?? str(input.notebook_path) ?? "";
  if (name === "Edit" && typeof input.old_string === "string" && typeof input.new_string === "string") {
    return [{ path, kind: "update", hunks: [simpleDiff(input.old_string, input.new_string)] }];
  }
  if (name === "MultiEdit" && Array.isArray(input.edits)) {
    const hunks = input.edits.filter(isObj).map((e) => simpleDiff(str(e.old_string) ?? "", str(e.new_string) ?? ""));
    return [{ path, kind: "update", hunks }];
  }
  if (name === "Write" && typeof input.content === "string") {
    return [{ path, kind: "add", hunks: [simpleDiff("", input.content)] }];
  }
  return undefined;
}

function claudeResultDiffs(toolUseResult: unknown): FileDiff[] | undefined {
  if (!isObj(toolUseResult)) return undefined;
  const sp = hunksFromStructured(toolUseResult.structuredPatch);
  if (sp.length > 0) return [{ path: str(toolUseResult.filePath) ?? "", kind: "update", hunks: sp }];
  const bed = toolUseResult.bashEditDiff;
  if (isObj(bed) && Array.isArray(bed.files)) {
    const files = bed.files
      .filter(isObj)
      .map((f) => ({ path: str(f.filePath) ?? "", kind: "update", hunks: hunksFromStructured(f.hunks) }))
      .filter((f) => f.hunks.length > 0);
    return files.length > 0 ? files : undefined;
  }
  return undefined;
}

function claudeUsage(value: Obj): RunLogEvent {
  const stats: UsageStat[] = [];
  const cost = num(value.total_cost_usd);
  if (cost !== undefined) stats.push({ label: "費用", value: `$${cost.toFixed(4)}` });
  const dur = num(value.duration_ms);
  if (dur !== undefined) stats.push({ label: "所要時間", value: fmtDurationMs(dur) });
  const api = num(value.duration_api_ms);
  if (api !== undefined) stats.push({ label: "API 時間", value: fmtDurationMs(api) });
  const turns = num(value.num_turns);
  if (turns !== undefined) stats.push({ label: "turn", value: fmtInt(turns) });
  const usage = isObj(value.usage) ? value.usage : undefined;
  if (usage) {
    for (const [key, label] of [
      ["input_tokens", "入力"],
      ["output_tokens", "出力"],
      ["cache_read_input_tokens", "キャッシュ読み"],
      ["cache_creation_input_tokens", "キャッシュ書き"],
    ] as const) {
      const n = num(usage[key]);
      if (n !== undefined) stats.push({ label: `${label} tokens`, value: fmtInt(n) });
    }
  }
  if (isObj(value.modelUsage)) {
    const models = Object.keys(value.modelUsage);
    if (models.length > 0) stats.push({ label: "モデル", value: models.join(", ") });
  }
  const isError = typeof value.is_error === "boolean" ? value.is_error : value.subtype !== "success";
  const subtype = str(value.subtype);
  const ev: RunLogEvent = {
    kind: "usage",
    title: isError ? `終了（${subtype ?? "error"}）` : "終了",
    stats,
    isError,
    raw: [],
  };
  const text = str(value.result);
  if (text) ev.text = text;
  return ev;
}

function claudeSystem(value: Obj): RunLogEvent {
  const subtype = str(value.subtype) ?? "system";
  switch (subtype) {
    case "init": {
      const model = str(value.model);
      const cwd = str(value.cwd);
      const tools = Array.isArray(value.tools) ? value.tools.length : undefined;
      const parts = [model, cwd, tools !== undefined ? `tools ${tools}` : undefined].filter(Boolean);
      return { kind: "system", label: "セッション開始", detail: parts.join(" · "), count: 1, raw: [] };
    }
    case "thinking_tokens":
      return {
        kind: "system",
        label: "思考中",
        detail: num(value.estimated_tokens) !== undefined ? `約 ${value.estimated_tokens} tokens` : undefined,
        count: 1,
        raw: [],
      };
    case "hook_started":
    case "hook_response":
      return {
        kind: "system",
        label: "hook",
        detail: [str(value.hook_name), str(value.outcome)].filter(Boolean).join(" "),
        count: 1,
        raw: [],
      };
    case "task_started":
    case "task_progress":
    case "task_updated":
    case "task_notification": {
      const status = str(value.status) ?? (isObj(value.patch) ? str(value.patch.status) : undefined);
      const label =
        subtype === "task_started"
          ? "サブタスク開始"
          : subtype === "task_notification" && status === "completed"
            ? "サブタスク完了"
            : subtype === "task_notification"
              ? "サブタスク通知"
              : "サブタスク進行";
      const detail = [str(value.description), status].filter(Boolean).join(" — ");
      return { kind: "system", label, detail: detail || undefined, count: 1, raw: [] };
    }
    case "permission_denied":
      return {
        kind: "error",
        text: `権限で拒否: ${str(value.tool_name) ?? ""}`,
        detail: str(value.message),
        raw: [],
      };
    default:
      return { kind: "system", label: `system: ${subtype}`, count: 1, raw: [] };
  }
}

function adaptClaude(b: Builder, value: Obj, line: string): boolean {
  const type = value.type;
  if (type === "assistant" || type === "user") {
    const message = value.message;
    if (!isObj(message)) return false;
    const content = message.content;
    const role = type;
    if (typeof content === "string") {
      b.push({ kind: "message", role, text: content, raw: [line] });
      return true;
    }
    if (!Array.isArray(content)) return false;
    let emitted = false;
    for (const item of content) {
      if (!isObj(item)) continue;
      if (item.type === "text" && typeof item.text === "string") {
        if (role === "user" && value.isSynthetic !== true && item.text.length === 0) continue;
        b.push({ kind: "message", role, text: item.text, raw: [line] });
        emitted = true;
      } else if (item.type === "thinking") {
        b.push({ kind: "thinking", text: str(item.thinking) ?? "", raw: [line] });
        emitted = true;
      } else if (item.type === "redacted_thinking") {
        b.push({ kind: "thinking", text: "", raw: [line] });
        emitted = true;
      } else if (item.type === "tool_use" && typeof item.name === "string") {
        const ev: RunLogEvent = {
          kind: "tool",
          id: str(item.id),
          name: item.name,
          summary: summarizeToolInput(item.name, item.input),
          input: item.input,
          diffs: claudeToolDiffs(item.name, item.input),
          pending: true,
          raw: [line],
        };
        b.push(ev, str(item.id));
        emitted = true;
      } else if (item.type === "tool_result") {
        const target = b.find(str(item.tool_use_id));
        const text = toolResultText(item.content);
        const isError = item.is_error === true;
        if (target?.kind === "tool") {
          target.result = text;
          target.isError = isError;
          target.pending = false;
          target.diffs = claudeResultDiffs(value.tool_use_result) ?? target.diffs;
          if (!target.raw.includes(line)) target.raw.push(line);
        } else {
          b.push({ kind: "tool", name: "ツールの結果", result: text, isError, pending: false, raw: [line] });
        }
        emitted = true;
      }
    }
    if (!emitted) b.push({ kind: "system", label: `${type}（本文なし）`, count: 1, raw: [line] });
    return true;
  }
  if (type === "system") {
    const ev = claudeSystem(value);
    ev.raw = [line];
    b.push(ev);
    return true;
  }
  if (type === "result") {
    const ev = claudeUsage(value);
    ev.raw = [line];
    b.push(ev);
    return true;
  }
  if (type === "rate_limit_event") {
    const info = isObj(value.rate_limit_info) ? value.rate_limit_info : {};
    b.push({
      kind: "system",
      label: "rate limit",
      detail: [str(info.status), str(info.rateLimitType)].filter(Boolean).join(" · "),
      count: 1,
      raw: [line],
    });
    return true;
  }
  if (type === "tool_progress") {
    b.push({
      kind: "system",
      label: `${str(value.tool_name) ?? "ツール"} 実行中`,
      detail: num(value.elapsed_time_seconds) !== undefined ? `${value.elapsed_time_seconds} 秒経過` : undefined,
      count: 1,
      raw: [line],
    });
    return true;
  }
  if (type === "stream_event") {
    b.push({ kind: "system", label: "stream_event", count: 1, raw: [line] });
    return true;
  }
  return false;
}

// ---------------------------------------------------------------------------------------------
// codex（exec --json）
// ---------------------------------------------------------------------------------------------

function codexErrorText(error: unknown, fallback: string): string {
  if (typeof error === "string") return error;
  if (isObj(error) && typeof error.message === "string") return error.message;
  return fallback;
}

function codexItem(b: Builder, item: Obj, line: string): void {
  const id = str(item.id);
  const itemType = str(item.type) ?? "item";
  const existing = b.find(id);
  if (existing && !existing.raw.includes(line)) existing.raw.push(line);
  switch (itemType) {
    case "agent_message":
    case "reasoning": {
      const text = str(item.text) ?? str(item.message) ?? str(item.content) ?? "";
      if (existing && (existing.kind === "message" || existing.kind === "thinking")) {
        existing.text = text;
        return;
      }
      b.push(
        itemType === "reasoning"
          ? { kind: "thinking", text, raw: [line] }
          : { kind: "message", role: "assistant", text, raw: [line] },
        id,
      );
      return;
    }
    case "command_execution": {
      const command = str(item.command) ?? "";
      const output = str(item.aggregated_output);
      const exitCode = item.exit_code === null ? null : num(item.exit_code);
      const status = str(item.status);
      if (existing?.kind === "command") {
        existing.command = command || existing.command;
        if (output !== undefined) existing.output = output;
        existing.exitCode = exitCode;
        existing.status = status;
        return;
      }
      b.push({ kind: "command", id, command, output, exitCode, status, raw: [line] }, id);
      return;
    }
    case "file_change": {
      const diffs: FileDiff[] = Array.isArray(item.changes)
        ? item.changes.filter(isObj).map((c) => {
            const diff = str(c.diff) ?? str(c.unified_diff);
            return {
              path: str(c.path) ?? "",
              kind: str(c.kind),
              hunks: diff
                ? [{ lines: diff.split("\n").filter((l) => !l.startsWith("---") && !l.startsWith("+++")) }]
                : [],
            };
          })
        : [];
      if (existing?.kind === "file_change") {
        existing.diffs = diffs;
        existing.status = str(item.status);
        return;
      }
      b.push({ kind: "file_change", id, diffs, status: str(item.status), raw: [line] }, id);
      return;
    }
    case "error": {
      b.push({ kind: "error", text: str(item.message) ?? "error", raw: [line] });
      return;
    }
    default: {
      // web_search / mcp_tool_call / todo_list 等はツール呼び出しとして出す。
      const name = itemType === "mcp_tool_call" ? `${str(item.server) ?? "mcp"}.${str(item.tool) ?? "tool"}` : itemType;
      const summary =
        str(item.query) ??
        (isObj(item.action) && Array.isArray(item.action.queries) ? item.action.queries.join(" / ") : undefined) ??
        summarizeToolInput(name, item.arguments ?? item);
      const status = str(item.status);
      const result =
        item.result !== undefined
          ? toolResultText(isObj(item.result) ? (item.result.content ?? item.result) : item.result)
          : undefined;
      const isError = status === "failed" || (item.error !== undefined && item.error !== null);
      if (existing?.kind === "tool") {
        existing.summary = summary ? oneLine(summary) : existing.summary;
        existing.input = item;
        if (result !== undefined) existing.result = result;
        existing.isError = isError;
        existing.pending = status === "in_progress";
        return;
      }
      b.push(
        {
          kind: "tool",
          id,
          name,
          summary: summary ? oneLine(summary) : undefined,
          input: item,
          result,
          isError,
          pending: status === "in_progress",
          raw: [line],
        },
        id,
      );
    }
  }
}

function adaptCodex(b: Builder, value: Obj, line: string): boolean {
  const type = str(value.type);
  if (!type) return false;
  if (type.startsWith("item.")) {
    if (!isObj(value.item)) return false;
    codexItem(b, value.item, line);
    return true;
  }
  if (type === "thread.started") {
    b.push({ kind: "system", label: "セッション開始", detail: str(value.thread_id), count: 1, raw: [line] });
    return true;
  }
  if (type === "turn.started") {
    b.push({ kind: "system", label: "turn 開始", count: 1, raw: [line] });
    return true;
  }
  if (type === "turn.completed") {
    const stats: UsageStat[] = [];
    const u = isObj(value.usage) ? value.usage : {};
    for (const [key, label] of [
      ["input_tokens", "入力"],
      ["cached_input_tokens", "キャッシュ読み"],
      ["output_tokens", "出力"],
      ["reasoning_output_tokens", "推論"],
    ] as const) {
      const n = num(u[key]);
      if (n !== undefined) stats.push({ label: `${label} tokens`, value: fmtInt(n) });
    }
    b.push({ kind: "usage", title: "turn 完了", stats, isError: false, raw: [line] });
    return true;
  }
  if (type === "turn.failed") {
    b.push({ kind: "error", text: codexErrorText(value.error, "turn.failed"), raw: [line] });
    return true;
  }
  if (type === "error") {
    b.push({ kind: "error", text: str(value.message) ?? "error", raw: [line] });
    return true;
  }
  return false;
}

// ---------------------------------------------------------------------------------------------
// ACP（opencode 等。JSON-RPC 2.0）
// ---------------------------------------------------------------------------------------------

function acpContentText(content: unknown): string {
  if (isObj(content)) {
    if (typeof content.text === "string") return content.text;
    if (isObj(content.content)) return acpContentText(content.content);
  }
  if (Array.isArray(content)) return content.map(acpContentText).join("\n");
  return typeof content === "string" ? content : "";
}

function acpToolContent(content: unknown): { text?: string; diffs?: FileDiff[] } {
  if (!Array.isArray(content)) return {};
  const texts: string[] = [];
  const diffs: FileDiff[] = [];
  for (const c of content) {
    if (!isObj(c)) continue;
    if (c.type === "diff") {
      diffs.push({
        path: str(c.path) ?? "",
        kind: c.oldText === null || c.oldText === undefined ? "add" : "update",
        hunks: [simpleDiff(str(c.oldText) ?? "", str(c.newText) ?? "")],
      });
    } else {
      const t = acpContentText(c);
      if (t) texts.push(t);
    }
  }
  return { text: texts.length > 0 ? texts.join("\n") : undefined, diffs: diffs.length > 0 ? diffs : undefined };
}

/** 表示する本文を持たない ACP の `sessionUpdate`（設定・コマンド一覧の通知）。これ以外の知らないものは `unknown`。 */
const ACP_SYSTEM_UPDATES = new Set([
  "available_commands_update",
  "config_option_update",
  "current_mode_update",
  "session_info_update",
  "usage_update",
]);

function adaptAcp(b: Builder, value: Obj, line: string): boolean {
  if (value.jsonrpc !== "2.0") return false;
  if (isObj(value.error)) {
    const data = value.error.data;
    b.push({
      kind: "error",
      text: str(value.error.message) ?? "JSON-RPC error",
      detail: data !== undefined ? JSON.stringify(data) : undefined,
      raw: [line],
    });
    return true;
  }
  const method = str(value.method);
  if (method === "session/update" && isObj(value.params) && isObj(value.params.update)) {
    const u = value.params.update;
    const kind = str(u.sessionUpdate) ?? "update";
    const last = b.events.at(-1);
    if (kind === "agent_message_chunk" || kind === "user_message_chunk") {
      const text = acpContentText(u.content);
      const role = kind === "agent_message_chunk" ? "assistant" : "user";
      if (last?.kind === "message" && last.role === role) {
        last.text += text;
        last.raw.push(line);
      } else b.push({ kind: "message", role, text, raw: [line] });
      return true;
    }
    if (kind === "agent_thought_chunk") {
      const text = acpContentText(u.content);
      if (last?.kind === "thinking") {
        last.text += text;
        last.raw.push(line);
      } else b.push({ kind: "thinking", text, raw: [line] });
      return true;
    }
    if (kind === "tool_call" || kind === "tool_call_update") {
      const id = str(u.toolCallId);
      const existing = b.find(id);
      const { text, diffs } = acpToolContent(u.content);
      const status = str(u.status);
      const title = str(u.title);
      const rawOutput = u.rawOutput !== undefined ? toolResultText(u.rawOutput) : undefined;
      if (existing?.kind === "tool") {
        if (!existing.raw.includes(line)) existing.raw.push(line);
        if (title) existing.summary = oneLine(title);
        if (u.rawInput !== undefined) existing.input = u.rawInput;
        if (text !== undefined || rawOutput !== undefined) existing.result = text ?? rawOutput;
        if (diffs) existing.diffs = diffs;
        if (status) {
          existing.pending = status === "pending" || status === "in_progress";
          existing.isError = status === "failed";
        }
        return true;
      }
      b.push(
        {
          kind: "tool",
          id,
          name: str(u.kind) ?? "tool",
          summary: title ? oneLine(title) : summarizeToolInput("", u.rawInput),
          input: u.rawInput,
          result: text ?? rawOutput,
          diffs,
          isError: status === "failed",
          pending: status !== "completed" && status !== "failed",
          raw: [line],
        },
        id,
      );
      return true;
    }
    if (kind === "plan" && Array.isArray(u.entries)) {
      const text = u.entries
        .filter(isObj)
        .map((e) => `- [${e.status === "completed" ? "x" : " "}] ${str(e.content) ?? ""}`)
        .join("\n");
      b.push({ kind: "message", role: "assistant", text: `**計画**\n\n${text}`, raw: [line] });
      return true;
    }
    if (ACP_SYSTEM_UPDATES.has(kind)) {
      b.push({ kind: "system", label: `ACP: ${kind}`, count: 1, raw: [line] });
    } else {
      b.push({ kind: "unknown", label: `ACP: ${kind}`, raw: [line] });
    }
    return true;
  }
  if (method) {
    b.push({ kind: "system", label: `ACP 要求: ${method}`, count: 1, raw: [line] });
    return true;
  }
  if ("result" in value) {
    const r = value.result;
    if (isObj(r) && typeof r.stopReason === "string") {
      const stats: UsageStat[] = [{ label: "stopReason", value: r.stopReason }];
      if (isObj(r.usage)) {
        for (const [k, v] of Object.entries(r.usage))
          if (typeof v === "number") stats.push({ label: k, value: fmtInt(v) });
      }
      b.push({ kind: "usage", title: "prompt 完了", stats, isError: false, raw: [line] });
      return true;
    }
    const agent =
      isObj(r) && isObj(r.agentInfo) ? [str(r.agentInfo.name), str(r.agentInfo.version)].filter(Boolean).join(" ") : "";
    const session = isObj(r) ? str(r.sessionId) : undefined;
    b.push({
      kind: "system",
      label: isObj(r) && "protocolVersion" in r ? "ACP 初期化" : session ? "ACP セッション" : "ACP 応答",
      detail: agent || session,
      count: 1,
      raw: [line],
    });
    return true;
  }
  return false;
}

// ---------------------------------------------------------------------------------------------
// 入口
// ---------------------------------------------------------------------------------------------

/**
 * `stdout.jsonl` の行（末尾の改行を除いた配列）をイベント列にする。空行は飛ばす。
 * どの adapter にも当てはまらない行（JSON でない行・知らない `type`・celeris 独自ワーカープロトコル）は `unknown`。
 */
export function parseRunLog(lines: readonly string[]): RunLogEvent[] {
  const b = new Builder();
  for (const line of lines) {
    if (line.trim().length === 0) continue;
    let parsed: unknown;
    try {
      parsed = JSON.parse(line);
    } catch {
      b.push({ kind: "unknown", label: "テキスト", raw: [line] });
      continue;
    }
    if (!isObj(parsed)) {
      b.push({ kind: "unknown", label: "JSON", raw: [line] });
      continue;
    }
    const handled = adaptAcp(b, parsed, line) || adaptCodex(b, parsed, line) || adaptClaude(b, parsed, line);
    if (!handled) {
      const type = str(parsed.type);
      b.push({ kind: "unknown", label: type ?? "JSON", raw: [line] });
    }
  }
  return b.events;
}

/** 表示の見出しに使う、種類ごとの件数。 */
export function countByKind(events: readonly RunLogEvent[]): Partial<Record<RunLogEventKind, number>> {
  const out: Partial<Record<RunLogEventKind, number>> = {};
  for (const ev of events) out[ev.kind] = (out[ev.kind] ?? 0) + 1;
  return out;
}

/** 元の行を人が読める JSON に（JSON でない行はそのまま）。 */
export function prettyRaw(raw: readonly string[]): string {
  return raw
    .map((l) => {
      try {
        return JSON.stringify(JSON.parse(l), null, 2);
      } catch {
        return l;
      }
    })
    .join("\n");
}
