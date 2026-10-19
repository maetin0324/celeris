//! 無試行の lock 入口 `celeris-campaign offline-lock`。
//! 通常経路と同じ関数（`preflight::expected_lock` / `lock_exec::begin`・`save` / `env_lock::establish`）だけで、
//! `env-lock.json`（env-lock/3）と `lock.json`（campaign lock）を作る・再照合する。executor・watch・runner・台帳は呼ばない。
//! catalog は承認済み実証跡（`GET /api/v1/llm/models` の本文を保存した file）から取り、その sha256 を呼び出し側が明示する。
use std::path::Path;

use serde_json::{Value, json};

use crate::{
    env_lock::{self, CatalogRow, Observed},
    grader::sha256_file,
    lock_exec::{self, Start},
    manifest::CampaignManifest,
    preflight::{self, Inputs},
};

/// 実証跡 file を hash 固定で読み、catalog を取り出す。hash の不一致・本文の不備・空 catalog は Err。
pub fn catalog_from_evidence(path: &Path, sha256: &str) -> Result<Vec<CatalogRow>, String> {
    let got = sha256_file(path)?;
    if !got.eq_ignore_ascii_case(sha256) {
        return Err(format!(
            "{}: evidence sha256 {got} differs from the approved {sha256}",
            path.display()
        ));
    }
    let body: Value = serde_json::from_slice(
        &std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?,
    )
    .map_err(|e| format!("{}: {e}", path.display()))?;
    env_lock::catalog_from_models(&body)
}

/// `verify_only` なら何も書かず、保存済みの env-lock と lock.json が現物から作り直した期待値と一致することだけを確かめる。
/// そうでなければ、無いものを作り、あるものは照合する。不一致・入力不足は何も書く前に Err。
pub fn establish(
    inp: &Inputs,
    m: &CampaignManifest,
    obs: &Observed,
    out: &Path,
    verify_only: bool,
) -> Result<Value, String> {
    let expected = preflight::expected_lock(inp, m)?;
    // 書く前に全て判定する（env-lock の期待値・lock の入力 source・out の状態）。
    env_lock::expected(m, obs).map_err(|e| e.join("\n"))?;
    let start = lock_exec::begin(m, &expected, out, inp.lock_sources())?;
    if verify_only && start == Start::Fresh {
        return Err(format!(
            "{}: no {} to verify",
            out.display(),
            lock_exec::LOCK_FILE
        ));
    }
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let env = env_lock::establish(m, obs, out, verify_only)?;
    if start == Start::Fresh {
        lock_exec::save(out, &expected)?;
    }
    lock_exec::load_saved(m, out, Some(&expected))?
        .ok_or_else(|| format!("{}: lock vanished after save", out.display()))?;
    let name = |c: bool| if c { "created" } else { "verified" };
    Ok(json!({
        "schema": "celeris.campaign-offline-lock/1",
        "paid_calls_made": 0,
        "env_lock": name(env == env_lock::Outcome::Created),
        "campaign_lock": name(start == Start::Fresh),
        "campaign_sha256": expected.campaign_sha256,
        "lock_sha256": expected.sha256,
    }))
}
