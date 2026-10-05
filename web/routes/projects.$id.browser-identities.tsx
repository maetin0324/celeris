import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";
import { Panel } from "../components/ui/panel";

export const Route = createFileRoute("/projects/$id/browser-identities")({ component: Screen });

function Screen() {
  const { id } = Route.useParams();
  return (
    <ScreenFrame
      title="ブラウザの本人情報"
      route="/projects/:id/browser-identities"
      breadcrumb={[
        { label: "案件", link: { to: "/projects" } },
        { label: "案件詳細", link: { to: "/projects/$id", params: { id } } },
        { label: "本人情報" },
      ]}
    >
      <Panel title="本人情報">登録済みの本人情報を準備しています。</Panel>
    </ScreenFrame>
  );
}
