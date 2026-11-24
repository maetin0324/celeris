import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import type {
  ProjectDetail,
  ProjectOverview,
  ProjectOverviewEntry,
  ProjectOverviewLink,
  ProjectOverviewMark,
  ProjectOverviewWarning,
} from "../../api/generated/types";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { Badge, type BadgeTone } from "../../components/ui/badge";
import { StatusBadge } from "../../components/ui/status-badge";
import { projectOverviewQuery } from "./project-queries";

// 案件の概要の panel（ADR 2026-11-23-project-observability-publish D1・D4）。`GET /projects/{id}/overview` の 14 項目を
// 決めた順で並べ、印（確認済み・推定・要判断・矛盾）を文字と色で出し、強い警告を先頭にまとめる。
// 概要に無い欄（Project.request の本文・task の status と題・予算の上限・Roadmap・task の木）は写さず、`GET /projects/{id}` の
// detail と同じ画面の既存の panel（概要・自動運営・途中目標と仕事・成果物）から出す。状態は持たない（読むだけ）。

/** D1.3 の 14 項目（key と表示）。応答の `items` もこの順。 */
export const overviewItems = [
  ["original_goal", "原目的"],
  ["approved_strategy", "承認戦略"],
  ["candidates", "候補の比較"],
  ["hypotheses", "仮説"],
  ["verification", "検証の状況"],
  ["refutations", "反証"],
  ["latest_findings", "最新に得た知見"],
  ["discussion_sources", "議論の出典"],
  ["experiments_and_tasks", "現在の実験と Task"],
  ["budget_spent", "予算と spent"],
  ["next_verification", "次の検証"],
  ["pending_human_decisions", "人の判断待ち"],
  ["risks", "危険"],
  ["publish_readiness", "公開の準備度"],
] as const;

const itemLabel = new Map<string, string>(overviewItems);

export const markViews: Record<ProjectOverviewMark, { label: string; tone: BadgeTone }> = {
  confirmed: { label: "確認済み", tone: "success" },
  estimated: { label: "推定", tone: "info" },
  needs_decision: { label: "要判断", tone: "warning" },
  contradicted: { label: "矛盾・反証", tone: "danger" },
};

/** 警告の code の文（D1.2: 応答は文を持たず、Web が code から組む）。知らない code は code のまま出す。 */
const warningLabels: Record<string, string> = {
  needs_decision: "人の判断が要る",
  contradicted: "矛盾・反証・失効がある",
  strategy_stale: "承認した戦略が古い（原目的・Roadmap の版の不一致、より新しい承認、file の上書き）",
  refute_unapplied: "反証が戦略に反映されていない",
  evaluation_unsupported: "評価の主張に根拠が無い",
  evaluation_off_track: "評価が親の目的から外れていると判定した",
  evaluation_invalid: "評価の記録が無効",
  citation_stale: "議論の引用が編集・撤回された",
  budget_exceeded: "消費が予算を超えた",
  cost_unknown: "単価の分からない model の run がある",
  signoff_stale: "署名の後に manifest が変わった",
  manifest_invalid: "manifest が検査を通らない",
  manifest_blocking_question: "manifest に公開を止める未解決の問いがある",
  manifest_confidential: "manifest に機密の資料が含まれる",
};

export function warningText(code: string): string {
  return warningLabels[code] ?? code;
}

export function strongWarnings(overview: ProjectOverview): ProjectOverviewWarning[] {
  return overview.warnings.filter((warning) => warning.strong);
}

type Detail = Record<string, unknown>;

function asDetail(value: unknown): Detail {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? (value as Detail) : {};
}

function text(value: unknown): string {
  if (value === null || value === undefined) return "";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  return JSON.stringify(value);
}

function count(value: unknown): number {
  return Array.isArray(value) ? value.length : 0;
}

const axisLabels: Record<string, string> = { execution: "実行", epistemic: "知見", goal: "目的（評価の値）" };

