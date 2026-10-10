import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { LiveViewFrame, liveViewState } from "./live-view-frame";

const owner = { available: true, isOwner: true };
const running = { state: "RUNNING" as const, live_path: "/browser/live/T1/R1", live: undefined };

describe("launcher Live View", () => {
  it("exposes a frame only to the owner and labels credential sessions", () => {
    expect(liveViewState({ taskId: "T1", runId: "R1", run: running, owner, authInterval: false })).toEqual({
      kind: "frame",
      href: "/browser/live/T1/R1",
    });
    const html = renderToStaticMarkup(
      <LiveViewFrame state={{ kind: "frame", href: "/browser/live/T1/R1" }} taskLabel="T1" credentialSession />,
    );
    expect(html).toContain("credential session の映像は本人だけに表示されます。");
    expect(html).toContain("読み取り専用");
  });

  it("shows the launcher protocol disabled reason and refuses non-owners", () => {
    expect(
      liveViewState({
        taskId: "T1",
        runId: "R1",
        run: { ...running, live: { state: "disabled", reason: "launcher_protocol_no_live_frames" } },
        owner,
        authInterval: false,
      }),
    ).toEqual({ kind: "unavailable", reason: "launcher_protocol_no_live_frames" });
    expect(
      liveViewState({
        taskId: "T1",
        runId: "R1",
        run: running,
        owner: { ...owner, isOwner: false },
        authInterval: false,
      }),
    ).toEqual({ kind: "unavailable", reason: "not_owner" });
  });
});
