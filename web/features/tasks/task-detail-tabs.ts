// /tasks/:id の tab（P3-08）。?tab= の search param で切り替え、取得を待たない。
// P3-08 は overview と timeline、P3-13 が changes・files・artifacts を足した（旧 GUI と同じ 5 tab）。
export const TASK_DETAIL_TABS = [
  { key: "overview", label: "概要" },
  { key: "timeline", label: "timeline" },
  { key: "changes", label: "変更" },
  { key: "files", label: "作業ツリー" },
  { key: "artifacts", label: "成果物" },
] as const;

export type TaskDetailTab = (typeof TASK_DETAIL_TABS)[number]["key"];

/** 知らない値・未指定は overview にする（旧 GUI の既定と同じ）。 */
export function parseTaskDetailTab(value: unknown): TaskDetailTab {
  return TASK_DETAIL_TABS.find((tab) => tab.key === value)?.key ?? "overview";
}

// スマホ幅（md 未満）で概要 tab の中を区画ごとに切り替える（2026-10-04 screens-ops mobile）。
// desktop（md 以上）は全区画を並べたまま（header-tree の配置）で、切り替えは出さない。
export const MOBILE_SECTIONS = [
  { key: "summary", label: "概要" },
  { key: "decision", label: "判断" },
  { key: "execution", label: "実行" },
  { key: "tree", label: "木" },
] as const;

export type MobileSection = (typeof MOBILE_SECTIONS)[number]["key"];

// header の「次の操作」と木の link は hash で移る。スマホではその移動先を含む区画を開く。
const SECTION_BY_HASH: Record<string, MobileSection> = {
  "decision-panel": "decision",
  "execution-panel": "execution",
  "task-tree": "tree",
  "integration-repair": "tree",
};

/** hash（# の有無を問わない）から開く区画。知らない hash は null（今の区画のまま）。 */
export function sectionForHash(hash: string | undefined): MobileSection | null {
  const key = (hash ?? "").replace(/^#/, "");
  return SECTION_BY_HASH[key] ?? null;
}

/**
 * 区画の包みの class。開いている区画はスマホで縦に並べ、閉じている区画はスマホでだけ隠す。
 * md 以上は display: contents で包みを消し、親の flex と gap をそのまま使う（desktop の配置を変えない）。
 */
export function mobileSectionClass(section: MobileSection, current: MobileSection | undefined): string {
  return current === undefined || current === section
    ? "flex min-w-0 flex-col gap-4 md:contents"
    : "hidden md:contents";
}
