import { type ReactNode, useEffect } from "react";
import { createRoot } from "react-dom/client";
import "../../../styles.css";
import {
  DisconnectedState,
  EmptyState,
  ErrorNotice,
  LoadingState,
  PermissionDeniedState,
  StaleState,
} from "../../fetch-state/fetch-frame";
import { Badge, badgeTones } from "../badge";
import { Button } from "../button";
import { CodeBlock, LogSurface } from "../code-block";
import { ConfirmDialog } from "../confirm-dialog";
import { DataList } from "../data-list";
import { Drawer } from "../drawer";
import { Icon, IconOnlyButton } from "../icon";
import { Input } from "../input";
import { Kbd } from "../kbd";
import { Notice } from "../notice";
import { Panel, Section } from "../panel";
import { ScrollTabs } from "../scroll-tabs";
import { Select } from "../select";
import { ShortId } from "../short-id";
import { StatusBadge, statusLabel } from "../status-badge";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "../table";

// 部品と状態を 1 画面に並べる確認用 fixture（reviewer が screenshot で DESIGN.md との一致を見る）。
// ?open=confirm / ?open=drawer で重なりの部品を開いた状態で描く。
const open = new URLSearchParams(window.location.search).get("open");
const noop = () => undefined;
// screenshot を決定的にするため固定の時刻を使う。
const fixedTime = Date.UTC(2026, 9, 4, 3, 0);

function Gallery({ title, children }: { title: string; children: ReactNode }) {
  return (
    <Section title={title} className="border-t border-border pt-4">
      <div className="mt-2 flex min-w-0 flex-col gap-3">{children}</div>
    </Section>
  );
}

/** 開いた状態で描くため、mount 後に trigger を押す（ConfirmDialog は open を外から受けない）。 */
function useOpenOnMount(triggerId: string, when: boolean) {
  useEffect(() => {
    if (when) document.getElementById(triggerId)?.click();
  }, [triggerId, when]);
}

