import { type ReactNode, useState } from "react";
import { CodeBlock } from "../ui/code-block";
import { Icon } from "../ui/icon";
import { copyText } from "../ui/short-id";

type LinkStyle = "inline" | "target";

const linkClass: Record<LinkStyle, string> = {
  inline: "underline break-all",
  // 本文の中でも 44px の操作領域を取る（チャットなど link が多い本文。FRONTEND_CONTRACT「Target サイズ」）。
  target: "inline-flex min-h-11 items-center underline break-all",
};

/** http(s) と same-origin の絶対 path だけを link にする。javascript: などは文字列のまま。 */
export function isSafeHref(href: string): boolean {
  return /^https?:\/\//.test(href) || /^\/(?!\/)/.test(href);
}

// React の text node のみで描画し、原文の HTML は実行しない。
function inline(source: string, links: LinkStyle = "inline"): ReactNode[] {
  const output: ReactNode[] = [];
  const pattern = /\[([^\]]+)\]\(([^)]+)\)|\*\*([^*]+)\*\*|`([^`]+)`/g;
  let cursor = 0;
  for (const match of source.matchAll(pattern)) {
    if (match.index > cursor) output.push(source.slice(cursor, match.index));
    if (match[1] && match[2]) {
      const href = match[2];
      output.push(
        isSafeHref(href) ? (
          <a key={match.index} href={href} rel="noopener noreferrer" className={linkClass[links]}>
            {match[1]}
          </a>
        ) : (
          match[1]
        ),
      );
    } else if (match[3]) output.push(<strong key={match.index}>{match[3]}</strong>);
    else
      output.push(
        <code key={match.index} className="rounded-sm bg-code px-1 font-mono text-code-foreground">
          {match[4]}
        </code>,
      );
    cursor = match.index + match[0].length;
  }
  if (cursor < source.length) output.push(source.slice(cursor));
  return output;
}

type Block =
  | { kind: "text"; text: string }
  | { kind: "code"; lang: string; text: string }
  | { kind: "table"; header: string[]; rows: string[][] };

const tableSeparator = /^\s*\|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)*\|?\s*$/;

function cells(line: string): string[] {
  return line
    .trim()
    .replace(/^\|/, "")
    .replace(/\|$/, "")
    .split("|")
    .map((cell) => cell.trim());
}

function textBlocks(text: string): Block[] {
  const blocks: Block[] = [];
  for (const part of text.split(/\n\s*\n/)) {
    if (part.trim() === "") continue;
    const lines = part.split("\n");
    if (lines.length >= 2 && lines[0]?.includes("|") && tableSeparator.test(lines[1] ?? "")) {
      blocks.push({ kind: "table", header: cells(lines[0]), rows: lines.slice(2).map(cells) });
    } else blocks.push({ kind: "text", text: part });
  }
  return blocks;
}

/** fenced code（```）を先に切り出し、残りを空行で段落・表に分ける。閉じていない fence は末尾まで code。 */
export function parseBlocks(source: string): Block[] {
  const blocks: Block[] = [];
  const fence = /^```([^\n`]*)\n([\s\S]*?)(?:\n```[ \t]*(?:\n|(?![\s\S]))|(?![\s\S]))/m;
  let rest = source;
  for (;;) {
    const match = fence.exec(rest);
    if (!match) break;
    blocks.push(...textBlocks(rest.slice(0, match.index)));
    blocks.push({ kind: "code", lang: (match[1] ?? "").trim(), text: match[2] ?? "" });
    rest = rest.slice(match.index + match[0].length);
  }
  blocks.push(...textBlocks(rest));
  return blocks;
}

type CopyState = "idle" | "copied" | "failed";

function CodeCopy({ text }: { text: string }) {
  const [state, setState] = useState<CopyState>("idle");
  const copy = async () => {
    const ok = await copyText(text, typeof navigator === "undefined" ? undefined : navigator.clipboard);
    setState(ok ? "copied" : "failed");
  };
  return (
    <span className="flex items-center justify-end gap-1">
      <span role="status" className="text-label text-muted-foreground empty:hidden">
        {state === "copied" ? "コピーしました" : state === "failed" ? "コピーできません" : ""}
      </span>
      <button
        type="button"
        aria-label="コードをコピー"
        title="コードをコピー"
        onClick={copy}
        onBlur={() => setState("idle")}
        className="inline-flex min-h-11 min-w-11 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
      >
        <Icon name={state === "copied" ? "check" : "copy"} size="sm" />
      </button>
    </span>
  );
}

function renderBlock(block: Block, index: number, links: LinkStyle): ReactNode {
  if (block.kind === "code")
    return (
      <div key={index} data-slot="markdown-code" className="min-w-0">
        <CodeCopy text={block.text} />
        <CodeBlock label={block.lang === "" ? "コード" : `コード（${block.lang}）`}>{block.text}</CodeBlock>
      </div>
    );
  if (block.kind === "table")
    return (
      <div key={index} className="min-w-0 max-w-full overflow-x-auto">
        <table className="border-collapse text-label">
          <thead>
            <tr>
              {block.header.map((cell, i) => (
                // biome-ignore lint/suspicious/noArrayIndexKey: 列は位置で決まる。
                <th key={i} scope="col" className="border border-border px-2 py-1 text-left font-semibold">
                  {inline(cell, links)}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {block.rows.map((row, r) => (
              // biome-ignore lint/suspicious/noArrayIndexKey: 行は位置で決まる。
              <tr key={r}>
                {row.map((cell, i) => (
                  // biome-ignore lint/suspicious/noArrayIndexKey: 列は位置で決まる。
                  <td key={i} className="border border-border px-2 py-1 align-top">
                    {inline(cell, links)}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    );
  const heading = /^(#{1,3})\s+(.+)$/s.exec(block.text);
  if (heading)
    return (
      <h3 key={index} className="font-semibold">
        {inline(heading[2], links)}
      </h3>
    );
  return (
    <p key={index} className="whitespace-pre-wrap">
      {inline(block.text, links)}
    </p>
  );
}

/** 安全な Markdown 表示。HTML は常に文字列として表示する。links="target" で link に 44px の操作領域を取る。 */
export function Markdown({ source, links = "inline" }: { source: string; links?: LinkStyle }) {
  return (
    <div className="min-w-0 space-y-2 break-words">
      {parseBlocks(source).map((block, index) => renderBlock(block, index, links))}
    </div>
  );
}
