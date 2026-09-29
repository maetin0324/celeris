//! dispatcher に渡す設定: `[reviewer]`・`[review]`・`[dispatch]`・`[plan]`・`[sessions]` と `Config::dispatch_config`。

use std::time::Duration;

use serde::Deserialize;
use task_core::{Tier, WorkerHint};
use task_dispatch::{AccountsRuntimeConfig, DispatchConfig};

use super::{Config, ConfigError, ProviderConfig};

/// `[sessions]`（ADR-0054 D1。Phase 67）: CoS の対話・部門長のレビュー run の継続セッション
/// （`node_sessions`）の逼迫判定。`approx_tokens`（run の usage の累計）がこれを超えたら、次の run は
/// 新しいセッションから始める（要約を前置きに。`preamble::session_diff_section`）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionsConfig {
    #[serde(default = "default_rollover_tokens")]
    pub rollover_tokens: u64,
}

impl Default for SessionsConfig {
    fn default() -> Self {
        Self {
            rollover_tokens: default_rollover_tokens(),
        }
    }
}

fn default_rollover_tokens() -> u64 {
    400_000
}

/// `[reviewer]`（ADR-0010 D9, P-30）: `Check::Reviewer` の判定 run に使う adapter / tier。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewerConfig {
    /// 省略時は tier だけで選ぶ（設定表の優先順）。
    #[serde(default)]
    pub adapter: Option<String>,
    /// ADR-0069 Phase 118 D4: 明示すればそれが勝つ（部署の `[profile] review.tier` を除く）。省略時
    /// （既定 `None`）は、worker run の lane と同じにして組織の天井で丸める
    /// （`Dispatcher::pick_reviewer` が決める。`reviewer_hint.tier` の後方互換の既定値には
    /// 引き続き `default_reviewer_tier()` を使う）。
    #[serde(default)]
    pub tier: Option<Tier>,
}

fn default_reviewer_tier() -> Tier {
    Tier::Standard
}

/// `[review]`（ADR-0054 D2, Phase 113）: `[reviewer]`（判定 run の adapter/tier）とは別。
/// Reviewer run 自身のインフラ都合の失敗（is_error の結果・プロセス失敗・resume 拒否・分類できない
/// レート制限文言）で判定を保留し、reviewing のままやり直す回数の上限。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewConfig {
    #[serde(default = "default_max_reviewer_retries")]
    pub max_reviewer_retries: u32,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            max_reviewer_retries: default_max_reviewer_retries(),
        }
    }
}

fn default_max_reviewer_retries() -> u32 {
    3
}

/// `[dispatch]`（ADR-0070 D3, Phase 116）: ワーカー run 自身のインフラ都合の失敗を、attempts を
/// 消費せず再試行できる連続回数の上限。`[review]`（reviewer run 自身のインフラ失敗）とは別のテーブル
/// （対象がワーカー run か reviewer run かで役割が違うため）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchTomlConfig {
    #[serde(default = "default_max_infra_retries")]
    pub max_infra_retries: u32,
    #[serde(default = "default_min_free_disk_mb")]
    pub min_free_disk_mb: u64,
}

impl Default for DispatchTomlConfig {
    fn default() -> Self {
        Self {
            max_infra_retries: default_max_infra_retries(),
            min_free_disk_mb: default_min_free_disk_mb(),
        }
    }
}

fn default_max_infra_retries() -> u32 {
    5
}

fn default_min_free_disk_mb() -> u64 {
    5120
}

/// `[plan]`（DESIGN §4.2, ADR-0007 D7）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanConfig {
    /// Plan の子を親 `done` と同時に `ready` にする（true）か、人間の `celerisctl approve` を待つ（false、既定）か。
    #[serde(default)]
    pub auto_accept: bool,
}

