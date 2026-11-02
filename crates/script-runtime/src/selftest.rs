//! 試験用の領域 `host_selftest` の呼び出しと結果の照合（`script-load-check`・`script-bench` が使う）。
//!
//! 領域は Host API の op を 1 回呼んで `{ host_api, reply }` を返す。制御の判定はしない。

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{Value, json};

use crate::dto::HOST_API_VERSION;
use crate::handle::ScriptCall;
use crate::manifest::ScriptManifest;

/// 試験用の領域の名前。
pub const SELFTEST_DOMAIN: &str = "host_selftest";

const TASK_ID: &str = "01M6DJ6QCMCWT80MTYJ7SNE0XK";
/// 2^53−1 を超える値（[`crate::dto::U64String`] の写像も通す）。
const TOKENS: &str = "9007199254740993";
const AT: &str = "2026-11-02T00:00:00Z";

/// manifest が試験用の領域を宣言しているか確かめる。
pub fn check_declared(manifest: &ScriptManifest) -> Result<(), String> {
    if manifest.domains.iter().any(|d| d == SELFTEST_DOMAIN) {
        Ok(())
    } else {
        Err(format!(
            "manifest does not declare domain {SELFTEST_DOMAIN}"
        ))
    }
}

/// 試験用の領域を 1 回呼ぶ要求。
pub fn selftest_call(timeout: Duration) -> ScriptCall {
    ScriptCall {
        domain: SELFTEST_DOMAIN.into(),
        input: json!({
            "task_id": TASK_ID,
            "note": "selftest",
            "tokens": TOKENS,
            "priority": "high",
            "at": AT,
        }),
        scope_task_ids: BTreeSet::from([TASK_ID.to_owned()]),
        timeout,
        host_sink: None,
    }
}

/// [`selftest_call`] の結果が Rust の op を通った値か確かめる。
pub fn check_output(output: &Value) -> Result<(), String> {
    let expected = json!({
        "host_api": HOST_API_VERSION,
        "reply": {
            "task_id": TASK_ID,
            "note": "selftest",
            "tokens": TOKENS,
            "priority": "high",
            "at": AT,
            "observation": { "kind": "noted", "length": 8 },
        },
    });
    if *output == expected {
        Ok(())
    } else {
        Err(format!("unexpected selftest output: {output}"))
    }
}
