//! celeris: デーモン本体（DESIGN §3, §5.2, ADR-0005 D7）。設定読込、ログ初期化、tick ループ。
//! 判断ロジックは `task-dispatch` にあり、ここはループと配線だけ。このファイルは入口（`mod` 宣言・
//! エラーと終了条件の型・再公開）で、配線の本体は `daemon` の下にある（module map は `daemon.rs`）。

mod accounts_admin;
pub mod cache_server;
mod cluster_admin;
pub mod config;
pub mod control_path;
mod daemon;
/// ADR-0064 D3 / D5（Phase 110a）: 背景チェックポイントと定期バックアップ。
pub mod db_maintenance;
pub mod delivery;
pub mod doc_gardener;
/// ADR-0040 D4（Phase 47）: インスタンスの役割（active / standby / draining / verify）とライブ引き継ぎ。
pub mod instance;
pub mod knowledge_gc;
/// ADR-0047 D4（Phase 62）: 知識の自動メンテナンス（決定的なトリガと適用。LLM は `langmem` アダプタの中）。
pub mod knowledge_maint;
/// ADR-0037（Phase 39）: 人の判断が要るときだけ Discord に知らせる（判定は決定的、送信は spawn）。
pub mod notify;
/// ADR-0040 D6（Phase 48）: `[selfdeploy] releases_dir` を読む／`promote.sh` を起こす。
pub mod releases;
/// ADR-0033 D3（Phase 25）: 報告の圧縮（まとめの run を起こす決定的な判断）。
pub mod reports;

use task_api::ApiError;
use task_core::{DaemonMode, StoreError};
use task_dispatch::DispatchError;

pub use config::{Config, ConfigError, Overrides};
pub use daemon::adapters::{build_adapters, effective_models, provider_lives, secret_usage};
pub use daemon::api::{api_settings, bind_reuseport, config_view};
pub use daemon::bootstrap::{build_dispatcher, seed_org_if_empty};
pub use daemon::clusters::{ClusterMasters, wire_cluster_liveness_hooks};
pub use daemon::run::run;
pub use instance::InstanceIdentity;

pub(crate) use daemon::secrets::resolve_secret;

#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("dispatch: {0}")]
    Dispatch(#[from] DispatchError),
    #[error("api: {0}")]
    Api(#[from] ApiError),
}

/// ループの終了条件。
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// idle な tick で exit する（テスト・バッチ用）。
    pub until_idle: bool,
    /// tick 数の上限（0 = 無制限）。
    pub max_ticks: u64,
    /// ADR-0040 D3（Phase 47）: `--mode`。`verify` は本番のデータのコピーに対する検証専用
    /// （dispatch しない、ワーカーを起こさない、tick の裏方を動かさない、`daemon_instances` に書かない）。
    pub mode: DaemonMode,
    /// ADR-0040 D4: `--release <sha12>`。無ければ環境変数 `CELERIS_RELEASE`、それも無ければ `"dev"`。
    pub release: Option<String>,
}

/// ループ終了の理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Idle,
    MaxTicks,
    Signal,
    /// ADR-0040 D4: 引き継ぎのために `draining` になり、手元の run が 0 になった（または
    /// `[handoff] drain_timeout_secs` を超えて残りを abort した）。プロセスは exit 0 で終わる
    /// （systemd の `Restart=on-failure` では再起動されない）。
    Drained,
    /// ADR-0040 D4: 同じ `release` の `active` が既に動いていた。何もせず exit 3。
    DuplicateRelease,
}

#[cfg(test)]
#[path = "lib/tests.rs"]
mod tests;
