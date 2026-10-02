import { runControlRequest } from "~/celeris/browser-control.server";
import { getCelerisClient } from "~/celeris/client.server";
import type { Route } from "./+types/browser.control";

export function loader({ request, params }: Route.LoaderArgs): Promise<Response> {
  return runControlRequest(getCelerisClient(), request, params);
}
export function action({ request, params }: Route.ActionArgs): Promise<Response> {
  return runControlRequest(getCelerisClient(), request, params);
}
