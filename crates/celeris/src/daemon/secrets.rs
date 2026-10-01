//! 秘密の解決と env の合成（ADR-0030）。アダプタ設定の組み立てが使う。

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

/// `[adapters.<種別>].env` にプロバイダの `env` を重ねる（同名キーはプロバイダが優先。順序は決定的）。
/// ADR-0030 以降、本体（`build_adapters`）は `merged_env_with_secrets` を使う。これはテストが期待値を
/// 組み立てるのに使う（`env_from_secrets` が空なら `merged_env_with_secrets` と同じ結果になる）。
#[cfg(test)]
pub(crate) fn merged_env(
    base: &HashMap<String, String>,
    provider: &HashMap<String, String>,
) -> Vec<(String, String)> {
    let mut merged: BTreeMap<String, String> =
        base.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    merged.extend(provider.iter().map(|(k, v)| (k.clone(), v.clone())));
    merged.into_iter().collect()
}

/// ADR-0030 D1: `[secrets] dir` の下の `<id>` ファイルを読み、末尾の改行を落とした値を返す。無い・読めない
/// ときは設定エラーにせず `warn!` を出して `None`（値はログに出さない。id だけ記録する。ADR-0024 D5 と同じ規律）。
pub(crate) fn resolve_secret(secrets_dir: Option<&Path>, id: &str) -> Option<String> {
    let dir = secrets_dir?;
    let path = dir.join(id);
    match std::fs::read_to_string(&path) {
        Ok(text) => Some(text.trim_end_matches(['\n', '\r']).to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::warn!(secret_id = %id, "secret not found; omitting env var (ADR-0030 D2)");
            None
        }
        Err(e) => {
            tracing::warn!(secret_id = %id, error = %e, "cannot read secret; omitting env var (ADR-0030 D2)");
            None
        }
    }
}

/// `mapping`（環境変数名 → 秘密 id）のキーを決定的な順で解決し、見つかったものだけ `merged` に上書きする
/// （見つからなければそのキーには**触れない**。下の層の値が残る。ADR-0030 D2）。
pub(crate) fn apply_env_from_secrets(
    merged: &mut BTreeMap<String, String>,
    mapping: &HashMap<String, String>,
    secrets_dir: Option<&Path>,
) {
    let mut env_keys: Vec<&String> = mapping.keys().collect();
    env_keys.sort();
    for env_key in env_keys {
        let secret_id = &mapping[env_key];
        if let Some(value) = resolve_secret(secrets_dir, secret_id) {
            merged.insert(env_key.clone(), value);
        }
    }
}

/// ADR-0030 D2: 優先順は celeris の環境（プロセス継承。ここでは扱わない）< `[adapters.*].env` <
/// `[adapters.*].env_from_secrets` < 行の `env` < 行の `env_from_secrets`。
pub(crate) fn merged_env_with_secrets(
    base_env: &HashMap<String, String>,
    base_env_from_secrets: &HashMap<String, String>,
    row_env: &HashMap<String, String>,
    row_env_from_secrets: &HashMap<String, String>,
    secrets_dir: Option<&Path>,
) -> Vec<(String, String)> {
    let mut merged: BTreeMap<String, String> = base_env
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    apply_env_from_secrets(&mut merged, base_env_from_secrets, secrets_dir);
    merged.extend(row_env.iter().map(|(k, v)| (k.clone(), v.clone())));
    apply_env_from_secrets(&mut merged, row_env_from_secrets, secrets_dir);
    merged.into_iter().collect()
}

/// プロバイダの `model` が空でなければそれ、空なら `[adapters.<種別>].model`（ADR-0012 D1）。
pub(crate) fn effective_model(
    provider_model: &str,
    adapter_model: &Option<String>,
) -> Option<String> {
    if provider_model.is_empty() {
        adapter_model.clone()
    } else {
        Some(provider_model.to_string())
    }
}
