// Console の画面状態（P3-02）。module 単位で持ち、画面遷移・daemon の再取得・再 mount で消えない。
// 入力中の文と展開の状態は sessionStorage にも写す（リロードでも残る。secret ではない）。

type Listener = () => void;
const listeners = new Set<Listener>();
const emit = () => {
  for (const l of listeners) l();
};

const KEY = "celeris-console-state";

type Persisted = {
  drafts: Record<string, string>;
  expanded: string[];
  conversations: Record<string, { id: string; since: string | null }>;
};

function storage(): Storage | null {
  try {
    return typeof sessionStorage === "undefined" ? null : sessionStorage;
  } catch {
    return null;
  }
}

function load(): Persisted {
  try {
    const raw = storage()?.getItem(KEY);
    if (raw) return { drafts: {}, expanded: [], conversations: {}, ...(JSON.parse(raw) as Partial<Persisted>) };
  } catch {
    // 壊れた値は捨てる
  }
  return { drafts: {}, expanded: [], conversations: {} };
}

let state = load();
function save() {
  try {
    storage()?.setItem(KEY, JSON.stringify(state));
  } catch {
    // 容量超過などは無視（メモリには残る）
  }
}

const subscribe = (l: Listener) => {
  listeners.add(l);
  return () => listeners.delete(l);
};

export const draftStore = {
  get: (scope: string) => state.drafts[scope] ?? "",
  set(scope: string, text: string) {
    state = { ...state, drafts: { ...state.drafts, [scope]: text } };
    save();
    emit();
  },
  subscribe,
};

export const expansionStore = {
  has: (id: string) => state.expanded.includes(id),
  snapshot: () => state.expanded,
  toggle(id: string) {
    state = {
      ...state,
      expanded: state.expanded.includes(id) ? state.expanded.filter((x) => x !== id) : [...state.expanded, id],
    };
    save();
    emit();
  },
  subscribe,
};

const initial = { id: "0", since: null as string | null };
export const conversationStore = {
  get: (scope: string) => state.conversations[scope] ?? initial,
  next(scope: string, since: string | null) {
    const current = state.conversations[scope] ?? initial;
    state = {
      ...state,
      conversations: { ...state.conversations, [scope]: { id: String(Number(current.id) + 1), since } },
    };
    save();
    emit();
  },
  subscribe,
};

/** テスト用。 */
export function resetConsoleStores(): void {
  state = { drafts: {}, expanded: [], conversations: {} };
  save();
  emit();
}
