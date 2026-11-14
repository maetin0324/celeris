import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { ProjectRepo } from "../../api/generated/types";
import { autoIntegrateDisabledReason, RepoAutoIntegrate, repoAutoIntegrateAction } from "./repo-auto-integrate";

const base = {
  created_at: "2026-11-14T00:00:00Z",
  id: "R1",
  kind: "git",
  location: { kind: "local", path: "/w/a" },
  name: "a",
} as unknown as ProjectRepo;
const remote = { ...base, location: { kind: "remote", cluster: "c", path: "/w/a" } } as ProjectRepo;

describe("リポジトリの自動取り込みの切替", () => {
  it("PATCH /api/repos/{id} に auto_integrate だけを送る", () => {
    expect(repoAutoIntegrateAction("R1", true)).toMatchObject({
      method: "PATCH",
      path: "/api/repos/R1",
      body: { auto_integrate: true },
    });
    expect(repoAutoIntegrateAction("a/b", false).path).toBe("/api/repos/a%2Fb");
  });

  it("local の git は切り替えられ、remote と dir は理由つきで無効", () => {
    expect(autoIntegrateDisabledReason(base)).toBeNull();
    expect(autoIntegrateDisabledReason(remote)).toContain("remote");
    expect(autoIntegrateDisabledReason({ ...base, kind: "dir" } as ProjectRepo)).toContain("git でない");
  });

  it("無効の行は checkbox が disabled で理由を出し、有効の行は戻せる", () => {
    const off = renderToStaticMarkup(<RepoAutoIntegrate repo={remote} pending={false} onToggle={() => {}} />);
    expect(off).toContain("disabled");
    expect(off).toContain("repo-auto-integrate-reason");
    const on = renderToStaticMarkup(
      <RepoAutoIntegrate repo={{ ...remote, auto_integrate: true }} pending={false} onToggle={() => {}} />,
    );
    expect(on).toContain("checked");
    expect(on).not.toContain("disabled");
  });
});
