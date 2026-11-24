import type { ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { ApiError } from "../../api/client";
import type { ManifestView, ProjectDetail, ProjectOverview } from "../../api/generated/types";
import { overviewItems, ProjectOverviewPanel } from "../projects/project-overview-panel";
import {
  MANIFEST_SECTIONS,
  manifestPath,
  manifestWarnings,
  PUBLISH_SIGN_NOTICE,
  referenceLink,
  signability,
  signoffFailure,
  signTarget,
} from "./manifest-model";
import { ManifestPanel } from "./manifest-view";

// 守る不変条件（ADR 2026-11-23-project-observability-publish D3.2〜D3.5・D4）: manifest を claims → evidence → 再現条件 →
// limitations・未解決 → 人の署名 の順に出す、unsupported の claim・未署名・失効を文字の強い警告にする、署名の操作は人の login で
// 署名の決定が open のときだけ出し、確認の文に manifest の sha256 と「Celeris は外に送らない。公開は人が行う。」を入れる、
// 公開の操作を出さない、403 publication_requires_human を文で出す、案件の概要の O14 から manifest の画面へ移れる。
// base にはこの画面も route も無く（features/publish が無い）、既存の試験はどれも manifest を描かないので検出できない。

vi.mock("@tanstack/react-router", () => ({
  Link: ({
    children,
    to,
    params,
    search,
    hash,
    ...rest
  }: {
    children: ReactNode;
    to: string;
    params?: Record<string, string>;
    search?: Record<string, unknown>;
    hash?: string;
  }) => {
    const path = Object.entries(params ?? {}).reduce((acc, [key, value]) => acc.replace(`$${key}`, value), to);
    const query = new URLSearchParams(
      Object.entries(search ?? {})
        .filter(([, v]) => v !== undefined)
        .map(([k, v]) => [k, String(v)]),
    ).toString();
    return (
      <a href={`${path}${query ? `?${query}` : ""}${hash ? `#${hash}` : ""}`} {...rest}>
        {children}
      </a>
    );
  },
}));

const SHA = "1".repeat(64);

function view(over: Partial<ManifestView> = {}): ManifestView {
  return {
    task_id: "T1",
    project_id: "P1",
    manifest_id: "io-paper",
    version: 1,
    versions: [1],
    name: "publish/io-paper/v1.json",
    recorded_sha256: SHA,
    current_sha256: SHA,
    file_matches_record: true,
    violations: [],
    unsupported_claims: ["c2"],
    strong_warnings: [],
    target: { kind: "paper", venue: "journal" },
    claims: [
      { id: "c1", statement: "主張 1", confidence: "confirmed", evidence: ["e1"] },
      { id: "c2", statement: "主張 2", confidence: "estimated", evidence: ["e2"] },
    ],
    evidence: [
      {
        id: "e1",
        stance: "support",
        statement: "測定",
        references: [
          { kind: "commit", repo: "benchfs", sha: "b".repeat(40) },
          { kind: "chat", thread_id: "th", message_id: "m1", revision: 2, sha256: "d".repeat(64) },
        ],
      },
      { id: "e2", stance: "refute", statement: "反例", references: [{ kind: "url", url: "https://example.org/p" }] },
    ],
    reproduction: [
      {
        id: "r1",
        claims: ["c1"],
        commands: ["cargo bench"],
        environment: { repo: "benchfs", commit: "b".repeat(40), host_class: "hpc" },
        data: [],
        expected: "2 GB/s",
      },
    ],
    limitations: [{ id: "l1", statement: "比較は文献の値", claims: ["c2"] }],
    open_questions: [{ id: "q1", statement: "反例の扱い", blocking: true }],
    signoff: { state: "unsigned", key: "publish-signoff-v1", decision_id: "D1", decision_status: "open" },
    ...over,
  };
}

const login = { authenticated: true, authRequired: true };
const answer = vi.fn();

function problem(status: number, body: unknown) {
  return new ApiError("http", { method: "POST", path: "/api/decisions/D1/answer", status, body });
}

describe("publish manifest", () => {
  it("publish_manifest_ui_sections_follow_claims_evidence_reproduction_limitations_signoff", () => {
    expect(MANIFEST_SECTIONS.map(([key]) => key)).toEqual([
      "claims",
      "evidence",
      "reproduction",
      "limitations",
      "signoff",
    ]);
    const html = renderToStaticMarkup(<ManifestPanel view={view()} session={login} answer={answer} />);
    const order = [...html.matchAll(/data-section="([a-z]+)"/g)].map((m) => m[1]);
    expect(order).toEqual(["claims", "evidence", "reproduction", "limitations", "signoff"]);
    // 強い警告は 5 段より前。
    expect(html.indexOf('data-testid="manifest-warnings"')).toBeLessThan(html.indexOf('data-section="claims"'));
    expect(manifestPath("T1", "io-paper", 2)).toBe("/api/tasks/T1/publish/io-paper?version=2");
  });

  it("publish_manifest_ui_unsupported_claim_and_unsigned_are_strong_warnings", () => {
    const warnings = manifestWarnings(view({ strong_warnings: ["manifest_blocking_question"] }));
    expect(warnings.map((w) => w.code)).toEqual(["unsupported_claim", "unsigned", "manifest_blocking_question"]);
    expect(warnings[0]?.text).toBe("根拠の無い主張（unsupported）: c2");
    const html = renderToStaticMarkup(<ManifestPanel view={view()} session={login} answer={answer} />);
    expect(html).toContain('data-claim="c2" data-unsupported="true"');
    expect(html).toContain("根拠無し（unsupported）");
    expect(html).toContain('data-claim="c1" data-unsupported="false"');
    expect(manifestWarnings(view({ unsupported_claims: [], signoff: { ...view().signoff, state: "signed" } }))).toEqual(
      [],
    );
  });

  it("publish_manifest_ui_stale_superseded_and_invalid_are_strong_warnings_and_block_signing", () => {
    const stale = view({ signoff: { ...view().signoff, state: "stale", decision_status: "answered" } });
    expect(manifestWarnings(stale).map((w) => w.code)).toContain("signoff_stale");
    expect(signability(stale, login)).toEqual({
      ok: false,
      reason: "署名は失効しました。新しい版の manifest と署名の決定が要ります。",
    });
    const superseded = view({ versions: [1, 2], signoff: { ...view().signoff, state: "superseded" } });
    expect(manifestWarnings(superseded).find((w) => w.code === "signoff_superseded")?.text).toContain(
      "より新しい版 v2",
    );
    expect(signability(superseded, login).ok).toBe(false);
    const changed = view({
      file_matches_record: false,
      current_sha256: "f".repeat(64),
      violations: [{ rule: "shape", code: "publish_manifest_stale", path: "", detail: "書き換えられた" }],
    });
    expect(manifestWarnings(changed).map((w) => w.code)).toEqual([
      "unsupported_claim",
      "file_changed",
      "manifest_invalid",
      "unsigned",
    ]);
    expect(signability(changed, login)).toEqual({ ok: false, reason: "manifest が検査を通らないので署名できません。" });
  });

  it("publish_manifest_ui_sign_controls_need_login_and_open_decision", () => {
    expect(signability(view(), login)).toEqual({ ok: true, decisionId: "D1" });
    expect(signability(view(), undefined).ok).toBe(false);
    expect(signability(view(), { authenticated: false, authRequired: true }).ok).toBe(false);
    const noDecision = view({ signoff: { state: "unsigned", key: "publish-signoff-v1" } });
    expect(signability(noDecision, login)).toEqual({
      ok: false,
      reason: "署名の決定（publish-signoff-v1）がまだ出ていません。",
    });
    const loggedIn = renderToStaticMarkup(<ManifestPanel view={view()} session={login} answer={answer} />);
    expect(loggedIn).toContain('data-testid="manifest-sign"');
    const anonymous = renderToStaticMarkup(
      <ManifestPanel view={view()} session={{ authenticated: false, authRequired: true }} answer={answer} />,
    );
    expect(anonymous).not.toContain('data-testid="manifest-sign"');
    expect(anonymous).toContain("署名は人の login のときだけできます。");
  });

  it("publish_manifest_ui_confirm_text_has_sha256_and_no_publish_action", () => {
    expect(signTarget(view())).toBe(`publish/io-paper/v1.json（sha256:${SHA}）`);
    expect(PUBLISH_SIGN_NOTICE).toBe("Celeris は外に送らない。公開は人が行う。");
    const html = renderToStaticMarkup(<ManifestPanel view={view()} session={login} answer={answer} />);
    expect(html).toContain(PUBLISH_SIGN_NOTICE);
    const buttons = [...html.matchAll(/<button[^>]*>([^<]*)<\/button>/g)].map((m) => m[1]);
    expect(buttons).toEqual(["この版に署名する", "署名しない"]);
    expect(html).not.toMatch(/<button[^>]*>[^<]*(公開する|publish)[^<]*<\/button>/i);
  });

  it("publish_manifest_ui_signoff_failure_names_publication_requires_human", () => {
    expect(signoffFailure(problem(403, { code: "publication_requires_human", detail: "x" })).message).toBe(
      "署名は人だけができます（publication_requires_human）。",
    );
    expect(signoffFailure(problem(403, undefined)).message).toContain("publication_requires_human");
    expect(signoffFailure(problem(502, { error: "daemon_auth" })).message).toContain("daemon_auth");
    expect(signoffFailure(problem(409, { code: "publish_manifest_stale" }))).toEqual({
      message: "manifest の file が署名の決定の sha256 と違います（publish_manifest_stale）。",
      stale: true,
    });
    expect(signoffFailure(problem(422, { code: "publish_manifest_invalid", detail: "M3" })).message).toBe(
      "manifest が検査を通りません（publish_manifest_invalid）: M3",
    );
  });

  it("publish_manifest_ui_references_drill_down_to_commit_artifact_literature_chat", () => {
    expect(referenceLink({ kind: "commit", repo: "r", sha: "a".repeat(40) }, "T1")).toEqual({
      kind: "git",
      repo: "r",
      sha: "a".repeat(40),
      task_id: "T1",
    });
    expect(referenceLink({ kind: "artifact", task_id: "T2", name: "n", sha256: SHA }, "T1")).toEqual({
      kind: "artifact",
      task_id: "T2",
      name: "n",
      sha256: SHA,
    });
    expect(referenceLink({ kind: "knowledge", path: "p.md", sha256: SHA }, "T1")).toEqual({
      kind: "literature",
      path: "p.md",
      sha256: SHA,
    });
    const html = renderToStaticMarkup(<ManifestPanel view={view()} session={login} answer={answer} />);
    expect(html).toContain('href="/tasks/T1/changes"');
    expect(html).toContain('href="/?thread=th"');
    expect(html).toContain('href="https://example.org/p" target="_blank" rel="noopener noreferrer"');
    expect(html).toContain('href="/tasks/T1#decision-D1"');
  });

  it("publish_manifest_ui_overview_publish_readiness_links_to_manifest", () => {
    const items = overviewItems.map(([key]) => ({
      key,
      recorded: false,
      mark: null,
      entries: [],
    })) as ProjectOverview["items"];
    items[13] = {
      key: "publish_readiness",
      recorded: true,
      mark: "needs_decision",
      entries: [
        {
          id: "T1/io-paper",
          mark: "needs_decision",
          detail: { task_id: "T1", manifest_id: "io-paper", version: 3, signoff: "unsigned", violations: [] },
          links: [{ kind: "task", task_id: "T1" }],
        },
      ],
    };
    const overview = {
      schema: "celeris.project-overview/1",
      project_id: "P1",
      generated_at: "",
      request_sha256: SHA,
      items,
      warnings: [],
      truncated: [],
    } as unknown as ProjectOverview;
    const detail = { project: { id: "P1" }, tasks: [] } as unknown as ProjectDetail;
    const html = renderToStaticMarkup(<ProjectOverviewPanel overview={overview} detail={detail} />);
    expect(html).toContain('href="/tasks/T1/publish/io-paper?version=3"');
    expect(html).toContain("manifest io-paper v3 を読む");
  });
});
