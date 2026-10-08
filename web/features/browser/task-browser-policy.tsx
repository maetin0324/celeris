import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useId, useRef, useState } from "react";
import { isApiError } from "../../api/client";
import type { BrowserAction, TaskDetail } from "../../api/generated/types";
import { taskKeys } from "../../api/queries/keys";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { Button, buttonVariants } from "../../components/ui/button";
import { fieldClassName } from "../../components/ui/input";
import { Notice } from "../../components/ui/notice";
import { Section } from "../../components/ui/panel";
import { taskTimelineQuery } from "../tasks/task-detail-query";
import {
  browserPrerequisite,
  editedPolicy,
  POLICY_ACTIONS,
  policyEditable,
  policyOriginLabel,
  policySaveError,
  saveTaskBrowserPolicy,
  type TaskBrowserPolicy,
  taskBrowserPolicyQuery,
} from "./task-browser-policy-model";

export function TaskBrowserPrerequisite({ detail }: { detail: TaskDetail }) {
  const timeline = useQuery({
    ...taskTimelineQuery(detail.task.id),
    enabled: detail.task.status === "blocked",
    retry: false,
  });
  const prerequisite = browserPrerequisite(detail.task.status, timeline.data);
  if (!prerequisite) return null;
  return (
    <BrowserPrerequisiteNotice
      message={prerequisite.message}
      policyMissing={prerequisite.code === "browser_policy_missing"}
    />
  );
}

export function BrowserPrerequisiteNotice({ message, policyMissing }: { message: string; policyMissing: boolean }) {
  return (
    <Notice title={message} data-testid="browser-prerequisite">
      <p>
        {policyMissing
          ? "概要のブラウザ policy に必要なサイトを入力してください。"
          : "運用者が適合台帳を配置すると、このタスクは自動で再開します。"}
      </p>
      <Link to="/browser/settings" className={buttonVariants({ variant: "secondary" })}>
        ブラウザ設定を確認
      </Link>
      <details className="mt-2">
        <summary className="min-h-11 cursor-pointer py-2 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring">
          運用手順を開く
        </summary>
        <div className="space-y-2 break-words">
          <p>
            運用者は本番 host で <code>celerisctl browser doctor</code> を実行し、不足している前提を確認します。
          </p>
          <p>
            台帳の未配置・古い版・破損の場合は、稼働中 release の SHA を確認して{" "}
            <code>bash scripts/selfdeploy/browser-ledger.sh &lt;sha12&gt; --force</code>{" "}
            で再生成します。台帳を手書きして承認や証拠の検査を省略しないでください。
          </p>
          <p>
            doctor で台帳の項目が OK になり、タスクが実行待ちへ戻ることを確認します。詳細はリポジトリの{" "}
            <code>docs/ops/browser-prod.md</code> を参照してください。
          </p>
        </div>
      </details>
    </Notice>
  );
}

export function TaskBrowserPolicySection({ detail }: { detail: TaskDetail }) {
  const taskId = detail.task.id;
  const policy = useQuery({ ...taskBrowserPolicyQuery(taskId), retry: false });
  const timeline = useQuery({ ...taskTimelineQuery(taskId), enabled: detail.task.status === "blocked", retry: false });
  const editable = policyEditable(detail.task.status, browserPrerequisite(detail.task.status, timeline.data) !== null);
  const queryClient = useQueryClient();
  return (
    <Section
      title="ブラウザ policy"
      description="このタスクが使えるサイトと操作を制限します。担当課の許可と起票時のサイト制限も適用されます。"
    >
      <Button
        variant="secondary"
        disabled={policy.isFetching}
        onClick={() => {
          void policy.refetch();
          void timeline.refetch();
        }}
      >
        policy を再取得
      </Button>
      <FetchFrame query={policy}>
        {policy.data ? (
          <TaskBrowserPolicyView
            key={taskId}
            policy={policy.data.policy}
            editable={editable && !policy.isError && !(detail.task.status === "blocked" && timeline.isError)}
            taskId={taskId}
            initialDomains={detail.task.requirements?.browser?.allowed_domains ?? []}
            onSaved={async () => {
              await Promise.all([
                queryClient.invalidateQueries({ queryKey: taskBrowserPolicyQuery(taskId).queryKey }),
                queryClient.invalidateQueries({ queryKey: taskKeys.detail(taskId) }),
                queryClient.invalidateQueries({ queryKey: taskKeys.timelines(taskId) }),
              ]);
            }}
          />
        ) : null}
      </FetchFrame>
    </Section>
  );
}

