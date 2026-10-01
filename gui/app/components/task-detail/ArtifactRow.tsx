import { useEffect, useState } from "react";
import { Link, useFetcher } from "react-router";
import type { DocsOpOutcome } from "~/celeris/action-types";
import type { ArtifactView } from "~/celeris/types";
import { CodeViewer } from "~/components/CodeViewer";
import { ErrorFlash } from "~/components/Flash";
import { ImageViewer } from "~/components/ImageViewer";
import { MarkdownViewer } from "~/components/MarkdownViewer";
import { Sha256Badge } from "~/components/Sha256Badge";
import { buttonClass } from "~/components/ui/button";
import { checkboxClass, chipLabelClass, inputClass, labelClass, touchLinkClass } from "~/components/ui/form";
import { Icon } from "~/components/ui/Icon";
import { artifactStatusMessage, isJson, pickViewer } from "~/lib/artifact-view";
import { defaultPromotePath, docsHref, isMarkdownName } from "~/lib/docs";
import {
  docsErrorHint,
  PROMOTE_OVERWRITE_LABEL,
  PROMOTE_TO_DOC_LABEL,
  PROMOTE_TO_DOC_SUBMIT_LABEL,
} from "~/lib/labels";
import { cn } from "~/lib/utils";

/**
 * 成果物 1 件の行（docs/adr/0006-g3-decisions.md D3/D4）。本体は「開く」を押したときだけ
 * `/files/tasks/:id/artifacts/:idx` を fetch し、celeris が返した実際の `Content-Type` でビューアを選ぶ
 * （拡張子からの推測はしない。celeris の値をそのまま使う）。403（`forbidden`）は一覧の `ArtifactView.forbidden`
 * だけで判定し、本体を取りに行かない。
 */
export function ArtifactRow({
  taskId,
  artifact,
  projectId,
  taskTitle,
  category,
}: {
  taskId: string;
  artifact: ArtifactView;
  /** ADR-0044 D7: 昇格の宛先は案件の文書なので、案件に属さないタスクでは出さない。 */
  projectId: string | null;
  taskTitle: string;
  category: string | null;
}) {
  const [open, setOpen] = useState(false);
  const [promoting, setPromoting] = useState(false);
  const [body, setBody] = useState<{ contentType: string; content?: string } | null>(null);
  const href = `/files/tasks/${taskId}/artifacts/${artifact.idx}`;
  const canOpen = artifact.exists && !artifact.forbidden;
  const statusMessage = artifactStatusMessage(artifact);
  // ADR-0044 D7: 昇格できるのは Markdown の成果物だけ（判定は名前だけ。中身は celeris が読む）。
  const canPromote = canOpen && projectId !== null && isMarkdownName(artifact.artifact.name);

  useEffect(() => {
    if (!open || body || !canOpen) return;
    let cancelled = false;
    (async () => {
      const res = await fetch(href);
      const contentType = res.headers.get("content-type") ?? "application/octet-stream";
      if (pickViewer(contentType, artifact.artifact.name) === "image") {
        if (!cancelled) setBody({ contentType });
        return;
      }
      const content = await res.text();
      if (!cancelled) setBody({ contentType, content });
    })();
    return () => {
      cancelled = true;
    };
  }, [open, body, canOpen, href, artifact.artifact.name]);

  return (
    <li data-testid="artifact-item" className="rounded-lg border border-border p-3 text-sm">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="min-w-0">
          <p className="truncate font-mono text-xs text-fg" data-testid="artifact-name" title={artifact.artifact.name}>
            {artifact.artifact.name}
          </p>
          <p className="mt-0.5 text-sm text-fg-subtle lg:text-xs">
            {artifact.artifact.kind} · run {artifact.run_id}
          </p>
        </div>
        {canOpen && (
          <div className="flex shrink-0 gap-2">
            <button
              type="button"
              onClick={() => setOpen((v) => !v)}
              data-testid="artifact-toggle"
              className={buttonClass({ variant: "secondary", size: "xs" })}
            >
              <Icon name={open ? "chevronDown" : "chevronRight"} />
              {open ? "閉じる" : "開く"}
            </button>
            <a
              href={`${href}?download=1`}
              download
              data-testid="artifact-download"
              className={buttonClass({ variant: "ghost", size: "xs" })}
            >
              保存
            </a>
            {canPromote && (
              <button
                type="button"
                onClick={() => setPromoting((v) => !v)}
                data-testid="artifact-promote"
                className={buttonClass({ variant: "ghost", size: "xs" })}
              >
                <Icon name="book" />
                {PROMOTE_TO_DOC_LABEL}
              </button>
            )}
          </div>
        )}
      </div>
      {promoting && projectId && (
        <PromoteToDoc
          taskId={taskId}
          projectId={projectId}
          name={artifact.artifact.name}
          taskTitle={taskTitle}
          category={category}
          onClose={() => setPromoting(false)}
        />
      )}
      {statusMessage && (
        <p
          data-testid={artifact.forbidden ? "artifact-forbidden" : "artifact-missing"}
          className="mt-2 rounded-md border border-danger-border bg-danger-soft px-2.5 py-1.5 text-danger-soft-fg"
        >
          {statusMessage}
        </p>
      )}
      <Sha256Badge
        recorded={artifact.artifact.sha256}
        current={artifact.sha256_current}
        matches={artifact.sha256_matches}
      />
      {open && body && (
        <div className="mt-3">
          {pickViewer(body.contentType, artifact.artifact.name) === "image" ? (
            <ImageViewer src={href} alt={artifact.artifact.name} />
          ) : pickViewer(body.contentType, artifact.artifact.name) === "markdown" ? (
            <MarkdownViewer content={body.content ?? ""} />
          ) : (
            <CodeViewer content={body.content ?? ""} json={isJson(body.contentType)} />
          )}
        </div>
      )}
    </li>
  );
}

