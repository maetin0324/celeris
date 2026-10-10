import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { SavedCredentialItem, SavedCredentialList } from "../../api/generated/types";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { Button } from "../../components/ui/button";
import { ConfirmDialog } from "../../components/ui/confirm-dialog";
import { Section } from "../../components/ui/panel";
import { browserActionGate } from "./browser-query";

const queryKey = ["browser", "saved-credentials"];

export function SavedCredentialsPanel({ csrf }: { csrf: string }) {
  const client = useQueryClient();
  const query = useQuery({
    queryKey,
    queryFn: async ({ signal }): Promise<SavedCredentialList> => {
      const response = await fetch("/browser/credentials", { credentials: "same-origin", cache: "no-store", signal });
      if (!response.ok) throw new Error("保存済みログイン情報を取得できません。");
      return response.json();
    },
  });
  async function remove(selected: SavedCredentialItem) {
    if (!csrf) throw new Error("本人確認が必要です。");
    try {
      await browserActionGate.run(async () => {
        const response = await fetch(`/browser/credentials/${encodeURIComponent(selected.credential_id)}`, {
          method: "DELETE",
          credentials: "same-origin",
          cache: "no-store",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ csrf }),
        });
        if (!response.ok) throw new Error("delete failed");
      });
      await client.invalidateQueries({ queryKey });
    } catch {
      throw new Error("削除できませんでした。一覧を再取得して確認してください。");
    }
  }
  return (
    <Section title="保存済みログイン情報">
      <p className="text-label text-muted-foreground">
        保存済みの情報は、次の実行で使用を承認するだけで使えます。秘密は表示しません。保存期間は既定90日、管理者が1〜90日に設定できます。
      </p>
      <FetchFrame query={query} subject="保存済みログイン情報">
        {query.data?.items.length === 0 ? <p>保存済みのログイン情報はありません。</p> : null}
        <ul className="space-y-3">
          {query.data?.items.map((item) => (
            <li key={item.credential_id} className="flex flex-wrap items-center justify-between gap-3">
              <div>
                <p>{item.policy_id}</p>
                <p className="text-label text-muted-foreground">
                  登録: {new Date(item.created_at * 1000).toLocaleDateString()} / 期限:{" "}
                  {new Date(item.expires_at * 1000).toLocaleDateString()}
                </p>
              </div>
              <ConfirmDialog
                trigger={
                  <Button
                    variant="destructive"
                    disabled={!csrf}
                    aria-label={`${item.policy_id} の保存済みログイン情報を削除`}
                  >
                    削除
                  </Button>
                }
                title="保存済みログイン情報を削除"
                target={item.policy_id}
                consequence="次の使用にはIDとパスワードの再入力が必要になります。"
                reversibility="削除後は再入力して登録できます。"
                followUp="この一覧で削除結果を確認できます。"
                confirmLabel="削除する"
                onConfirm={() => remove(item)}
              />
            </li>
          ))}
        </ul>
      </FetchFrame>
    </Section>
  );
}
