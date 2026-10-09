import { useQuery } from "@tanstack/react-query";
import { Link, useLocation } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { ScrollTabs } from "../../components/ui/scroll-tabs";
import { TaskArtifactsPanel } from "../artifacts/task-artifacts-view";
import { TaskBrowserPrerequisite } from "../browser/task-browser-policy";
import { TaskChangesPanel } from "../changes/changes-view";
import { TaskDecisionsPanel } from "../decisions/decision-detail";
import { TaskFilesPanel } from "../files/task-files-view";
import { DecisionPanel } from "./decision-panel";
import { ExecutionPanel } from "./execution-panel";
import { OverviewView, TaskDetailHeader } from "./overview-view";
import { taskDetailQuery, taskTimelineQuery } from "./task-detail-query";
import {
  MOBILE_SECTIONS,
  type MobileSection,
  mobileSectionClass,
  sectionForHash,
  TASK_DETAIL_TABS,
  type TaskDetailTab,
} from "./task-detail-tabs";
import { TimelineView } from "./timeline-view";

// /tasks/:id の枠（P3-08、R23）。見出しと tab は取得を待たずに出し、中身だけが FetchFrame で待つ（S1）。
// header（題・状態・現在の run・次の操作）は tab に関係なく h1 の下に出す。tab は ?tab= の search param。
// 概要 tab は目的の本文を先頭に置き、判断パネル（P3-09）と実行・routing（P3-10）はその下（最初の 1 画面に目的を入れる）。
// changes・files・artifacts（P3-13）は /tasks/:id/changes・/tasks/:id/files・/artifacts と同じ部品を置く。
// スマホ幅（md 未満）では概要 tab の中を「概要・判断・実行・木」の区画に切り替える（desktop は全区画を並べる）。
// 区画の切り替えは横 1 列の ScrollTabs に置き、上の tab 行と 2 段の箱にしない。
export function TaskDetailScreen({ taskId, tab }: { taskId: string; tab: TaskDetailTab }) {
  return (
    <ScreenFrame title={`タスクの詳細 ${taskId}`} route="/tasks/:id">
      <TaskDetailHeader taskId={taskId} />
      <BrowserPrerequisiteBanner taskId={taskId} />
      {/* 360 では 5 つの tab が 1 行に収まらず末尾の「成果物」が右で切れていた。狭い幅は tab の左右の余白を詰め、
          それでも収まらない幅では折り返して、どの tab も枠の中に全文で出す（fix-r6 narrow）。 */}
      <nav aria-label="タスクの表示" className="min-w-0 border-b border-border">
        <ul className="flex flex-wrap gap-x-1">
          {TASK_DETAIL_TABS.map((item) => (
            <li key={item.key}>
              <Link
                to="/tasks/$id"
                params={{ id: taskId }}
                search={{ tab: item.key === "overview" ? undefined : item.key }}
                // 既定の判定は search の部分一致で、tab の無い概要の link が他の tab でも「現在」になる。
                // 選択は ?tab= から決めた `tab` だけで示す（URL から復元した tab だけが aria-current）。
                activeOptions={{ exact: true }}
                aria-current={item.key === tab ? "page" : undefined}
                data-tab={item.key}
                className={`inline-flex min-h-11 items-center whitespace-nowrap px-2 text-label sm:px-3 ${
                  item.key === tab
                    ? "border-b-2 border-primary font-semibold text-foreground"
                    : "text-muted-foreground hover:text-foreground"
                }`}
              >
                {item.label}
              </Link>
            </li>
          ))}
        </ul>
      </nav>
      <TabBody taskId={taskId} tab={tab} />
    </ScreenFrame>
  );
}

function TabBody({ taskId, tab }: { taskId: string; tab: TaskDetailTab }) {
  switch (tab) {
    case "timeline":
      return <TimelineTab taskId={taskId} />;
    case "changes":
      return <TaskChangesPanel taskId={taskId} />;
    case "files":
      return <TaskFilesPanel taskId={taskId} search={{}} />;
    case "artifacts":
      return <TaskArtifactsPanel taskId={taskId} />;
    default:
      return <OverviewTab taskId={taskId} />;
  }
}