impl Config {
    pub fn dispatch_config(&self) -> DispatchConfig {
        DispatchConfig {
            delivery: task_ops::delivery::DeliveryPolicy {
                projects: self.selfdeploy.delivery_projects.clone(),
                repo: self.selfdeploy.repo.clone(),
            },
            max_concurrency: self.max_concurrency,
            lease_grace: Duration::from_secs(self.lease_grace_secs),
            idle_timeout: Duration::from_secs(self.idle_timeout_secs),
            kill_grace: Duration::from_secs(self.kill_grace_secs),
            review_timeout: Duration::from_secs(self.review_timeout_secs),
            workspace_root: self.workspace_root.clone(),
            plan_auto_accept: self.plan.auto_accept,
            retry_backoff_base: Duration::from_secs(self.retry_backoff_base_secs),
            retry_backoff_max: Duration::from_secs(self.retry_backoff_max_secs),
            max_requeues: self.max_requeues,
            max_reviewer_retries: self.review.max_reviewer_retries,
            max_infra_retries: self.dispatch.max_infra_retries,
            min_free_disk_mb: self.dispatch.min_free_disk_mb,
            reviewer_hint: WorkerHint {
                // ADR-0069 Phase 118 D4: 他に何も分からないときの既定値（後方互換）。実際の reviewer
                // run の lane は `Dispatcher::pick_reviewer` が `reviewer_tier_override` /
                // `profile.review_tier` / worker lane から動的に決める。
                tier: self.reviewer.tier.unwrap_or_else(default_reviewer_tier),
                adapter: self.reviewer.adapter.clone(),
            },
            // ADR-0069 Phase 118 D4: `[reviewer] tier` が明示されているときだけ `Some`。
            reviewer_tier_override: self.reviewer.tier,
            clusters: self.cluster_specs(),
            // ADR-0018 D2: 多重接続が無いクラスタは、プロバイダの cooldown と同じ長さだけ外す。
            cluster_cooldown: Duration::from_secs(self.error_cooldown_secs),
            roles: self.role_specs(),
            genres: self.genre_specs(),
            delegation: self.delegation_limits(),
            accounts: self.accounts.as_ref().map(|a| AccountsRuntimeConfig {
                roots: a.roots(),
                max_runs_per_account: a.max_runs_per_account,
                check_model: a.check_model.clone(),
                fallback_cooldown_secs: self.error_cooldown_secs,
            }),
            // ADR-0033 D6: `[memory]` が無ければ記憶を読まないし書かない。
            memory_dir: self.memory.as_ref().map(|m| m.dir.clone()),
            // ADR-0047 D2（Phase 61）: 知識ベースの根と既定のマウント（前置きの索引を組むのに使う）。
            knowledge: task_dispatch::KnowledgeRuntimeConfig {
                root: self.knowledge.root.clone(),
                default_mounts: self.knowledge.mounts().unwrap_or_default(),
                // ADR-0052 D1（Phase 64）: dispatch の直前に `GET <base_url>/models` を当てる先。
                langmem_base_url: self.knowledge.langmem.base_url.clone(),
                // Phase 65b: probe の `Authorization: Bearer` に使う平文のトークン（`llm-proxy` の
                // ように `/v1/models` が認証を要求する上流を `[knowledge.langmem].base_url` に
                // 指したときのため）。`[secrets] dir` が無い・見つからないなら `None`（検査は従来どおり
                // トークン無しで行い、401/403 は `Unknown` として扱われる）。**値はここにしか無い**
                // （`build_adapters` の langmem アダプタと同じ解決。ログには出さない）。
                langmem_api_key: self
                    .knowledge
                    .langmem
                    .api_key_secret
                    .as_deref()
                    .and_then(|id| {
                        crate::resolve_secret(self.secrets.as_ref().map(|s| s.dir.as_path()), id)
                    }),
                // ADR-0052 D2: `knowledge` ハーネスの `fallback`（組み込みの既定は tier `cheap`）。
                fallback_tier: self
                    .harness_registry()
                    .get(task_core::BUILTIN_KNOWLEDGE)
                    .and_then(task_core::HarnessSpec::fallback_tier),
            },
            // ADR-0041 D1 / ADR-0042 D3: ローカルの worktree（既定 `celeris/`）。
            worktree_branch_prefix: self.workspace.worktree_branch_prefix.clone(),
            releases_dir: Some(self.selfdeploy.releases_dir.clone()),
            // ADR-0043 D3（Phase 56）: コンテナ実行。綴りは `validate()` が通してある。
            containers: task_dispatch::ContainersRuntimeConfig {
                preference: task_worker::RuntimePreference::parse(&self.containers.runtime)
                    .unwrap_or_default(),
                image_default: self.containers.image_default.clone(),
                build_dir: self.containers.build_dir.clone(),
                build_timeout: Duration::from_secs(self.containers.build_timeout_secs),
            },
            // ADR-0054 D1（Phase 67）: 継続セッションの逼迫判定。
            session_rollover_tokens: self.sessions.rollover_tokens,
            // ADR-0066 D1 / D2（Phase 110b）。
            shared_build_cache: self.workspace.shared_build_cache,
            build_cache_dir: self.workspace.build_cache_dir.clone(),
            workspace_prune_after_secs: self.workspace.prune_after_secs,
            // ADR-0075（Phase G1）: scratch pool（NFS 上なら無効化した理由つき）。
            scratch: self.scratch_settings(),
            // ADR-0072 D18（Phase E1）/ D13・D14（Phase E3）: continuation・gate・planner。
            execution: task_dispatch::ExecutionConfig {
                continuation: self.execution.continuation,
                max_continuations_per_work_unit: self.execution.max_continuations_per_work_unit,
                no_progress_limit: self.execution.no_progress_limit,
                gate: task_core::GateMode::parse(&self.execution.gate).unwrap_or_default(),
                planner: task_core::PlannerConfig {
                    adapter: self.execution.planner.adapter.clone(),
                    permission_mode: self.execution.planner.permission_mode.clone(),
                    tier: self.execution.planner.tier,
                    max_turns: self.execution.planner.max_turns,
                    max_wall_secs: self.execution.planner.max_wall_secs,
                },
                max_repairs: self.execution.max_repairs,
                max_repairs_per_class: self.execution.max_repairs_per_class,
                max_replans: self.execution.max_replans,
                work_unit_lane_cap: task_core::WorkUnitLaneCap::parse(
                    &self.execution.work_unit_lane_cap,
                )
                .unwrap_or_default(),
                parallel: self.execution.parallel,
                max_parallel_work_units: self.execution.max_parallel_work_units,
                // Phase F5-fix3: config.toml に欄は無い（ADR-0072 D18 / ADR-0074 §4 の既定のまま）。
                // ADR-0079 D3（Phase R1a）: `[execution.tree]` は plan/3 の検証だけに効く。
                limits: task_core::ExecutionLimits {
                    tree: self.execution.tree.limits(),
                    ..task_core::ExecutionLimits::default()
                },
            },
        }
    }
}

