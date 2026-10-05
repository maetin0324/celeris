import { useConnectionState } from "../../api/realtime/connection-state";
import { Notice } from "../ui/notice";

/** SSE が切れた間、取得済みの本文が更新されないことをその場で伝える。 */
export function ConnectionStaleNotice() {
  const connection = useConnectionState();
  if (connection !== "reconnecting") return null;
  return (
    <Notice title="表示中の情報は更新されていません" data-testid="connection-stale-notice">
      接続を再試行しています。判断や操作の前に、再接続後の内容を確認してください。
    </Notice>
  );
}
