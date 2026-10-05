// 状態の変種の一覧。screens.ts の台帳（画面ごとに 1 行・check:parity が数える）とは別に置き、
// 偽 daemon の設定と、その状態を当てる各画面群の代表画面（fixture の URL）を組にする。
// states.spec.ts が各状態の描画を確かめ、screenshots.mjs --states が状態 × 幅で撮る。
import type { Page } from "@playwright/test";
import type { InboxItem, Notice } from "../../api/generated/types";
import type { FakeDaemonOptions } from "./fake-daemon.mjs";
import { inboxItemsFixture, noticesFixture, richFixtures } from "./fake-daemon.mjs";

export type StateKey =
  | "long-text"
  | "long-id"
  | "empty"
  | "many"
  | "loading"
  | "error"
  | "stale"
  | "forbidden"
  | "reviewing";

export type FixtureState = {
  key: StateKey;
  /** 記録・screenshot に出す短い説明。 */
  label: string;
  /** createFakeDaemon に渡す option（profile は既定 rich）。 */
  daemon: FakeDaemonOptions;
  /**
   * ブラウザから見た gateway の応答の差し替え（page.route）。gateway（web/server/relay.js）は daemon の
   * 401/403 を 502 daemon_auth に変えるので、画面の 403 表示は偽 daemon の設定では出せない。
   */
  route?: { status: number; paths: readonly string[] };
  /** 当てる代表画面の URL（主画面・task 系・管理などの各群から）。 */
  screens: readonly string[];
  /**
   * screenshots.mjs --states が撮る前に待つもの（既定 settled）。held: 読み込み中の表示が出たら保留のまま撮る。
   * alert: 取得失敗の表示（role=alert と「再試行」）が出るまで待つ。settled: 見出しが出て読み込み中が消えるまで待つ。
   */
  capture?: "held" | "alert" | "settled";
};

/** 区切りの無い長い ID（ULID 2 つ分）。折り返し・横 scroll の確認に使う。 */
export const LONG_TASK_ID = "01M44C5GXAC1A8D95VMMRYYG0F01M44DCZWBN71Z80577YM0HTPR";
const LONG_PROJECT_ID = "P01M44C5GXAC1A8D95VMMRYYG0FPROJECTWITHAVERYLONGIDENTIFIER";

const longText =
  "長い文の折り返しを確かめるための説明です。区切りの少ない英数字 averyveryverylongwordwithoutanybreakopportunity_0123456789 も含めます。".repeat(
    3,
  );

type Fixtures = Record<string, unknown>;
type TaskList = { items: Array<Record<string, unknown>>; total: number; next_cursor: null; counts_by_status: object };
type Detail = { task: Record<string, unknown> } & Record<string, unknown>;

function rich(): Fixtures {
  return richFixtures();
}

function longTextFixtures(): Fixtures {
  const base = rich();
  const list = structuredClone(base["/api/v1/tasks"]) as TaskList;
  list.items[0] = { ...list.items[0], title: longText };
  const detail = structuredClone(base["/api/v1/tasks/T1"]) as Detail;
  detail.task = { ...detail.task, title: longText, objective: `${longText}\n${longText}` };
  return { "/api/v1/tasks": list, "/api/v1/tasks/T1": detail };
}

function longTextInbox(): InboxItem[] {
  return inboxItemsFixture().map((item, i) => (i === 0 ? { ...item, title: longText, detail: longText } : item));
}

function longIdFixtures(): Fixtures {
  const base = rich();
  const list = structuredClone(base["/api/v1/tasks"]) as TaskList;
  list.items[0] = { ...list.items[0], id: LONG_TASK_ID, project_id: LONG_PROJECT_ID };
  const detail = structuredClone(base["/api/v1/tasks/T1"]) as Detail;
  detail.task = { ...detail.task, id: LONG_TASK_ID, project_id: LONG_PROJECT_ID };
  // 詳細画面が取得する T1 の下位の path（timeline・execution・routing・changes・tree・artifacts・runs）を
  // 長い ID の path に写す。写さないと既定の 404 になり、実行節などに取得失敗帯が出る（fix-r7.md）。
  const sub: Fixtures = {};
  for (const [path, value] of Object.entries(base)) {
    if (!path.startsWith("/api/v1/tasks/T1/")) continue;
    sub[path.replace("/api/v1/tasks/T1/", `/api/v1/tasks/${LONG_TASK_ID}/`)] = withLongTaskId(value);
  }
  return {
    ...sub,
    "/api/v1/tasks": list,
    [`/api/v1/tasks/${LONG_TASK_ID}`]: detail,
  };
}