function App() {
  useOpenOnMount("confirm-trigger", open === "confirm");
  return (
    <main className="mx-auto flex max-w-5xl min-w-0 flex-col gap-6 p-4">
      <h1 className="text-title font-semibold">部品と状態の一覧</h1>

      <Gallery title="StatusBadge">
        <div className="flex flex-wrap gap-2">
          {Object.keys(statusLabel).map((status) => (
            <StatusBadge key={status} status={status} />
          ))}
          <StatusBadge status="mystery_state" />
        </div>
        <p className="text-label text-muted-foreground">
          running は静止した印と「実行中」の文字、未知の状態（mystery_state）は neutral の「未確認」で出す。
        </p>
      </Gallery>

      <Gallery title="Badge と Button">
        <div className="flex flex-wrap gap-2">
          {badgeTones.map((tone) => (
            <Badge key={tone} tone={tone}>
              {tone}
            </Badge>
          ))}
        </div>
        <div className="flex flex-wrap gap-2">
          <Button variant="primary">承認する</Button>
          <Button variant="secondary">再取得</Button>
          <Button variant="ghost">詳細</Button>
          <Button variant="destructive">タスクを削除</Button>
          <Button disabled>送信中</Button>
          <Button size="sm">小さい操作</Button>
        </div>
      </Gallery>

      <Gallery title="Kbd">
        <p className="text-body">
          検索は <Kbd>/</Kbd>、閉じるは <Kbd>Esc</Kbd>
        </p>
      </Gallery>

      <Gallery title="Section と Panel">
        <Panel title="状態">daemon は稼働中です。</Panel>
        <Section
          level={3}
          title="入れ子の節"
          description="Section は見出しと説明と操作を持つ"
          actions={<Button>操作</Button>}
        >
          <p className="text-body">本文</p>
        </Section>
      </Gallery>

      <Gallery title="DataList">
        <DataList
          items={[
            { label: "担当", value: "ui-ux" },
            { label: "状態", value: <StatusBadge status="running" /> },
            { label: "ブランチ", value: "celeris-wu/01M3YZBE1Z94YJJWTFQGFM8RGV/gallery" },
          ]}
        />
      </Gallery>

      <Gallery title="Table">
        <Table aria-label="タスクの一覧">
          <TableHeader>
            <TableRow>
              <TableHead>タスク</TableHead>
              <TableHead>状態</TableHead>
              <TableHead>担当</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow>
              <TableCell>shared foundation</TableCell>
              <TableCell>
                <StatusBadge status="running" />
              </TableCell>
              <TableCell>ui-ux</TableCell>
            </TableRow>
            <TableRow>
              <TableCell>UX audit</TableCell>
              <TableCell>
                <StatusBadge status="done" />
              </TableCell>
              <TableCell>ui-ux</TableCell>
            </TableRow>
            <TableRow>
              <TableCell>統合</TableCell>
              <TableCell>
                <StatusBadge status="failed" />
              </TableCell>
              <TableCell>software-engineering</TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </Gallery>

      <Gallery title="CodeBlock と LogSurface">
        <CodeBlock label="検査コマンド">corepack pnpm@12.6.0 -C web test</CodeBlock>
        <LogSurface label="実行ログ" size="sm">
          {"[03:00:01] run started\n[03:00:04] cargo test --workspace\n[03:01:12] 1204 passed\n[03:01:13] run done"}
        </LogSurface>
      </Gallery>

      <Gallery title="Icon">
        <div className="flex flex-wrap items-center gap-3 text-body">
          <span className="inline-flex items-center gap-1">
            <Icon name="check" /> 完了
          </span>
          <span className="inline-flex items-center gap-1">
            <Icon name="alert" /> 注意
          </span>
          <span className="inline-flex items-center gap-1">
            <Icon name="info" size="sm" /> 情報
          </span>
          <IconOnlyButton name="refresh" label="再読み込み" />
          <IconOnlyButton name="copy" label="コピー" />
        </div>
      </Gallery>

      <Gallery title="状態表示">
        <h3 className="text-body font-semibold">loading</h3>
        <LoadingState phase="slow" onRetry={noop} />
        <h3 className="text-body font-semibold">empty</h3>
        <EmptyState message="表示するタスクはありません。" action={<Button>タスクを作る</Button>} />
        <h3 className="text-body font-semibold">error</h3>
        <ErrorNotice subject="タスク" onRetry={noop} />
        <h3 className="text-body font-semibold">stale</h3>
        <StaleState lastFetchedAt={fixedTime} onRetry={noop}>
          <p className="mt-2 text-body">前回取得した内容を残して表示する。</p>
        </StaleState>
        <h3 className="text-body font-semibold">disconnected</h3>
        <DisconnectedState onRetry={noop} />
        <h3 className="text-body font-semibold">permission-denied</h3>
        <PermissionDeniedState subject="監査ログ" />
      </Gallery>

      <Gallery title="素の border">
        <div data-testid="plain-border" className="rounded border p-3 text-body">
          色を指定しない border は --color-border（#D3DCE2）で描く。
        </div>
        <label className="mt-3 flex max-w-sm flex-col gap-1 text-body">
          入力欄（色を指定しない）
          <input data-testid="plain-input" className="rounded border bg-white p-2" defaultValue="枠の色" />
        </label>
      </Gallery>

      <Gallery title="Notice と入力">
        <Notice tone="danger" title="保存できませんでした" action={<Button size="sm">再試行</Button>}>
          接続を確かめてから、もう一度保存してください。
        </Notice>
        <Notice title="下書きのままです">公開するまで他の人には見えません。</Notice>
        <label htmlFor="g-name" className="flex max-w-sm flex-col gap-1 text-body">
          名前
          <Input id="g-name" placeholder="例: Pluvio 調査" />
        </label>
        <label htmlFor="g-state" className="flex max-w-sm flex-col gap-1 text-body">
          状態
          <Select id="g-state" defaultValue="running">
            <option value="running">実行中</option>
            <option value="done">完了</option>
          </Select>
        </label>
        <label htmlFor="g-disabled" className="flex max-w-sm flex-col gap-1 text-body">
          無効
          <Input id="g-disabled" disabled defaultValue="変更できません" />
        </label>
        <ShortId value="01M44C5GXRV021GW7QDHTNC054" label="タスク ID" />
      </Gallery>

      <Gallery title="ScrollTabs">
        <ScrollTabs aria-label="タブの例" className="flex gap-2">
          {["概要", "変更", "ファイル", "成果物", "実行", "ログ", "決定", "履歴", "設定"].map((name) => (
            <Button key={name} variant="ghost" className="shrink-0">
              {name}
            </Button>
          ))}
        </ScrollTabs>
      </Gallery>

      <Gallery title="ConfirmDialog と Drawer">
        <div className="flex flex-wrap gap-2">
          <ConfirmDialog
            trigger={<Button id="confirm-trigger">削除を確認</Button>}
            title="タスクを削除"
            target="タスク A"
            consequence="タスク A とその実行履歴を削除します。"
            reversibility="元に戻せません。"
            followUp="タスク一覧で確認できます。"
            confirmLabel="タスク A を削除"
            onConfirm={noop}
          />
          <Drawer
            trigger={<Button>詳細を開く</Button>}
            title="タスクの詳細"
            description="対象の状態を確認します"
            defaultOpen={open === "drawer"}
          >
            <DataList
              items={[
                { label: "状態", value: <StatusBadge status="running" /> },
                { label: "担当", value: "ui-ux" },
              ]}
            />
          </Drawer>
        </div>
      </Gallery>
    </main>
  );
}

const root = document.getElementById("root");
if (!root) throw new Error("fixture root がありません");
createRoot(root).render(<App />);
