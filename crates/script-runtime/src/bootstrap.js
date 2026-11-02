// celeris の静的 bootstrap（crates/script-runtime/src/actor.rs が bundle より先に classic script として評価する）。
// 1. celeris:host が使う op だけを 1 回限りの関数で渡す。
// 2. 時計と乱数を呼ぶと throw させる（境界 ADR 2026-11-02 (c)。現在時刻と seed は入力 DTO で渡す）。
// 3. Deno.* と __bootstrap を消し、bundle から deno_core の op 表へ届かないようにする。
((globalThis) => {
  "use strict";
  const coreOps = globalThis.Deno.core.ops;
  const hostOps = Object.freeze({
    version: coreOps.op_host_version,
    selftestEcho: coreOps.op_host_selftest_echo,
    cosDigestListRuns: coreOps.op_cos_digest_list_runs,
    cosDigestReadRun: coreOps.op_cos_digest_read_run,
  });
  let claimed = false;
  Object.defineProperty(globalThis, "__celerisHostOps", {
    configurable: true,
    enumerable: false,
    writable: false,
    value: () => {
      if (claimed) throw new TypeError("celeris: host ops already claimed");
      claimed = true;
      delete globalThis.__celerisHostOps;
      return hostOps;
    },
  });

  const forbidden = (what) => () => {
    throw new TypeError(
      `celeris: ${what} is not available (pass the time or seed in the input DTO)`,
    );
  };
  Math.random = forbidden("Math.random");
  const OrigDate = globalThis.Date;
  const nowForbidden = forbidden("the current time");
  function Date(...args) {
    if (!new.target) nowForbidden();
    if (args.length === 0) nowForbidden();
    return Reflect.construct(OrigDate, args, new.target);
  }
  Object.setPrototypeOf(Date, OrigDate);
  Date.prototype = OrigDate.prototype;
  Object.defineProperty(OrigDate.prototype, "constructor", {
    value: Date,
    configurable: true,
    writable: true,
  });
  Date.now = nowForbidden;
  Date.UTC = OrigDate.UTC;
  Date.parse = OrigDate.parse;
  globalThis.Date = Date;
  if (globalThis.performance !== undefined) {
    delete globalThis.performance;
  }

  delete globalThis.Deno;
  delete globalThis.__bootstrap;
})(globalThis);