/** entry の detail（項目ごとに形が違う JSON）を 1 行の文にする。読む欄は ADR D1.3 の「概要が返すもの」。 */
export function entrySummary(key: string, raw: unknown): string {
  const d = asDetail(raw);
  switch (key) {
    case "original_goal":
      return `Project.request の sha256 ${text(d.request_sha256).slice(0, 16)}`;
    case "approved_strategy": {
      const approved = asDetail(d.approved);
      const pending = Array.isArray(d.pending_versions) ? d.pending_versions.map(text).join("・") : "";
      const head = d.approved
        ? `${text(d.strategy_id)} v${text(approved.version)}（承認: ${text(approved.approved_by)}）`
        : `${text(d.strategy_id)} は承認済みの版が無い`;
      const stale = count(d.stale) > 0 ? `、古い理由: ${(d.stale as unknown[]).map(text).join("・")}` : "";
      return `${head}${pending ? `、回答待ち v${pending}` : ""}${stale}`;
    }
    case "candidates":
      return `${text(d.title)}（${text(d.state)}${d.selected ? "・選択" : ""}）、実験の task ${count(d.experiment_tasks)} 件`;
    case "hypotheses":
    case "next_verification":
      return text(d.statement);
    case "verification":
      return `${text(d.hypothesis)}: 支持 ${text(d.support)}・反証 ${text(d.refute)}・未確認 ${text(d.unknown)}、識別実験の task ${count(d.experiment_tasks)} 件`;
    case "refutations":
      return `${text(d.hypothesis)}: ${text(d.statement)}（参照 ${text(d.references)} 件）`;
    case "latest_findings":
      if ("verdict" in d) return `判定 ${text(d.verdict)}・次 ${text(d.next)}（記録の状態 ${text(d.status)}）`;
      if ("axis" in d) return `${axisLabels[text(d.axis)] ?? text(d.axis)}の軸: ${text(d.value)}`;
      if ("negative_result" in d) return `負の結果: ${text(d.negative_result)}`;
      return `${text(d.statement)}（${d.supported ? "根拠あり" : "根拠無し"}）`;
    case "discussion_sources":
      if ("cited_message_id" in d) return `引用 ${text(d.cited_message_id)} r${text(d.revision)}: ${text(d.state)}`;
      return `${text(d.source_label) || text(d.author)}${d.untrusted ? "（外部の文・untrusted）" : ""}、採否 ${text(d.review_state) || "無し"}`;
    case "experiments_and_tasks":
      return `${text(d.strategy_name)} の候補 ${text(d.candidate) || "—"}`;
    case "budget_spent": {
      const budget = asDetail(d.budget);
      const spent = asDetail(d.spent);
      return `予算 ${text(budget.usd_max) || "—"} USD・${text(budget.wall_hours_max) || "—"} 時間 / 消費 ${text(spent.tokens)} token（確認済み）・${text(spent.list_price_usd)} USD（定価の換算、推定）`;
    }
    case "pending_human_decisions":
      return `${text(d.key_kind)} ${text(d.decision_id ?? d.message_id)}`;
    case "risks": {
      const rows = Array.isArray(d.by_code) ? d.by_code.map(asDetail) : [];
      return rows.map((row) => `${warningText(text(row.code))} ${text(row.count)} 件`).join("、") || "警告 0 件";
    }
    case "publish_readiness": {
      const violations = Array.isArray(d.violations) ? d.violations.map(text).join("・") : "";
      return `${text(d.manifest_id)} v${text(d.version)}・署名 ${text(d.signoff) || "無し"}${violations ? `・違反 ${violations}` : ""}`;
    }
    default:
      return text(raw);
  }
}

export function MarkBadge({ mark }: { mark: ProjectOverviewMark }) {
  const view = markViews[mark];
  return (
    <Badge tone={view.tone} data-mark={mark} className="whitespace-nowrap break-normal">
      {view.label}
    </Badge>
  );
}

const linkClass = "inline-flex min-h-11 min-w-11 items-center break-all text-primary underline";

