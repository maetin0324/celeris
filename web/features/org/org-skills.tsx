import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useId, useRef, useState } from "react";
import { ApiError, apiGet, apiMutate } from "../../api/client";
import type { EffectiveProfile, OrgNode, SkillDetailView, SkillList } from "../../api/generated/types";
import { orgKeys, skillKeys } from "../../api/queries/keys";
import { Markdown } from "../../components/content/markdown";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { Badge } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { ConfirmDialog } from "../../components/ui/confirm-dialog";
import { skillRemovalImpact, skillSource } from "./org-tree";

// 入力欄の枠は --color-input（DESIGN.md「入力欄」）。error は枠の色ではなく文と aria-invalid で示す。
const fieldClass =
  "block min-h-11 w-full min-w-0 rounded-md border border-input bg-surface px-3 py-2 text-body text-foreground focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring";
const FORBIDDEN_REASON =
  "この token には組織を変更する権限がありません（403）。skill の追加と外す操作は、権限のある token で開き直してから行ってください。";

function SkillPreview({ name }: { name: string }) {
  const query = useQuery({
    queryKey: ["skills", "detail", name],
    queryFn: ({ signal }) => apiGet<SkillDetailView>(`/api/skills/${encodeURIComponent(name)}`, signal),
  });
  return <FetchFrame query={query}>{query.data && <Markdown source={query.data.skill_md} />}</FetchFrame>;
}

function failureMessage(error: unknown) {
  const api = error instanceof ApiError ? error : null;
  if (api?.status === 409) return "状態が変わりました。最新の状態を確認してください。";
  const body = api?.body;
  const detail = body && typeof body === "object" ? (body as Record<string, unknown>).detail : null;
  if (typeof detail === "string") return detail;
  if (api?.kind === "timeout" || api?.kind === "network")
    return "結果を確認できません。再取得して状態を確認してください。";
  return "操作に失敗しました。";
}

const names = (nodes: readonly OrgNode[]) => nodes.map((item) => item.name).join("、");

