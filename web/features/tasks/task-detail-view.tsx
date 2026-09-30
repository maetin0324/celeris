import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { TaskArtifactsPanel } from "../artifacts/task-artifacts-view";
import { TaskChangesPanel } from "../changes/changes-view";
import { TaskFilesPanel } from "../files/task-files-view";
import { DecisionPanel } from "./decision-panel";
import { ExecutionPanel } from "./execution-panel";
import { OverviewView } from "./overview-view";
import { taskDetailQuery, taskTimelineQuery } from "./task-detail-query";
import { TASK_DETAIL_TABS, type TaskDetailTab } from "./task-detail-tabs";
import { TimelineView } from "./timeline-view";

// /tasks/:id の枠（P3-08、R23）。見出しと tab は取得を待たずに出し、中身だけが FetchFrame で待つ（S1）。
// tab は ?tab= の search param。判断パネル（P3-09）と実行・routing（P3-10）は overview の上。
// changes・files・artifacts（P3-13）は /tasks/:id/changes・/tasks/:id/files・/artifacts と同じ部品を置く。
export function TaskDetailScreen({ taskId, tab }: { taskId: string; tab: TaskDetailTab }) {
  return (
    <ScreenFrame title={`タスクの詳細 ${taskId}`} route="/tasks/:id">
      <nav aria-label="タスクの表示" className="min-w-0 overflow-x-auto border-b border-neutral-300">
        <ul className="flex gap-1">
          {TASK_DETAIL_TABS.map((item) => (
            <li key={item.key}>
              <Link
                to="/tasks/$id"
                params={{ id: taskId }}
                search={{ tab: item.key === "overview" ? undefined : item.key }}
                aria-current={item.key === tab ? "page" : undefined}
                data-tab={item.key}
                className={`inline-flex min-h-11 items-center px-3 text-sm ${
                  item.key === tab ? "border-b-2 border-neutral-900 font-semibold" : "text-neutral-600"
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
  return (
    <FetchFrame query={detail}>
      {detail.data ? (
        <div className="min-w-0 space-y-4">
          <DecisionPanel key={detail.data.task.id} detail={detail.data} />
          <ExecutionPanel key={`execution-${detail.data.task.id}`} detail={detail.data} />
          <OverviewView detail={detail.data} />
        </div>
      ) : null}
    </FetchFrame>
  );
}

function TimelineTab({ taskId }: { taskId: string }) {
  const timeline = useQuery(taskTimelineQuery(taskId));
  return <FetchFrame query={timeline}>{timeline.data ? <TimelineView timeline={timeline.data} /> : null}</FetchFrame>;
}
