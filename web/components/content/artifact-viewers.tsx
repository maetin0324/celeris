import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { cn } from "../../lib/utils";
import { ErrorNotice } from "../fetch-state/fetch-frame";
import { Button, buttonClassName } from "../ui/button";
import { Markdown } from "./markdown";

// 成果物の形式ごとの表示（ArtifactPreview が選ぶ）。本文は gateway の `/files/tasks/:id/artifacts/:idx`。
// - text・code・JSON・CSV・log: offset/length の範囲取得で分けて読み、行番号と折り返しの切り替えを付ける。
// - 画像: `<img>` の拡大・縮小。SVG は `view=1`（image/svg+xml、CSP sandbox）を `<img>` で読む（script は走らない）。
// - PDF: `<object type="application/pdf">` で browser の viewer。viewer の無い browser（Android 等）は代わりの案内。
// - HTML: `sandbox=""` の iframe（opaque origin、script・form・popup なし）。応答も CSP sandbox（二重）。
// 安全性の判断は agent-docs/adr/2026-10-05-web-artifact-inline-view.md。

/** 1 回の範囲取得の大きさ。大きい log も先頭から分けて読む。 */
export const TEXT_CHUNK_BYTES = 128 * 1024;

const frame = "min-w-0 max-w-full overflow-auto overscroll-contain rounded-sm border border-border";

function Loading() {
  return (
    <p role="status" aria-busy="true" className="text-body text-muted-foreground">
      読み込み中…
    </p>
  );
}

function bytes(size: number): string {
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
  return `${(size / 1024 / 1024).toFixed(1)} MB`;
}

export function MarkdownViewer({ href }: { href: string }) {
  const [content, setContent] = useState<string>();
  const [error, setError] = useState(false);
  const [attempt, setAttempt] = useState(0);
  // biome-ignore lint/correctness/useExhaustiveDependencies: attempt は明示的な再取得操作で effect をやり直すための世代。
  useEffect(() => {
    const controller = new AbortController();
    setError(false);
    setContent(undefined);
    fetch(href, { signal: controller.signal, credentials: "same-origin" })
      .then((response) => {
        if (!response.ok) throw new Error("preview failed");
        return response.text();
      })
      .then((text) => {
        if (!controller.signal.aborted) setContent(text);
      })
      .catch(() => {
        if (!controller.signal.aborted) setError(true);
      });
    return () => controller.abort();
  }, [href, attempt]);
  return (
    <div className={cn(frame, "max-h-96 p-3")}>
      {error ? (
        <ErrorNotice subject="本文" onRetry={() => setAttempt((value) => value + 1)} />
      ) : content === undefined ? (
        <Loading />
      ) : (
        <Markdown source={content} />
      )}
    </div>
  );
}

type TextState = { text: string; loaded: number; total?: number };

