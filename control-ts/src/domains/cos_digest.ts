// cos_digest 領域（境界 ADR 2026-11-02 付記 S4-1〜S4-3、S4-11〜S4-18）。
// 入口は pass ごとの `cosDigestPass(ctx)`（S4-12）。Host API の `cos_digest:list_runs` で digest を待つ run の一覧を読み、
// 一覧の順に `cos_digest:read_run` で run ごとの入力を読み、run ごとの command 列を純粋関数 `decide()` で決めて返す。
// `decide()` は crates/task-core/src/chat/digest.rs の `cos_digest_decide` と同じ command 列（文面・cards・requeue）を
// 同じ文字列・同じ順で返す。DB の更新はしない。返した command は Rust が検証してから apply する（S4-14）。
//
// op の error の扱い（S4-18）:
// - `read_run` の `exception`（Rust の store の読み出しの失敗）は、その run を飛ばす（Rust の参照実装と同じ。規則 8）。
// - それ以外（`denied`・`terminated`・`generation_gone`・`timeout`・`busy` と `list_runs` のすべての error）は throw し、
//   Rust の fallback に任せる。
//
// 入力の扱い:
// - 余計な欄・欠けた欄・型の違いは throw する（Option の欄も null を明示させる）。
// - `run_state`・item の `state`・`source_kind` は Rust でも文字列で、未知の値にも決まった文面を返す
//   （`run は <state> で終わった`・`状態不明`・`項目`）。fixture の参照出力と byte 一致させるため、ここも拒まない。
// - `route` は Rust では文字列だが、書き込む値は `escalation`・`fallback` だけ。それ以外の値は throw する
//   （digest の出力には使わない欄なので、拒んでも一致は崩れない）。
import { cosDigestListRuns, cosDigestReadRun, type RequestContext } from "celeris:host";

export type CosDigestRunState = string;

/** Rust の `CosTriageOption`（欄の順は key・label・detail）。 */
export interface CosTriageOption {
  key: string;
  label: string;
  detail: string | null;
}

/** Rust の `CosTriageDecision`。 */
export interface CosTriageDecision {
  summary: string;
  background: string | null;
  on_answer: string | null;
  options: CosTriageOption[];
  recommended: string | null;
  recommendation_reason: string | null;
  web_path: string;
}

/** Rust の `CosTriageItemReport`。 */
export interface CosTriageItemReport {
  item_id: string;
  thread_id: string;
  source_kind: string;
  source_key: string;
  source_revision: string;
  state: string;
  summary: string;
  detail: string;
  reason: string | null;
  operation_id: string | null;
  run_id: string | null;
  created_at: string;
  route: "escalation" | "fallback" | null;
  decision: CosTriageDecision | null;
}

/** Rust の `CosDigestInput`。 */
export interface CosDigestInput {
  thread: string;
  run_id: string;
  run_state: CosDigestRunState;
  reason: string | null;
  items: CosTriageItemReport[];
}

export type ChatCardKind = "decision" | "question" | "approval" | "plan_gate" | "notice" | "task";

/** Rust の `ChatCard`（欄の順は宣言順）。 */
export interface ChatCard {
  kind: ChatCardKind;
  id: string;
  title: string;
  state: string;
  href: string;
  actor: "cos";
  reason: string | null;
  operation_id: string | null;
}

/** Rust の `CosDigestCommand`（`kind` が先頭、残りは variant の宣言順）。 */
export type CosDigestCommand =
  | { kind: "post_digest"; thread: string; idempotency_key: string; text: string; cards: ChatCard[]; run_id: string }
  | { kind: "requeue_run"; run_id: string };

/** S4-2 の出力 `CosDigestOutput`。 */
export interface CosDigestOutput {
  commands: CosDigestCommand[];
}

/** Rust の `COS_TRIAGE_DIGEST_KEY_PREFIX`。 */
export const COS_TRIAGE_DIGEST_KEY_PREFIX = "cos-triage-digest:";

const ROUTES: readonly string[] = ["escalation", "fallback"];

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** 欄の集合が `fields` と完全に同じ object かを確かめる（余計な欄・欠けた欄を拒む）。 */
function object(v: unknown, where: string, fields: readonly string[]): Record<string, unknown> {
  if (!isPlainObject(v)) throw new Error(`cos_digest: ${where} must be an object`);
  for (const key of Object.keys(v)) {
    if (!fields.includes(key)) throw new Error(`cos_digest: unknown field in ${where}: ${key}`);
  }
  for (const key of fields) {
    if (!Object.hasOwn(v, key)) throw new Error(`cos_digest: missing field in ${where}: ${key}`);
  }
  return v;
}

