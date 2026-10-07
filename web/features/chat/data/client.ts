// CoS チャットの API client（ADR 2026-10-05-cos-chat-home D2、gui-api.md §3.127・§3.129）。
// JSON の要求は api/client.ts の apiFetch を通す（same-origin・timeout・session の打ち切り）。
// 添付の upload だけは進捗と取消のため XHR を使う。変更系はここで再送しない（再送は send.ts が同じ冪等 key で行う）。

import { ApiError, type ApiErrorKind, apiGet, apiMutate, assertApiPath, kindForStatus } from "../../../api/client";
import type {
  AnswerBody,
  ApprovalDecideBody,
  ApprovalDecideResult,
  ChatAttachment,
  ChatAttachmentResponse,
  ChatCreateThreadRequest,
  ChatEventListResponse,
  ChatMessageListResponse,
  ChatMessageResponse,
  ChatPatchThreadRequest,
  ChatPostMessageRequest,
  ChatPostMessageResponse,
  ChatReferenceRequest,
  ChatReferenceResponse,
  ChatRunResponse,
  ChatStopResponse,
  ChatThreadDetailResponse,
  ChatThreadListResponse,
  ChatThreadResponse,
  ChatThreadStatus,
  DecisionAnswerBody,
  DecisionOutcome,
  InboxAnswerBody,
  InboxAnswerResult,
  OverrideBody,
  OverrideResponse,
  PlanGateRequest,
  TransitionResult,
} from "../../../api/generated/types";

// Browser-facing gateway は /api を daemon の /api/v1 に中継する。
const API = "/api";
const seg = encodeURIComponent;

function query(params: Record<string, string | number | null | undefined>): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value === undefined || value === null || value === "") continue;
    search.set(key, String(value));
  }
  const text = search.toString();
  return text === "" ? "" : `?${text}`;
}

export const chatPaths = {
  threads: () => `${API}/chat/threads`,
  thread: (t: string) => `${API}/chat/threads/${seg(t)}`,
  messages: (t: string) => `${API}/chat/threads/${seg(t)}/messages`,
  message: (t: string, m: string) => `${API}/chat/threads/${seg(t)}/messages/${seg(m)}`,
  stop: (t: string) => `${API}/chat/threads/${seg(t)}/stop`,
  resumeQueue: (t: string) => `${API}/chat/threads/${seg(t)}/resume-queue`,
  run: (t: string, r: string) => `${API}/chat/threads/${seg(t)}/runs/${seg(r)}`,
  runEvents: (t: string, r: string) => `${API}/chat/threads/${seg(t)}/runs/${seg(r)}/events`,
  stream: (t: string) => `${API}/chat/threads/${seg(t)}/stream`,
  attachments: (t: string) => `${API}/chat/threads/${seg(t)}/attachments`,
  attachment: (a: string) => `${API}/chat/attachments/${seg(a)}`,
  attachmentContent: (a: string) => `${API}/chat/attachments/${seg(a)}/content`,
  attachmentPreview: (a: string) => `${API}/chat/attachments/${seg(a)}/preview`,
  attachmentReferences: (a: string) => `${API}/chat/attachments/${seg(a)}/references`,
};

// ---- thread ----

export type ThreadListParams = { q?: string; status?: ChatThreadStatus; before?: string; limit?: number };

export function listThreads(params: ThreadListParams = {}, signal?: AbortSignal) {
  return apiGet<ChatThreadListResponse>(`${chatPaths.threads()}${query(params)}`, signal);
}

export function createThread(body: ChatCreateThreadRequest, signal?: AbortSignal) {
  return apiMutate<ChatThreadResponse>("POST", chatPaths.threads(), body, signal);
}

export function getThread(threadId: string, signal?: AbortSignal) {
  return apiGet<ChatThreadDetailResponse>(chatPaths.thread(threadId), signal);
}

export function patchThread(threadId: string, body: ChatPatchThreadRequest, signal?: AbortSignal) {
  return apiMutate<ChatThreadResponse>("PATCH", chatPaths.thread(threadId), body, signal);
}

export function resumeQueue(threadId: string, expectedRevision: number, signal?: AbortSignal) {
  return apiMutate<ChatThreadResponse>(
    "POST",
    chatPaths.resumeQueue(threadId),
    { expected_revision: expectedRevision },
    signal,
  );
}

// ---- message ----

/** before_seq と after_seq は排他（API は両方を 400 にする）。どちらも無ければ最新の頁。 */
export type MessageListParams = { before_seq?: number; after_seq?: number; limit?: number };

export function listMessages(threadId: string, params: MessageListParams = {}, signal?: AbortSignal) {
  if (params.before_seq !== undefined && params.after_seq !== undefined) {
    throw new TypeError("before_seq and after_seq are exclusive");
  }
  return apiGet<ChatMessageListResponse>(`${chatPaths.messages(threadId)}${query(params)}`, signal);
}

export function postMessage(threadId: string, body: ChatPostMessageRequest, signal?: AbortSignal) {
  return apiMutate<ChatPostMessageResponse>("POST", chatPaths.messages(threadId), body, signal);
}

/** queued の人の発言だけ取り消せる（開始済みは 409）。 */
export function cancelMessage(threadId: string, messageId: string, signal?: AbortSignal) {
  return apiMutate<ChatMessageResponse>("DELETE", chatPaths.message(threadId, messageId), undefined, signal);
}

// ---- run ----

export function stopRun(threadId: string, runId: string, signal?: AbortSignal) {
  return apiMutate<ChatStopResponse>("POST", chatPaths.stop(threadId), { run_id: runId }, signal);
}

export function getRun(threadId: string, runId: string, signal?: AbortSignal) {
  return apiGet<ChatRunResponse>(chatPaths.run(threadId, runId), signal);
}

