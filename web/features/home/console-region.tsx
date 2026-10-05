import { type ReactNode, useEffect, useLayoutEffect, useRef, useState } from "react";
import { Button } from "../../components/ui/button";
import { Icon } from "../../components/ui/icon";

// ホームの会話の枠。会話（ConsoleView）をこの枠の中だけで scroll させ、ページ全体を viewport の高さに収める。
// ConsoleView は追記のたびにページ（window）を末尾へ送るので、ページが viewport より高いと初回表示で
// h1 と判断待ちが上へ押し出される。枠の高さを「viewport − 枠の上端 − main の下の余白」に合わせて
// ページを伸ばさず、window の scroll を起こさない。高さは runtime の viewport 補正なので style へ直接書く。
// 枠の中は末尾にいる間だけ追記に合わせて末尾へ送り、離れて読んでいる間は動かさずに「最新へ」を出す。末尾は
// ConsoleView の下の余白（fixed の送信欄の逃げ）のうち送信欄に隠れる分までとし、最後の block を送信欄のすぐ上に置く。

/** 枠の最低の高さ（px）。これより低い viewport ではページ側の scroll に任せる。 */
const MIN_HEIGHT = 240;
/** 末尾とみなす距離（px）。 */
const NEAR_BOTTOM = 48;
/** 最後の block・「最新へ」と送信欄の間に残す隙間（px）。 */
const GAP = 12;

function composerOf(el: HTMLElement): Element | null {
  return el.querySelector('[data-testid="console-composer"]');
}

// 末尾へ送ったときに見せなくてよい下の余白の量。ConsoleView の padding-bottom のうち、送信欄に隠れず空白に見える分。
function slack(el: HTMLElement, content: Element): number {
  const pad = Number.parseFloat(getComputedStyle(content).paddingBottom) || 0;
  const composer = composerOf(el);
  const overlap = composer ? Math.max(0, el.getBoundingClientRect().bottom - composer.getBoundingClientRect().top) : 0;
  return Math.max(0, pad - overlap - GAP);
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
      let below = 0;
      if (main) {
        below += Number.parseFloat(getComputedStyle(main).paddingBottom) || 0;
        const column = main.parentElement;
        if (column) below += Math.max(0, column.getBoundingClientRect().bottom - main.getBoundingClientRect().bottom);
      }
      const height = Math.max(MIN_HEIGHT, Math.floor(window.innerHeight - top - below));
      if (el.style.height !== `${height}px`) el.style.height = `${height}px`;
    };
    fitHeight();
    const observer = new ResizeObserver(fitHeight);
    if (el.parentElement) observer.observe(el.parentElement);
    if (main?.parentElement) observer.observe(main.parentElement);
    window.addEventListener("resize", fitHeight);
    return () => {
      observer.disconnect();
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
