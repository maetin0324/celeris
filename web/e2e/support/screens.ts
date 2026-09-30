// V3 の画面台帳。path は gateway の宣言と同じ pattern、fixture は実際に開く URL。
// 動的 id は偽 daemon の T1 / R1 / cos を使う。
export const screens = [
  { path: "/", fixture: "/", heading: "ホーム", v3: true },
  { path: "/inbox", fixture: "/inbox", heading: "受信箱", v3: true },
  { path: "/login", fixture: "/login", heading: "Celeris にログイン" },
  { path: "/org", fixture: "/org", heading: "組織" },
  { path: "/org/secretary", fixture: "/org/cos", heading: "組織の人 cos" },
  { path: "/org/$id", fixture: "/org/cos", heading: "組織の人 cos", v3: true },
  { path: "/projects", fixture: "/projects", heading: "案件" },
  { path: "/projects/$id", fixture: "/projects/P1", heading: "案件の詳細 P1" },
  { path: "/projects/$id/docs", fixture: "/projects/P1/docs", heading: "案件の文書 P1" },
  { path: "/projects/$id/docs/maintenance", fixture: "/projects/P1/docs/maintenance", heading: "文書の保守 P1" },
  { path: "/board", fixture: "/board", heading: "ボード" },
  { path: "/knowledge", fixture: "/knowledge", heading: "知識" },
  { path: "/knowledge/inbox", fixture: "/knowledge/inbox", heading: "知識の候補" },
  { path: "/knowledge/skills", fixture: "/knowledge/skills", heading: "skills" },
  { path: "/reports", fixture: "/reports", heading: "報告" },
  { path: "/approvals", fixture: "/approvals", heading: "承認" },
  { path: "/artifacts", fixture: "/artifacts", heading: "成果物" },
  { path: "/tasks", fixture: "/tasks", heading: "タスク", v3: true },
  { path: "/tasks/new", fixture: "/tasks/new", heading: "タスクの作成" },
  { path: "/tasks/$id", fixture: "/tasks/T1", heading: "タスクの詳細 T1" },
  { path: "/tasks/$id/files", fixture: "/tasks/T1/files", heading: "作業ツリーと成果物 T1" },
  { path: "/tasks/$id/changes", fixture: "/tasks/T1/changes", heading: "変更 T1" },
  { path: "/tasks/$id/runs/$runId", fixture: "/tasks/T1/runs/R1", heading: "run ログ T1 / R1" },
  { path: "/plans/new", fixture: "/plans/new", heading: "計画の作成" },
  { path: "/daemon", fixture: "/daemon", heading: "daemon" },
  { path: "/providers", fixture: "/providers", heading: "プロバイダ" },
  { path: "/accounts", fixture: "/accounts", heading: "アカウント" },
  { path: "/clusters", fixture: "/clusters", heading: "クラスタ" },
  { path: "/releases", fixture: "/releases", heading: "リリース" },
  { path: "/graph", fixture: "/graph", heading: "依存グラフ" },
  { path: "/help", fixture: "/help", heading: "ヘルプ" },
] as const satisfies readonly Screen[];

export type Screen = { path: string; fixture: string; heading: string; v3?: boolean };

export function v3Screens(items: readonly Screen[] = screens): Screen[] {
  return items.filter((screen) => screen.v3 === true);
}