/** D1.4 の 7 種の link を既存の画面へ移す。外部の URL は https だけを新しい tab で開き、Celeris は取りに行かない。 */
export function OverviewLink({ link }: { link: ProjectOverviewLink }) {
  switch (link.kind) {
    case "task":
      return (
        <Link className={linkClass} to="/tasks/$id" params={{ id: link.task_id }} data-link="task">
          task {link.task_id}
        </Link>
      );
    case "artifact":
    case "notebook":
      return (
        <span className="inline-flex flex-wrap items-center gap-2">
          <Link
            className={linkClass}
            to="/tasks/$id"
            params={{ id: link.task_id }}
            search={{ tab: "artifacts" }}
            data-link={link.kind}
          >
            {link.kind === "notebook" ? "Notebook" : "成果物"} {link.name}
          </Link>
          {link.kind === "notebook" ? (
            <Link className={linkClass} to="/tasks/$id/files" params={{ id: link.task_id }} data-link="files">
              作業ツリー
            </Link>
          ) : null}
        </span>
      );
    case "log":
      return (
        <Link
          className={linkClass}
          to="/tasks/$id/runs/$runId"
          params={{ id: link.task_id, runId: link.run_id }}
          data-link="log"
        >
          log {link.run_id}
        </Link>
      );
    case "git":
      return link.task_id ? (
        <Link className={linkClass} to="/tasks/$id/changes" params={{ id: link.task_id }} data-link="git">
          Git {link.repo}@{link.sha.slice(0, 12)}
        </Link>
      ) : (
        <code className="break-all" data-link="git">
          {link.repo} {link.sha}
        </code>
      );
    case "literature":
      if (link.url?.startsWith("https://"))
        return (
          <a className={linkClass} href={link.url} target="_blank" rel="noopener noreferrer" data-link="literature">
            文献（外部・untrusted） {link.url}
          </a>
        );
      // knowledge の page へ path で直接移る route は無い（ADR D1.4 の未確認）。path を写せる文字で出し、検索の画面へ移す。
      return (
        <span className="inline-flex flex-wrap items-center gap-2" data-link="literature">
          <code className="break-all">{link.path ?? link.url ?? ""}</code>
          <Link className={linkClass} to="/knowledge">
            知識を検索
          </Link>
        </span>
      );
    case "chat":
      return (
        <Link className={linkClass} to="/" search={{ thread: link.thread_id }} data-link="chat">
          chat {link.message_id} r{link.revision}
          {link.state && link.state !== "current" ? `（引用: ${link.state}）` : ""}
        </Link>
      );
  }
}

function Links({ links }: { links: ProjectOverviewLink[] }) {
  if (links.length === 0) return null;
  return (
    <span className="flex flex-wrap items-center gap-x-3">
      {links.map((link, index) => (
        // biome-ignore lint/suspicious/noArrayIndexKey: link は同じ entry の中で順が固定（応答の順）
        <OverviewLink key={index} link={link} />
      ))}
    </span>
  );
}

/** O14 の entry から manifest の画面（/tasks/:id/publish/:manifestId、D4）へ移る。版は entry の版に固定する。 */
function ManifestLink({ detail }: { detail: Detail }) {
  const taskId = text(detail.task_id);
  const manifestId = text(detail.manifest_id);
  const version = typeof detail.version === "number" ? detail.version : undefined;
  if (!taskId || !manifestId) return null;
  return (
    <Link
      className={linkClass}
      to="/tasks/$id/publish/$manifestId"
      params={{ id: taskId, manifestId }}
      search={{ version }}
      data-link="manifest"
    >
      manifest {manifestId}
      {version === undefined ? "" : ` v${version}`} を読む
    </Link>
  );
}

function EntryRow({
  itemKey,
  entry,
  taskStatus,
}: {
  itemKey: string;
  entry: ProjectOverviewEntry;
  taskStatus: Map<string, string>;
}) {
  const taskId = itemKey === "experiments_and_tasks" ? text(asDetail(entry.detail).task_id) : "";
  const status = taskId ? taskStatus.get(taskId) : undefined;
  return (
    <li className="space-y-1 break-words" data-entry={entry.id} data-mark={entry.mark}>
      <p className="flex flex-wrap items-center gap-2">
        <MarkBadge mark={entry.mark} />
        {status ? <StatusBadge status={status} /> : null}
        <span>{entrySummary(itemKey, entry.detail)}</span>
      </p>
      <Links links={entry.links} />
      {itemKey === "publish_readiness" ? <ManifestLink detail={asDetail(entry.detail)} /> : null}
    </li>
  );
}

