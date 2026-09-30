// /tasks/:id の tab（P3-08）。?tab= の search param で切り替え、取得を待たない。
// P3-08 は overview と timeline。P3-13 が changes・files・artifacts を足す（この配列に並べるだけ）。
export const TASK_DETAIL_TABS = [
  { key: "overview", label: "概要" },
  { key: "timeline", label: "timeline" },
] as const;

export type TaskDetailTab = (typeof TASK_DETAIL_TABS)[number]["key"];

/** 知らない値・未指定は overview にする（旧 GUI の既定と同じ）。 */
export function parseTaskDetailTab(value: unknown): TaskDetailTab {
  return TASK_DETAIL_TABS.find((tab) => tab.key === value)?.key ?? "overview";
}
