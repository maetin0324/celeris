// V3 の画面台帳。path は gateway の宣言と同じ pattern、fixture は実際に開く URL。
// 動的 id は偽 daemon の T1 / R1 / cos を使う。
export const screens = [
  { path: "/", fixture: "/", heading: "ホーム", v3: true },
  { path: "/inbox", fixture: "/inbox", heading: "受信箱", v3: true },
  { path: "/notifications", fixture: "/notifications", heading: "通知", v3: true },
  { path: "/login", fixture: "/login", heading: "Celeris にログイン" },
  { path: "/org", fixture: "/org", heading: "組織", v3: true },
  { path: "/org/secretary", fixture: "/org/cos", heading: "組織の人 cos", v3: true },
  { path: "/org/$id", fixture: "/org/cos", heading: "組織の人 cos", v3: true },
  { path: "/projects", fixture: "/projects", heading: "案件", v3: true },
  { path: "/projects/$id", fixture: "/projects/P1", heading: "案件の詳細 P1", v3: true },
  { path: "/projects/$id/docs", fixture: "/projects/P1/docs", heading: "案件の文書 P1", v3: true },
  {
    path: "/projects/$id/docs/maintenance",
    fixture: "/projects/P1/docs/maintenance",
    heading: "文書の保守 P1",
    v3: true,
  },
  {
    path: "/projects/$id/browser-identities",
    fixture: "/projects/P1/browser-identities",
    heading: "ブラウザの本人情報",
  },
  { path: "/board", fixture: "/board", heading: "ボード", v3: true },
  { path: "/knowledge", fixture: "/knowledge", heading: "知識", v3: true },
  { path: "/knowledge/inbox", fixture: "/knowledge/inbox", heading: "知識の候補", v3: true },
  { path: "/knowledge/skills", fixture: "/knowledge/skills", heading: "skills", v3: true },
  { path: "/reports", fixture: "/reports", heading: "報告", v3: true },
  { path: "/approvals", fixture: "/approvals", heading: "承認", v3: true },
  { path: "/artifacts", fixture: "/artifacts", heading: "成果物", v3: true },
  { path: "/tasks", fixture: "/tasks", heading: "タスク", v3: true },
  { path: "/tasks/new", fixture: "/tasks/new", heading: "タスクの作成", v3: true },
  { path: "/tasks/$id", fixture: "/tasks/T1", heading: "タスクの詳細 T1", v3: true },
  { path: "/tasks/$id/files", fixture: "/tasks/T1/files", heading: "作業ツリーと成果物 T1", v3: true },
  { path: "/tasks/$id/changes", fixture: "/tasks/T1/changes", heading: "変更 T1", v3: true },
  { path: "/tasks/$id/runs/$runId", fixture: "/tasks/T1/runs/R1", heading: "run ログ T1 / R1", v3: true },
  { path: "/browser", fixture: "/browser", heading: "ブラウザ" },
  { path: "/browser/runs/$taskId/$runId", fixture: "/browser/runs/T1/R1", heading: "ブラウザ実行" },
  { path: "/plans/new", fixture: "/plans/new", heading: "計画の作成", v3: true },
  { path: "/daemon", fixture: "/daemon", heading: "daemon", v3: true },
  { path: "/providers", fixture: "/providers", heading: "プロバイダ", v3: true },
  { path: "/accounts", fixture: "/accounts", heading: "アカウント", v3: true },
  { path: "/clusters", fixture: "/clusters", heading: "クラスタ", v3: true },
  { path: "/releases", fixture: "/releases", heading: "リリース", v3: true },
  { path: "/graph", fixture: "/graph", heading: "依存グラフ", v3: true },
  { path: "/help", fixture: "/help", heading: "ヘルプ", v3: true },
] as const satisfies readonly Screen[];

export type Screen = { path: string; fixture: string; heading: string; v3?: boolean };

export function v3Screens(items: readonly Screen[] = screens): Screen[] {
  return items.filter((screen) => screen.v3 === true);
}
