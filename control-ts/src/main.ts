// bundle の入口。Rust の actor は globalThis.__celerisDispatch(domain, input, ctx) を呼ぶ。
import type { RequestContext } from "celeris:host";
import { domains } from "./domains/index";

declare global {
  var __celerisDispatch: (domain: string, input: unknown, ctx: RequestContext) => Promise<unknown>;
}

globalThis.__celerisDispatch = async (domain, input, ctx) => {
  const handler = domains[domain];
  if (handler === undefined) throw new Error(`unknown domain: ${domain}`);
  return handler(input, ctx);
};
