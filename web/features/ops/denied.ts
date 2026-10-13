import type { ActionResult } from "../../components/actions/use-action-result";

export function isDenied(results: Record<string, ActionResult>): boolean {
  return Object.values(results).some((r) => r.status === 403 || r.status === 401);
}

/** 権限が無いときの理由文。操作の名前を添えて、何が止まっているかを書く。 */
export function deniedMessage(actions: string): string {
  return `この操作を行う権限がありません。${actions}は止めています。管理者に権限を確認してください。`;
}
