import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import type { DecisionAnswerBody, DecisionOutcome, ManifestView } from "../../api/generated/types";
import { decisionKeys, projectKeys } from "../../api/queries/keys";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge, type BadgeTone } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { ConfirmDialog } from "../../components/ui/confirm-dialog";
import { Icon } from "../../components/ui/icon";
import { Section } from "../../components/ui/panel";
import { fetchSession, type Session } from "../../lib/session";
import { answerDecision } from "../chat/data/client";
import { decisionAnchor } from "../decisions/decision-model";
import { OverviewLink } from "../projects/project-overview-panel";
import {
  asClaims,
  asEvidence,
  asLimitations,
  asOpenQuestions,
  asReproduction,
  CONFIDENCE_LABEL,
  MANIFEST_SECTIONS,
  type ManifestConfidence,
  type ManifestStance,
  manifestKeys,
  manifestQuery,
  manifestWarnings,
  PUBLISH_SIGN_NOTICE,
  referenceLink,
  SIGNOFF_LABEL,
  STANCE_LABEL,
  signability,
  signoffFailure,
  signTarget,
} from "./manifest-model";

// /tasks/:id/publish/:manifestId（ADR 2026-11-23-project-observability-publish D3・D4）。人が publish の責任者として manifest を
// claims → evidence → 再現条件 → limitations・未解決 → 人の署名 の順に読み、unsupported の claim・未署名・失効を強い警告で見る。
// 署名は既存の決定の回答（`POST /decisions/{id}/answer`、option `sign`）で記録だけされる。公開の操作はこの画面に無い（D3.5）。

const confidenceTone: Record<ManifestConfidence, BadgeTone> = {
  confirmed: "success",
  estimated: "info",
  contradicted: "danger",
  needs_decision: "warning",
};

const stanceTone: Record<ManifestStance, BadgeTone> = { support: "success", refute: "danger", unknown: "neutral" };

const sessionQuery = { queryKey: ["gateway-session"] as const, queryFn: () => fetchSession() };

export type SignAnswer = (decisionId: string, body: DecisionAnswerBody) => Promise<DecisionOutcome>;

export function ManifestScreen({
  taskId,
  manifestId,
  version,
}: {
  taskId: string;
  manifestId: string;
  version?: number;
}) {
  const manifest = useQuery(manifestQuery(taskId, manifestId, version));
  const session = useQuery(sessionQuery);
  const client = useQueryClient();
  const reload = () => {
    void client.invalidateQueries({ queryKey: manifestKeys.all });
    void client.invalidateQueries({ queryKey: decisionKeys.all });
    void client.invalidateQueries({ queryKey: projectKeys.all });
  };
  return (
    <ScreenFrame title={`publish manifest ${manifestId}`} route="/tasks/:id/publish/:manifestId">
      <p className="text-label">
        <Link
          to="/tasks/$id"
          params={{ id: taskId }}
          className="inline-flex min-h-11 items-center gap-1 text-primary underline underline-offset-2"
        >
          <Icon name="chevron-left" size="sm" />
          タスクの詳細へ
        </Link>
      </p>
      <FetchFrame query={manifest} subject="publish manifest">
        {manifest.data ? (
          <ManifestPanel view={manifest.data} session={session.data} answer={answerDecision} onChanged={reload} />
        ) : null}
      </FetchFrame>
    </ScreenFrame>
  );
}

function Versions({ view }: { view: ManifestView }) {
  return (
    <p className="flex flex-wrap items-center gap-2 text-label" data-testid="manifest-versions">
      <span>版:</span>
      {view.versions.map((v) =>
        v === view.version ? (
          <strong key={v} aria-current="page">
            v{v}（表示中）
          </strong>
        ) : (
          <Link
            key={v}
            className="inline-flex min-h-11 min-w-11 items-center text-primary underline"
            to="/tasks/$id/publish/$manifestId"
            params={{ id: view.task_id, manifestId: view.manifest_id }}
            search={{ version: v }}
            data-version={v}
          >
            v{v}
          </Link>
        ),
      )}
    </p>
  );
}

