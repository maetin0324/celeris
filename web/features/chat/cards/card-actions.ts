// カードからの送信。回答は受信箱と同じ `POST /inbox/items/{id}/answer`（選択肢も受信箱の項目のもの）、
// 代答の修正は `POST /cos/operations/{o}/override`。API は差し替えられるようにし、送信の規則を DOM 無しで試す。

import type {
  InboxAnswerBody,
  InboxAnswerResult,
  InboxItem,
  InboxOption,
  OverrideBody,
  OverrideMode,
  OverrideResponse,
} from "../../../api/generated/types";
import { answerInboxItem } from "../../../api/queries/inbox-notifications";
import { type AnswerOutcome, answerFailure } from "../../inbox/inbox-model";
import { overrideOperation } from "../data/client";
import { type OverrideOutcome, overrideFailure, overrideSuccess } from "./card-model";

export type CardApi = {
  answer: (itemId: string, body: InboxAnswerBody) => Promise<InboxAnswerResult>;
  override: (operationId: string, body: OverrideBody) => Promise<OverrideResponse>;
};

export const defaultCardApi: CardApi = {
  answer: (itemId, body) => answerInboxItem(itemId, body),
  override: (operationId, body) => overrideOperation(operationId, body),
};

/** 選択肢で答える。自由文が要る選択肢は詳細画面で答えるので、ここでは送らない。 */
export async function sendCardAnswer(api: CardApi, item: InboxItem, option: InboxOption): Promise<AnswerOutcome> {
  if (option.needs_note)
    return { ok: false, message: `「${option.label}」には理由が要ります。詳細画面で答えてください。`, field: "note" };
  try {
    const result = await api.answer(item.id, { option: option.key });
    return { ok: true, removed: result.removed };
  } catch (error) {
    return answerFailure(error);
  }
}

/** CoS 代答を取消（revoke）・差し戻し（return）する。理由は必須（API も空を 422 にする）。 */
export async function sendOverride(
  api: CardApi,
  operationId: string,
  mode: OverrideMode,
  reason: string,
): Promise<OverrideOutcome> {
  const trimmed = reason.trim();
  if (trimmed === "") return { ok: false, message: "理由を書いてください。", conflict: false, stale: false };
  try {
    return overrideSuccess(mode, await api.override(operationId, { action: mode, reason: trimmed }));
  } catch (error) {
    return overrideFailure(error);
  }
}
