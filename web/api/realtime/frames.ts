// SSE フレームの envelope 検証（ADR-0081 D4）。不正・不明な payload は null にして捨て、画面へは出さない。

import type { DaemonSnapshot, EventRow, StreamHello, StreamReset } from "../generated/types";
import { isEventKind } from "./event-kinds";

export type Frame =
  | { type: "hello"; data: StreamHello }
  | { type: "heartbeat" }
  | { type: "task.event"; data: EventRow }
  | { type: "daemon"; data: DaemonSnapshot }
  | { type: "reset"; data: StreamReset }
  | { type: "inbox_changed" }
  | { type: "notifications_changed" };

/** 中身を持たない軽い合図（ADR-0133 D5）。受信箱・通知の key を取り直すきっかけだけ。 */
export type SignalFrame = Extract<Frame, { type: "inbox_changed" | "notifications_changed" }>;

export const FRAME_TYPES = [
  "hello",
  "heartbeat",
  "task.event",
  "daemon",
  "reset",
  "inbox_changed",
  "notifications_changed",
] as const;

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function isCursor(v: unknown): v is number {
  return typeof v === "number" && Number.isSafeInteger(v) && v >= 0;
}

/** SSE の event 名と data 文字列から Frame を作る。検証に通らなければ null。 */
export function parseFrame(name: string, raw: string): Frame | null {
  if (name === "heartbeat") return { type: "heartbeat" };
  // 合図の data は `{}`。中身は読まない（将来の欄が増えても捨てない）。
  if (name === "inbox_changed" || name === "notifications_changed") return { type: name };
  let json: unknown;
  try {
    json = JSON.parse(raw);
  } catch {
    return null;
  }
  if (!isRecord(json)) return null;
  switch (name) {
    case "hello": {
      if (!isCursor(json.cursor) || typeof json.now !== "string") return null;
      if (json.daemon != null && !isRecord(json.daemon)) return null;
      return { type: "hello", data: json as unknown as StreamHello };
    }
    case "reset": {
      if (!isCursor(json.cursor) || typeof json.reason !== "string") return null;
      return { type: "reset", data: json as unknown as StreamReset };
    }
    case "daemon":
      return { type: "daemon", data: json as unknown as DaemonSnapshot };
    case "task.event": {
      const event = json.event;
      if (
        !isCursor(json.id) ||
        !isCursor(json.seq) ||
        typeof json.task_id !== "string" ||
        json.task_id === "" ||
        typeof json.ts !== "string" ||
        !isRecord(event) ||
        !isEventKind(event.type)
      ) {
        return null;
      }
      return { type: "task.event", data: json as unknown as EventRow };
    }
    default:
      return null;
  }
}
