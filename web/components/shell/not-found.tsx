import { Link } from "@tanstack/react-router";
import { buttonClassName } from "../ui/button";
import { ScreenFrame } from "./screen-frame";

// shell の中の 404（R42）。gateway も未定義の path に 404 を返す。ナビ・ホームへ戻れる。
export function NotFound() {
  return (
    <ScreenFrame title="ページが見つかりません" route="*">
      <p className="text-sm text-neutral-700">この URL の画面はありません。ナビかホームから移ってください。</p>
      <Link to="/" className={`${buttonClassName} self-start`}>
        ホームへ戻る
      </Link>
    </ScreenFrame>
  );
}
