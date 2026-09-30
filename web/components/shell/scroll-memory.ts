import type { AnyRouter, ParsedLocation } from "@tanstack/react-router";

// 戻る・進むで window の scroll 位置を戻す（S5・P2-02）。履歴の項目ごとに位置を覚え、新しい項目は先頭から出す。
// 位置は memory だけに持ち、storage には書かない（P2-06 の storage の規律）。
function entryKey(location: ParsedLocation): string {
  const state = location.state as { __TSR_key?: string; key?: string } | undefined;
  return state?.__TSR_key ?? state?.key ?? location.href;
}

export function installScrollMemory(router: AnyRouter): () => void {
  const positions = new Map<string, number>();
  let current = entryKey(router.state.location);
  let navigating = false;
  const onScroll = () => {
    if (!navigating) positions.set(current, window.scrollY);
  };
  window.addEventListener("scroll", onScroll, { passive: true });
  const offBefore = router.subscribe("onBeforeNavigate", () => {
    positions.set(current, window.scrollY);
    navigating = true;
  });
  const offResolved = router.subscribe("onResolved", ({ toLocation }) => {
    current = entryKey(toLocation);
    const y = positions.get(current) ?? 0;
    requestAnimationFrame(() => {
      window.scrollTo(0, y);
      navigating = false;
    });
  });
  return () => {
    window.removeEventListener("scroll", onScroll);
    offBefore();
    offResolved();
  };
}