impl ReviewerConfig {
    /// ADR-0010 D9: Reviewer run を満たせるプロバイダが無い設定は、Reviewer 条件のタスクが無音で待ち続ける原因になる。
    /// ADR-0069 Phase 118 D4: `[reviewer] tier` が未設定なら lane はタスクごとに動的に決まる
    /// （worker run の lane に一致・組織の天井で丸め）ので、特定の 1 tier だけを検査する意味が無い。
    /// その場合は「（`adapter` 制約を満たす）プロバイダが 1 つ以上の tier を提供しているか」に緩める。
    pub(super) fn validate(&self, providers: &[ProviderConfig]) -> Result<(), ConfigError> {
        let reviewer = self;
        let reviewer_ok = match reviewer.tier {
            Some(explicit) => providers.iter().any(|p| {
                p.tiers.contains(&explicit)
                    && reviewer.adapter.as_deref().is_none_or(|a| p.adapter == a)
            }),
            None => providers.iter().any(|p| {
                !p.tiers.is_empty() && reviewer.adapter.as_deref().is_none_or(|a| p.adapter == a)
            }),
        };
        if !reviewer_ok {
            let adapter_suffix = reviewer
                .adapter
                .as_deref()
                .map(|a| format!(" with adapter {a:?}"))
                .unwrap_or_default();
            return Err(ConfigError::Invalid(match reviewer.tier {
                Some(t) => format!(
                    "[reviewer] no provider offers tier {t:?}{adapter_suffix} for reviewer runs"
                ),
                None => format!(
                    "[reviewer] no provider offers any tier{adapter_suffix} for reviewer runs"
                ),
            }));
        }
        Ok(())
    }
}
