import { runLiveViewRoute } from "~/celeris/browser-live.server";
import { getCelerisClient } from "~/celeris/client.server";
import type { Route } from "./+types/browser.live";

/**
 * `/browser/live/:taskId/:runId` と、その下の全ての経路（assets・API・WebSocket upgrade）（ADR-0080 D6）。
 * どの入口も同じ owner guard を通す。dashboard の URL は応答・Location に出さない。
 */
export async function loader({ request, params }: Route.LoaderArgs): Promise<Response> {
  return runLiveViewRoute(getCelerisClient(), request, params.taskId, params.runId);
}

export async function action({ request, params }: Route.ActionArgs): Promise<Response> {
  return runLiveViewRoute(getCelerisClient(), request, params.taskId, params.runId);
}