/** fixture の task_id（execution の plan・routing の runs の中も）を長い ID に差し替える。関数の fixture はそのまま。 */
function withLongTaskId(value: unknown): unknown {
  if (typeof value === "function" || value === null || typeof value !== "object") return value;
  if (Array.isArray(value)) return value.map(withLongTaskId);
  return Object.fromEntries(
    Object.entries(value).map(([k, v]) => [k, k === "task_id" && v === "T1" ? LONG_TASK_ID : withLongTaskId(v)]),
  );
}

function longIdInbox(): InboxItem[] {
  return inboxItemsFixture().map((item, i) => {
    if (i !== 0 || !item.task) return item;
    const task = { ...item.task, id: LONG_TASK_ID };
    return { ...item, id: `decision:${LONG_TASK_ID}`, task, blocking: { ...item.blocking, tasks: [task] } };
  });
}

function emptyFixtures(): Fixtures {
  return {
    "/api/v1/tasks": { items: [], total: 0, next_cursor: null, counts_by_status: {} },
    "/api/v1/graph": { nodes: [], edges: [] },
  };
}

export const MANY_TASKS = 150;
export const MANY_INBOX = 40;
export const MANY_NOTICES = 60;

function manyFixtures(): Fixtures {
  const base = rich();
  const list = structuredClone(base["/api/v1/tasks"]) as TaskList;
  const template = list.items[3];
  list.items = Array.from({ length: MANY_TASKS }, (_, i) => ({
    ...template,
    id: `T${i + 1}`,
    title: `多数行の確認 ${i + 1}: 一覧の密度と scroll を確かめる`,
  }));
  list.total = MANY_TASKS;
  list.counts_by_status = { ready: MANY_TASKS };
  return { "/api/v1/tasks": list };
}

function manyInbox(): InboxItem[] {
  const [first] = inboxItemsFixture();
  return Array.from({ length: MANY_INBOX }, (_, i) => ({
    ...first,
    id: `decision:D${i + 1}`,
    title: `多数の判断 ${i + 1}`,
    answer: { ...first.answer, path: `/api/v1/inbox/items/decision:D${i + 1}/answer` },
  }));
}

function manyNotices(): Notice[] {
  const [first] = noticesFixture();
  return Array.from({ length: MANY_NOTICES }, (_, i) => ({
    ...first,
    id: `N${i + 1}`,
    group_key: `task_done:N${i + 1}`,
    title: `多数の通知 ${i + 1}`,
  }));
}

export const states = [
  {
    key: "long-text",
    label: "長文（区切りの少ない語を含む題名・目的・判断）",
    daemon: { fixtures: longTextFixtures(), inboxItems: longTextInbox() },
    screens: ["/inbox", "/tasks", "/tasks/T1"],
  },
  {
    key: "long-id",
    label: "区切りの無い長い ID（task・案件・判断）",
    daemon: { fixtures: longIdFixtures(), inboxItems: longIdInbox() },
    screens: ["/inbox", "/tasks", `/tasks/${LONG_TASK_ID}`],
  },
  {
    key: "empty",
    label: "0 件（取得は成功し項目が無い）",
    daemon: { fixtures: emptyFixtures(), inboxItems: [], notices: [] },
    screens: ["/inbox", "/notifications", "/tasks", "/graph"],
  },
  {
    key: "many",
    label: `多数行（task ${MANY_TASKS}・判断 ${MANY_INBOX}・通知 ${MANY_NOTICES}）`,
    daemon: { fixtures: manyFixtures(), inboxItems: manyInbox(), notices: manyNotices() },
    screens: ["/inbox", "/notifications", "/tasks"],
  },
  {
    key: "loading",
    label: "読み込み中（応答を保留し、試験が releaseHeld() で解く）",
    daemon: { hold: {} },
    capture: "held",
    screens: ["/inbox", "/tasks", "/tasks/T1", "/providers"],
  },
  {
    key: "error",
    label: "5xx（画面の取得が 503）",
    daemon: {
      fault: { status: 503, paths: ["/api/v1/inbox", "/api/v1/tasks", "/api/v1/providers"] },
    },
    capture: "alert",
    screens: ["/inbox", "/tasks", "/tasks/T1", "/providers"],
  },
  {
    key: "stale",
    label: "SSE 切断（stream が 503 を返し続け、再接続も失敗する）",
    daemon: { streamStatus: 503 },
    screens: ["/", "/tasks", "/tasks/T1/runs/R1", "/providers"],
  },
  {
    key: "forbidden",
    label: "permission denied（gateway の応答が 403）",
    daemon: {},
    route: { status: 403, paths: ["/api/inbox/items", "/api/tasks", "/api/providers"] },
    screens: ["/inbox", "/tasks", "/tasks/T1", "/providers"],
  },
  {
    key: "reviewing",
    label: "レビュー待ち（判断を開けるタスク）",
    daemon: {},
    screens: ["/tasks/T25"],
  },
] as const satisfies readonly FixtureState[];