function string(v: unknown, where: string): string {
  if (typeof v !== "string") throw new Error(`cos_digest: ${where} must be a string`);
  return v;
}

function nullableString(v: unknown, where: string): string | null {
  return v === null ? null : string(v, `${where} (or null)`);
}

function array<T>(v: unknown, where: string, item: (x: unknown, at: string) => T): T[] {
  if (!Array.isArray(v)) throw new Error(`cos_digest: ${where} must be an array`);
  return v.map((x, i) => item(x, `${where}[${i}]`));
}

function option(v: unknown, where: string): CosTriageOption {
  const o = object(v, where, ["key", "label", "detail"]);
  return {
    key: string(o.key, `${where}.key`),
    label: string(o.label, `${where}.label`),
    detail: nullableString(o.detail, `${where}.detail`),
  };
}

function decisionPacket(v: unknown, where: string): CosTriageDecision | null {
  if (v === null) return null;
  const o = object(v, where, [
    "summary",
    "background",
    "on_answer",
    "options",
    "recommended",
    "recommendation_reason",
    "web_path",
  ]);
  return {
    summary: string(o.summary, `${where}.summary`),
    background: nullableString(o.background, `${where}.background`),
    on_answer: nullableString(o.on_answer, `${where}.on_answer`),
    options: array(o.options, `${where}.options`, option),
    recommended: nullableString(o.recommended, `${where}.recommended`),
    recommendation_reason: nullableString(o.recommendation_reason, `${where}.recommendation_reason`),
    web_path: string(o.web_path, `${where}.web_path`),
  };
}

function item(v: unknown, where: string): CosTriageItemReport {
  const o = object(v, where, [
    "item_id",
    "thread_id",
    "source_kind",
    "source_key",
    "source_revision",
    "state",
    "summary",
    "detail",
    "reason",
    "operation_id",
    "run_id",
    "created_at",
    "route",
    "decision",
  ]);
  const route = nullableString(o.route, `${where}.route`);
  if (route !== null && !ROUTES.includes(route)) {
    throw new Error(`cos_digest: unknown value of ${where}.route: ${route}`);
  }
  return {
    item_id: string(o.item_id, `${where}.item_id`),
    thread_id: string(o.thread_id, `${where}.thread_id`),
    source_kind: string(o.source_kind, `${where}.source_kind`),
    source_key: string(o.source_key, `${where}.source_key`),
    source_revision: string(o.source_revision, `${where}.source_revision`),
    state: string(o.state, `${where}.state`),
    summary: string(o.summary, `${where}.summary`),
    detail: string(o.detail, `${where}.detail`),
    reason: nullableString(o.reason, `${where}.reason`),
    operation_id: nullableString(o.operation_id, `${where}.operation_id`),
    run_id: nullableString(o.run_id, `${where}.run_id`),
    created_at: string(o.created_at, `${where}.created_at`),
    route: route as CosTriageItemReport["route"],
    decision: decisionPacket(o.decision, `${where}.decision`),
  };
}

/** 入力を検査して CosDigestInput にする。Rust の deny_unknown_fields と同じく余計な欄を拒み、欠けた欄も拒む。 */
export function parseInput(input: unknown): CosDigestInput {
  const o = object(input, "input", ["thread", "run_id", "run_state", "reason", "items"]);
  return {
    thread: string(o.thread, "input.thread"),
    run_id: string(o.run_id, "input.run_id"),
    run_state: string(o.run_state, "input.run_state"),
    reason: nullableString(o.reason, "input.reason"),
    items: array(o.items, "input.items", item),
  };
}

// Rust の `char::is_whitespace`（Unicode White_Space）。JS の trim は U+FEFF を含み U+0085 を含まないので使わない。
const RUST_WHITESPACE = new Set([
  0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x20, 0x85, 0xa0, 0x1680, 0x2000, 0x2001, 0x2002, 0x2003, 0x2004, 0x2005, 0x2006,
  0x2007, 0x2008, 0x2009, 0x200a, 0x2028, 0x2029, 0x202f, 0x205f, 0x3000,
]);

