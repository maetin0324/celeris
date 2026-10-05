import { useState } from "react";
import type { ConnectionState } from "../../api/realtime/connection-state";

/**
 * ホームの「未確認」（黄帯と badge）を出すか。再接続に失敗し続けている間、transport は試すたびに
 * 「接続を確認中」と「再接続中」を行き来する。そのたびに黄帯が消えて出るとページの高さが跳ね、ページを末尾へ
 * 送って会話を読んでいても scroll が先頭へ戻った（fix-r6 narrow）。一度「再接続中」「未接続」になったら、
 * 繋がる（open）か認証が要る（unauthorized）まで、途中の「接続を確認中」でも未確認のままにする。
 */
export function stickyUnconfirmed(previous: boolean, connection: ConnectionState): boolean {
  if (connection === "reconnecting" || connection === "closed") return true;
  if (connection === "connecting") return previous;
  return false;
}

export function useUnconfirmedConnection(connection: ConnectionState): boolean {
  const [state, setState] = useState({ connection, unconfirmed: stickyUnconfirmed(false, connection) });
  if (state.connection !== connection) {
    const next = { connection, unconfirmed: stickyUnconfirmed(state.unconfirmed, connection) };
    setState(next);
    return next.unconfirmed;
  }
  return state.unconfirmed;
}