export function listRunEvents(
  threadId: string,
  runId: string,
  params: { after?: string; limit?: number } = {},
  signal?: AbortSignal,
) {
  return apiGet<ChatEventListResponse>(`${chatPaths.runEvents(threadId, runId)}${query(params)}`, signal);
}

// ---- attachment ----

export function getAttachment(attachmentId: string, signal?: AbortSignal) {
  return apiGet<ChatAttachmentResponse>(chatPaths.attachment(attachmentId), signal);
}

/** 未参照の upload だけ消せる（参照ありは 409、削除済みは 204）。 */
export function deleteAttachment(attachmentId: string, signal?: AbortSignal) {
  return apiMutate<void>("DELETE", chatPaths.attachment(attachmentId), undefined, signal);
}

export function addAttachmentReference(attachmentId: string, body: ChatReferenceRequest, signal?: AbortSignal) {
  return apiMutate<ChatReferenceResponse>("POST", chatPaths.attachmentReferences(attachmentId), body, signal);
}

export type UploadProgress = { loaded: number; total: number | undefined };

/** XHR の必要な部分だけ（テストで差し替える）。 */
export type XhrLike = {
  open(method: string, url: string): void;
  setRequestHeader(name: string, value: string): void;
  send(body: FormData): void;
  abort(): void;
  readonly status: number;
  readonly responseText: string;
  withCredentials: boolean;
  upload: { onprogress: ((event: { loaded: number; total: number; lengthComputable: boolean }) => void) | null };
  onload: (() => void) | null;
  onerror: (() => void) | null;
  onabort: (() => void) | null;
  ontimeout: (() => void) | null;
};

export type UploadOptions = {
  /** 冪等 key。同じ key・同じ bytes の再送は 200 で同じ添付が返る。 */
  clientUploadId: string;
  onProgress?: (progress: UploadProgress) => void;
  signal?: AbortSignal;
  createXhr?: () => XhrLike;
};

const defaultXhr = (): XhrLike => new XMLHttpRequest() as unknown as XhrLike;

function parseJson(text: string): unknown {
  if (text === "") return undefined;
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return text;
  }
}

/** multipart（`file` と `client_upload_id`）で 1 個 upload する。進捗を通知し、signal で取り消せる。 */
export function uploadAttachment(threadId: string, file: Blob & { name?: string }, options: UploadOptions) {
  const path = chatPaths.attachments(threadId);
  assertApiPath(path);
  const method = "POST";
  return new Promise<ChatAttachment>((resolve, reject) => {
    const fail = (kind: ApiErrorKind, status?: number, body?: unknown) =>
      reject(new ApiError(kind, { method, path, status, body }));
    if (options.signal?.aborted) {
      fail("aborted");
      return;
    }
    const xhr = (options.createXhr ?? defaultXhr)();
    const onAbortSignal = () => xhr.abort();
    const cleanup = () => options.signal?.removeEventListener("abort", onAbortSignal);
    xhr.open(method, path);
    xhr.withCredentials = true;
    xhr.setRequestHeader("Accept", "application/json");
    xhr.upload.onprogress = (event) =>
      options.onProgress?.({ loaded: event.loaded, total: event.lengthComputable ? event.total : undefined });
    xhr.onload = () => {
      cleanup();
      const body = parseJson(xhr.responseText);
      if (xhr.status >= 200 && xhr.status < 300) {
        const attachment = (body as Partial<ChatAttachmentResponse> | undefined)?.attachment;
        if (attachment) resolve(attachment);
        else fail("parse", xhr.status, body);
        return;
      }
      fail(kindForStatus(xhr.status), xhr.status, body);
    };
    xhr.onerror = () => {
      cleanup();
      fail("network");
    };
    xhr.ontimeout = () => {
      cleanup();
      fail("timeout");
    };
    xhr.onabort = () => {
      cleanup();
      fail("aborted");
    };
    options.signal?.addEventListener("abort", onAbortSignal, { once: true });
    const form = new FormData();
    form.append("client_upload_id", options.clientUploadId);
    form.append("file", file, file.name ?? "file");
    xhr.send(form);
  });
}

// ---- カードからの回答（既存の API） ----

export function answerDecision(decisionId: string, body: DecisionAnswerBody, signal?: AbortSignal) {
  return apiMutate<DecisionOutcome>("POST", `${API}/decisions/${seg(decisionId)}/answer`, body, signal);
}

export function decideApproval(approvalId: string, body: ApprovalDecideBody, signal?: AbortSignal) {
  return apiMutate<ApprovalDecideResult>("POST", `${API}/approvals/${seg(approvalId)}/decide`, body, signal);
}

export function answerQuestion(taskId: string, body: AnswerBody, signal?: AbortSignal) {
  return apiMutate<TransitionResult>("POST", `${API}/tasks/${seg(taskId)}/answer`, body, signal);
}

export function answerPlanGate(taskId: string, body: PlanGateRequest, signal?: AbortSignal) {
  return apiMutate<TransitionResult>("POST", `${API}/tasks/${seg(taskId)}/execution/plan-gate`, body, signal);
}

export function answerInboxItem(itemId: string, body: InboxAnswerBody, signal?: AbortSignal) {
  return apiMutate<InboxAnswerResult>("POST", `${API}/inbox/items/${seg(itemId)}/answer`, body, signal);
}

/** CoS 代答の取消（revoke）・差し戻し（return）。人の認証だけが使える。 */
export function overrideOperation(operationId: string, body: OverrideBody, signal?: AbortSignal) {
  return apiMutate<OverrideResponse>("POST", `${API}/cos/operations/${seg(operationId)}/override`, body, signal);
}
