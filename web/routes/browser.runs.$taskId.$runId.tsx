import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";
import { Panel } from "../components/ui/panel";

export const Route = createFileRoute("/browser/runs/$taskId/$runId")({ component: Screen });

function Screen() {
  return (
    <ScreenFrame
      title="ブラウザ実行"
      route="/browser/runs/:taskId/:runId"
      breadcrumb={[{ label: "ブラウザ", link: { to: "/browser" } }, { label: "実行" }]}
    >
      <Panel title="実行の状態">Live View と操作状態を準備しています。</Panel>
    </ScreenFrame>
  );
}