/** manifest の本体。状態は持たず、props の view（daemon が導いた値）だけを出す。 */
export function ManifestPanel({
  view,
  session,
  answer,
  onChanged = () => {},
}: {
  view: ManifestView;
  session: Session | undefined;
  answer: SignAnswer;
  onChanged?: () => void;
}) {
  const warnings = manifestWarnings(view);
  const claims = asClaims(view);
  const evidence = asEvidence(view);
  const reproduction = asReproduction(view);
  const limitations = asLimitations(view);
  const questions = asOpenQuestions(view);
  const unsupported = new Set(view.unsupported_claims);
  const target = view.target as { kind?: string; venue?: string } | null | undefined;
  return (
    <div className="min-w-0 space-y-4" data-testid="publish-manifest" data-signoff={view.signoff.state}>
      <div className="space-y-1 break-words">
        <p>
          <code className="break-all">{view.name}</code>
        </p>
        <p className="text-label text-muted-foreground">
          記録の sha256: <code className="break-all">{view.recorded_sha256}</code>
          {view.current_sha256 && view.current_sha256 !== view.recorded_sha256 ? (
            <>
              、今の file の sha256: <code className="break-all">{view.current_sha256}</code>
            </>
          ) : null}
        </p>
        {target?.venue ? (
          <p className="text-label text-muted-foreground">
            公開する先（説明だけ。Celeris はここへ送らない）: {target.kind} {target.venue}
          </p>
        ) : null}
        <Versions view={view} />
      </div>
      <section
        aria-label="強い警告"
        data-testid="manifest-warnings"
        data-count={warnings.length}
        className={`space-y-2 rounded-md border p-3 ${warnings.length > 0 ? "border-destructive" : "border-border"}`}
      >
        <h2 className="font-semibold">強い警告 {warnings.length} 件</h2>
        {warnings.length === 0 ? (
          <p className="text-muted-foreground">強い警告はありません。</p>
        ) : (
          <ul className="space-y-1">
            {warnings.map((w, index) => (
              // biome-ignore lint/suspicious/noArrayIndexKey: 同じ code の警告（claim ごと）が並ぶので順を key に含める
              <li key={`${w.code}/${index}`} className="break-words text-destructive" data-warning={w.code}>
                <span className="font-semibold">強い警告</span>: {w.text}
              </li>
            ))}
          </ul>
        )}
      </section>
      <ol className="space-y-4" aria-label="manifest の 5 段">
        {MANIFEST_SECTIONS.map(([key, title]) => (
          <li key={key} data-section={key} className="rounded-md border border-border p-3">
            <Section title={title} level={2}>
              {key === "claims" ? (
                <ul className="space-y-2">
                  {claims.map((claim) => (
                    <li
                      key={claim.id}
                      data-claim={claim.id}
                      data-unsupported={unsupported.has(claim.id) ? "true" : "false"}
                      className="space-y-1 break-words"
                    >
                      <p className="flex flex-wrap items-center gap-2">
                        <code>{claim.id}</code>
                        <Badge tone={confidenceTone[claim.confidence]}>{CONFIDENCE_LABEL[claim.confidence]}</Badge>
                        {unsupported.has(claim.id) ? <Badge tone="danger">根拠無し（unsupported）</Badge> : null}
                        <span>{claim.statement}</span>
                      </p>
                      <p className="text-label text-muted-foreground">根拠: {claim.evidence.join("・") || "無し"}</p>
                    </li>
                  ))}
                </ul>
              ) : null}
              {key === "evidence" ? (
                <ul className="space-y-2">
                  {evidence.map((item) => (
                    <li
                      key={item.id}
                      data-evidence={item.id}
                      data-stance={item.stance}
                      className="space-y-1 break-words"
                    >
                      <p className="flex flex-wrap items-center gap-2">
                        <code>{item.id}</code>
                        <Badge tone={stanceTone[item.stance]}>{STANCE_LABEL[item.stance]}</Badge>
                        <span>{item.statement}</span>
                      </p>
                      <span className="flex flex-wrap items-center gap-x-3">
                        {item.references.map((reference, index) => (
                          // biome-ignore lint/suspicious/noArrayIndexKey: 参照は文書の順で固定
                          <OverviewLink key={index} link={referenceLink(reference, view.task_id)} />
                        ))}
                      </span>
                    </li>
                  ))}
                </ul>
              ) : null}
              {key === "reproduction" ? (
                reproduction.length === 0 ? (
                  <p className="text-muted-foreground">再現条件はありません。</p>
                ) : (
                  <ul className="space-y-3">
                    {reproduction.map((r) => (
                      <li key={r.id} data-reproduction={r.id} className="min-w-0 space-y-1 break-words">
                        <p>
                          <code>{r.id}</code>（主張 {r.claims.join("・")}）
                        </p>
                        <p className="text-label text-muted-foreground">
                          環境: {r.environment.repo}@<code className="break-all">{r.environment.commit}</code>・
                          {r.environment.host_class}
                        </p>
                        <pre className="overflow-x-auto rounded-md bg-muted p-2 text-label">
                          <code>{r.commands.join("\n")}</code>
                        </pre>
                        {r.data.length > 0 ? <p className="text-label">データ: {r.data.join("・")}</p> : null}
                        <p className="text-label">期待する結果: {r.expected}</p>
                      </li>
                    ))}
                  </ul>
                )
              ) : null}
              {key === "limitations" ? (
                <ul className="space-y-2">
                  {limitations.map((l) => (
                    <li key={l.id} data-limitation={l.id} className="break-words">
                      <code>{l.id}</code> {l.statement}
                      {l.claims.length > 0 ? `（主張 ${l.claims.join("・")}）` : ""}
                    </li>
                  ))}
                  {questions.map((q) => (
                    <li
                      key={q.id}
                      data-question={q.id}
                      data-blocking={q.blocking ? "true" : "false"}
                      className="break-words"
                    >
                      <code>{q.id}</code> 未解決: {q.statement}
                      {q.blocking ? <Badge tone="danger">公開を止める</Badge> : null}
                    </li>
                  ))}
                  {limitations.length + questions.length === 0 ? (
                    <li className="text-muted-foreground">limitations・未解決はありません。</li>
                  ) : null}
                </ul>
              ) : null}
              {key === "signoff" ? (
                <SignoffBlock view={view} session={session} answer={answer} onChanged={onChanged} />
              ) : null}
            </Section>
          </li>
        ))}
      </ol>
    </div>
  );
}

