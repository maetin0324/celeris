import type { BadgeTone } from "../../components/ui/badge";

// 差分の 1 行の種類。色は token の組（success / danger / info）で付け、行頭の +/− を原文のまま残して色だけに頼らない。
export type DiffLineKind = "add" | "remove" | "hunk" | "meta" | "context";

export function diffLineKind(line: string): DiffLineKind {
  if (line.startsWith("+++") || line.startsWith("---")) return "meta";
  if (line.startsWith("diff ") || line.startsWith("index ") || line.startsWith("\\ ")) return "meta";
  if (line.startsWith("@@")) return "hunk";
  if (line.startsWith("+")) return "add";
  if (line.startsWith("-")) return "remove";
  return "context";
}

/** 原文を行に分ける。末尾の改行で空行を 1 つ足さない。 */
export function diffLines(diff: string): { kind: DiffLineKind; text: string }[] {
  const lines = diff.split("\n");
  if (lines.at(-1) === "") lines.pop();
  return lines.map((text) => ({ kind: diffLineKind(text), text }));
}

// git の status 1 文字（M/A/D/R/C/?）を可視ラベルと tone へ。未知の文字は原文のまま neutral。
const fileStatus: Record<string, { label: string; tone: BadgeTone }> = {
  M: { label: "変更", tone: "info" },
  A: { label: "追加", tone: "success" },
  D: { label: "削除", tone: "danger" },
  R: { label: "名前変更", tone: "neutral" },
  C: { label: "複製", tone: "neutral" },
  "?": { label: "未追跡", tone: "warning" },
};

export function fileStatusView(status: string): { label: string; tone: BadgeTone } {
  return fileStatus[status.trim().charAt(0).toUpperCase()] ?? { label: status, tone: "neutral" };
}

// 取り込みの状態語を運用者が読めるラベルと tone へ。
export function integrationLabel(state: string): string {
  const labels: Record<string, string> = {
    merged: "取り込み済み",
    done: "完了",
    conflict: "衝突あり",
    failed: "失敗",
    error: "エラー",
    open: "PR 公開中",
    pending: "処理待ち",
  };
  return labels[state] ?? `未確認（${state}）`;
}

export function integrationTone(state: string): BadgeTone {
  if (state === "merged" || state === "done") return "success";
  if (state === "conflict" || state === "failed" || state === "error") return "danger";
  if (state === "open" || state === "pending") return "info";
  return "neutral";
}

/**
 * 変更ファイルの path を「全部に共通の dir」と「各ファイルの dir の残り・ファイル名」に分ける。
 * 狭い幅で長い共通 prefix が毎行折り返され、ファイル名を比べにくかった（fix-r6 narrow）。
 * 共通 dir は 2 件以上のときだけ取り出す（1 件なら dir の残りに全部を置く）。区切りは `/` 単位で、名前の途中では切らない。
 */
export function splitChangedPaths(paths: readonly string[]): {
  common: string;
  files: { path: string; dir: string; name: string }[];
} {
  const dirs = paths.map((path) => path.split("/").slice(0, -1));
  let depth = 0;
  if (paths.length > 1) {
    const first = dirs[0] ?? [];
    while (depth < first.length && dirs.every((dir) => dir[depth] === first[depth])) depth += 1;
  }
  const common = depth > 0 ? `${(dirs[0] ?? []).slice(0, depth).join("/")}/` : "";
  return {
    common,
    files: paths.map((path, i) => {
      const rest = (dirs[i] ?? []).slice(depth);
      return { path, dir: rest.length > 0 ? `${rest.join("/")}/` : "", name: path.split("/").at(-1) ?? path };
    }),
  };
}
