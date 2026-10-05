import { type ReactNode, useEffect, useLayoutEffect, useRef, useState } from "react";
import { Button } from "../../components/ui/button";
import { Icon } from "../../components/ui/icon";

// ホームの会話の枠。会話（ConsoleView）をこの枠の中だけで scroll させ、ページ全体を viewport の高さに収める。
// ConsoleView のページ追従は contained モードで止める。枠の高さを「viewport − 枠の上端 − main の下の余白」に合わせて
// ページを伸ばさず、window の scroll を起こさない。高さは runtime の viewport 補正なので style へ直接書く。
// 枠の中は末尾にいる間だけ追記に合わせて末尾へ送り、離れて読んでいる間は動かさずに「最新へ」を出す。末尾は
// ConsoleView の下の余白（fixed の送信欄の逃げ）のうち送信欄に隠れる分までとし、最後の block を送信欄のすぐ上に置く。

/**
 * 枠の中で会話が見える最低の高さ（px）。枠の上端に留めた宛先の行と、枠の下に重なる fixed の送信欄の高さは別に足す。
 * これより低い viewport ではページ側の scroll に任せる。以前は枠の高さ全体を 160px にしていたので、stale の黄帯が出た
 * 360 では宛先の行と送信欄に食われて会話が約 40px しか見えなかった（fix-r6 narrow）。
 */
const MIN_VISIBLE = 160;
/** 末尾とみなす距離（px）。 */
const NEAR_BOTTOM = 48;
/** 最後の block・「最新へ」と送信欄の間に残す隙間（px）。 */
const GAP = 12;

function composerOf(el: HTMLElement): Element | null {
  return el.querySelector('[data-testid="console-composer"]');
}

// 末尾へ送ったときに見せなくてよい下の余白の量。ConsoleView の padding-bottom のうち、送信欄に隠れず空白に見える分。
// 重なる量は「ページを末尾まで scroll したときの枠の下端」から測る（送信欄の高さ − 枠の下の余白）。枠の今の位置から
// 測ると、最低の高さを保って枠が viewport の下へはみ出す低い viewport で余白を全部見せてしまった（fix-r6 narrow）。
function slack(el: HTMLElement, content: Element): number {
  const pad = Number.parseFloat(getComputedStyle(content).paddingBottom) || 0;
  const composer = composerOf(el);
  const overlap = composer ? Math.max(0, composer.getBoundingClientRect().height - belowOf(el)) : 0;
  return Math.max(0, pad - overlap - GAP);
}

// 枠の下端から viewport の下端までに残る量（main の下の余白と、main の後ろに続く列の分）。
function belowOf(el: HTMLElement): number {
  const main = el.closest("main");
  if (!main) return 0;
  let below = Number.parseFloat(getComputedStyle(main).paddingBottom) || 0;
  const column = main.parentElement;
  if (column) below += Math.max(0, column.getBoundingClientRect().bottom - main.getBoundingClientRect().bottom);
  return below;
}

export function ConsoleRegion({ children }: { children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  const latestRef = useRef<HTMLDivElement>(null);
  const atBottom = useRef(true);
  const [away, setAway] = useState(false);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const main = el.closest("main");
    const fitHeight = () => {
      const top = el.getBoundingClientRect().top + window.scrollY;
      const below = belowOf(el);
      const toolbar = el.querySelector("[data-console-toolbar]");
      const composer = composerOf(el);
      const covered =
        (toolbar ? toolbar.getBoundingClientRect().height : 0) +
        (composer ? composer.getBoundingClientRect().height : 0);
      const height = Math.max(MIN_VISIBLE + covered, Math.floor(window.innerHeight - top - below));
      if (el.style.height !== `${height}px`) el.style.height = `${height}px`;
    };
    fitHeight();
    const observer = new ResizeObserver(fitHeight);
    if (el.parentElement) observer.observe(el.parentElement);
    if (main?.parentElement) observer.observe(main.parentElement);
    // 宛先の行・送信欄は読み込みや失敗の文で高さが変わる。後から現れることもあるので枠の子の変化も見る。
    const watchCovered = () => {
      for (const node of [el.querySelector("[data-console-toolbar]"), composerOf(el)]) if (node) observer.observe(node);
    };
    watchCovered();
    const mutations = new MutationObserver(() => {
      watchCovered();
      fitHeight();
    });
    mutations.observe(el, { childList: true, subtree: true });
    window.addEventListener("resize", fitHeight);
    return () => {
      observer.disconnect();
      mutations.disconnect();
      window.removeEventListener("resize", fitHeight);
    };
  }, []);

  useEffect(() => {
    const el = ref.current;
    const content = el?.firstElementChild;
    if (!el || !content) return;
    let lastHeight = content.getBoundingClientRect().height;
    const onScroll = () => {
      atBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight - slack(el, content) <= NEAR_BOTTOM;
      if (atBottom.current) setAway(false);
    };
    const follow = () => {
      const height = content.getBoundingClientRect().height;
      const grew = height > lastHeight;
      lastHeight = height;
      if (atBottom.current) el.scrollTop = Math.max(0, el.scrollHeight - el.clientHeight - slack(el, content));
      else if (grew) setAway(true);
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    const observer = new ResizeObserver(follow);
    observer.observe(content);
    return () => {
      el.removeEventListener("scroll", onScroll);
      observer.disconnect();
    };
  }, []);

  // 「最新へ」は送信欄のすぐ上に置く。送信欄の高さ（送信の失敗の文・safe area）は runtime に測る。
  useLayoutEffect(() => {
    const el = ref.current;
    const button = latestRef.current;
    if (!away || !el || !button) return;
    const composer = composerOf(el);
    button.style.bottom = `${(composer ? composer.getBoundingClientRect().height : 0) + GAP}px`;
  }, [away]);

  function toLatest() {
    const el = ref.current;
    const content = el?.firstElementChild;
    if (!el || !content) return;
    atBottom.current = true;
    setAway(false);
    el.scrollTop = Math.max(0, el.scrollHeight - el.clientHeight - slack(el, content));
  }

  return (
    <div ref={ref} data-home-console className="min-h-0 min-w-0 overflow-y-auto overscroll-contain">
      {children}
      {away ? (
        <div ref={latestRef} className="fixed right-4 z-30">
          <Button
            type="button"
            size="sm"
            className="shadow-popover"
            onClick={toLatest}
            data-testid="home-console-latest"
          >
            <Icon name="chevron-down" />
            最新へ
          </Button>
        </div>
      ) : null}
    </div>
  );
}