/** Rust の `str::trim` と同じ。 */
export function rustTrim(s: string): string {
  let start = 0;
  let end = s.length;
  while (start < end && RUST_WHITESPACE.has(s.charCodeAt(start))) start += 1;
  while (end > start && RUST_WHITESPACE.has(s.charCodeAt(end - 1))) end -= 1;
  return s.slice(start, end);
}

/** stopped・interrupted の run の `running` の item は待ち行列に戻す（Rust の `requeues`）。 */
function requeues(runState: string): boolean {
  return runState === "stopped" || runState === "interrupted";
}

/** Rust の `cos_kind_label`。 */
export function kindLabel(sourceKind: string): string {
  switch (sourceKind) {
    case "decision":
      return "決定";
    case "question":
      return "質問";
    case "authorization":
      return "認可";
    case "acceptance_check":
      return "受け入れ検査";
    case "plan_gate":
      return "計画の承認";
    case "phase_gate":
      return "段の承認";
    case "notice":
      return "知らせ";
    default:
      return "項目";
  }
}

/** Rust の `cos_card_kind`。 */
export function cardKind(sourceKind: string): ChatCardKind {
  switch (sourceKind) {
    case "decision":
      return "decision";
    case "question":
      return "question";
    case "authorization":
    case "acceptance_check":
      return "approval";
    case "plan_gate":
    case "phase_gate":
      return "plan_gate";
    case "notice":
      return "notice";
    default:
      return "task";
  }
}

/** Rust の `CosTriageItemReport::title`。 */
function title(item: CosTriageItemReport): string {
  return rustTrim(item.summary) === "" ? `${item.source_kind}:${item.source_key}` : item.summary;
}

/** Rust の `judgment`。 */
function judgment(item: CosTriageItemReport, runState: string): string {
  switch (item.state) {
    case "answered":
      return "代わりに答えた";
    case "observed":
      return "見ただけ（判断は不要）";
    case "escalated":
      return "人に回した";
    case "fallback":
      return "未処理のまま人へ直接通知した（CoS 不在の退避）";
    case "resolved":
      return "元の待ちが閉じていた（何もしていない）";
    case "pending":
      return "中断されたので次の run で扱う";
    case "running":
      return requeues(runState) ? "中断されたので次の run で扱う" : "未処理（人へ直接通知する）";
    default:
      return "状態不明";
  }
}

/** Rust の `options_line`。detail が 1 つでもあれば 1 選択肢 1 行、無ければラベルを " / " でつなぐ。 */
function optionsLine(item: CosTriageItemReport): string | null {
  const d = item.decision;
  if (d === null || d.options.length === 0) return null;
  const label = (o: CosTriageOption) => (d.recommended === o.key ? `${o.label}（推奨）` : o.label);
  if (d.options.some((o) => o.detail !== null)) {
    return d.options.map((o) => (o.detail !== null ? `\n  - ${label(o)}: ${o.detail}` : `\n  - ${label(o)}`)).join("");
  }
  return d.options.map(label).join(" / ");
}

/** Rust の `cos_digest_message`: digest の文面と cards。 */
export function digestMessage(
  runId: string,
  runState: string,
  reason: string | null,
  items: readonly CosTriageItemReport[],
): { text: string; cards: ChatCard[] } {
  let text = `受信箱の一次対応の結果（${items.length} 件`;
  if (runState === "completed") {
    text += ")";
  } else if (runState === "failed") {
    text += "。run は失敗";
    if (reason !== null && rustTrim(reason) !== "") text += `: ${reason}`;
    text += ")";
  } else {
    text += `。run は ${runState} で終わった)`;
  }
  text += "\n";
  const cards: ChatCard[] = [];
  for (const it of items) {
    const t = title(it);
    text += `\n### ${t}（${kindLabel(it.source_kind)}）\n- 判断: ${judgment(it, runState)}\n`;
    const trimmed = it.reason === null ? "" : rustTrim(it.reason);
    text += `- 理由: ${trimmed === "" ? "理由の記録なし" : trimmed}\n`;
    let state = it.state;
    let href = "/inbox";
    let cardReason = it.reason;
    if (it.state === "escalated" || it.state === "fallback") {
      const d = it.decision;
      if (d !== null) {
        if (d.background !== null && d.background !== "") text += `- 背景: ${d.background}\n`;
        if (d.summary !== "") text += `- 人が決めること: ${d.summary}\n`;
        const options = optionsLine(it);
        if (options !== null) text += `- 選択肢: ${options}\n`;
        if (d.recommendation_reason !== null && d.recommendation_reason !== "") {
          text += `- 推奨の理由: ${d.recommendation_reason}\n`;
          cardReason = d.recommendation_reason;
        }
        if (d.on_answer !== null && d.on_answer !== "") text += `- 答えると: ${d.on_answer}\n`;
        if (d.web_path !== "") href = d.web_path;
      }
      // 元の待ちはまだ開いている: card は答えられる状態のまま。
      if (it.state === "fallback") state = "pending";
    } else if (it.state === "running" && requeues(runState)) {
      state = "pending";
    }
    cards.push({
      kind: cardKind(it.source_kind),
      id: it.source_key,
      title: t,
      state,
      href,
      actor: "cos",
      reason: cardReason,
      operation_id: it.operation_id,
    });
  }
  text += `\n(run ${runId})\n`;
  return { text, cards };
}

