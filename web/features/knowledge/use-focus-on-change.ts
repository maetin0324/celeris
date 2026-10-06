import { useEffect, useRef } from "react";

/**
 * 一覧で選び直して `key`（知識の path・手順書の名前）が変わった時だけ、中身の見出しへ focus を移す。
 * 直接開いた初回の表示では動かない（shell が遷移後に h1 へ focus を移す S4 と競合させない）。
 * 選び直しで中身の部品ごと mount し直す画面は、`pendingAtMount` に「一覧から選び直した」ことを渡す。
 * 中身の読み込みが終わって `ready` になってから 1 度だけ動く。見出しが viewport に入っていれば scroll しない
 * （広い幅では scroll 位置を保ち、狭い幅では一覧を畳んで中身を上に出すので通常は scroll 不要）。
 */
export function useFocusOnChange<T extends HTMLElement>(key: string, ready: boolean, pendingAtMount = false) {
  const ref = useRef<T>(null);
  const previous = useRef(key);
  const pending = useRef(pendingAtMount);
  useEffect(() => {
    if (previous.current === key) return;
    previous.current = key;
    pending.current = key !== "";
  }, [key]);
  useEffect(() => {
    const element = ref.current;
    if (!pending.current || !ready || !element || previous.current !== key) return;
    pending.current = false;
    element.focus({ preventScroll: true });
    const box = element.getBoundingClientRect();
    if (box.top < 0 || box.bottom > window.innerHeight) element.scrollIntoView({ block: "start" });
  }, [key, ready]);
  return ref;
}