function OverviewTab({ taskId }: { taskId: string }) {
  const detail = useQuery(taskDetailQuery(taskId));
  const section = useMobileSection();
  return (
    <FetchFrame query={detail}>
      {detail.data ? (
        <div className="flex min-w-0 flex-col gap-4">
          <SectionSwitcher current={section.current} onSelect={section.select} />
          <OverviewView
            detail={detail.data}
            section={section.current}
            panels={
              <>
                <div className={mobileSectionClass("decision", section.current)}>
                  <DecisionPanel key={detail.data.task.id} detail={detail.data} />
                  <TaskDecisionsPanel key={`decisions-${detail.data.task.id}`} taskId={detail.data.task.id} />
                </div>
                <div className={mobileSectionClass("execution", section.current)}>
                  <ExecutionPanel key={`execution-${detail.data.task.id}`} detail={detail.data} />
                </div>
              </>
            }
          />
        </div>
      ) : null}
    </FetchFrame>
  );
}

// 開く区画は画面の状態（URL は変えない）。header・木の hash link で移ったときは、その移動先の区画を開いて
// 移動先まで scroll する。同じ hash へ 2 回移っても開き直すよう、history の key も見る。
function useMobileSection() {
  // hash だけでなく history の key も含めた 1 つの文字列にする（同じ hash への再移動でも effect が走る）。
  const nav = useLocation({ select: (value) => `${value.state.__TSR_key ?? ""}#${value.hash.replace(/^#/, "")}` });
  const [current, setCurrent] = useState<MobileSection>(() => sectionForHash(nav.slice(nav.indexOf("#"))) ?? "summary");
  useEffect(() => {
    const hash = nav.slice(nav.indexOf("#") + 1);
    const target = sectionForHash(hash);
    if (!target) return;
    setCurrent(target);
    const frame = requestAnimationFrame(() => document.getElementById(hash)?.scrollIntoView({ block: "start" }));
    return () => cancelAnimationFrame(frame);
  }, [nav]);
  return { current, select: setCurrent };
}

/** スマホ幅だけの区画切り替え（横 1 列、溢れたら枠の中で横 scroll）。desktop では隠れ、全区画が並ぶ。 */
function SectionSwitcher({
  current,
  onSelect,
}: {
  current: MobileSection;
  onSelect: (section: MobileSection) => void;
}) {
  return (
    <fieldset data-testid="mobile-sections" className="m-0 min-w-0 border-0 p-0 md:hidden">
      <legend className="sr-only">概要の区画</legend>
      <ScrollTabs className="flex gap-1">
        {MOBILE_SECTIONS.map((item) => (
          <button
            key={item.key}
            type="button"
            data-section={item.key}
            aria-pressed={item.key === current}
            onClick={() => onSelect(item.key)}
            className={`inline-flex min-h-11 min-w-11 shrink-0 items-center justify-center whitespace-nowrap rounded-md px-3 text-label focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring ${
              item.key === current
                ? "bg-accent font-semibold text-foreground"
                : "text-muted-foreground hover:text-foreground"
            }`}
          >
            {item.label}
          </button>
        ))}
      </ScrollTabs>
    </fieldset>
  );
}

function TimelineTab({ taskId }: { taskId: string }) {
  const timeline = useQuery(taskTimelineQuery(taskId));
  return <FetchFrame query={timeline}>{timeline.data ? <TimelineView timeline={timeline.data} /> : null}</FetchFrame>;
}

function BrowserPrerequisiteBanner({ taskId }: { taskId: string }) {
  const detail = useQuery(taskDetailQuery(taskId));
  return detail.data?.task.skills?.includes("browser-enabled") ? (
    <TaskBrowserPrerequisite detail={detail.data} />
  ) : null;
}