/** Rust の `cos_digest_decide`: post_digest を常に 1 件、stopped・interrupted ならその後ろに requeue_run を 1 件。 */
export function decide(input: CosDigestInput): CosDigestOutput {
  const { text, cards } = digestMessage(input.run_id, input.run_state, input.reason, input.items);
  const commands: CosDigestCommand[] = [
    {
      kind: "post_digest",
      thread: input.thread,
      idempotency_key: `${COS_TRIAGE_DIGEST_KEY_PREFIX}${input.run_id}`,
      text,
      cards,
      run_id: input.run_id,
    },
  ];
  if (requeues(input.run_state)) commands.push({ kind: "requeue_run", run_id: input.run_id });
  return { commands };
}

/** digest を待つ run 1 つ（Rust の `CosDigestRunRef`）。 */
export interface CosDigestRunRef {
  thread: string;
  run_id: string;
  run_state: string;
}

/** S4-12 の出力 `CosDigestPassOutput` の 1 run。 */
export interface CosDigestPassRun {
  run_id: string;
  commands: CosDigestCommand[];
}

/** S4-12 の出力 `CosDigestPassOutput`（欄の順は Rust の宣言順）。 */
export interface CosDigestPassOutput {
  runs: CosDigestPassRun[];
}

function runRef(v: unknown, where: string): CosDigestRunRef {
  const o = object(v, where, ["thread", "run_id", "run_state"]);
  return {
    thread: string(o.thread, `${where}.thread`),
    run_id: string(o.run_id, `${where}.run_id`),
    run_state: string(o.run_state, `${where}.run_state`),
  };
}

/** `cos_digest:list_runs` の応答を検査する（余計な欄・欠けた欄を拒む）。 */
export function parseListReply(reply: unknown): { runs: CosDigestRunRef[] } {
  const o = object(reply, "list_runs reply", ["runs"]);
  return { runs: array(o.runs, "list_runs reply.runs", runRef) };
}

/** Host API の HostError で、code が `code` のものか。bundle の外の差し替え（試験の stub）でも同じに判定するため name と code で見る。 */
function isHostError(e: unknown, code: string): boolean {
  return e instanceof Error && e.name === "HostError" && (e as { code?: unknown }).code === code;
}

/**
 * digest pass 全体（付記 S4-12）。list を 1 回読み、一覧の順に run を 1 つずつ read して decide する。
 * 一覧に無い run は読まない。read の `exception` はその run を飛ばし、他の error は throw する。
 */
export async function cosDigestPass(ctx: RequestContext): Promise<CosDigestPassOutput> {
  const list = parseListReply(await cosDigestListRuns(ctx, {}));
  const runs: CosDigestPassRun[] = [];
  for (const ref of list.runs) {
    let reply: unknown;
    try {
      reply = await cosDigestReadRun(ctx, { thread: ref.thread, run_id: ref.run_id });
    } catch (e) {
      if (isHostError(e, "exception")) continue;
      throw e;
    }
    const input = parseInput(reply);
    if (input.thread !== ref.thread || input.run_id !== ref.run_id) {
      throw new Error(`cos_digest: read_run answered ${input.thread}/${input.run_id} for ${ref.thread}/${ref.run_id}`);
    }
    runs.push({ run_id: ref.run_id, commands: decide(input).commands });
  }
  return { runs };
}