export function OrgSkills({
  node,
  items,
  profile,
}: {
  node: OrgNode;
  items: readonly OrgNode[];
  profile?: EffectiveProfile;
}) {
  const client = useQueryClient();
  const list = useQuery({
    queryKey: skillKeys.list(),
    queryFn: ({ signal }) => apiGet<SkillList>("/api/skills", signal),
    retry: false,
  });
  const id = useId();
  const selectId = `${id}-select`;
  const hintId = `${id}-hint`;
  const errorId = `${id}-error`;
  const reasonId = `${id}-reason`;
  const selectRef = useRef<HTMLSelectElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const reasonRef = useRef<HTMLParagraphElement>(null);
  const [choice, setChoice] = useState("");
  const [preview, setPreview] = useState<string>();
  const [fieldError, setFieldError] = useState<string>();
  const [status, setStatus] = useState<string>();
  const [forbidden, setForbidden] = useState(false);
  const [focusTarget, setFocusTarget] = useState<{ to: "list" | "field" | "reason"; n: number }>();
  const moveFocus = (to: "list" | "field" | "reason") => setFocusTarget((prev) => ({ to, n: (prev?.n ?? 0) + 1 }));
  const busyRef = useRef(false);
  const [busy, setBusy] = useState(false);
  const own = node.profile?.skills_mounts ?? [];
  const effective = profile?.skills_mounts ?? own;
  const inherited = effective.filter((name) => !own.includes(name));
  const options = list.data?.items.filter((item) => !effective.includes(item.name)) ?? [];
  const denied = forbidden || (list.error instanceof ApiError && list.error.kind === "forbidden");
  const disabled = busy || denied;

  // 失敗時は欄へ、確定後は一覧へ、403 では理由へ focus を移す。ConfirmDialog は閉じるときに trigger へ
  // focus を返す（Radix は閉じた後の setTimeout で返す）ので、ダイアログが消えてからその後ろで移す。
  useEffect(() => {
    if (!focusTarget) return;
    const element = { list: listRef, field: selectRef, reason: reasonRef }[focusTarget.to].current;
    let frames = 0;
    let frame = 0;
    let timer = 0;
    const tick = () => {
      if (document.querySelector('[role="alertdialog"]') && frames++ < 120) {
        frame = requestAnimationFrame(tick);
        return;
      }
      timer = window.setTimeout(() => element?.focus(), 0);
    };
    tick();
    return () => {
      cancelAnimationFrame(frame);
      window.clearTimeout(timer);
    };
  }, [focusTarget]);

  async function refresh() {
    await Promise.all([
      client.invalidateQueries({ queryKey: orgKeys.list(), exact: true }),
      client.invalidateQueries({ queryKey: skillKeys.list(), exact: true }),
    ]);
  }

  /** 成功なら true。失敗は呼び出し側が欄か確認ダイアログに出す。403 は操作を無効にする。 */
  async function change(
    skill: string,
    mount: boolean,
  ): Promise<{ ok: true } | { ok: false; forbidden?: boolean; message: string }> {
    if (busyRef.current) return { ok: false, message: "前の操作を処理中です。" };
    busyRef.current = true;
    setBusy(true);
    setStatus(undefined);
    try {
      await apiMutate(
        mount ? "POST" : "DELETE",
        mount
          ? `/api/org/${encodeURIComponent(node.id)}/skills`
          : `/api/org/${encodeURIComponent(node.id)}/skills/${encodeURIComponent(skill)}`,
        mount ? { skill } : undefined,
      );
      // org の詳細情報は一覧応答に含まれる。関連する org と skill 一覧だけ更新する。
      await refresh();
      return { ok: true };
    } catch (error) {
      const api = error instanceof ApiError ? error : null;
      if (api?.kind === "forbidden") {
        setForbidden(true);
        return { ok: false, forbidden: true, message: FORBIDDEN_REASON };
      }
      if (api?.status === 409) await refresh();
      return { ok: false, message: failureMessage(error) };
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  }

  async function mount() {
    if (!choice) {
      setFieldError("mount する skill を選んでください。");
      moveFocus("field");
      return;
    }
    setFieldError(undefined);
    const result = await change(choice, true);
    if (result.ok) {
      setStatus(`${choice} を mount しました。${node.name} と配下の worker に届きます。`);
      setChoice("");
      moveFocus("list");
    } else if (result.forbidden) {
      moveFocus("reason");
    } else {
      setFieldError(result.message);
      moveFocus("field");
    }
  }

  async function unmount(skill: string) {
    const result = await change(skill, false);
    if (result.ok) {
      setStatus(`${skill} を外しました。`);
      moveFocus("list");
    } else if (result.forbidden) {
      // 権限が無いので再試行させない。ダイアログを閉じ、一覧の上の理由を読ませる。
      moveFocus("reason");
    } else {
      // ConfirmDialog は throw された理由を出して開いたままにする。
      throw new Error(result.message);
    }
  }

  function impactText(skill: string) {
    const impact = skillRemovalImpact(items, node, skill);
    if (impact.stillFrom)
      return `上位の ${impact.stillFrom.name} も ${skill} を mount しているので、worker には届き続けます。${node.name} の独自設定から消えるだけです。`;
    return `${names(impact.affected)} の worker に ${skill} が届かなくなります。実行中の run には影響せず、次の run から外れます。`;
  }

  return (
    <section className="min-w-0 space-y-3" aria-label="担当の skill">
      <div>
        <h4 className="text-label font-semibold text-foreground">skill</h4>
        <p className="mt-1 text-label text-muted-foreground">
          mount した skill は、この担当と配下の worker の作業場所に届きます。
        </p>
      </div>
      {denied && (
        <p
          ref={reasonRef}
          id={reasonId}
          tabIndex={-1}
          role="alert"
          className="rounded-md bg-warning p-3 text-label text-warning-foreground"
        >
          {FORBIDDEN_REASON}
        </p>
      )}
      {own.length === 0 && inherited.length === 0 ? (
        <p className="text-label text-muted-foreground">mount された skill はありません。</p>
      ) : null}
      <ul
        ref={listRef}
        tabIndex={-1}
        aria-label="mount された skill"
        className="divide-y divide-border rounded-md border border-border focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
      >
        {own.map((name) => (
          <li key={name} className="flex flex-col gap-2 px-3 py-2 sm:flex-row sm:items-center">
            <div className="flex min-w-0 flex-1 flex-wrap items-center gap-2">
              <span className="min-w-0 break-all text-body font-medium text-foreground">{name}</span>
              <Badge tone="info">独自</Badge>
            </div>
            <div className="flex flex-wrap gap-2">
              <Button
                variant="ghost"
                onClick={() => setPreview(preview === name ? undefined : name)}
                aria-expanded={preview === name}
              >
                {name} を表示
              </Button>
              <ConfirmDialog
                trigger={
                  <Button
                    variant="destructive"
                    disabled={disabled}
                    aria-describedby={denied ? reasonId : undefined}
                    aria-label={`${name} を外す`}
                  >
                    外す
                  </Button>
                }
                title={`${name} を外しますか`}
                target={`${node.name} の skill「${name}」`}
                consequence={impactText(name)}
                reversibility={`同じ欄で ${name} を選んで mount すると戻せます。`}
                followUp="この欄の一覧と、配下の担当の skill 数で確かめられます。"
                confirmLabel={`${name} を外す`}
                onConfirm={() => unmount(name)}
              />
            </div>
          </li>
        ))}
        {inherited.map((name) => {
          const source = skillSource(items, node, name);
          return (
            <li key={name} className="flex flex-col gap-2 px-3 py-2 sm:flex-row sm:items-center">
              <div className="flex min-w-0 flex-1 flex-wrap items-center gap-2">
                <span className="min-w-0 break-all text-body font-medium text-foreground">{name}</span>
                <Badge tone="neutral">{source ? `${source.name} から継承` : "継承"}</Badge>
              </div>
              <div className="flex flex-wrap gap-2">
                <Button
                  variant="ghost"
                  onClick={() => setPreview(preview === name ? undefined : name)}
                  aria-expanded={preview === name}
                >
                  {name} を表示
                </Button>
              </div>
            </li>
          );
        })}
      </ul>
      {status && (
        <p role="status" className="text-label text-foreground">
          {status}
        </p>
      )}
      {list.isError && !denied && (
        <p role="alert" className="rounded-md bg-danger p-3 text-label text-danger-foreground">
          skill の一覧を読めませんでした。
        </p>
      )}
      {list.data && (
        <form
          noValidate
          className="flex flex-col gap-2 sm:flex-row sm:items-end"
          onSubmit={(event) => {
            event.preventDefault();
            void mount();
          }}
        >
          <div className="min-w-0 flex-1 space-y-1">
            <label htmlFor={selectId} className="block text-label font-medium text-foreground">
              mount する skill
            </label>
            <select
              id={selectId}
              ref={selectRef}
              className={fieldClass}
              value={choice}
              disabled={denied}
              aria-invalid={fieldError ? true : undefined}
              aria-describedby={[hintId, fieldError ? errorId : "", denied ? reasonId : ""].filter(Boolean).join(" ")}
              onChange={(event) => {
                setChoice(event.target.value);
                if (event.target.value) setFieldError(undefined);
              }}
            >
              <option value="">選ぶ</option>
              {options.map((item) => (
                <option key={item.name} value={item.name}>
                  {item.name}
                </option>
              ))}
            </select>
            <p id={hintId} className="text-label text-muted-foreground">
              {options.length === 0
                ? "追加できる skill はありません。"
                : `追加できる skill は ${options.length} 件です。`}
            </p>
            {fieldError && (
              <p id={errorId} className="text-label font-medium text-danger-foreground">
                {fieldError}
              </p>
            )}
          </div>
          <Button type="submit" variant="primary" disabled={disabled}>
            mount
          </Button>
        </form>
      )}
      {preview && <SkillPreview name={preview} />}
    </section>
  );
}
