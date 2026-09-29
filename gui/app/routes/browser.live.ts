import { runLiveViewRoute } from "~/celeris/browser-live.server";
import { getCelerisClient } from "~/celeris/client.server";
import type { Route } from "./+types/browser.live";

/**
 * `/browser/live/:taskId/:runId` と、その下の全ての経路（ADR-0080 D6）。どの入口も同じ owner guard を通す。
 * relay が設定されていれば express の middleware（`browser-live-relay.server.ts`）が先に処理するので、
 * ここに来るのは未設定（503）と relay が受け持たない経路だけ。dashboard の URL は応答・Location に出さない。
 */
export async function loader({ request, params }: Route.LoaderArgs): Promise<Response> {
  return runLiveViewRoute(getCelerisClient(), request, params.taskId, params.runId);
}

export async function action({ request, params }: Route.ActionArgs): Promise<Response> {
  return runLiveViewRoute(getCelerisClient(), request, params.taskId, params.runId);
}
