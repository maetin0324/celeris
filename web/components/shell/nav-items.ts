// ナビの項目（P2-02）。path は現行 gui/ と同じ（S5）。
// group はナビの見出しの区分（DESIGN.md: 日々の仕事と管理に分ける）。並びは navItems の順のまま。
export const navItems = [
  { to: "/", label: "ホーム", group: "work" },
  { to: "/inbox", label: "受信箱", group: "work" },
  { to: "/notifications", label: "通知", group: "work" },
  { to: "/tasks", label: "タスク", group: "work" },
  { to: "/projects", label: "案件", group: "work" },
  { to: "/board", label: "ボード", group: "work" },
  { to: "/approvals", label: "承認", group: "work" },
  { to: "/artifacts", label: "成果物", group: "work" },
  { to: "/graph", label: "依存グラフ", group: "work" },
  { to: "/org", label: "組織", group: "admin" },
  { to: "/knowledge", label: "知識", group: "admin" },
  { to: "/reports", label: "報告", group: "admin" },
  { to: "/daemon", label: "daemon", group: "admin" },
  { to: "/providers", label: "プロバイダ", group: "admin" },
  { to: "/accounts", label: "アカウント", group: "admin" },
  { to: "/clusters", label: "クラスタ", group: "admin" },
  { to: "/releases", label: "リリース", group: "admin" },
  { to: "/help", label: "ヘルプ", group: "admin" },
] as const;

export type NavGroup = (typeof navItems)[number]["group"];

export const navGroups: readonly { key: NavGroup; label: string }[] = [
  { key: "work", label: "日々の仕事" },
  { key: "admin", label: "組織と管理" },
];
