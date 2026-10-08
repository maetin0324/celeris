// CoS の返事が thread workspace の file に触れた所を、その添付への link にする
// （ADR 2026-10-08-cos-workspace-files-in-chat D3）。対応表は message.workspace_files（daemon が決める）。
// - 絶対 path（…/workspace/<path>）・相対 path（<path>・./<path>）を対象にし、境界は daemon（workspace_files.rs）と同じ:
//   前後が path の文字（ASCII 英数と _-./~+@%）なら別の名前の一部とみなす。文末の「.」は外。
// - fenced code（```）はそのまま。既存の Markdown link は workspace path 宛てだけ置き換える。inline code は中身全体が path のときだけ link にする。
import type { ChatWorkspaceFile } from "../../../api/generated/types";

const pathChar = /[A-Za-z0-9_\-./~+@%]/;

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** path 1 件の正規表現。前置き（…/workspace/ か ./）付きの全体を 1 つの一致にする。 */
function pathPattern(path: string): RegExp {
  return new RegExp(`(?:[A-Za-z0-9_\\-./~+@%]*/workspace/|\\./)?${escapeRegExp(path)}`, "g");
}

function boundaryOk(text: string, start: number, end: number): boolean {
  const before = text[start - 1];
  // 前置きが無い一致の直前は path の外。前置き付きでも、その前置きの直前が外。
  if (before !== undefined && pathChar.test(before)) return false;
  const after = text[end];
  if (after === undefined) return true;
  if (after === ".") {
    const next = text[end + 1];
    return next === undefined || !pathChar.test(next) || next === ".";
  }
  return !pathChar.test(after);
}

type Span = { start: number; end: number; href: string };

function linkPlain(text: string, files: ChatWorkspaceFile[], hrefFor: (id: string) => string): string {
  const spans: Span[] = [];
  // 長い path を先に当て、短い path（別 dir の同名 file）が重ねて当たらないようにする。
  const ordered = [...files].sort((a, b) => b.path.length - a.path.length);
  for (const file of ordered) {
    for (const match of text.matchAll(pathPattern(file.path))) {
      const start = match.index;
      const end = start + match[0].length;
      if (!boundaryOk(text, start, end)) continue;
      if (spans.some((span) => start < span.end && span.start < end)) continue;
      spans.push({ start, end, href: hrefFor(file.attachment_id) });
    }
  }
  if (spans.length === 0) return text;
  spans.sort((a, b) => a.start - b.start);
  let out = "";
  let cursor = 0;
  for (const span of spans) {
    out += `${text.slice(cursor, span.start)}[${text.slice(span.start, span.end)}](${span.href})`;
    cursor = span.end;
  }
  return out + text.slice(cursor);
}

function linkSegment(text: string, files: ChatWorkspaceFile[], hrefFor: (id: string) => string): string {
  // inline code と既存の link を切り出し、残りの地の文だけを置き換える。
  return text
    .split(/(`[^`\n]+`|\[[^\]\n]+\]\([^)\n]+\))/)
    .map((part, index) => {
      if (index % 2 === 0) return linkPlain(part, files, hrefFor);
      if (!part.startsWith("`")) {
        const link = /^\[([^\]]+)\]\(([^)]+)\)$/.exec(part);
        if (!link) return part;
        const destination = link[2];
        const file = files.find((file) => {
          const match = pathPattern(file.path).exec(destination);
          return match?.index === 0 && match[0].length === destination.length;
        });
        return file ? `[${link[1]}](${hrefFor(file.attachment_id)})` : part;
      }
      const inner = part.slice(1, -1).trim();
      const linked = linkPlain(inner, files, hrefFor);
      // 中身全体が 1 つの path なら link にする（code の見た目は失うが押せる方を取る）。
      return linked.startsWith("[") && linked.endsWith(")") && !linked.slice(1).includes("[") ? linked : part;
    })
    .join("");
}

/** 返事の本文の path を添付の link（Markdown）にした本文。対応が無ければ元の本文。 */
export function linkWorkspacePaths(
  text: string,
  files: ChatWorkspaceFile[] | undefined,
  hrefFor: (attachmentId: string) => string,
): string {
  if (!files || files.length === 0 || text === "") return text;
  // fenced code は置き換えない（奇数番目が fence の中）。閉じていない fence は末尾まで code。
  return text
    .split(/(^```[^\n]*\n[\s\S]*?(?:\n```[ \t]*(?=\n|$)|$(?![\s\S])))/m)
    .map((part, index) => (index % 2 === 1 ? part : linkSegment(part, files, hrefFor)))
    .join("");
}

/** link の href が返事の添付の本文 URL なら、その添付 id。 */
export function workspaceAttachmentFor(
  href: string,
  files: ChatWorkspaceFile[] | undefined,
  hrefFor: (attachmentId: string) => string,
): string | undefined {
  return files?.find((file) => hrefFor(file.attachment_id) === href)?.attachment_id;
}
