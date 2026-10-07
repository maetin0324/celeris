import type { ChatAttachment } from "../../../api/generated/types";
import { deleteAttachment, type UploadProgress, uploadAttachment } from "../data/client";
import { newClientId } from "../data/send";

export type UploadItem = {
  id: string;
  file: File;
  state: "uploading" | "ready" | "failed";
  progress: number | null;
  attachment?: ChatAttachment;
  error?: string;
  preview?: string;
};

export type UploadApi = {
  upload: (
    threadId: string,
    file: File,
    options: { clientUploadId: string; signal: AbortSignal; onProgress: (p: UploadProgress) => void },
  ) => Promise<ChatAttachment>;
  remove: (attachmentId: string) => Promise<unknown>;
};

export const defaultUploadApi: UploadApi = { upload: uploadAttachment, remove: deleteAttachment };

export function pastedFiles(data: Pick<DataTransfer, "items" | "files"> | null): File[] {
  if (!data) return [];
  const items = Array.from(data.items ?? [])
    .filter((item) => item.kind === "file")
    .map((item) => item.getAsFile())
    .filter((file): file is File => file !== null);
  return items.length ? items : Array.from(data.files ?? []);
}

export function createUploadQueue(threadId: string, api: UploadApi = defaultUploadApi) {
  let items: UploadItem[] = [];
  const controllers = new Map<string, AbortController>();
  const listeners = new Set<() => void>();
  const publish = (next: UploadItem[]) => {
    items = next;
    for (const listener of listeners) listener();
  };
  const update = (id: string, patch: Partial<UploadItem>) => {
    if (items.some((item) => item.id === id))
      publish(items.map((item) => (item.id === id ? { ...item, ...patch } : item)));
  };
  const start = (id: string) => {
    const item = items.find((entry) => entry.id === id);
    if (!item) return;
    const controller = new AbortController();
    controllers.set(id, controller);
    update(id, { state: "uploading", progress: 0, error: undefined });
    void api
      .upload(threadId, item.file, {
        clientUploadId: id,
        signal: controller.signal,
        onProgress: ({ loaded, total }) => {
          if (controllers.get(id) === controller)
            update(id, { progress: total ? Math.min(100, Math.round((loaded / total) * 100)) : null });
        },
      })
      .then((attachment) => {
        if (!items.some((entry) => entry.id === id) || controllers.get(id) !== controller) {
          void api.remove(attachment.id);
          return;
        }
        update(id, { state: "ready", progress: 100, attachment });
      })
      .catch((error: unknown) => {
        if (items.some((entry) => entry.id === id) && controllers.get(id) === controller)
          update(id, { state: "failed", error: error instanceof Error ? error.message : String(error) });
      })
      .finally(() => {
        if (controllers.get(id) === controller) controllers.delete(id);
      });
  };
  return {
    getSnapshot: () => items,
    subscribe: (listener: () => void) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    add(files: Iterable<File>) {
      for (const file of files) {
        const id = newClientId("upload");
        const preview =
          ["image/jpeg", "image/png", "image/webp", "image/gif"].includes(file.type) &&
          typeof URL.createObjectURL === "function"
            ? URL.createObjectURL(file)
            : undefined;
        publish([...items, { id, file, state: "uploading", progress: 0, preview }]);
        start(id);
      }
    },
    retry(id: string) {
      if (items.find((item) => item.id === id)?.state === "failed") start(id);
    },
    remove(id: string) {
      const item = items.find((entry) => entry.id === id);
      if (!item) return;
      controllers.get(id)?.abort();
      if (item.attachment) void api.remove(item.attachment.id);
      if (item.preview) URL.revokeObjectURL(item.preview);
      publish(items.filter((entry) => entry.id !== id));
    },
    clearSent(ids: readonly string[]) {
      const sent = new Set(ids);
      for (const item of items) if (sent.has(item.id) && item.preview) URL.revokeObjectURL(item.preview);
      publish(items.filter((item) => !sent.has(item.id)));
    },
    dispose() {
      for (const controller of controllers.values()) controller.abort();
      for (const item of items) if (item.preview) URL.revokeObjectURL(item.preview);
      listeners.clear();
    },
  };
}

export function shouldSendOnEnter(event: {
  key: string;
  shiftKey: boolean;
  isComposing: boolean;
  keyCode: number;
}): boolean {
  return event.key === "Enter" && !event.shiftKey && !event.isComposing && event.keyCode !== 229;
}
