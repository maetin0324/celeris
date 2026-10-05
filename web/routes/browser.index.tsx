import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";
import { Panel } from "../components/ui/panel";

export const Route = createFileRoute("/browser/")({ component: Screen });

function Screen() {
  return (
    <ScreenFrame title="ブラウザ" route="/browser" description="ブラウザ実行と対応待ちを確認します。">
      <Panel title="ブラウザ実行">実行一覧を準備しています。</Panel>
    </ScreenFrame>
  );
}
