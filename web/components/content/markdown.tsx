import type { ReactNode } from "react";

// React の text node のみで描画し、原文の HTML は実行しない。
function inline(source: string): ReactNode[] {
  const output: ReactNode[] = [];
  const pattern = /\[([^\]]+)\]\(([^)]+)\)|\*\*([^*]+)\*\*|`([^`]+)`/g;
  let cursor = 0;
  for (const match of source.matchAll(pattern)) {
    if (match.index > cursor) output.push(source.slice(cursor, match.index));
    if (match[1] && match[2]) {
      const href = match[2];
      output.push(
        /^https?:\/\//.test(href) || /^\/(?!\/)/.test(href) ? (
          <a key={match.index} href={href} rel="noopener noreferrer" className="underline break-all">
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

/** 安全な Markdown 表示。HTML は常に文字列として表示する。 */
export function Markdown({ source }: { source: string }) {
  return (
    <div className="space-y-2 break-words">
      {source.split(/\n\s*\n/).map((block) => {
        const heading = /^(#{1,3})\s+(.+)$/s.exec(block);
        if (heading)
          return (
            <h3 key={block} className="font-semibold">
              {inline(heading[2])}
            </h3>
          );
        return (
          <p key={block} className="whitespace-pre-wrap">
            {inline(block)}
          </p>
        );
      })}
    </div>
  );
}
