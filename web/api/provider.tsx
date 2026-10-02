// 認証 session 内で 1 つの QueryClient を配る。session が終わると新しい client に切り替わる。

import { QueryClientProvider } from "@tanstack/react-query";
import { type ReactNode, useEffect, useState } from "react";
import { registerProtectedCache } from "../lib/session";
import { getSessionQueryClient } from "./query-client";

export function SessionQueryProvider({ children }: { children: ReactNode }) {
  const [client, setClient] = useState(getSessionQueryClient);
  // endQuerySession（bindQueryClientToSession で登録済み）の後に新しい client へ切り替える。
  useEffect(() => registerProtectedCache(() => setClient(getSessionQueryClient())), []);
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}