export function stateByKey(key: StateKey): FixtureState {
  const found = states.find((state) => state.key === key);
  if (!found) throw new Error(`unknown state: ${key}`);
  return found;
}

// FetchFrame の読み込み中（data-fetch-state="loading"）と、読み込み中の表示（aria-busy）。
const LOADING_SELECTOR = '[data-fetch-state="loading"], [aria-busy="true"]';
const CAPTURE_TIMEOUT = { timeout: 20_000 };

/**
 * 状態を撮れる所まで待つ（固定の時間では待たない）。error は応答の 503 の後に取得失敗の表示（role=alert の中の
 * 「再試行」）が出るまで待つ。待たずに撮ると読み込み中のまま写り、取得失敗の文と再試行が写らなかった（fix-r5）。
 */
export async function waitForStateCapture(page: Page, state: FixtureState): Promise<void> {
  await page
    .locator("h1")
    .first()
    .waitFor({ state: "visible", ...CAPTURE_TIMEOUT });
  const capture = state.capture ?? "settled";
  if (capture === "held") {
    await page
      .locator('[data-fetch-state="loading"]')
      .first()
      .waitFor({ state: "visible", ...CAPTURE_TIMEOUT });
  } else if (capture === "alert") {
    await page
      .locator('[data-fetch-state="error"][role="alert"]')
      .first()
      .getByRole("button", { name: "再試行" })
      .waitFor({ state: "visible", ...CAPTURE_TIMEOUT });
  } else {
    await page.waitForFunction((selector) => !document.querySelector(selector), LOADING_SELECTOR, CAPTURE_TIMEOUT);
    if (state.key === "reviewing") {
      await page
        .locator('[data-testid="decision-status"][data-status="reviewing"]')
        .waitFor({ state: "attached", ...CAPTURE_TIMEOUT });
      const decisionSwitch = page.locator('[data-testid="mobile-sections"] [data-section="decision"]');
      if (await decisionSwitch.isVisible()) await decisionSwitch.click();
      await page
        .locator('[data-testid="decision-panel"] button')
        .first()
        .waitFor({ state: "visible", ...CAPTURE_TIMEOUT });
    }
    if (state.key === "stale" && (page.url().endsWith("/tasks") || page.url().includes("/runs/"))) {
      await page.locator('[data-testid="connection-stale-notice"]').waitFor({ state: "visible", ...CAPTURE_TIMEOUT });
    }
  }
}

/** 状態の route（403 など）を page に当てる。route の無い状態では何もしない。 */
export async function applyStateRoute(page: Page, state: FixtureState): Promise<void> {
  const rule = state.route;
  if (!rule) return;
  await page.route(
    (url) => rule.paths.some((prefix) => url.pathname === prefix || url.pathname.startsWith(`${prefix}/`)),
    (route) =>
      route.fulfill({
        status: rule.status,
        contentType: "application/json",
        body: JSON.stringify({ error: rule.status === 403 ? "forbidden" : "error" }),
      }),
  );
}