function SignoffBlock({
  view,
  session,
  answer,
  onChanged,
}: {
  view: ManifestView;
  session: Session | undefined;
  answer: SignAnswer;
  onChanged: () => void;
}) {
  const signoff = view.signoff;
  const can = signability(view, session);
  const send = (decisionId: string, option: "sign" | "reject") => async () => {
    try {
      await answer(decisionId, { option });
    } catch (error) {
      const failure = signoffFailure(error);
      if (failure.stale) onChanged();
      throw new Error(failure.message);
    }
    onChanged();
  };
  return (
    <div className="space-y-2 break-words" data-testid="manifest-signoff" data-state={signoff.state}>
      <p className="flex flex-wrap items-center gap-2">
        <Badge tone={signoff.state === "signed" ? "success" : "danger"} data-slot="signoff-state">
          {SIGNOFF_LABEL[signoff.state]}
        </Badge>
        <span className="text-label">決定 {signoff.key}</span>
        {signoff.answered_by ? (
          <span className="text-label text-muted-foreground">
            回答: {signoff.answered_by}
            {signoff.answered_at ? `（${signoff.answered_at}）` : ""}
          </span>
        ) : null}
      </p>
      <p data-testid="manifest-no-publish">{PUBLISH_SIGN_NOTICE}</p>
      {signoff.decision_id ? (
        <p className="text-label">
          <Link
            className="inline-flex min-h-11 items-center text-primary underline"
            to="/tasks/$id"
            params={{ id: view.task_id }}
            hash={decisionAnchor(signoff.decision_id)}
            data-link="decision"
          >
            task の決定の画面
          </Link>
        </p>
      ) : null}
      {can.ok ? (
        <div className="flex flex-wrap gap-2" data-testid="manifest-sign-actions">
          <ConfirmDialog
            trigger={<Button data-testid="manifest-sign">この版に署名する</Button>}
            title="publish manifest に署名"
            target={signTarget(view)}
            consequence={`${PUBLISH_SIGN_NOTICE} 署名は決定 ${signoff.key} への人の回答として記録だけされる。`}
            reversibility="本文が変われば署名は失効する。答えは task の決定の画面で変えられる。"
            followUp="この画面の「5. 人の署名」と、案件の概要の「公開の準備度」。"
            confirmLabel="この sha256 に署名する"
            onConfirm={send(can.decisionId, "sign")}
          />
          <ConfirmDialog
            trigger={
              <Button variant="secondary" data-testid="manifest-reject">
                署名しない
              </Button>
            }
            title="publish manifest に署名しない"
            target={signTarget(view)}
            consequence={`${PUBLISH_SIGN_NOTICE} 決定 ${signoff.key} に「署名しない」と回答する。`}
            reversibility="task の決定の画面で答えを変えられる。"
            followUp="この画面の「5. 人の署名」。"
            confirmLabel="署名しないと回答する"
            onConfirm={send(can.decisionId, "reject")}
          />
        </div>
      ) : (
        <p className="text-muted-foreground" data-testid="manifest-sign-unavailable">
          {can.reason}
        </p>
      )}
    </div>
  );
}
