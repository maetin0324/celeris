import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useRouter } from "@tanstack/react-router";
import { type MouseEvent, type ReactNode, useId, useRef, useState } from "react";
import { apiGet } from "../../api/client";
import type { HumanInboxView, InboxItem, InboxKind, InboxOption, ProjectList } from "../../api/generated/types";
import { answerInboxItem, inboxItemsQuery } from "../../api/queries/inbox-notifications";
import { inboxKeys, projectKeys } from "../../api/queries/keys";
import { Markdown } from "../../components/content/markdown";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { ConfirmDialog } from "../../components/ui/confirm-dialog";
import { Section } from "../../components/ui/panel";
import { formatAbsolute, formatRelative, serverNowMs } from "../../lib/time";
import {
  type AnswerOutcome,
  answerFailure,
  INBOX_KINDS,
  isDestructive,
  KIND_LABELS,
  nativeTarget,
  needsNativeScreen,
} from "./inbox-model";

// /inbox（ADR-0133・web ADR 2026-10-04 D4）。判断待ち（GET /inbox/items）だけを 1 行 1 項目で並べ、
// その場で POST /inbox/items/{id}/answer を返す。答えた項目は一覧から消える。専用操作の項目は専用画面へ誘導する。

export type InboxSearch = { project?: string; kind?: InboxKind };

const fieldClass =
  "block min-h-11 w-full rounded-md border border-input bg-surface px-3 py-2 text-body text-foreground focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring";

/** web 内の path へ SPA のまま移る。修飾キー付きの click は既定（新しいタブ等）に任せる。 */
function InternalLink({ href, children, className }: { href: string; children: ReactNode; className?: string }) {
  const router = useRouter();
  function onClick(event: MouseEvent<HTMLAnchorElement>) {
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || event.button !== 0) return;
    event.preventDefault();
    void router.navigate({ href });
  }
  return (
    <a href={href} onClick={onClick} className={className ?? "underline break-words"}>
      {children}
    </a>
  );
}

/** 答えて消えた項目を cache から外す（SSE・再取得を待たずに消す）。件数も合わせる。 */
function withoutItem(view: HumanInboxView | undefined, item: InboxItem): HumanInboxView | undefined {
  if (!view?.items.some((row) => row.id === item.id)) return view;
  const byKind = { ...view.counts.by_kind };
  if (byKind[item.kind]) byKind[item.kind] = Math.max(0, (byKind[item.kind] ?? 0) - 1);
  return {
    ...view,
    items: view.items.filter((row) => row.id !== item.id),
    counts: { ...view.counts, total: Math.max(0, view.counts.total - 1), by_kind: byKind },
  };
}

type Answered = { id: string; title: string; option: string };

function useInboxAnswer(onAnswered: (answer: Answered) => void) {
  const queryClient = useQueryClient();
  const pendingRef = useRef(false);
  const [pendingId, setPendingId] = useState<string | null>(null);
  async function send(item: InboxItem, option: InboxOption, note: string): Promise<AnswerOutcome> {
    if (pendingRef.current) return { ok: false, message: "送信中です。" };
    pendingRef.current = true;
    setPendingId(item.id);
    try {
      const trimmed = note.trim();
      const result = await answerInboxItem(item.id, { option: option.key, ...(trimmed ? { note: trimmed } : {}) });
      if (result.removed) {
        queryClient.setQueriesData<HumanInboxView>({ queryKey: inboxKeys.items() }, (view) => withoutItem(view, item));
        onAnswered({ id: item.id, title: item.title, option: option.label });
      }
      await queryClient.invalidateQueries({ queryKey: inboxKeys.all });
      return { ok: true, removed: result.removed };
    } catch (error) {
      const failure = answerFailure(error);
      if (failure.stale) await queryClient.invalidateQueries({ queryKey: inboxKeys.all });
      return failure;
    } finally {
      pendingRef.current = false;
      setPendingId(null);
    }
  }
  return { send, pendingId };
}

type Sender = ReturnType<typeof useInboxAnswer>;

function Due({ dueAt }: { dueAt?: string | null }) {
  if (!dueAt) return <span className="text-muted-foreground">期限なし</span>;
  const overdue = new Date(dueAt).getTime() < serverNowMs();
  return (
    <span className="inline-flex flex-wrap items-center gap-1">
      <time dateTime={dueAt} title={formatAbsolute(dueAt)}>
        {formatAbsolute(dueAt)}（{formatRelative(dueAt)}）
      </time>
      {overdue ? <Badge tone="danger">期限切れ</Badge> : null}
    </span>
  );
}