export function ProjectOverviewPanel({ overview, detail }: { overview: ProjectOverview; detail: ProjectDetail }) {
  const strong = strongWarnings(overview);
  const items = new Map(overview.items.map((item) => [item.key, item]));
  // task の status は概要に写さず、`GET /projects/{id}` の tasks[] から引く（D1.1）。
  const taskStatus = new Map(detail.tasks.map((task) => [task.id, task.status]));
  return (
    <div className="min-w-0 space-y-3" data-testid="project-overview-panel">
      <section
        aria-label="強い警告"
        data-testid="overview-strong-warnings"
        data-count={strong.length}
        className={`space-y-2 rounded-md border p-3 ${strong.length > 0 ? "border-destructive" : "border-border"}`}
      >
        <h3 className="font-semibold">強い警告 {strong.length} 件</h3>
        {strong.length === 0 ? (
          <p className="text-muted-foreground">強い警告はありません。</p>
        ) : (
          <ul className="space-y-2">
            {strong.map((warning, index) => (
              <li
                // biome-ignore lint/suspicious/noArrayIndexKey: 同じ code・entry の警告が並びうるので順を key に含める
                key={`${warning.item}/${warning.code}/${warning.entry ?? ""}/${index}`}
                className="space-y-1 break-words"
                data-warning={warning.code}
                data-warning-item={warning.item}
              >
                <p className="text-destructive">
                  <span className="font-semibold">強い警告</span>（{itemLabel.get(warning.item) ?? warning.item}）:{" "}
                  {warningText(warning.code)}
                  {warning.entry ? <span className="text-muted-foreground"> — {warning.entry}</span> : null}
                </p>
                <Links links={warning.links} />
              </li>
            ))}
          </ul>
        )}
      </section>
      <ol className="space-y-3" aria-label="案件の概要の 14 項目">
        {overviewItems.map(([key, label], index) => {
          const item = items.get(key);
          const truncated = overview.truncated.includes(key);
          return (
            <li key={key} className="space-y-2 rounded-md border border-border p-3" data-item={key}>
              <h3 className="flex flex-wrap items-center gap-2 font-semibold">
                <span>
                  O{index + 1} {label}
                </span>
                {item?.recorded && item.mark ? <MarkBadge mark={item.mark} /> : null}
                {!item?.recorded ? <span className="font-normal text-muted-foreground">記録無し</span> : null}
              </h3>
              {key === "original_goal" ? (
                <p className="text-label text-muted-foreground">本文は「概要」の節（Project.request の正本）。</p>
              ) : null}
              {key === "budget_spent" ? (
                <p className="text-label text-muted-foreground">
                  案件の予算の上限と root の消費は「自動運営」の節、task の木は「途中目標と仕事」の節。
                </p>
              ) : null}
              {item && item.entries.length > 0 ? (
                <ul className="space-y-2">
                  {item.entries.map((entry) => (
                    <EntryRow key={entry.id} itemKey={key} entry={entry} taskStatus={taskStatus} />
                  ))}
                </ul>
              ) : null}
              {truncated ? (
                <p className="text-label text-muted-foreground">新しい順に 50 件だけ出しています。</p>
              ) : null}
            </li>
          );
        })}
      </ol>
    </div>
  );
}

export function ProjectOverviewSection({ detail }: { detail: ProjectDetail }) {
  const overview = useQuery(projectOverviewQuery(detail.project.id));
  return (
    <FetchFrame query={overview} subject="案件の概要">
      {overview.data ? <ProjectOverviewPanel overview={overview.data} detail={detail} /> : null}
    </FetchFrame>
  );
}
