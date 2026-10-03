//! `[[providers]]` からアダプタを組み立てる（ADR-0012 D1）。秘密の使われ方と実効モデルの一覧も返す。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use task_dispatch::ProviderId;
use task_ops::daemon::ProviderLive;
use task_worker::{
    AcpAdapter, AcpConfig, AiderAdapter, AiderConfig, BrowserSpecialistAdapter, ClaudeCodeAdapter,
    ClaudeCodeConfig, CodexAdapter, CodexConfig, FakeAdapter, LangMemAdapter, LangMemConfig,
    LdrAdapter, LdrConfig, PaperQaAdapter, PaperQaConfig, WorkerAdapter,
};

use super::secrets::{effective_model, merged_env_with_secrets, resolve_secret};
use crate::{Config, config};

/// ADR-0012 D1: `[[providers]]` の各行（モデル供給元）ごとにアダプタのインスタンスを作る。`[adapters.<種別>]` を基本設定とし、
/// プロバイダの `env` と `model` を重ねる。キーはプロバイダ ID。
pub fn build_adapters(config: &Config) -> HashMap<ProviderId, Arc<dyn WorkerAdapter>> {
    let secrets_dir = config.secrets.as_ref().map(|s| s.dir.as_path());
    let mut adapters: HashMap<ProviderId, Arc<dyn WorkerAdapter>> = HashMap::new();
    for p in &config.providers {
        let adapter: Arc<dyn WorkerAdapter> = match p.adapter.as_str() {
            ClaudeCodeAdapter::ID => {
                let base = &config.adapters.claude_code;
                Arc::new(ClaudeCodeAdapter::new(ClaudeCodeConfig {
                    command: base.command.clone(),
                    extra_args: base.extra_args.clone(),
                    permission_mode: base.permission_mode.clone(),
                    model: effective_model(&p.model, &base.model),
                    env: merged_env_with_secrets(
                        &base.env,
                        &base.env_from_secrets,
                        &p.env,
                        &p.env_from_secrets,
                        secrets_dir,
                    ),
                    // ADR-0043 D3（Phase 56）: コンテナで走らせるかはタスクごとに決まるので、ここでは常に `None`
                    // （ディスパッチャが `with_container` で包んだ複製を作る）。
                    container: None,
                }))
            }
            CodexAdapter::ID => {
                let base = &config.adapters.codex;
                Arc::new(CodexAdapter::new(CodexConfig {
                    command: base.command.clone(),
                    extra_args: base.extra_args.clone(),
                    model: effective_model(&p.model, &base.model),
                    // ADR-0069 Phase 118 D1: tier ごとの effort は `TieredAdapter::run` が
                    // `with_reasoning_effort` で動的に足す（`p.tier_models` から）。ここは常に `None`。
                    reasoning_effort: None,
                    env: merged_env_with_secrets(
                        &base.env,
                        &base.env_from_secrets,
                        &p.env,
                        &p.env_from_secrets,
                        secrets_dir,
                    ),
                    // ADR-0043 D3（Phase 56）: コンテナで走らせるかはタスクごとに決まるので、ここでは常に `None`
                    // （ディスパッチャが `with_container` で包んだ複製を作る）。
                    container: None,
                    // ADR-0054 D1（Phase 67）: `[adapters.codex] resume_mode`（既定 `exec_resume`）。
                    resume_mode: base.resolved_resume_mode(),
                    // ADR-0054 Phase 112 D1: `[adapters.codex] resume_bypass`（既定 off）。
                    resume_bypass: base.resolved_resume_bypass(),
                }))
            }
            AiderAdapter::ID => {
                let base = &config.adapters.aider;
                Arc::new(AiderAdapter::new(AiderConfig {
                    command: base.command.clone(),
                    extra_args: base.extra_args.clone(),
                    model: effective_model(&p.model, &base.model),
                    env: merged_env_with_secrets(
                        &base.env,
                        &base.env_from_secrets,
                        &p.env,
                        &p.env_from_secrets,
                        secrets_dir,
                    ),
                    // ADR-0043 D3（Phase 56）: コンテナで走らせるかはタスクごとに決まるので、ここでは常に `None`
                    // （ディスパッチャが `with_container` で包んだ複製を作る）。
                    container: None,
                }))
            }
            AcpAdapter::ID | BrowserSpecialistAdapter::ID => {
                let base = &config.adapters.acp;
                let inner: Arc<dyn WorkerAdapter> = Arc::new(AcpAdapter::new(AcpConfig {
                    // ADR-0026 D2: `command`/`args` は行ごとに上書きできる（別の ACP エージェントを同居させる
                    // ため）。`Config::validate` が ACP と browser-specialist 以外での指定を拒否している。
                    command: p.command.clone().unwrap_or_else(|| base.command.clone()),
                    args: p.args.clone().unwrap_or_else(|| base.args.clone()),
                    env: merged_env_with_secrets(
                        &base.env,
                        &base.env_from_secrets,
                        &p.env,
                        &p.env_from_secrets,
                        secrets_dir,
                    ),
                    permission: base.permission,
                    // ADR-0026 D3: `[adapters.acp]` にモデルの既定値は無い（CLI の `--model` フラグではなく
                    // `session/set_config_option` で渡すので、行の `model` が空ならモデル指定なしになるだけ）。
                    model: effective_model(&p.model, &None),
                    model_option_id: base.model_option_id.clone(),
                    startup_timeout: Duration::from_secs(base.startup_timeout_secs),
                    // ADR-0043 D3（Phase 56）: コンテナで走らせるかはタスクごとに決まるので、ここでは常に `None`
                    // （ディスパッチャが `with_container` で包んだ複製を作る）。
                    container: None,
                }));
                if p.adapter == BrowserSpecialistAdapter::ID {
                    Arc::new(BrowserSpecialistAdapter::new(inner))
                } else {
                    inner
                }
            }
            PaperQaAdapter::ID => {
                let base = &config.adapters.paperqa;
                Arc::new(PaperQaAdapter::new(PaperQaConfig {
                    command: base.command.clone(),
                    // ADR-0027 D3: 行ごとに設定ファイルを上書きできる（`command`/`args` と同じ作り）。
                    settings: p.settings.clone().or_else(|| base.settings.clone()),
                    paper_directory: base.paper_directory.clone(),
                    index_directory: base.index_directory.clone(),
                    index_name: base.index_name.clone(),
                    // ADR-0027 D3: `model`（行の値。空なら None）は PaperQA の設定ファイルより優先して `--llm` に渡す。
                    // `[adapters.paperqa]` にモデルの既定値は無い（acp と同じ理由: 行＝アカウント/エンドポイントごと）。
                    model: effective_model(&p.model, &None),
                    env: merged_env_with_secrets(
                        &base.env,
                        &base.env_from_secrets,
                        &p.env,
                        &p.env_from_secrets,
                        secrets_dir,
                    ),
                    extra_args: base.extra_args.clone(),
                    // ADR-0035 D1 / D3: 取得と証拠ゲートは行ごとの上書きが無い（他の paperqa 設定と同じ扱い）。
                    acquire: base.acquire.clone(),
                    evidence: base.evidence,
                    // ADR-0063 Phase 109d C3: `max_asks` も行ごとの上書きが無い。
                    max_asks: base.max_asks,
                    // ADR-0063 Phase 109g A: コンテナと同じ `[knowledge] root`（`ContainerPlan.knowledge_root`
                    // と同じ絶対パス）。`paperqa_ask.py` が比較先のページ本文をここから直接読む。
                    knowledge_root: Some(config.knowledge.root.clone()),
                }))
            }
            LdrAdapter::ID => {
                let base = &config.adapters.local_deep_research;
                // ADR-0029 D1: 行ごとの上書きは `model`（`settings` の `llm.model` を上書き）と `env` だけ
                // （`ProviderConfig.settings` は `paperqa` 専用フィールドなので LDR では再利用しない。
                // celeris 側の実装判断。行ごとに調査対象を変えたければ `[[roles]]`/`[[genres]]` で使い分ける）。
                let mut settings: Vec<(String, String)> = base
                    .settings
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                settings.sort();
                Arc::new(LdrAdapter::new(LdrConfig {
                    command: base.command.clone(),
                    mode: base.mode,
                    iterations: base.iterations,
                    questions_per_iteration: base.questions_per_iteration,
                    settings,
                    model: effective_model(&p.model, &None),
                    env: merged_env_with_secrets(
                        &base.env,
                        &base.env_from_secrets,
                        &p.env,
                        &p.env_from_secrets,
                        secrets_dir,
                    ),
                    // ADR-0031 D2: 証拠ゲートの閾値は行ごとの上書きが無い（他の LDR 設定と同じ扱い）。
                    evidence: base.evidence,
                    // ADR-0063 D2: 再挑戦時の mode / iterations も行ごとの上書きは無い。
                    retry_mode: base.retry_mode,
                    retry_iterations: base.retry_iterations,
                    // ADR-0063 Phase 109c B3: 構造化合成の有無も行ごとの上書きは無い。
                    structured_synthesis: base.structured_synthesis,
                }))
            }
            LangMemAdapter::ID => {
                let base = &config.adapters.langmem;
                let llm = &config.knowledge.langmem;
                // ADR-0047 D4: `[adapters.langmem]`（起動コマンド・無出力タイムアウト）と
                // `[knowledge.langmem]`（LLM の接続先）を合わせて 1 つのアダプタ設定にする。
                Arc::new(LangMemAdapter::new(LangMemConfig {
                    command: base.command.clone(),
                    idle_timeout_secs: base.idle_timeout_secs,
                    provider: llm.provider,
                    base_url: llm.base_url.clone(),
                    model: llm.model.clone(),
                    // ADR-0139 D2: proxy を指すなら proxy が照合する `[api]` のトークン。
                    api_key: config.langmem_api_key(),
                    env: merged_env_with_secrets(
                        &base.env,
                        &base.env_from_secrets,
                        &p.env,
                        &p.env_from_secrets,
                        secrets_dir,
                    ),
                }))
            }
            // `Config::validate` が fake / claude-code / codex / aider / acp / paperqa /
            // local-deep-research / langmem 以外を拒否している。
            _ => {
                let mut fake = FakeAdapter::new(config.adapters.fake.command.clone());
                fake.set_env(merged_env_with_secrets(
                    &config.adapters.fake.env,
                    &config.adapters.fake.env_from_secrets,
                    &p.env,
                    &p.env_from_secrets,
                    secrets_dir,
                ));
                Arc::new(fake)
            }
        };
        adapters.insert(
            p.id.clone(),
            Arc::new(task_worker::tiered::TieredAdapter {
                base: adapter,
                models: p.tier_models.clone(),
                account_id: p.account_id.clone(),
                credential_error: p.env_from_secrets.iter()
                    .find(|(key, id)| task_core::model_routing::CREDENTIAL_KEYS.contains(&key.as_str()) && resolve_secret(secrets_dir, id).is_none())
                    .map(|(_, id)| format!("credential reference {id} is missing or unreadable; configure it in Accounts")),
            }),
        );
    }
    adapters
}