function Meta({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex min-w-0 flex-wrap gap-x-1">
      <dt className="text-muted-foreground">{label}:</dt>
      <dd className="min-w-0 break-words">{children}</dd>
    </div>
  );
}

function Blocking({ item, titles }: { item: InboxItem; titles: Map<string, string> }) {
  const { blocking } = item;
  const tasks = blocking.tasks.filter((task) => task.id !== blocking.root?.id);
  return (
    <>
      <Meta label="止めている範囲">
        <span className="inline-flex flex-wrap gap-x-2">
          {blocking.summary ? <span>{blocking.summary}</span> : null}
          {blocking.root ? (
            <Link className="underline" to="/tasks/$id" params={{ id: blocking.root.id }}>
              {blocking.root.title}
            </Link>
          ) : null}
          {tasks.map((task) => (
            <Link key={task.id} className="underline" to="/tasks/$id" params={{ id: task.id }}>
              {task.title}
            </Link>
          ))}
          {blocking.units.length > 0 ? <span>葉 {blocking.units.join("・")}</span> : null}
          {!blocking.summary && !blocking.root && tasks.length === 0 && blocking.units.length === 0 ? (
            <span className="text-muted-foreground">なし</span>
          ) : null}
        </span>
      </Meta>
      {item.blocked_by.length > 0 ? (
        <Meta label="先に答える項目">
          <span className="inline-flex flex-wrap gap-x-2">
            {item.blocked_by.map((id) =>
              titles.has(id) ? (
                <a key={id} className="underline" href={`#item-${encodeURIComponent(id)}`}>
                  {titles.get(id)}
                </a>
              ) : (
                <span key={id}>{id}</span>
              ),
            )}
          </span>
        </Meta>
      ) : null}
    </>
  );
}

function NativeGuide({ item, message }: { item: InboxItem; message?: string }) {
  const target = nativeTarget(item);
  return (
    <div
      role={message ? "alert" : undefined}
      className="flex flex-col gap-2 rounded-md bg-info p-3 text-info-foreground"
    >
      <p>{message ?? "この項目は画面から直接は答えられません。専用の画面で操作してください。"}</p>
      <InternalLink href={target.href} className="inline-flex min-h-11 items-center font-medium underline">
        {target.label}で操作する
      </InternalLink>
    </div>
  );
}

function AnswerForm({ item, sender }: { item: InboxItem; sender: Sender }) {
  const id = useId();
  const noteId = `${id}-note`;
  const errorId = `${id}-error`;
  const [note, setNote] = useState("");
  const [failure, setFailure] = useState<Extract<AnswerOutcome, { ok: false }> | null>(null);
  const [partial, setPartial] = useState(false);
  const pending = sender.pendingId !== null;
  const noteRequired = item.options.filter((option) => option.needs_note).map((option) => option.label);

  async function submit(option: InboxOption): Promise<AnswerOutcome> {
    if (option.needs_note && !note.trim()) {
      const local = {
        ok: false as const,
        message: `「${option.label}」には理由（note）が要ります。`,
        field: "note" as const,
      };
      setFailure(local);
      return local;
    }
    setFailure(null);
    const outcome = await sender.send(item, option, note);
    if (outcome.ok) setPartial(!outcome.removed);
    else setFailure(outcome);
    return outcome;
  }

  if (failure?.native) return <NativeGuide item={item} message={failure.message} />;
  return (
    <div className="flex min-w-0 flex-col gap-2">
      <label htmlFor={noteId} className="text-label font-medium">
        理由・note{noteRequired.length > 0 ? `（${noteRequired.join("・")}は必須）` : "（任意）"}
      </label>
      <textarea
        id={noteId}
        className={`${fieldClass} min-h-11`}
        rows={2}
        value={note}
        onChange={(event) => setNote(event.target.value)}
        aria-invalid={failure?.field === "note" ? true : undefined}
        aria-describedby={failure ? errorId : undefined}
      />
      <ul aria-label="選択肢" className="flex flex-col gap-2">
        {item.options.map((option) => {
          const recommended = item.recommended === option.key;
          const label = `${option.label}${recommended ? "（推奨）" : ""}`;
          const button = isDestructive(option) ? (
            <ConfirmDialog
              trigger={
                <Button variant="destructive" size="sm" disabled={pending}>
                  {label}
                </Button>
              }
              title={`「${option.label}」を選びますか`}
              target={item.title}
              consequence={option.effect || "この判断を確定します。"}
              reversibility="答えた判断は受信箱からは取り消せません。必要なら対象のタスクから操作し直します。"
              followUp={
                item.task
                  ? `タスク「${item.task.title}」の状態で確かめられます。`
                  : "受信箱から消えたことで確かめられます。"
              }
              confirmLabel={`「${item.title}」を${option.label}`}
              onConfirm={async () => {
                const outcome = await submit(option);
                if (!outcome.ok && !outcome.native) throw new Error(outcome.message);
              }}
            />
          ) : (
            <Button
              variant={recommended ? "primary" : "secondary"}
              size="sm"
              disabled={pending}
              onClick={() => void submit(option)}
            >
              {label}
            </Button>
          );
          return (
            <li key={option.key} className="flex min-w-0 flex-col gap-1 sm:flex-row sm:items-center sm:gap-3">
              <span className="shrink-0">{button}</span>
              {option.effect ? <span className="min-w-0 text-label text-muted-foreground">{option.effect}</span> : null}
            </li>
          );
        })}
      </ul>
      {failure ? (
        <p id={errorId} role="alert" className="rounded-md bg-danger p-2 text-danger-foreground">
          {failure.message}
        </p>
      ) : null}
      {partial ? (
        <p role="status" className="text-label text-muted-foreground">
          受け付けました。残りがあるため項目はまだ残っています。
        </p>
      ) : null}
    </div>
  );
}

