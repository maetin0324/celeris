// 成果物の表示の種類（file 名の拡張子で決める）。daemon の Content-Type の表（api.md §3.8）に無い html・svg・pdf は
// gateway の `view=1` が拡張子で補う（web/server/files.js）。表に無い・読めない形式は other（download だけ）。
export type ArtifactKind = "markdown" | "text" | "image" | "svg" | "pdf" | "html" | "other";

const text = new Set([
  "txt",
  "log",
  "out",
  "err",
  "json",
  "jsonl",
  "ndjson",
  "csv",
  "tsv",
  "diff",
  "patch",
  "toml",
  "yaml",
  "yml",
  "ini",
  "cfg",
  "conf",
  "xml",
  "rs",
  "py",
  "sh",
  "bash",
  "ts",
  "tsx",
  "js",
  "jsx",
  "mjs",
  "c",
  "h",
  "cc",
  "cpp",
  "hpp",
  "go",
  "java",
  "sql",
  "tex",
  "bib",
  "r",
  "jl",
]);
const image = new Set(["png", "jpg", "jpeg", "gif", "webp"]);

export function artifactKind(name: string): ArtifactKind {
  const ext = /\.([a-z0-9]+)$/i.exec(name)?.[1]?.toLowerCase();
  if (!ext) return /(^|\/)(readme|license|makefile|dockerfile)$/i.test(name) ? "text" : "other";
  if (ext === "md" || ext === "markdown") return "markdown";
  if (ext === "svg") return "svg";
  if (ext === "pdf") return "pdf";
  if (ext === "html" || ext === "htm") return "html";
  if (image.has(ext)) return "image";
  if (text.has(ext)) return "text";
  return "other";
}

/** 画面の中で開ける種類か。other は download だけ。 */
export function previewable(kind: ArtifactKind): boolean {
  return kind !== "other";
}
