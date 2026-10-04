import { Link, useRouterState } from "@tanstack/react-router";
import { buttonVariants } from "../ui/button";
import { ScreenFrame } from "./screen-frame";

// shell の中の 404（R42）。gateway も未定義の path に 404 を返す。ナビ・ホームへ戻れる。
export function NotFound() {
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  return (
    <ScreenFrame
      title="ページが見つかりません"
      route="*"
      breadcrumb={[{ label: "ホーム", link: { to: "/" } }, { label: "ページが見つかりません" }]}
      description="この URL の画面はありません。ナビかホームから移ってください。"
      actions={
        <Link to="/" className={buttonVariants({ variant: "primary" })}>
          ホームへ戻る
        </Link>
      }
    >
      <p data-not-found-path className="min-w-0 text-label text-muted-foreground">
        開こうとした URL:{" "}
        <code className="break-all rounded-sm bg-code px-1 font-mono text-code text-code-foreground">{pathname}</code>
      </p>
    </ScreenFrame>
  );
}