function InboxRow({
  item,
  sender,
  titles,
  projectTitle,
}: {
  item: InboxItem;
  sender: Sender;
  titles: Map<string, string>;
  projectTitle?: string;
}) {
  const headingId = `item-${item.id}-title`;
  const recommended = item.options.find((option) => option.key === item.recommended);
  return (
    <li
      id={`item-${encodeURIComponent(item.id)}`}
      aria-labelledby={headingId}
      data-inbox-item={item.id}
      className="flex min-w-0 scroll-mt-4 flex-col gap-3 border-b border-border py-4 lg:flex-row lg:gap-6"
    >
      <div className="flex min-w-0 flex-1 flex-col gap-2">
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          <Badge tone={item.kind === "failed" ? "danger" : "info"}>{KIND_LABELS[item.kind] ?? item.kind}</Badge>
          <h3 id={headingId} className="min-w-0 break-words text-body font-semibold">
            {item.title}
          </h3>
        </div>
        <dl className="flex min-w-0 flex-col gap-1 text-label">
          <Meta label="推奨">
            {recommended ? <strong>{recommended.label}</strong> : <span className="text-muted-foreground">なし</span>}
          </Meta>
          <Meta label="期限">
            <Due dueAt={item.due_at} />
          </Meta>
          <Blocking item={item} titles={titles} />
          {item.project_id ? (
            <Meta label="案件">
              <Link className="underline" to="/projects/$id" params={{ id: item.project_id }}>
                {projectTitle ?? item.project_id}
              </Link>
            </Meta>
          ) : null}
          <Meta label="待ち">
            <time dateTime={item.created_at} title={formatAbsolute(item.created_at)}>
              {formatRelative(item.created_at)}から
            </time>
          </Meta>
          {item.links.length > 0 ? (
            <Meta label="関連">
              <span className="inline-flex flex-wrap gap-x-2">
                {item.links.map((link) =>
                  link.href.startsWith("/") && !link.href.startsWith("//") ? (
                    <InternalLink key={link.href} href={link.href}>
                      {link.label}
                    </InternalLink>
                  ) : (
                    <span key={link.href}>{link.label}</span>
                  ),
                )}
              </span>
            </Meta>
          ) : null}
        </dl>
        {item.detail ? (
          <div className="min-w-0 text-label">
            <Markdown source={item.detail} />
          </div>
        ) : null}
      </div>
      <div className="min-w-0 lg:w-96 lg:shrink-0">
        {needsNativeScreen(item) ? <NativeGuide item={item} /> : <AnswerForm item={item} sender={sender} />}
      </div>
    </li>
  );
}

