// manifest 試験の手書きの bundle（業務 TS ではない）。本文を変えたら manifest.json の bundle_sha256 を直す。
import { hostVersion, selftestEcho } from "celeris:host";

globalThis.__celerisDispatch = async (domain, input, ctx) => {
  if (domain !== "host_selftest") throw new Error(`unknown domain: ${domain}`);
  return { host_api: hostVersion(), reply: await selftestEcho(ctx, input) };
};
