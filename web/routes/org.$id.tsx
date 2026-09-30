import { createFileRoute } from "@tanstack/react-router";
import { ScreenFrame } from "../components/shell/screen-frame";
import { ConsoleView } from "../features/console/console-view";

// R08 /org/:id（その人の Console。中身は features/console）。
export const Route = createFileRoute("/org/$id")({
  component: Screen,
});

function Screen() {
  const params = Route.useParams();
  return (
    <ScreenFrame title={`組織の人 ${params.id}`} route="/org/:id">
      <ConsoleView key={params.id} scope={`node:${params.id}`} label={params.id} />
    </ScreenFrame>
  );
}