/// ADR-0030 D3: `GET /secrets` の `used_by` を設定から導く（秘密 id → それを使っている adapter/provider の
/// env_from_secrets の一覧）。設定順・キー順で決定的に並べる。
pub fn secret_usage(config: &Config) -> HashMap<String, Vec<task_api::types::SecretUse>> {
    fn push(
        map: &mut HashMap<String, Vec<task_api::types::SecretUse>>,
        secret_id: &str,
        scope: &str,
        name: &str,
        env: &str,
    ) {
        map.entry(secret_id.to_string())
            .or_default()
            .push(task_api::types::SecretUse {
                scope: scope.to_string(),
                name: name.to_string(),
                env: env.to_string(),
            });
    }

    let mut map: HashMap<String, Vec<task_api::types::SecretUse>> = HashMap::new();
    let adapters: [(&str, &HashMap<String, String>); 8] = [
        (
            ClaudeCodeAdapter::ID,
            &config.adapters.claude_code.env_from_secrets,
        ),
        (CodexAdapter::ID, &config.adapters.codex.env_from_secrets),
        (AiderAdapter::ID, &config.adapters.aider.env_from_secrets),
        (FakeAdapter::ID, &config.adapters.fake.env_from_secrets),
        (AcpAdapter::ID, &config.adapters.acp.env_from_secrets),
        (
            PaperQaAdapter::ID,
            &config.adapters.paperqa.env_from_secrets,
        ),
        (
            LdrAdapter::ID,
            &config.adapters.local_deep_research.env_from_secrets,
        ),
        (
            LangMemAdapter::ID,
            &config.adapters.langmem.env_from_secrets,
        ),
    ];
    for (name, from_secrets) in adapters {
        let mut env_keys: Vec<&String> = from_secrets.keys().collect();
        env_keys.sort();
        for env_key in env_keys {
            push(&mut map, &from_secrets[env_key], "adapter", name, env_key);
        }
    }
    // ADR-0047 D4: `[knowledge.langmem].api_key_secret`（環境変数の写像ではなく 1 つの LLM 鍵）。
    if let Some(id) = &config.knowledge.langmem.api_key_secret {
        push(&mut map, id, "adapter", LangMemAdapter::ID, "api_key");
    }
    let mut providers: Vec<&config::ProviderConfig> = config.providers.iter().collect();
    providers.sort_by(|a, b| a.id.cmp(&b.id));
    for p in providers {
        let mut env_keys: Vec<&String> = p.env_from_secrets.keys().collect();
        env_keys.sort();
        for env_key in env_keys {
            push(
                &mut map,
                &p.env_from_secrets[env_key],
                "provider",
                &p.id,
                env_key,
            );
        }
    }
    map
}

