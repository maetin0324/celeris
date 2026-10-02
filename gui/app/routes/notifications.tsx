import { useFetcher } from "react-router";
import { type CelerisClient, getCelerisClient } from "~/celeris/client.server";
import { CelerisError, celerisErrorResponse, isCelerisUnavailable } from "~/celeris/errors";
import { Button } from "~/components/ui/button";
import { Card, CardBody } from "~/components/ui/card";
import { Alert, EmptyState, PageHeader } from "~/components/ui/misc";
import type { Route } from "./+types/notifications";

export interface NotificationsData {
  feed: { items: Notice[]; unread: number; next_before?: string | null };
  unread: number;
}

interface Notice {
  id: string;
  kind: string;
  title: string;
  summary: string;
  count: number;
  last_at: string;
  read_at?: string | null;
}

export async function loadNotifications(client: CelerisClient, request: Request): Promise<NotificationsData> {
  try {
    const [feed, unread] = await Promise.all([
      client.get<NotificationsData["feed"]>("/notifications", { signal: request.signal }),
      client.get<{ unread: number }>("/notifications/unread-count", { signal: request.signal }),
    ]);
    return { feed, unread: unread.unread };
  } catch (error) {
    if (isCelerisUnavailable(error)) throw celerisErrorResponse(error);
    if (error instanceof CelerisError) throw celerisErrorResponse(error);
    throw error;
  }
}

export async function loader({ request }: Route.LoaderArgs): Promise<NotificationsData> {
  return loadNotifications(getCelerisClient(), request);
}

export async function markNoticeRead(client: CelerisClient, id: string, signal?: AbortSignal) {
  await client.post(`/notifications/${encodeURIComponent(id)}/read`, {}, { signal });
}

export async function action({ request }: Route.ActionArgs) {
  const form = await request.formData();
  const id = form.get("id");
  if (typeof id !== "string" || !id) return { ok: false };
  try {
    await markNoticeRead(getCelerisClient(), id, request.signal);
    return { ok: true, id };
  } catch (error) {
    if (isCelerisUnavailable(error) || error instanceof CelerisError) throw celerisErrorResponse(error);
    throw error;
  }
}

export default function Notifications({ loaderData }: Route.ComponentProps) {
  const fetcher = useFetcher();
  return (
    <main className="space-y-5" data-testid="notifications-page">
      <PageHeader title="通知" description={`未読 ${loaderData.unread} 件`} />
      {loaderData.feed.items.length === 0 ? (
        <EmptyState title="通知はありません" description="判断が要らない知らせがここに届きます。" />
      ) : (
        <div className="space-y-3">
          {loaderData.feed.items.map((notice: Notice) => (
            <Card key={notice.id}>
              <CardBody className="flex items-start justify-between gap-4">
                <div>
                  <h2 className="font-semibold">{notice.title}</h2>
                  <p className="mt-1 text-sm text-fg-muted">{notice.summary}</p>
                  <p className="mt-2 text-xs text-fg-subtle">{notice.count} 件 · {notice.last_at}</p>
                </div>
                {notice.read_at == null && (
                  <fetcher.Form method="post">
                    <input type="hidden" name="id" value={notice.id} />
                    <Button type="submit" size="sm" disabled={fetcher.state !== "idle"}>既読にする</Button>
                  </fetcher.Form>
                )}
              </CardBody>
            </Card>
          ))}
        </div>
      )}
      {fetcher.data && !(fetcher.data as { ok?: boolean }).ok && <Alert tone="danger">既読にできませんでした。</Alert>}
    </main>
  );
}