export function TaskBrowserPolicyView({
  policy,
  editable,
  taskId,
  initialDomains = [],
  onSaved,
}: {
  policy: TaskBrowserPolicy | null;
  editable: boolean;
  taskId: string;
  initialDomains?: string[];
  onSaved: () => Promise<void>;
}) {
  const prefix = useId();
  const [editing, setEditing] = useState(false);
  const [domains, setDomains] = useState((policy?.network_domains ?? initialDomains).join("\n"));
  const [credentials, setCredentials] = useState((policy?.credential_policy_ids ?? []).join("\n"));
  const [actions, setActions] = useState<BrowserAction[]>(
    policy?.allowed_actions ?? ["navigate", "click", "snapshot", "extract", "screenshot", "download", "scroll"],
  );
  const [pending, setPending] = useState(false);
  const sending = useRef(false);
  const [invalid, setInvalid] = useState(false);
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);
  async function save() {
    if (sending.current) return;
    sending.current = true;
    setPending(true);
    setInvalid(false);
    setMessage(null);
    try {
      await saveTaskBrowserPolicy(taskId, editedPolicy(policy, domains, credentials, actions));
      setMessage({ ok: true, text: "ブラウザ policy を保存しました。" });
      setEditing(false);
      await onSaved();
    } catch (error) {
      setInvalid(isApiError(error) && error.kind === "validation");
      setMessage({ ok: false, text: policySaveError(error) });
    } finally {
      sending.current = false;
      setPending(false);
    }
  }
  return (
    <div className="min-w-0 space-y-3" data-testid="task-browser-policy">
      {policy ? (
        <dl className="space-y-2 break-words">
          <div>
            <dt className="text-label text-muted-foreground">設定の由来</dt>
            <dd>{policyOriginLabel(policy)}</dd>
          </div>
          <div>
            <dt className="text-label text-muted-foreground">許可サイト</dt>
            <dd>{policy.network_domains.join("、") || "未設定"}</dd>
          </div>
          <div>
            <dt className="text-label text-muted-foreground">許可操作</dt>
            <dd>{policy.allowed_actions.map((action) => POLICY_ACTIONS[action]).join("、") || "未設定"}</dd>
          </div>
          <div>
            <dt className="text-label text-muted-foreground">Credential policy</dt>
            <dd>{policy.credential_policy_ids.join("、") || "未設定（資格情報を使いません）"}</dd>
          </div>
          {policy.approval_actions.length ? (
            <div>
              <dt className="text-label text-muted-foreground">追加の承認対象</dt>
              <dd>{policy.approval_actions.map((action) => POLICY_ACTIONS[action]).join("、")}</dd>
            </div>
          ) : null}
        </dl>
      ) : (
        <p>ブラウザ policy は未設定です。必要なサイトを指定してください。</p>
      )}
      <p className="text-label text-muted-foreground">
        クリック・ダウンロード・資格情報の使用は、許可操作に含めても毎回、人の承認が必要です。ID と password
        はここには入力しません。
      </p>
      {message ? (
        <p id={`${prefix}-result`} role={message.ok ? "status" : "alert"}>
          {message.text}
        </p>
      ) : null}
      {editable ? (
        !editing ? (
          <Button
            variant="secondary"
            disabled={pending}
            onClick={() => {
              setDomains((policy?.network_domains ?? initialDomains).join("\n"));
              setCredentials((policy?.credential_policy_ids ?? []).join("\n"));
              setActions(
                policy?.allowed_actions ?? [
                  "navigate",
                  "click",
                  "snapshot",
                  "extract",
                  "screenshot",
                  "download",
                  "scroll",
                ],
              );
              setMessage(null);
              setEditing(true);
            }}
          >
            ブラウザ policy を編集
          </Button>
        ) : null
      ) : (
        <p className="text-label text-muted-foreground">
          下書き・実行待ち、またはブラウザの前提不足で停止しているときに編集できます。
        </p>
      )}
      {editing && editable ? (
        <form
          className="space-y-4"
          aria-busy={pending}
          onSubmit={(event) => {
            event.preventDefault();
            void save();
          }}
        >
          <fieldset disabled={pending} className="min-w-0 space-y-4">
            <legend className="sr-only">ブラウザ policy の編集</legend>
            <div>
              <label htmlFor={`${prefix}-domains`}>許可サイト（1 行に 1 origin）</label>
              <textarea
                id={`${prefix}-domains`}
                className={fieldClassName}
                rows={3}
                value={domains}
                required
                aria-invalid={invalid}
                aria-describedby={`${prefix}-hint ${prefix}-result`}
                onChange={(event) => setDomains(event.target.value)}
              />
              <p id={`${prefix}-hint`} className="text-label text-muted-foreground">
                例: https://manaba.example.ac.jp。起票時のサイト制限や担当課の許可を広げる設定ではありません。
              </p>
            </div>
            <fieldset aria-describedby={`${prefix}-result`}>
              <legend>許可操作</legend>
              {(Object.keys(POLICY_ACTIONS) as BrowserAction[]).map((action) => (
                <label key={action} className="flex min-h-11 items-center gap-2">
                  <input
                    type="checkbox"
                    checked={actions.includes(action)}
                    onChange={(event) =>
                      setActions(
                        event.target.checked ? [...actions, action] : actions.filter((item) => item !== action),
                      )
                    }
                  />
                  {POLICY_ACTIONS[action]}
                </label>
              ))}
            </fieldset>
            <div>
              <label htmlFor={`${prefix}-credentials`}>Credential policy ID（1 行に 1 ID）</label>
              <textarea
                id={`${prefix}-credentials`}
                className={fieldClassName}
                rows={2}
                aria-invalid={invalid}
                aria-describedby={`${prefix}-result`}
                value={credentials}
                onChange={(event) => setCredentials(event.target.value)}
              />
              <p className="text-label text-muted-foreground">
                ブラウザ設定で登録した policy ID を指定します。使う場合は「資格情報を使う」も選びます。
              </p>
            </div>
            <Button type="submit">{pending ? "保存中…" : "ブラウザ policy を保存"}</Button>
          </fieldset>
        </form>
      ) : null}
    </div>
  );
}