/**
 * 成果物を案件の文書に昇格する（ADR-0044 D7、docs/celeris-api-v1.md §3.97。**管理系**。Phase 57 / G20）。
 * 宛先のパスは人が決める（既定は `docs/<種類>/<題名の slug>.md`）。宛先が既にあれば celeris が
 * 409 `page_exists` を返すので、そのときだけ「上書きする」を選び直す（GUI では判定しない）。
 */
function PromoteToDoc({
  taskId,
  projectId,
  name,
  taskTitle,
  category,
  onClose,
}: {
  taskId: string;
  projectId: string;
  name: string;
  taskTitle: string;
  category: string | null;
  onClose: () => void;
}) {
  const fetcher = useFetcher<DocsOpOutcome>({ key: `promote-${taskId}-${name}` });
  const submitting = fetcher.state !== "idle";
  const error = fetcher.data && !fetcher.data.ok ? fetcher.data.error : null;
  return (
    <div className="mt-3 rounded-lg border border-border bg-surface-2/50 p-3" data-testid="artifact-promote-form">
      <fetcher.Form method="post" action={`/tasks/${taskId}`} className="space-y-2">
        <input type="hidden" name="intent" value="promote" />
        <input type="hidden" name="name" value={name} />
        <label className={labelClass} htmlFor={`promote-path-${name}`}>
          文書の置き場（案件の文書の根からの相対パス。`.md`）
        </label>
        <input
          id={`promote-path-${name}`}
          name="path"
          className={inputClass}
          defaultValue={defaultPromotePath("docs", category, taskTitle, name)}
          data-testid="artifact-promote-path"
        />
        <label className={labelClass} htmlFor={`promote-title-${name}`}>
          題名（任意。省略すると中身の 1 行目）
        </label>
        <input id={`promote-title-${name}`} name="title" className={inputClass} defaultValue={taskTitle} />
        <label className={chipLabelClass}>
          <input type="checkbox" name="overwrite" value="1" className={checkboxClass} />
          {PROMOTE_OVERWRITE_LABEL}
        </label>
        <div className="flex items-center gap-2">
          <button
            type="submit"
            disabled={submitting}
            className={buttonClass({ variant: "primary", size: "xs" })}
            data-testid="artifact-promote-submit"
          >
            {PROMOTE_TO_DOC_SUBMIT_LABEL}
          </button>
          <button type="button" onClick={onClose} className={buttonClass({ variant: "ghost", size: "xs" })}>
            やめる
          </button>
        </div>
      </fetcher.Form>
      {error && (
        <div data-testid="artifact-promote-error">
          <ErrorFlash error={error} />
          {docsErrorHint(error.code) && <p className="text-sm text-fg-muted lg:text-xs">{docsErrorHint(error.code)}</p>}
        </div>
      )}
      {fetcher.data?.ok && fetcher.data.op === "docs_promote" && (
        <p className="mt-2 text-sm" data-testid="artifact-promote-done">
          <Link
            to={docsHref(projectId, { path: fetcher.data.result.path })}
            className={cn(touchLinkClass, "font-medium text-primary hover:underline")}
          >
            {fetcher.data.result.path}
          </Link>{" "}
          に昇格しました。
        </p>
      )}
    </div>
  );
}
