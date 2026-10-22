import { useEffect, useId, useState, useSyncExternalStore } from "react";
import type { ChatThread } from "../../../api/generated/types";
import { Badge } from "../../../components/ui/badge";
import { Button } from "../../../components/ui/button";
import { Drawer } from "../../../components/ui/drawer";
import { Input } from "../../../components/ui/input";
import { ThreadsModel, threadTotals } from "./model";

export type ChatThreadsProps = {
  selectedThreadId?: string | null;
  onSelectThread: (id: string) => void;
  model?: ThreadsModel;
};

/** 未読・要対応の数の badge。0 のときは出さない。 */
function CountBadges({ item }: { item: ChatThread }) {
  return (
    <>
      {item.attention_count > 0 && (
        <Badge tone="warning" className="shrink-0 whitespace-nowrap">
          要対応 {item.attention_count}
        </Badge>
      )}
      {item.unread_count > 0 && (
        <Badge tone="info" className="shrink-0 whitespace-nowrap">
          未読 {item.unread_count}
        </Badge>
      )}
    </>
  );
}

const KIND_LABEL: Record<ChatThread["kind"], string> = { general: "統括", project: "案件", legacy: "旧会話" };

function ThreadList({
  model,
  selectedThreadId,
  onSelectThread,
  onSelected,
}: ChatThreadsProps & { model: ThreadsModel; onSelected?: () => void }) {
  const state = useSyncExternalStore(model.subscribe, model.snapshot, model.snapshot);
  const [search, setSearch] = useState(state.query);
  const searchId = useId();
  useEffect(() => {
    setSearch(state.query);
  }, [state.query]);
  useEffect(() => {
    if (search === state.query) return;
    const handle = setTimeout(() => void model.load(search.trim()), 250);
    return () => clearTimeout(handle);
  }, [model, search, state.query]);
  const select = (id: string) => {
    onSelectThread(id);
    onSelected?.();
  };
  const totals = threadTotals(state.items);
  return (
    <div className="flex h-full min-h-0 flex-col gap-3">
      <p className="text-label text-muted-foreground" data-thread-totals>
        CoS {state.items.filter((item) => item.kind !== "legacy").length} 件・要対応 {totals.attention}・未読{" "}
        {totals.unread}
      </p>
      <div className="flex flex-col gap-1">
        <label htmlFor={searchId} className="text-label text-muted-foreground">
          会話を検索
        </label>
        <Input
          id={searchId}
          type="search"
          value={search}
          onChange={(event) => setSearch(event.target.value)}
          placeholder="題名・本文を検索"
        />
      </div>
      {state.error && (
        <p role="alert" className="text-body text-destructive">
          {state.error}
        </p>
      )}
      <nav aria-label="会話" className="min-h-0 overflow-y-auto">
        {state.items.length === 0 && !state.loading && (
          <p className="text-body text-muted-foreground">会話がありません</p>
        )}
        <ul className="space-y-1">
          {state.items.map((item) => (
            <li key={item.id} className="rounded-md border border-border bg-surface p-1" data-thread-kind={item.kind}>
              <div className="flex min-w-0 items-center gap-1">
                <Button
                  variant="ghost"
                  className="min-w-0 flex-1 justify-start text-left"
                  aria-label={item.title}
                  aria-current={selectedThreadId === item.id ? "page" : undefined}
                  onClick={() => select(item.id)}
                >
                  <Badge tone={item.kind === "legacy" ? "neutral" : "info"} className="shrink-0 whitespace-nowrap">
                    {KIND_LABEL[item.kind]}
                  </Badge>
                  <span className="min-w-0 truncate">{item.title}</span>
                  <CountBadges item={item} />
                </Button>
                {item.kind === "legacy" && (
                  <Button
                    variant="ghost"
                    size="icon"
                    aria-label={`${item.title}をアーカイブ`}
                    disabled={state.busy}
                    onClick={() => void model.archive(item)}
                  >
                    保管
                  </Button>
                )}
              </div>
            </li>
          ))}
        </ul>
        {state.nextCursor && (
          <Button className="w-full" disabled={state.loading} onClick={() => void model.more()}>
            さらに表示
          </Button>
        )}
        {state.loading && (
          <p role="status" className="text-body text-muted-foreground">
            読み込み中
          </p>
        )}
      </nav>
    </div>
  );
}

/** home が選択を ?thread= へ写す。幅切替は CSS で行い、mobile は Drawer が focus を trigger に戻す。 */
export function ChatThreads(props: ChatThreadsProps) {
  const [defaultModel] = useState(() => new ThreadsModel());
  const model = props.model ?? defaultModel;
  const [desktopOpen, setDesktopOpen] = useState(true);
  const [drawerOpen, setDrawerOpen] = useState(false);
  useEffect(() => {
    void model.load();
  }, [model]);
  return (
    <>
      <div className="hidden md:flex md:h-full md:min-h-0 md:flex-col">
        <Button
          aria-expanded={desktopOpen}
          aria-controls="chat-thread-sidebar"
          onClick={() => setDesktopOpen(!desktopOpen)}
        >
          {desktopOpen ? "会話一覧を閉じる" : "会話一覧を開く"}
        </Button>
        {desktopOpen && (
          <aside
            id="chat-thread-sidebar"
            className="min-h-0 w-72 max-w-full flex-1 overflow-y-auto border-r border-border bg-surface p-3"
          >
            <ThreadList {...props} model={model} />
          </aside>
        )}
      </div>
      <div className="md:hidden">
        <Drawer
          title="会話一覧"
          trigger={<Button aria-label="会話一覧を開く">会話一覧</Button>}
          open={drawerOpen}
          onOpenChange={setDrawerOpen}
        >
          <ThreadList {...props} model={model} onSelected={() => setDrawerOpen(false)} />
        </Drawer>
      </div>
    </>
  );
}