/// `WorkerStarted.model` に記録する、プロバイダごとの実効モデル名（ADR-0012 D1）。
pub fn effective_models(config: &Config) -> HashMap<ProviderId, String> {
    config
        .providers
        .iter()
        .map(|p| {
            let adapter_model = match p.adapter.as_str() {
                ClaudeCodeAdapter::ID => config.adapters.claude_code.model.clone(),
                CodexAdapter::ID => config.adapters.codex.model.clone(),
                AiderAdapter::ID => config.adapters.aider.model.clone(),
                // ADR-0026 D3 / ADR-0027 D3: acp / paperqa には `[adapters.<種別>].model` が無い。
                // 行の `model` が空なら `None` になる。
                _ => None,
            };
            (
                p.id.clone(),
                effective_model(&p.model, &adapter_model).unwrap_or_default(),
            )
        })
        .collect()
}

/// ADR-0013 D4: デーモンのスナップショットに載せる `[[providers]]` の定義（`in_use` はディスパッチャが毎 tick 埋める）。
pub fn provider_lives(config: &Config) -> Vec<ProviderLive> {
    let models = effective_models(config);
    config
        .providers
        .iter()
        .map(|p| {
            let mut env_keys: Vec<String> = p.env.keys().cloned().collect();
            env_keys.sort();
            ProviderLive {
                kind: config.provider_kind(&p.id).unwrap_or_default(),
                llm_source: config.provider_llm_source(&p.id),
                credential_refs: task_core::model_routing::credential_refs(&p.env_from_secrets),
                tier_models: p.tier_models.clone(),
                account_id: p.account_id.clone(),
                id: p.id.clone(),
                adapter: p.adapter.clone(),
                tiers: p.tiers.clone(),
                concurrency: p.concurrency,
                model: models.get(&p.id).filter(|m| !m.is_empty()).cloned(),
                env_keys,
                in_use: 0,
                in_use_cos: 0,
                // ADR-0022 D2: 確認の記録は Dispatcher 側（SnapshotPublisher.provider_checks）が持つ。
                last_check: None,
                account_pool: p.account_pool,
            }
        })
        .collect()
}
