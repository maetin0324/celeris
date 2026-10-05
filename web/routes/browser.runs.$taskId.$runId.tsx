import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";
import { BrowserRunScreen } from "../features/browser/browser-run-screen";

export const Route = createFileRoute("/browser/runs/$taskId/$runId")({ component: Screen });

function Screen() {
  const { taskId, runId } = Route.useParams();
  return (
    <ScreenFrame
      title="ブラウザ実行"
      route="/browser/runs/:taskId/:runId"
      breadcrumb={[{ label: "ブラウザ", link: { to: "/browser" } }, { label: "実行" }]}
      description={`task ${taskId} の run ${runId} を監視・操作します。`}
    >
      <BrowserRunScreen taskId={taskId} runId={runId} />
    </ScreenFrame>
  );
}