export function TextViewer({
  href,
  name,
  chunkBytes = TEXT_CHUNK_BYTES,
}: {
  href: string;
  name: string;
  chunkBytes?: number;
}) {
  const [state, setState] = useState<TextState>({ text: "", loaded: 0 });
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(false);
  const [wrap, setWrap] = useState(true);
  const decoder = useRef(new TextDecoder("utf-8"));
  const controller = useRef<AbortController | null>(null);

  const load = useCallback(
    (offset: number) => {
      controller.current?.abort();
      const current = new AbortController();
      controller.current = current;
      if (offset === 0) decoder.current = new TextDecoder("utf-8");
      setLoading(true);
      setError(false);
      fetch(`${href}?offset=${offset}&length=${chunkBytes}`, { signal: current.signal, credentials: "same-origin" })
        .then(async (response) => {
          // offset が末尾ちょうどを越えた（file が縮んだ）ときは、読めた所までで終える。
          if (response.status === 416) return { body: new Uint8Array(), size: offset };
          if (!response.ok) throw new Error("preview failed");
          const body = new Uint8Array(await response.arrayBuffer());
          const header = Number(response.headers.get("x-celeris-size"));
          const size = Number.isFinite(header) && response.headers.has("x-celeris-size") ? header : undefined;
          return { body, size };
        })
        .then(({ body, size }) => {
          if (current.signal.aborted) return;
          const loaded = offset + body.length;
          const total = size ?? (body.length < chunkBytes ? loaded : undefined);
          const more = total === undefined || loaded < total;
          // 範囲の境目で UTF-8 の文字が割れても、続きの範囲と合わせて復号する（stream）。
          const chunk = decoder.current.decode(body, { stream: more });
          setState((previous) => ({ text: offset === 0 ? chunk : previous.text + chunk, loaded, total }));
          setLoading(false);
        })
        .catch(() => {
          if (current.signal.aborted) return;
          setLoading(false);
          setError(true);
        });
    },
    [href, chunkBytes],
  );

  useEffect(() => {
    load(0);
    return () => controller.current?.abort();
  }, [load]);

  const lines = useMemo(() => {
    const split = state.text.split("\n");
    if (split.length > 1 && split.at(-1) === "") split.pop();
    return split;
  }, [state.text]);
  const more = state.total === undefined || state.loaded < state.total;

  return (
    <div className="flex min-w-0 flex-col gap-2" data-artifact-viewer="text">
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <Button size="sm" variant="ghost" aria-pressed={wrap} onClick={() => setWrap((value) => !value)}>
          {wrap ? "折り返しを解除" : "折り返す"}
        </Button>
        <span className="text-label text-muted-foreground tabular-nums" aria-live="polite">
          {state.total !== undefined && state.loaded < state.total
            ? `${bytes(state.loaded)} / ${bytes(state.total)} を表示中`
            : state.loaded > 0 || state.total === 0
              ? `${lines.length} 行・${bytes(state.loaded)}`
              : null}
        </span>
      </div>
      {state.loaded > 0 || !loading ? (
        <section
          aria-label={`${name} の本文`}
          // biome-ignore lint/a11y/noNoninteractiveTabindex: scroll 領域を keyboard で読めるようにする（WCAG 2.1.1）。
          tabIndex={0}
          data-wrap={wrap ? "true" : "false"}
          className={cn(frame, "max-h-128 bg-code font-mono text-label leading-relaxed text-code-foreground")}
        >
          <table className={cn("border-collapse", wrap ? "w-full table-fixed" : "w-max min-w-full")}>
            <colgroup>
              <col className={wrap ? "w-12" : undefined} />
              <col />
            </colgroup>
            <tbody>
              {lines.map((line, i) => (
                // biome-ignore lint/suspicious/noArrayIndexKey: 行番号そのものが key（追記は末尾だけ）。
                <tr key={i}>
                  <td
                    aria-hidden="true"
                    className="select-none border-r border-border px-2 text-right align-top text-muted-foreground tabular-nums"
                  >
                    {i + 1}
                  </td>
                  <td className={cn("px-3 align-top", wrap ? "whitespace-pre-wrap break-all" : "whitespace-pre")}>
                    {line || "​"}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      ) : null}
      {error ? (
        <ErrorNotice subject="本文" onRetry={() => load(state.loaded)} />
      ) : loading ? (
        <Loading />
      ) : more ? (
        <Button className="self-start" onClick={() => load(state.loaded)}>
          続きを読む（次の {bytes(chunkBytes)}）
        </Button>
      ) : null}
    </div>
  );
}

const zoomSteps = [0.25, 0.5, 0.75, 1, 1.5, 2, 3, 4];

export function ImageViewer({ src, name }: { src: string; name: string }) {
  const image = useRef<HTMLImageElement>(null);
  const [natural, setNatural] = useState<{ width: number; height: number }>();
  const [scale, setScale] = useState<number | null>(null);
  const [error, setError] = useState(false);
  const [attempt, setAttempt] = useState(0);
  // 「全体を表示」の今の倍率。拡大・縮小はここから次の段へ進む。
  const current = () => scale ?? (natural && image.current ? image.current.clientWidth / natural.width : 1);
  const zoom = (direction: 1 | -1) => {
    const now = current();
    const next =
      direction > 0
        ? (zoomSteps.find((step) => step > now + 0.001) ?? zoomSteps.at(-1))
        : (zoomSteps.findLast((step) => step < now - 0.001) ?? zoomSteps[0]);
    setScale(next ?? 1);
  };
  const label = scale === null ? "全体を表示" : `${Math.round(scale * 100)}%`;
  if (error)
    return (
      <ErrorNotice
        subject="画像"
        onRetry={() => {
          setError(false);
          setAttempt((value) => value + 1);
        }}
      />
    );
  return (
    <div className="flex min-w-0 flex-col gap-2" data-artifact-viewer="image">
      <fieldset className="flex min-w-0 flex-wrap items-center gap-2" aria-label={`${name} の表示倍率`}>
        <Button size="sm" variant="ghost" onClick={() => zoom(-1)} disabled={!natural || current() <= zoomSteps[0]}>
          縮小
        </Button>
        <Button size="sm" variant="ghost" onClick={() => zoom(1)} disabled={!natural || current() >= 4}>
          拡大
        </Button>
        <Button size="sm" variant="ghost" onClick={() => setScale(1)} aria-pressed={scale === 1}>
          等倍
        </Button>
        <Button size="sm" variant="ghost" onClick={() => setScale(null)} aria-pressed={scale === null}>
          全体を表示
        </Button>
        <output className="text-label text-muted-foreground tabular-nums" data-testid="artifact-image-zoom">
          {label}
          {natural ? `（${natural.width}×${natural.height}）` : ""}
        </output>
      </fieldset>
      <section
        aria-label={`${name} の画像`}
        // biome-ignore lint/a11y/noNoninteractiveTabindex: 拡大した画像を keyboard で scroll できるようにする（WCAG 2.1.1）。
        tabIndex={0}
        className={cn(frame, "max-h-128 bg-muted p-2")}
      >
        <img
          key={attempt}
          ref={image}
          src={src}
          alt={name}
          className={scale === null ? "h-auto max-w-full" : "max-w-none"}
          // 拡大・縮小の幅は利用者の操作で決まる実行時の値（デザインの寸法ではない）。
          style={scale !== null && natural ? { width: natural.width * scale } : undefined}
          onLoad={(event) =>
            setNatural({ width: event.currentTarget.naturalWidth, height: event.currentTarget.naturalHeight })
          }
          onError={() => setError(true)}
        />
      </section>
    </div>
  );
}

export function PdfViewer({ src, name }: { src: string; name: string }) {
  return (
    <div className="flex min-w-0 flex-col gap-2" data-artifact-viewer="pdf">
      <object data={src} type="application/pdf" aria-label={`${name}（PDF）`} className={cn(frame, "h-128 w-full")}>
        <div className="flex min-w-0 flex-col items-start gap-2 p-3 text-body">
          <p>この browser では PDF をここに表示できません。新しいタブで開くか、ダウンロードしてください。</p>
          <a className={buttonClassName} href={src} target="_blank" rel="noopener noreferrer">
            PDF を新しいタブで開く
          </a>
        </div>
      </object>
    </div>
  );
}

export function HtmlViewer({ src, name }: { src: string; name: string }) {
  return (
    <div className="flex min-w-0 flex-col gap-2" data-artifact-viewer="html">
      <p className="text-label text-muted-foreground">
        安全のため、HTML の script・フォーム・外部の画像や読み込みは動かさずに表示します。
      </p>
      <iframe
        src={src}
        title={`${name} の表示`}
        // 空の sandbox: opaque origin で script・form・popup・top への遷移をすべて止める（allow-* を足さない）。
        sandbox=""
        referrerPolicy="no-referrer"
        className={cn(frame, "h-128 w-full bg-background")}
      />
    </div>
  );
}
