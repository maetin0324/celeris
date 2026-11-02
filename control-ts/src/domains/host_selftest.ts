// 試験用の領域: celeris:host の op を 1 回呼んで写し返す。制御の挙動はここに移さない。
import { hostVersion, selftestEcho, type RequestContext, type SelftestEchoRequest } from "celeris:host";

export async function hostSelftest(input: unknown, ctx: RequestContext) {
  // 入力の検証は Rust の op が deny_unknown_fields で行う。TS 側の検査は境界にしない。
  const reply = await selftestEcho(ctx, input as SelftestEchoRequest);
  return { host_api: hostVersion(), reply };
}