function Filters({ search, projects }: { search: InboxSearch; projects: ProjectList | undefined }) {
  const navigate = useNavigate();
  const id = useId();
  const set = (next: InboxSearch) => void navigate({ to: "/inbox", search: next });
  return (
    <fieldset className="m-0 flex min-w-0 flex-col gap-3 border-0 p-0 sm:flex-row sm:items-end">
      <legend className="sr-only">受信箱の絞り込み</legend>
      <div className="flex min-w-0 flex-col gap-1 sm:w-64">
        <label htmlFor={`${id}-project`} className="text-label font-medium">
          案件
        </label>
        <select
          id={`${id}-project`}
          className={fieldClass}
          value={search.project ?? ""}
          onChange={(event) => set({ ...search, project: event.target.value || undefined })}
        >
          <option value="">すべての案件</option>
          {projects?.items.map((project) => (
            <option key={project.id} value={project.id}>
              {project.title}
            </option>
          ))}
          {search.project && !projects?.items.some((project) => project.id === search.project) ? (
            <option value={search.project}>{search.project}</option>
          ) : null}
        </select>
      </div>
      <div className="flex min-w-0 flex-col gap-1 sm:w-64">
        <label htmlFor={`${id}-kind`} className="text-label font-medium">
          種類
        </label>
        <select
          id={`${id}-kind`}
          className={fieldClass}
          value={search.kind ?? ""}
          onChange={(event) =>
            set({
              ...search,
              kind: (INBOX_KINDS as readonly string[]).includes(event.target.value)
                ? (event.target.value as InboxKind)
                : undefined,
            })
          }
        >
          <option value="">すべての種類</option>
          {INBOX_KINDS.map((kind) => (
            <option key={kind} value={kind}>
              {KIND_LABELS[kind]}
            </option>
          ))}
        </select>
      </div>
      {search.project || search.kind ? (
        <Button type="button" variant="ghost" onClick={() => set({})}>
          絞り込みを外す
        </Button>
      ) : null}
    </fieldset>
  );
}

function InboxList({
  view,
  sender,
  projects,
  filtered,
}: {
  view: HumanInboxView;
  sender: Sender;
  projects: ProjectList | undefined;
  filtered: boolean;
}) {
  const titles = new Map(view.items.map((item) => [item.id, item.title]));
  const projectTitles = new Map(projects?.items.map((project) => [project.id, project.title]));
  const byKind = Object.entries(view.counts.by_kind).filter(([, count]) => count > 0);
  return (
    <Section
      title={`判断待ち（${view.counts.total}）`}
      description={
        byKind.length > 0
          ? byKind.map(([kind, count]) => `${KIND_LABELS[kind as InboxKind] ?? kind} ${count}`).join(" / ")
          : undefined
      }
    >
      {view.items.length === 0 ? (
        <p className="text-muted-foreground">
          {filtered ? "この条件で判断待ちの項目はありません。" : "いま決めることはありません。"}
        </p>
      ) : (
        <ul aria-label="判断待ちの項目" className="flex min-w-0 flex-col border-t border-border">
          {view.items.map((item) => (
            <InboxRow
              key={item.id}
              item={item}
              sender={sender}
              titles={titles}
              projectTitle={item.project_id ? projectTitles.get(item.project_id) : undefined}
            />
          ))}
        </ul>
      )}
    </Section>
  );
}

export function InboxScreen({ search = {} }: { search?: InboxSearch }) {
  const filters = { project: search.project, kind: search.kind };
  const query = useQuery(inboxItemsQuery(filters));
  const projects = useQuery({
    queryKey: projectKeys.list(),
    queryFn: ({ signal }) => apiGet<ProjectList>("/api/projects", signal),
  });
  const [answered, setAnswered] = useState<Answered[]>([]);
  const sender = useInboxAnswer((answer) => setAnswered((previous) => [answer, ...previous].slice(0, 5)));
  return (
    <ScreenFrame
      title="受信箱"
      route="/inbox"
      description="人の判断を待っている項目です。ここで答えると一覧から消えます。知らせは通知で読みます。"
    >
      <Filters search={search} projects={projects.data} />
      <div role="status" aria-live="polite" className="min-w-0">
        {answered.length > 0 ? (
          <ul aria-label="答えた項目" className="flex flex-col gap-1 rounded-md bg-success p-3 text-success-foreground">
            {answered.map((row) => (
              <li key={row.id} className="break-words">
                「{row.title}」に「{row.option}」と答えました。
              </li>
            ))}
          </ul>
        ) : null}
      </div>
      <FetchFrame query={query}>
        {query.data && (
          <InboxList
            view={query.data}
            sender={sender}
            projects={projects.data}
            filtered={Boolean(search.project || search.kind)}
          />
        )}
      </FetchFrame>
    </ScreenFrame>
  );
}
