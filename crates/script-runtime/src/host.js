// celeris:host v1（静的。Rust の crates/script-runtime/src/loader.rs が module として渡す）。
// op の結線と HostError だけを持つ。権限の検査は Rust の op が行う（ここで検査しても境界にならない）。
const ops = globalThis.__celerisHostOps();

const ADDITIONAL_KEYS = Symbol.for("errorAdditionalPropertyKeys");

export class HostError extends Error {
  constructor(code, message) {
    super(message);
    this.name = "HostError";
    this.code = code;
    // Rust 側の JsError.additional_properties に code を載せる。
    this[ADDITIONAL_KEYS] = ["code"];
  }
}

function unwrap(result) {
  if (result.kind === "ok") return result.value;
  throw new HostError(result.error.code, result.error.message);
}

export const HOST_API_VERSION = ops.version();

export function hostVersion() {
  return ops.version();
}

// ctx は __celerisDispatch の第 3 引数（{ token }）。
export async function selftestEcho(ctx, request) {
  return unwrap(await ops.selftestEcho(ctx.token, request));
}
